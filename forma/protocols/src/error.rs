use reqwest::{Error as ReqwestError, StatusCode};
use serde_json::Error as SerdeError;
use std::{error::Error, fmt::Display};
use tokio::sync::mpsc::error::SendError;

/// Represents an error that can occur during the execution of a request.
#[derive(Debug, Clone)]
pub enum FormaError {
    /// An error related to the communication channels
    Channel(String),
    /// An error that happens when processing the request.
    Execution(String),
    /// A user error
    Input(String),
    /// An error that happens when making an API call to external services.
    ApiCall {
        /// The URL of the API endpoint that was called.
        url: Option<String>,
        /// The error message from the API call.
        msg: String,
    },
    /// An error related to serialization and deserialization of data.
    Serde(String),
    /// An error that happens when running evaluations
    EvalError(String),
    /// Returned when another run is still working on the session, so starting
    /// one now would have two runs writing to the same history.
    SessionBusy(String),
    /// Returned when the requested session is not found.
    SessionNotFound(String),
    /// Returned when the requested agent is not found.
    AgentDeploymentNotFound(String),
    /// Returned when the requested message is not found.
    MessageNotFound(String),
    /// Returned for a message that is already in its session: a channel
    /// delivered it again (WhatsApp redelivers whatever it is not sure
    /// arrived), and answering it twice would repeat the work and the reply.
    DuplicateMessage(String),
    /// Returned when a request cannot prove where it comes from (e.g., a
    /// webhook whose signature does not match).
    Unauthorized(String),
    /// Returned when the user is not authorized to
    /// access the requested resource.
    SessionNotOwnedByUser(String),
    /// Returned when there is an OAuth error.
    OAuthError {
        /// The URL of the OAuth provider.
        url: String,
        /// The error message from the OAuth provider.
        msg: String,
    },
    /// Returned when an environment variable was not found
    MissingEnvVar {
        /// The name of the environment variable that was not found.
        key: String,
        /// The error message from the environment variable lookup.
        msg: String,
    },
}

impl From<FormaError> for (StatusCode, String) {
    fn from(error: FormaError) -> Self {
        match error {
            FormaError::Input(msg) => (StatusCode::BAD_REQUEST, msg),
            FormaError::Channel(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            FormaError::Execution(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            FormaError::ApiCall { msg, .. } => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            FormaError::SessionNotFound(id) => {
                (StatusCode::NOT_FOUND, format!("Session not found: {id}"))
            }
            FormaError::AgentDeploymentNotFound(id) => (
                StatusCode::NOT_FOUND,
                format!("Agent deployment not found: {id}"),
            ),

            FormaError::Serde(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            FormaError::EvalError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            FormaError::SessionBusy(id) => (StatusCode::CONFLICT, format!("Session busy: {id}")),
            FormaError::MessageNotFound(id) => {
                (StatusCode::NOT_FOUND, format!("Message not found: {id}"))
            }
            FormaError::DuplicateMessage(id) => (
                StatusCode::CONFLICT,
                format!("Message already received: {id}"),
            ),
            FormaError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            FormaError::SessionNotOwnedByUser(id) => (
                StatusCode::FORBIDDEN,
                format!("Session not owned by user: {id}"),
            ),
            FormaError::OAuthError { url, msg } => (
                StatusCode::UNAUTHORIZED,
                format!("OAuth Error: {msg} (url: {url})"),
            ),
            FormaError::MissingEnvVar { key, msg } => (
                StatusCode::BAD_REQUEST,
                format!("Missing environment variable: {key} ({msg})"),
            ),
        }
    }
}

impl Display for FormaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormaError::Input(msg) => write!(f, "Input Error: {msg}"),
            FormaError::Channel(msg) => write!(f, "Channel Error: {msg}"),
            FormaError::Execution(msg) => write!(f, "Runtime Error: {msg}"),
            FormaError::ApiCall { msg, .. } => write!(f, "API Call Error: {msg}"),
            FormaError::Serde(msg) => write!(f, "Serde Error: {msg}"),
            FormaError::EvalError(msg) => write!(f, "Eval Error: {msg}"),
            FormaError::SessionBusy(id) => write!(f, "Session busy: {id}"),
            FormaError::SessionNotFound(id) => write!(f, "Session not found: {id}"),
            FormaError::MessageNotFound(id) => write!(f, "Message not found: {id}"),
            FormaError::DuplicateMessage(id) => write!(f, "Message already received: {id}"),
            FormaError::Unauthorized(msg) => write!(f, "Unauthorized: {msg}"),
            FormaError::SessionNotOwnedByUser(id) => write!(f, "Session not owned by user: {id}"),
            FormaError::OAuthError { url, msg } => write!(f, "OAuth Error: {msg} (url: {url})"),
            FormaError::MissingEnvVar { key, msg } => {
                write!(f, "Missing environment variable: {key} ({msg})")
            }
            FormaError::AgentDeploymentNotFound(id) => write!(f, "Agent not found: {id}"),
        }
    }
}
impl Error for FormaError {}

