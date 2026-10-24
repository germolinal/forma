#![deny(missing_docs)]

mod msg_extractor;

/// Contains definitions for the settings that
/// wrap a Forma agent
pub mod runtime;
pub use runtime::{Client, FormaRuntime, RuntimeTrait};

/// A trait defining the shared
/// behaviour of an Axum App State
/// for Forma
pub mod state;

/// The OpenAPI descriptions of the servers' HTTP APIs.
#[cfg(feature = "openapi")]
pub mod openapi;
#[cfg(any(feature = "multi-tenant", feature = "cli"))]
use agent::Agent;
#[cfg(feature = "cli")]
use agent::Callable;
use axum::http::HeaderMap;

use protocols::FormaDB;

use protocols::{
    DEPLOYMENT_ID_HEADER, DeploymentCredentials, EncryptedTokens, FormaError, FormaMessage,
    SESSION_ID_HEADER, Session, TIME_ZONE_HEADER, USER_ID_HEADER, USER_ROLES_HEADER,
};
#[cfg(feature = "multi-tenant")]
use std::collections::HashMap;
use tokio::sync::Mutex;
#[cfg(feature = "multi-tenant")]
use tracing::error;
use tracing::{info, instrument};

use std::sync::Arc;

use crate::server::state::SharedAgent;

/// The arguments needed to execute and log
/// a job
#[derive(Debug, Clone)]
pub struct JobArgs<DB: FormaDB> {
    /// not used by all runtimes.
    pub(crate) _max_history_messages: usize,
    pub(crate) session_id: Arc<str>,
    pub(crate) user_id: Arc<str>,
    pub(crate) agent_id: Arc<str>,
    pub(crate) deployment_id: Option<Arc<str>>,
    pub(crate) roles: Arc<Vec<String>>,
    /// The user's time zone, from `x-time-zone`; see `CallContext`.
    pub(crate) user_timezone: Option<Arc<str>>,
    /// The agent's own time zone, from its definition.
    pub(crate) agent_timezone: Option<Arc<str>>,
    pub(crate) user_credentials: Arc<Mutex<EncryptedTokens>>,
    /// This is not used by single-tenant runtime
    pub(crate) deployment_credentials: Arc<DeploymentCredentials>,
    pub(crate) agent: SharedAgent,
    pub(crate) db: Arc<DB>,
    pub(crate) session: Session,
}

impl<DB: FormaDB> JobArgs<DB> {
    /// Appends the message to the session, if any. Will push to the
    /// database if needed
    async fn append_msg(&self, message: Option<FormaMessage>) -> Result<(), FormaError> {
        if let Some(m) = message {
            // Push locally
            self.session.write().await.messages.push(m.clone());
            // Push to DB
            if self.db.persist() {
                self.db.push_msg(&self.session_id, &self.user_id, m).await?;
            }
        }
        Ok(())
    }
}

/// Retrieves the value in `name` from the given `HeaderMap`.
/// Returns an error if the value is not there.
fn expect_header(headers: &HeaderMap, name: &str) -> Result<String, FormaError> {
    let r = match headers.get(name) {
        Some(v) => v.to_str().map_err(|e| {
            info!("expect_header: could not parse {name} into a string: {e:?}");
            FormaError::Input(format!("could not parse {name} into a string: {e}"))
        }),
        None => {
            info!("expect_header: no {name} found in header");
            return Err(FormaError::Input(format!("no {name} found in header")));
        }
    }?;
    Ok(r.to_string())
}

/// Retrieves the user ID as provided in the header.
/// Returns an error if it is not there.
pub fn expect_user_id(headers: &HeaderMap) -> Result<String, FormaError> {
    expect_header(headers, USER_ID_HEADER)
}

/// Retrieves the Agent ID as provided in the header.
/// Returns an error if it is not there.
pub fn expect_deployment_id(headers: &HeaderMap) -> Result<String, FormaError> {
    expect_header(headers, DEPLOYMENT_ID_HEADER)
}

/// Retrieves the Session ID as provided in the header.
/// Returns an error if it is not there.
pub fn expect_session_id(headers: &HeaderMap) -> Result<String, FormaError> {
    expect_header(headers, SESSION_ID_HEADER)
}

