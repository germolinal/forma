use std::sync::Arc;

use protocols::{
    AgentDeployment, ChatMessageState, DeploymentCredentials, EncryptedTokens, FormaDB, FormaError,
    FormaMessage, PendingToolCall, SessionData, SessionSummary,
};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{NoneDB, file::FileDB, mongo::MongoDB};

/// A Forma Database client that connects to different backends.
#[derive(Clone, Debug)]
pub enum MultiDB {
    /// A MongoDB backend.
    Mongo(MongoDB),
    /// No backend, no persistence.
    None(NoneDB),
    /// A file-based backend.
    File(FileDB),
}

impl MultiDB {
    /// Creates a new `MultiDB` instance with a `NoneDB` backend.
    pub async fn new_none() -> Result<Self, FormaError> {
        Ok(Self::None(NoneDB::new().await?))
    }

    /// Creates a new `MultiDB` instance with a `MongoDBClient` backend.
    pub async fn new_mongo() -> Result<Self, FormaError> {
        Ok(Self::Mongo(MongoDB::new().await?))
    }

    /// Creates a new `MultiDB` instance with a `FileDB` backend.
    pub async fn new_file() -> Result<Self, FormaError> {
        Ok(Self::File(FileDB::new().await?))
    }
}

#[async_trait::async_trait]
impl FormaDB for MultiDB {
    async fn remove_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
    ) -> Result<String, FormaError> {
        match self {
            Self::Mongo(db) => db.remove_agent_deployment(user_id, deployment_id).await,
            Self::None(db) => db.remove_agent_deployment(user_id, deployment_id).await,
            Self::File(db) => db.remove_agent_deployment(user_id, deployment_id).await,
        }
    }
    async fn remove_oauth_tokens(&self, user_id: &str) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.remove_oauth_tokens(user_id).await,
            Self::None(db) => db.remove_oauth_tokens(user_id).await,
            Self::File(db) => db.remove_oauth_tokens(user_id).await,
        }
    }

    async fn get_agent_deployment(&self, agent_id: &str) -> Result<AgentDeployment, FormaError> {
        match self {
            Self::Mongo(db) => db.get_agent_deployment(agent_id).await,
            Self::None(db) => db.get_agent_deployment(agent_id).await,
            Self::File(db) => db.get_agent_deployment(agent_id).await,
        }
    }

    async fn new_agent_deployment(
        &self,
        user_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        match self {
            Self::Mongo(db) => db.new_agent_deployment(user_id, agent, credentials).await,
            Self::None(db) => db.new_agent_deployment(user_id, agent, credentials).await,
            Self::File(db) => db.new_agent_deployment(user_id, agent, credentials).await,
        }
    }

    async fn update_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        match self {
            Self::Mongo(db) => {
                db.update_agent_deployment(user_id, deployment_id, agent, credentials)
                    .await
            }
            Self::None(db) => {
                db.update_agent_deployment(user_id, deployment_id, agent, credentials)
                    .await
            }
            Self::File(db) => {
                db.update_agent_deployment(user_id, deployment_id, agent, credentials)
                    .await
            }
        }
    }

    fn persist(&self) -> bool {
        match self {
            Self::Mongo(db) => db.persist(),
            Self::None(db) => db.persist(),
            Self::File(db) => db.persist(),
        }
    }

    async fn new() -> Result<Self, FormaError> {
        panic!("MultiDB::new() should not be called directly")
    }

    fn init(&mut self) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.init(),
            Self::None(db) => db.init(),
            Self::File(db) => db.init(),
        }
    }

    async fn get_oauth_tokens(&self, user_id: &str) -> Result<EncryptedTokens, FormaError> {
        match self {
            Self::Mongo(db) => db.get_oauth_tokens(user_id).await,
            Self::None(db) => db.get_oauth_tokens(user_id).await,
            Self::File(db) => db.get_oauth_tokens(user_id).await,
        }
    }

    async fn save_oauth_tokens(
        &self,
        user_id: &str,
        tokens: EncryptedTokens,
    ) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.save_oauth_tokens(user_id, tokens).await,
            Self::None(db) => db.save_oauth_tokens(user_id, tokens).await,
            Self::File(db) => db.save_oauth_tokens(user_id, tokens).await,
        }
    }

    async fn new_session(&self, agent_id: &str, user_id: &str) -> Result<SessionData, FormaError> {
        match self {
            Self::Mongo(db) => db.new_session(agent_id, user_id).await,
            Self::None(db) => db.new_session(agent_id, user_id).await,
            Self::File(db) => db.new_session(agent_id, user_id).await,
        }
    }

    // async fn handle_resume(
    //     &self,
    //     session_id: &str,
    //     user_id: &str,
    //     request: &ResumeRequest,
    // ) -> Result<bool, FormaError> {
    //     match self {
    //         Self::Mongo(db) => db.handle_resume(session_id, user_id, request).await,
    //         Self::None(db) => db.handle_resume(session_id, user_id, request).await,
    //         Self::File(db) => db.handle_resume(session_id, user_id, request).await,
    //     }
    // }

    async fn __append_msg(&self, msg: FormaMessage) -> Result<FormaMessage, FormaError> {
        match self {
            Self::Mongo(db) => db.__append_msg(msg).await,
            Self::None(db) => db.__append_msg(msg).await,
            Self::File(db) => db.__append_msg(msg).await,
        }
    }

    async fn __append_msgs(
        &self,
        msgs: Vec<FormaMessage>,
    ) -> Result<Vec<FormaMessage>, FormaError> {
        match self {
            Self::Mongo(db) => db.__append_msgs(msgs).await,
            Self::None(db) => db.__append_msgs(msgs).await,
            Self::File(db) => db.__append_msgs(msgs).await,
        }
    }

    async fn clear_state(&self, session_id: &str) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.clear_state(session_id).await,
            Self::None(db) => db.clear_state(session_id).await,
            Self::File(db) => db.clear_state(session_id).await,
        }
    }

    async fn save_pending_tool_calls(&self, session: &SessionData) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.save_pending_tool_calls(session).await,
            Self::None(db) => db.save_pending_tool_calls(session).await,
            Self::File(db) => db.save_pending_tool_calls(session).await,
        }
    }

    async fn start_run(
        &self,
        session_id: &str,
        stale_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, FormaError> {
        match self {
            Self::Mongo(db) => db.start_run(session_id, stale_before).await,
            Self::None(db) => db.start_run(session_id, stale_before).await,
            Self::File(db) => db.start_run(session_id, stale_before).await,
        }
    }

    async fn finish_run(&self, session_id: &str) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.finish_run(session_id).await,
            Self::None(db) => db.finish_run(session_id).await,
            Self::File(db) => db.finish_run(session_id).await,
        }
    }

    async fn get_session_by_id(
        &self,
        session_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        match self {
            Self::Mongo(db) => db.get_session_by_id(session_id, n_messages).await,
            Self::None(db) => db.get_session_by_id(session_id, n_messages).await,
            Self::File(db) => db.get_session_by_id(session_id, n_messages).await,
        }
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError> {
        match self {
            Self::Mongo(db) => db.get_message_by_id(message_id).await,
            Self::None(db) => db.get_message_by_id(message_id).await,
            Self::File(db) => db.get_message_by_id(message_id).await,
        }
    }

    async fn suspend_session(
        &self,
        session: Arc<RwLock<SessionData>>,
        call: &PendingToolCall,
    ) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.suspend_session(session, call).await,
            Self::None(db) => db.suspend_session(session, call).await,
            Self::File(db) => db.suspend_session(session, call).await,
        }
    }

    async fn get_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        match self {
            Self::Mongo(db) => {
                db.get_session_by_user_id(agent_id, user_id, n_messages)
                    .await
            }
            Self::None(db) => {
                db.get_session_by_user_id(agent_id, user_id, n_messages)
                    .await
            }
            Self::File(db) => {
                db.get_session_by_user_id(agent_id, user_id, n_messages)
                    .await
            }
        }
    }

    async fn delete_session(&self, session_id: &str, user_id: &str) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.delete_session(session_id, user_id).await,
            Self::None(db) => db.delete_session(session_id, user_id).await,
            Self::File(db) => db.delete_session(session_id, user_id).await,
        }
    }

    async fn rename_session(
        &self,
        session_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.rename_session(session_id, user_id, name).await,
            Self::None(db) => db.rename_session(session_id, user_id, name).await,
            Self::File(db) => db.rename_session(session_id, user_id, name).await,
        }
    }

    async fn get_user_sessions(
        &self,
        agent_id: &str,
        user_id: &str,
        n_sessions: usize,
    ) -> Result<Vec<SessionSummary>, FormaError> {
        match self {
            Self::Mongo(db) => db.get_user_sessions(agent_id, user_id, n_sessions).await,
            Self::None(db) => db.get_user_sessions(agent_id, user_id, n_sessions).await,
            Self::File(db) => db.get_user_sessions(agent_id, user_id, n_sessions).await,
        }
    }

    async fn update_msg_status(
        &self,
        session_id: &str,
        message_id: &str,
        status: ChatMessageState,
    ) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.update_msg_status(session_id, message_id, status).await,
            Self::None(db) => db.update_msg_status(session_id, message_id, status).await,
            Self::File(db) => db.update_msg_status(session_id, message_id, status).await,
        }
    }

    async fn add_msg_reaction(
        &self,
        session_id: &str,
        message_id: &str,
        emoji: &Option<String>,
    ) -> Result<(), FormaError> {
        match self {
            Self::Mongo(db) => db.add_msg_reaction(session_id, message_id, emoji).await,
            Self::None(db) => db.add_msg_reaction(session_id, message_id, emoji).await,
            Self::File(db) => db.add_msg_reaction(session_id, message_id, emoji).await,
        }
    }
}
