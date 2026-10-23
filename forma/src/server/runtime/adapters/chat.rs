//! The conversation a chat app (WhatsApp, Telegram) holds with an agent.
//!
//! A chat app delivers what people write to a webhook, and is answered
//! through its own API. Each adapter only translates its platform's terms:
//! who wrote, what, and how to send something back ([`ChatApp`]). Everything
//! else lives here, so that an agent behaves the same on every chat app:
//!
//! - Each sender has one conversation with the agent, which never ends.
//! - A message is answered once, however many times it is delivered: it is
//!   stored under the id the app gave it.
//! - A message that arrives while the agent is still answering waits a
//!   little for its turn (see [`in_turn`]), and is otherwise refused, for the
//!   app to deliver it again later.
//! - Calls that need an approval are asked with two buttons; calls to client
//!   tools are answered at once, as a chat app has no client to run them.
//! - The app only hears of a failure when delivering again could help.

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use genai_client::{
    LLMMessage,
    events::UsageTally,
    openai::v1::{
        OpenAIFile, OpenAIFilePart, OpenAIImagePart, OpenAIImageUrl, OpenAIMessagePart,
        OpenAITextPart,
    },
};
use protocols::{
    FormaDB, FormaError, FormaMessage, PendingToolCall, ResumeRequest, SessionData, USER_ID_HEADER,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::server::{
    JobArgs,
    runtime::adapters::{CompletionResult, ResumeResult, completion, in_turn, resume_completion},
    state::RuntimeState,
};

/// A file that travels over a chat app, in the form the rest of Forma
/// speaks: raw bytes plus the media type that describes them.
#[derive(Debug)]
pub(crate) struct ChatMedia {
    pub(crate) mime_type: String,
    pub(crate) bytes: Vec<u8>,
}

impl ChatMedia {
    /// The `data:` URL form, which is what the OpenAI-shaped content parts
    /// carry and therefore what reaches a provider.
    pub(crate) fn to_data_url(&self) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        format!("data:{};base64,{}", self.mime_type, B64.encode(&self.bytes))
    }
}

/// What a chat app does for the agent: deliver what it says.
#[async_trait::async_trait]
pub(crate) trait ChatApp: Send + Sync {
    /// The app's name, as people know it.
    const NAME: &'static str;
    /// The most characters one text message can carry.
    const MAX_TEXT: usize;
    /// The most characters a picture's caption can carry.
    const MAX_CAPTION: usize;

    /// Sends `text` to `to`.
    async fn send_text(&self, to: &str, text: &str) -> Result<(), FormaError>;

    /// Sends a picture to `to`, with a caption if there is one.
    async fn send_image(
        &self,
        to: &str,
        image: &ChatMedia,
        caption: Option<&str>,
    ) -> Result<(), FormaError>;

    /// Asks `to` to approve or reject a call, with two buttons whose ids are
    /// `approval.approve` and `approval.reject`.
    async fn send_approval(&self, to: &str, approval: &Approval) -> Result<(), FormaError>;
}

// ── what people send ────────────────────────────────────────────────────────

/// A file extension for a media type, used only to name uploads and
/// downloads — chat apps key off the media type itself, not the name.
pub(crate) fn media_extension(mime_type: &str) -> &str {
    match mime_type.split(';').next().unwrap_or_default() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/amr" => "amr",
        "application/pdf" => "pdf",
        // Anything unlisted falls back to its subtype, but only when there
        // really is one: a string with no slash is not a media type at all.
        other => match other.split_once('/') {
            Some((_, subtype)) if !subtype.is_empty() => subtype,
            _ => "bin",
        },
    }
}

/// A picture someone sent, as the agent receives it.
pub(crate) fn image_part(media: &ChatMedia) -> OpenAIMessagePart {
    OpenAIMessagePart::ImageUrl(OpenAIImagePart {
        image_url: OpenAIImageUrl {
            url: media.to_data_url(),
            detail: None,
        },
    })
}

/// Any other file someone sent (a voice note, a document), as the agent
/// receives it.
///
/// A voice note is whatever container the app chose (usually OGG/Opus),
/// which `input_audio`'s format enum cannot name, so audio travels as a file
/// too, whose data URL states the type exactly.
pub(crate) fn file_part(media: &ChatMedia, filename: String) -> OpenAIMessagePart {
    OpenAIMessagePart::File(OpenAIFilePart {
        file: OpenAIFile {
            file_data: Some(media.to_data_url()),
            file_id: None,
            filename: Some(filename),
        },
    })
}

