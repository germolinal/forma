//! The OpenAPI descriptions of what Forma serves: one per client of the
//! runtime (`sync`, `sse`, `ai-sdk-v5`, `whatsapp`, `telegram`), whose routes
//! share their paths but not what they take and return, and the manager's.
//!
//! The runtime's handlers are generic over the database and the server's
//! state, and live in `impl` blocks, where `utoipa` cannot describe them. So
//! each client's operations are described here, on the paths the router uses
//! ([`CHAT_PATH`] and the others), with the types the handlers take and
//! return; a test sends every operation described to the client's router, so
//! a route that moves or goes away fails it.

use protocols::{CHAT_PATH, FormaError, INIT_SESSION_PATH, RESUME_PATH, ResumeRequest};
use serde::Serialize;
use utoipa::{
    IntoParams, Modify, OpenApi, ToSchema,
    openapi::{
        self,
        security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme},
    },
};

use crate::server::runtime::adapters::{
    Answer, CompletionResult, NewSessionResult, ResumeResult, ai_sdk_v5::message::AiSdkV5Message,
    ai_sdk_v5::sse::AiSdkV5Event,
};
use genai_client::{OpenAIChatMessage, SSEvent, events::UsageTally};

/// The APIs there is a description of, by the name `forma openapi` takes.
pub const APIS: &[&str] = &[
    "sync",
    "sse",
    "ai-sdk-v5",
    "whatsapp",
    "telegram",
    "manager",
];

/// The OpenAPI description of `api`, one of [`APIS`], as JSON.
pub fn spec(api: &str) -> Result<serde_json::Value, FormaError> {
    let spec = match api {
        "sync" => SyncApi::openapi(),
        "sse" => SseApi::openapi(),
        "ai-sdk-v5" => AiSdkV5Api::openapi(),
        "whatsapp" => WhatsappApi::openapi(),
        "telegram" => TelegramApi::openapi(),
        #[cfg(feature = "api")]
        "manager" => crate::manager::openapi(),
        _ => {
            return Err(FormaError::Input(format!(
                "there is no '{api}' API: it is one of {}",
                APIS.join(", ")
            )));
        }
    };
    let mut spec = serde_json::to_value(spec)?;
    // `utoipa` names generic schemas after their arguments.
    for (from, to) in [
        ("FormaMessage_String", "FormaMessage"),
        ("Answer_CompletionResult_OpenAIChatMessage", "ChatAnswer"),
        ("Answer_ResumeResult_OpenAIChatMessage", "ResumeAnswer"),
        ("Answer_ResumeResult_AiSdkV5Message", "ResumeAnswer"),
    ] {
        rename_schema(&mut spec, from, to);
    }
    Ok(spec)
}

/// Renames the schema `from` to `to`, and every reference to it.
///
/// For the generic types whose date parameter is described as a string
/// (`GenericFormaMessage<DATE>`, as `FormaMessage`): `utoipa` names them after
/// their generic arguments too (`FormaMessage_String`).
fn rename_schema(spec: &mut serde_json::Value, from: &str, to: &str) {
    let (from_ref, to_ref) = (
        format!("#/components/schemas/{from}"),
        format!("#/components/schemas/{to}"),
    );
    fn rename_refs(value: &mut serde_json::Value, from: &str, to: &str) {
        match value {
            serde_json::Value::String(s) if s == from => *s = to.to_string(),
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|v| rename_refs(v, from, to))
            }
            serde_json::Value::Object(map) => {
                map.values_mut().for_each(|v| rename_refs(v, from, to))
            }
            _ => {}
        }
    }
    rename_refs(spec, &from_ref, &to_ref);
    if let Some(schemas) = spec["components"]["schemas"].as_object_mut()
        && let Some(schema) = schemas.remove(from)
    {
        schemas.insert(to.to_string(), schema);
    }
}

/// The name of the API key's security scheme.
const API_KEY: &str = "api_key";

/// Requires the API key (`X-API-Key`) on every operation: what the `sync`,
/// `sse` and `ai-sdk-v5` clients do unless `no_api_key: true`.
struct ApiKeyAuth;

impl Modify for ApiKeyAuth {
    fn modify(&self, openapi: &mut openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            API_KEY,
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-API-Key",
                "The server's `FORMA_AGENT_KEY`. Not required when the runtime file says \
                 `no_api_key: true`. It identifies your back-end, not the user: keep it out of \
                 browsers and apps, and put a back-end of your own in front of the agent.",
            ))),
        );
        openapi.security = Some(vec![SecurityRequirement::new(
            API_KEY,
            Vec::<String>::new(),
        )]);
    }
}

/// The headers `/v1/init` reads.
#[derive(IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
struct InitHeaders {
    /// The user the conversation is for. Forma does not check it: your
    /// back-end authenticates the user, and sends their id. Only the user who
    /// started a conversation can write in it. Required when the server keeps
    /// conversations (any `storage` but `none`).
    #[param(rename = "X-User-ID")]
    user_id: Option<String>,
    /// Multi-tenant servers only: which deployment (agent) to talk to, as the
    /// manager returned it when deploying it. Set by your reverse proxy, from
    /// what it authenticated: never from what the client sent.
    #[param(rename = "X-Deployment-ID")]
    deployment_id: Option<String>,
}

