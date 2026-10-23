#![warn(clippy::dbg_macro)]
#![deny(missing_docs)]

pub(crate) mod agent_definition;
pub(crate) mod agent_features;
pub(crate) mod api_call_tool_definition;
pub(crate) mod code_execution_tool_definition;
pub(crate) mod current_date_tool_definition;
pub(crate) mod delegated_tool_definition;
pub(crate) mod eval_definition;
pub(crate) mod fetch_tool_definition;
pub mod generative;
pub(crate) mod google_agent_search_tool_definition;
pub(crate) mod google_gen_image_tool_definition;
pub(crate) mod mailgun_tool_definition;
pub(crate) mod mcp_tool_definition;
pub(crate) mod node_definition;
pub(crate) mod tavily_tool_definition;
pub(crate) mod template_field;
pub(crate) mod template_tool_definition;
pub(crate) mod tool_definition;
pub(crate) mod workflow_definition;
pub(crate) mod workspace_tool_definition;

pub use agent_definition::{AgentDefinition, InputVariable};
pub use agent_features::AgentFeatures;
use protocols::FormaError;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub use current_date_tool_definition::CurrentDateToolDefinition;
pub use fetch_tool_definition::FetchToolDefinition;
pub use google_agent_search_tool_definition::GoogleAgentSearchDefinition;
pub use google_gen_image_tool_definition::GoogleGenImageDefinition;
pub use workspace_tool_definition::{GoogleWorkspaceDefinition, ScrapbookDefinition};
// pub use knowledgebase_tool_definition::{ MongoDocstoreDefinition};
pub use mailgun_tool_definition::MailgunToolDefinition;
pub use mcp_tool_definition::McpToolDefinition;
pub use tavily_tool_definition::TavilyWebSearchDefinition;

/// The two things every definition can do before it is compiled: resolve the
/// `$file` references written inside it, and say whether it is well formed.
///
/// Public because the definitions themselves are: a caller holding an
/// [`AgentDefinition`] — the manager service, the CLI — needs both.
pub trait ObjectDefinition:
    'static + Sized + Clone + Serialize + for<'a> Deserialize<'a> + Debug
{
    /// Expands the object, resolving any nested references.
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError>;

    /// Checks that the definition is well formed, handing it back untouched so
    /// a read can be chained: `read_definition(f)?.check()?`.
    ///
    /// This is the definition-side counterpart of `Callable::check`, for when an
    /// agent is inspected *without* being compiled — `forma expand`. It only
    /// sees what the files say, which is less than a compiled agent shows: an
    /// MCP server contributes tools whose names only arrive with the
    /// connection, and a `$file` reference that has not been expanded yet
    /// contributes nothing at all. Both are skipped rather than guessed at.
    ///
    /// What it does check, it checks by the same rules as the runtime, so an
    /// agent cannot pass one and fail the other.
    ///
    /// Defaults to accepting the definition, for the many that hold nothing
    /// worth checking before they are compiled.
    fn check(self) -> Result<Self, FormaError> {
        Ok(self)
    }
}

/// Sets `GOOGLE_CLOUD_PROJECT` for as long as it is alive, then puts back
/// whatever was there before.
///
/// Definitions are meant to be readable without a Google project in the
/// environment at all, and the tests that check that have to say so by removing
/// it. The environment is process-wide while the test harness runs a thread per
/// test, so those tests exclude each other through the lock in here. Poisoning
/// is ignored: a panicking test says nothing about whether the variable is safe
/// to touch.
#[cfg(test)]
pub(crate) struct ProjectEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<String>,
}

#[cfg(test)]
impl ProjectEnvGuard {
    /// Sets the variable to `value`, or unsets it when given `None`.
    pub(crate) fn set(value: Option<&str>) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var("GOOGLE_CLOUD_PROJECT").ok();
        Self::write(value);
        Self {
            _lock: lock,
            previous,
        }
    }

    fn write(value: Option<&str>) {
        // SAFETY: every writer of this variable in this crate's tests holds
        // the lock above, so no other thread is reading it here.
        match value {
            Some(v) => unsafe { std::env::set_var("GOOGLE_CLOUD_PROJECT", v) },
            None => unsafe { std::env::remove_var("GOOGLE_CLOUD_PROJECT") },
        }
    }
}

#[cfg(test)]
impl Drop for ProjectEnvGuard {
    fn drop(&mut self) {
        Self::write(self.previous.as_deref());
    }
}
