use crate::server::{
    RuntimeTrait,
    runtime::{
        adapters::{
            CompletionResult, NewSessionResult, ResumeResult, completion, create_session,
            resume_completion,
        },
        default_context_window_messages,
    },
    state::RuntimeState,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use derive::FormaDocs;
use genai_client::{
    OpenAIChatMessage,
    openai::v1::{OpenAIAssistantMessage, OpenAIFunctionToolChoice, OpenAIToolChoice},
};
use serde::{Deserialize, Serialize};

use crate::server::{get_user_id, msg_extractor::UserChatMessage};
use protocols::{
    CHAT_PATH, DeploymentCredentials, FormaDB, FormaError, INIT_SESSION_PATH, RESUME_PATH,
    ResumeRequest,
};

/// Offers a single endpoint, with a Chat Completion, returning
/// a fully resolved OpenAI message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct OpenAISyncAdapter {
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
}

impl Default for OpenAISyncAdapter {
    fn default() -> Self {
        Self {
            no_api_key: false,
            max_history_messages: default_context_window_messages(),
        }
    }
}

impl OpenAISyncAdapter {
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

    async fn chat<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        UserChatMessage(message): UserChatMessage<OpenAIChatMessage>,
    ) -> Result<Json<CompletionResult<OpenAIChatMessage>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let result = completion(args, message).await?;
        Ok(Json(result.result))
    }

    async fn resume<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        Json(req): Json<ResumeRequest>,
    ) -> Result<Json<ResumeResult<OpenAIChatMessage>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let result = resume_completion::<DB, OpenAIChatMessage>(args, req).await?;
        Ok(Json(result.result))
    }
}

impl From<CompletionResult<OpenAIChatMessage>> for OpenAIChatMessage {
    fn from(result: CompletionResult<OpenAIChatMessage>) -> Self {
        match result {
            CompletionResult::Completed(msg) => msg,
            CompletionResult::Suspended {
                tool_call_id,
                tool_name,
                arguments,
                ..
            } => {
                let arguments = serde_json::to_string(&arguments).unwrap_or_default();
                OpenAIChatMessage::Assistant(OpenAIAssistantMessage {
                    tool_calls: vec![OpenAIToolChoice::Function {
                        id: tool_call_id,
                        function: OpenAIFunctionToolChoice {
                            name: tool_name,
                            arguments,
                        },
                    }],
                    ..Default::default()
                })
            }
        }
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for OpenAISyncAdapter {
    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }
    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        Ok(DeploymentCredentials::default())
    }

    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError> {
        let n_messages = self.max_history_messages;

        let mut app = app.route(
            CHAT_PATH,
            post(move |h, s, m| Self::chat::<DB, T>(n_messages, h, s, m)),
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
    use genai_client::openai::v1::OpenAIMessageContent;
    use serde_json::json;

    fn completed_assistant_msg() -> OpenAIChatMessage {
        OpenAIChatMessage::Assistant(OpenAIAssistantMessage {
            content: Some(OpenAIMessageContent::String("Hello!".to_string())),
            ..Default::default()
        })
    }

    #[test]
    fn completed_result_returns_message_unchanged() {
        let msg = completed_assistant_msg();
        let result: CompletionResult<OpenAIChatMessage> = CompletionResult::Completed(msg.clone());
        let out: OpenAIChatMessage = result.into();
        // Should round-trip through JSON identically
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            serde_json::to_string(&msg).unwrap(),
        );
    }

    #[test]
    fn suspended_result_returns_assistant_with_tool_call() {
        let arguments = json!({"city": "Auckland"});
        let result: CompletionResult<OpenAIChatMessage> = CompletionResult::Suspended {
            session_id: "sess-1".to_string(),
            tool_call_id: "call-42".to_string(),
            tool_name: "get_weather".to_string(),
            arguments: arguments.clone(),
        };

        let out: OpenAIChatMessage = result.into();
        let OpenAIChatMessage::Assistant(inner) = out else {
            panic!("expected Assistant variant");
        };

        assert!(inner.content.is_none());
        assert_eq!(inner.tool_calls.len(), 1);

        let OpenAIToolChoice::Function { id, function } = &inner.tool_calls[0] else {
            panic!("expected Function tool choice");
        };
        assert_eq!(id, "call-42");
        assert_eq!(function.name, "get_weather");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&function.arguments).unwrap(),
            arguments,
        );
    }
}
