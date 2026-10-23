use agent::{CallContext, Callable};
use chrono::Utc;
use futures::stream::{BoxStream, Stream};

use protocols::{
    FormaDB, FormaError, FormaMessage, FormaMessageBody, FormaMessagePart, FormaTextPart,
    PendingToolCall, ResumeRequest, SessionData, session::UNANSWERED_TOOL_CALL_RESULT,
};
use serde_json::Value;
use std::{collections::HashSet, fmt::Debug, sync::Arc, time::Duration};
use tokio::sync::{
    RwLock,
    mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use tracing::instrument;

use crate::server::JobArgs;

use axum::{
    Json,
    http::StatusCode,
    response::sse::{Event, Sse},
};
use genai_client::{
    LLMMessage, SSEvent,
    events::UsageTally,
    openai::v1::{OpenAIFunctionToolChoice, OpenAIToolChoice},
};
use serde::{Deserialize, Serialize};

use crate::observability::{RunOutcome, UsageRecorder};
use tracing::{Instrument, error, info};
use uuid::Uuid;

pub(crate) mod ai_sdk_v5;
pub(crate) mod chat;
pub(crate) mod openai_stream;
pub(crate) mod openai_sync;
pub(crate) mod telegram;
pub(crate) mod whatsapp;

fn default_agent_message() -> FormaMessage {
    FormaMessage::new_assistant()
}

pub(crate) trait ClientSSE: Sized + Serialize + Send + 'static {
    fn translate(event: SSEvent) -> Option<Self>;
}

impl ClientSSE for SSEvent {
    fn translate(event: SSEvent) -> Option<Self> {
        Some(event)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(crate) struct NewSessionResult {
    session_id: String,
}

/// Creates a new session
#[instrument(skip(db))]
pub(crate) async fn create_session<DB: FormaDB>(
    db: Arc<DB>,
    agent_id: &str,
    user_id: &str,
) -> Result<Json<NewSessionResult>, (StatusCode, String)> {
    if user_id.is_empty() {
        info!("create_session called with empty user_id");
        return Err((
            StatusCode::BAD_REQUEST,
            "cannot create session with an empty user_id".to_string(),
        ));
    }

    if !db.persist() {
        return Ok(Json(NewSessionResult {
            session_id: "no-memory-agent;unnecessary-session-id".to_string(),
        }));
    }

    let session = db.new_session(agent_id, user_id).await.map_err(|e| {
        error!(agent_id, user_id, error = %e, "failed to create session in DB");
        e
    })?;
    let session_id = session.id_string()?;
    Ok(Json(NewSessionResult { session_id }))
}

pub(crate) async fn resume_stream<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: ClientSSE>(
    args: JobArgs<DB>,
    req: ResumeRequest,
) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, FormaError> {
    req.validate()?;
    if !args.db.persist() {
        return Err(FormaError::Input(
            "cannot resume a session that was not persisted".to_string(),
        ));
    }

    let still_pending = {
        let mut s = args.session.write().await;
        s.handle_resume(&req, &args.db).await?
    };
    let stream: BoxStream<'static, Result<Event, FormaError>> = if still_pending {
        let s = async_stream::stream! {
            if let Some(event) = O::translate(SSEvent::StillSuspended) {
                let data = serde_json::to_string(&event).unwrap();
                yield Ok(Event::default().data(data.as_str()));
            }
        };
        Box::pin(s)
    } else {
        let msg: Option<I> = None;

        agent_box_stream::<DB, I, O>(CallMode::ResumeJob, args, msg).await?
    };

    Ok(Sse::new(stream))
}

/// Calls the agent, and streams results using SSE
///
/// The message passed will be appended to the databse, if `state.settings.persist_sessions`
/// is true.
async fn stream<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: ClientSSE>(
    args: JobArgs<DB>,
    message: I,
) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, FormaError> {
    raw_stream::<DB, I, O>(CallMode::NewJob, args, Some(message)).await
}

/// Calls the agent, and streams results using SSE
///
/// The message passed will be appended to the databse, if `state.settings.persist_sessions`
/// is true.
async fn raw_stream<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: ClientSSE>(
    mode: CallMode,
    args: JobArgs<DB>,
    message: Option<I>,
) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, FormaError> {
    Ok(Sse::new(
        agent_box_stream::<DB, I, O>(mode, args, message).await?,
    ))
}

/// Inner implementation of [`raw_stream`] that returns a [`BoxStream`] so that
/// callers which need to unify multiple stream branches (e.g. [`resume_stream`])
/// can box all branches to a common type.
async fn agent_box_stream<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: ClientSSE>(
    mode: CallMode,
    args: JobArgs<DB>,
    message: Option<I>,
) -> Result<BoxStream<'static, Result<Event, FormaError>>, FormaError> {
    let session_id = args.session_id.to_string().clone();

    let (mut rx, session, claim) = prepare_agent_call(mode, args.clone(), message).await?;

    // Turn rx into an SSE stream
    let mut is_suspended = false;
    let stream = async_stream::stream! {
        // Held for as long as the stream lives; see `RunClaim`.
        let claim = claim;
        let mut finished = false;
        let mut responses = ResponseMessages::default();
        // Recorded on the way out, or when the stream is dropped mid-run: a
        // client that leaves, or a request timeout, drops it right here.
        let mut usage = UsageRecorder::new(mode.as_str());

        while let Some(msg) = rx.recv().await {
            // `is_suspended` must latch: once a turn suspends, trailing events
            // (e.g. `Finish`) must not flip it back to `false`, otherwise the
            // post-loop `clear_state` would wipe the just-persisted pending call.
            is_suspended |= match consume_event::<DB,FormaMessage>(
                &msg,
                &mut responses,
                &mut usage.tally,
                &session_id,
                &session,
                &args.db
            ).await {
                Ok(suspended) => suspended.is_some(),
                Err(e) => {
                    // A run that dies part way through has still burned every
                    // token that got it here, so the tally is reported before
                    // bailing out -- otherwise failures look free.
                    usage.finish(RunOutcome::Errored);
                    let event: Option<O> = O::translate(msg.clone());
                    if let Some(event) = event {
                        let s = serde_json::to_string(&event).unwrap();
                        yield Ok(Event::default().data(s.as_str()));
                    } else {
                        yield Err(FormaError::Execution(e.to_string()));
                    }
                    return;
                }
            };
            // we send this at the end
            if let SSEvent::Usage { .. } | SSEvent::ToolUsage { .. } = msg {
                continue;
            };
            // So is this: it is what tells a client it can send its next
            // message, which must find the turn stored and the session free.
            if let SSEvent::Finish = msg {
                finished = true;
                continue;
            }

            // Emit event if one is available
            let event: Option<O> = O::translate(msg);
            if let Some(event) = &event {
                let s = serde_json::to_string(event).unwrap();
                yield Ok(Event::default().data(s.as_str()));
            }

        }

        usage.finish(if is_suspended { RunOutcome::Suspended } else { RunOutcome::Completed });

        // Persist original message
        responses.persist(
            is_suspended,
            args.db.clone(),
            &session
        ).await?;
        if let Some(claim) = claim {
            claim.release().await;
        }

        if finished && let Some(event) = O::translate(SSEvent::Finish) {
            let s = serde_json::to_string(&event).unwrap();
            yield Ok(Event::default().data(s.as_str()));
        }
        let usage = serde_json::to_string(&usage.tally).unwrap();
        yield Ok(Event::default().data(usage.as_str()));
    };
    Ok(Box::pin(stream))
}

/// Builds a [`PendingToolCall`] from a `RequestFeedback` event, records the
/// corresponding `OpenAIToolChoice` in `ret_msg`, and—when `persist_session`
/// is true—suspends the session.
#[allow(clippy::too_many_arguments)]
async fn mark_as_pending<DB: FormaDB>(
    session: Arc<RwLock<SessionData>>,
    tool_name: String,
    arguments: Value,
    node_id: String,
    tool_call_id: String,
    from_root: bool,
    tool_type: String,
    bundle: Option<String>,
    db: &Arc<DB>,
) -> Result<PendingToolCall, FormaError> {
    let call = PendingToolCall {
        tool_name: tool_name.clone(),
        arguments: arguments.clone(),
        node_id,
        tool_call_id: tool_call_id.clone(),
        from_root,
        tool_type,
        bundle,
        ..Default::default()
    };
    if db.persist() {
        db.suspend_session(session.clone(), &call).await?;
    }
    Ok(call)
}

