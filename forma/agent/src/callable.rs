use super::SSEvent;
use crate::{FormaError, Tool};
use async_trait::async_trait;

use genai_client::AssistantMessage;
use protocols::{DeploymentCredentials, EncryptedTokens, FormaDB, Session};
use serde_json::Value;
use std::{fmt::Debug, sync::Arc};
use tokio::sync::{Mutex, mpsc::UnboundedSender};
use tracing::{error, info};

/// Elements necessary to call nodes, agents, workflows
/// and LLMs
#[derive(Clone, Debug)]
pub struct CallContext<DB: FormaDB> {
    /// The session, its messages, tool results, etc.
    pub session: Session,
    /// a Sender to send SSEvents back to the caller
    pub tx: UnboundedSender<SSEvent>,
    /// The job ID for this call
    pub job_id: Arc<str>,
    /// The roles of the user making the call
    pub user_roles: Arc<Vec<String>>,
    /// The user ID
    pub user_id: Arc<str>,
    /// The user's time zone, as their client sent it (`x-time-zone`), if it
    /// did. Not validated here: the `current-date` tool ignores a bad one.
    pub user_timezone: Option<Arc<str>>,
    /// The time zone the agent's author set, used when the user's is unknown.
    pub agent_timezone: Option<Arc<str>>,
    /// The ID of the agent handling the call, as given by its definition.
    ///
    /// This is the *entry-point* agent for the whole call tree: workflow tools
    /// reuse their caller's context, so a nested workflow reports the agent the
    /// conversation started with rather than whichever node is running.
    pub agent_id: Arc<str>,
    /// The deployment this call belongs to; `None` in single-tenant mode.
    ///
    /// User IDs are only unique *within* a deployment (they come from a
    /// per-request header), so anything keyed on a user must be keyed on this
    /// too to stay tenant-isolated.
    pub deployment_id: Option<Arc<str>>,
    /// OAuth tokens for the user making the call
    pub user_credentials: Arc<Mutex<EncryptedTokens>>,
    /// The deployment credentials for the agent making the call
    pub deployment_credentials: Arc<DeploymentCredentials>,
    /// The DB Client
    pub db: Arc<DB>,
}