/// Text someone sent, or a caption. The caption of a picture or document is
/// the question about it, so it goes after it, the same way an attachment is
/// sent from the web UI.
pub(crate) fn text_part(text: &str) -> OpenAIMessagePart {
    OpenAIMessagePart::Text(OpenAITextPart {
        text: text.to_string(),
    })
}

// ── what the agent answers ──────────────────────────────────────────────────

/// The data URL of a part that holds a picture, whichever kind of part it is.
///
/// A generated image arrives as a `file` part rather than an `image_url` one,
/// so what the part *is* decides this, not which variant carried it.
fn part_image_data_url(part: &OpenAIMessagePart) -> Option<String> {
    let url = match part {
        OpenAIMessagePart::ImageUrl(p) => p.image_url.url.clone(),
        OpenAIMessagePart::File(p) => p.file.file_data.clone()?,
        _ => return None,
    };
    url.starts_with("data:image/").then_some(url)
}

/// Decodes a base64 data URL into the bytes an upload needs.
fn data_url_to_media(url: &str) -> Option<ChatMedia> {
    use base64::{Engine, engine::general_purpose::STANDARD as B64};

    let (meta, payload) = url.strip_prefix("data:")?.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let mime_type = meta.split(';').next().filter(|m| !m.is_empty())?;

    Some(ChatMedia {
        mime_type: mime_type.to_string(),
        bytes: B64.decode(payload.trim()).ok()?,
    })
}

/// What one agent answer turns into on a chat app, which has no multi-part
/// message: each picture is its own send, and the text either rides along as
/// a caption or follows as a message of its own.
///
/// Kept separate from the sending so the decision can be inspected without a
/// network call — the failure mode being guarded against is a reply that
/// silently arrives as text only.
#[derive(Debug, Default)]
struct ReplyPlan {
    images: Vec<ChatMedia>,
    /// Set only when exactly one picture can carry the text; more than one
    /// would repeat it, and a long one would not fit (`max_caption`).
    caption: Option<String>,
    /// The text when it is not riding along as a caption.
    standalone_text: Option<String>,
}