/// The messages a turn produces, in the order the agent produced them.
///
/// Stored order is what a reopened session is rebuilt from
/// (`front/src/lib/session_transform.ts` folds everything after a user message
/// into one bubble, in storage order), so text, tool calls and tool results
/// must be kept interleaved exactly as they streamed. Bucketing them by kind
/// put every tool call before any text on reload.
///
/// The shape follows OpenAI's: an assistant message holds content followed by
/// the calls it dispatched, and each result is a `tool` message after it. So
/// content that arrives after a call opens a new assistant message, while
/// consecutive texts stay as separate parts of the same one.
///
/// A call's result must follow it directly — Gemini refuses a history where
/// anything else sits between the two — so a file a tool sends while it runs
/// (a generated picture) is held until every call is answered, and goes after
/// the results.
#[derive(Debug, Clone, Default)]
struct ResponseMessages {
    messages: Vec<FormaMessage>,
    /// Files that arrived while calls were waiting for their results.
    held: Vec<FormaMessagePart>,
}

impl ResponseMessages {
    /// The assistant message whose calls are still waiting for a result, if
    /// any: nothing may be stored between it and those results.
    fn awaiting_results(&self) -> Option<usize> {
        let (at, calls) =
            self.messages
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, m)| match &m.content {
                    FormaMessageBody::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                        Some((i, tool_calls.len()))
                    }
                    _ => None,
                })?;
        let answered = self.messages[at + 1..]
            .iter()
            .filter(|m| matches!(m.content, FormaMessageBody::Tool { .. }))
            .count();
        (answered < calls).then_some(at)
    }

    /// The turn's messages as they are stored. A file still held — its turn
    /// stopped before every call was answered, waiting for an approval — goes
    /// with the calls it arrived during, where it cannot come between a call
    /// and its result, rather than being lost.
    fn settled(&self) -> Vec<FormaMessage> {
        let mut messages = self.messages.clone();
        if !self.held.is_empty() {
            match self.awaiting_results() {
                Some(at) => messages[at]
                    .content
                    .content_mut()
                    .extend(self.held.iter().cloned()),
                None => {
                    let mut message = default_agent_message();
                    message
                        .content
                        .content_mut()
                        .extend(self.held.iter().cloned());
                    messages.push(message);
                }
            }
        }
        messages
    }

    /// The assistant message new content should go into: the last message if
    /// it is an assistant that has not dispatched calls yet, otherwise a new one.
    fn content_message(&mut self) -> &mut FormaMessage {
        let reusable = matches!(
            self.messages.last().map(|m| &m.content),
            Some(FormaMessageBody::Assistant { tool_calls, .. }) if tool_calls.is_empty()
        );
        if !reusable {
            self.messages.push(default_agent_message());
        }
        self.messages.last_mut().expect("just ensured")
    }

    /// The assistant message a tool call belongs to: the last message if it is
    /// an assistant (parallel calls share one), otherwise a new one.
    fn tool_call_message(&mut self) -> &mut FormaMessage {
        let reusable = matches!(
            self.messages.last().map(|m| &m.content),
            Some(FormaMessageBody::Assistant { .. })
        );
        if !reusable {
            self.messages.push(default_agent_message());
        }
        self.messages.last_mut().expect("just ensured")
    }

    fn push_event(&mut self, event: &SSEvent) {
        match event {
            SSEvent::FileAvailable { part, .. } if self.awaiting_results().is_some() => {
                self.held.push(part.clone().into());
            }
            SSEvent::TextStart { .. } | SSEvent::FileAvailable { .. } => {
                *self.content_message() += event.clone();
            }
            // Continues the part its `TextStart` opened, which is in the last
            // assistant message.
            SSEvent::TextDelta { .. } => {
                let last = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m.content, FormaMessageBody::Assistant { .. }));
                match last {
                    Some(m) => *m += event.clone(),
                    None => *self.content_message() += event.clone(),
                }
            }
            // The start carries the bundle, and lands in the same message
            // as the input that follows it.
            SSEvent::ToolInputStart { .. } | SSEvent::ToolInputAvailable { .. } => {
                *self.tool_call_message() += event.clone();
            }
            SSEvent::ToolOutputAvailable {
                tool_call_id,
                output,
            } => {
                let mut out_msg = FormaMessage::new_tool();
                out_msg.content = FormaMessageBody::Tool {
                    tool_call_id: tool_call_id.to_string(),
                    content: vec![FormaMessagePart::Text(FormaTextPart {
                        text: output.to_string(),
                    })],
                };
                self.messages.push(out_msg);
                if self.awaiting_results().is_none() && !self.held.is_empty() {
                    let held = std::mem::take(&mut self.held);
                    self.content_message().content.content_mut().extend(held);
                }
            }
            _ => { /* nothing to store */ }
        }
    }

    /// Everything the agent said this turn, as the single reply a
    /// non-streaming client gets: all assistant content, in order, without the
    /// tool calls.
    fn response(&self) -> FormaMessage {
        let mut ret = default_agent_message();
        if let FormaMessageBody::Assistant { content, .. } = &mut ret.content {
            for m in &self.settled() {
                if let FormaMessageBody::Assistant { content: c, .. } = &m.content {
                    content.extend(c.iter().cloned());
                }
            }
        }
        ret
    }

    async fn persist<DB: FormaDB>(
        &self,
        is_suspended: bool,
        db: Arc<DB>,
        session: &Arc<RwLock<SessionData>>,
    ) -> Result<(), FormaError> {
        if !db.persist() {
            return Ok(());
        }
        let (session_id, user_id) = {
            let session = session.read().await;
            let session_id = session.id_string()?;
            let user_id = session.user_id.to_string();
            (session_id, user_id)
        };

        store_turn(is_suspended, &session_id, &user_id, self.settled(), &db).await?;
        Ok(())
    }
}

/// Stores the messages a turn produced, all in one go, and — unless the turn
/// is suspended, waiting for an answer — clears the state it ran with. The two
/// touch different documents, so they are sent together: storing each message
/// and clearing the state after each took two round trips per message.
async fn store_turn<DB: FormaDB>(
    is_suspended: bool,
    session_id: &str,
    user_id: &str,
    messages: Vec<FormaMessage>,
    db: &Arc<DB>,
) -> Result<Vec<FormaMessage>, FormaError> {
    let messages: Vec<FormaMessage> = messages.into_iter().filter(|m| !m.is_empty()).collect();
    let store = async {
        if messages.is_empty() {
            return Ok(vec![]);
        }
        db.push_msgs(session_id, user_id, messages)
            .await
            .map_err(|e| {
                error!(session_id, user_id, error = %e, "failed to persist messages to DB");
                e
            })
    };
    let clear = async {
        if is_suspended {
            return Ok(());
        }
        db.clear_state(session_id).await.map_err(|e| {
            error!(session_id, error = %e, "failed to clear session state in DB");
            e
        })
    };
    let (saved, ()) = tokio::try_join!(store, clear)?;
    Ok(saved)
}

/// A result paired with what producing it cost.
///
/// Kept alongside the result rather than inside it because spend is orthogonal
/// to outcome: a run that suspends waiting for a human, or that is rejected,
/// has already burned the tokens that got it there and is billed for them.
/// Wrapping also leaves `CompletionResult`'s wire format untouched, which
/// matters because the OpenAI-compatible adapter serialises it to clients.
pub(crate) struct Metered<T> {
    pub result: T,
    pub usage: UsageTally,
}

/// Result from a Completion, indicating whether the
/// request was completed or has been suspended because
/// human feedback was needed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "data")]
pub(crate) enum CompletionResult<O> {
    Completed(O),
    Suspended {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        arguments: Value,
    },
}

/// Result from a Resume, indicating whether the
/// request was completed, whether the resumed run stopped at a new call
/// that needs an answer, or whether the session is still
/// suspended (other calls are waiting for an answer).
///
/// A rejected call is completed like any other: the rejection becomes the
/// call's result, and the agent answers with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "data")]
pub(crate) enum ResumeResult<O> {
    Completed(O),
    Suspended {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        arguments: Value,
    },
    StillSuspended,
}