/// The headers `/v1/chat` and `/v1/resume` read.
#[derive(IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
struct ConversationHeaders {
    /// The user who writes. Forma does not check it: your back-end
    /// authenticates the user, and sends their id. It must be the user who
    /// started the conversation.
    #[param(rename = "X-User-ID")]
    user_id: String,
    /// The conversation, as `/v1/init` returned it. Required when the server
    /// keeps conversations (any `storage` but `none`); otherwise every
    /// message is a conversation of its own.
    #[param(rename = "X-Session-ID")]
    session_id: Option<String>,
    /// The user's roles, comma-separated (`admin, editor`): which tools the
    /// agent may call for them (a tool's `just_for`). Not checked either:
    /// your back-end sends what it knows.
    #[param(rename = "X-User-Roles")]
    user_roles: Option<String>,
    /// The user's time zone, as an IANA name (`America/Santiago`): what the
    /// `current-date` tool answers in, before the agent's own `timezone`. A
    /// name that is not a time zone is ignored.
    #[param(rename = "X-Time-Zone", max_length = 64)]
    time_zone: Option<String>,
    /// Multi-tenant servers only: which deployment (agent) to talk to, as the
    /// manager returned it when deploying it. Set by your reverse proxy, from
    /// what it authenticated: never from what the client sent.
    #[param(rename = "X-Deployment-ID")]
    deployment_id: Option<String>,
}

/// A request Forma refused before reading it as a message: its body is not
/// JSON, is not a message, has no content, or is not the user's.
#[derive(ToSchema)]
#[allow(dead_code)]
struct MessageRejection {
    /// Why.
    error: String,
}

/// One `data:` line of the `sse` client's stream: an event, and, last, what
/// the job consumed.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum SseLine {
    /// Something that happened.
    Event(SSEvent),
    /// What the job consumed. The last line of a stream that was not cut
    /// short by an error.
    Usage(UsageTally),
}

/// One `data:` line of the `ai-sdk-v5` client's stream: an event of the AI
/// SDK's UI message stream, and, last, what the job consumed.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum AiSdkV5Line {
    /// Something that happened.
    Event(AiSdkV5Event),
    /// What the job consumed. The last line of a stream that was not cut
    /// short by an error.
    Usage(UsageTally),
}

/// What the `sync` and `sse` clients are told about the conversation.
const CONVERSATIONS: &str = "Conversations";

/// The descriptions of the operations every client that is not a webhook
/// serves, with the same meaning.
macro_rules! init_operation {
    () => {
        /// Starts a conversation.
        ///
        /// Creates a conversation for the user, and returns its id: send it as
        /// `X-Session-ID` with every message of it. A conversation never ends:
        /// start a new one whenever the user wants a fresh start. In a server
        /// that keeps no conversations (`storage: none`), it returns a
        /// placeholder, and stores nothing.
        #[utoipa::path(
            get,
            path = INIT_SESSION_PATH,
            tag = CONVERSATIONS,
            params(InitHeaders),
            responses(
                (status = OK, description = "The conversation started", body = NewSessionResult),
                (status = BAD_REQUEST, description = "A header is missing (`X-User-ID`; `X-Deployment-ID` in a multi-tenant server)", body = String, content_type = "text/plain"),
                (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
                (status = FORBIDDEN, description = "The `X-API-Key` is wrong", body = String, content_type = "text/plain"),
                (status = NOT_FOUND, description = "Multi-tenant servers: there is no such deployment", body = String, content_type = "text/plain"),
                (status = INTERNAL_SERVER_ERROR, description = "The server has no `FORMA_AGENT_KEY`, or its storage failed", body = String, content_type = "text/plain"),
            )
        )]
        fn init() {}
    };
}

/// The `sync` client: one JSON answer per request.
// Its functions only carry the operations' descriptions: they are never called.
#[allow(dead_code)]
mod sync {
    use super::*;

    init_operation!();

    /// Sends a message, and waits for the agent's answer.
    ///
    /// The message is the user's, in the shape of OpenAI's chat messages:
    /// `role` is `user`, and `content` is a string or a list of parts (text,
    /// images, audio, files, as base64 `data:` URLs). It is added to the
    /// conversation in `X-Session-ID`, and the agent answers it, reading the
    /// last `max_history_messages` messages before it.
    ///
    /// The agent can stop at a tool call that needs an answer: an approval
    /// (`confirm_before_calling`), or the result of a delegated tool, which
    /// your client runs. The answer is then `suspended`, and the conversation
    /// waits for `/v1/resume`. A new message in the meantime moves on instead:
    /// whatever was waiting is rejected (it never runs), and the agent answers
    /// the new message, knowing that.
    ///
    /// A conversation answers one message at a time: a message sent while the
    /// agent still answers the one before is refused (`409`).
    #[utoipa::path(
        post,
        path = CHAT_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body(content = OpenAIChatMessage, description = "The user's message: `role` must be `user`, and `content` cannot be empty. Attachments travel inline, and the body can be 2 MB at most."),
        responses(
            (status = OK, description = "The agent answered (`completed`, with its message: one text part per node that answered, in order), or stopped at a call that needs an answer (`suspended`)", body = Answer<CompletionResult<OpenAIChatMessage>>),
            (status = BAD_REQUEST, description = "The body is not a user message with content (a JSON `{\"error\": ...}`), or a header is missing (plain text)", content(
                (MessageRejection = "application/json"),
                (String = "text/plain"),
            )),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still answering the conversation's previous message: wait, and send it again", body = String, content_type = "text/plain"),
            (status = PAYLOAD_TOO_LARGE, description = "The body is larger than 2 MB (attachments included)", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The agent failed (e.g., its LLM provider could not be reached)", body = String, content_type = "text/plain"),
        )
    )]
    fn chat() {}

    /// Answers the tool call a conversation waits on.
    ///
    /// Approves it, with the arguments to run it with (which may differ from
    /// the ones the agent proposed); answers a delegated tool with its
    /// `result`; or rejects it (`reject: true`): the tool does not run, and the
    /// agent is told the user rejected it. Each call is answered once.
    ///
    /// The agent goes on once every call of its turn is answered: until then,
    /// the answer is `still_suspended`. Then it answers like `/v1/chat`:
    /// `completed`, or `suspended` at another call.
    #[utoipa::path(
        post,
        path = RESUME_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body = ResumeRequest,
        responses(
            (status = OK, description = "The agent answered (`completed`), stopped at another call (`suspended`), or waits for the turn's other calls (`still_suspended`)", body = Answer<ResumeResult<OpenAIChatMessage>>),
            (status = BAD_REQUEST, description = "The call is not waiting for an answer (it does not exist, was answered, or a new message rejected it); the body is not valid JSON, or has both or neither of `arguments` and `result`; a header is missing; or the server keeps no conversations", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still working on the conversation", body = String, content_type = "text/plain"),
            (status = UNPROCESSABLE_ENTITY, description = "The body is not a resume request (a missing `tool_call_id` or `reject`, a wrong type)", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The agent failed", body = String, content_type = "text/plain"),
        )
    )]
    fn resume() {}
}

