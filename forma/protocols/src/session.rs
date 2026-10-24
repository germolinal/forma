use chrono::{DateTime, Utc};
use genai_client::AssistantMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    sync::Arc,
};

use crate::{
    FormaDB, FormaError,
    message::{FormaMessage, FormaMessagePart},
    pending_tool_call::PendingToolCall,
};

/// What the model is told a tool returned when the user rejected calling it.
pub const REJECTED_TOOL_CALL_RESULT: &str =
    "The user rejected this tool call, so it did not run: nothing was done.";

/// What the model is told a tool returned when the user sent a new message
/// while the call was waiting for an answer, which rejects it.
pub const UNANSWERED_TOOL_CALL_RESULT: &str = "The user sent a new message instead of approving this tool call, so it did not run: nothing was done.";

/// The request body for the resume endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResumeRequest {
    /// ID of the pending tool call this feedback applies to.
    pub tool_call_id: String,
    /// If `true`, the call is rejected: the tool does not run, and the model
    /// is told the user rejected it.
    pub reject: bool,
    /// Arguments to pass to the tool (required when `reject` is false).
    pub arguments: Option<Value>,
    /// The result of the tool call (required when `reject` is false).
    pub result: Option<Value>,
}

impl ResumeRequest {
    /// Validates the request body, ensuring the required fields are present and valid.
    pub fn validate(&self) -> Result<(), FormaError> {
        if self.tool_call_id.is_empty() {
            return Err(FormaError::Input("tool_call_id is required".to_string()));
        }
        if self.reject {
            // No need for arguments
            return Ok(());
        }
        if self.arguments.is_none() && self.result.is_none() {
            return Err(FormaError::Input(
                "arguments or result is required".to_string(),
            ));
        }
        if self.arguments.is_some() && self.result.is_some() {
            return Err(FormaError::Input(
                "arguments and result cannot be both present".to_string(),
            ));
        }
        Ok(())
    }
}

/// Represents a cached result of a tool call,
/// to be used when resuming a session
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CachedToolResult {
    /// Original ID of the tool call, used to identify the result
    pub tool_call_id: String,
    /// The name of the tool that was called
    pub tool_name: String,
    /// The result of the tool call, serialized as a JSON value
    pub result: Value,
    /// Indicates whether the result is derived from a tool
    /// in a root workflow, or not.
    pub from_root: bool,
}

/// Represents the ID of a session, used to identify it in the database
#[derive(Debug, Serialize, Deserialize, Clone, Default, Eq, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SessionId(pub String);

impl Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Represents the data associated with a session, including its ID and creation date.
pub type SessionData = GenericSessionData<SessionId, DateTime<Utc>>;

/// A lightweight summary of a session, for listing a user's past
/// conversations without pulling in every message they contain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SessionSummary {
    /// The ID of the session.
    pub id: String,
    /// When the session was created.
    pub created_at: Option<DateTime<Utc>>,
    /// A short label for the session, derived from its first user message.
    pub display_name: String,
}

/// Derives a short display name for a session from its messages: the first
/// ~60 characters of its first user message, trimmed to a word boundary with
/// an ellipsis if it was cut short. Falls back to "New chat" when there is no
/// user message yet (e.g. a session that was created but never used).
pub fn session_display_name(messages: &[FormaMessage]) -> String {
    const MAX_LEN: usize = 60;

    let Some(first_user) = messages
        .iter()
        .find(|m| matches!(m.content, crate::FormaMessageBody::User { .. }))
    else {
        return "New chat".to_string();
    };

    let text: String = (&first_user.content).into();
    let text = text.trim();
    if text.chars().count() <= MAX_LEN {
        return text.to_string();
    }

    let truncated: String = text.chars().take(MAX_LEN).collect();
    match truncated.rsplit_once(' ') {
        Some((head, _)) if !head.is_empty() => format!("{head}…"),
        _ => format!("{truncated}…"),
    }
}

