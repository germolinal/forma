#![warn(clippy::dbg_macro)]
#![deny(missing_docs)]
//! A library with all protocols defining how messages are shared
//! and what they are made of

/// A Session wrapped in Arc and RwLock for safe
/// multithreaded/async sharing
pub type Session = Arc<RwLock<SessionData>>;

/// Defines the interface for a Forma database.
pub mod db_trait;
pub use db_trait::{AgentDeployment, DeploymentCredentials, FORMA_DATABASE_ID, FormaDB};

/// Defines a backend-agnostic interface over a knowledgebase of documents,
/// used by agentic-retrieval tools.
pub mod docstore;
pub use docstore::{
    AttachedDoc, DocContent, DocMeta, DocStore, SignedAccess, SignedRequest, SignedUpload,
};

/// The definitions of errors that can occur during the execution of a request.
pub mod error;
use std::sync::Arc;

pub use error::FormaError;

/// Utilities for accessing environment variables used by Forma.
pub mod envar;

/// Definitions determining the elements of a
/// session that a Forma agent is expecting
pub mod session;
pub use session::{
    CachedToolResult, MAX_SESSION_NAME_LEN, ResumeRequest, SessionData, SessionId, SessionSummary,
    normalize_session_name, session_display_name,
};

/// The definition of a tool that has been registered
/// in the session. This session is now pending,
/// waiting for all tool calls to be resolved externally
/// and submitted.
pub mod pending_tool_call;
pub use pending_tool_call::PendingToolCall;

/// The definition of a Message, as stored
/// in the short-term memory
pub mod message;
pub use message::{
    ChatMessageState, FormaAudioPart, FormaFile, FormaFilePart, FormaImagePart, FormaImageUrl,
    FormaInputAudio, FormaMessage, FormaMessageBody, FormaMessagePart, FormaTextPart,
    FormaWorkspaceFilePart, MessageId, from_provider_parts, into_provider_parts,
};
// reexport basic GenAI ones.
pub use genai_client::{
    AssistantMessage, LLMMessage,
    openai::v1::{OpenAIMessageContent, OpenAIMessagePart, OpenAITextPart, OpenAIToolMessage},
};

/// The structure containing the OAuth tokens
/// for a user (e.g., MCP server credentials).
pub mod credentials;
pub use credentials::{EncryptedTokens, OauthToken, UserTokens};
use tokio::sync::RwLock;

/// The default port for the Forma agent API.
pub const DEFAULT_PORT: u16 = 8080;
/// The environment variable name used to specify the Forma agent API key.
pub const FORMA_KEY_ENV_VAR: &str = "FORMA_AGENT_KEY";
/// The HTTP header name used to specify the Forma agent API key.
pub const FORMA_AGENT_API_KEY: &str = "x-api-key";
/// The path for the init session endpoint.
pub const INIT_SESSION_PATH: &str = "/v1/init";
/// The path for the chat endpoint.
pub const CHAT_PATH: &str = "/v1/chat";
/// The path for the resume endpoint.
pub const RESUME_PATH: &str = "/v1/resume";
/// The HTTP header name used to specify the user ID.
pub const USER_ID_HEADER: &str = "x-user-id";
/// The HTTP header name used to specify the session ID.
pub const SESSION_ID_HEADER: &str = "x-session-id";
/// The HTTP header name used to specify the agent ID.
pub const DEPLOYMENT_ID_HEADER: &str = "x-deployment-id";
/// The HTTP header name used to specify the user roles.
pub const USER_ROLES_HEADER: &str = "x-user-roles";
/// The user's time zone, as an IANA name (`America/Santiago`), sent by clients
/// that know it (the web app, the Apple client). Read by the `current-date`
/// tool; like the roles, it is not verified.
pub const TIME_ZONE_HEADER: &str = "x-time-zone";
/// The environment variable name used to specify the URL of the sessions database.
pub const DB_CONNECTION_STRING_ENV: &str = "SESSIONS_DB_URL";
/// The environment variable name used to specify the Forma encryption key.
pub const FORMA_ENCRYPTION_KEY_ENV: &str = "FORMA_ENCRYPTION_KEY";