/// The `sse` client: Server-Sent Events.
// Its functions only carry the operations' descriptions: they are never called.
#[allow(dead_code)]
mod sse {
    use super::*;

    init_operation!();

    /// Sends a message, and streams what the agent does.
    ///
    /// The message, and how the conversation goes on, are as with the `sync`
    /// client. The answer is a stream of Server-Sent Events: one `data:`
    /// line per event, whose JSON `type` says what happened (`start`, text
    /// and reasoning as it is written, tool calls and their results, files,
    /// sources), up to `finish`, after which a last line, with no `type`,
    /// reports what the job consumed (tokens per provider and model, tools
    /// called). An `error` event ends the stream early, without it.
    ///
    /// A stop at a call that needs an answer is a `request_feedback` event,
    /// followed by `finish`: answer it with `/v1/resume`. `finish` also means
    /// the turn is stored, so the next message can be sent.
    ///
    /// Errors found before the agent starts (a missing header, a busy
    /// conversation) are answered with a status, not a stream.
    #[utoipa::path(
        post,
        path = CHAT_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body(content = OpenAIChatMessage, description = "The user's message: `role` must be `user`, and `content` cannot be empty. Attachments travel inline, so the body can be as large as the runtime's `max_request_mb` (25 MB by default)."),
        responses(
            (status = OK, description = "A stream of Server-Sent Events. Each `data:` line holds one JSON object: an event (with a `type`), or, last, the job's usage (without one).", body = SseLine, content_type = "text/event-stream"),
            (status = BAD_REQUEST, description = "The body is not a user message with content (a JSON `{\"error\": ...}`), or a header is missing (plain text)", content(
                (MessageRejection = "application/json"),
                (String = "text/plain"),
            )),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still answering the conversation's previous message: wait, and send it again", body = String, content_type = "text/plain"),
            (status = PAYLOAD_TOO_LARGE, description = "The body is larger than `max_request_mb`", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The server has no `FORMA_AGENT_KEY`, or its storage failed", body = String, content_type = "text/plain"),
        )
    )]
    fn chat() {}

    /// Answers the tool call a conversation waits on, and streams what the
    /// agent does next.
    ///
    /// The request is as with the `sync` client. While other calls of the
    /// turn wait for an answer, the stream is a single `still_suspended`
    /// event. Once every call is answered, the agent goes on, and the stream
    /// is as `/v1/chat`'s.
    #[utoipa::path(
        post,
        path = RESUME_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body = ResumeRequest,
        responses(
            (status = OK, description = "A stream of Server-Sent Events, as `/v1/chat`'s; or a single `still_suspended` event", body = SseLine, content_type = "text/event-stream"),
            (status = BAD_REQUEST, description = "The call is not waiting for an answer (it does not exist, was answered, or a new message rejected it); the body is not valid JSON, or has both or neither of `arguments` and `result`; a header is missing; or the server keeps no conversations", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still working on the conversation", body = String, content_type = "text/plain"),
            (status = UNPROCESSABLE_ENTITY, description = "The body is not a resume request", body = String, content_type = "text/plain"),
        )
    )]
    fn resume() {}
}

/// The `ai-sdk-v5` client: Vercel's AI SDK (v5).
// Its functions only carry the operations' descriptions: they are never called.
#[allow(dead_code)]
mod ai_sdk_v5 {
    use super::*;

    init_operation!();