impl<T> From<SendError<T>> for FormaError {
    fn from(err: SendError<T>) -> Self {
        Self::Channel(err.to_string())
    }
}
impl From<String> for FormaError {
    fn from(err: String) -> Self {
        Self::Execution(err)
    }
}
impl From<&str> for FormaError {
    fn from(err: &str) -> Self {
        Self::Execution(err.to_string())
    }
}

impl From<ReqwestError> for FormaError {
    fn from(err: ReqwestError) -> Self {
        let source: String = match err.source() {
            Some(s) => s.to_string(),
            None => "unknown source".to_string(),
        };
        Self::ApiCall {
            url: err.url().map(|u| u.to_string()),
            msg: format!("'{err}' caused by '{source}'"),
        }
    }
}

impl From<SerdeError> for FormaError {
    fn from(err: SerdeError) -> Self {
        Self::Serde(err.to_string())
    }
}

#[cfg(test)]
mod testing {

    use reqwest::StatusCode;
    use serde::Deserialize;

    use crate::FormaError;

    #[test]
    fn fmt() {
        let err = "oh no!";
        assert_eq!(
            FormaError::Input(err.to_string()).to_string(),
            format!("Input Error: {err}")
        );
        assert_eq!(
            FormaError::Channel(err.to_string()).to_string(),
            format!("Channel Error: {err}")
        );
        assert_eq!(
            FormaError::Execution(err.to_string()).to_string(),
            format!("Runtime Error: {err}")
        );
        assert_eq!(
            FormaError::ApiCall {
                url: None,
                msg: err.to_string()
            }
            .to_string(),
            format!("API Call Error: {err}")
        );
        assert_eq!(
            FormaError::Serde(err.to_string()).to_string(),
            format!("Serde Error: {err}")
        );
        assert_eq!(
            FormaError::EvalError(err.to_string()).to_string(),
            format!("Eval Error: {err}")
        );
    }

    #[test]
    fn from_string() {
        let err = "some error";
        if let FormaError::Execution(s) = err.to_string().into() {
            assert_eq!(s, err);
        }
    }

    #[test]
    fn from_str() {
        let err = "some error";
        if let FormaError::Execution(s) = err.into() {
            assert_eq!(s, err);
        }
    }

    #[derive(Deserialize, Debug)]
    struct SomeStruct {
        _name: String,
    }

    #[test]
    fn from_serde_err() {
        let res = serde_json::from_str::<SomeStruct>(r#"{ "_name": 1}"#);
        assert!(res.is_err());
        let err: FormaError = res.unwrap_err().into();
        if let FormaError::Serde(_) = err {
            println!("ok")
        } else {
            panic!("serde error should turn into an AgentError::Serde error")
        }
    }

    #[test]
    fn test_status_codes() {
        let err = FormaError::DuplicateMessage("message_id".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::CONFLICT);

        let err = FormaError::Unauthorized("bad signature".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let err = FormaError::SessionBusy("session_id".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::CONFLICT);

        let err = FormaError::SessionNotFound("session_id".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::NOT_FOUND);

        let err = FormaError::SessionNotOwnedByUser("session_id".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::FORBIDDEN);

        let err = FormaError::Channel("channel_error".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

        let err = FormaError::Execution("execution_error".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

        let err = FormaError::Input("input_error".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let err = FormaError::ApiCall {
            url: None,
            msg: "api_error".to_string(),
        };
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

        let err = FormaError::EvalError("eval_error".to_string());
        let (status, _msg) = err.into();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
