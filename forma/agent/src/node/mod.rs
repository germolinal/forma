use crate::callable::{CallContext, Identifiable};
use crate::eval_agent::{Evaluatable, EvaluatableLLM, EvaluatableSet, SingleEvaluatable};
use crate::template::{is_valid_variable_name, render_template};
use crate::tool::workspace::attach::resolve_workspace_file;
use crate::{Eval, ToolRunner, Workflow};
use crate::{Tool, callable::Callable};
use async_trait::async_trait;

use futures::StreamExt;
use futures::future::join_all;
use genai_client::openai::v1::{
    OpenAIFinishReason, OpenAIFunctionToolChoice, OpenAIMessagePart, OpenAITextPart,
    OpenAIToolChoice, OpenAIUsage, ToolCallAccumulator,
};
use genai_client::{
    AssistantMessage, ChatResponse, LLMChunk, LLMFinishReason, LLMMessage, MessageChoice,
};
use genai_client::{GenaiClient, GenaiClientTrait, LLMResponse, SSEvent};

use protocols::{
    FormaDB, FormaError, FormaMessage, FormaMessageBody, FormaMessagePart, FormaTextPart,
    FormaWorkspaceFilePart, PendingToolCall, Session, from_provider_parts,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use tokio::task::JoinHandle;
use tracing::{Instrument, Span, info, span, warn};

/// Takes an [`OpenAIToolChoice`] and flattens it into a `(String, String, serde_json::Value)` tuple,
/// corresponding to `tool_call_id`, `function_name`, and `arguments`.
fn flatten_tool_choice(
    tool_calls: &OpenAIToolChoice,
) -> Result<(String, String, serde_json::Value), FormaError> {
    let v = match tool_calls {
        OpenAIToolChoice::Function { id, function } => {
            let v = serde_json::from_str(&function.arguments)?;
            (id.clone(), function.name.clone(), v)
        }
        OpenAIToolChoice::Custom { id, function } => {
            let v = serde_json::from_str(&function.input)?;
            (id.clone(), function.name.clone(), v)
        }
    };
    Ok(v)
}

/// An LLM call's span, following the OpenTelemetry GenAI conventions for
/// inference: named `chat {model}` (through `otel.name`) once
/// [`Node::init_span`] knows the model. `$name` (`triage`, `summarize`) is
/// Forma's step, kept as `forma.step`.
macro_rules! llm_span {
    ($name:expr) => {{
        tracing::span!(
            tracing::Level::INFO,
            $name,
            otel.name = tracing::field::Empty,
            forma.step = $name,
            node_id = tracing::field::Empty,
            user.id = tracing::field::Empty,
            gen_ai.operation.name = tracing::field::Empty,
            gen_ai.provider.name = tracing::field::Empty,
            gen_ai.agent.id = tracing::field::Empty,
            gen_ai.conversation.id = tracing::field::Empty,
            gen_ai.request.model = tracing::field::Empty,
            gen_ai.request.frequency_penalty = tracing::field::Empty,
            gen_ai.request.max_tokens = tracing::field::Empty,
            gen_ai.request.presence_penalty = tracing::field::Empty,
            gen_ai.request.temperature = tracing::field::Empty,
            gen_ai.request.top_p = tracing::field::Empty,
            gen_ai.response.finish_reasons = tracing::field::Empty,
            gen_ai.response.id = tracing::field::Empty,
            gen_ai.response.model = tracing::field::Empty,
            gen_ai.usage.input_tokens = tracing::field::Empty,
            gen_ai.usage.output_tokens = tracing::field::Empty,
            server.address = tracing::field::Empty,
            // Content: recorded only when it is opted into
            // (`genai_client::telemetry::capture_content`).
            gen_ai.input.messages = tracing::field::Empty,
            gen_ai.output.messages = tracing::field::Empty,
            gen_ai.system_instructions = tracing::field::Empty,
        )
    }};
}

pub fn default_node_id() -> String {
    // Make sure it does not start with a number
    let ret = uuid::Uuid::new_v4().to_string();
    let tail = &ret[1..];
    let ret = format!("n{tail}");
    ret
}

pub fn default_max_iterations() -> usize {
    10
}

pub fn default_system_prompt() -> String {
    "You are a helpful assistant".to_string()
}

/// A Node is the key of Forma Agents' execution. They are the
/// ones that call LLMs, process their response, and decide whether
/// tools should be called. It always does the same thing:
///
/// 1. **Triage** – an LLM reads the whole conversation and decides what to do next: reply right away, or call tools.
/// 2. **Tools (optional)** – the node runs the tools the LLM chose, and triage runs again with their results. This repeats until triage replies without calling a tool, and that reply is the node's answer, or until `max_iterations` rounds of tools have run.
/// 3. **Summarisation (only at the limit)** – if `max_iterations` runs out while the LLM is still calling tools, the node calls an LLM once more, without tools. It is told it reached the limit, so it says what it found and what is missing, and asks the user, in the language of the conversation, whether to go on. A simple yes starts a new turn. `summarization_prompt` and `summarization_llm` shape this call.
///
/// **Load time checks**
///
/// - ID is well-formed
/// - Tools are all well formed
/// - It does not have output format AND tools simultaneously
/// - Does not have summarization prompt or summarization LLM if it does not have tools
#[derive(Clone, Serialize, Debug, Deserialize)]
pub struct Node {
    /// The ID of the node. If absent, one will be provided.
    /// The value given here is used to identify the output
    /// of this node in subsequent nodes within a workflow,
    /// allowing for template interpolation
    #[serde(default = "default_node_id")]
    pub id: String,

    /// The main LLM and settings, used for triage and, unless
    /// `summarization_llm` is given, for summarisation.
    #[serde(default)]
    pub llm: GenaiClient,

    /// The main instruction given to the LLM.
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,

    /// An optional LLM for summarisation, the reply written when
    /// `max_iterations` runs out. If absent, the main `llm` is used.
    pub summarization_llm: GenaiClient,

    /// An optional instruction for summarisation, the reply written when
    /// `max_iterations` runs out while the LLM is still calling tools. If
    /// absent, the `system_prompt` is used. Either way, the node adds a note
    /// saying the limit was reached and the user should be asked whether to
    /// go on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarization_prompt: Option<String>,

    /// The tools that the LLM can decide to call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// The evaluations used to test this specific node as a
    /// whole (different from Triage- and Summarize-specific evals)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub evals: Vec<Eval>,

    /// The evaluations used to test the triage LLM specifically
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub triage_evals: Vec<Eval>,

    /// The evaluations used to test the summarize LLM specifically
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub summarize_evals: Vec<Eval>,

    /// The maximum number of rounds of tool calls in one turn. When they run
    /// out, the node summarises instead of calling more tools.
    ///
    /// Defaults to 10.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// Whether to add tool results to the summarization prompt.
    #[serde(default)]
    pub add_tool_results_in_summarization_prompt: bool,

    /// Whether to stream tokens to the user as they are generated.
    ///
    /// When `false` (the default), the node generates the whole
    /// response before sending it to the user. When `true`, tokens
    /// are streamed as they arrive.
    #[serde(default)]
    pub stream: bool,
}

impl Default for Node {
    fn default() -> Self {
        Self {
            id: default_node_id(),
            llm: GenaiClient::default(),
            system_prompt: default_system_prompt(),
            summarization_llm: GenaiClient::default(),
            summarization_prompt: None,
            tools: vec![],
            evals: vec![],
            triage_evals: vec![],
            summarize_evals: vec![],
            max_iterations: default_max_iterations(),
            add_tool_results_in_summarization_prompt: false,
            stream: false,
        }
    }
}

/// Turns the conversation into what a provider is sent.
///
/// The one place a message leaves Forma's own shape, and so the one place a
/// part that names something instead of carrying it — a workspace file — is
/// fetched. Anything left unresolved is refused by the conversion itself
/// rather than reaching a provider.
///
/// An attached file is hoisted out of the tool result it arrived on and into a
/// user message of its own, because a tool result may only carry text on most
/// providers. That message goes after the last of the results it arrived
/// among: when calls ran in parallel, their results must stay together. Nothing of that is stored: the conversation keeps the reference
/// where it belongs, on the tool call that asked for the file.
/// How much attached file may ride along in one request, as sent (base64, so
/// about a third larger than the files themselves).
///
/// Each attachment is capped on its own where it is fetched; this caps the
/// conversation. Without it a chat that attaches a few documents quietly grows
/// a request no provider will take, and the refusal arrives far from the turn
/// that caused it.
const MAX_REQUEST_ATTACHMENT_BYTES: usize = 24 * 1024 * 1024;

/// What happens to one reference while a request is being built.
#[derive(Debug, PartialEq)]
enum Admit {
    /// Send the file here.
    Carry,
    /// The same file is attached further down; it is sent there instead.
    Duplicate,
    /// There is no room left in this request for it.
    TooMuch,
}

/// Decides which attachments a request carries, walking the conversation from
/// its end so the newest win both the budget and the duplicates.
#[derive(Debug)]
struct AttachBudget {
    seen: std::collections::HashSet<(String, String)>,
    left: usize,
}

impl AttachBudget {
    fn new(left: usize) -> Self {
        Self {
            seen: std::collections::HashSet::new(),
            left,
        }
    }

    fn admit(&mut self, key: &(String, String), size: usize) -> Admit {
        if self.seen.contains(key) {
            return Admit::Duplicate;
        }
        if size > self.left {
            return Admit::TooMuch;
        }
        self.left -= size;
        self.seen.insert(key.clone());
        Admit::Carry
    }
}

/// How much of a request one part takes up.
fn payload_len(part: &FormaMessagePart) -> usize {
    match part {
        FormaMessagePart::ImageUrl(p) => p.image_url.url.len(),
        FormaMessagePart::File(p) => p.file.file_data.as_ref().map_or(0, String::len),
        FormaMessagePart::Text(p) => p.text.len(),
        _ => 0,
    }
}

/// Turns the conversation into what a provider is sent.
///
/// The one place a message leaves Forma's own shape, and so the one place a
/// part that names something instead of carrying it — a workspace file — is
/// fetched. Anything left unresolved is refused by the conversion itself
/// rather than reaching a provider.
///
/// An attached file is hoisted out of the tool result it arrived on and into a
/// user message of its own, because a tool result may only carry text on most
/// providers. That message goes after the last of the results it arrived
/// among: when calls ran in parallel, their results must stay together. Nothing of that is stored: the conversation keeps the reference
/// where it belongs, on the tool call that asked for the file.
///
/// Where a file cannot be sent — the same one twice, or more than one request
/// can hold — a line of text takes its place, so the model can see that
/// something is missing and ask for it again.
async fn for_provider<DB: FormaDB>(
    mut messages: Vec<FormaMessage>,
    tools: &[Tool],
    ctx: &CallContext<DB>,
) -> Result<Vec<LLMMessage>, FormaError> {
    // Every file carried inline is named for the model, so it can point a
    // tool at one (`from_conversation`) instead of having to repeat its bytes.
    crate::tool::workspace::conversation::label(&mut messages);

    // Where the references are, in the order they appear.
    let mut references = Vec::new();
    for (mi, message) in messages.iter().enumerate() {
        for (pi, part) in message.content.content().iter().enumerate() {
            if let FormaMessagePart::WorkspaceFile(reference) = part {
                references.push(((mi, pi), reference.clone()));
            }
        }
    }
    if references.is_empty() {
        return messages.into_iter().map(LLMMessage::try_from).collect();
    }

    // Decided from the end of the conversation backwards: the most recent
    // mention of a file is the one worth carrying, and the one closest to the
    // question being asked.
    let mut budget = AttachBudget::new(MAX_REQUEST_ATTACHMENT_BYTES);
    let mut fetched: HashMap<(String, String), FormaMessagePart> = HashMap::new();
    let mut carried: HashMap<(usize, usize), FormaMessagePart> = HashMap::new();
    let mut notes: HashMap<(usize, usize), String> = HashMap::new();

    for (position, reference) in references.iter().rev() {
        let key = (reference.workspace_id.clone(), reference.path.clone());
        let file = match fetched.get(&key) {
            Some(file) => file.clone(),
            None => {
                let file = resolve_workspace_file(reference, tools, ctx).await?;
                fetched.insert(key.clone(), file.clone());
                file
            }
        };
        let note = match budget.admit(&key, payload_len(&file)) {
            Admit::Carry => {
                carried.insert(*position, file);
                format!("[{} follows]", reference.path)
            }
            Admit::Duplicate => format!(
                "[{} is attached further down in this conversation]",
                reference.path
            ),
            Admit::TooMuch => format!(
                "[{} was left out: this conversation already carries as much as one \
                 request can hold. Read it again once something else is no longer \
                 needed.]",
                reference.path
            ),
        };
        notes.insert(*position, note);
    }

    let mut resolved = Vec::with_capacity(messages.len());
    for (mi, mut message) in messages.drain(..).enumerate() {
        let mut attached = Vec::new();
        for (pi, part) in message.content.content_mut().iter_mut().enumerate() {
            if !matches!(part, FormaMessagePart::WorkspaceFile(_)) {
                continue;
            }
            // What the model reads in place of the file: a tool result that
            // said nothing would be an odd answer to a call that worked.
            *part = FormaMessagePart::Text(FormaTextPart {
                text: notes.remove(&(mi, pi)).unwrap_or_default(),
            });
            if let Some(file) = carried.remove(&(mi, pi)) {
                attached.push(file);
            }
        }
        resolved.push((message, attached));
    }
    with_attachments(resolved)
        .into_iter()
        .map(LLMMessage::try_from)
        .collect()
}

/// The messages, each followed by the files hoisted out of it — but only once
/// the run of tool results it belongs to has ended. The results of one turn's
/// calls must follow each other with nothing in between, or Gemini refuses
/// the request; so when calls ran in parallel, their files go together, after
/// the last result.
fn with_attachments(messages: Vec<(FormaMessage, Vec<FormaMessagePart>)>) -> Vec<FormaMessage> {
    let mut out = Vec::with_capacity(messages.len());
    let mut waiting = Vec::new();
    for (message, attached) in messages {
        let is_result = matches!(message.content, FormaMessageBody::Tool { .. });
        if !is_result && !waiting.is_empty() {
            out.push(FormaMessage::new_user_with_parts(std::mem::take(
                &mut waiting,
            )));
        }
        out.push(message);
        waiting.extend(attached);
    }
    if !waiting.is_empty() {
        out.push(FormaMessage::new_user_with_parts(waiting));
    }
    out
}

/// The file a tool says it attached, if it says so.
///
/// A knowledgebase read tool reports what it put in front of the model
/// under `attached`; that becomes a part naming the file, which is fetched when
/// a request is built. This is the only shape the convention takes, and
/// [`crate::tool::workspace`] is its only producer.
fn attached_file(output: &Value) -> Option<FormaWorkspaceFilePart> {
    let attached = output.get("attached")?;
    Some(FormaWorkspaceFilePart {
        workspace_id: attached.get("workspace")?.as_str()?.to_string(),
        path: attached.get("path")?.as_str()?.to_string(),
        media_type: attached
            .get("media_type")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

async fn push_in_memory_tool_result<DB: FormaDB>(
    tool_call_id: &str,
    output: &Value,
    ctx: &CallContext<DB>,
) -> Result<(), FormaError> {
    let content_str = serde_json::to_string(&output).unwrap_or_default();
    let mut content = vec![FormaMessagePart::Text(FormaTextPart { text: content_str })];
    if let Some(reference) = attached_file(output) {
        content.push(FormaMessagePart::WorkspaceFile(reference));
    }
    let tool_result_msg = FormaMessage {
        content: FormaMessageBody::Tool {
            tool_call_id: tool_call_id.to_string(),
            content,
        },
        ..Default::default()
    };
    ctx.session.write().await.messages.push(tool_result_msg);
    Ok(())
}

/// `messages` without what a history window cut in half at its start: tool
/// results whose call was left out of it, and calls whose results would come
/// first.
///
/// A session is read back as its last few messages (`max_history_messages`),
/// and nothing makes that window start where a turn does. When it starts on a
/// tool result, the provider refuses the whole conversation: OpenAI and Gemini
/// both require a result to follow the call it answers, and Gemini a call to
/// follow the user. So the history starts at the first message that is
/// neither — or, when there is none, at the first that is not a result.
fn whole_calls(messages: &[FormaMessage]) -> &[FormaMessage] {
    let is_result = |m: &FormaMessage| matches!(m.content, FormaMessageBody::Tool { .. });
    let is_call = |m: &FormaMessage| matches!(&m.content, FormaMessageBody::Assistant { tool_calls, .. } if !tool_calls.is_empty());
    let start = messages
        .iter()
        .position(|m| !is_result(m) && !is_call(m))
        .or_else(|| messages.iter().position(|m| !is_result(m)))
        .unwrap_or(messages.len());
    &messages[start..]
}

/// The IDs of the tool calls that already have a result in `messages`.
fn answered_tool_calls(messages: &[FormaMessage]) -> HashSet<String> {
    messages
        .iter()
        .filter_map(|m| match &m.content {
            FormaMessageBody::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

async fn session_data<DB: FormaDB>(ctx: &CallContext<DB>) -> (String, String, String) {
    let a = ctx.session.read().await;
    let session_id = a.id_string().expect("session has no id");
    (session_id, a.agent_id.clone(), a.user_id.clone())
}

impl Node {
    /// Gets the key that will be used to store the result in the state.
    fn state_key(&self, suffix: &str) -> String {
        format!("{}-{}", self.id, suffix)
    }

    async fn call_tool<DB: FormaDB>(
        tool: Tool,
        call: OpenAIToolChoice,
        is_root: bool,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let (tool_call_id, tool_name, arguments) = flatten_tool_choice(&call)?;
        if let Some(output) = tool.cached_result(&ctx, &tool_call_id).await {
            // Answered while the session was suspended: by the client (a
            // delegated tool's result) or by the user (a rejection). Calls
            // whose results are already in the history never get here (see
            // `handle_tool_calls`), so this is the first time the
            // conversation sees it.
            info!("tool {} is already resolved", &tool_name);
            push_in_memory_tool_result(&tool_call_id, &output, &ctx).await?;
            Self::emit_tool_outputs(&ctx, &tool_call_id, &output)?;
            return Ok(Some(output));
        }
        let (session_id, agent_id, user_id) = session_data(&ctx).await;
        // An `execute_tool` span, in the OpenTelemetry GenAI conventions. The
        // arguments and the result are content: recorded only when opted into.
        let capture = genai_client::telemetry::capture_content();
        let span = tracing::span!(
            tracing::Level::INFO,
            "call_tool",
            otel.name = format!("execute_tool {tool_name}"),
            gen_ai.operation.name = "execute_tool",
            gen_ai.tool.name = &tool_name,
            gen_ai.tool.call.id = &tool_call_id,
            gen_ai.agent.id = agent_id,
            gen_ai.conversation.id = &session_id,
            user.id = user_id,
            error.type = tracing::field::Empty,
            gen_ai.tool.call.arguments = tracing::field::Empty,
            gen_ai.tool.call.result = tracing::field::Empty,
        );
        if capture {
            span.record("gen_ai.tool.call.arguments", arguments.to_string());
        }

        // Run the tool. A tool *execution* failure is reported back to the model
        // as a tool result (so it can recover or explain to the user) instead of
        // aborting the whole run — mirroring how a user-rejected call is handled.
        // This also guarantees every tool_call has a matching tool result, which
        // the chat APIs require.
        let output = match tool
            .run(arguments.clone(), ctx.clone())
            .instrument(span.clone())
            .await
        {
            // output == None means the tool is pending
            Ok(None) => return Ok(None),
            Ok(Some(output)) => {
                if capture {
                    span.record("gen_ai.tool.call.result", output.to_string());
                }
                output
            }
            Err(err) => {
                span.record("error.type", "tool_error");
                warn!(
                    tool = %tool_name,
                    error = %err,
                    "tool execution failed; reporting error to the model"
                );
                serde_json::json!({ "error": err.to_string() })
            }
        };

        // Push tool result into the in-memory session so summarisation sees it
        push_in_memory_tool_result(&tool_call_id, &output, &ctx).await?;

        // Inform step and persist
        tool.register_result(&ctx, &tool_call_id, &output, is_root)
            .await?;

        Self::emit_tool_outputs(&ctx, &tool_call_id, &output)?;

        Ok(Some(output))
    }

    /// Completes a span, populating the values of everything that can only be known
    /// after generating an answer (e.g., token usage)
    fn complete_span(&self, span: &Span, response: &LLMResponse) {
        if span.is_disabled() {
            return;
        }
        span.record("gen_ai.usage.input_tokens", response.usage.prompt_tokens);
        span.record(
            "gen_ai.usage.output_tokens",
            response.usage.completion_tokens,
        );
        span.record("gen_ai.response.model", &response.model);
        span.record("gen_ai.response.id", &response.id);
        let finish_reason = response.choices.first().and_then(|c| {
            serde_json::to_value(&c.finish_reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
        });
        if let Some(reason) = &finish_reason {
            // A string[] in the conventions; tracing records it as JSON.
            span.record(
                "gen_ai.response.finish_reasons",
                serde_json::json!([reason]).to_string(),
            );
        }
        if genai_client::telemetry::capture_content()
            && let Some(genai_client::LLMMessage::Assistant(outmsg)) = response.message()
        {
            span.record(
                "gen_ai.output.messages",
                genai_client::telemetry::output_messages(outmsg, finish_reason.as_deref()),
            );
        }
    }

    /// Reports what a provider call consumed, for billing.
    ///
    /// Called next to every [`Self::complete_span`], which is the one thing that
    /// happens after every LLM response and only after a real one — a cached
    /// result goes through `complete_cached_span` instead and correctly reports
    /// nothing, because no provider was called.
    ///
    /// Deliberately not gated on `is_root`: an inner node's tokens cost the same
    /// as the root's, and `is_root` only decides what the *user* sees.
    ///
    /// `llm` is passed rather than assumed to be `self.llm` because a node has
    /// two clients — `llm` and `summarization_llm` — which may be different
    /// models, and attributing a summarisation to the triage model would
    /// misprice it.
    fn emit_usage<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        llm: &GenaiClient,
        response: &LLMResponse,
    ) -> Result<(), FormaError> {
        // Prefer the model the provider echoed back; fall back to the configured
        // one when a provider omits it, so the record is never unattributable.
        let model = if response.model.is_empty() {
            llm.model().to_string()
        } else {
            response.model.clone()
        };
        ctx.tx.send(SSEvent::Usage {
            node_id: self.id.clone(),
            model,
            provider: llm.otel_provider().to_string(),
            usage: (&response.usage).into(),
        })?;
        Ok(())
    }

    /// Completes a span, populating the values of everything that can only be known
    /// after generating an answer (e.g., token usage)
    fn complete_cached_span(&self, span: &Span, message: &AssistantMessage) {
        if span.is_disabled() || !genai_client::telemetry::capture_content() {
            return;
        }
        span.record(
            "gen_ai.output.messages",
            genai_client::telemetry::output_messages(message, None),
        );
    }

    /// Populates span values, adding everything that can be known
    /// before generating an answer.
    async fn init_span(
        &self,
        llm: &GenaiClient,
        span: &Span,
        session: &Session,
        messages: &[FormaMessage],
    ) {
        if span.is_disabled() {
            return;
        }
        let (session_id, agent_id, user_id) = {
            let a = session.read().await;
            let session_id = a.id_string().expect("session has no id");
            (session_id, a.agent_id.clone(), a.user_id.clone())
        };

        span.record("otel.name", format!("chat {}", llm.model()));
        span.record("gen_ai.operation.name", "chat");
        span.record("gen_ai.provider.name", llm.otel_provider());
        span.record("gen_ai.request.model", llm.model());
        span.record("gen_ai.conversation.id", &session_id);
        span.record("gen_ai.agent.id", agent_id);
        span.record("user.id", user_id);
        span.record("node_id", &self.id);
        span.record("gen_ai.request.frequency_penalty", llm.frequency_penalty());
        span.record("gen_ai.request.max_tokens", llm.max_tokens());
        span.record("gen_ai.request.presence_penalty", llm.presence_penalty());
        span.record("gen_ai.request.temperature", llm.temperature());
        span.record("gen_ai.request.top_p", llm.top_p());
        span.record("server.address", llm.chat_endpoint().unwrap());

        // The conversation, once, and only when content is opted into: it is
        // what makes a span large.
        if genai_client::telemetry::capture_content() {
            let (system, input) = protocols::message::telemetry_input(messages);
            if let Some(system) = system {
                span.record("gen_ai.system_instructions", system);
            }
            span.record("gen_ai.input.messages", input);
        }
    }

    /// Borrows the ID of the node.
    pub fn get_id(&self) -> &String {
        &self.id
    }

    /// Finds a tool within this specific node
    fn find_tool(&self, tool_name: &str) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.name == tool_name)
    }

    /// Returns all the messages in the session, prepended by the
    /// system instructions which will be rendered from the `template`,
    /// using the values within the `session`'s state.
    async fn get_messages_history(
        &self,
        template: &str,
        session: Session,
    ) -> Result<Vec<FormaMessage>, String> {
        // Get reading lock
        let session = session.read().await;
        let template = render_template(template, &session.variables)?;
        let history = whole_calls(&session.messages);
        let mut messages = Vec::with_capacity(history.len() + 1);
        // system prompt + messages
        messages.push(FormaMessage::new_system_with_content(template));
        messages.extend_from_slice(history);
        Ok(messages)
    }

    async fn summarise_tool_results<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        is_root: bool,
    ) -> Result<LLMResponse, FormaError> {
        let session = ctx.session.clone();
        let the_span = llm_span!("summarize");
        let span = the_span.clone();
        async move {
            info!(node=%self.id, "summarizing results");
            let mut sum_template = match &self.summarization_prompt {
                Some(template) => template.clone(),
                None => self.system_prompt.clone(),
            };
            if self.add_tool_results_in_summarization_prompt {
                // Add tool results to string
                sum_template.push_str("\n\n<tool-generated-information>\n");
                for (tool_id, output) in &session.read().await.tool_calls_results {
                    let output = serde_json::to_string_pretty(output).unwrap_or_default();
                    sum_template
                        .push_str(format!("<tool id=\"{tool_id}\" format=\"json\">\n").as_str());
                    sum_template.push_str(output.as_str());
                    sum_template.push_str("\n</tool>\n");
                }
                sum_template.push_str("</tool-generated-information>\n");
            }
            // This step only runs when `max_iterations` ran out with the
            // agent still calling tools, and its request carries no tools.
            sum_template.push_str(ITERATION_LIMIT_NOTE);

            let messages = self
                .get_messages_history(&sum_template, session.clone())
                .await?;

            let sum_llm = &self.summarization_llm;
            self.init_span(sum_llm, &span, &session, &messages).await;

            // Summarisation never has tools, so its response is always
            // text-only and safe to stream.
            let ret = if self.stream {
                self.stream_generate(
                    sum_llm,
                    messages,
                    Some(vec![]), // remove all tools.
                    ctx,
                    ctx.job_id.as_ref(),
                    is_root,
                )
                .await?
            } else {
                sum_llm
                    .generate(
                        for_provider(messages, &self.tools, ctx).await?,
                        Some(vec![]), // remove all tools.
                    )
                    .await?
            };
            self.complete_span(&span, &ret);
            self.emit_usage(ctx, sum_llm, &ret)?;
            Ok(ret)
        }
        .instrument(the_span)
        .await
    }

    /// The first step of the Agentic node's process: takes the
    /// session, and decides which—if any—tool to call.
    async fn triage<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        iteration: usize,
        is_root: bool,
    ) -> Result<AssistantMessage, FormaError> {
        let session = &ctx.session;
        let messages = self
            .get_messages_history(&self.system_prompt, session.clone())
            .await?;
        let span = llm_span!("triage");
        self.init_span(&self.llm, &span, session, &messages).await;

        // Instrumented rather than entered: a guard held across `.await`
        // stays entered on whichever thread the task was on, where every
        // other task would then nest under it, and the span never closes.
        async {
            let key = format!("triage-{iteration}");
            let key = self.state_key(&key);

            // Fast forward, if we have solved this already
            if ctx.is_cached(&key).await {
                info!(node=%self.id, "returning cached triage result");
                match ctx.cached_result(&key).await {
                    Some(v) => {
                        self.complete_cached_span(&span, &v);
                        return Ok(v);
                    }
                    None => unreachable!("we just checked that there is a result"),
                };
            }
            info!(node=%self.id, "triage result not cached... generating result");
            if iteration == 0 {
                ctx.tx.send(SSEvent::StartStep {
                    id: self.id.clone(),
                })?;
            }

            let allowed_tools = CallContext::<DB>::allowed_tools(&ctx.user_roles, &self.tools);

            // Stream tokens live when the caller opted in and this is the root
            // node. Any tool calls the model makes are accumulated from the stream
            // and reconstructed into the response (see `StreamedResponse`), while
            // only the text is emitted to the user as it arrives.
            let should_stream = self.stream && is_root;

            let mut retries = 0;
            let response = loop {
                let response = if should_stream {
                    self.stream_generate(
                        &self.llm,
                        messages.clone(),
                        Some(allowed_tools.clone()),
                        ctx,
                        ctx.job_id.as_ref(),
                        is_root,
                    )
                    .await?
                } else {
                    self.llm
                        .generate(
                            for_provider(messages.clone(), &self.tools, ctx).await?,
                            Some(allowed_tools.clone()),
                        )
                        .await?
                };
                self.complete_span(&span, &response);
                // Every attempt was paid for, so every attempt is reported.
                self.emit_usage(ctx, &self.llm, &response)?;

                let Some(mistake) = self.model_mistake(&response, &ctx.user_roles) else {
                    break response;
                };
                // Text already streamed to the user cannot be taken back: asking
                // again would show it twice.
                let shown = should_stream && has_text(&response);
                if retries == MODEL_MISTAKE_RETRIES || shown {
                    warn!(node = %self.id, model = self.llm.model(), %mistake, retries, shown,
                        "the model got its turn wrong; not asking again");
                    break response;
                }
                retries += 1;
                warn!(node = %self.id, model = self.llm.model(), %mistake, retries,
                    "the model got its turn wrong; asking again");
            };

            check_triage_result(&response)?;

            let response = &response.choices[0];
            log_finish_reason(&response.finish_reason)?;

            if let LLMMessage::Assistant(msg) = &response.message {
                ctx.cache_result(&key, msg).await?;
                // Emit the response, unless we already streamed the tokens live.
                if is_root && !should_stream {
                    self.emit_triage_response(&msg.content, ctx).await?;
                }

                Ok(msg.clone())
            } else {
                unreachable!("no tools or content returned")
            }
        }
        .instrument(span.clone())
        .await
    }

    async fn emit_triage_response<DB: FormaDB>(
        &self,
        content: &[OpenAIMessagePart],
        ctx: &CallContext<DB>,
    ) -> Result<(), FormaError> {
        if content.is_empty() {
            return Ok(());
        }
        let content: Vec<String> = content
            .iter()
            .filter_map(|c| {
                let s: String = c.into();
                if s.is_empty() { None } else { Some(s) }
            })
            .collect();
        // A turn with only tool calls may come with empty text parts: opening
        // a text block for them would store an empty message.
        if content.is_empty() {
            return Ok(());
        }

        ctx.tx.send(SSEvent::TextStart {
            id: ctx.job_id.to_string(),
        })?;

        for x in content.iter() {
            let s: String = x.into();
            ctx.tx.send(SSEvent::TextDelta {
                id: ctx.job_id.to_string(),
                delta: s.clone(),
            })?;
        }

        ctx.tx.send(SSEvent::TextEnd {
            id: ctx.job_id.to_string(),
        })?;

        Ok(())
    }

    /// Streams the LLM response to the user token-by-token, emitting
    /// [`SSEvent::TextDelta`] events as text chunks arrive, and reconstructs
    /// the full [`LLMResponse`] from the accumulated chunks so the rest of the
    /// pipeline (caching, spans, variable registration) is unchanged.
    ///
    /// Any tool calls the model makes are accumulated silently (their argument
    /// fragments are merged, see [`StreamedResponse`]) and surfaced later via
    /// the usual tool-call handling; only text is emitted here. The
    /// [`SSEvent::TextStart`]/[`SSEvent::TextEnd`] pair is emitted lazily, so a
    /// turn that produces only tool calls does not emit an empty text block.
    async fn stream_generate<DB: FormaDB>(
        &self,
        llm: &GenaiClient,
        messages: Vec<FormaMessage>,
        allowed_tools: Option<Vec<String>>,
        ctx: &CallContext<DB>,
        emit_id: &str,
        is_root: bool,
    ) -> Result<LLMResponse, FormaError> {
        let mut stream = llm
            .stream(
                for_provider(messages, &self.tools, ctx).await?,
                allowed_tools,
            )
            .await
            .map_err(|e| FormaError::from(format!("streaming request failed: {e}")))?;

        let mut acc = StreamedResponse::default();
        let mut text_started = false;
        while let Some(chunk) = stream.next().await {
            let delta = acc.push(chunk);

            // Documents are announced whole and out of band of the text
            // block, so a turn that returns only an image still delivers it.
            if is_root {
                for part in delta.files {
                    ctx.tx.send(SSEvent::FileAvailable {
                        id: emit_id.to_string(),
                        part,
                    })?;
                }
            }

            if delta.text.is_empty() {
                continue;
            }
            if is_root {
                if !text_started {
                    ctx.tx.send(SSEvent::TextStart {
                        id: emit_id.to_string(),
                    })?;
                    text_started = true;
                }
                ctx.tx.send(SSEvent::TextDelta {
                    id: emit_id.to_string(),
                    delta: delta.text,
                })?;
            }
        }

        if is_root && text_started {
            ctx.tx.send(SSEvent::TextEnd {
                id: emit_id.to_string(),
            })?;
        }

        Ok(acc.into_response())
    }

    /// Handles tool calls by calling each tool asynchronously and returning the results.
    /// If a tool is marked as 'confirm_before_calling', this function
    /// will return `None` rather than calling the tool. All other tools
    /// will be called as usual and their results registered
    async fn summarise_and_emit<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        is_root: bool,
    ) -> Result<Option<AssistantMessage>, FormaError> {
        let key = self.state_key("summarise");
        if ctx.is_cached(&key).await {
            info!(node=%self.id, "returning cached result in summarisation");
            match ctx.cached_result(&key).await {
                Some(v) => return Ok(Some(v)),
                None => unreachable!("we just checked that there is a result"),
            };
        }

        // When streaming, `summarise_tool_results` already emitted the tokens
        // live, so we must not emit the summary again below.
        let streamed = self.stream && is_root;

        let response = self
            .summarise_tool_results(ctx, is_root)
            .in_current_span()
            .await?;
        let id = response.id.clone();
        check_triage_result(&response)?;
        let response = &response.choices[0];
        log_finish_reason(&response.finish_reason)?;

        if let LLMMessage::Assistant(m) = &response.message {
            let summary: Vec<String> = m
                .content
                .iter()
                .map(|x| {
                    let s: String = x.into();
                    s
                })
                .collect();
            let summary = summary.join("\n");
            if is_root && !streamed {
                ctx.tx.send(SSEvent::TextStart { id: id.clone() })?;
                ctx.tx.send(SSEvent::TextDelta {
                    id: id.clone(),
                    delta: (&summary).into(),
                })?;
                ctx.tx.send(SSEvent::TextEnd { id })?;
            }

            ctx.cache_result(&key, m).await?;
            Ok(Some(m.clone()))
        } else {
            unreachable!("non-assistant msg in summarise and emit?")
        }
    }

    /// What the model got wrong in `response`, when it is the model's own
    /// mistake, so that asking again with the same history is likely to work.
    ///
    /// Arguments that are valid JSON but do not fit the tool are not counted:
    /// the tool's error goes back to the model, which can fix its call
    /// knowing what was wrong.
    fn model_mistake(&self, response: &LLMResponse, user_roles: &[String]) -> Option<String> {
        let Some(choice) = response.choices.first() else {
            return Some("no choices".to_string());
        };
        if choice.finish_reason == LLMFinishReason::MalformedToolCall {
            return Some("malformed tool call".to_string());
        }
        let LLMMessage::Assistant(msg) = &choice.message else {
            return Some("not an assistant message".to_string());
        };
        if msg.tool_calls.is_empty() && msg.content.is_empty() {
            return Some("empty response".to_string());
        }
        for call in &msg.tool_calls {
            let name = match call {
                OpenAIToolChoice::Function { function, .. } => &function.name,
                OpenAIToolChoice::Custom { function, .. } => &function.name,
            };
            if let Err(e) = flatten_tool_choice(call) {
                return Some(format!("arguments for '{name}' are not valid JSON: {e}"));
            }
            if let Err(e) = self.find_allowed_tool(name, user_roles) {
                return Some(e.to_string());
            }
        }
        None
    }

    fn find_allowed_tool(
        &self,
        tool_name: &str,
        user_roles: &[String],
    ) -> Result<&Tool, FormaError> {
        let tool = match self.find_tool(tool_name) {
            None => return Err(format!("no tool '{}' found!", tool_name).into()),
            Some(t) => t,
        };
        if !tool.is_allowed(user_roles) {
            return Err(
                "the agent wanted to call a tool that the user is not allowed to call"
                    .to_string()
                    .into(),
            );
        }
        Ok(tool)
    }

    async fn join_tool_calls(
        &self,
        tool_futures: Vec<JoinHandle<Result<Option<Value>, FormaError>>>,
    ) -> Result<bool, FormaError> {
        let mut any_pending = false;
        for result in join_all(tool_futures).await {
            match result {
                Err(join_err) => return Err(FormaError::from(join_err.to_string())),
                Ok(Err(e)) => return Err(e),
                Ok(Ok(None)) => any_pending = true,
                Ok(Ok(Some(_))) => {}
            }
        }
        Ok(any_pending)
    }

    /// Finds the right arguments to use to call a tool.
    /// Returns `None` if we are not ready to call (i.e., the tool call is pending approval)
    /// Otherwise, returns the approved arguments to use.
    fn get_approved_tool_call_argument<DB: FormaDB>(
        &self,
        must_check_approval: bool,
        call: &OpenAIToolChoice,
        pending_tool_calls: &[PendingToolCall],
        tool: &Tool,
        is_root: bool,
        ctx: &CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let (tool_call_id, tool_name, arguments) = flatten_tool_choice(call)?;
        // No need to do anything. Just return same args.
        if !must_check_approval {
            return Ok(Some(arguments));
        }

        // By ID, not by name: the model may call the same tool twice in one
        // turn, and only the call the user answered may run.
        let pending_call = pending_tool_calls
            .iter()
            .find(|p| p.tool_call_id == tool_call_id);
        let r = match pending_call {
            None => {
                info!(
                    "user has not been asked to confirm calling tool '{}'",
                    &tool_name
                );
                ctx.tx.send(SSEvent::RequestFeedback {
                    tool_name,
                    arguments,
                    node_id: self.id.clone(),
                    tool_call_id: tool_call_id.to_string(),
                    from_root: is_root,
                    tool_type: tool.kind().to_string(),
                    bundle: tool.bundle.clone(),
                })?;

                None
            }
            Some(pending_call) => {
                if pending_call.approved {
                    info!("user has already approved calling tool '{}'", &tool_name);
                    Some(pending_call.arguments.clone())
                } else {
                    info!("user has not yet confirmed calling tool '{}'", &tool_name);
                    None
                }
            }
        };
        Ok(r)
    }

    /// logs whether the tool call requires confirmation or not
    fn log_tool_call_state(tool: &Tool, must_check_approval: bool) {
        if must_check_approval {
            if tool.confirm_before_calling {
                info!("tool '{}' requires confirmation", &tool.name);
            } else {
                info!("tool '{}' must be delegated", &tool.name);
            }
        } else {
            info!("tool '{}' does not require confirmation", &tool.name);
        }
    }

    /// `tool` is the resolved runner behind the call, carried through so the
    /// event can name what kind of machinery is about to run. Billing keys on
    /// that rather than on the tool's name, which is author-chosen and says
    /// nothing about cost.
    fn emit_tool_inputs<DB: FormaDB>(
        ctx: &CallContext<DB>,
        call: &OpenAIToolChoice,
        tool: &Tool,
    ) -> Result<(), FormaError> {
        let (tool_call_id, tool_name, arguments) = flatten_tool_choice(call)?;
        // if is_root {
        ctx.tx.send(SSEvent::ToolInputAvailable {
            tool_name,
            tool_call_id,
            tool_kind: tool.tool.kind().to_string(),
            input: arguments,
        })?;
        // }
        Ok(())
    }

    fn emit_tool_outputs<DB: FormaDB>(
        ctx: &CallContext<DB>,
        tool_call_id: &str,
        output: &Value,
    ) -> Result<(), FormaError> {
        // if is_root {
        ctx.tx.send(SSEvent::ToolOutputAvailable {
            tool_call_id: tool_call_id.to_string(),
            output: output.clone(),
        })?;
        // }
        Ok(())
    }

    /// Handles the tool calls as decided by the AI Agent. If a tool
    /// requires external feedback, they will NOT be called,
    /// and the client will be prompted for confirmation before execution.
    ///
    /// Returns `true` if we need to summarize; `false` otherwise.
    ///
    /// `content` is the text the assistant produced alongside the tool calls
    /// (its "let me check that" preamble). It is recorded verbatim in the
    /// session so the next iteration sees what it already told the user.
    async fn handle_tool_calls<DB: FormaDB>(
        &self,
        content: &[OpenAIMessagePart],
        tool_calls: &[OpenAIToolChoice],
        thought_signature: Option<bytes::Bytes>,
        ctx: CallContext<DB>,
        is_root: bool,
    ) -> Result<bool, FormaError> {
        let mut tool_futures: Vec<JoinHandle<Result<Option<Value>, FormaError>>> =
            Vec::with_capacity(tool_calls.len());
        let mut has_confirm = false;
        // to update the return message's tool call results
        let mut dispatched_tool_calls: Vec<OpenAIToolChoice> =
            Vec::with_capacity(tool_futures.len());

        let (pending_tool_calls, answered) = {
            let session = ctx.session.read().await;
            (
                session.collect_pending_tool_calls(&self.id),
                answered_tool_calls(&session.messages),
            )
        };
        // Resuming a suspended turn: its text was shown, and stored, when the
        // turn first ran.
        let resuming = tool_calls.iter().any(|call| {
            flatten_tool_choice(call)
                .is_ok_and(|(id, ..)| pending_tool_calls.iter().any(|p| p.tool_call_id == id))
        });

        for call in tool_calls.iter() {
            let (tool_call_id, tool_name, _arguments) = flatten_tool_choice(call)?;
            // Already run, and stored with its result, before the turn
            // suspended: running or recording it again would duplicate it.
            if answered.contains(&tool_call_id) {
                continue;
            }

            info!(tool = tool_name, "tool_call");

            let tool = self.find_allowed_tool(&tool_name, &ctx.user_roles)?;
            let must_check_approval =
                tool.confirm_before_calling || matches!(tool.tool, ToolRunner::Delegated(_));
            Self::log_tool_call_state(tool, must_check_approval);

            // By default, we use this arguments. If we do not need approval,
            // inform the user we will do this.
            let approved_arguments = self.get_approved_tool_call_argument(
                must_check_approval,
                call,
                &pending_tool_calls,
                tool,
                is_root,
                &ctx,
            )?;
            let Some(approved_arguments) = approved_arguments else {
                // Not ready to call? Emit ToolInputAvailable to notify the runtime that there
                // is a tool_call to be stored in the session's message
                has_confirm = true;
                continue;
            };
            // What runs, and what the history records, is what was approved,
            // which the user may have edited.
            let call = OpenAIToolChoice::Function {
                id: tool_call_id.clone(),
                function: OpenAIFunctionToolChoice {
                    name: tool_name.clone(),
                    arguments: serde_json::to_string(&approved_arguments).unwrap_or_default(),
                },
            };
            // Announce the tool call is beginning *before* emitting its resolved
            // input, so clients receive start → input → output in that order.
            // Previously ToolInputStart was sent from inside the spawned `call_tool`,
            // which raced with — and usually arrived after — this input event.
            ctx.tx.send(SSEvent::ToolInputStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                tool_type: tool.kind().to_string(),
                bundle: tool.bundle.clone(),
            })?;
            Self::emit_tool_inputs(&ctx, &call, tool)?;
            // add it in memory, so we do not have to re-query from db
            dispatched_tool_calls.push(call.clone());

            let tool_clone = tool.clone();
            let ctx_clone = ctx.clone();
            let call_clone = call;
            tool_futures.push(tokio::spawn(
                Self::call_tool(tool_clone, call_clone, is_root, ctx_clone).in_current_span(),
            ));
        } // end loop

        // if !is_root {
        // If it is NOT root, this does not come with the session.
        // Likewise, if this session was not suspended before, it
        // will not include this.
        // The assistant's own text goes in alongside the calls: dropping it
        // makes every later iteration read as if it were the first turn, so
        // the model re-introduces itself ("Sure, let me check...") each round.
        let content = if resuming { vec![] } else { content.to_vec() };
        // Nothing to record when every call was already answered.
        let record = !content.is_empty() || !dispatched_tool_calls.is_empty();
        let assistant_msg = FormaMessage {
            content: FormaMessageBody::Assistant {
                content: from_provider_parts(content),
                tool_calls: dispatched_tool_calls,
                thought_signature: thought_signature.clone(),
                tool_bundles: Default::default(),
                tool_types: Default::default(),
            },
            ..Default::default()
        };
        if record {
            ctx.session.write().await.messages.push(assistant_msg);
        }
        // }

        // Wait, propagating any tool errors
        let any_pending = self.join_tool_calls(tool_futures).in_current_span().await?;

        // We need to wait. Return early
        if has_confirm || any_pending {
            info!("session is still pending");
            return Ok(false);
        }
        Ok(true)
    }
}

impl Identifiable for Node {
    const CLASS: &str = "node";

    fn get_id(&self) -> &str {
        &self.id
    }
}

impl Evaluatable for Node {
    fn get_evals(&self) -> &[Eval] {
        &self.evals
    }

    fn evals_mut(&mut self) -> &mut Vec<Eval> {
        &mut self.evals
    }

    fn collect_evals(&self, callables: &mut EvaluatableSet) -> Result<(), String> {
        // Add the triage LLM within the node
        let triage = EvaluatableLLM::new_triage(self);
        let id = triage.get_id().to_string();
        if callables.llms.contains_key(&id) {
            return Err(format!("llm with id '{id}' already exists"));
        }
        callables.llms.insert(id, SingleEvaluatable::new(&triage));

        // add the summarize LLM within the node
        if !self.tools.is_empty() {
            let summarize = EvaluatableLLM::new_summarization(self);
            let id = summarize.get_id().to_string();
            if callables.llms.contains_key(&id) {
                return Err(format!("llm with id '{id}' already exists"));
            }
            callables
                .llms
                .insert(id, SingleEvaluatable::new(&summarize));
        }

        // Add the Workflow tools within the node
        for tool in self.tools.iter() {
            #[allow(irrefutable_let_patterns)]
            if let ToolRunner::Workflow(w) = &tool.tool {
                // inser
                let id = w.id.clone();
                if callables.workflows.contains_key(&id) {
                    return Err(format!("workflow with id '{id}' already exists"));
                }
                callables.workflows.insert(id, SingleEvaluatable::new(w));

                // insert contents
                w.collect_evals(callables)?;
            }
        }
        Ok(())
    }
}

fn log_finish_reason(reason: &LLMFinishReason) -> Result<(), FormaError> {
    match reason {
        LLMFinishReason::Stop | LLMFinishReason::ToolCalls | LLMFinishReason::FunctionCalls => {}
        LLMFinishReason::Length => {
            warn!("LLM generation was truncated");
        }
        LLMFinishReason::ContentFilter => {
            warn!("LLM generation was blocked by content filter");
            return Err(FormaError::from(
                "LLM generation blocked by content filter".to_string(),
            ));
        }
        LLMFinishReason::MalformedToolCall => {
            return Err(FormaError::from(
                "the model kept producing malformed tool calls".to_string(),
            ));
        }
        LLMFinishReason::Other(_) => {
            warn!("LLM finish reason is not recognized");
            return Err(FormaError::from(
                "LLM finish reason is not recognized".to_string(),
            ));
        }
    }
    Ok(())
}

/// Told to the model when it has used every iteration of a turn. Without it,
/// the model does not know its tools are gone: it tends to promise more
/// searching, try a call it cannot make, or say nothing at all.
const ITERATION_LIMIT_NOTE: &str = "\n\nYou have reached the limit of tool calls \
    for this turn: you cannot call any more tools now. Tell the user briefly what \
    you have found so far and what is still missing, then ask them, in the \
    language of the conversation, whether you should continue. A simple yes from \
    them is enough for you to go on.\n";

/// How many more times the model is asked for its turn when it gets it wrong
/// (see [`Node::model_mistake`]).
const MODEL_MISTAKE_RETRIES: usize = 2;

/// Whether `response` has any text in it.
fn has_text(response: &LLMResponse) -> bool {
    response.choices.iter().any(|c| match &c.message {
        LLMMessage::Assistant(msg) => msg
            .content
            .iter()
            .any(|p| matches!(p, OpenAIMessagePart::Text(t) if !t.text.is_empty())),
        _ => false,
    })
}

fn check_triage_result(triage_result: &ChatResponse) -> Result<(), FormaError> {
    if triage_result.choices.is_empty() {
        return Err("LLM returned no choices".into());
    } else if triage_result.choices.len() > 1 {
        warn!("LLM returned more than one choice, using the first one");
    }
    Ok(())
}

/// Accumulates streaming [`LLMChunk`]s into a single [`LLMResponse`],
/// reconstructing the assistant message from the individual deltas.
///
/// Text deltas are concatenated, and tool-call fragments are merged by their
/// `index` (via [`ToolCallAccumulator`]) so the full tool calls can be
/// recovered once the stream completes.
#[derive(Default)]
struct StreamedResponse {
    id: String,
    model: String,
    created: usize,
    text: String,
    tool_calls: ToolCallAccumulator,
    finish_reason: Option<OpenAIFinishReason>,
    /// Gemini thought signature, carried on a chunk's delta. It must be echoed
    /// back with the assistant turn on subsequent requests, so we retain it and
    /// restore it onto the reconstructed message.
    thought_signature: Option<bytes::Bytes>,
    /// Non-text parts the model returned — an image from a model generating
    /// one natively, for instance. Kept whole rather than flattened into
    /// `text`, which is where they used to end up as the string `[file]`.
    files: Vec<OpenAIMessagePart>,
    /// Token usage, populated from the final chunk when the request asked for
    /// it via `stream_options.include_usage`.
    usage: OpenAIUsage,
}

/// What one chunk contributed, split by how each half has to be delivered:
/// text is streamed as deltas, documents are announced whole.
#[derive(Default)]
struct StreamDelta {
    text: String,
    files: Vec<OpenAIMessagePart>,
}

impl StreamedResponse {
    /// Folds a single chunk into the accumulated response and returns what it
    /// contributed (empty if the chunk carried no content).
    fn push(&mut self, chunk: LLMChunk) -> StreamDelta {
        if self.id.is_empty() {
            self.id = chunk.id;
        }
        if self.model.is_empty() {
            self.model = chunk.model;
        }
        if self.created == 0 {
            self.created = chunk.created;
        }
        // Only the final chunk carries usage (the rest are null).
        if let Some(usage) = chunk.usage {
            self.usage = usage;
        }

        let mut delta = StreamDelta::default();
        for choice in chunk.choices {
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
            if let Some(content) = choice.delta.content {
                for part in content.as_parts() {
                    match &part {
                        OpenAIMessagePart::Text(t) => delta.text.push_str(&t.text),
                        // Anything else is a document. Turning it into a
                        // string here is what used to reduce a returned image
                        // to the six characters `[file]`.
                        _ => delta.files.push(part),
                    }
                }
            }
            if let Some(tool_calls) = choice.delta.tool_calls {
                self.tool_calls.apply(tool_calls);
            }
            if let Some(thought_signature) = choice.delta.thought_signature {
                self.thought_signature = Some(thought_signature);
            }
        }
        self.text.push_str(&delta.text);
        self.files.extend(delta.files.iter().cloned());
        delta
    }

    /// Builds the final [`LLMResponse`] from the accumulated chunks.
    ///
    /// Token usage comes from the final chunk when the request asked for it
    /// (`stream_options.include_usage`); it stays zero if the stream was
    /// interrupted before that chunk arrived.
    fn into_response(self) -> LLMResponse {
        let mut message = AssistantMessage::default();
        if !self.text.is_empty() {
            message
                .content
                .push(OpenAIMessagePart::Text(OpenAITextPart { text: self.text }));
        }
        message.content.extend(self.files);
        message.tool_calls = self.tool_calls.finish();
        message.thought_signature = self.thought_signature;

        let choice = MessageChoice {
            finish_reason: self
                .finish_reason
                .map(LLMFinishReason::from)
                .unwrap_or_default(),
            index: 0,
            logprobs: None,
            message: LLMMessage::Assistant(message),
        };

        ChatResponse {
            choices: vec![choice],
            created: self.created,
            id: self.id,
            model: self.model,
            object: "chat.completion".to_string(),
            usage: self.usage,
        }
    }
}

#[async_trait]
impl<DB: FormaDB> Callable<DB> for Node {
    /// **Load time checks**
    ///
    /// - ID is well-formed
    /// - Tools are all well formed
    /// - It does not have output format AND tools simultaneously
    /// - Does not have summarization prompt if it does not have tools
    fn check(&self) -> Result<(), FormaError> {
        is_valid_variable_name(&self.id)?;
        if self.max_iterations < 1 {
            return Err(FormaError::Input(
                "max_iterations must be at least 1".to_string(),
            ));
        }

        let mut seen_tool_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for tool in self.tools.iter() {
            if !seen_tool_names.insert(&tool.name) {
                return Err(FormaError::Input(format!(
                    "node '{}' has duplicate tool name '{}'",
                    self.id, tool.name
                )));
            }
            tool.check::<DB>()?;
        }

        if self.tools.is_empty() {
            // Should not have Summarization prompts or agent
            if self.summarization_prompt.is_some() {
                return Err(FormaError::Input(format!(
                    "node '{}' has a summarization_prompt but no tools",
                    self.id
                )));
            }
        } else {
            if self.llm.has_output_schema() {
                return Err(FormaError::Input(format!(
                    "node '{}' has an output schema and also tools",
                    self.id
                )));
            }
            if self.summarization_llm.has_output_schema() {
                return Err(FormaError::Input(format!(
                    "node '{}' has a summarization_llm with an output schema",
                    self.id
                )));
            }
        }
        Ok(())
    }

    /// Runs the node: triages, calls tools if needed, summarizes if needed.
    async fn run(
        &self,
        ctx: CallContext<DB>,
        is_root: bool,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        // Report start step
        let span = span!(tracing::Level::INFO, "node", node=%self.id, job_id=%ctx.job_id);

        async move {
            let mut any_tools_called = false;
            let mut should_summarize = false;
            let mut triage_result = self.triage(&ctx, 0, is_root).in_current_span().await?;
            for iteration in 0..self.max_iterations {
                if triage_result.tool_calls.is_empty() && triage_result.content.is_empty() {
                    tracing::error!("no tools or content returned");
                    return Ok(None);
                }

                if triage_result.tool_calls.is_empty() {
                    should_summarize = false; // Last 'triage' becomes Summarization
                    break; // AI agent said we are done.
                }

                should_summarize = self
                    .handle_tool_calls(
                        &triage_result.content,
                        &triage_result.tool_calls,
                        triage_result.thought_signature.clone(),
                        ctx.clone(),
                        is_root,
                    )
                    .in_current_span()
                    .await?;
                if !should_summarize {
                    return Ok(None); // something is pending.
                }
                any_tools_called = true;

                // skip last iteration
                if iteration + 1 < self.max_iterations {
                    triage_result = self
                        .triage(&ctx, iteration + 1, is_root)
                        .in_current_span()
                        .await?;
                }
            }

            let summarize_result = if any_tools_called && should_summarize {
                self.summarise_and_emit(&ctx, is_root)
                    .in_current_span()
                    .await?
            } else {
                None
            };

            match summarize_result {
                Some(v) => {
                    let s: String = (&v).into();
                    ctx.register_variable(&self.id, s).await?;
                    let ret = serde_json::to_value(&v).unwrap();
                    Ok(Some(ret))
                }
                None => {
                    let s: String = (&triage_result).into();
                    ctx.register_variable(&self.id, s).await?;
                    let ret = serde_json::to_value(&triage_result).unwrap();
                    Ok(Some(ret))
                }
            }
        }
        .instrument(span)
        .await
    }

    fn inner_ids(&self) -> Vec<String> {
        let mut ret = Vec::new();
        ret.push(self.id.clone());
        for tool in &self.tools {
            match &tool.tool {
                ToolRunner::Workflow(w) => {
                    ret.extend(<Workflow as Callable<DB>>::inner_ids(w));
                }
                ToolRunner::Template(_) => {}
                ToolRunner::ApiCall(_) => {}
                ToolRunner::Delegated(_) => {}
                ToolRunner::Mcp(_) => {}
                ToolRunner::Mailgun(_) => {}
                ToolRunner::TavilyWebSearch(_) => {}
                ToolRunner::GoogleAgentSearch(_) => {}
                ToolRunner::GoogleGenImage(_) => {}
                ToolRunner::Fetch(_) => {}
                ToolRunner::Knowledgebase(_) => {}
                ToolRunner::CodeExecution(_) => {}
                ToolRunner::CurrentDate(_) => {}
            }
        }
        ret
    }
}

#[cfg(test)]
mod test {
    use super::{
        Admit, AttachBudget, FormaMessageBody, FormaMessagePart, FormaTextPart,
        FormaWorkspaceFilePart, StreamedResponse, attached_file, for_provider, payload_len,
    };
    use genai_client::{
        LLMChunk,
        openai::v1::{
            OpenAIChatDelta, OpenAIFile, OpenAIFilePart, OpenAIMessageChoiceDelta,
            OpenAIMessageContent,
        },
    };

    /// One chunk whose content carries the given parts.
    fn chunk_with(parts: Vec<OpenAIMessagePart>) -> LLMChunk {
        LLMChunk {
            choices: vec![OpenAIMessageChoiceDelta {
                finish_reason: None,
                index: 0,
                logprobs: None,
                delta: OpenAIChatDelta {
                    content: Some(OpenAIMessageContent::Parts(parts)),
                    ..Default::default()
                },
            }],
            ..Default::default()
        }
    }

    fn a_file_part() -> OpenAIMessagePart {
        OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some("data:image/png;base64,iVBORw0KGgo=".into()),
                file_id: None,
                filename: Some("chart.png".into()),
            },
        })
    }

    #[test]
    fn streamed_documents_are_reported_separately_from_text() {
        let mut acc = StreamedResponse::default();

        let delta = acc.push(chunk_with(vec![OpenAIMessagePart::Text(OpenAITextPart {
            text: "here is a chart".into(),
        })]));
        assert_eq!(delta.text, "here is a chart");
        assert!(delta.files.is_empty());

        // The document must not be folded into the text delta, which is what
        // used to turn it into the literal string `[file]`.
        let delta = acc.push(chunk_with(vec![a_file_part()]));
        assert!(delta.text.is_empty());
        assert_eq!(delta.files.len(), 1);
    }

    #[test]
    fn streamed_documents_survive_into_the_response() {
        let mut acc = StreamedResponse::default();
        acc.push(chunk_with(vec![OpenAIMessagePart::Text(OpenAITextPart {
            text: "here you go".into(),
        })]));
        acc.push(chunk_with(vec![a_file_part()]));

        let response = acc.into_response();
        let LLMMessage::Assistant(m) = &response.choices[0].message else {
            panic!("expected an assistant message");
        };

        assert_eq!(m.content.len(), 2);
        assert!(matches!(m.content[0], OpenAIMessagePart::Text(_)));
        assert!(matches!(m.content[1], OpenAIMessagePart::File(_)));
    }

    use std::{collections::HashMap, sync::Arc};

    use db::NoneDB;
    use genai_client::{
        AssistantMessage, LLMFinishReason, LLMMessage, LLMResponse, MessageChoice, Role, SSEvent,
        openai::v1::{
            OpenAICustomToolChoice, OpenAIFunctionToolChoice, OpenAIMessagePart, OpenAITextPart,
            OpenAIToolChoice,
        },
    };
    use protocols::{
        CachedToolResult, DeploymentCredentials, EncryptedTokens, FormaDB, FormaMessage,
        PendingToolCall, Session, SessionData,
    };
    use serde_json::{Value, json};
    use tokio::sync::{Mutex, RwLock, mpsc};

    use crate::{
        Callable, Node, Tool, ToolRunner, Workflow,
        callable::CallContext,
        eval_agent::{Evaluatable, EvaluatableSet, SingleEvaluatable},
        node::{default_node_id, flatten_tool_choice},
        tool::TemplateTool,
    };

    #[test]
    fn default() {
        let n = Node::default();
        assert!(n.id.starts_with('n'));
        assert!(n.summarization_prompt.is_none());
        assert!(n.tools.is_empty());
        assert!(n.evals.is_empty());
    }

    fn text_chunk(
        id: &str,
        text: Option<&str>,
        finish: Option<genai_client::openai::v1::OpenAIFinishReason>,
    ) -> genai_client::LLMChunk {
        use genai_client::openai::v1::{
            OpenAIChatDelta, OpenAIMessageChoiceDelta, OpenAIMessageContent,
        };
        genai_client::LLMChunk {
            id: id.to_string(),
            model: "test-model".to_string(),
            created: 42,
            choices: vec![OpenAIMessageChoiceDelta {
                finish_reason: finish,
                index: 0,
                logprobs: None,
                delta: OpenAIChatDelta {
                    content: text.map(|t| OpenAIMessageContent::String(t.to_string())),
                    ..Default::default()
                },
            }],
            ..Default::default()
        }
    }

    #[test]
    fn streamed_response_accumulates_text() {
        use genai_client::openai::v1::OpenAIFinishReason;

        let mut acc = super::StreamedResponse::default();
        // First chunk usually carries the role only (no text).
        assert_eq!(acc.push(text_chunk("resp-1", None, None)).text, "");
        assert_eq!(
            acc.push(text_chunk("resp-1", Some("Hello"), None)).text,
            "Hello"
        );
        assert_eq!(
            acc.push(text_chunk("resp-1", Some(", world"), None)).text,
            ", world"
        );
        // Final content chunk carries the finish reason and no text.
        assert_eq!(
            acc.push(text_chunk("resp-1", None, Some(OpenAIFinishReason::Stop)))
                .text,
            ""
        );
        // The usage chunk (from stream_options.include_usage) has empty choices
        // and populated usage.
        let mut usage_chunk = text_chunk("resp-1", None, None);
        usage_chunk.choices.clear();
        usage_chunk.usage = Some(genai_client::openai::v1::OpenAIUsage {
            prompt_tokens: 5,
            completion_tokens: 7,
            total_tokens: 12,
            ..Default::default()
        });
        assert_eq!(acc.push(usage_chunk).text, "");

        let response = acc.into_response();
        assert_eq!(response.id, "resp-1");
        assert_eq!(response.model, "test-model");
        assert_eq!(response.created, 42);
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].finish_reason, LLMFinishReason::Stop);
        assert_eq!(response.usage.prompt_tokens, 5);
        assert_eq!(response.usage.completion_tokens, 7);
        assert_eq!(response.usage.total_tokens, 12);

        match &response.choices[0].message {
            LLMMessage::Assistant(m) => {
                let text: String = m.into();
                assert_eq!(text, "Hello, world");
                assert!(m.tool_calls.is_empty());
            }
            _ => panic!("expected an assistant message"),
        }
    }

    #[test]
    fn streamed_response_reconstructs_tool_calls() {
        use genai_client::openai::v1::{
            OpenAIChatDelta, OpenAIFinishReason, OpenAIFunctionCallDelta, OpenAIMessageChoiceDelta,
            OpenAIToolCallDelta,
        };

        fn tool_chunk(
            tool_calls: Option<Vec<OpenAIToolCallDelta>>,
            finish: Option<OpenAIFinishReason>,
        ) -> genai_client::LLMChunk {
            genai_client::LLMChunk {
                id: "resp-tool".to_string(),
                model: "test-model".to_string(),
                created: 7,
                choices: vec![OpenAIMessageChoiceDelta {
                    finish_reason: finish,
                    index: 0,
                    logprobs: None,
                    delta: OpenAIChatDelta {
                        tool_calls,
                        ..Default::default()
                    },
                }],
                ..Default::default()
            }
        }

        let mut acc = super::StreamedResponse::default();
        acc.push(tool_chunk(
            Some(vec![OpenAIToolCallDelta {
                index: 0,
                id: Some("call_1".to_string()),
                call_type: Some("function".to_string()),
                function: Some(OpenAIFunctionCallDelta {
                    name: Some("get_weather".to_string()),
                    arguments: Some("{\"ci".to_string()),
                }),
            }]),
            None,
        ));
        acc.push(tool_chunk(
            Some(vec![OpenAIToolCallDelta {
                index: 0,
                function: Some(OpenAIFunctionCallDelta {
                    name: None,
                    arguments: Some("ty\":\"Paris\"}".to_string()),
                }),
                ..Default::default()
            }]),
            Some(OpenAIFinishReason::ToolCalls),
        ));

        let response = acc.into_response();
        assert_eq!(
            response.choices[0].finish_reason,
            LLMFinishReason::ToolCalls
        );
        match &response.choices[0].message {
            LLMMessage::Assistant(m) => {
                // No text was streamed for a tool-only turn.
                let text: String = m.into();
                assert!(text.is_empty());
                assert_eq!(m.tool_calls.len(), 1);
                assert_eq!(m.tool_calls[0].id(), "call_1");
                assert_eq!(m.tool_calls[0].name(), "get_weather");
                assert_eq!(m.tool_calls[0].arguments(), r#"{"city":"Paris"}"#);
            }
            _ => panic!("expected an assistant message"),
        }
    }

    #[test]
    fn streamed_response_preserves_thought_signature() {
        use genai_client::openai::v1::{OpenAIChatDelta, OpenAIMessageChoiceDelta};

        // A single chunk carrying a Gemini thought signature (as the Vertex
        // unary-fallback stream emits).
        let chunk = genai_client::LLMChunk {
            id: "resp-ts".to_string(),
            model: "test-model".to_string(),
            created: 1,
            choices: vec![OpenAIMessageChoiceDelta {
                finish_reason: None,
                index: 0,
                logprobs: None,
                delta: OpenAIChatDelta {
                    thought_signature: Some(bytes::Bytes::from_static(b"sig-123")),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let mut acc = super::StreamedResponse::default();
        acc.push(chunk);
        let response = acc.into_response();
        match &response.choices[0].message {
            LLMMessage::Assistant(m) => {
                assert_eq!(m.thought_signature.as_deref(), Some(b"sig-123".as_slice()));
            }
            _ => panic!("expected an assistant message"),
        }
    }

    #[tokio::test]
    async fn test_resolved_node() {
        // Create a node with a specific ID
        let node = Node {
            id: "test_node".to_string(),
            ..Default::default()
        };

        // Create a session with the node's result already in the state
        let expected_result = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "cached".to_string(),
            })],
            ..Default::default()
        };
        let db = NoneDB::new().await.unwrap();
        let mut cached_messages = HashMap::new();
        cached_messages.insert(node.state_key("triage-0"), expected_result.clone());
        let mut session = db.new_session("agent_id", "user_id").await.unwrap();
        session.cached_messages = cached_messages;
        let session = Arc::new(RwLock::new(session));

        // Create a context
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "test_job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        // Run the node
        // We expect it to return the cached result without calling the LLM
        let result = node.run(ctx, false).await;
        assert!(result.is_ok());
        let v = result.unwrap().unwrap();
        let res: AssistantMessage = serde_json::from_value(v).unwrap();
        let s: String = res.into();
        let exp_s: String = expected_result.into();
        assert_eq!(s, exp_s);
    }

    /// A tool that reports an attachment hands the conversation a reference to
    /// the file, not the file.
    #[test]
    fn test_attached_file_reads_what_the_attach_tool_reports() {
        let output = serde_json::json!({
            "attached": {
                "workspace": "bucket/handbook",
                "path": "invoices/march.pdf",
                "media_type": "application/pdf",
                "size_bytes": 2048
            },
            "note": "anything"
        });
        let part = attached_file(&output).expect("a reference");
        assert_eq!(part.workspace_id, "bucket/handbook");
        assert_eq!(part.path, "invoices/march.pdf");
        assert_eq!(part.media_type.as_deref(), Some("application/pdf"));

        // Every other tool result is just text.
        assert!(attached_file(&serde_json::json!({ "documents": [] })).is_none());
        assert!(attached_file(&serde_json::json!({ "attached": { "path": "x" } })).is_none());
    }

    /// The same file mentioned twice rides along once: the latest mention
    /// carries it, the earlier one says where it is.
    #[test]
    fn test_attach_budget_sends_a_file_once() {
        let mut budget = AttachBudget::new(1_000);
        let file = ("bucket/kb".to_string(), "march.pdf".to_string());
        // Walking backwards, the latest mention is the first one offered.
        assert_eq!(budget.admit(&file, 100), Admit::Carry);
        assert_eq!(budget.admit(&file, 100), Admit::Duplicate);
        // A duplicate costs nothing: only the copy that is sent is paid for.
        assert_eq!(budget.left, 900);
    }

    /// What does not fit is left out, and what was already admitted stays.
    #[test]
    fn test_attach_budget_stops_at_what_a_request_can_hold() {
        let mut budget = AttachBudget::new(1_000);
        let big = ("kb".to_string(), "big.pdf".to_string());
        let small = ("kb".to_string(), "small.png".to_string());
        let other = ("kb".to_string(), "other.png".to_string());

        assert_eq!(budget.admit(&big, 900), Admit::Carry);
        assert_eq!(budget.admit(&small, 200), Admit::TooMuch);
        // Something that still fits is taken, rather than everything after the
        // first refusal being dropped.
        assert_eq!(budget.admit(&other, 100), Admit::Carry);
        assert_eq!(budget.left, 0);
    }

    /// A file too large for an empty request is refused rather than sent.
    #[test]
    fn test_attach_budget_refuses_what_never_fits() {
        let mut budget = AttachBudget::new(100);
        let huge = ("kb".to_string(), "huge.pdf".to_string());
        assert_eq!(budget.admit(&huge, 101), Admit::TooMuch);
        assert_eq!(budget.left, 100);
    }

    #[test]
    fn test_payload_len_measures_what_is_sent() {
        let image = FormaMessagePart::ImageUrl(protocols::FormaImagePart {
            image_url: protocols::FormaImageUrl {
                url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        });
        assert_eq!(payload_len(&image), 26);

        let file = FormaMessagePart::File(protocols::FormaFilePart {
            file: protocols::FormaFile {
                file_data: Some("data:application/pdf;base64,AA".to_string()),
                file_id: None,
                filename: Some("a.pdf".to_string()),
            },
        });
        assert_eq!(payload_len(&file), 30);

        // A reference carries nothing; it is what gets resolved.
        assert_eq!(
            payload_len(&FormaMessagePart::WorkspaceFile(FormaWorkspaceFilePart {
                workspace_id: "kb".to_string(),
                path: "a.pdf".to_string(),
                media_type: None,
            })),
            0
        );
    }

    async fn test_context() -> CallContext<NoneDB> {
        let db = NoneDB::new().await.unwrap();
        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(session)),
            tx,
            job_id: "test_job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        }
    }

    /// Files attached by calls that ran in parallel wait for the last of their
    /// results, so the results stay together: Gemini refuses a request where
    /// anything comes between them.
    #[test]
    fn test_attachments_go_after_the_last_parallel_result() {
        let result = |id: &str| FormaMessage {
            content: FormaMessageBody::Tool {
                tool_call_id: id.to_string(),
                content: vec![FormaMessagePart::Text(FormaTextPart { text: "{}".into() })],
            },
            ..Default::default()
        };
        let file = || {
            FormaMessagePart::Text(FormaTextPart {
                text: "a file".into(),
            })
        };
        let roles = |messages: &[FormaMessage]| -> Vec<String> {
            messages
                .iter()
                .map(|m| format!("{:?}", m.content.role()))
                .collect()
        };

        let out = super::with_attachments(vec![
            (
                FormaMessage::new_user_with_content("look".to_string()),
                vec![],
            ),
            (result("tc1"), vec![file()]),
            (result("tc2"), vec![file()]),
            (
                FormaMessage::new_user_with_content("thanks".to_string()),
                vec![],
            ),
        ]);
        assert_eq!(roles(&out), ["User", "Tool", "Tool", "User", "User"]);
        // Both files, in one message, right after the results.
        assert_eq!(out[3].content.content().len(), 2);

        // At the end of the conversation too.
        let out =
            super::with_attachments(vec![(result("tc1"), vec![]), (result("tc2"), vec![file()])]);
        assert_eq!(roles(&out), ["Tool", "Tool", "User"]);
    }

    /// A history window that starts part way through a turn loses the results
    /// whose call was cut off, and the calls before the first message a
    /// provider takes as a start; the rest is left as it is.
    #[test]
    fn test_history_starts_with_whole_calls() {
        let user = |text: &str| FormaMessage::new_user_with_content(text.to_string());
        let reply = |text: &str| FormaMessage::new_assistant_with_content(text.to_string());
        let result = |id: &str| FormaMessage {
            content: FormaMessageBody::Tool {
                tool_call_id: id.to_string(),
                content: vec![FormaMessagePart::Text(FormaTextPart { text: "{}".into() })],
            },
            ..Default::default()
        };
        let call = |id: &str| FormaMessage {
            content: FormaMessageBody::Assistant {
                content: vec![],
                tool_calls: vec![OpenAIToolChoice::Function {
                    id: id.to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "lookup".to_string(),
                        arguments: "{}".to_string(),
                    },
                }],
                thought_signature: None,
                tool_bundles: Default::default(),
                tool_types: Default::default(),
            },
            ..Default::default()
        };
        let roles = |messages: &[FormaMessage]| -> Vec<String> {
            messages
                .iter()
                .map(|m| format!("{:?}", m.content.role()))
                .collect()
        };

        // A whole conversation is left alone.
        let whole = [user("hi"), call("a"), result("a"), reply("done")];
        assert_eq!(super::whole_calls(&whole).len(), 4);

        // Cut after a call: its result goes, and so does the next call.
        let cut = [
            result("a"),
            call("b"),
            result("b"),
            reply("done"),
            user("hi"),
        ];
        assert_eq!(roles(super::whole_calls(&cut)), ["Assistant", "User"]);

        // Nothing but calls and results: only the orphan results go.
        let busy = [result("a"), call("b"), result("b")];
        assert_eq!(roles(super::whole_calls(&busy)), ["Assistant", "Tool"]);

        assert!(super::whole_calls(&[]).is_empty());
    }

    /// An ordinary conversation goes to the provider as it is: one message in,
    /// one message out, and nothing fetched.
    #[tokio::test]
    async fn test_for_provider_leaves_plain_messages_alone() {
        let ctx = test_context().await;
        let messages = vec![
            FormaMessage::new_system_with_content("be helpful".to_string()),
            FormaMessage::new_user_with_content("hello".to_string()),
        ];
        let out = for_provider(messages, &[], &ctx).await.expect("converts");
        assert_eq!(out.len(), 2);
    }

    /// A reference is resolved through the node's own tools, so one naming a
    /// workspace this agent cannot reach is refused — an old message cannot
    /// outlive the access it was made under.
    #[tokio::test]
    async fn test_for_provider_refuses_a_workspace_the_agent_cannot_reach() {
        let ctx = test_context().await;
        let message = FormaMessage {
            content: FormaMessageBody::Tool {
                tool_call_id: "call-1".to_string(),
                content: vec![
                    FormaMessagePart::Text(FormaTextPart {
                        text: "{\"attached\":{}}".to_string(),
                    }),
                    FormaMessagePart::WorkspaceFile(FormaWorkspaceFilePart {
                        workspace_id: "bucket/gone".to_string(),
                        path: "invoices/march.pdf".to_string(),
                        media_type: Some("application/pdf".to_string()),
                    }),
                ],
            },
            ..Default::default()
        };

        let err = for_provider(vec![message], &[], &ctx)
            .await
            .expect_err("must not reach a provider");
        assert!(
            err.to_string().contains("bucket/gone"),
            "the error should name the workspace: {err}"
        );
    }

    #[test]
    fn test_default_node_id_format() {
        let id = default_node_id();
        assert!(id.starts_with('n'));
        assert_eq!(id.len(), 36);
    }

    #[test]
    fn test_node_default_initialization() {
        let node = Node::default();
        assert!(node.id.starts_with('n'));
        assert!(node.evals.is_empty());
    }

    #[test]
    fn test_node_identifiable() {
        let node = Node::default();
        assert_eq!(node.get_id(), &node.id);
    }

    #[test]
    fn test_node_evaluatable_evals() {
        let node = Node::default();
        assert!(node.get_evals().is_empty());
    }

    #[tokio::test]
    async fn test_complete_span() {
        let node = Node::default();
        let db = NoneDB::new().await.unwrap();
        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let session = Arc::new(RwLock::new(session));
        let messages = vec![FormaMessage::from("hello!".to_string())];

        // Empty input... should not crash
        let span = llm_span!("some span");
        node.init_span(&node.llm, &span, &session, &messages).await;

        let response = LLMResponse {
            ..Default::default()
        };
        node.complete_span(&span, &response);

        // One message, should not crash
        let span = llm_span!("some span");
        node.init_span(&node.llm, &span, &session, &messages).await;

        let model = "the-model";
        let response = LLMResponse {
            model: model.to_string(),
            choices: vec![MessageChoice::default()],
            ..Default::default()
        };
        node.complete_span(&span, &response);
    }

    #[tokio::test]
    async fn test_call_tool() {
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let name = "template".to_string();
        let arguments = "{ \"name\": \"Emilia\" }".to_string();
        let tool_call_id = "some-id".to_string();
        let call = OpenAIToolChoice::Function {
            id: tool_call_id.clone(),
            function: OpenAIFunctionToolChoice {
                arguments,
                name: name.clone(),
            },
        };

        let tool = Tool {
            name: name.clone(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["Hello, {{name}}!".to_string()],
            }),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let db = NoneDB::new().await.unwrap();

        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let session = Arc::new(RwLock::new(session));
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "some-job-id".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let r = Node::call_tool(tool, call, true, ctx).await;
        assert!(r.is_ok());
        let s = session.read().await;
        if let Some(v) = s.tool_calls_results.get(&tool_call_id) {
            let r = serde_json::to_string(&v.result).unwrap();
            // This particular tool returns a string.
            assert_eq!(r, "\"Hello, Emilia!\"")
        } else {
            panic!("tool did not register its result")
        }

        // todo!()
    }

    #[tokio::test]
    async fn test_call_tool_failure_is_reported_as_result() {
        // A tool that errors during execution should NOT abort the run: the
        // error must be recorded as this tool_call's result so the model sees
        // what happened (and every tool_call keeps a matching tool result).
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let name = "fetch".to_string();
        let tool_call_id = "fetch-id".to_string();
        // localhost is refused by the SSRF guard, so `run` returns an Err
        // before any network access (works in every build).
        let call = OpenAIToolChoice::Function {
            id: tool_call_id.clone(),
            function: OpenAIFunctionToolChoice {
                arguments: r#"{ "url": "http://localhost/secret" }"#.to_string(),
                name: name.clone(),
            },
        };
        let tool = Tool {
            name: name.clone(),
            description: None,
            tool: ToolRunner::Fetch(crate::tool::fetch_tool::FetchTool::default()),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let db = NoneDB::new().await.unwrap();
        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let session = Arc::new(RwLock::new(session));
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "some-job-id".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let r = Node::call_tool(tool, call, true, ctx).await;
        // The failure is surfaced as an Ok result, not a propagated error.
        assert!(r.is_ok(), "tool failure should not abort the run");
        let output = r
            .unwrap()
            .expect("failed tool should still produce a result");
        assert!(
            output.get("error").and_then(|e| e.as_str()).is_some(),
            "result should carry an error message, got {output:?}"
        );

        // And it must be registered as this call's result.
        let s = session.read().await;
        let stored = s
            .tool_calls_results
            .get(&tool_call_id)
            .expect("failed tool did not register a result");
        assert!(stored.result.get("error").is_some());
    }

    #[tokio::test]
    async fn test_get_msgs() {
        let template = "Hello, my name is {{name: string \"the name of this human\"}}";
        let node = Node::default();
        let messages = vec![FormaMessage::from("hello!".to_string())];
        let session = SessionData {
            messages: messages.clone(),
            variables: HashMap::from([(
                "name".to_string(),
                serde_json::from_str("\"peter\"").unwrap(),
            )]),
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(session));

        let ret_msgs = node
            .get_messages_history(template, session)
            .await
            .expect("could not do it");
        assert_eq!(ret_msgs.len(), messages.len() + 1);
        assert_eq!(ret_msgs[0].content.role(), Role::System);
        assert_eq!(ret_msgs[0].to_string(), "Hello, my name is peter\n")
    }

    #[test]
    fn test_get_id() {
        let node = Node::default();
        let id = node.get_id();
        assert_eq!(id, &node.id);
        assert!(id.starts_with('n'));
    }

    #[test]
    fn test_inner_ids_no_tools() {
        let node = Node {
            id: "node_1".to_string(),
            ..Default::default()
        };
        let ids = <Node as Callable<NoneDB>>::inner_ids(&node);
        assert_eq!(ids, vec!["node_1"]);
    }

    #[test]
    fn test_inner_ids_with_template_tools_only() {
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![
                Tool {
                    name: "t1".to_string(),
                    description: None,
                    tool: ToolRunner::Template(TemplateTool {
                        templates: vec!["result".to_string()],
                    }),
                    just_for: vec![],
                    confirm_before_calling: false,
                    bundle: None,
                },
                Tool {
                    name: "t2".to_string(),
                    description: None,
                    tool: ToolRunner::Template(TemplateTool {
                        templates: vec!["result".to_string()],
                    }),
                    just_for: vec![],
                    confirm_before_calling: false,
                    bundle: None,
                },
            ],
            ..Default::default()
        };
        // Template tools are excluded; only the node's own id is returned
        let ids = <Node as Callable<NoneDB>>::inner_ids(&node);
        assert_eq!(ids, vec!["node_1"]);
    }

    #[test]
    fn test_inner_ids_with_workflow_tool() {
        let inner_node = Node {
            id: "inner_node".to_string(),
            ..Default::default()
        };
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![vec![inner_node]],
            ..Default::default()
        };
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![Tool {
                name: "wf_tool".to_string(),
                description: None,
                tool: ToolRunner::Workflow(workflow),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let ids = <Node as Callable<NoneDB>>::inner_ids(&node);
        // Own id first, then workflow id, then the workflow's inner node id
        assert!(ids.contains(&"node_1".to_string()));
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"inner_node".to_string()));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_inner_ids_with_multiple_workflow_tools() {
        let workflow_a = Workflow {
            id: "workflow_a".to_string(),
            ..Default::default()
        };
        let workflow_b = Workflow {
            id: "workflow_b".to_string(),
            ..Default::default()
        };
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![
                Tool {
                    name: "wf_a".to_string(),
                    description: None,
                    tool: ToolRunner::Workflow(workflow_a),
                    just_for: vec![],
                    confirm_before_calling: false,
                    bundle: None,
                },
                Tool {
                    name: "wf_b".to_string(),
                    description: None,
                    tool: ToolRunner::Workflow(workflow_b),
                    just_for: vec![],
                    confirm_before_calling: false,
                    bundle: None,
                },
            ],
            ..Default::default()
        };
        let ids = <Node as Callable<NoneDB>>::inner_ids(&node);
        assert!(ids.contains(&"node_1".to_string()));
        assert!(ids.contains(&"workflow_a".to_string()));
        assert!(ids.contains(&"workflow_b".to_string()));
        // Each default Workflow also contains one default Node, so 1 + 2*2 = 5
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn test_find_tool() {
        let tool1 = Tool {
            name: "tool_a".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["template_a".to_string()],
            }),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let tool2 = Tool {
            name: "tool_b".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["template_b".to_string()],
            }),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let node = Node {
            tools: vec![tool1.clone(), tool2.clone()],
            ..Default::default()
        };

        // Test finding existing tool
        let found = node.find_tool("tool_a");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "tool_a");

        // Test finding another existing tool
        let found = node.find_tool("tool_b");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "tool_b");

        // Test finding non-existent tool
        let found = node.find_tool("tool_c");
        assert!(found.is_none());

        // Test with empty tools
        let node_empty = Node {
            tools: vec![],
            ..Default::default()
        };
        let found = node_empty.find_tool("tool_a");
        assert!(found.is_none());
    }

    #[test]
    fn test_flatten_no_workflows() {
        let node = Node {
            tools: vec![Tool {
                name: "tool_a".to_string(),
                description: None,
                tool: ToolRunner::Template(TemplateTool {
                    templates: vec!["template_a".to_string()],
                }),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let mut callables = EvaluatableSet::default();
        let result = node.collect_evals(&mut callables);
        assert!(result.is_ok());
        assert!(callables.workflows.is_empty());
    }

    #[test]
    fn test_flatten_with_workflow() {
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            ..Default::default()
        };
        let tool = Tool {
            name: "workflow_tool".to_string(),
            description: None,
            tool: ToolRunner::Workflow(workflow),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let mut callables = EvaluatableSet::default();
        let result = node.collect_evals(&mut callables);
        assert!(result.is_ok());
        assert!(callables.workflows.contains_key("workflow_1"));
    }

    #[test]
    fn test_flatten_duplicate_workflow_id() {
        let workflow = Workflow {
            id: "duplicate_id".to_string(),
            ..Default::default()
        };
        let tool = Tool {
            name: "workflow_tool".to_string(),
            description: None,
            tool: ToolRunner::Workflow(workflow),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let mut callables = EvaluatableSet::default();
        callables.workflows.insert(
            "duplicate_id".to_string(),
            SingleEvaluatable::new(&Workflow {
                id: "duplicate_id".to_string(),
                ..Default::default()
            }),
        );

        let result = node.collect_evals(&mut callables);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_flatten_tool_choice_function_variant() {
        let choice = OpenAIToolChoice::Function {
            id: "call_1".to_string(),
            function: OpenAIFunctionToolChoice {
                name: "my_tool".to_string(),
                arguments: r#"{"key":"value"}"#.to_string(),
            },
        };
        let (id, name, args) = flatten_tool_choice(&choice).unwrap();
        assert_eq!(id, "call_1");
        assert_eq!(name, "my_tool");
        assert_eq!(args["key"], "value");
    }

    #[test]
    fn test_flatten_tool_choice_custom_variant() {
        let choice = OpenAIToolChoice::Custom {
            id: "call_2".to_string(),
            function: OpenAICustomToolChoice {
                name: "custom_tool".to_string(),
                input: r#"{"x":42}"#.to_string(),
            },
        };
        let (id, name, args) = flatten_tool_choice(&choice).unwrap();
        assert_eq!(id, "call_2");
        assert_eq!(name, "custom_tool");
        assert_eq!(args["x"], 42);
    }

    #[test]
    fn test_flatten_tool_choice_invalid_json_returns_err() {
        let choice = OpenAIToolChoice::Function {
            id: "bad".to_string(),
            function: OpenAIFunctionToolChoice {
                name: "tool".to_string(),
                arguments: "not-json".to_string(),
            },
        };
        assert!(flatten_tool_choice(&choice).is_err());
    }

    fn make_tool_choice(tool_call_id: &str, name: &str, arguments: &str) -> OpenAIToolChoice {
        OpenAIToolChoice::Function {
            id: tool_call_id.to_string(),
            function: OpenAIFunctionToolChoice {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn make_tool(name: &str, confirm: bool) -> Tool {
        Tool {
            name: name.to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["result".to_string()],
            }),
            just_for: vec![],
            confirm_before_calling: confirm,
            bundle: None,
        }
    }

    /// All confirm tools: returns None without executing any tool.
    #[tokio::test]
    async fn test_handle_tool_calls_all_confirm_returns_none() {
        let tool = make_tool("confirm_tool", true);
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(SessionData::default()));
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let choices = vec![make_tool_choice("id1", "confirm_tool", "{}")];
        let result = node
            .handle_tool_calls(&[], &choices, None, ctx, false)
            .await;

        assert!(result.is_ok());
        let should_summarise = result.unwrap();
        assert!(!should_summarise, "should return None for confirm tools");

        // RequestFeedback must have been sent, no ToolInputAvailable
        let mut events = vec![];
        rx.close();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SSEvent::RequestFeedback { tool_name, .. } if tool_name == "confirm_tool")),
            "RequestFeedback must be sent for the confirm tool"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SSEvent::ToolInputAvailable { .. })),
            "ToolInputAvailable must NOT be sent for a confirm tool"
        );

        // Tool result must not be in session
        let s = session.read().await;
        assert!(s.tool_calls_results.get("confirm_tool").is_none());
    }

    /// The assistant's preamble text is stored with the tool calls, so the
    /// next iteration knows what it already said to the user.
    #[tokio::test]
    async fn test_handle_tool_calls_keeps_assistant_content() {
        let node = Node {
            tools: vec![make_tool("normal_tool", false)],
            ..Default::default()
        };
        let db = NoneDB::new().await.unwrap();
        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let session = Arc::new(RwLock::new(session));
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let content = vec![OpenAIMessagePart::Text(OpenAITextPart {
            text: "Sure, let me check that.".to_string(),
        })];
        let choices = vec![make_tool_choice("normal_tool_call_id", "normal_tool", "{}")];
        node.handle_tool_calls(&content, &choices, None, ctx, true)
            .await
            .unwrap();

        let s = session.read().await;
        let assistant = s
            .messages
            .iter()
            .find(|m| matches!(m.content, protocols::FormaMessageBody::Assistant { .. }))
            .expect("assistant message must be stored");
        let text: String = assistant
            .content
            .content()
            .iter()
            .map(|p| {
                let s: String = p.into();
                s
            })
            .collect();
        assert_eq!(text, "Sure, let me check that.");
    }

    /// Mixed tools: confirm tools are skipped, non-confirm tools are executed.
    /// Returns None because at least one confirm tool was present.
    fn hitl_context(session: Session) -> (CallContext<NoneDB>, mpsc::UnboundedReceiver<SSEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session,
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        (ctx, rx)
    }

    fn drain(mut rx: mpsc::UnboundedReceiver<SSEvent>) -> Vec<SSEvent> {
        rx.close();
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        events
    }

    fn answered_pending(tool_call_id: &str, tool_name: &str, arguments: Value) -> PendingToolCall {
        PendingToolCall {
            tool_call_id: tool_call_id.to_string(),
            node_id: Node::default().id,
            tool_name: tool_name.to_string(),
            arguments,
            approved: true,
            from_root: true,
            tool_type: String::new(),
            bundle: None,
        }
    }

    async fn hitl_session(node: &Node, pending: Vec<PendingToolCall>) -> Session {
        let mut session = NoneDB::new()
            .await
            .unwrap()
            .new_session("agent_id", "user_id")
            .await
            .unwrap();
        session.pending_tool_calls = pending
            .into_iter()
            .map(|mut p| {
                p.node_id = node.id.clone();
                p
            })
            .collect();
        Arc::new(RwLock::new(session))
    }

    /// What runs, and what is recorded, is what the user approved, not what
    /// the model first proposed.
    #[tokio::test]
    async fn test_an_approved_call_runs_with_the_approved_arguments() {
        let node = Node {
            tools: vec![make_tool("refund", true)],
            ..Default::default()
        };
        let session = hitl_session(
            &node,
            vec![answered_pending(
                "call-1",
                "refund",
                json!({ "amount": 30 }),
            )],
        )
        .await;
        let (ctx, rx) = hitl_context(session.clone());

        let choices = vec![make_tool_choice("call-1", "refund", r#"{"amount":35}"#)];
        let done = node
            .handle_tool_calls(&[], &choices, None, ctx, true)
            .await
            .unwrap();
        assert!(done);

        let events = drain(rx);
        let input = events.iter().find_map(|e| match e {
            SSEvent::ToolInputAvailable { input, .. } => Some(input.clone()),
            _ => None,
        });
        assert_eq!(input, Some(json!({ "amount": 30 })));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SSEvent::RequestFeedback { .. }))
        );

        let s = session.read().await;
        let recorded = s.messages.iter().find_map(|m| match &m.content {
            FormaMessageBody::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                Some(flatten_tool_choice(&tool_calls[0]).unwrap().2)
            }
            _ => None,
        });
        assert_eq!(recorded, Some(json!({ "amount": 30 })));
    }

    /// An approval is for one call: another call to the same tool still
    /// needs its own.
    #[tokio::test]
    async fn test_an_approval_does_not_cover_other_calls_to_the_same_tool() {
        let node = Node {
            tools: vec![make_tool("refund", true)],
            ..Default::default()
        };
        let session = hitl_session(
            &node,
            vec![answered_pending(
                "call-1",
                "refund",
                json!({ "amount": 30 }),
            )],
        )
        .await;
        let (ctx, rx) = hitl_context(session.clone());

        let choices = vec![make_tool_choice("call-2", "refund", r#"{"amount":500}"#)];
        let done = node
            .handle_tool_calls(&[], &choices, None, ctx, true)
            .await
            .unwrap();
        assert!(!done, "call-2 must wait for its own approval");

        let events = drain(rx);
        assert!(events.iter().any(
            |e| matches!(e, SSEvent::RequestFeedback { tool_call_id, .. } if tool_call_id == "call-2")
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SSEvent::ToolInputAvailable { .. }))
        );
    }

    /// A delegated tool's result, or a rejection, answered while the session
    /// was suspended, reaches the conversation, and is reported so it is
    /// stored.
    #[tokio::test]
    async fn test_an_answer_given_while_suspended_reaches_the_conversation() {
        let node = Node {
            tools: vec![make_tool("refund", true)],
            ..Default::default()
        };
        let session = hitl_session(
            &node,
            vec![answered_pending(
                "call-1",
                "refund",
                json!({ "amount": 35 }),
            )],
        )
        .await;
        session.write().await.tool_calls_results.insert(
            "call-1".to_string(),
            CachedToolResult {
                tool_call_id: "call-1".to_string(),
                tool_name: "refund".to_string(),
                result: json!("rejected"),
                from_root: true,
            },
        );
        let (ctx, rx) = hitl_context(session.clone());

        let choices = vec![make_tool_choice("call-1", "refund", r#"{"amount":35}"#)];
        node.handle_tool_calls(&[], &choices, None, ctx, true)
            .await
            .unwrap();

        let events = drain(rx);
        assert!(events.iter().any(|e| matches!(
            e,
            SSEvent::ToolOutputAvailable { tool_call_id, output } if tool_call_id == "call-1" && *output == json!("rejected")
        )));
        let s = session.read().await;
        let kinds: Vec<&str> = s
            .messages
            .iter()
            .map(|m| match &m.content {
                FormaMessageBody::Assistant { .. } => "assistant",
                FormaMessageBody::Tool { .. } => "tool",
                _ => "other",
            })
            .collect();
        // The call, and right after it, its result.
        assert_eq!(kinds, vec!["assistant", "tool"]);
    }

    /// Calls that ran before the turn suspended are already stored, with
    /// their results: resuming must not run or record them again.
    #[tokio::test]
    async fn test_calls_answered_before_suspending_are_not_repeated() {
        let node = Node {
            tools: vec![make_tool("lookup", false), make_tool("refund", true)],
            ..Default::default()
        };
        let session =
            hitl_session(&node, vec![answered_pending("call-2", "refund", json!({}))]).await;
        session
            .write()
            .await
            .messages
            .push(FormaMessage::new_tool_with_content(
                "call-1",
                "found".to_string(),
            ));
        let (ctx, rx) = hitl_context(session.clone());

        let choices = vec![
            make_tool_choice("call-1", "lookup", "{}"),
            make_tool_choice("call-2", "refund", "{}"),
        ];
        node.handle_tool_calls(&[], &choices, None, ctx, true)
            .await
            .unwrap();

        let started: Vec<String> = drain(rx)
            .into_iter()
            .filter_map(|e| match e {
                SSEvent::ToolInputStart { tool_call_id, .. } => Some(tool_call_id),
                _ => None,
            })
            .collect();
        assert_eq!(started, vec!["call-2".to_string()]);
    }

    #[tokio::test]
    async fn test_handle_tool_calls_mixed_executes_non_confirm_skips_confirm() {
        let node = Node {
            tools: vec![
                make_tool("normal_tool", false),
                make_tool("confirm_tool", true),
            ],
            ..Default::default()
        };
        let db = NoneDB::new().await.unwrap();
        let session = db.new_session("agent_id", "user_id").await.unwrap();
        let session = Arc::new(RwLock::new(session));
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let choices = vec![
            make_tool_choice("normal_tool_call_id", "normal_tool", "{}"),
            make_tool_choice("confirm_tool_call_id", "confirm_tool", "{}"),
        ];
        let result = node.handle_tool_calls(&[], &choices, None, ctx, true).await;

        // Still returns None because a confirm tool was present
        assert!(result.is_ok());
        assert!(
            !result.unwrap(),
            "should return False when any confirm tool is present"
        );

        let mut events = vec![];
        rx.close();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }

        // RequestFeedback for the confirm tool
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SSEvent::RequestFeedback { tool_name, .. } if tool_name == "confirm_tool")),
            "RequestFeedback must be sent for the confirm tool"
        );

        // ToolInputAvailable for the non-confirm tool
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SSEvent::ToolInputAvailable { tool_name, .. } if tool_name == "normal_tool")),
            "ToolInputAvailable must be sent for the non-confirm tool"
        );

        // Non-confirm tool result must be registered in session
        let s = session.read().await;
        assert!(
            s.tool_calls_results.get("normal_tool_call_id").is_some(),
            "non-confirm tool result must be in session"
        );
        // Confirm tool result must NOT be registered
        assert!(
            s.tool_calls_results.get("confirm_tool_call_id").is_none(),
            "confirm tool must not have been called"
        );
    }

    #[tokio::test]
    async fn test_handle_verbal_no_schema_stores_parts_array() {
        use genai_client::openai::v1::OpenAITextPart;

        let node = Node {
            id: "n_plain".to_string(),
            ..Default::default()
        };

        let session = Arc::new(RwLock::new(SessionData::default()));
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let content = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "Hello, world!".to_string(),
            })],
            ..Default::default()
        };

        let result = node.emit_triage_response(&content.content, &ctx).await;
        assert!(result.is_ok());
    }

    /// Multiple confirm tools: RequestFeedback is sent for each one.
    #[tokio::test]
    async fn test_handle_tool_calls_multiple_confirm_sends_feedback_for_each() {
        let node = Node {
            tools: vec![make_tool("confirm_a", true), make_tool("confirm_b", true)],
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(SessionData::default()));
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let choices = vec![
            make_tool_choice("id1", "confirm_a", "{}"),
            make_tool_choice("id2", "confirm_b", "{}"),
        ];
        let result = node
            .handle_tool_calls(&[], &choices, None, ctx, false)
            .await;

        assert!(result.is_ok());
        let should_summarise = result.unwrap();
        assert!(
            !should_summarise,
            "should not summarise when confirm tools are present"
        );

        let mut events = vec![];
        rx.close();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SSEvent::RequestFeedback { tool_name, .. } if tool_name == "confirm_a")),
            "RequestFeedback must be sent for confirm_a"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SSEvent::RequestFeedback { tool_name, .. } if tool_name == "confirm_b")),
            "RequestFeedback must be sent for confirm_b"
        );
    }

    // ── Node::check ───────────────────────────────────────────────────────────

    fn make_template_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["hello".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        }
    }

    #[test]
    fn test_check_valid_node_no_tools() {
        let node = Node {
            id: "my_node".to_string(),
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_valid_node_with_tools() {
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![make_template_tool("tool_a"), make_template_tool("tool_b")],
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_node_invalid_id() {
        let node = Node {
            id: "1bad".to_string(),
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_node_empty_id() {
        let node = Node {
            id: String::new(),
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_node_summarization_prompt_without_tools_is_err() {
        let node = Node {
            id: "my_node".to_string(),
            summarization_prompt: Some("summarize this".to_string()),
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_node_summarization_prompt_with_tools_is_ok() {
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![make_template_tool("tool_a")],
            summarization_prompt: Some("summarize this".to_string()),
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_node_duplicate_tool_names() {
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![
                make_template_tool("dup_tool"),
                make_template_tool("dup_tool"),
            ],
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_node_duplicate_not_flagged_for_unique_names() {
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![make_template_tool("tool_a"), make_template_tool("tool_b")],
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_node_propagates_tool_error() {
        let bad_tool = Tool {
            name: "1bad_name".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["hello".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![bad_tool],
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_node_duplicate_detected_before_tool_validity() {
        // First tool is valid, second is a duplicate of the first (also valid by itself).
        // The duplicate check runs before per-tool check, so we get the duplicate error.
        let node = Node {
            id: "my_node".to_string(),
            tools: vec![make_template_tool("same"), make_template_tool("same")],
            ..Default::default()
        };
        let check = <Node as Callable<NoneDB>>::check(&node);
        assert!(check.is_err());
    }

    #[test]
    fn test_find_allowed_tool_not_found() {
        let node = Node::default();
        let err = node.find_allowed_tool("missing", &[]).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn test_find_allowed_tool_no_restriction() {
        let tool = make_tool("my_tool", false);
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let result = node.find_allowed_tool("my_tool", &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "my_tool");
    }

    #[test]
    fn test_find_allowed_tool_user_has_required_role() {
        let mut tool = make_tool("restricted_tool", false);
        tool.just_for = vec!["admin".to_string()];
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let result = node.find_allowed_tool("restricted_tool", &["admin".to_string()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "restricted_tool");
    }

    #[test]
    fn test_find_allowed_tool_user_lacks_required_role() {
        let mut tool = make_tool("restricted_tool", false);
        tool.just_for = vec!["admin".to_string()];
        let node = Node {
            tools: vec![tool],
            ..Default::default()
        };
        let err = node
            .find_allowed_tool("restricted_tool", &["guest".to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("not allowed"));
    }

    #[tokio::test]
    async fn test_join_tool_calls_empty() {
        let node = Node::default();
        let result = node.join_tool_calls(vec![]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_join_tool_calls_all_ok() {
        use protocols::FormaError;
        use tokio::task::JoinHandle;

        let node = Node::default();
        let handles: Vec<JoinHandle<Result<Option<Value>, FormaError>>> = vec![
            tokio::spawn(async { Ok(Some(Value::Null)) }),
            tokio::spawn(async { Ok(Some(Value::Null)) }),
        ];
        let result = node.join_tool_calls(handles).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_join_tool_calls_one_fails() {
        use protocols::FormaError;
        use tokio::task::JoinHandle;

        let node = Node::default();
        let handles: Vec<JoinHandle<Result<Option<Value>, FormaError>>> = vec![
            tokio::spawn(async { Ok(None) }),
            tokio::spawn(async { Err(FormaError::from("task failed".to_string())) }),
        ];
        let err = node.join_tool_calls(handles).await.unwrap_err();
        assert!(err.to_string().contains("task failed"));
    }

    #[tokio::test]
    async fn test_join_tool_calls_panicking_task() {
        use protocols::FormaError;
        use tokio::task::JoinHandle;

        let node = Node::default();
        let handles: Vec<JoinHandle<Result<Option<Value>, FormaError>>> =
            vec![tokio::spawn(async { panic!("boom") })];
        let result = node.join_tool_calls(handles).await;
        assert!(result.is_err());
    }

    // ── handle_pending_tool_calls ─────────────────────────────────────────────

    // fn make_pending_call(tool_name: &str) -> PendingToolCall {
    //     PendingToolCall {
    //         tool_call_id: "tc1".to_string(),
    //         node_id: "n1".to_string(),
    //         tool_name: tool_name.to_string(),
    //         arguments: serde_json::json!({}),
    //         approved: true,
    //     }
    // }

    // #[tokio::test]
    // async fn test_handle_pending_tool_calls_unknown_tool_returns_err() {
    //     let node = Node::default(); // no tools
    //     let session = Arc::new(RwLock::new(SessionData::default()));
    //     let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
    //     let ctx = CallContext {
    //         session,
    //         tx,
    //         job_id: "job".to_string(),
    //         user_roles: vec![],
    //     };

    //     let calls = vec![make_pending_call("nonexistent_tool")];
    //     let err = node
    //         .handle_pending_tool_calls(&calls, ctx, false)
    //         .await
    //         .unwrap_err();
    //     assert!(err.to_string().contains("nonexistent_tool"));
    // }

    // #[tokio::test]
    // async fn test_handle_pending_tool_calls_disallowed_tool_returns_err() {
    //     let mut tool = make_tool("admin_tool", false);
    //     tool.just_for = vec!["admin".to_string()];
    //     let node = Node {
    //         tools: vec![tool],
    //         ..Default::default()
    //     };
    //     let session = Arc::new(RwLock::new(SessionData::default()));
    //     let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
    //     let ctx = CallContext {
    //         session,
    //         tx,
    //         job_id: "job".to_string(),
    //         user_roles: vec!["guest".to_string()],
    //     };

    //     let calls = vec![make_pending_call("admin_tool")];
    //     let err = node
    //         .handle_pending_tool_calls(&calls, ctx, false)
    //         .await
    //         .unwrap_err();
    //     assert!(err.to_string().contains("not allowed"));
    // }

    // #[tokio::test]
    // async fn test_handle_pending_tool_calls_second_tool_unknown_returns_err() {
    //     // First tool is valid; second is not — error is caught before any tool runs.
    //     let node = Node {
    //         tools: vec![make_tool("tool_a", false)],
    //         ..Default::default()
    //     };
    //     let session = Arc::new(RwLock::new(SessionData::default()));
    //     let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
    //     let ctx = CallContext {
    //         session,
    //         tx,
    //         job_id: "job".to_string(),
    //         user_roles: vec![],
    //     };

    //     let calls = vec![make_pending_call("tool_a"), make_pending_call("missing")];
    //     let err = node
    //         .handle_pending_tool_calls(&calls, ctx, false)
    //         .await
    //         .unwrap_err();
    //     assert!(err.to_string().contains("missing"));
    // }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {
    use super::Callable;

    use crate::{CallContext, SSEvent, Tool, ToolRunner, tool::TemplateTool};
    use db::NoneDB;
    use protocols::{DeploymentCredentials, EncryptedTokens, FormaDB, SessionData};
    use std::{collections::HashMap, sync::Arc};

    use genai_client::{GenaiClient, Ollama};
    use tokio::sync::{Mutex, RwLock, mpsc};

    use super::Node;

    #[tokio::test]
    async fn test_execute_default_node() {
        // let template = r"Hello, {{world}}".to_string();
        let mut state = HashMap::new();
        state.insert(
            "world".to_string(),
            serde_json::Value::String("Snowy.".to_string()),
        );
        let node = Node {
            llm: GenaiClient::default(),
            system_prompt: "Behave like Pikachu and use emojies".to_string(),
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(SessionData {
            variables: state,
            messages: vec!["Hi!".to_string().into()],
            ..Default::default()
        }));
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let db = NoneDB::new().await.unwrap();
        let ctx = CallContext {
            user_timezone: None,
            agent_timezone: None,
            session,
            tx,
            job_id: "the-job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(db),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "some_user".into(),
        };
        node.run(ctx, true).await.expect("Failed to run node");
        while let Some(event) = rx.recv().await {
            // Handle SSE events
            let s = serde_json::to_string(&event).expect("Failed to serialize SSEvent");
            println!("data: {s}\n\n");
        }
    }
    #[tokio::test]
    async fn test_execute_default_node_with_tool() {
        // let template = r"Hello, {{world}}".to_string();
        let mut state = HashMap::new();
        state.insert(
            "world".to_string(),
            serde_json::Value::String("Snowy.".to_string()),
        );
        let tools = vec![Tool {
            just_for: vec![],
            name: "test_tool".to_string(),
            description: Some("Use this tool when anyone says 'hi'".to_string()),
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["{{var}}".to_string()],
            }),
            confirm_before_calling: false,
            bundle: None,
        }];
        let llm = Ollama::default();

        let node = Node {
            llm: GenaiClient::Ollama(llm),
            summarization_llm: GenaiClient::default(),
            system_prompt: "Behave like Pikachu and use emojies".to_string(),
            tools,
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(SessionData {
            variables: state,
            messages: vec!["Hi!".to_string().into()],
            ..Default::default()
        }));
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let db = NoneDB::new().await.unwrap();
        let ctx = CallContext {
            user_timezone: None,
            agent_timezone: None,
            session,
            tx,
            job_id: "the-job".to_string().into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            db: Arc::new(db),
            user_id: "some_user".into(),
        };
        node.run(ctx, true).await.expect("Failed to run node");
        while let Some(event) = rx.recv().await {
            // Handle SSE events
            let s = serde_json::to_string(&event).expect("Failed to serialize SSEvent");
            println!("data: {s}\n\n");
        }
    }
}

#[cfg(test)]
mod model_mistakes {
    use super::{LLMFinishReason, Node};
    use crate::{Tool, ToolRunner, tool::TemplateTool};
    use genai_client::openai::v1::{
        OpenAIFunctionToolChoice, OpenAIMessagePart, OpenAITextPart, OpenAIToolChoice,
    };
    use genai_client::{AssistantMessage, ChatResponse, LLMMessage, MessageChoice};

    fn node() -> Node {
        Node {
            id: "n".to_string(),
            tools: vec![Tool {
                name: "lookup".to_string(),
                description: None,
                tool: ToolRunner::Template(TemplateTool {
                    templates: vec!["ok".to_string()],
                }),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        }
    }

    fn response(msg: AssistantMessage, finish_reason: LLMFinishReason) -> ChatResponse {
        ChatResponse {
            choices: vec![MessageChoice {
                finish_reason,
                message: LLMMessage::Assistant(msg),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn calling(name: &str, arguments: &str) -> ChatResponse {
        let msg = AssistantMessage {
            tool_calls: vec![OpenAIToolChoice::Function {
                id: "c1".to_string(),
                function: OpenAIFunctionToolChoice {
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                },
            }],
            ..Default::default()
        };
        response(msg, LLMFinishReason::ToolCalls)
    }

    fn mistake(r: &ChatResponse) -> Option<String> {
        node().model_mistake(r, &[])
    }

    #[test]
    fn a_good_turn_is_no_mistake() {
        assert_eq!(mistake(&calling("lookup", r#"{"q": 1}"#)), None);
        let text = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "hola".to_string(),
            })],
            ..Default::default()
        };
        assert_eq!(mistake(&response(text, LLMFinishReason::Stop)), None);
    }

    #[test]
    fn botched_turns_are_mistakes() {
        let malformed = response(
            AssistantMessage::default(),
            LLMFinishReason::MalformedToolCall,
        );
        assert!(mistake(&malformed).unwrap().contains("malformed"));
        let empty = response(AssistantMessage::default(), LLMFinishReason::Stop);
        assert!(mistake(&empty).unwrap().contains("empty"));
        assert!(
            mistake(&ChatResponse::default())
                .unwrap()
                .contains("no choices")
        );
        assert!(
            mistake(&calling("lookup", "{\"q\": "))
                .unwrap()
                .contains("not valid JSON")
        );
        assert!(
            mistake(&calling("made_up", "{}"))
                .unwrap()
                .contains("made_up")
        );
    }

    /// Arguments of the wrong shape go back to the model as the tool's error,
    /// which says what to fix; they are not a reason to ask again blindly.
    #[test]
    fn wrong_but_valid_arguments_are_left_to_the_tool() {
        assert_eq!(mistake(&calling("lookup", r#"{"limit": "many"}"#)), None);
    }
}