impl ReplyPlan {
    fn from_message(result: &LLMMessage, max_caption: usize) -> Self {
        let content = match result {
            LLMMessage::Assistant(m) => &m.content,
            // Every other role is a programming error here, but answering
            // nothing is better than answering wrongly.
            _ => {
                warn!("the agent answered with a non-assistant message");
                return Self::default();
            }
        };

        let text = content
            .iter()
            .filter_map(|p| match p {
                OpenAIMessagePart::Text(t) if !t.text.trim().is_empty() => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let images: Vec<ChatMedia> = content
            .iter()
            .filter_map(part_image_data_url)
            .filter_map(|url: String| match data_url_to_media(&url) {
                Some(media) => Some(media),
                None => {
                    warn!("dropping an image whose data URL could not be decoded");
                    None
                }
            })
            .collect();

        let fits_in_caption =
            images.len() == 1 && !text.trim().is_empty() && text.chars().count() <= max_caption;

        Self {
            images,
            caption: fits_in_caption.then(|| text.clone()),
            standalone_text: (!fits_in_caption && !text.trim().is_empty()).then_some(text),
        }
    }

    fn is_empty(&self) -> bool {
        self.images.is_empty() && self.standalone_text.is_none() && self.caption.is_none()
    }
}

/// Cuts `text` into messages of at most `max` characters, at the last
/// paragraph break, line break or space that fits, so that no word is cut in
/// two unless it is longer than a whole message.
fn split_text(text: &str, max: usize) -> Vec<String> {
    let mut chunks = vec![];
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let limit = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let window = &rest[..limit];
        let cut = window
            .rfind("\n\n")
            .or_else(|| window.rfind('\n'))
            .or_else(|| window.rfind(' '))
            .filter(|&i| i > 0)
            .unwrap_or(limit);
        chunks.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        chunks.push(rest.to_string());
    }
    chunks
}

/// Sends the agent's answer: each picture on its own, the first taking the
/// text as its caption when that fits, so the common case — one image, one
/// line — arrives as a single message rather than two.
async fn send_reply<C: ChatApp>(app: &C, to: &str, reply: &LLMMessage) -> Result<(), FormaError> {
    let plan = ReplyPlan::from_message(reply, C::MAX_CAPTION);

    if plan.is_empty() {
        warn!(
            "the agent produced no content to send back over {}",
            C::NAME
        );
        return Ok(());
    }

    // A picture that cannot be sent must not take the answer down with it:
    // apps reject media for reasons Forma cannot predict (size, type, an
    // expired token), and the reader is better served by the words alone
    // than by silence.
    let mut caption_delivered = false;
    for image in &plan.images {
        match app.send_image(to, image, plan.caption.as_deref()).await {
            Ok(()) => caption_delivered |= plan.caption.is_some(),
            Err(e) => warn!("could not send a picture over {}: {e}", C::NAME),
        }
    }

    // The text goes out on its own when it was not riding on a picture —
    // including when the picture that was carrying it never made it.
    let leftover = plan
        .standalone_text
        .clone()
        .or_else(|| plan.caption.clone().filter(|_| !caption_delivered));

    if let Some(text) = leftover {
        for chunk in split_text(&text, C::MAX_TEXT) {
            app.send_text(to, &chunk).await?;
        }
    }
    Ok(())
}

// ── approvals ───────────────────────────────────────────────────────────────

/// The words on an approval's buttons, and under its details.
pub(crate) const APPROVE_LABEL: &str = "✅ Approve";
pub(crate) const REJECT_LABEL: &str = "✖️ Reject";
pub(crate) const APPROVAL_PROMPT: &str = "Approve to continue";

/// The longest button id every app takes (Telegram's `callback_data`).
const MAX_BUTTON_ID: usize = 64;

/// How much of any single argument is worth showing before it starts
/// crowding out the others.
const ARGUMENT_VALUE_LIMIT: usize = 280;

/// A request to approve a call, as a person reads it.
pub(crate) struct Approval {
    /// The action, e.g. `🔧 Issue refund`.
    pub(crate) title: String,
    /// Its arguments, one per paragraph, with `*bold*` labels.
    pub(crate) details: String,
    /// The id of the button that approves it.
    pub(crate) approve: String,
    /// The id of the button that rejects it.
    pub(crate) reject: String,
}

impl Approval {
    pub(crate) fn new(call: &PendingToolCall) -> Self {
        Self {
            title: format!("🔧 {}", humanise(&call.tool_name)),
            details: format_arguments(&call.arguments),
            approve: button_id(true, &call.tool_call_id),
            reject: button_id(false, &call.tool_call_id),
        }
    }
}

/// `approve|TOOL_CALL_ID` or `reject|TOOL_CALL_ID`. The conversation it
/// belongs to is the sender's. An id that does not fit is cut, and the call
/// found by how it starts.
fn button_id(approve: bool, tool_call_id: &str) -> String {
    let mut id = format!(
        "{}|{tool_call_id}",
        if approve { "approve" } else { "reject" }
    );
    while id.len() > MAX_BUTTON_ID {
        id.pop();
    }
    id
}

/// The tool call a button answers (or how its id starts), and whether it
/// approves it. Buttons sent as `approve|SESSION_ID|TOOL_CALL_ID` still work:
/// the id is the last segment.
pub(crate) fn parse_button_id(button_id: &str) -> Option<(bool, String)> {
    let (action, rest) = button_id.split_once('|')?;
    let tool_call_id = rest.rsplit('|').next().filter(|id| !id.is_empty())?;
    let approve = match action {
        "approve" => true,
        "reject" => false,
        _ => return None,
    };
    Some((approve, tool_call_id.to_string()))
}

/// Shortens to `max` *characters*.
///
/// Counting characters rather than bytes is the point: slicing a `String` at a
/// byte offset panics when it lands inside a multi-byte character, which any
/// accented word or emoji in a tool's arguments would produce.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Turns `make-image` into `Make image`, so the confirmation names the action
/// in the reader's language rather than the agent author's identifier.
fn humanise(name: &str) -> String {
    let spaced = name.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Renders one argument value as something a person would read.
///
/// Strings lose their quotes, lists become comma-separated, and only a
/// genuinely nested structure falls back to JSON — in monospace, so the app
/// stops trying to interpret the braces as formatting.
fn render_value(value: &serde_json::Value) -> String {
    use serde_json::Value;
    let rendered = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Value::Null => "—".to_string(),
        Value::Array(items) if items.iter().all(|i| !i.is_object() && !i.is_array()) => items
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        other => format!(
            "```{}```",
            serde_json::to_string(other).unwrap_or_else(|_| format!("{other:?}"))
        ),
    };
    truncate_chars(&rendered, ARGUMENT_VALUE_LIMIT)
}

/// Lays out a tool's arguments as a readable list rather than a JSON dump.
///
/// A short value sits on the same line as its label; anything longer or
/// multi-line gets its own, which is what keeps a paragraph-length prompt
/// legible on a phone.
fn format_arguments(arguments: &serde_json::Value) -> String {
    let serde_json::Value::Object(fields) = arguments else {
        // Not the shape a tool call normally takes, but showing it beats
        // showing nothing.
        return render_value(arguments);
    };

    if fields.is_empty() {
        return "_No details._".to_string();
    }

    fields
        .iter()
        .map(|(key, value)| {
            let label = humanise(key);
            let rendered = render_value(value);
            if rendered.contains('\n') || rendered.chars().count() > 60 {
                format!("*{label}*\n{rendered}")
            } else {
                format!("*{label}:* {rendered}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── the conversation ────────────────────────────────────────────────────────

/// The kind of tool a client runs, which a chat app has no client for.
const CLIENT_TOOL: &str = "delegated";

/// How a run ended, as far as the person chatting is concerned.
enum Turn {
    /// With an answer to send.
    Answered(Box<LLMMessage>),
    /// Waiting on calls that need an answer.
    Waiting,
}

impl From<CompletionResult<LLMMessage>> for Turn {
    fn from(result: CompletionResult<LLMMessage>) -> Self {
        match result {
            CompletionResult::Completed(reply) => Self::Answered(Box::new(reply)),
            CompletionResult::Suspended { .. } => Self::Waiting,
        }
    }
}

/// The job something from `user_id` (a phone number, a chat) runs as: the
/// one conversation that sender has with the agent, found or started here.
async fn job_args<DB: FormaDB, T: RuntimeState<DB>>(
    user_id: &str,
    max_history_messages: usize,
    headers: &HeaderMap,
    state: &T,
) -> Result<JobArgs<DB>, FormaError> {
    let mut headers = headers.clone();
    let user = HeaderValue::from_str(user_id)
        .map_err(|x| FormaError::Input(format!("invalid user id '{x}'")))?;
    headers.insert(HeaderName::from_static(USER_ID_HEADER), user);

    let (agent, credentials) = state.__agent_and_credentials(&headers).await?;
    // The agent's own id in a single-tenant server, and the deployment's in a
    // multi-tenant one: either way, two agents sharing a database keep their
    // conversations apart.
    #[cfg(feature = "hot-reload")]
    let agent_id = agent.read().await.id.clone();
    #[cfg(not(feature = "hot-reload"))]
    let agent_id = agent.id.clone();

    let session = state
        .db()
        .get_or_create_session(&agent_id, user_id, max_history_messages)
        .await?;
    let session = Arc::new(RwLock::new(session));

    state
        .job_args_with(
            session,
            agent,
            credentials,
            max_history_messages,
            vec![],
            &headers,
        )
        .await
}

/// Answers a message (a text, a picture...) that `from` sent, and that the
/// app calls `id`. Returns what answering it cost.
///
/// The message is stored under the app's id, so a delivery the app repeats is
/// recognised and skipped. One that arrives while the agent is still
/// answering the previous one waits its turn.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn answer_message<DB: FormaDB, T: RuntimeState<DB>, C: ChatApp>(
    app: &C,
    from: &str,
    id: &str,
    parts: Vec<OpenAIMessagePart>,
    max_history_messages: usize,
    headers: &HeaderMap,
    state: &T,
) -> Result<Vec<UsageTally>, FormaError> {
    let mut message = FormaMessage::new_user_with_parts(protocols::from_provider_parts(parts));
    message.id = Some(id.to_string().into());

    let args = job_args::<DB, T>(from, max_history_messages, headers, state).await?;
    let (db, session_id) = (args.db.clone(), args.session_id.clone());

    // The job this request started with serves the first attempt; any other
    // reads the session again, as the run before it left it.
    let mut first = Some(args);
    let run = in_turn(|| {
        let first = first.take();
        let message = message.clone();
        async move {
            let args = match first {
                Some(args) => args,
                None => job_args::<DB, T>(from, max_history_messages, headers, state).await?,
            };
            let session = args.session.clone();
            let response = completion::<DB, FormaMessage, LLMMessage>(args, message).await?;
            Ok((response, session))
        }
    })
    .await;

    let (response, session) = match run {
        Ok(done) => done,
        Err(FormaError::DuplicateMessage(id)) => {
            info!(message_id = %id, "{} delivered a message again; it was already answered", C::NAME);
            return Ok(vec![]);
        }
        // Delivering it again only helps a message that never made it into
        // the conversation: a stored one would be skipped as a duplicate, so
        // its failure is reported here, and acknowledged.
        Err(e) if db.persist() && db.has_message(&session_id, id).await.unwrap_or(false) => {
            error!(message_id = %id, "failed to answer a {} message: {e}", C::NAME);
            return Ok(vec![]);
        }
        Err(e) => return Err(e),
    };

    let mut usage = vec![response.usage];
    usage.extend(
        settle(
            app,
            from,
            response.result.into(),
            session,
            max_history_messages,
            headers,
            state,
        )
        .await,
    );
    Ok(usage)
}

/// What came of a press on an approval's button.
pub(crate) enum ButtonOutcome {
    /// The call is not waiting for an answer: it was answered already, a
    /// new message rejected it, or the button is not one of Forma's.
    NotWaiting,
    /// The call was answered, at this cost.
    Answered(Vec<UsageTally>),
}

/// Answers the call one of an approval's buttons (`button`, its id) was sent
/// for, and resumes the agent.
pub(crate) async fn answer_button<DB: FormaDB, T: RuntimeState<DB>, C: ChatApp>(
    app: &C,
    from: &str,
    button: &str,
    max_history_messages: usize,
    headers: &HeaderMap,
    state: &T,
) -> Result<ButtonOutcome, FormaError> {
    let Some((approve, call_id)) = parse_button_id(button) else {
        warn!(button, "ignoring a {} button Forma did not send", C::NAME);
        return Ok(ButtonOutcome::NotWaiting);
    };

    let args = job_args::<DB, T>(from, max_history_messages, headers, state).await?;
    // What is approved is what was proposed: the arguments come from the call
    // the conversation is waiting on, never from the button.
    let call = args
        .session
        .read()
        .await
        .pending_tool_calls
        .iter()
        .find(|call| !call.approved && call.tool_call_id.starts_with(&call_id))
        .cloned();
    let Some(call) = call else {
        info!(call_id = %call_id, "a button for a call that is no longer waiting");
        return Ok(ButtonOutcome::NotWaiting);
    };

    let req = ResumeRequest {
        tool_call_id: call.tool_call_id,
        reject: !approve,
        arguments: approve.then_some(call.arguments),
        result: None,
    };
    let session = args.session.clone();
    let resumed = match resume_completion::<DB, LLMMessage>(args, req).await {
        Ok(resumed) => resumed,
        Err(e) => {
            error!("could not resume the agent after a {} button: {e}", C::NAME);
            return Ok(ButtonOutcome::Answered(vec![]));
        }
    };

    let mut usage: Vec<UsageTally> = Some(resumed.usage)
        .filter(|u| !u.is_empty())
        .into_iter()
        .collect();
    let turn = match resumed.result {
        ResumeResult::Completed(reply) => Some(Turn::Answered(Box::new(reply))),
        ResumeResult::Suspended { .. } => Some(Turn::Waiting),
        // Other calls of the same turn are still waiting: their buttons were
        // sent with this one's.
        ResumeResult::StillSuspended => None,
    };
    if let Some(turn) = turn {
        usage.extend(
            settle(
                app,
                from,
                turn,
                session,
                max_history_messages,
                headers,
                state,
            )
            .await,
        );
    }
    Ok(ButtonOutcome::Answered(usage))
}

/// Adds a reaction to the message it reacts to. Reactions are not answered.
///
/// Only WhatsApp sends them: Telegram does only to bots that ask for them,
/// and that are admins of the chat.
pub(crate) async fn store_reaction<DB: FormaDB, T: RuntimeState<DB>>(
    from: &str,
    message_id: &str,
    emoji: Option<&str>,
    max_history_messages: usize,
    headers: &HeaderMap,
    state: &T,
) -> Result<(), FormaError> {
    // An empty emoji is a reaction being removed.
    let Some(emoji) = emoji.filter(|e| !e.is_empty()) else {
        return Ok(());
    };
    let args = job_args::<DB, T>(from, max_history_messages, headers, state).await?;
    match args
        .db
        .add_msg_reaction(&args.session_id, message_id, &Some(emoji.to_string()))
        .await
    {
        Ok(()) => {}
        // The agent's answers are stored under Forma's ids, not the app's, so
        // a reaction to one of them finds nothing.
        Err(FormaError::MessageNotFound(id)) => {
            debug!(message_id = %id, "a reaction to a message that is not stored");
        }
        Err(e) => warn!("could not store a reaction: {e}"),
    }
    Ok(())
}

/// Delivers how a run ended: its answer, or the buttons for every call it
/// waits on (a turn can stop at several at once, and each needs its own
/// answer). Calls to client tools are answered here, and the agent resumed:
/// a chat app has no client to run them. Returns what resuming it cost.
#[allow(clippy::too_many_arguments)]
async fn settle<DB: FormaDB, T: RuntimeState<DB>, C: ChatApp>(
    app: &C,
    to: &str,
    mut turn: Turn,
    mut session: Arc<RwLock<SessionData>>,
    max_history_messages: usize,
    headers: &HeaderMap,
    state: &T,
) -> Vec<UsageTally> {
    let mut usage = vec![];
    loop {
        match turn {
            Turn::Answered(reply) => {
                // The turn is stored by now, so a failure to send it is
                // reported here: delivering the message again would be
                // skipped as a duplicate, and send nothing.
                if let Err(e) = send_reply(app, to, &reply).await {
                    error!("could not send the agent's answer over {}: {e}", C::NAME);
                }
                return usage;
            }
            Turn::Waiting => {
                let waiting: Vec<PendingToolCall> = session
                    .read()
                    .await
                    .pending_tool_calls
                    .iter()
                    .filter(|call| !call.approved)
                    .cloned()
                    .collect();

                let Some(client_tool) = waiting.iter().find(|call| call.tool_type == CLIENT_TOOL)
                else {
                    for call in &waiting {
                        if let Err(e) = app.send_approval(to, &Approval::new(call)).await {
                            error!(tool_call_id = %call.tool_call_id, "could not send an approval over {}: {e}", C::NAME);
                        }
                    }
                    return usage;
                };

                let req = ResumeRequest {
                    tool_call_id: client_tool.tool_call_id.clone(),
                    reject: false,
                    arguments: None,
                    result: Some(serde_json::json!({
                        "error": format!(
                            "This tool runs in an app, and the user is chatting on {}, which cannot run it: nothing was done.",
                            C::NAME
                        )
                    })),
                };
                let args = match job_args::<DB, T>(to, max_history_messages, headers, state).await {
                    Ok(args) => args,
                    Err(e) => {
                        error!("could not answer a client tool over {}: {e}", C::NAME);
                        return usage;
                    }
                };
                session = args.session.clone();
                match resume_completion::<DB, LLMMessage>(args, req).await {
                    Ok(resumed) => {
                        if !resumed.usage.is_empty() {
                            usage.push(resumed.usage);
                        }
                        turn = match resumed.result {
                            ResumeResult::Completed(reply) => Turn::Answered(Box::new(reply)),
                            ResumeResult::Suspended { .. } | ResumeResult::StillSuspended => {
                                Turn::Waiting
                            }
                        };
                    }
                    Err(e) => {
                        error!("could not answer a client tool over {}: {e}", C::NAME);
                        return usage;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::server::runtime::adapters::ResponseMessages;

    use super::*;

    // ── media helpers ──────────────────────────────────────────────────────

    #[test]
    fn extension_comes_from_the_media_type() {
        assert_eq!(media_extension("image/jpeg"), "jpg");
        assert_eq!(media_extension("image/png"), "png");
        assert_eq!(media_extension("application/pdf"), "pdf");
        // Voice notes arrive with codec parameters attached.
        assert_eq!(media_extension("audio/ogg; codecs=opus"), "ogg");
        // Anything unlisted falls back to its subtype rather than failing.
        assert_eq!(media_extension("image/heic"), "heic");
        assert_eq!(media_extension("nonsense"), "bin");
    }

    #[test]
    fn a_picture_is_found_whichever_part_carries_it() {
        // A generated image is a `file` part, an uploaded one is `image_url`,
        // and both have to reach the app.
        let generated = OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some("data:image/png;base64,iVBORw0KGgo=".to_string()),
                file_id: None,
                filename: Some("generated-1.png".to_string()),
            },
        });
        assert_eq!(
            part_image_data_url(&generated).as_deref(),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );

        let uploaded = OpenAIMessagePart::ImageUrl(OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: "data:image/jpeg;base64,/9j/4AA=".to_string(),
                detail: None,
            },
        });
        assert!(part_image_data_url(&uploaded).is_some());
    }

    #[test]
    fn non_pictures_are_not_mistaken_for_images() {
        assert!(part_image_data_url(&text_part("hello")).is_none());

        // A PDF is a file part too, but it is not a picture.
        let pdf = OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some("data:application/pdf;base64,JVBERi0=".to_string()),
                file_id: None,
                filename: Some("report.pdf".to_string()),
            },
        });
        assert!(part_image_data_url(&pdf).is_none());
    }

    #[test]
    fn a_data_url_decodes_to_the_bytes_an_upload_needs() {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        let png = [0x89u8, 0x50, 0x4e, 0x47, 0xff];
        let url = format!("data:image/png;base64,{}", B64.encode(png));

        let media = data_url_to_media(&url).expect("should decode");
        assert_eq!(media.mime_type, "image/png");
        assert_eq!(media.bytes, png);

        // And a round trip back to the form a content part carries.
        assert_eq!(media.to_data_url(), url);
    }

    #[test]
    fn an_undecodable_data_url_is_rejected_rather_than_sent() {
        assert!(data_url_to_media("https://example.com/a.png").is_none());
        assert!(data_url_to_media("data:image/png,notbase64").is_none());
        assert!(data_url_to_media("data:;base64,AAA=").is_none());
    }

    // ── what actually reaches the app ──────────────────────────────────────

    fn generated_image() -> OpenAIMessagePart {
        use base64::{Engine, engine::general_purpose::STANDARD as B64};
        OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(format!("data:image/png;base64,{}", B64.encode([1u8, 2, 3]))),
                file_id: None,
                filename: Some("generated-1.png".to_string()),
            },
        })
    }

    fn assistant(parts: Vec<OpenAIMessagePart>) -> LLMMessage {
        LLMMessage::Assistant(genai_client::AssistantMessage {
            content: parts,
            ..Default::default()
        })
    }

    #[test]
    fn a_generated_image_is_the_route_the_tool_actually_takes() {
        // The whole point of the outbound path: the tool emits
        // `SSEvent::FileAvailable`, `consume_event` folds it into the
        // assistant message, and this is what has to come out the far end.
        let mut responses = ResponseMessages::default();
        responses.push_event(&genai_client::SSEvent::TextStart { id: "t".into() });
        responses.push_event(&genai_client::SSEvent::TextDelta {
            id: "t".into(),
            delta: "Here's your picture!".into(),
        });
        responses.push_event(&genai_client::SSEvent::FileAvailable {
            id: "gen-image-1".into(),
            part: generated_image(),
        });

        let message =
            LLMMessage::try_from(responses.response()).expect("all parts are text or files");
        let plan = ReplyPlan::from_message(&message, 1024);

        assert_eq!(plan.images.len(), 1, "the picture must survive to the send");
        assert_eq!(plan.images[0].mime_type, "image/png");
        assert_eq!(plan.images[0].bytes, [1, 2, 3]);
        assert_eq!(plan.caption.as_deref(), Some("Here's your picture!"));
        assert!(
            plan.standalone_text.is_none(),
            "one image carries the text as its caption"
        );
    }

    #[test]
    fn text_only_replies_are_unchanged() {
        let plan = ReplyPlan::from_message(&assistant(vec![text_part("just words")]), 1024);
        assert!(plan.images.is_empty());
        assert_eq!(plan.standalone_text.as_deref(), Some("just words"));
        assert!(plan.caption.is_none());
    }

    #[test]
    fn several_images_send_the_text_once_on_its_own() {
        let plan = ReplyPlan::from_message(
            &assistant(vec![
                generated_image(),
                generated_image(),
                text_part("two versions"),
            ]),
            1024,
        );
        assert_eq!(plan.images.len(), 2);
        assert!(plan.caption.is_none(), "a caption would be repeated");
        assert_eq!(plan.standalone_text.as_deref(), Some("two versions"));
    }

    #[test]
    fn an_image_with_no_words_sends_just_the_image() {
        let plan = ReplyPlan::from_message(&assistant(vec![generated_image()]), 1024);
        assert_eq!(plan.images.len(), 1);
        assert!(plan.caption.is_none());
        assert!(plan.standalone_text.is_none());
    }

    #[test]
    fn an_overlong_answer_follows_the_image_as_its_own_message() {
        // Captions are capped (1024 characters on WhatsApp and Telegram); the
        // answer is sent whole rather than cut in half.
        let long = "x".repeat(1100);
        let plan =
            ReplyPlan::from_message(&assistant(vec![generated_image(), text_part(&long)]), 1024);
        assert_eq!(plan.images.len(), 1);
        assert!(plan.caption.is_none());
        assert_eq!(plan.standalone_text.as_deref(), Some(long.as_str()));
    }

    #[test]
    fn a_pdf_is_not_sent_as_a_picture() {
        // Documents are not handled on the way out yet, and mistaking one for
        // an image would have the app reject the upload.
        let pdf = OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some("data:application/pdf;base64,JVBERi0=".to_string()),
                file_id: None,
                filename: Some("report.pdf".to_string()),
            },
        });
        let plan = ReplyPlan::from_message(&assistant(vec![pdf, text_part("see attached")]), 1024);
        assert!(plan.images.is_empty());
        assert_eq!(plan.standalone_text.as_deref(), Some("see attached"));
    }

    #[test]
    fn nothing_to_say_sends_nothing() {
        assert!(ReplyPlan::from_message(&assistant(vec![]), 1024).is_empty());
    }

    #[test]
    fn a_long_answer_is_sent_in_several_messages_without_cutting_words() {
        let paragraph = "word ".repeat(300); // 1500 characters
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
        let chunks = split_text(&text, 2000);
        assert_eq!(chunks.len(), 3, "cut at the paragraph breaks");
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 2000);
            assert!(chunk.starts_with("word") && chunk.ends_with("word"));
        }

        // A single word longer than a message is cut where it must be.
        let chunks = split_text(&"x".repeat(25), 10);
        assert_eq!(chunks, ["x".repeat(10), "x".repeat(10), "x".repeat(5)]);

        // Characters, not bytes: accents are not cut in half.
        let chunks = split_text(&"á".repeat(15), 10);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 15);

        assert_eq!(split_text("short", 4096), ["short"]);
        assert!(split_text("   ", 4096).is_empty());
    }

    // ── approvals ──────────────────────────────────────────────────────────

    fn call(id: &str) -> PendingToolCall {
        PendingToolCall {
            tool_call_id: id.to_string(),
            tool_name: "issue-refund".to_string(),
            arguments: serde_json::json!({"order_id": "C-9", "amount": 12}),
            ..Default::default()
        }
    }

    #[test]
    fn an_approval_reads_as_an_action_and_its_details() {
        let approval = Approval::new(&call("call-1"));
        assert_eq!(approval.title, "🔧 Issue refund");
        assert!(
            approval.details.contains("*Amount:* 12"),
            "{}",
            approval.details
        );
        assert!(
            approval.details.contains("*Order id:* C-9"),
            "{}",
            approval.details
        );
        assert_eq!(approval.approve, "approve|call-1");
        assert_eq!(approval.reject, "reject|call-1");
    }

    #[test]
    fn buttons_name_the_call_they_answer() {
        assert_eq!(
            parse_button_id("approve|call-1"),
            Some((true, "call-1".to_string()))
        );
        assert_eq!(
            parse_button_id("reject|call-1"),
            Some((false, "call-1".to_string()))
        );
        // Buttons sent before the session id was dropped from them.
        assert_eq!(
            parse_button_id("approve|session-1|call-1"),
            Some((true, "call-1".to_string()))
        );
        assert_eq!(parse_button_id("maybe|call-1"), None);
        assert_eq!(parse_button_id("approve"), None);
        assert_eq!(parse_button_id("approve|"), None);
    }

    #[test]
    fn a_button_id_fits_every_app() {
        // Telegram takes 64 bytes at most; the call is then found by how its
        // id starts.
        let long_id = "call_".to_string() + &"a".repeat(80);
        let id = Approval::new(&call(&long_id)).approve;
        assert!(id.len() <= 64);
        let (approve, prefix) = parse_button_id(&id).unwrap();
        assert!(approve);
        assert!(long_id.starts_with(&prefix));
    }
}
