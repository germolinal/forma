use std::{collections::HashMap, fmt::Debug, sync::Arc};

use axum::http::HeaderMap;

use agent::{Agent, ToolRunner};
use protocols::{
    DeploymentCredentials, EncryptedTokens, FormaDB, FormaError, Session, SessionData,
};

use tokio::sync::{Mutex, RwLock};

use crate::server::{
    JobArgs, expect_user_id, get_session_id, state::SharedAgent, user_roles, user_timezone,
};

/// Whether running `agent` may need the user's OAuth tokens, which only MCP
/// tools use (see `CallContext::get_oauth_token`).
fn uses_oauth(agent: &Agent) -> bool {
    agent
        .tools()
        .any(|tool| matches!(tool.tool, ToolRunner::Mcp(_)))
}

async fn get_session_by_id<DB: FormaDB>(
    db: Arc<DB>,
    session_id: &str,
    user_id: &str,
    n_messages: usize,
) -> Result<Session, FormaError> {
    let s = if db.persist() {
        let s = db.get_session_by_id(session_id, n_messages).await?;
        // Validate session ownership
        if s.user_id != user_id {
            return Err(FormaError::SessionNotOwnedByUser(session_id.to_string()));
        }
        s
    } else {
        SessionData {
            user_id: user_id.to_string(),
            agent_id: "ephemeral-session-agent".to_string(),
            id: Some(session_id.to_string().into()),
            messages: Vec::with_capacity(2),
            ..Default::default()
        }
    };
    Ok(Arc::new(RwLock::new(s)))
}

/// A trait defining the shared
/// behaviour of an Axum App State
/// for Forma
#[async_trait::async_trait]
pub trait RuntimeState<DB: FormaDB>: 'static + Debug + Clone + Sync + Send {
    /// borrows a pointer to the DB Backend of this state
    fn db(&self) -> &Arc<DB>;

    /// Gets the deployment ID if in multitenant... None if in single-tenant
    fn deployment_id(&self, headers: &HeaderMap) -> Result<Option<Arc<str>>, FormaError>;

    /// The credentials of the deployment a request is for, without loading
    /// its agent: what a webhook needs before it can trust its payload (to
    /// check a signature, or answer a verification challenge). A single-tenant
    /// server has its own, so it never looks for a deployment.
    async fn deployment_credentials(
        &self,
        headers: &HeaderMap,
    ) -> Result<Arc<DeploymentCredentials>, FormaError>;

    /// Retrieves the agent and credentials associated with the call.
    /// If in multi-tenant mode, a call to the database should be made.
    async fn __agent_and_credentials(
        &self,
        headers: &HeaderMap,
    ) -> Result<(SharedAgent, Arc<DeploymentCredentials>), FormaError>;

    /// Gets the job arguments that can be used to call the
    /// agent
    async fn job_args(
        &self,
        headers: &HeaderMap,
        n_messages: usize,
    ) -> Result<JobArgs<DB>, FormaError> {
        let db = self.db();
        let session_id = get_session_id(db.persist(), headers)?;
        tracing::info!("session_id: {session_id}");
        let user_id = expect_user_id(headers)?;
        tracing::info!("user_id: {user_id}");
        let roles = user_roles(headers)?;
        tracing::info!("roles: {roles:?}");
        // Neither needs the other, and for a multi-tenant server both are a
        // trip to the database.
        let (session, (agent, credentials)) = tokio::try_join!(
            get_session_by_id(db.clone(), &session_id, &user_id, n_messages),
            self.__agent_and_credentials(headers),
        )?;
        tracing::info!("session_id: {session_id}");

        self.job_args_with(session, agent, credentials, n_messages, roles, headers)
            .await
    }

    /// Gets the job arguments that can be used to call the
    /// agent
    async fn job_args_for_session(
        &self,
        session: Session,
        max_history_messages: usize,
        roles: Vec<String>,
        headers: &HeaderMap,
    ) -> Result<JobArgs<DB>, FormaError> {
        let (agent, credentials) = self.__agent_and_credentials(headers).await?;
        self.job_args_with(
            session,
            agent,
            credentials,
            max_history_messages,
            roles,
            headers,
        )
        .await
    }

    /// Gets the job arguments that can be used to call `agent`, once the
    /// session and the agent have been read.
    async fn job_args_with(
        &self,
        session: Session,
        agent: SharedAgent,
        credentials: Arc<DeploymentCredentials>,
        max_history_messages: usize,
        roles: Vec<String>,
        headers: &HeaderMap,
    ) -> Result<JobArgs<DB>, FormaError> {
        let (session_id, user_id) = {
            let s = session.read().await;
            (s.id_string()?, s.user_id.to_string())
        };
        let db = self.db();

        #[cfg(feature = "hot-reload")]
        let (agent_id, agent_variables, agent_timezone, needs_tokens) = {
            let agent = agent.read().await;
            (
                agent.id.clone(),
                agent.variables.clone(),
                agent.timezone.clone(),
                uses_oauth(&agent),
            )
        };
        #[cfg(not(feature = "hot-reload"))]
        let (agent_id, agent_variables, agent_timezone, needs_tokens) = {
            (
                agent.id.to_string(),
                agent.variables.clone(),
                agent.timezone.clone(),
                uses_oauth(&agent),
            )
        };

        // Only an MCP tool uses the user's tokens: for an agent without one,
        // reading them is a database round trip for nothing.
        let oauth_tokens = if needs_tokens {
            db.get_oauth_tokens(&user_id).await?
        } else {
            EncryptedTokens::default()
        };
        let oauth_tokens = Arc::new(Mutex::new(oauth_tokens));

        // merge variables into the agent's variables
        {
            let mut w_session = session.write().await;
            w_session.variables = agent_variables
                .into_iter()
                .chain(w_session.variables.clone())
                .collect::<HashMap<String, serde_json::Value>>();
        }

        Ok(JobArgs {
            deployment_id: self.deployment_id(headers)?,
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            user_id: user_id.into(),
            roles: roles.into(),
            user_timezone: user_timezone(headers).map(Into::into),
            agent_timezone: agent_timezone.map(Into::into),
            user_credentials: oauth_tokens,
            deployment_credentials: credentials,
            agent,
            session,
            db: db.clone(),
            _max_history_messages: max_history_messages,
        })
    }
}