/// The longest name a user may give a conversation. Anything longer is cut
/// down to it: the sidebar truncates it visually anyway, and unbounded
/// strings have no business reaching the database.
pub const MAX_SESSION_NAME_LEN: usize = 100;

/// Normalises the name a user typed for a conversation: trimmed, and capped
/// at [`MAX_SESSION_NAME_LEN`] characters. Returns `None` when nothing is
/// left, which is how a user clears a name they gave earlier and puts the
/// session back to the one derived by [`session_display_name`].
pub fn normalize_session_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.chars().take(MAX_SESSION_NAME_LEN).collect())
}

/// Data associated with a conversation, to be stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = SessionData, bound = ""))]
pub struct GenericSessionData<ID, DATE> {
    /// The ID of the session itslf
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub id: Option<ID>,

    /// The date and time when the session was created
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = DateTime))]
    pub created_at: Option<DATE>,

    /// A flag indicating whether the session is suspended,
    /// meaning that it is expecting external or human feedback.
    #[serde(default)]
    pub pending_tool_calls: Vec<PendingToolCall>,
    /// When the run now working on this session started, if one is. Set and
    /// cleared through [`FormaDB::start_run`] and [`FormaDB::finish_run`],
    /// which keep a second run from writing to the same history at once —
    /// say, a user who left mid-run and came back to send another message.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = DateTime))]
    pub running_since: Option<DATE>,
    /// The ID of the user who ownes this session. When a new
    /// message is requested, Forma will check that the user owns
    /// the corresponding session
    pub user_id: String,
    /// The id of the agent involved in this conversation
    pub agent_id: String,
    /// The id of the channel, as chosen by the developer (e.g., 'whatsapp' or 'web')
    #[serde(default)]
    pub channel_id: String,
    /// The name the user gave this conversation, if they renamed it. `None`
    /// — which is every session that was never renamed, including all the
    /// ones stored before this field existed — means the name shown for it is
    /// derived from its first message by [`session_display_name`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    /// The messages that have been exchanged in this session
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<crate::message::GenericFormaMessage<String>>))]
    pub messages: Vec<FormaMessage>,
    /// Contains the variables available for templates of
    /// runnables downstream.
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub variables: HashMap<String, Value>,
    /// Contains the results of each node execution
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub cached_messages: HashMap<String, AssistantMessage>,
    /// The result of the tools that have been called,
    /// cached by the tool-call id
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub tool_calls_results: HashMap<String, CachedToolResult>,
    /// Files a tool produced during this turn — a generated picture — which
    /// reach `messages` only once the turn is saved, from the events it
    /// streamed. Kept so the same turn can already refer to them (to save one
    /// into a workspace, say). Never stored: by the next turn they are in the
    /// history.
    #[serde(skip)]
    pub produced_files: Vec<FormaMessagePart>,
}

impl<ID, DATE> Default for GenericSessionData<ID, DATE>
where
    ID: Serialize + for<'a> Deserialize<'a> + Clone + Display + Debug,
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn default() -> Self {
        Self {
            id: None,
            created_at: None,
            pending_tool_calls: Vec::new(),
            running_since: None,
            user_id: String::new(),
            agent_id: String::new(),
            channel_id: String::new(),
            name: None,
            messages: Vec::new(),
            variables: HashMap::new(),
            cached_messages: HashMap::new(),
            tool_calls_results: HashMap::new(),
            produced_files: Vec::new(),
        }
    }
}