    /// Sends a message, and streams what the agent does, in the AI SDK's
    /// protocol.
    ///
    /// Meant for the AI SDK's `useChat`, with a transport that sends only the
    /// last message (Forma keeps the conversation; the SDK's default is to
    /// send all of it). The message is a `UIMessage`: an `id`, a `role`
    /// (`user`), and its `parts`. The answer is the SDK's UI message stream:
    /// `start`, text and reasoning parts, tool calls, files, Forma's own
    /// `data-*` parts, `finish`, and a last line with what the job consumed.
    ///
    /// A call that needs an answer is announced as a `tool-input-available`
    /// that gets no output: answer it with `/v1/resume`. How the
    /// conversation goes on otherwise is as with the `sync` client.
    #[utoipa::path(
        post,
        path = CHAT_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body(content = AiSdkV5Message, description = "The user's message, as the AI SDK sends it: `role` must be `user`, and `parts` cannot be empty. Attachments travel inline, and the body can be 2 MB at most."),
        responses(
            (status = OK, description = "A stream of Server-Sent Events: the AI SDK's UI message stream. Each `data:` line holds one JSON object: an event (with a `type`), or, last, the job's usage (without one).", body = AiSdkV5Line, content_type = "text/event-stream"),
            (status = BAD_REQUEST, description = "The body is not a user message with parts (a JSON `{\"error\": ...}`), or a header is missing (plain text)", content(
                (MessageRejection = "application/json"),
                (String = "text/plain"),
            )),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still answering the conversation's previous message: wait, and send it again", body = String, content_type = "text/plain"),
            (status = PAYLOAD_TOO_LARGE, description = "The body is larger than 2 MB (attachments included)", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The server has no `FORMA_AGENT_KEY`, or its storage failed", body = String, content_type = "text/plain"),
        )
    )]
    fn chat() {}

    /// Answers the tool call a conversation waits on, and waits for the
    /// agent's answer.
    ///
    /// The request is as with the `sync` client, and so is the answer (one
    /// JSON object, not a stream), but its message is in the AI SDK's shape.
    /// It does not stream because the AI SDK's handling of human in the loop
    /// does not work with a resumed stream.
    #[utoipa::path(
        post,
        path = RESUME_PATH,
        tag = CONVERSATIONS,
        params(ConversationHeaders),
        request_body = ResumeRequest,
        responses(
            (status = OK, description = "The agent answered (`completed`), stopped at another call (`suspended`), or waits for the turn's other calls (`still_suspended`)", body = Answer<ResumeResult<AiSdkV5Message>>),
            (status = BAD_REQUEST, description = "The call is not waiting for an answer (it does not exist, was answered, or a new message rejected it); the body is not valid JSON, or has both or neither of `arguments` and `result`; a header is missing; or the server keeps no conversations", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "No `X-API-Key`", body = String, content_type = "text/plain"),
            (status = FORBIDDEN, description = "The `X-API-Key` is wrong, or the conversation belongs to another user", body = String, content_type = "text/plain"),
            (status = NOT_FOUND, description = "There is no such conversation (or, multi-tenant, deployment)", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent is still working on the conversation", body = String, content_type = "text/plain"),
            (status = UNPROCESSABLE_ENTITY, description = "The body is not a resume request", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The agent failed", body = String, content_type = "text/plain"),
        )
    )]
    fn resume() {}
}

/// The tag of a chat app's webhook.
const WEBHOOK: &str = "Webhook";

/// The `whatsapp` client: a webhook of WhatsApp's Cloud API.
// Its functions only carry the operations' descriptions: they are never called.
#[allow(dead_code)]
mod whatsapp {
    use super::*;

    /// The query Meta verifies the webhook with.
    #[derive(IntoParams)]
    #[into_params(parameter_in = Query)]
    #[allow(dead_code)]
    struct Challenge {
        /// Always `subscribe`.
        #[param(rename = "hub.mode")]
        mode: String,
        /// What to answer with.
        #[param(rename = "hub.challenge")]
        challenge: String,
        /// The verify token chosen in Meta's App Dashboard: the server's
        /// `WHATSAPP_VERIFY_TOKEN` (or the deployment's `verify_token`).
        #[param(rename = "hub.verify_token")]
        verify_token: String,
    }

    /// Verifies the webhook (Meta calls it).
    ///
    /// Meta calls it when the webhook's URL or verify token is set in the
    /// App Dashboard. If the token is the server's, the server answers with
    /// the challenge, and Meta starts delivering messages.
    #[utoipa::path(
        get,
        path = CHAT_PATH,
        tag = WEBHOOK,
        params(
            Challenge,
            ("X-Deployment-ID" = Option<String>, Header, description = "Multi-tenant servers only: the deployment the webhook is for. Your reverse proxy adds it, from the URL it gave Meta for that deployment."),
        ),
        responses(
            (status = OK, description = "The challenge, as it was sent", body = String, content_type = "text/plain"),
            (status = BAD_REQUEST, description = "`hub.mode` is not `subscribe`, a query parameter is missing, or the server has no WhatsApp credentials", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "The verify token is not the server's", body = String, content_type = "text/plain"),
        )
    )]
    fn verify() {}

    /// Receives what WhatsApp delivers (Meta calls it).
    ///
    /// Every delivery must be signed by Meta with the App secret
    /// (`X-Hub-Signature-256`), or it is refused. Messages (text, pictures,
    /// voice notes, documents) are answered over WhatsApp: each sender has
    /// one conversation with the agent, whose id is their phone number.
    /// Reactions are stored on the message they react to; statuses are
    /// acknowledged. A message Meta delivers again is answered once, and one
    /// that arrives while the agent still answers the sender waits its turn.
    ///
    /// Calls that need an approval are asked with two buttons; delegated
    /// tools are answered at once, with an error (there is no client to run
    /// them). The answer to Meta is what the deliveries' jobs consumed.
    #[utoipa::path(
        post,
        path = CHAT_PATH,
        tag = WEBHOOK,
        params(
            ("X-Hub-Signature-256" = String, Header, description = "`sha256=` and the HMAC-SHA256 of the body, with the App secret (`WHATSAPP_APP_SECRET`, or the deployment's `app_secret`). Meta adds it."),
            ("X-Deployment-ID" = Option<String>, Header, description = "Multi-tenant servers only: the deployment the webhook is for. Your reverse proxy adds it, from the URL it gave Meta for that deployment, and passes the body and Meta's headers on unchanged."),
        ),
        request_body(content = Object, description = "A [WhatsApp Cloud API webhook payload](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/overview/), as Meta sends it."),
        responses(
            (status = OK, description = "The delivery was handled (or ignored: another number's, or a kind the agent does not handle). What answering it consumed, one entry per job.", body = Vec<UsageTally>),
            (status = BAD_REQUEST, description = "The body is not a WhatsApp delivery, or the server has no WhatsApp credentials", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "The signature is missing, malformed, or wrong", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent was still answering the sender's previous message after a while: Meta delivers it again later", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The message could not be stored or answered, and delivering it again may help", body = String, content_type = "text/plain"),
        )
    )]
    fn webhook() {}
}

