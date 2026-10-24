#![warn(clippy::dbg_macro)]

/// Operations over datasets
pub mod dataset;

/// Start the development server, with a chat UI
pub mod dev;

/// Authenticate MCP servers
pub mod login;

/// Operations over evaluations
pub mod eval;

/// Expand the Agent
pub mod expand;

/// Initialize a new project
pub mod init;

/// Describe Forma's HTTP APIs (OpenAPI)
pub mod openapi;

/// Helpers for paths
mod path_file;

/// The platform that keeps datasets and evaluation results
pub mod platform;

/// Start the Development server
pub mod serve;

/// Ask "Personas" to ask questions, which
/// will be put in a dataset
pub mod tester;