#[instrument(skip(headers))]
pub(crate) fn get_user_id(persist: bool, headers: &HeaderMap) -> Result<String, FormaError> {
    if persist {
        expect_user_id(headers)
    } else {
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

#[instrument(skip(headers))]
pub(crate) fn get_session_id(persist: bool, headers: &HeaderMap) -> Result<String, FormaError> {
    if persist {
        let session_id = expect_session_id(headers)?;
        Ok(session_id)
    } else {
        // Let's create a fake one
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

/// Takes a header value and splits it by commas into a vector of strings
pub fn parse_user_roles(header: &str) -> Vec<String> {
    header
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Takes a header value and splits it by commas into a vector of strings
#[instrument(skip(headers))]
pub(crate) fn user_roles(headers: &HeaderMap) -> Result<Vec<String>, FormaError> {
    let h = match headers.get(USER_ROLES_HEADER) {
        Some(v) => v.to_str().map_err(|e| {
            FormaError::Input(format!(
                "could not parse {USER_ROLES_HEADER} into a string: {e}"
            ))
        }),
        None => {
            return Ok(vec![]);
        }
    }?;

    let roles: Vec<String> = parse_user_roles(h);
    Ok(roles)
}

/// The user's time zone, as their client sent it in `x-time-zone`. A value
/// that is not text, or is empty, is dropped; whether it names a real zone is
/// for the `current-date` tool to decide, which ignores one that does not.
pub(crate) fn user_timezone(headers: &HeaderMap) -> Option<String> {
    headers
        .get(TIME_ZONE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .map(str::to_string)
}

/// Load the agent from a file. Expands it as well.
#[cfg(feature = "cli")]
pub async fn load_agent<DB: FormaDB>(
    filename: &str,
    user_id: &str,
    db: Arc<DB>,
) -> Result<Agent, FormaError> {
    use crate::api::{AgentDefinition, ObjectDefinition};
    let file_contents = match std::fs::read_to_string(filename) {
        Ok(r) => r,
        Err(_) => {
            return Err(FormaError::Input(format!(
                "Failed to read agent file '{filename}'"
            )));
        }
    };
    let definition: AgentDefinition = if filename.ends_with(".json") {
        serde_json::from_str(&file_contents).map_err(|x| FormaError::Input(x.to_string()))?
    } else if filename.ends_with(".yaml") {
        serde_yaml::from_str(&file_contents).map_err(|x| FormaError::Input(x.to_string()))?
    } else {
        return Err(FormaError::Input(
            "agent file should be either `json` or `yaml`".into(),
        ));
    };
    let oauth_tokens = db.get_oauth_tokens(user_id).await?;

    let definition = definition.expand(filename)?;
    let agent = definition.into_agent(&oauth_tokens).await?;
    Callable::<DB>::check(&agent)?;

    Ok(agent)
}

#[cfg(feature = "multi-tenant")]
#[instrument(skip(db, _user_roles))]
pub(crate) async fn get_deployed_agent<DB: FormaDB>(
    db: Arc<DB>,
    agent_db_id: &str,
    user_id: &str,
    _user_roles: &[String],
) -> Result<(Agent, DeploymentCredentials), FormaError> {
    let owned = db.get_agent_deployment(agent_db_id).await.map_err(|e| {
        error!(agent_db_id, error = %e, "failed to fetch agent deployment from DB");
        e
    })?;
    // if user_id != owned.owner_id {
    //     info!(
    //         user_id,
    //         agent_db_id, "user does not own this agent deployment"
    //     );
    //     return Err(FormaError::AgentDeploymentNotFound(agent_db_id.to_string()));
    // }
    let agent = match serde_json::from_value(owned.agent) {
        Ok(agent) => agent,
        Err(e) => return Err(FormaError::Serde(e.to_string())),
    };
    let credentials = match owned.credentials {
        Some(credentials) => credentials,
        None => HashMap::with_capacity(0).into(), // Empty credentials
    };
    Ok((agent, credentials))
}

#[cfg(test)]
mod test {
    use axum::http::{HeaderMap, HeaderValue};

    use crate::server::{
        TIME_ZONE_HEADER, USER_ROLES_HEADER, get_session_id, get_user_id, user_roles, user_timezone,
    };

    #[test]
    fn reads_the_time_zone_header() {
        let mut headers = HeaderMap::new();
        assert_eq!(user_timezone(&headers), None);
        headers.insert(
            TIME_ZONE_HEADER,
            HeaderValue::from_static(" America/Santiago "),
        );
        assert_eq!(user_timezone(&headers).as_deref(), Some("America/Santiago"));
        headers.insert(TIME_ZONE_HEADER, HeaderValue::from_static(""));
        assert_eq!(user_timezone(&headers), None);
    }

    #[test]
    fn test_user_roles_single_role() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_ROLES_HEADER, HeaderValue::from_static("admin"));
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), vec!["admin"]);
    }

    #[test]
    fn test_user_roles_multiple_roles() {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_ROLES_HEADER,
            HeaderValue::from_static("admin,user,moderator"),
        );
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), vec!["admin", "user", "moderator"]);
    }

    #[test]
    fn test_user_roles_with_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_ROLES_HEADER,
            HeaderValue::from_static("admin , user , moderator"),
        );
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), vec!["admin", "user", "moderator"]);
    }

    #[test]
    fn test_user_roles_empty_string() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_ROLES_HEADER, HeaderValue::from_static(""));
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_user_roles_missing_header() {
        let headers = HeaderMap::new();
        let r = user_roles(&headers);
        assert!(r.unwrap().is_empty());
    }

    #[test]
    fn test_user_roles_empty_elements_filtered() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_ROLES_HEADER, HeaderValue::from_static("admin,,user,,"));
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), vec!["admin", "user"]);
    }

    #[test]
    fn test_no_user_roles() {
        let headers = HeaderMap::new();
        let r = user_roles(&headers);
        assert!(r.is_ok());
        assert!(r.unwrap().is_empty());
    }

    #[test]
    fn test_get_session_id_no_persist_returns_non_empty() {
        let headers = HeaderMap::new();
        let result = get_session_id(false, &headers).unwrap();

        assert!(!result.is_empty());
        assert!(result.len() > 0);
    }

    #[test]
    fn test_get_session_id_consistency_with_get_user_id() {
        let headers = HeaderMap::new();

        let session_id = get_session_id(false, &headers).unwrap();
        let user_id = get_user_id(false, &headers).unwrap();

        // Both should be valid UUIDs
        assert!(uuid::Uuid::parse_str(&session_id).is_ok());
        assert!(uuid::Uuid::parse_str(&user_id).is_ok());

        // But they should be different
        assert_ne!(session_id, user_id);
    }

    #[test]
    fn test_get_session_id_no_persist_returns_valid_uuid_format() {
        let headers = HeaderMap::new();
        let result = get_session_id(false, &headers);

        assert!(result.is_ok());
        let session_id = result.unwrap();

        // Verify UUID v4 format
        let parsed = uuid::Uuid::parse_str(&session_id);
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().get_version(), Some(uuid::Version::Random));
    }

    #[test]
    fn test_get_user_id_no_persist_generates_different_ids() {
        let headers = HeaderMap::new();
        let id1 = get_user_id(false, &headers).unwrap();
        let id2 = get_user_id(false, &headers).unwrap();

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_get_session_id_no_persist() {
        let headers = HeaderMap::new();
        let result = get_session_id(false, &headers);

        assert!(result.is_ok());
        let session_id = result.unwrap();
        assert!(!session_id.is_empty());
        assert!(uuid::Uuid::parse_str(&session_id).is_ok());
    }

    #[test]
    fn test_get_session_id_no_persist_generates_different_ids() {
        let headers = HeaderMap::new();
        let id1 = get_session_id(false, &headers).unwrap();
        let id2 = get_session_id(false, &headers).unwrap();

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_get_user_id_no_persist() {
        let headers = HeaderMap::new();
        let result = get_user_id(false, &headers);

        assert!(result.is_ok());
        let user_id = result.unwrap();
        assert!(!user_id.is_empty());
        // Verify it's a valid UUID
        assert!(uuid::Uuid::parse_str(&user_id).is_ok());
    }
}