/// The `telegram` client: a Telegram bot's webhook.
// Its functions only carry the operations' descriptions: they are never called.
#[allow(dead_code)]
mod telegram {
    use super::*;

    /// Receives what Telegram delivers (Telegram calls it).
    ///
    /// Every update must carry the webhook's secret token, or it is refused.
    /// Messages (text, pictures, voice notes, documents) are answered in the
    /// chat: each chat has one conversation with the agent, whose id is the
    /// chat's. A press on an approval's buttons answers the call it was sent
    /// for. An update Telegram delivers again is answered once, and one that
    /// arrives while the agent still answers the chat waits its turn.
    ///
    /// Register the webhook with Telegram's `setWebhook`, with the server's
    /// secret token. The answer to Telegram is what the update's jobs
    /// consumed.
    #[utoipa::path(
        post,
        path = CHAT_PATH,
        tag = WEBHOOK,
        params(
            ("X-Telegram-Bot-Api-Secret-Token" = String, Header, description = "The `secret_token` given to `setWebhook`: the server's `TELEGRAM_SECRET_TOKEN` (or the deployment's `secret_token`). Telegram adds it."),
            ("X-Deployment-ID" = Option<String>, Header, description = "Multi-tenant servers only: the deployment the webhook is for. Your reverse proxy adds it, from the URL it registered for that deployment, and passes the body and Telegram's headers on unchanged."),
        ),
        request_body(content = Object, description = "A [Telegram Update](https://core.telegram.org/bots/api#update), as Telegram sends it."),
        responses(
            (status = OK, description = "The update was handled (or ignored: a kind the agent does not handle). What answering it consumed, one entry per job.", body = Vec<UsageTally>),
            (status = BAD_REQUEST, description = "The body is not a Telegram update, or the server has no Telegram credentials", body = String, content_type = "text/plain"),
            (status = UNAUTHORIZED, description = "The secret token is missing or wrong", body = String, content_type = "text/plain"),
            (status = CONFLICT, description = "The agent was still answering the chat's previous message after a while: Telegram delivers it again later", body = String, content_type = "text/plain"),
            (status = INTERNAL_SERVER_ERROR, description = "The message could not be stored or answered, and delivering it again may help", body = String, content_type = "text/plain"),
        )
    )]
    fn webhook() {}
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma runtime API: the sync client",
        description = "A Forma runtime serves one agent (or, multi-tenant, every deployed \
agent) to one kind of client, which its runtime file names (`client:`). The paths are the \
same for every client; what they take and return is not. This describes one client.

**`client: sync`**: one JSON answer per request, once the agent is done. For back-ends that \
ask an agent something.

Requests come from your back-end, which authenticates the user and says who they are \
(`X-User-ID`): Forma checks the API key, not the user.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(sync::init, sync::chat, sync::resume),
    modifiers(&ApiKeyAuth),
    tags((name = "Conversations", description = "Start a conversation, write in it, and answer the tool calls it waits on.")),
    external_docs(url = "https://germolinal.github.io/forma/docs/api/runtime.html", description = "The runtime's API, explained"),
)]
struct SyncApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma runtime API: the sse client",
        description = "A Forma runtime serves one agent (or, multi-tenant, every deployed \
agent) to one kind of client, which its runtime file names (`client:`). The paths are the \
same for every client; what they take and return is not. This describes one client.

**`client: sse`**: the agent's work streamed as Server-Sent Events, as it happens. For apps \
that show what the agent does, through a back-end of their own.

Requests come from your back-end, which authenticates the user and says who they are \
(`X-User-ID`): Forma checks the API key, not the user.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(sse::init, sse::chat, sse::resume),
    modifiers(&ApiKeyAuth),
    tags((name = "Conversations", description = "Start a conversation, write in it, and answer the tool calls it waits on.")),
    external_docs(url = "https://germolinal.github.io/forma/docs/api/runtime.html", description = "The runtime's API, explained"),
)]
struct SseApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma runtime API: the ai-sdk-v5 client",
        description = "A Forma runtime serves one agent (or, multi-tenant, every deployed \
agent) to one kind of client, which its runtime file names (`client:`). The paths are the \
same for every client; what they take and return is not. This describes one client.

