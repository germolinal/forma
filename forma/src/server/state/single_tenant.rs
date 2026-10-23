use crate::server::state::{RuntimeState, SharedAgent};
use axum::http::HeaderMap;
use protocols::{DeploymentCredentials, FormaDB, FormaError};
use std::sync::Arc;

/// Application state for a single-tenant app.
/// This means, a service that serves a single
/// agent exclusively
#[derive(Clone, Debug)]
pub struct AppState<DB: FormaDB> {
    /// Credentials
    pub(crate) credentials: Arc<DeploymentCredentials>,
    /// The agent to serve
    pub(crate) agent: SharedAgent,
    /// The DB Client
    pub(crate) db: Arc<DB>,
}

#[async_trait::async_trait]
impl<DB: FormaDB> RuntimeState<DB> for AppState<DB> {
    fn db(&self) -> &Arc<DB> {
        &self.db
    }

    async fn __agent_and_credentials(
        &self,
        _headers: &HeaderMap,
    ) -> Result<(SharedAgent, Arc<DeploymentCredentials>), FormaError> {
        Ok((self.agent.clone(), self.credentials.clone()))
    }

    fn deployment_id(&self, _headers: &HeaderMap) -> Result<Option<Arc<str>>, FormaError> {
        // Does not need it, it is not a shared service
        Ok(None)
    }

    async fn deployment_credentials(
        &self,
        _headers: &HeaderMap,
    ) -> Result<Arc<DeploymentCredentials>, FormaError> {
        Ok(self.credentials.clone())
    }
}

impl<DB: FormaDB> AppState<DB> {
    pub(crate) async fn new(
        agent: SharedAgent,
        db: Arc<DB>,
        credentials: DeploymentCredentials,
    ) -> Result<Self, FormaError> {
        Ok(Self {
            agent,
            db,
            credentials: Arc::new(credentials),
        })
    }
}