/// Resumes a suspended completion, updating the session state and returning
/// the result.
pub(crate) async fn resume_completion<DB: FormaDB, O: From<LLMMessage>>(
    args: JobArgs<DB>,
    req: ResumeRequest,
) -> Result<Metered<ResumeResult<O>>, FormaError> {
    req.validate()?;
    if !args.db.persist() {
        return Err(FormaError::Input(
            "cannot resume a session that was not persisted".to_string(),
        ));
    }

    let still_pending = {
        let mut s = args.session.write().await;
        s.handle_resume(&req, &args.db).await?
    };

    // This does not run the agent, so it spends nothing.
    if still_pending {
        return Ok(Metered {
            result: ResumeResult::StillSuspended,
            usage: UsageTally::new_empty(),
        });
    }

    let msg: Option<FormaMessage> = None;
    let r = raw_completion(CallMode::ResumeJob, args, msg).await?;
    let result = match r.result {
        CompletionResult::Completed(o) => ResumeResult::Completed(o),
        CompletionResult::Suspended {
            session_id,
            tool_call_id,
            tool_name,
            arguments,
        } => ResumeResult::Suspended {
            session_id,
            tool_call_id,
            tool_name,
            arguments,
        },
    };
    Ok(Metered {
        result,
        usage: r.usage,
    })
}

/// Calls the agent, and returns a single return JSON
///
/// The message passed will be appended to the databse, if `state.settings.persist_sessions`
/// is true.
async fn completion<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: From<LLMMessage>>(
    args: JobArgs<DB>,
    message: I,
) -> Result<Metered<CompletionResult<O>>, FormaError> {
    raw_completion(CallMode::NewJob, args, Some(message)).await
}

async fn consume_event<DB: FormaDB, O>(
    msg: &SSEvent,
    responses: &mut ResponseMessages,
    usage: &mut UsageTally,
    session_id: &str,
    session: &Arc<RwLock<SessionData>>,
    db: &Arc<DB>,
) -> Result<Option<CompletionResult<O>>, FormaError> {
    // Tallied before the match rather than as an arm of it, because usage is
    // orthogonal to what an event means for the conversation: the run's spend
    // has to be reported whether it completed, suspended, or errored.
    usage.add(msg);

    let mut suspended_msg = None;
    match &msg {
        SSEvent::Error { error } => {
            return Err(FormaError::Execution(error.to_string()));
        }
        SSEvent::RequestFeedback {
            tool_name,
            arguments,
            node_id,
            tool_call_id,
            from_root,
            tool_type,
            bundle,
        } => {
            mark_as_pending(
                session.clone(),
                tool_name.to_string(),
                arguments.clone(),
                node_id.to_string(),
                tool_call_id.to_string(),
                *from_root,
                tool_type.clone(),
                bundle.clone(),
                db,
            )
            .await?;
            suspended_msg = Some(CompletionResult::Suspended {
                session_id: session_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.clone(),
            });
        }
        _ => responses.push_event(msg),
    }
    Ok(suspended_msg)
}

/// Calls the agent, and returns a single return JSON
///
/// The message passed will be appended to the databse, if `state.settings.persist_sessions`
/// is true.
async fn raw_completion<DB: FormaDB, I: Into<FormaMessage> + Serialize, O: From<LLMMessage>>(
    mode: CallMode,
    args: JobArgs<DB>,
    message: Option<I>,
) -> Result<Metered<CompletionResult<O>>, FormaError> {
    let session_id = args.session_id.clone();
    let (mut rx, session, claim) = prepare_agent_call(mode, args.clone(), message).await?;
    let mut responses = ResponseMessages::default();
    let mut usage = UsageRecorder::new(mode.as_str());
    let mut suspended_msg: Option<CompletionResult<O>> = None;

    let db = &args.db;
    while let Some(msg) = rx.recv().await {
        match consume_event(
            &msg,
            &mut responses,
            &mut usage.tally,
            &session_id,
            &session,
            db,
        )
        .await
        {
            // `suspended_msg` must latch, exactly as `is_suspended` does in
            // `agent_box_stream`: once a turn suspends, trailing events (e.g.
            // `Finish`) must not clear it. A plain assignment here meant a
            // suspended run reported as completed, so `persist` went on to call
            // `clear_state` and wiped the pending call `mark_as_pending` had
            // just written -- the approval never got back to the client.
            Ok(suspended) => {
                if suspended.is_some() {
                    suspended_msg = suspended;
                }
            }
            Err(e) => {
                // As in `agent_box_stream`: report what the run spent
                // before it died, then propagate.
                usage.finish(RunOutcome::Errored);
                return Err(e);
            }
        }
    }

    usage.finish(if suspended_msg.is_some() {
        RunOutcome::Suspended
    } else {
        RunOutcome::Completed
    });

    responses
        .persist(suspended_msg.is_some(), args.db.clone(), &session)
        .await?;
    if let Some(claim) = claim {
        claim.release().await;
    }

    let result = match suspended_msg {
        Some(s) => s,
        None => {
            let ret = LLMMessage::try_from(responses.response())?;
            CompletionResult::Completed(ret.into())
        }
    };
    Ok(Metered {
        result,
        usage: std::mem::take(&mut usage.tally),
    })
}

/// The mode of the agent call, either resuming a job or starting a new one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum CallMode {
    /// Resume an existing job
    ResumeJob,
    /// Start a new job
    NewJob,
}

impl CallMode {
    /// Label form for metrics. Closed set, so it is safe as a dimension.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CallMode::ResumeJob => "resume",
            CallMode::NewJob => "new",
        }
    }
}

/// The message passed will be appended to the session. If `state.settings.persist_sessions`
/// is false, the session utilized will contain only the message
/// passed to it.
pub(crate) async fn prepare_agent_call<DB: FormaDB, I: Into<FormaMessage> + Serialize>(
    mode: CallMode,
    args: JobArgs<DB>,
    message: Option<I>,
) -> Result<
    (
        UnboundedReceiver<SSEvent>,
        Arc<RwLock<SessionData>>,
        Option<RunClaim<DB>>,
    ),
    FormaError,
> {
    // Assign an id to this run
    let job_id = Uuid::new_v4().to_string();
    let deployment_id = args.deployment_id.as_deref().unwrap_or("<none>");
    let root_span = tracing::info_span!(
        "FormaJob",
        job_id = %job_id,
        session_id = %args.session_id,
        deployment_id = %deployment_id,
        agent_id = %args.agent_id,
        user_id = %args.user_id,
        input=serde_json::to_string(&message).unwrap(),
    );

    let message = match message {
        None => None,
        Some(message) => {
            // Inbound messages arrive in Forma's own shape; turning them into
            // what a provider is sent happens later, once every part has been
            // resolved.
            let message: FormaMessage = message.into();
            Some(message)
        }
    };

    // Everything here comes before the message is stored: a refused message
    // would otherwise sit in the history as if the agent had been asked it.
    let claim = RunClaim::take(&args).await?;

    // A channel that redelivers gives each message its own id, and a message
    // already stored under it has been answered (or is being answered).
    // Checked while holding the session, so two deliveries of the same
    // message cannot both get past it.
    if let Some(message_id) = message.as_ref().and_then(|m| m.id.as_ref())
        && args.db.persist()
        && args.db.has_message(&args.session_id, &message_id.0).await?
    {
        if let Some(claim) = claim {
            claim.release().await;
        }
        return Err(FormaError::DuplicateMessage(message_id.to_string()));
    }

    // A new message while the session waits for an answer (an approval, or a
    // delegated tool's result) moves the conversation on: whatever is waiting
    // is rejected, and the agent answers the new message knowing it.
    if mode == CallMode::NewJob && args.session.read().await.is_pending() {
        close_suspended_turn(&args).await?;
    }

    args.append_msg(message).await?;

    // Build communication channels to stream to client
    let (tx, rx) = mpsc::unbounded_channel::<SSEvent>();
    // Retrieve session data
    // TODO: Should distinguish between "not found" and "error"
    info!(target: "agent","starting job {}", job_id);
    tx.send(SSEvent::Start {
        message_id: job_id.clone(),
    })?;

    let ret_session = args.session.clone();
    // On the server's own runtime, whose threads have room for the deep
    // future chain an agent builds (see `forma::runtime`). It used to get an
    // OS thread and a runtime of its own per request, which cost a thread
    // start and a runtime build every time, and meant that nothing built
    // while it ran — a pooled connection, a provider's client — outlived the
    // request. Boxed so the future goes straight to the heap rather than
    // being moved across the stack on its way into the task.
    tokio::spawn(Box::pin(
        call_agent(args.session.clone(), job_id, args, tx).instrument(root_span),
    ));
    Ok((rx, ret_session, claim))
}

