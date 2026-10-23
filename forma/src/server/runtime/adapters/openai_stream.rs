use crate::server::runtime::adapters::{NewSessionResult, create_session, resume_stream, stream};
use crate::server::runtime::{RuntimeTrait, default_context_window_messages};
use crate::server::state::RuntimeState;
use crate::server::{get_user_id, msg_extractor::UserChatMessage};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
    routing::{get, post},
};
use derive::FormaDocs;
use futures::stream::Stream;
use genai_client::{OpenAIChatMessage, SSEvent};
use protocols::{
    CHAT_PATH, DeploymentCredentials, FormaDB, FormaError, INIT_SESSION_PATH, RESUME_PATH,
    ResumeRequest,
};
use serde::{Deserialize, Serialize};

/// Offers a single endpoint, with a Chat Completion, returning
/// a set of SSE
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct OpenAISSEAdapter {
    /// Option to NOT require an API Key. If false, requests should include an
    /// `X-API-Key: $KEY` header, which will be compared to the
    /// environment variable `FORMA_AGENT_KEY` in the server.
    #[serde(default)]
    pub no_api_key: bool,

    /// The maximum number of messages to retrieve from the
    /// sessions database in each user interaction. The purpose
    /// is to prevent (1) the system prompt from being diluted by
    /// a large number of messages, and (2) reduce the token usage.
    #[serde(default = "default_context_window_messages")]
    pub(crate) max_history_messages: usize,

    /// The largest chat request body accepted, in megabytes.
    ///
    /// Attachments travel inline as base64, which inflates them by about a
    /// third, so this is what bounds the size of the files a user can send
    /// rather than just the length of their text. Axum's own default is 2 MB,
    /// which a single phone photo already exceeds.
    #[serde(default = "default_max_request_mb")]
    pub(crate) max_request_mb: usize,
}

pub(crate) fn default_max_request_mb() -> usize {
    25
}

impl Default for OpenAISSEAdapter {
    fn default() -> Self {
        Self {
            no_api_key: false,
            max_history_messages: default_context_window_messages(),
            max_request_mb: default_max_request_mb(),
        }
    }
}

impl OpenAISSEAdapter {
    async fn create_session<DB: FormaDB, T: RuntimeState<DB>>(
        headers: HeaderMap,
        State(state): State<T>,
    ) -> Result<Json<NewSessionResult>, (StatusCode, String)> {
        let db = state.db().clone();
        let user_id = get_user_id(db.persist(), &headers)?;
        let (agent, ..) = state.__agent_and_credentials(&headers).await?;
        #[cfg(feature = "hot-reload")]
        let agent_id = agent.read().await.id.clone();
        #[cfg(not(feature = "hot-reload"))]
        let agent_id = agent.id.clone();

        create_session(db, &agent_id, &user_id).await
    }