impl<ID, DATE> GenericSessionData<ID, DATE>
where
    ID: Serialize + for<'a> Deserialize<'a> + Clone + Display + Debug,
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    /// Checks if the session has any pending tool calls.
    pub fn is_pending(&self) -> bool {
        self.pending_tool_calls.iter().any(|c| !c.approved)
    }

    /// Collects the pending tool calls associated with a specific node.
    pub fn collect_pending_tool_calls(&self, node_id: &str) -> Vec<PendingToolCall> {
        self.pending_tool_calls
            .iter()
            .filter(|call| call.node_id == node_id)
            .cloned()
            .collect()
    }

    /// Retrieves the ID of the session. Returns an Internal Server error
    /// if the session does not have na ID
    pub fn id_string(&self) -> Result<String, FormaError> {
        match &self.id {
            Some(id) => Ok(id.to_string()),
            None => Err(FormaError::Execution(
                "when retrieving session id: session has no id".to_string(),
            )),
        }
    }

    /// The pending call with this ID, if it is still waiting for an answer.
    /// A call that was already answered, or that finished, cannot be answered
    /// (again).
    fn unanswered_call(&mut self, tool_call_id: &str) -> Result<&mut PendingToolCall, FormaError> {
        self.pending_tool_calls
            .iter_mut()
            .find(|call| call.tool_call_id == tool_call_id && !call.approved)
            .ok_or_else(|| {
                FormaError::Input(format!(
                    "tool call '{tool_call_id}' is not waiting for an answer"
                ))
            })
    }

    /// Rejects a pending tool call. The tool will not run: the rejection
    /// becomes its result, so the model learns it was rejected and the
    /// history keeps every call followed by its result.
    fn reject_tool_call(&mut self, tool_call_id: &str) -> Result<bool, FormaError> {
        tracing::info!("rejecting tool call: {}", tool_call_id);
        self.accept_tool_call_result(
            tool_call_id,
            Value::String(REJECTED_TOOL_CALL_RESULT.to_string()),
        )
    }

    fn accept_tool_call_result(
        &mut self,
        tool_call_id: &str,
        approved_result: Value,
    ) -> Result<bool, FormaError> {
        tracing::info!("accepting tool call result: {}", tool_call_id);
        let call = self.unanswered_call(tool_call_id)?;
        call.approved = true;
        let (tool_name, from_root) = (call.tool_name.clone(), call.from_root);
        // Add result in cache
        let cached_result = CachedToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name,
            result: approved_result.clone(),
            from_root,
        };
        self.tool_calls_results
            .insert(tool_call_id.to_string(), cached_result);
        Ok(self.is_pending())
    }

    /// Accepts the inputs for a pending tool call.
    /// Because the user has the chance to modify the inputs to the
    /// tool call, we need to update the pending tool call with the
    /// approved inputs.
    fn accept_tool_call_inputs(
        &mut self,
        tool_call_id: &str,
        approved_arguments: Value,
    ) -> Result<bool, FormaError> {
        tracing::info!("accepting tool call inputs: {}", tool_call_id);
        let call = self.unanswered_call(tool_call_id)?;
        call.approved = true;
        call.arguments = approved_arguments;
        Ok(self.is_pending())
    }
}