impl<DB: FormaDB> CallContext<DB> {
    /// Returns a subset of tools that can be called
    /// based on the user roles
    pub fn allowed_tools(user_roles: &[String], tools: &[Tool]) -> Vec<String> {
        tools
            .iter()
            .filter_map(|x| {
                if x.is_allowed(user_roles) {
                    Some(x.name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Gets the token stored in the OAuth tokens map for the given URL
    ///
    /// If the token is expired, it will be refreshed before returning.
    /// It'll update the token in the map with the refreshed one.
    pub async fn get_oauth_token(&self, url: &str) -> Result<String, FormaError> {
        let mut tokens = self.user_credentials.lock().await;
        let Some(token) = tokens.get(url) else {
            info!(url, "no OAuth token found for URL");
            return Err(FormaError::OAuthError {
                url: url.to_string(),
                msg: "no OAuth token found".to_string(),
            });
        };
        let mut token = token.clone();
        if token.is_expired() {
            info!(url, "OAuth token expired, refreshing");
            token = token.refresh().await?;
            tokens.insert(url.to_string(), token.clone());
            let user_id = {
                let session = self.session.read().await;
                session.user_id.clone()
            };
            self.db
                .save_oauth_tokens(&user_id, tokens.clone())
                .await
                .map_err(|e| {
                    error!(url, error = %e, "failed to persist refreshed OAuth token to DB");
                    e
                })?;
        }
        Ok(token.access_token)
    }

    /// Caches an LLM result into the state, adding at as `Value`
    /// indexex by its `id`
    pub async fn cache_result(
        &self,
        key: &str,
        result: &AssistantMessage,
    ) -> Result<(), FormaError> {
        let mut session = self.session.write().await;
        session
            .cached_messages
            .insert(key.to_string(), result.clone());
        Ok(())
    }

    /// Checks whether the cache in `ctx` contains
    /// anything under the key `self.id`
    pub async fn is_cached(&self, key: &str) -> bool {
        let session = self.session.read().await;
        session.cached_messages.contains_key(key)
    }

    /// Retrieves the result from the cache.
    pub async fn cached_result(&self, key: &str) -> Option<AssistantMessage> {
        let session = self.session.read().await;
        session.cached_messages.get(key).cloned()
    }

    /// Caches an LLM result into the state, adding at as `Value`
    /// indexex by its `id`
    pub async fn register_variable<T: Into<Value>>(
        &self,
        key: &str,
        result: T,
    ) -> Result<(), FormaError> {
        let mut session = self.session.write().await;
        session.variables.insert(key.to_string(), result.into());
        Ok(())
    }

    pub async fn n_pending_tools(&self) -> usize {
        let session = self.session.read().await;
        session.pending_tool_calls.len()
    }
}

pub trait Identifiable {
    /// The name of this kind of node
    const CLASS: &str;

    /// Retrieves the ID of the object
    fn get_id(&self) -> &str;
}

/// Meant to be used by the internal elements of an agent (e.g., Node, Workflow, Agent)
///
/// Part of the reason this trait exists is to allow running bits
/// of agents for debugging/testing purposes.
#[async_trait]
pub trait Callable<DB: FormaDB>: Identifiable + Clone + Sync + Send + 'static + Debug {
    /// Checks that the callable element is well formed.
    /// What we check will depend on the specific callable type.
    ///
    /// Should return an `FormaError::Input` if the element is not well formed.
    fn check(&self) -> Result<(), FormaError>;

    /// Executes this callable, and returns a response.
    /// If the execution is suspended (e.g., waiting for user input),
    /// returns `Ok(None)` and the caller should wait for a future call.
    async fn run(
        &self,
        ctx: CallContext<DB>,
        is_root: bool,
    ) -> Result<Option<serde_json::Value>, FormaError>;

    /// Retrieves all the IDs of the inner objects,
    /// plus its own. This excludes Tools
    /// (as they can be reused)
    fn inner_ids(&self) -> Vec<String>;

    /// The time zone its author set, if any: only an agent has one.
    fn timezone(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod testing {
    use db::NoneDB;

    use crate::tool::{TemplateTool, ToolRunner};

    use super::*;

    #[test]
    fn test_allowed_tools() {
        let tool1 = Tool {
            name: "tool_a".to_string(),
            description: None,
            tool: crate::ToolRunner::Template(TemplateTool {
                templates: vec!["template_a".to_string()],
            }),
            just_for: vec!["admin".to_string()],
            confirm_before_calling: false,
            bundle: None,
        };
        let tool2 = Tool {
            name: "tool_b".to_string(),
            description: None,
            tool: crate::ToolRunner::Template(TemplateTool {
                templates: vec!["template_b".to_string()],
            }),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        };
        let tool3 = Tool {
            name: "tool_c".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["template_c".to_string()],
            }),
            just_for: vec!["user".to_string()],
            confirm_before_calling: false,
            bundle: None,
        };
        let tools = vec![tool1, tool2, tool3];

        // Test with admin role
        let user_roles = vec!["admin".to_string()];
        let allowed = CallContext::<NoneDB>::allowed_tools(&user_roles, &tools);
        assert!(allowed.contains(&"tool_a".to_string()));
        assert!(allowed.contains(&"tool_b".to_string()));
        assert!(!allowed.contains(&"tool_c".to_string()));

        // Test with user role
        let user_roles = vec!["user".to_string()];
        let allowed = CallContext::<NoneDB>::allowed_tools(&user_roles, &tools);
        assert!(!allowed.contains(&"tool_a".to_string()));
        assert!(allowed.contains(&"tool_b".to_string()));
        assert!(allowed.contains(&"tool_c".to_string()));

        // Test with no roles
        let user_roles = vec![];
        let allowed = CallContext::<NoneDB>::allowed_tools(&user_roles, &tools);
        assert!(!allowed.contains(&"tool_a".to_string()));
        assert!(allowed.contains(&"tool_b".to_string()));
        assert!(!allowed.contains(&"tool_c".to_string()));
    }
}