**`client: ai-sdk-v5`**: the protocol of [Vercel's AI SDK](https://ai-sdk.dev/) (v5), for \
front-ends built with it. The SDK sends only the last message, and Forma keeps the \
conversation.

Requests come from your back-end, which authenticates the user and says who they are \
(`X-User-ID`): Forma checks the API key, not the user.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(ai_sdk_v5::init, ai_sdk_v5::chat, ai_sdk_v5::resume),
    modifiers(&ApiKeyAuth),
    tags((name = "Conversations", description = "Start a conversation, write in it, and answer the tool calls it waits on.")),
    external_docs(url = "https://germolinal.github.io/forma/docs/api/runtime.html", description = "The runtime's API, explained"),
)]
struct AiSdkV5Api;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma runtime API: the whatsapp client",
        description = "A Forma runtime serves one agent (or, multi-tenant, every deployed \
agent) to one kind of client, which its runtime file names (`client:`). The paths are the \
same for every client; what they take and return is not. This describes one client.

**`client: whatsapp`**: the agent as the webhook of a WhatsApp Cloud API app. Meta calls \
it, and the agent answers over WhatsApp: nothing of yours calls it. It always keeps \
conversations, and needs no API key: every delivery is signed by Meta.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(whatsapp::verify, whatsapp::webhook),
    tags((name = "Webhook", description = "What the chat app calls.")),
    external_docs(url = "https://germolinal.github.io/forma/docs/how-to/whatsapp.html", description = "Your agent on WhatsApp"),
)]
struct WhatsappApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma runtime API: the telegram client",
        description = "A Forma runtime serves one agent (or, multi-tenant, every deployed \
agent) to one kind of client, which its runtime file names (`client:`). The paths are the \
same for every client; what they take and return is not. This describes one client.

**`client: telegram`**: the agent as a Telegram bot's webhook. Telegram calls it, and the \
agent answers in the chat: nothing of yours calls it. It always keeps conversations, and \
needs no API key: every update carries the webhook's secret token.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(telegram::webhook),
    tags((name = "Webhook", description = "What the chat app calls.")),
    external_docs(url = "https://germolinal.github.io/forma/docs/how-to/telegram.html", description = "Your agent on Telegram"),
)]
struct TelegramApi;

#[cfg(all(test, any(not(feature = "multi-tenant"), feature = "cli")))]
mod test {
    use std::sync::Arc;

    use agent::Agent;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use db::none::NoneDB;
    use genai_client::{
        SSEvent,
        events::{TokenUsage, UsageTally},
        openai::v1::{
            OpenAIAssistantMessage, OpenAIImagePart, OpenAIImageUrl, OpenAIMessageContent,
            OpenAIMessagePart, OpenAITextPart,
        },
    };
    use protocols::{DeploymentCredentials, FormaDB};
    use serde::Serialize;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::server::{
        RuntimeTrait,
        runtime::{Client, adapters::ClientSSE},
        state::AppState,
    };

    /// A server for `client`, with an agent that is never run.
    async fn router(client: &str) -> Router {
        let client: Client = serde_yaml::from_str(client).unwrap();
        let agent = Agent {
            id: "described".to_string(),
            ..Default::default()
        };
        #[cfg(feature = "hot-reload")]
        let agent = tokio::sync::RwLock::new(agent);
        let state = AppState {
            agent: Arc::new(agent),
            db: Arc::new(NoneDB::new().await.unwrap()),
            credentials: Arc::new(DeploymentCredentials::default()),
        };
        RuntimeTrait::<NoneDB, AppState<NoneDB>>::build_app(&client, Router::new())
            .unwrap()
            .with_state(state)
    }

    /// The status `router` answers `method path` with: a body it cannot
    /// read, but on a route that exists.
    async fn status(router: &Router, method: &str, path: &str) -> StatusCode {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        router.clone().oneshot(request).await.unwrap().status()
    }

    /// The operations a description lists, as `(METHOD, path)`.
    fn operations(api: &str) -> Vec<(String, String)> {
        let spec = spec(api).unwrap();
        let mut operations = vec![];
        for (path, item) in spec["paths"].as_object().unwrap() {
            for method in item.as_object().unwrap().keys() {
                operations.push((method.to_uppercase(), path.clone()));
            }
        }
        operations
    }

    #[tokio::test]
    async fn every_operation_described_is_served() {
        for (api, client) in [
            ("sync", "client: sync\nno_api_key: true"),
            ("sse", "client: sse\nno_api_key: true"),
            ("ai-sdk-v5", "client: ai-sdk-v5\nno_api_key: true"),
            ("whatsapp", "client: whatsapp"),
            ("telegram", "client: telegram"),
        ] {
            let router = router(client).await;
            let operations = operations(api);
            assert!(!operations.is_empty(), "{api} describes nothing");
            for (method, path) in operations {
                let status = status(&router, &method, &path).await;
                assert!(
                    status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED,
                    "{api}: {method} {path} is described, but not served ({status})"
                );
            }
            // And the check can tell: what is not served is answered so.
            let status = status(&router, "PATCH", CHAT_PATH).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{api}");
        }
    }

    #[test]
    fn every_api_is_described() {
        for api in APIS {
            let spec = spec(api).unwrap();
            assert_eq!(spec["openapi"], "3.1.0", "{api}");
            assert!(spec["paths"].as_object().is_some_and(|p| !p.is_empty()));
        }
        assert!(spec("nope").is_err());
    }

