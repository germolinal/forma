#![warn(clippy::dbg_macro)]
#![deny(missing_docs)]

//! The wrapper around Forma's internal systems, designed to
//! read inputs, manage observability, and start servers.

/// The interface between the inputs that users write and the
/// actual structures utilized internally by Forma.
///
/// Many of the structures and enums in this module have a Suffix
/// "Definition", which means that they have an analogue internally
/// in forma (e.g., `AgentDefinition` is the API version of `Agent`)
#[cfg(feature = "api")]
pub mod api;

#[cfg(debug_assertions)]
mod print_docs;

/// Defines opentelemetry subscribers
pub mod observability;

/// Tools for spining up a Forma Agent's server
pub mod server;
pub use server::RuntimeTrait;

pub use agent::Agent;

/// The stack each of the runtime's threads gets. An agent's run is a deep
/// chain of futures — agent, workflow, node, tool, and again for a workflow
/// used as a tool — and polling it can take more than tokio's 2 MiB default,
/// debug builds above all.
const THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

/// The tokio runtime the `forma` binary runs on, agents included.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(THREAD_STACK_SIZE)
        .build()
        .expect("failed to build the tokio runtime")
}

/// Picks rustls' crypto backend for the whole process. Must run before any
/// TLS connection is made.
///
/// Both backends end up compiled in (`mongodb` enables `ring`; `reqwest` and
/// the Google clients enable `aws-lc-rs`), and with two available rustls
/// refuses to guess: the first connection that needs it panics.
pub fn install_crypto_provider() {
    // Only fails if a provider is already installed, which is fine.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// CLI input, only available without the feature `static`.
#[cfg(feature = "cli")]
pub mod cli;

/// Arguments for running in production.
#[cfg(not(feature = "cli"))]
pub mod prod;
