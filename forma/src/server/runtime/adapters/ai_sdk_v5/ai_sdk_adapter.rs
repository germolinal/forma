use super::{message::AiSdkV5Message, sse::AiSdkV5Event};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
    routing::{get, post},
};
use derive::FormaDocs;
use futures::stream::Stream;
use protocols::{
    CHAT_PATH, DeploymentCredentials, FormaDB, FormaError, INIT_SESSION_PATH, RESUME_PATH,
    ResumeRequest,
};
use serde::{Deserialize, Serialize};

use crate::{
    RuntimeTrait,
    server::{
        get_user_id,
        msg_extractor::UserChatMessage,
        runtime::{
            adapters::{NewSessionResult, ResumeResult, create_session, resume_completion, stream},
            default_context_window_messages,
        },
        state::RuntimeState,
    },
};

/// Meant to be called from the Vercel AI SDK v5-compatible client.
///
/// It offers 3 basic endpoints:
/// - `v1/init` to initiate a session
/// - `v1/chat` to send a message and get a streamed response
/// - `v1/resume` to resume a session
///
/// Note that `v1/resume` does not use SSE streaming, but a synchronous response.
/// This is due to some issues we found when trying to incorporate v6 and human-in-the-loop
/// capabilities. [Issue here (not by us)](https://github.com/vercel/ai/issues/9980)
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct AiSdkV5Adapter {
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

impl std::default::Default for AiSdkV5Adapter {
    fn default() -> Self {
        Self {
            no_api_key: false,
            max_history_messages: default_context_window_messages(),
        }
    }
}

impl AiSdkV5Adapter {
    async fn resume<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        Json(req): Json<ResumeRequest>,
    ) -> Result<Json<ResumeResult<AiSdkV5Message>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let a = resume_completion::<DB, AiSdkV5Message>(args, req).await?;
        Ok(Json(a.result))
    }

    async fn stream<DB: FormaDB, T: RuntimeState<DB>>(
        n_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        UserChatMessage(message): UserChatMessage<AiSdkV5Message>,
    ) -> Result<Sse<impl Stream<Item = Result<Event, FormaError>>>, (StatusCode, String)> {
        let args = state.job_args(&headers, n_messages).await?;
        let r = stream::<DB, AiSdkV5Message, AiSdkV5Event>(args, message).await?;
        Ok(r)
    }

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
        create_session(db.clone(), &agent_id, &user_id).await
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for AiSdkV5Adapter {
    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }
    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        // no credentials needed
        Ok(DeploymentCredentials::default())
    }

    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError> {
        let n_messages = self.max_history_messages;

        let mut app = app.route(
            CHAT_PATH,
            post(move |h, s, m| Self::stream::<DB, T>(n_messages, h, s, m)),
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