/// Ends the turn a session is suspended on, so a new message can start the
/// next one: every call the turn left waiting is rejected, and the suspended
/// state is cleared.
///
/// Must be called while holding the session ([`RunClaim`]).
async fn close_suspended_turn<DB: FormaDB>(args: &JobArgs<DB>) -> Result<(), FormaError> {
    // Read again now that this run holds the session: the copy the request
    // started with may predate an answer that has resumed it since, or the
    // end of the turn that suspended it.
    let fresh = args
        .db
        .get_session_by_id(&args.session_id, args._max_history_messages)
        .await?;

    let mut session = args.session.write().await;
    session.messages = fresh.messages;
    session.pending_tool_calls = fresh.pending_tool_calls;
    session.cached_messages = fresh.cached_messages;
    session.tool_calls_results = fresh.tool_calls_results;
    if !session.is_pending() {
        return Ok(());
    }

    info!(
        session_id = %args.session_id,
        "a new message rejects the calls the session was waiting on"
    );
    let closing = closing_messages(&session.messages, &session.pending_tool_calls);
    let stored = store_turn(false, &args.session_id, &args.user_id, closing, &args.db).await?;
    session.messages.extend(stored);
    session.pending_tool_calls.clear();
    session.cached_messages.clear();
    session.tool_calls_results.clear();
    Ok(())
}

/// What closes a suspended turn: a rejection for every call it left without
/// a result, each right after its call, as Gemini requires.
///
/// Some of those calls are in the history already: they ran, but wait on
/// something inside them (a workflow tool whose inner node needs an
/// approval). The rest never reached it, because they were waiting to be
/// approved before running; they are recorded first, so the model knows what
/// it proposed, and that it did not happen.
fn closing_messages(history: &[FormaMessage], pending: &[PendingToolCall]) -> Vec<FormaMessage> {
    // Only the last turn: anything before its user message was settled.
    let turn_start = history
        .iter()
        .rposition(|m| matches!(m.content, FormaMessageBody::User { .. }))
        .map_or(0, |i| i + 1);
    let turn = &history[turn_start..];

    let answered: HashSet<&str> = turn
        .iter()
        .filter_map(|m| match &m.content {
            FormaMessageBody::Tool { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    let called: Vec<String> = turn
        .iter()
        .flat_map(|m| match &m.content {
            FormaMessageBody::Assistant { tool_calls, .. } => {
                tool_calls.iter().map(OpenAIToolChoice::id).collect()
            }
            _ => vec![],
        })
        .collect();

    let mut messages: Vec<FormaMessage> = called
        .iter()
        .filter(|id| !answered.contains(id.as_str()))
        .map(|id| unanswered_call_result(id))
        .collect();

    let unrecorded: Vec<&PendingToolCall> = pending
        .iter()
        .filter(|call| call.from_root && !called.contains(&call.tool_call_id))
        .collect();
    if !unrecorded.is_empty() {
        let mut proposed = FormaMessage::new_assistant();
        proposed.content = FormaMessageBody::Assistant {
            content: vec![],
            tool_calls: unrecorded
                .iter()
                .map(|call| OpenAIToolChoice::Function {
                    id: call.tool_call_id.clone(),
                    function: OpenAIFunctionToolChoice {
                        name: call.tool_name.clone(),
                        arguments: call.arguments.to_string(),
                    },
                })
                .collect(),
            thought_signature: None,
            tool_bundles: unrecorded
                .iter()
                .filter_map(|call| Some((call.tool_call_id.clone(), call.bundle.clone()?)))
                .collect(),
            tool_types: unrecorded
                .iter()
                .filter(|call| !call.tool_type.is_empty())
                .map(|call| (call.tool_call_id.clone(), call.tool_type.clone()))
                .collect(),
        };
        messages.push(proposed);
        messages.extend(
            unrecorded
                .iter()
                .map(|call| unanswered_call_result(&call.tool_call_id)),
        );
    }
    messages
}

fn unanswered_call_result(tool_call_id: &str) -> FormaMessage {
    let mut message = FormaMessage::new_tool();
    message.content = FormaMessageBody::Tool {
        tool_call_id: tool_call_id.to_string(),
        content: vec![FormaMessagePart::Text(FormaTextPart {
            text: UNANSWERED_TOOL_CALL_RESULT.to_string(),
        })],
    };
    message
}

/// How long a message waits for the run before it to finish (see
/// [`in_turn`]): enough for most answers, and short enough not to hold a
/// request (and the instance serving it) for long.
const TURN_WAIT: Duration = Duration::from_secs(15);

/// Runs `attempt` once the session is free, so that a message arriving while
/// the agent is still answering is answered right after, instead of being
/// refused as [`FormaError::SessionBusy`].
///
/// For webhooks, whose platform redelivers what is refused, but at a time of
/// its choosing (Meta retries at once, then with growing gaps). `attempt` is
/// called again from scratch each time, so that it reads the session as the
/// run before it left it. After [`TURN_WAIT`], the session is reported busy
/// after all, and delivering the message again is up to the platform.
pub(crate) async fn in_turn<T, F, Fut>(mut attempt: F) -> Result<T, FormaError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, FormaError>>,
{
    let deadline = tokio::time::Instant::now() + TURN_WAIT;
    let mut pause = Duration::from_millis(200);
    loop {
        match attempt().await {
            Err(FormaError::SessionBusy(session_id))
                if tokio::time::Instant::now() + pause < deadline =>
            {
                info!(session_id, "waiting for the run before this message");
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(2));
            }
            result => return result,
        }
    }
}

/// How long a run's claim on its session lasts. Matches the channel service's
/// request timeout (`infra/forma-channels.tf`): no run outlives the request
/// carrying it, so an older claim belongs to a run that died without releasing
/// it — a crashed instance — and is taken over.
const RUN_CLAIM_TTL: chrono::TimeDelta = chrono::TimeDelta::hours(1);

/// A run's claim on its session, so that no second run starts on the same
/// history until it ends. Released when dropped, which covers every way a run
/// ends: completed, suspended, errored, or its stream dropped part way.
pub(crate) struct RunClaim<DB: FormaDB> {
    db: Arc<DB>,
    session_id: String,
}

impl<DB: FormaDB> RunClaim<DB> {
    /// Claims the session for a new run, or fails with
    /// [`FormaError::SessionBusy`] while another run holds it. Sessions that
    /// are not stored have no shared history, and are never claimed.
    async fn take(args: &JobArgs<DB>) -> Result<Option<Self>, FormaError> {
        if !args.db.persist() {
            return Ok(None);
        }
        let session_id = args.session_id.to_string();
        if !args
            .db
            .start_run(&session_id, Utc::now() - RUN_CLAIM_TTL)
            .await?
        {
            info!(
                session_id,
                "refusing a run on a session another run is working on"
            );
            return Err(FormaError::SessionBusy(session_id));
        }
        Ok(Some(Self {
            db: args.db.clone(),
            session_id,
        }))
    }

    /// Gives the session back now, rather than when dropped: a run that has
    /// stored its turn releases it before its client hears the turn is over,
    /// so a client that answers at once does not find the session still
    /// claimed. A failure is logged, as one when dropped would be; the claim
    /// then expires.
    async fn release(mut self) {
        let session_id = std::mem::take(&mut self.session_id);
        if let Err(e) = self.db.finish_run(&session_id).await {
            error!(session_id, error = %e, "failed to release the run's claim on the session");
        }
    }
}

impl<DB: FormaDB> Drop for RunClaim<DB> {
    fn drop(&mut self) {
        // Already released.
        if self.session_id.is_empty() {
            return;
        }
        // Drop cannot wait, so the release runs on its own. Without a runtime
        // to run it on, the claim is left to expire.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            error!(
                session_id = self.session_id,
                "no runtime to release the run's claim on"
            );
            return;
        };
        let db = self.db.clone();
        let session_id = std::mem::take(&mut self.session_id);
        runtime.spawn(async move {
            if let Err(e) = db.finish_run(&session_id).await {
                error!(session_id, error = %e, "failed to release the run's claim on the session");
            }
        });
    }
}

