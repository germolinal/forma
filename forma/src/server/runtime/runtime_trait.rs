use crate::server::state::RuntimeState;
use axum::Router;

use protocols::{DeploymentCredentials, FormaDB, FormaError};

/// A trait that defines the runtime for the server.
#[async_trait::async_trait]
pub trait RuntimeTrait<DB: FormaDB, T: RuntimeState<DB>> {
    /// A method for loading environment
    /// variables, compiling, or doing whatever
    /// else this runtime needs
    fn init(&mut self) -> Result<(), FormaError>;

    /// Builds the App which will be served
    /// through Axum
    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError>;

    /// Builds credentials from environment variables. Used
    /// inly in single-tenant mode.
    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError>;
}