impl SessionData {
    /// Applies the client's answer to a pending tool call, and stores the
    /// resulting state, so that it survives until every call is answered.
    ///
    /// Returns whether other calls are still waiting for an answer.
    pub async fn handle_resume<DB: FormaDB>(
        &mut self,
        request: &ResumeRequest,
        db: &Arc<DB>,
    ) -> Result<bool, FormaError> {
        if !request.reject && request.result.is_some() && request.arguments.is_some() {
            return Err(FormaError::Input(
                "resuming a session requires either a result or arguments, not both".to_string(),
            ));
        }

        let still_pending = if request.reject {
            self.reject_tool_call(&request.tool_call_id)?
        } else if let Some(approved_result) = &request.result {
            self.accept_tool_call_result(&request.tool_call_id, approved_result.clone())?
        } else if let Some(approved_arguments) = &request.arguments {
            self.accept_tool_call_inputs(&request.tool_call_id, approved_arguments.clone())?
        } else {
            return Err(FormaError::Input(
                "resuming a session requires at least a result or arguments field".to_string(),
            ));
        };
        db.save_pending_tool_calls(self).await?;
        Ok(still_pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentDeployment, ChatMessageState, DeploymentCredentials, EncryptedTokens, FormaMessage,
    };
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::sync::RwLock;

    /// A minimal in-memory `FormaDB` for exercising the resume logic.
    /// It records every message handed to `__append_msg`, and the pending
    /// calls last saved, so tests can check what reaches the database.
    #[derive(Clone, Debug, Default)]
    struct MockDB {
        appended: Arc<Mutex<Vec<FormaMessage>>>,
        saved: Arc<Mutex<Option<Vec<PendingToolCall>>>>,
    }

    #[async_trait::async_trait]
    impl FormaDB for MockDB {
        fn persist(&self) -> bool {
            false
        }
        async fn new() -> Result<Self, FormaError> {
            Ok(Self::default())
        }
        fn init(&mut self) -> Result<(), FormaError> {
            Ok(())
        }
        async fn __append_msg(&self, msg: FormaMessage) -> Result<FormaMessage, FormaError> {
            self.appended.lock().unwrap().push(msg.clone());
            Ok(msg)
        }
        async fn get_oauth_tokens(&self, _user_id: &str) -> Result<EncryptedTokens, FormaError> {
            Ok(Default::default())
        }
        async fn save_oauth_tokens(
            &self,
            _user_id: &str,
            _tokens: EncryptedTokens,
        ) -> Result<(), FormaError> {
            Ok(())
        }
        async fn remove_oauth_tokens(&self, _user_id: &str) -> Result<(), FormaError> {
            Ok(())
        }
        async fn new_session(
            &self,
            agent_id: &str,
            user_id: &str,
        ) -> Result<SessionData, FormaError> {
            Ok(SessionData {
                id: Some(SessionId("session".to_string())),
                user_id: user_id.to_string(),
                agent_id: agent_id.to_string(),
                ..Default::default()
            })
        }
        async fn clear_state(&self, _session_id: &str) -> Result<(), FormaError> {
            Ok(())
        }
        async fn start_run(
            &self,
            _session_id: &str,
            _stale_before: DateTime<Utc>,
        ) -> Result<bool, FormaError> {
            Ok(true)
        }
        async fn finish_run(&self, _session_id: &str) -> Result<(), FormaError> {
            Ok(())
        }
        async fn get_session_by_id(
            &self,
            session_id: &str,
            _n_messages: usize,
        ) -> Result<SessionData, FormaError> {
            Err(FormaError::SessionNotFound(session_id.to_string()))
        }
        async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError> {
            Err(FormaError::MessageNotFound(message_id.to_string()))
        }
        async fn suspend_session(
            &self,
            session: Arc<RwLock<SessionData>>,
            call: &PendingToolCall,
        ) -> Result<(), FormaError> {
            session.write().await.pending_tool_calls.push(call.clone());
            Ok(())
        }
        async fn save_pending_tool_calls(&self, session: &SessionData) -> Result<(), FormaError> {
            *self.saved.lock().unwrap() = Some(session.pending_tool_calls.clone());
            Ok(())
        }
        async fn get_session_by_user_id(
            &self,
            agent_id: &str,
            user_id: &str,
            _n_messages: usize,
        ) -> Result<SessionData, FormaError> {
            Err(FormaError::SessionNotFound(format!("{user_id}:{agent_id}")))
        }
        async fn delete_session(&self, session_id: &str, _user_id: &str) -> Result<(), FormaError> {
            Err(FormaError::SessionNotFound(session_id.to_string()))
        }
        async fn rename_session(
            &self,
            session_id: &str,
            _user_id: &str,
            _name: Option<&str>,
        ) -> Result<(), FormaError> {
            Err(FormaError::SessionNotFound(session_id.to_string()))
        }
        async fn get_user_sessions(
            &self,
            _agent_id: &str,
            _user_id: &str,
            _n_sessions: usize,
        ) -> Result<Vec<SessionSummary>, FormaError> {
            Ok(Vec::new())
        }
        async fn update_msg_status(
            &self,
            _session_id: &str,
            _message_id: &str,
            _status: ChatMessageState,
        ) -> Result<(), FormaError> {
            Ok(())
        }
        async fn add_msg_reaction(
            &self,
            _session_id: &str,
            _message_id: &str,
            _emoji: &Option<String>,
        ) -> Result<(), FormaError> {
            Ok(())
        }
        async fn get_agent_deployment(
            &self,
            agent_id: &str,
        ) -> Result<AgentDeployment, FormaError> {
            Err(FormaError::AgentDeploymentNotFound(agent_id.to_string()))
        }
        async fn new_agent_deployment(
            &self,
            _user_id: &str,
            _agent: Value,
            _credentials: Option<DeploymentCredentials>,
        ) -> Result<String, FormaError> {
            Ok(String::new())
        }

        async fn update_agent_deployment(
            &self,
            _user_id: &str,
            _deployment_id: &str,
            _agent: Value,
            _credentials: Option<DeploymentCredentials>,
        ) -> Result<String, FormaError> {
            Ok(String::new())
        }

        async fn remove_agent_deployment(
            &self,
            _user_id: &str,
            deployment_id: &str,
        ) -> Result<String, FormaError> {
            Ok(deployment_id.to_string())
        }
    }

    fn pending(tool_call_id: &str) -> PendingToolCall {
        PendingToolCall {
            tool_call_id: tool_call_id.to_string(),
            node_id: "node".to_string(),
            tool_name: "search".to_string(),
            arguments: json!({ "q": "original" }),
            approved: false,
            from_root: true,
            tool_type: String::new(),
            bundle: None,
        }
    }

    fn session_with(pending_calls: Vec<PendingToolCall>) -> SessionData {
        SessionData {
            id: Some(SessionId("session-1".to_string())),
            user_id: "user-1".to_string(),
            agent_id: "agent-1".to_string(),
            pending_tool_calls: pending_calls,
            ..Default::default()
        }
    }

    fn accept_inputs(tool_call_id: &str, arguments: Value) -> ResumeRequest {
        ResumeRequest {
            tool_call_id: tool_call_id.to_string(),
            reject: false,
            arguments: Some(arguments),
            result: None,
        }
    }

    fn accept_result(tool_call_id: &str, result: Value) -> ResumeRequest {
        ResumeRequest {
            tool_call_id: tool_call_id.to_string(),
            reject: false,
            arguments: None,
            result: Some(result),
        }
    }

    fn reject(tool_call_id: &str) -> ResumeRequest {
        ResumeRequest {
            tool_call_id: tool_call_id.to_string(),
            reject: true,
            arguments: None,
            result: None,
        }
    }

    // ---- ResumeRequest::validate ----

    #[test]
    fn validate_rejects_empty_tool_call_id() {
        let req = ResumeRequest {
            tool_call_id: String::new(),
            reject: false,
            arguments: Some(json!({})),
            result: None,
        };
        assert!(matches!(req.validate(), Err(FormaError::Input(_))));
    }

    #[test]
    fn validate_allows_rejection_without_payload() {
        assert!(reject("call-1").validate().is_ok());
    }

    #[test]
    fn validate_requires_arguments_or_result_when_accepting() {
        let req = ResumeRequest {
            tool_call_id: "call-1".to_string(),
            reject: false,
            arguments: None,
            result: None,
        };
        assert!(matches!(req.validate(), Err(FormaError::Input(_))));
    }

    #[test]
    fn validate_rejects_both_arguments_and_result() {
        let req = ResumeRequest {
            tool_call_id: "call-1".to_string(),
            reject: false,
            arguments: Some(json!({})),
            result: Some(json!({})),
        };
        assert!(matches!(req.validate(), Err(FormaError::Input(_))));
    }

    #[test]
    fn validate_accepts_arguments_only() {
        assert!(
            accept_inputs("call-1", json!({ "q": "x" }))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn validate_accepts_result_only() {
        assert!(accept_result("call-1", json!("done")).validate().is_ok());
    }

    // ---- Accepting tool-call inputs ----

    #[tokio::test]
    async fn accept_inputs_marks_approved_and_overrides_arguments() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let still_pending = session
            .handle_resume(&accept_inputs("call-1", json!({ "q": "edited" })), &db)
            .await
            .unwrap();

        assert!(!still_pending);
        let call = &session.pending_tool_calls[0];
        assert!(call.approved);
        assert_eq!(call.arguments, json!({ "q": "edited" }));
        // Accepting inputs does not cache a result.
        assert!(session.tool_calls_results.is_empty());
        // Nothing is persisted for an input approval.
        assert!(db.appended.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn accept_inputs_leaves_other_calls_pending() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1"), pending("call-2")]);

        let still_pending = session
            .handle_resume(&accept_inputs("call-1", json!({})), &db)
            .await
            .unwrap();

        assert!(still_pending, "call-2 is still awaiting approval");
        assert!(session.pending_tool_calls[0].approved);
        assert!(!session.pending_tool_calls[1].approved);
    }

    #[tokio::test]
    async fn accept_inputs_unknown_tool_call_errors() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let err = session
            .handle_resume(&accept_inputs("missing", json!({})), &db)
            .await
            .unwrap_err();

        assert!(matches!(err, FormaError::Input(_)));
        // Unchanged.
        assert!(!session.pending_tool_calls[0].approved);
    }

    // ---- Accepting tool-call results ----

    #[tokio::test]
    async fn accept_result_caches_and_marks_approved() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let still_pending = session
            .handle_resume(&accept_result("call-1", json!({ "answer": 42 })), &db)
            .await
            .unwrap();

        assert!(!still_pending);
        assert!(session.pending_tool_calls[0].approved);

        let cached = session.tool_calls_results.get("call-1").unwrap();
        assert_eq!(cached.tool_call_id, "call-1");
        assert_eq!(cached.tool_name, "search");
        assert_eq!(cached.result, json!({ "answer": 42 }));
        assert!(cached.from_root);
    }

    #[tokio::test]
    async fn accept_result_unknown_tool_call_errors() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let err = session
            .handle_resume(&accept_result("missing", json!("x")), &db)
            .await
            .unwrap_err();

        assert!(matches!(err, FormaError::Input(_)));
        assert!(session.tool_calls_results.is_empty());
    }

