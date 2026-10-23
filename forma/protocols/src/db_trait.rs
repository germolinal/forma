use std::{fmt::Debug, sync::Arc};

use crate::{
    ChatMessageState, EncryptedTokens, FormaError, FormaMessage, PendingToolCall, SessionData,
    SessionSummary, credentials::EncryptedTokenSet,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// The ID of the default Forma database.
pub const FORMA_DATABASE_ID: &str = "forma-db";

/// Represents the credentials for a deployment, stored
/// encrypted in the database.
pub type DeploymentCredentials = EncryptedTokenSet<Value>;

/// Represents an agent that is owned by a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDeployment {
    /// The logical ID of the agent (i.e., `agent.id`).
    pub agent_id: String,
    /// The ID of the user who owns this agent.
    pub owner_id: String,
    /// The timestamp when this agent was created.
    pub created_at: chrono::DateTime<Utc>,
    /// The timestamp when this agent was last updated.
    pub updated_at: chrono::DateTime<Utc>,
    /// The version of the agent.
    pub version: usize,
    /// The agent's state.
    pub agent: Value,
    /// Credentials
    pub credentials: Option<DeploymentCredentials>,
}

impl AgentDeployment {
    /// Replaces the deployment's credentials under every key `update` names,
    /// keeping the ones it does not mention — so a redeploy can swap the
    /// workspace grants without dropping a channel's secrets.
    pub fn replace_credentials(&mut self, update: DeploymentCredentials) {
        let credentials = self
            .credentials
            .get_or_insert_with(DeploymentCredentials::default);
        for (key, value) in update.0 {
            credentials.insert(key, value);
        }
    }
}