/// Calls the agent
async fn call_agent<DB: FormaDB>(
    session: Arc<RwLock<SessionData>>,
    job_id: String,
    args: JobArgs<DB>,
    tx: UnboundedSender<SSEvent>,
) {
    let ctx = CallContext {
        user_timezone: args.user_timezone.clone(),
        agent_timezone: args.agent_timezone.clone(),
        session: session.clone(),
        tx: tx.clone(),
        job_id: job_id.clone().into(),
        user_roles: args.roles.clone(),
        user_credentials: args.user_credentials.clone(),
        db: args.db.clone(),
        user_id: args.user_id.clone(),
        agent_id: args.agent_id.clone(),
        deployment_id: args.deployment_id.clone(),
        deployment_credentials: args.deployment_credentials.clone(),
    };

    #[cfg(feature = "hot-reload")]
    let agent = args.agent.read().await;
    #[cfg(not(feature = "hot-reload"))]
    let agent = &args.agent;

    let start = Utc::now();
    match agent.run(ctx, true).await {
        Ok(_) => {
            let end = Utc::now();
            let duration = (end - start).as_seconds_f32();
            info!(target: "agent","done running job {} in {:.1} seconds", job_id, duration);
            tx.send(SSEvent::Finish).unwrap();
        }
        Err(e) => {
            tx.send(SSEvent::Error {
                error: e.to_string(),
            })
            .unwrap();
            error!("{}: {}", job_id, e);
        }
    }
}

#[cfg(all(test, any(not(feature = "multi-tenant"), feature = "cli")))]
mod testing {

    use db::{mongo::MongoDB, none::NoneDB};
    use protocols::DeploymentCredentials;
    #[cfg(feature = "hot-reload")]
    use tokio::sync::RwLock;

    use crate::server::state::AppState;

    use super::*;
    use agent::Agent;
    use genai_client::Role;

    // ── helpers ──────────────────────────────────────────────────────────────

    async fn make_app_state<DB: FormaDB>(agent_id: &str) -> AppState<DB> {
        let agent = Agent {
            id: agent_id.to_string(),
            ..Default::default()
        };
        #[cfg(feature = "hot-reload")]
        let agent = RwLock::new(agent);
        let agent = Arc::new(agent);

        let db = DB::new().await.unwrap();
        AppState {
            agent,
            credentials: DeploymentCredentials::default().into(),
            db: Arc::new(db),
        }
    }

    // ── existing tests ────────────────────────────────────────────────────────

    #[test]
    fn test_ret_message() {
        let r = default_agent_message();
        assert_eq!(r.content.role(), Role::Assistant);
    }

    #[test]
    fn test_translate_sse() {
        let s = SSEvent::Finish;
        let e = SSEvent::translate(s);
        assert!(e.is_some());
        if let SSEvent::Finish = e.unwrap() {
        } else {
            panic!("should have been SSEvent::Finish")
        };
    }

    // ── NewSessionResult ──────────────────────────────────────────────────────