    async fn resume<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        Json(req): Json<ResumeRequest>,
    ) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let result = resume_stream::<DB, OpenAIChatMessage, SSEvent>(args, req).await?;
        Ok(result)
    }

    async fn stream<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        UserChatMessage(message): UserChatMessage<OpenAIChatMessage>,
    ) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let r = stream::<DB, OpenAIChatMessage, SSEvent>(args, message).await?;
        Ok(r)
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for OpenAISSEAdapter {
    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }
    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        Ok(DeploymentCredentials::default())
    }
    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError> {
        let n_messages = self.max_history_messages;

        // Only the chat route carries attachments, so the raised limit is
        // scoped to it; every other route keeps axum's 2 MB default.
        let mut app = app.route(
            CHAT_PATH,
            post(move |h, s, m| Self::stream::<DB, T>(n_messages, h, s, m))
                .layer(DefaultBodyLimit::max(self.max_request_mb * 1024 * 1024)),
        );
        app = app.route(
            RESUME_PATH,
            post(move |h, s, m| Self::resume::<DB, T>(n_messages, h, s, m)),
        );
        app = app.route(
            INIT_SESSION_PATH,
            get(move |h, s| Self::create_session::<DB, T>(h, s)),
        );
        Ok(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai_client::{
        GenaiMessageTrait, LLMMessage, Role,
        openai::v1::{OpenAIMessageContent, OpenAIMessagePart},
    };
    use serde_json::json;

    /// The parts of a user message, as the extractor would see them.
    ///
    /// These cases are the exact payloads `src/cli/ui/index.html` builds, so a
    /// change to either side that breaks the other fails here rather than at
    /// runtime with an opaque 400.
    fn user_parts(body: serde_json::Value) -> Vec<OpenAIMessagePart> {
        let msg: OpenAIChatMessage =
            serde_json::from_value(body).expect("the UI payload should deserialize");

        // What `UserChatMessage` checks before the message reaches the agent.
        assert!(msg.has_content());
        assert_eq!(msg.role(), Role::User);

        match msg {
            OpenAIChatMessage::User(m) => match m.content {
                OpenAIMessageContent::Parts(p) => p,
                OpenAIMessageContent::String(_) => panic!("expected parts, got a string"),
            },
            _ => panic!("expected a user message"),
        }
    }

    #[test]
    fn text_only_message_still_parses() {
        let parts = user_parts(json!({
            "id": "some-uuid",
            "role": "user",
            "content": [{ "type": "text", "text": "hello" }],
        }));

        assert_eq!(parts.len(), 1);
        match &parts[0] {
            OpenAIMessagePart::Text(t) => assert_eq!(t.text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn image_attachment_parses_as_a_data_url() {
        let parts = user_parts(json!({
            "role": "user",
            "content": [
                {
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" },
                },
                { "type": "text", "text": "what is this?" },
            ],
        }));

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            OpenAIMessagePart::ImageUrl(p) => {
                assert_eq!(p.image_url.url, "data:image/png;base64,iVBORw0KGgo=");
            }
            other => panic!("expected an image, got {other:?}"),
        }
    }

    #[test]
    fn file_and_audio_attachments_parse() {
        let parts = user_parts(json!({
            "role": "user",
            "content": [
                {
                    "type": "file",
                    "file": {
                        "file_data": "data:application/pdf;base64,JVBERi0=",
                        "filename": "report.pdf",
                    },
                },
                {
                    "type": "input_audio",
                    "input_audio": { "data": "AAECAw==", "format": "wav" },
                },
            ],
        }));

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            OpenAIMessagePart::File(p) => {
                assert_eq!(p.file.filename.as_deref(), Some("report.pdf"));
                assert!(
                    p.file
                        .file_data
                        .as_deref()
                        .unwrap_or_default()
                        .starts_with("data:application/pdf;base64,")
                );
            }
            other => panic!("expected a file, got {other:?}"),
        }
        match &parts[1] {
            OpenAIMessagePart::InputAudio(p) => assert_eq!(p.input_audio.data, "AAECAw=="),
            other => panic!("expected audio, got {other:?}"),
        }
    }

    #[test]
    fn attachments_survive_the_conversion_the_agent_sees() {
        // `stream` hands the message on as an `LLMMessage`, so the parts have
        // to survive that hop to reach a provider at all.
        let msg: OpenAIChatMessage = serde_json::from_value(json!({
            "role": "user",
            "content": [
                {
                    "type": "file",
                    "file": {
                        "file_data": "data:application/pdf;base64,JVBERi0=",
                        "filename": "report.pdf",
                    },
                },
                { "type": "text", "text": "summarise this" },
            ],
        }))
        .expect("should deserialize");

        match LLMMessage::from(msg) {
            LLMMessage::User(m) => {
                assert_eq!(m.content.len(), 2);
                assert!(matches!(m.content[0], OpenAIMessagePart::File(_)));
                assert!(matches!(m.content[1], OpenAIMessagePart::Text(_)));
            }
            other => panic!("expected a user message, got {other:?}"),
        }
    }

    #[test]
    fn body_limit_leaves_room_for_attachments() {
        // Axum's default is 2 MB, which a single phone photo exceeds; the
        // point of the setting is to be comfortably above that.
        let adapter = OpenAISSEAdapter::default();
        assert!(adapter.max_request_mb * 1024 * 1024 > 2 * 1024 * 1024);
    }
}