    /// What is wrong with `value`, as serde sends it, as the schema `schema`
    /// of `api`'s description sees it.
    #[track_caller]
    fn errors<T: Serialize>(api: &str, schema: &str, value: &T) -> Vec<String> {
        let mut root = spec(api).unwrap();
        assert!(
            root["components"]["schemas"].get(schema).is_some(),
            "{api} has no schema '{schema}'"
        );
        root["$ref"] = json!(format!("#/components/schemas/{schema}"));
        let validator = jsonschema::validator_for(&root).unwrap();
        let value = serde_json::to_value(value).unwrap();
        validator
            .iter_errors(&value)
            .map(|e| format!("{e} (at {})", e.instance_path))
            .collect()
    }

    /// Checks `value`, as serde sends it, against the schema `schema` of
    /// `api`'s description.
    #[track_caller]
    fn conforms<T: Serialize>(api: &str, schema: &str, value: &T) {
        let errors = errors(api, schema, value);
        let value = serde_json::to_value(value).unwrap();
        assert!(
            errors.is_empty(),
            "{api}: {value} is not a {schema}: {errors:?}"
        );
    }

    #[test]
    fn the_schemas_refuse_what_is_not_sent() {
        for (api, schema, value) in [
            ("sse", "SseLine", json!({"type": "nope"})),
            ("sse", "SseLine", json!({"type": "text_delta", "id": "t"})),
            (
                "ai-sdk-v5",
                "AiSdkV5Line",
                json!({"type": "data-weather", "data": {}}),
            ),
            ("sync", "ChatAnswer", json!({"status": "done", "usage": {}})),
            (
                "sync",
                "ChatAnswer",
                json!({"status": "completed", "data": {"role": "assistant"}}),
            ),
            ("sync", "OpenAIChatMessage", json!({"role": "user"})),
            ("sync", "ResumeRequest", json!({"reject": true})),
            ("manager", "SessionData", json!({"user_id": "user"})),
        ] {
            assert!(
                !errors(api, schema, &value).is_empty(),
                "{api}: {value} passes as a {schema}"
            );
        }
    }

    fn text(t: &str) -> OpenAIMessagePart {
        OpenAIMessagePart::Text(OpenAITextPart {
            text: t.to_string(),
        })
    }