/// A trait that defines the interface for a Forma database.
#[async_trait::async_trait]
pub trait FormaDB: 'static + Clone + Debug + Sized + Send + Sync {
    /// Returns whether this DB Client is meant to store things
    /// in a DB, file or somewhere else
    fn persist(&self) -> bool;

    /// Generates a default client.
    /// (cannot be `Default::default()` because it is async)
    async fn new() -> Result<Self, FormaError>;

    /// Loads env variables and more. Fails if something is off
    fn init(&mut self) -> Result<(), FormaError>;

    /// Retrieves OAuth tokens for the given user
    async fn get_oauth_tokens(&self, user_id: &str) -> Result<EncryptedTokens, FormaError>;

    /// Saves OAuth tokens for the given user
    async fn save_oauth_tokens(
        &self,
        user_id: &str,
        tokens: EncryptedTokens,
    ) -> Result<(), FormaError>;

    /// Removes OAuth tokens for the given user
    async fn remove_oauth_tokens(&self, user_id: &str) -> Result<(), FormaError>;

    /// Creates a new session for the given agent and user.
    async fn new_session(&self, agent_id: &str, user_id: &str) -> Result<SessionData, FormaError>;

    /// Inserts a message into the session,
    /// returns the inserted message, with its
    /// ID
    async fn __append_msg(&self, mut msg: FormaMessage) -> Result<FormaMessage, FormaError>;

    /// Pushes a message to the session. Sets the `id` (as per `__append_msg`), `user_id`, `session_id`, and `created_at` fields.
    async fn push_msg<T: Send + Into<FormaMessage>>(
        &self,
        session_id: &str,
        user_id: &str,
        msg: T,
    ) -> Result<FormaMessage, FormaError> {
        let mut msg: FormaMessage = msg.into();
        msg.session_id = session_id.to_string();
        msg.user_id = user_id.to_string();
        msg.created_at = Some(Utc::now());
        if msg.id.is_none() {
            // Add a UUID to the message ID if it's not already set
            msg.id = Some(uuid::Uuid::new_v4().to_string().into());
        }

        let msg = self.__append_msg(msg).await?;
        Ok(msg)
    }

    /// Inserts several messages into their session, in order, returning them
    /// as stored. One at a time unless the backend can do better: a database
    /// should store them in a single round trip.
    async fn __append_msgs(
        &self,
        msgs: Vec<FormaMessage>,
    ) -> Result<Vec<FormaMessage>, FormaError> {
        let mut saved = Vec::with_capacity(msgs.len());
        for msg in msgs {
            saved.push(self.__append_msg(msg).await?);
        }
        Ok(saved)
    }

    /// Pushes several messages to the session at once, in order: what
    /// [`Self::push_msg`] does for each of them, in one round trip where the
    /// backend allows it.
    ///
    /// Their `created_at` are a millisecond apart, because that is the order a
    /// session is read back in, to the millisecond: stored together, they
    /// would otherwise share a timestamp and come back in any order.
    async fn push_msgs(
        &self,
        session_id: &str,
        user_id: &str,
        msgs: Vec<FormaMessage>,
    ) -> Result<Vec<FormaMessage>, FormaError> {
        let now = Utc::now();
        let msgs = msgs
            .into_iter()
            .enumerate()
            .map(|(i, mut msg)| {
                msg.session_id = session_id.to_string();
                msg.user_id = user_id.to_string();
                msg.created_at = Some(now + chrono::TimeDelta::milliseconds(i as i64));
                if msg.id.is_none() {
                    msg.id = Some(uuid::Uuid::new_v4().to_string().into());
                }
                msg
            })
            .collect();
        self.__append_msgs(msgs).await
    }

    /// Clears all runtime state from the session in the database after a job
    /// completes without suspension (`state`, `tool_calls`, `pending_tool_calls`).
    /// Messages are stored separately and are not touched.
    async fn clear_state(&self, session_id: &str) -> Result<(), FormaError>;

    /// Claims the session for a run, unless another run already holds it.
    /// Returns `false` when one does and started after `stale_before`; a claim
    /// older than that belongs to a run that can no longer be alive (the
    /// request carrying it has timed out) and is taken over.
    ///
    /// Must be atomic: two messages arriving together may both try to claim
    /// the session, and only one of them may get it.
    async fn start_run(
        &self,
        session_id: &str,
        stale_before: chrono::DateTime<Utc>,
    ) -> Result<bool, FormaError>;

    /// Releases the claim [`FormaDB::start_run`] made, however the run ended.
    async fn finish_run(&self, session_id: &str) -> Result<(), FormaError>;

    /// Returns the session data for the given session ID, with the most recent `n_messages`.
    async fn get_session_by_id(
        &self,
        session_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError>;

    /// Returns the message with the given id
    async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError>;

    /// Whether the session already holds a message with this id. Channels
    /// that redeliver (WhatsApp does, whenever it is not sure a delivery
    /// arrived) store each message under the id they gave it, and ask this
    /// before answering it.
    async fn has_message(&self, session_id: &str, message_id: &str) -> Result<bool, FormaError> {
        match self.get_message_by_id(message_id).await {
            Ok(message) => Ok(message.session_id == session_id),
            Err(FormaError::MessageNotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Suspends the session by adding the pending tool call to the session's `pending_tool_calls` list.
    async fn suspend_session(
        &self,
        session: Arc<RwLock<SessionData>>,
        call: &PendingToolCall,
    ) -> Result<(), FormaError>;

    /// Stores the session's `pending_tool_calls` and `tool_calls_results`, as
    /// they are after the client answered one of the pending calls. Without
    /// it, an answer that leaves other calls pending would be lost, because
    /// every request reads the session from the database again.
    async fn save_pending_tool_calls(&self, session: &SessionData) -> Result<(), FormaError>;

    /// Returns the session data for the given agent ID and user ID, with the most recent `n_messages`.
    async fn get_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError>;

    /// Returns the last `n_sessions` sessions for the given user on the given
    /// agent/deployment, most recent first, each with a display name derived
    /// from its first user message.
    async fn get_user_sessions(
        &self,
        agent_id: &str,
        user_id: &str,
        n_sessions: usize,
    ) -> Result<Vec<SessionSummary>, FormaError>;

    /// Deletes one of `user_id`'s own sessions, together with the messages
    /// in it.
    ///
    /// A session that belongs to someone else is reported as
    /// [`FormaError::SessionNotFound`], the same way
    /// [`Self::update_agent_deployment`] treats a deployment it does not own:
    /// existence is not leaked to whoever asked.
    async fn delete_session(&self, session_id: &str, user_id: &str) -> Result<(), FormaError>;

    /// Renames one of `user_id`'s own sessions. `name` is what the session is
    /// called from now on, and `None` puts it back to the name derived from
    /// its first message. Ownership is treated exactly as in
    /// [`Self::delete_session`].
    ///
    /// The name is stored as it is given, so whoever takes it from a user
    /// runs it through [`crate::normalize_session_name`] first.
    async fn rename_session(
        &self,
        session_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> Result<(), FormaError>;

    /// Updates the status of a message in the session.
    async fn update_msg_status(
        &self,
        session_id: &str,
        message_id: &str,
        status: ChatMessageState,
    ) -> Result<(), FormaError>;

    /// Adds a reactionto a message
    async fn add_msg_reaction(
        &self,
        session_id: &str,
        message_id: &str,
        emoji: &Option<String>,
    ) -> Result<(), FormaError>;

    /// Returns the session data for the given session ID, with the most recent `n_messages`.
    /// If the session is not found, a new session is created.
    async fn get_or_create_session(
        &self,
        agent_id: &str,
        user_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        match self
            .get_session_by_user_id(agent_id, user_id, n_messages)
            .await
        {
            Ok(session) => Ok(session), // found!
            Err(FormaError::SessionNotFound(_)) => self.new_session(agent_id, user_id).await,
            Err(e) => Err(e), // different kind of error
        }
    }

    /// Retrieves an Agent (not a definition) stored under the given ID.
    /// This is NOT the ID given by the user to the agent definition,
    /// but the id that the Database assigns to the agent.
    async fn get_agent_deployment(&self, agent_id: &str) -> Result<AgentDeployment, FormaError>;

    /// Saves an Agent (not a definition), returns the ID of the saved agent in the DB.
    ///
    /// This creates a whole new deployment, with version 1. If the plan is to
    /// update an existing deployment, use [`Self::update_agent_deployment`] instead.
    async fn new_agent_deployment(
        &self,
        user_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError>;

    /// Updates an existing Agent deployment. It replaces the
    /// existing agent data with the new one, and increments the version number.
    ///
    /// Credentials stay the same, except under the keys `credentials` names,
    /// which replace the deployment's own (see
    /// [`AgentDeployment::replace_credentials`]).
    ///
    /// If the deployment does not exist, it returns an error. Likewise, if the
    /// deployment belongs to a different user, it returns an error.
    async fn update_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError>;

    /// Removes an deployment from the DB, based on its id.
    /// Returns the ID of the removed agent.
    async fn remove_agent_deployment(&self, deployment_id: &str) -> Result<String, FormaError>;
}

#[cfg(test)]
mod replace_credentials_tests {
    use super::*;
    use serde_json::json;

    fn deployment(credentials: Option<DeploymentCredentials>) -> AgentDeployment {
        AgentDeployment {
            agent_id: "a".to_string(),
            owner_id: "o".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 1,
            agent: Value::Null,
            credentials,
        }
    }

    #[test]
    fn named_keys_are_replaced_and_the_rest_kept() {
        let mut existing = DeploymentCredentials::default();
        existing.insert("whatsapp_api".to_string(), json!({ "token": "secret" }));
        existing.insert(
            "workspaces".to_string(),
            json!([{ "path": "b/old", "scopes": ["read"] }]),
        );
        let mut d = deployment(Some(existing));

        // A new version that uses no workspace at all loses its grant, and
        // keeps the channel's secret, which nobody resent.
        let mut update = DeploymentCredentials::default();
        update.insert("workspaces".to_string(), json!([]));
        d.replace_credentials(update);

        let credentials = d.credentials.unwrap();
        assert_eq!(credentials.get("workspaces"), Some(&json!([])));
        assert_eq!(
            credentials.get("whatsapp_api"),
            Some(&json!({ "token": "secret" }))
        );
    }

    #[test]
    fn a_deployment_without_credentials_gets_them() {
        let mut d = deployment(None);
        let mut update = DeploymentCredentials::default();
        update.insert(
            "workspaces".to_string(),
            json!([{ "path": "b/kb", "scopes": ["read"] }]),
        );
        d.replace_credentials(update);
        assert!(d.credentials.unwrap().get("workspaces").is_some());
    }
}
