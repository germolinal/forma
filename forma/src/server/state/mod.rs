/// A trait that defines the behavior of an Axum state
/// for a Forma app
pub mod state_trait;
use agent::Agent;
pub use state_trait::RuntimeState;
use std::sync::Arc;

#[cfg(feature = "hot-reload")]
use tokio::sync::RwLock;

/// A state for multi-tenant Forma services
#[cfg(feature = "multi-tenant")]
pub mod multi_tenant;
#[cfg(feature = "multi-tenant")]
pub use multi_tenant::MultitenantAppState;
/// A state for single-tenant Forma services
#[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
pub mod single_tenant;
#[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
pub use single_tenant::AppState;

#[cfg(not(feature = "hot-reload"))]
pub(crate) type SharedAgent = Arc<Agent>;
#[cfg(feature = "hot-reload")]
pub(crate) type SharedAgent = Arc<RwLock<Agent>>;