    fn picture() -> OpenAIMessagePart {
        OpenAIMessagePart::ImageUrl(OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                detail: None,
            },
        })
    }

    /// One of every event, as an agent's run streams them.
    fn events() -> Vec<SSEvent> {
        vec![
            SSEvent::Start {
                message_id: "job".into(),
            },
            SSEvent::StartStep { id: "reply".into() },
            SSEvent::TextStart { id: "t".into() },
            SSEvent::TextDelta {
                id: "t".into(),
                delta: "Hello".into(),
            },
            SSEvent::TextEnd { id: "t".into() },
            SSEvent::ReasoningStart { id: "r".into() },
            SSEvent::ReasoningDelta {
                id: "r".into(),
                delta: "Hm".into(),
            },
            SSEvent::ReasoningEnd { id: "r".into() },
            SSEvent::ToolInputStart {
                tool_call_id: "call".into(),
                tool_name: "current-date".into(),
                tool_type: "current-date".into(),
                bundle: None,
            },
            SSEvent::ToolInputAvailable {
                tool_call_id: "call".into(),
                tool_name: "current-date".into(),
                tool_kind: "current-date".into(),
                input: json!({"timezone": "UTC"}),
            },
            SSEvent::ToolOutputAvailable {
                tool_call_id: "call".into(),
                output: json!({"date": "2026-10-23"}),
            },
            SSEvent::RequestFeedback {
                tool_call_id: "call".into(),
                node_id: "reply".into(),
                tool_name: "refund".into(),
                arguments: json!({"amount": 10}),
                from_root: true,
                tool_type: "api-call".into(),
                bundle: Some("payments".into()),
            },
            SSEvent::StillSuspended,
            SSEvent::Reference {
                title: "Forma".into(),
                url: "https://example.com".into(),
                snippet: Some("A runtime".into()),
                extractive_answer: None,
            },
            SSEvent::GroundingScore { score: 0.9 },
            SSEvent::FileAvailable {
                id: "t".into(),
                part: picture(),
            },
            SSEvent::Error {
                error: "it failed".into(),
            },
            SSEvent::Finish,
        ]
    }

    /// What a run that called a model and a tool reports.
    fn usage() -> UsageTally {
        let mut usage = UsageTally::new();
        usage.add(&SSEvent::Start {
            message_id: "job".into(),
        });
        usage.add(&SSEvent::Usage {
            node_id: "reply".into(),
            model: "gemini".into(),
            provider: "gcp.vertex_ai".into(),
            server_address: "aiplatform.googleapis.com".into(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        });
        usage.add(&SSEvent::ToolUsage {
            tool_kind: "code-execution".into(),
            unit: "second".into(),
            quantity: 3,
        });
        usage
    }

    /// `result`, as a JSON answer carries it: with the job's usage.
    fn answered<R>(result: R) -> Answer<R> {
        Answer {
            result,
            usage: usage(),
        }
    }

    fn answer() -> OpenAIChatMessage {
        OpenAIChatMessage::Assistant(OpenAIAssistantMessage {
            content: Some(OpenAIMessageContent::Parts(vec![text("Hi"), picture()])),
            ..Default::default()
        })
    }

    fn suspended<O>() -> CompletionResult<O> {
        CompletionResult::Suspended {
            session_id: "session".into(),
            tool_call_id: "call".into(),
            tool_name: "refund".into(),
            arguments: json!({"amount": 10}),
        }
    }

    #[test]
    fn what_requests_carry_is_described() {
        let message: OpenAIChatMessage = serde_json::from_value(json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "What is this?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}},
                {"type": "file", "file": {"file_data": "data:application/pdf;base64,JVBERi0=", "filename": "a.pdf"}},
                {"type": "input_audio", "input_audio": {"data": "AAECAw==", "format": "wav"}},
            ],
        }))
        .unwrap();
        let plain: OpenAIChatMessage =
            serde_json::from_value(json!({"role": "user", "content": "Hi"})).unwrap();
        for api in ["sync", "sse"] {
            conforms(api, "OpenAIChatMessage", &message);
            conforms(api, "OpenAIChatMessage", &plain);
        }

        let message: AiSdkV5Message = serde_json::from_value(json!({
            "id": "m1",
            "role": "user",
            "parts": [{"type": "text", "text": "Hi"}],
        }))
        .unwrap();
        conforms("ai-sdk-v5", "AiSdkV5Message", &message);

        for resume in [
            json!({"tool_call_id": "call", "reject": false, "arguments": {"amount": 10}}),
            json!({"tool_call_id": "call", "reject": false, "result": {"items": []}}),
            json!({"tool_call_id": "call", "reject": true}),
        ] {
            let resume: ResumeRequest = serde_json::from_value(resume).unwrap();
            for api in ["sync", "sse", "ai-sdk-v5"] {
                conforms(api, "ResumeRequest", &resume);
            }
        }
    }

    #[test]
    fn what_the_sync_client_answers_is_described() {
        conforms("sync", "NewSessionResult", &NewSessionResult::default());
        let completed = CompletionResult::Completed(answer());
        conforms("sync", "ChatAnswer", &answered(completed));
        conforms(
            "sync",
            "ChatAnswer",
            &answered(suspended::<OpenAIChatMessage>()),
        );
        for resumed in [
            ResumeResult::Completed(answer()),
            ResumeResult::Suspended {
                session_id: "session".into(),
                tool_call_id: "call".into(),
                tool_name: "refund".into(),
                arguments: json!({}),
            },
            ResumeResult::StillSuspended,
        ] {
            conforms("sync", "ResumeAnswer", &answered(resumed));
        }
    }

    #[test]
    fn what_the_streams_send_is_described() {
        for event in events() {
            conforms("sse", "SseLine", &event);
            if let Some(event) = AiSdkV5Event::translate(event) {
                conforms("ai-sdk-v5", "AiSdkV5Line", &event);
            }
        }
        conforms("sse", "SseLine", &usage());
        conforms("ai-sdk-v5", "AiSdkV5Line", &usage());

        let answer: AiSdkV5Message = genai_client::LLMMessage::from(answer()).into();
        conforms(
            "ai-sdk-v5",
            "ResumeAnswer",
            &answered(ResumeResult::Completed(answer)),
        );
        conforms(
            "ai-sdk-v5",
            "ResumeAnswer",
            &Answer {
                result: ResumeResult::<AiSdkV5Message>::StillSuspended,
                usage: UsageTally::new_empty(),
            },
        );
    }

    #[test]
    fn what_webhooks_answer_is_described() {
        for api in ["whatsapp", "telegram"] {
            conforms(api, "UsageTally", &usage());
        }
    }

    #[test]
    fn what_the_manager_answers_is_described() {
        use protocols::{
            FormaMessage, FormaMessageBody, FormaMessagePart, FormaTextPart, PendingToolCall,
            SessionData, SessionSummary,
        };

        let mut question = FormaMessage::new_user();
        question.content = FormaMessageBody::User {
            content: vec![FormaMessagePart::Text(FormaTextPart {
                text: "Refund me".into(),
            })],
        };
        question.created_at = Some(chrono::Utc::now());
        let mut answer = FormaMessage::new_assistant();
        answer.content = FormaMessageBody::Assistant {
            content: vec![],
            tool_calls: vec![
                serde_json::from_value(json!({
                    "type": "function",
                    "id": "call",
                    "function": {"name": "refund", "arguments": "{\"amount\":10}"},
                }))
                .unwrap(),
            ],
            thought_signature: Some(serde_json::from_value(json!([115, 105, 103])).unwrap()),
            tool_bundles: Default::default(),
            tool_types: [("call".to_string(), "api-call".to_string())].into(),
        };
        let session = SessionData {
            id: Some(protocols::SessionId("6716a3c1f1a2b3c4d5e6f708".into())),
            created_at: Some(chrono::Utc::now()),
            user_id: "user".into(),
            agent_id: "deployment".into(),
            name: Some("Refunds".into()),
            messages: vec![question, answer],
            pending_tool_calls: vec![PendingToolCall {
                tool_call_id: "call".into(),
                node_id: "reply".into(),
                tool_name: "refund".into(),
                arguments: json!({"amount": 10}),
                from_root: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        conforms("manager", "SessionData", &session);
        conforms(
            "manager",
            "SessionSummary",
            &SessionSummary {
                id: "6716a3c1f1a2b3c4d5e6f708".into(),
                created_at: Some(chrono::Utc::now()),
                display_name: "Refund me".into(),
            },
        );
        conforms(
            "manager",
            "CheckAgentResponse",
            &crate::manager::CheckAgentResponse {
                mcp_definitions: vec![],
                features: Default::default(),
            },
        );
    }
}