    // ---- Rejecting tool calls ----

    #[tokio::test]
    async fn reject_answers_the_call_with_a_rejection() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let still_pending = session.handle_resume(&reject("call-1"), &db).await.unwrap();

        assert!(!still_pending);
        assert!(session.pending_tool_calls[0].approved);
        // The rejection is the call's result: the tool will not run, and the
        // model is told why.
        let cached = session.tool_calls_results.get("call-1").unwrap();
        assert_eq!(cached.result, json!(REJECTED_TOOL_CALL_RESULT));
        assert_eq!(cached.tool_name, "search");
        // The history is written by the run that follows, not here.
        assert!(session.messages.is_empty());
        assert!(db.appended.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reject_keeps_other_pending_calls() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![
            pending("call-1"),
            pending("call-2"),
            pending("call-3"),
        ]);

        let still_pending = session.handle_resume(&reject("call-2"), &db).await.unwrap();

        assert!(still_pending, "call-1 and call-3 are still pending");
        assert!(!session.pending_tool_calls[0].approved);
        assert!(session.pending_tool_calls[1].approved);
        assert!(!session.pending_tool_calls[2].approved);
    }

    #[tokio::test]
    async fn an_unknown_call_cannot_be_rejected() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let err = session
            .handle_resume(&reject("missing"), &db)
            .await
            .unwrap_err();

        assert!(matches!(err, FormaError::Input(_)));
        assert!(session.tool_calls_results.is_empty());
        assert!(db.saved.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn an_answered_call_cannot_be_answered_again() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);
        session
            .handle_resume(&accept_inputs("call-1", json!({ "q": "edited" })), &db)
            .await
            .unwrap();

        for req in [reject("call-1"), accept_inputs("call-1", json!({}))] {
            let err = session.handle_resume(&req, &db).await.unwrap_err();
            assert!(matches!(err, FormaError::Input(_)));
        }
        assert_eq!(
            session.pending_tool_calls[0].arguments,
            json!({ "q": "edited" })
        );
        assert!(session.tool_calls_results.is_empty());
    }

    #[tokio::test]
    async fn every_answer_is_saved() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1"), pending("call-2")]);

        session
            .handle_resume(&accept_inputs("call-1", json!({})), &db)
            .await
            .unwrap();

        // call-2 is still pending, so no run follows: unless it is saved, the
        // approval of call-1 is gone by the next request.
        let saved = db.saved.lock().unwrap().clone().unwrap();
        assert!(saved[0].approved);
        assert!(!saved[1].approved);
    }

    // ---- handle_resume dispatch guards ----

    #[tokio::test]
    async fn handle_resume_rejects_both_result_and_arguments() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let req = ResumeRequest {
            tool_call_id: "call-1".to_string(),
            reject: false,
            arguments: Some(json!({})),
            result: Some(json!({})),
        };
        let err = session.handle_resume(&req, &db).await.unwrap_err();
        assert!(matches!(err, FormaError::Input(_)));
    }

    #[tokio::test]
    async fn handle_resume_requires_result_or_arguments() {
        let db = Arc::new(MockDB::default());
        let mut session = session_with(vec![pending("call-1")]);

        let req = ResumeRequest {
            tool_call_id: "call-1".to_string(),
            reject: false,
            arguments: None,
            result: None,
        };
        let err = session.handle_resume(&req, &db).await.unwrap_err();
        assert!(matches!(err, FormaError::Input(_)));
    }

    #[test]
    fn is_pending_reflects_unapproved_calls() {
        let mut session = session_with(vec![pending("call-1")]);
        assert!(session.is_pending());
        session.pending_tool_calls[0].approved = true;
        assert!(!session.is_pending());
    }

    // ---- session_display_name ----

    #[test]
    fn display_name_falls_back_when_no_user_message() {
        assert_eq!(session_display_name(&[]), "New chat");

        let assistant_only = vec![FormaMessage::new_assistant_with_content(
            "hello there".to_string(),
        )];
        assert_eq!(session_display_name(&assistant_only), "New chat");
    }

    #[test]
    fn display_name_uses_short_first_user_message_verbatim() {
        let messages = vec![FormaMessage::new_user_with_content(
            "what's the weather like?".to_string(),
        )];
        assert_eq!(session_display_name(&messages), "what's the weather like?");
    }

    #[test]
    fn display_name_finds_first_user_message_even_if_not_first_overall() {
        let messages = vec![
            FormaMessage::new_assistant_with_content(
                "(a greeting the runtime injected)".to_string(),
            ),
            FormaMessage::new_user_with_content("hi, can you help me plan a trip?".to_string()),
        ];
        assert_eq!(
            session_display_name(&messages),
            "hi, can you help me plan a trip?"
        );
    }

    #[test]
    fn display_name_truncates_long_message_at_word_boundary() {
        let long = "please summarize the quarterly earnings report and highlight anything unusual";
        let messages = vec![FormaMessage::new_user_with_content(long.to_string())];
        let name = session_display_name(&messages);
        assert!(name.ends_with('…'));
        assert!(name.chars().count() <= 61); // 60 chars + ellipsis
        assert!(long.starts_with(name.trim_end_matches('…')));
    }

    // ---- normalize_session_name ----

    #[test]
    fn a_name_is_taken_trimmed() {
        assert_eq!(
            normalize_session_name("  Budget talk \n"),
            Some("Budget talk".to_string())
        );
    }

    #[test]
    fn a_blank_name_clears_it() {
        assert_eq!(normalize_session_name(""), None);
        assert_eq!(normalize_session_name("   \t "), None);
    }

    #[test]
    fn a_long_name_is_cut_to_the_cap() {
        // Counted in characters, not bytes, so a multi-byte name is not cut
        // in the middle of one.
        let long = "ñ".repeat(MAX_SESSION_NAME_LEN + 40);
        let name = normalize_session_name(&long).unwrap();
        assert_eq!(name.chars().count(), MAX_SESSION_NAME_LEN);
    }
}
