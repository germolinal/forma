use std::sync::Arc;

use protocols::{
    AgentDeployment, ChatMessageState, DeploymentCredentials, EncryptedTokens, FormaDB, FormaError,
    FormaMessage, PendingToolCall, SessionData, SessionId, SessionSummary,
};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

/// A backend that does not store any data.
#[derive(Clone, Debug, Default)]
pub struct NoneDB {}

#[async_trait::async_trait]
impl FormaDB for NoneDB {
    async fn get_agent_deployment(&self, agent_id: &str) -> Result<AgentDeployment, FormaError> {
        Err(FormaError::AgentDeploymentNotFound(agent_id.to_string()))
    }

    async fn new_agent_deployment(
        &self,
        _user_id: &str,
        _agent: Value,
        _credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        Ok(String::new())
    }
    async fn update_agent_deployment(
        &self,
        _user_id: &str,
        _deployment_id: &str,
        _agent: Value,
        _credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        Ok(String::new())
    }

    async fn remove_agent_deployment(&self, deployment_id: &str) -> Result<String, FormaError> {
        Ok(deployment_id.to_string())
    }

    fn persist(&self) -> bool {
        false
    }

    async fn new() -> Result<Self, FormaError> {
        tracing::info!("using NoneDB backend");
        Ok(Self::default())
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError> {
        Err(FormaError::MessageNotFound(message_id.to_string()))
    }

    async fn add_msg_reaction(
        &self,
        _session_id: &str,
        _message_id: &str,
        _emoji: &Option<String>,
    ) -> Result<(), FormaError> {
        Ok(())
    }

    async fn get_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
        _n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        Err(FormaError::SessionNotFound(format!(
            "u-{user_id}:a-{agent_id}"
        )))
    }

    async fn delete_session(&self, session_id: &str, _user_id: &str) -> Result<(), FormaError> {
        Err(FormaError::SessionNotFound(session_id.to_string()))
    }

    async fn rename_session(
        &self,
        session_id: &str,
        _user_id: &str,
        _name: Option<&str>,
    ) -> Result<(), FormaError> {
        Err(FormaError::SessionNotFound(session_id.to_string()))
    }

    async fn get_user_sessions(
        &self,
        _agent_id: &str,
        _user_id: &str,
        _n_sessions: usize,
    ) -> Result<Vec<SessionSummary>, FormaError> {
        Ok(Vec::new())
    }

    async fn update_msg_status(
        &self,
        _session_id: &str,
        _message_id: &str,
        _status: ChatMessageState,
    ) -> Result<(), FormaError> {
        Ok(())
    }

    async fn suspend_session(
        &self,
        session: Arc<RwLock<SessionData>>,
        call: &PendingToolCall,
    ) -> Result<(), FormaError> {
        let mut session = session.write().await;
        session.pending_tool_calls.push(call.clone());
        Ok(())
    }

    async fn get_session_by_id(
        &self,
        session_id: &str,
        _n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        Err(FormaError::SessionNotFound(session_id.to_string()))
    }

    async fn clear_state(&self, _session_id: &str) -> Result<(), FormaError> {
        Ok(())
    }

    async fn save_pending_tool_calls(&self, _session: &SessionData) -> Result<(), FormaError> {
        Ok(())
    }

    async fn start_run(
        &self,
        _session_id: &str,
        _stale_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, FormaError> {
        // Nothing is stored, so there is no shared history to protect.
        Ok(true)
    }

    async fn finish_run(&self, _session_id: &str) -> Result<(), FormaError> {
        Ok(())
    }

    async fn __append_msg(&self, msg: FormaMessage) -> Result<FormaMessage, FormaError> {
        Ok(msg)
    }

    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }

    async fn get_oauth_tokens(&self, _user_id: &str) -> Result<EncryptedTokens, FormaError> {
        Ok(Default::default())
    }

    async fn save_oauth_tokens(
        &self,
        _user_id: &str,
        _tokens: EncryptedTokens,
    ) -> Result<(), FormaError> {
        Ok(())
    }

    async fn remove_oauth_tokens(&self, _user_id: &str) -> Result<(), FormaError> {
        Ok(())
    }

    async fn new_session(&self, agent_id: &str, user_id: &str) -> Result<SessionData, FormaError> {
        let session = SessionData {
            user_id: user_id.to_string(),
            agent_id: agent_id.to_string(),
            id: Some(SessionId(Uuid::new_v4().to_string())),
            ..Default::default()
        };
        Ok(session)
    }
}