    #[test]
    fn test_new_session_result_default_is_empty() {
        let r = NewSessionResult::default();
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["session_id"], "");
    }

    #[test]
    fn test_new_session_result_serde_round_trip() {
        let json = serde_json::json!({"session_id": "abc123"});
        let r: NewSessionResult = serde_json::from_value(json.clone()).unwrap();
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back, json);
    }

    // ── CompletionResult ──────────────────────────────────────────────────────

    #[test]
    fn test_completion_result_completed_serializes_with_status_tag() {
        let r: CompletionResult<String> = CompletionResult::Completed("hello".to_string());
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "completed");
        assert_eq!(json["data"], "hello");
    }

    #[test]
    fn test_completion_result_suspended_serializes_with_status_tag() {
        let r: CompletionResult<String> = CompletionResult::Suspended {
            session_id: "s1".to_string(),
            tool_call_id: "tc1".to_string(),
            tool_name: "my_tool".to_string(),
            arguments: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "suspended");
        assert_eq!(json["data"]["session_id"], "s1");
        assert_eq!(json["data"]["tool_name"], "my_tool");
    }

    #[test]
    fn test_completion_result_serde_round_trip() {
        let original: CompletionResult<serde_json::Value> =
            CompletionResult::Completed(serde_json::json!(42));
        let json = serde_json::to_value(&original).unwrap();
        let back: CompletionResult<serde_json::Value> = serde_json::from_value(json).unwrap();
        match back {
            CompletionResult::Completed(v) => assert_eq!(v, serde_json::json!(42)),
            _ => panic!("expected Completed"),
        }
    }

    // ── ResumeResult ──────────────────────────────────────────────────────────

    #[test]
    fn test_resume_result_completed_status_tag() {
        let r: ResumeResult<String> = ResumeResult::Completed("done".to_string());
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "completed");
        assert_eq!(json["data"], "done");
    }

    #[test]
    fn test_resume_result_still_suspended_status_tag() {
        let r: ResumeResult<String> = ResumeResult::StillSuspended;
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "still_suspended");
        assert!(json.get("data").is_none());
    }

    #[test]
    fn test_resume_result_suspended_reads_like_a_suspended_completion() {
        // A resumed run that stops at a new call says which, as /v1/chat does.
        let r: ResumeResult<String> = ResumeResult::Suspended {
            session_id: "s".into(),
            tool_call_id: "call-2".into(),
            tool_name: "issue-refund".into(),
            arguments: serde_json::json!({"amount": 12}),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "suspended");
        assert_eq!(json["data"]["tool_call_id"], "call-2");
    }

    // ── ResumeRequest ─────────────────────────────────────────────────────────

    #[test]
    fn test_resume_request_serde_round_trip() {
        let req = ResumeRequest {
            tool_call_id: "tc42".to_string(),
            reject: false,
            arguments: Some(serde_json::json!({"foo": "bar"})),
            result: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        let back: ResumeRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.tool_call_id, "tc42");
        assert!(!back.reject);
        assert_eq!(back.arguments.as_ref().unwrap()["foo"], "bar");
        assert!(back.result.is_none());
    }

    #[test]
    fn test_resume_request_reject_true() {
        let json = serde_json::json!({
            "tool_call_id": "tc1",
            "reject": true,
            "arguments": null,
            "result": null
        });
        let req: ResumeRequest = serde_json::from_value(json).unwrap();
        assert!(req.reject);
    }

    // ── ResumeRequest::validate ───────────────────────────────────────────────

    #[test]
    fn test_validate_empty_tool_call_id_returns_error() {
        let req = ResumeRequest {
            tool_call_id: "".to_string(),
            reject: false,
            arguments: Some(serde_json::json!({})),
            result: None,
        };
        let err = req.validate().unwrap_err();
        assert!(matches!(err, FormaError::Input(msg) if msg.contains("tool_call_id")));
    }

    #[test]
    fn test_validate_reject_true_is_always_ok() {
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: true,
            arguments: None,
            result: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_reject_true_ignores_missing_arguments_and_result() {
        // reject=true short-circuits before checking arguments/result
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: true,
            arguments: None,
            result: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_both_none_returns_error() {
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: false,
            arguments: None,
            result: None,
        };
        let err = req.validate().unwrap_err();
        assert!(
            matches!(err, FormaError::Input(msg) if msg.contains("arguments or result is required"))
        );
    }

    #[test]
    fn test_validate_both_some_returns_error() {
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: false,
            arguments: Some(serde_json::json!({"a": 1})),
            result: Some(serde_json::json!({"b": 2})),
        };
        let err = req.validate().unwrap_err();
        assert!(matches!(err, FormaError::Input(msg) if msg.contains("cannot be both present")));
    }

    #[test]
    fn test_validate_only_arguments_is_ok() {
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: false,
            arguments: Some(serde_json::json!({"x": 42})),
            result: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_only_result_is_ok() {
        let req = ResumeRequest {
            tool_call_id: "tc1".to_string(),
            reject: false,
            arguments: None,
            result: Some(serde_json::json!("done")),
        };
        assert!(req.validate().is_ok());
    }

    // ── CallMode ──────────────────────────────────────────────────────────────

    #[test]
    fn test_call_mode_equality() {
        assert_eq!(CallMode::NewJob, CallMode::NewJob);
        assert_eq!(CallMode::ResumeJob, CallMode::ResumeJob);
        assert_ne!(CallMode::NewJob, CallMode::ResumeJob);
    }

    #[test]
    fn test_call_mode_is_copy() {
        let m = CallMode::NewJob;
        let m2 = m; // copy
        assert_eq!(m, m2);
    }

    #[test]
    fn test_call_mode_debug() {
        assert_eq!(format!("{:?}", CallMode::NewJob), "NewJob");
        assert_eq!(format!("{:?}", CallMode::ResumeJob), "ResumeJob");
    }

    // #[tokio::test]
    // async fn test_create_session_empty_user_id() {
    //     // Should fail whether persist or not
    //     let state =
    //         make_app_state::<MongoDBClient>(&format!("test-create-no-persist-{}", Uuid::new_v4()))
    //             .await;
    //     let result = create_session(&state, "").await;
    //     assert!(result.is_err());
    //     let state =
    //         make_app_state::<NoneDB>(&format!("test-create-no-persist-{}", Uuid::new_v4())).await;
    //     let result = create_session(&state, "").await;
    //     assert!(result.is_err());
    // }

    // #[tokio::test]
    // async fn test_create_session_no_persist_returns_dummy_id() {
    //     let state =
    //         make_app_state::<NoneDB>(&format!("test-create-no-persist-{}", Uuid::new_v4())).await;
    //     let result = create_session(&state, "user1").await;
    //     assert!(result.is_ok());
    //     let Json(body) = result.unwrap();
    //     let json = serde_json::to_value(&body).unwrap();
    //     assert_eq!(json["session_id"], "no-memory-agent;unnecessary-session-id");
    // }

    // // ── resume_completion (no-persist early return) ───────────────────────────

    // #[tokio::test]
    // async fn test_resume_completion_no_persist_returns_error() {
    //     let state =
    //         make_app_state::<NoneDB>(&format!("test-resume-compl-{}", Uuid::new_v4())).await;
    //     let req = ResumeRequest {
    //         tool_call_id: "tc1".to_string(),
    //         reject: false,
    //         arguments: Some(serde_json::json!({})),
    //         result: None,
    //     };
    //     let args = JobArgs {
    //         n_messages: 123,
    //         session_id: Arc::from("session"),
    //         user_id: Arc::from("user"),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //     };
    //     let result = resume_completion::<NoneDB, serde_json::Value>(args, req).await;
    //     assert!(result.is_err());
    //     let err = result.unwrap_err().to_string();
    //     assert!(err.contains("not persisted"));
    // }

    // #[tokio::test]
    // async fn test_get_session_no_persist_no_message() {
    //     let agent_id = format!("test-get-sess-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("any-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let session = args.append_msg(None).await.unwrap();
    //     assert_eq!(session.user_id, "user1");
    //     assert_eq!(session.agent_id, agent_id);
    //     assert!(session.messages.is_empty());
    // }

    // #[tokio::test]
    // async fn test_get_session_no_persist_with_message() {
    //     let agent_id = format!("test-get-sess-msg-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let msg = FormaMessage::new_user();
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("any-id"),
    //         user_id: Arc::from("user2"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let session = args.append_msg(Some(msg)).await.unwrap();
    //     assert_eq!(session.user_id, "user2");
    //     assert_eq!(session.messages.len(), 1);
    // }

    // ── record_tool_call ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_record_tool_call_returns_correct_fields() {
        let session = Arc::new(RwLock::new(SessionData::default()));
        let args = serde_json::json!({"x": 1});
        let db = Arc::new(NoneDB::default());

        let call = mark_as_pending(
            session,
            "my_tool".to_string(),
            args.clone(),
            "node1".to_string(),
            "tc99".to_string(),
            true,
            "mailgun".to_string(),
            None,
            &db,
        )
        .await
        .unwrap();

        assert_eq!(call.tool_name, "my_tool");
        assert_eq!(call.tool_call_id, "tc99");
        assert_eq!(call.arguments, args);
        assert_eq!(call.tool_type, "mailgun");
        assert_eq!(call.bundle, None);
    }

    // ── consume_event ─────────────────────────────────────────────────────────

    /// Guards the premise behind the `suspended_msg` latch in `raw_completion`
    /// and the `is_suspended |=` latch in `agent_box_stream`: a suspension is
    /// reported by exactly one event, and any event after it reports `None`.
    /// Assigning that `None` over the suspension is what made a suspended run
    /// look completed, which sent `persist` on to `clear_state` and deleted the
    /// pending tool call.
    #[tokio::test]
    async fn test_consume_event_reports_suspension_only_on_the_feedback_event() {
        let session = Arc::new(RwLock::new(SessionData::default()));
        let db = Arc::new(NoneDB::default());
        let mut responses = ResponseMessages::default();
        let mut usage = UsageTally::new();

        let feedback = SSEvent::RequestFeedback {
            tool_call_id: "tc1".to_string(),
            node_id: "node1".to_string(),
            tool_name: "my_tool".to_string(),
            arguments: serde_json::json!({"x": 1}),
            from_root: true,
            tool_type: String::new(),
            bundle: None,
        };
        let suspended = consume_event::<NoneDB, FormaMessage>(
            &feedback,
            &mut responses,
            &mut usage,
            "session-1",
            &session,
            &db,
        )
        .await
        .unwrap();
        assert!(
            suspended.is_some(),
            "RequestFeedback should report the suspension"
        );

        // A trailing event -- the runtime emits `Finish` after the feedback
        // request -- reports no suspension of its own.
        let trailing = consume_event::<NoneDB, FormaMessage>(
            &SSEvent::Finish,
            &mut responses,
            &mut usage,
            "session-1",
            &session,
            &db,
        )
        .await
        .unwrap();
        assert!(
            trailing.is_none(),
            "a trailing event reports None, so callers must latch rather than assign"
        );
    }

    #[tokio::test]
    async fn test_consume_event_folds_a_file_into_the_assistant_message() {
        use genai_client::openai::v1::{OpenAIFile, OpenAIFilePart, OpenAIMessagePart};

        let session = Arc::new(RwLock::new(SessionData::default()));
        let db = Arc::new(NoneDB::default());
        let mut responses = ResponseMessages::default();
        let mut usage = UsageTally::new();

        // A tool emits this the way it emits `Reference`, returning only a
        // summary to the LLM through its own JSON result.
        let file = SSEvent::FileAvailable {
            id: "msg-1".to_string(),
            part: OpenAIMessagePart::File(OpenAIFilePart {
                file: OpenAIFile {
                    file_data: Some("data:image/png;base64,iVBORw0KGgo=".to_string()),
                    file_id: None,
                    filename: Some("chart.png".to_string()),
                },
            }),
        };

        let suspended = consume_event::<NoneDB, FormaMessage>(
            &file,
            &mut responses,
            &mut usage,
            "session-1",
            &session,
            &db,
        )
        .await
        .unwrap();
        assert!(suspended.is_none(), "a file does not suspend the turn");

        // It belongs to the assistant's message, not to a tool result, so that
        // the document is still in the history the next turn is built from.
        match &responses.response().content {
            FormaMessageBody::Assistant { content, .. } => {
                assert_eq!(content.len(), 1);
                assert!(matches!(content[0], FormaMessagePart::File(_)));
            }
            other => panic!("expected an assistant body, got {other:?}"),
        }
        assert_eq!(responses.messages.len(), 1, "a file is not a tool result");
    }

    fn call(id: &str, name: &str) -> SSEvent {
        SSEvent::ToolInputAvailable {
            tool_call_id: id.into(),
            tool_name: name.into(),
            tool_kind: String::new(),
            input: serde_json::json!({}),
        }
    }

    fn result(id: &str) -> SSEvent {
        SSEvent::ToolOutputAvailable {
            tool_call_id: id.into(),
            output: serde_json::json!({"ok": true}),
        }
    }

    fn picture() -> SSEvent {
        use genai_client::openai::v1::{OpenAIFile, OpenAIFilePart, OpenAIMessagePart};
        SSEvent::FileAvailable {
            id: "gen-image-1".into(),
            part: OpenAIMessagePart::File(OpenAIFilePart {
                file: OpenAIFile {
                    file_data: Some("data:image/png;base64,iVBORw0KGgo=".into()),
                    file_id: None,
                    filename: Some("generated-1.png".into()),
                },
            }),
        }
    }

    fn kinds(messages: &[FormaMessage]) -> Vec<&'static str> {
        messages
            .iter()
            .map(|m| match &m.content {
                FormaMessageBody::Assistant { tool_calls, .. } if !tool_calls.is_empty() => "calls",
                FormaMessageBody::Assistant { .. } => "content",
                FormaMessageBody::Tool { .. } => "result",
                _ => "other",
            })
            .collect()
    }

    /// A picture a tool sends while it runs must not land between the call
    /// and its result: Gemini refuses the next request if it does. It goes
    /// after the results — all of them, when calls ran in parallel.
    #[test]
    fn test_a_file_sent_during_a_call_goes_after_its_result() {
        let mut responses = ResponseMessages::default();
        for e in [call("tc1", "gen-image"), picture(), result("tc1")] {
            responses.push_event(&e);
        }
        assert_eq!(kinds(&responses.settled()), ["calls", "result", "content"]);

        let mut parallel = ResponseMessages::default();
        for e in [
            call("tc1", "gen-image"),
            call("tc2", "web-search"),
            result("tc2"),
            picture(),
            result("tc1"),
        ] {
            parallel.push_event(&e);
        }
        assert_eq!(
            kinds(&parallel.settled()),
            ["calls", "result", "result", "content"]
        );
    }

    /// A turn that stops before every call is answered — one is waiting for
    /// an approval — still keeps the picture, with the calls it arrived during.
    #[test]
    fn test_a_file_held_when_the_turn_stops_is_kept_with_its_calls() {
        let mut responses = ResponseMessages::default();
        for e in [
            call("tc1", "gen-image"),
            call("tc2", "send-email"),
            picture(),
            result("tc1"),
        ] {
            responses.push_event(&e);
        }
        let settled = responses.settled();
        assert_eq!(kinds(&settled), ["calls", "result"]);
        assert!(matches!(
            settled[0].content.content()[0],
            FormaMessagePart::File(_)
        ));
    }

    /// Text, a tool call, its result and more text must be stored in that
    /// order, since a reopened session is rebuilt from storage order.
    #[test]
    fn test_response_messages_keep_the_order_events_arrived_in() {
        let mut responses = ResponseMessages::default();
        let text = |id: &str, t: &str| {
            [
                SSEvent::TextStart { id: id.into() },
                SSEvent::TextDelta {
                    id: id.into(),
                    delta: t.into(),
                },
                SSEvent::TextEnd { id: id.into() },
            ]
        };
        let events = text("a", "Let me search.")
            .into_iter()
            .chain([
                SSEvent::ToolInputStart {
                    tool_call_id: "tc1".into(),
                    tool_name: "web-search".into(),
                    tool_type: "tavily-web-search".into(),
                    // A single tool belongs to no group.
                    bundle: None,
                },
                SSEvent::ToolInputAvailable {
                    tool_call_id: "tc1".into(),
                    tool_name: "web-search".into(),
                    tool_kind: String::new(),
                    input: serde_json::json!({"q": "x"}),
                },
                SSEvent::ToolOutputAvailable {
                    tool_call_id: "tc1".into(),
                    output: serde_json::json!({"results": []}),
                },
            ])
            .chain(text("b", "Found it."))
            .chain(text("c", "Anything else?"));
        for e in events {
            responses.push_event(&e);
        }

        let m = &responses.messages;
        assert_eq!(m.len(), 3);
        match &m[0].content {
            FormaMessageBody::Assistant {
                content,
                tool_calls,
                tool_bundles,
                tool_types,
                ..
            } => {
                assert_eq!(content.len(), 1);
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(
                    tool_types.get("tc1").map(String::as_str),
                    Some("tavily-web-search")
                );
                assert!(tool_bundles.is_empty());
            }
            other => panic!("expected an assistant body, got {other:?}"),
        }
        assert!(matches!(
            &m[1].content,
            FormaMessageBody::Tool { tool_call_id, .. } if tool_call_id == "tc1"
        ));
        match &m[2].content {
            FormaMessageBody::Assistant {
                content,
                tool_calls,
                ..
            } => {
                // Consecutive texts are separate parts of one message.
                let texts: Vec<_> = content
                    .iter()
                    .map(|p| match p {
                        FormaMessagePart::Text(t) => t.text.as_str(),
                        _ => panic!("expected text"),
                    })
                    .collect();
                assert_eq!(texts, ["Found it.", "Anything else?"]);
                assert!(tool_calls.is_empty());
            }
            other => panic!("expected an assistant body, got {other:?}"),
        }

        // A non-streaming client still gets all the text as one reply.
        assert_eq!(responses.response().content.content().len(), 3);
    }

    // ── store_turn ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_store_turn() {
        let state = make_app_state::<NoneDB>(&format!("test-st-{}", Uuid::new_v4())).await;
        let session = state.db.new_session("agent_id", "user_id").await.unwrap();
        let session_id = session.id_string().unwrap();
        let msg = FormaMessage::new_assistant_with_content("hi!".to_string());
        let saved = store_turn(false, &session_id, "user_id", vec![msg], &state.db)
            .await
            .unwrap();
        assert!(saved[0].id.clone().unwrap().0.len() > 0);

        // Stored all at once, a turn still reads back in its order.
        let state = make_app_state::<MongoDB>(&format!("test-st-{}", Uuid::new_v4())).await;
        let session = state.db.new_session("agent_id", "user_id").await.unwrap();
        let session_id = session.id_string().unwrap();
        let texts = ["first", "second", "third"];
        let turn = texts
            .iter()
            .map(|t| FormaMessage::new_assistant_with_content(t.to_string()))
            .collect();
        let saved = store_turn(false, &session_id, "user_id", turn, &state.db)
            .await
            .unwrap();
        assert!(saved.iter().all(|m| m.id.is_some()));
        let read = state.db.get_session_by_id(&session_id, 10).await.unwrap();
        let read: Vec<String> = read
            .messages
            .iter()
            .map(|m| m.content.to_string().trim().to_string())
            .collect();
        assert_eq!(read, texts);
    }

    #[tokio::test]
    async fn test_store_turn_suspended_also_assigns_ids() {
        let state = make_app_state::<NoneDB>(&format!("test-st-susp-{}", Uuid::new_v4())).await;
        let session = state.db.new_session("agent_id", "user_id").await.unwrap();
        let session_id = session.id_string().unwrap();
        let msg = FormaMessage::new_assistant_with_content("hi!".to_string());
        let empty = FormaMessage::new_assistant();
        let saved = store_turn(true, &session_id, "user_id", vec![msg, empty], &state.db)
            .await
            .unwrap();
        // The empty message is not stored.
        assert_eq!(saved.len(), 1);
        assert!(saved[0].id.clone().unwrap().0.len() > 0);
    }

    // ── a new message on a suspended session ──────────────────────────────────

    fn user(text: &str) -> FormaMessage {
        FormaMessage::new_user_with_content(text.to_string())
    }

    fn assistant_calling(ids: &[&str]) -> FormaMessage {
        let mut message = FormaMessage::new_assistant();
        message.content = FormaMessageBody::Assistant {
            content: vec![],
            tool_calls: ids
                .iter()
                .map(|id| OpenAIToolChoice::Function {
                    id: id.to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "a-tool".into(),
                        arguments: "{}".into(),
                    },
                })
                .collect(),
            thought_signature: None,
            tool_bundles: Default::default(),
            tool_types: Default::default(),
        };
        message
    }

    fn tool_result(id: &str) -> FormaMessage {
        let mut message = FormaMessage::new_tool();
        message.content = FormaMessageBody::Tool {
            tool_call_id: id.to_string(),
            content: vec![FormaMessagePart::Text(FormaTextPart { text: "ok".into() })],
        };
        message
    }

    fn pending(id: &str, from_root: bool) -> PendingToolCall {
        PendingToolCall {
            tool_call_id: id.to_string(),
            node_id: "reply".into(),
            tool_name: "issue-refund".into(),
            arguments: serde_json::json!({"order_id": "C-9", "amount": 12}),
            from_root,
            tool_type: "api-call".into(),
            ..Default::default()
        }
    }

    fn answered_ids(messages: &[FormaMessage]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|m| match &m.content {
                FormaMessageBody::Tool {
                    tool_call_id,
                    content,
                } => {
                    let text = match &content[0] {
                        FormaMessagePart::Text(t) => t.text.as_str(),
                        _ => "",
                    };
                    assert_eq!(
                        text, UNANSWERED_TOOL_CALL_RESULT,
                        "the model is told the call did not run"
                    );
                    Some(tool_call_id.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_a_call_waiting_for_approval_is_recorded_then_rejected() {
        // It never ran, so it is not in the history: the model is shown what
        // it proposed, and that it did not happen.
        let mut preamble = FormaMessage::new_assistant();
        preamble
            .content
            .content_mut()
            .push(FormaMessagePart::Text(FormaTextPart {
                text: "Sure, let me refund that.".into(),
            }));
        let history = vec![user("refund C-9"), preamble];

        let closing = closing_messages(&history, &[pending("call-1", true)]);
        assert_eq!(kinds(&closing), ["calls", "result"]);
        match &closing[0].content {
            FormaMessageBody::Assistant {
                tool_calls,
                tool_types,
                ..
            } => {
                assert_eq!(tool_calls[0].id(), "call-1");
                assert_eq!(tool_types["call-1"], "api-call");
            }
            other => panic!("expected the proposed call, got {other:?}"),
        }
        assert_eq!(answered_ids(&closing), ["call-1"]);
    }

    #[test]
    fn test_a_call_that_ran_but_waits_inside_gets_its_result_after_it() {
        // A workflow tool whose inner node waits for an approval: the tool's
        // own call is in the history, and is what gets the result. The inner
        // call never reached the history.
        let history = vec![
            user("plan my trip"),
            assistant_calling(&["quick", "workflow"]),
            tool_result("quick"),
        ];
        let closing = closing_messages(&history, &[pending("inner", false)]);
        assert_eq!(kinds(&closing), ["result"]);
        assert_eq!(answered_ids(&closing), ["workflow"]);
    }

    #[test]
    fn test_answered_calls_and_earlier_turns_are_left_alone() {
        let history = vec![
            user("first"),
            // Unanswered, but a turn that ended long ago is not this one's to fix.
            assistant_calling(&["old"]),
            user("second"),
            assistant_calling(&["new"]),
            tool_result("new"),
        ];
        assert!(closing_messages(&history, &[]).is_empty());
    }

    // #[tokio::test]
    // async fn test_persist_after_loop_persist_true_saves_message() {
    //     let agent_id = format!("test-pal-persist-{}", Uuid::new_v4());
    //     let state = make_app_state::<MongoDBClient>(&agent_id).await;
    //     // First create a session so push_msg has something to attach to
    //     let Json(new_session) = create_session(&state, "user1").await.unwrap();
    //     let session_id = new_session.session_id;

    //     let msg = FormaMessage::new_assistant_with_content("hi!".to_string());
    //     let saved = persist_after_loop(false, &session_id, "user1", msg, &state.db)
    //         .await
    //         .unwrap();
    //     assert!(saved.id.unwrap().0.len() > 0);
    // }

    // #[tokio::test]
    // async fn test_persist_after_loop_persist_true_suspended_also_saves() {
    //     let agent_id = format!("test-pal-persist-susp-{}", Uuid::new_v4());
    //     let state = make_app_state::<MongoDBClient>(&agent_id).await;
    //     let Json(new_session) = create_session(&state, "user1").await.unwrap();
    //     let session_id = new_session.session_id;

    //     let msg = FormaMessage::new_assistant_with_content("hi!".to_string());
    //     let saved = persist_after_loop(
    //         true, // is suspended
    //         &session_id,
    //         "user1",
    //         msg,
    //         &state.db,
    //     )
    //     .await
    //     .unwrap();
    //     assert!(saved.id.unwrap().0.len() > 0);
    // }

    // ── prepare_agent_call ────────────────────────────────────────────────────

    // #[tokio::test]
    // async fn test_prepare_agent_call_sends_start_event() {
    //     let agent_id = format!("test-prepare-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let result = prepare_agent_call(CallMode::NewJob, args, None::<FormaMessage>).await;
    //     assert!(
    //         result.is_ok(),
    //         "prepare_agent_call failed: {:?}",
    //         result.err()
    //     );
    //     let (mut rx, _session) = result.unwrap();
    //     let first = rx.recv().await.expect("channel closed before first event");
    //     assert!(
    //         matches!(first, SSEvent::Start { .. }),
    //         "expected SSEvent::Start, got {:?}",
    //         first
    //     );
    // }

    // // ── agent_box_stream ──────────────────────────────────────────────────────

    // #[tokio::test]
    // async fn test_agent_box_stream_no_persist_returns_ok_stream() {
    //     let agent_id = format!("test-abs-smoke-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let result = agent_box_stream::<NoneDB, FormaMessage, SSEvent>(
    //         CallMode::NewJob,
    //         args,
    //         None::<FormaMessage>,
    //     )
    //     .await;
    //     assert!(result.is_ok());
    // }

    // #[tokio::test]
    // async fn test_agent_box_stream_invalid_session_returns_err() {
    //     let agent_id = format!("test-abs-inv-{}", Uuid::new_v4());
    //     let state = make_app_state::<MongoDBClient>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("invalid-session-id"),
    //         user_id: Arc::from("user1"),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //     };
    //     let result = agent_box_stream::<MongoDBClient, FormaMessage, SSEvent>(
    //         CallMode::NewJob,
    //         args,
    //         None::<FormaMessage>,
    //     )
    //     .await;
    //     assert!(result.is_err());
    // }

    // #[tokio::test]
    // async fn test_agent_box_stream_start_event_passes_through_underscore_arm() {
    //     use futures::StreamExt;
    //     let agent_id = format!("test-abs-start-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let mut stream = agent_box_stream::<NoneDB, FormaMessage, SSEvent>(
    //         CallMode::NewJob,
    //         args,
    //         None::<FormaMessage>,
    //     )
    //     .await
    //     .unwrap();
    //     // SSEvent::Start goes through the `_` arm and should be yielded as Ok
    //     let first = stream
    //         .next()
    //         .await
    //         .expect("stream should yield at least one event");
    //     assert!(first.is_ok(), "Start event should yield Ok(Event)");
    // }

    // #[tokio::test]
    // async fn test_agent_box_stream_error_with_translate_some_yields_ok_event() {
    //     use futures::StreamExt;
    //     // With O = SSEvent, translate always returns Some, so an Error event is
    //     // yielded as Ok(Event) rather than Err.
    //     let agent_id = format!("test-abs-err-ok-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //     };
    //     let mut stream = agent_box_stream::<NoneDB, FormaMessage, SSEvent>(
    //         CallMode::NewJob,
    //         args,
    //         None::<FormaMessage>,
    //     )
    //     .await
    //     .unwrap();
    //     let mut items = Vec::new();
    //     while let Some(item) = stream.next().await {
    //         items.push(item);
    //     }
    //     assert!(!items.is_empty(), "stream should yield at least one event");
    //     assert!(
    //         items.iter().all(|i| i.is_ok()),
    //         "all events should be Ok when translate always returns Some"
    //     );
    // }

    // #[tokio::test]
    // async fn test_agent_box_stream_error_with_translate_none_yields_err() {
    //     // A ClientSSE that returns None for Error triggers the Err yield path.
    //     #[derive(Serialize)]
    //     struct ErrorToNone;
    //     impl ClientSSE for ErrorToNone {
    //         fn translate(event: SSEvent) -> Option<Self> {
    //             match event {
    //                 SSEvent::Error { .. } => None,
    //                 _ => Some(ErrorToNone),
    //             }
    //         }
    //     }
    //     let agent_id = format!("test-abs-err-none-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let mut stream = agent_box_stream::<NoneDB, FormaMessage, ErrorToNone>(
    //         CallMode::NewJob,
    //         args,
    //         None::<FormaMessage>,
    //     )
    //     .await
    //     .unwrap();
    //     let mut items = Vec::new();
    //     while let Some(item) = stream.next().await {
    //         items.push(item);
    //     }
    //     assert!(
    //         items.iter().any(|i| i.is_err()),
    //         "Error event should yield Err when translate returns None"
    //     );
    // }

    // // ── raw_completion ────────────────────────────────────────────────────────

    // #[tokio::test]
    // async fn test_raw_completion_invalid_session_returns_err() {
    //     let agent_id = format!("test-rc-inv-{}", Uuid::new_v4());
    //     let state = make_app_state::<MongoDBClient>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("invalid-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let result = raw_completion::<MongoDBClient, FormaMessage, serde_json::Value>(
    //         CallMode::NewJob,
    //         args,
    //         None,
    //     )
    //     .await;
    //     assert!(result.is_err());
    // }

    // #[tokio::test]
    // async fn test_raw_completion_error_event_returns_err() {
    //     // SSEvent::Error from a failing agent propagates as Err (not Suspended/Completed).
    //     let agent_id = format!("test-rc-err-{}", Uuid::new_v4());
    //     let state = make_app_state::<NoneDB>(&agent_id).await;
    //     let args = JobArgs {
    //         n_messages: 10,
    //         session_id: Arc::from("no-memory-agent;unnecessary-session-id"),
    //         user_id: Arc::from("user1"),
    //         roles: Arc::from(vec![]),
    //         oauth_tokens: Arc::new(Mutex::new(HashMap::new())),
    //         db: state.db.clone(),
    //         agent_id: state.agent_id.clone(),
    //         agent: state.agent.clone(),
    //     };
    //     let result =
    //         raw_completion::<NoneDB, FormaMessage, serde_json::Value>(CallMode::NewJob, args, None)
    //             .await;
    //     assert!(result.is_err());
    // }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {}
