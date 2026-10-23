use axum::http::HeaderMap;
use protocols::{DeploymentCredentials, FormaDB, FormaError};
use std::sync::Arc;
#[cfg(feature = "hot-reload")]
use tokio::sync::RwLock;

use crate::server::{
    expect_deployment_id, get_deployed_agent, get_user_id,
    state::{RuntimeState, SharedAgent},
    user_roles,
};

/// State for multi-tenant Forma service
#[derive(Clone, Debug)]
pub struct MultitenantAppState<DB: FormaDB> {
    /// The DB Client
    pub(crate) db: Arc<DB>,
}
impl<DB: FormaDB> MultitenantAppState<DB> {
    /// Creates a new state
    pub fn new(db: Arc<DB>) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl<DB: FormaDB> RuntimeState<DB> for MultitenantAppState<DB> {
    fn db(&self) -> &Arc<DB> {
        &self.db
    }

    async fn __agent_and_credentials(
        &self,
        headers: &HeaderMap,
    ) -> Result<(SharedAgent, Arc<DeploymentCredentials>), FormaError> {
        let user_id = get_user_id(self.db.persist(), headers)?;
        let agent_db_id = expect_deployment_id(headers)?;
        let roles = user_roles(headers)?;
        let (mut agent, credentials) =
            get_deployed_agent(self.db.clone(), &agent_db_id, &user_id, &roles).await?;
        agent.id = agent_db_id;

        #[cfg(feature = "hot-reload")]
        let agent = RwLock::new(agent);

        let agent = Arc::new(agent);
        let credentials = Arc::new(credentials);
        Ok((agent, credentials))
    }

    fn deployment_id(&self, headers: &HeaderMap) -> Result<Option<Arc<str>>, FormaError> {
        let deployment_id = expect_deployment_id(headers)?;
        Ok(Some(deployment_id.into()))
    }

    async fn deployment_credentials(
        &self,
        headers: &HeaderMap,
    ) -> Result<Arc<DeploymentCredentials>, FormaError> {
        let deployment_id = expect_deployment_id(headers)?;
        let deployment = self.db.get_agent_deployment(&deployment_id).await?;
        Ok(Arc::new(deployment.credentials.unwrap_or_default()))
    }
}
