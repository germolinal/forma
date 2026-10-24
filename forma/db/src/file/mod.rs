use std::{collections::HashMap, path::Path, path::PathBuf, sync::Arc};

use protocols::{
    AgentDeployment, ChatMessageState, DeploymentCredentials, EncryptedTokens, FormaDB, FormaError,
    FormaMessage, PendingToolCall, SessionData, SessionId, SessionSummary, session_display_name,
};
use tokio::sync::RwLock;
use uuid::Uuid;

const DEFAULT_PATH: &str = "./forma-db";

/// A backend that stores everything in the local filesystem.
///
/// Layout under `path/`:
/// - `sessions/{id}.json`  — full SessionData including messages
/// - `tokens/{key}.json`   — OAuth token map per user+agent
/// - `index.json`          — user_id:agent_id → session_id
/// - `msg_index.json`      — message_id → session_id
/// - `deployments/{id}.json` — an `AgentDeployment`, keyed by a DB-assigned
///   deployment id (not the logical `agent_id`, since a user can have several
///   deployments of the same `agent_id`)
#[derive(Debug, Clone)]
pub struct FileDB {
    /// The filesystem path where data is stored.
    path: Arc<Path>,
}

impl FileDB {
    /// Creates a new `FileDB` with the specified path.
    pub fn new_with_path(mut path: String) -> Result<Self, FormaError> {
        eprintln!("Initializing FileDB with path: {path}");
        if path.ends_with('/') {
            path = path.trim_end_matches('/').to_string();
        }
        let path_obj = Path::new(&path);
        if !path_obj.exists() {
            eprintln!("Path '{path}' does not exist... creating it");
            if let Err(e) = std::fs::create_dir(&path) {
                return Err(FormaError::Input(format!(
                    "could not create path '{path}': {e}"
                )));
            }
        }
        if !path_obj.is_dir() {
            return Err(FormaError::Input(format!(
                "  Error: Path '{path}' is not a directory"
            )));
        }
        Ok(Self {
            path: Arc::from(path_obj),
        })
    }

    fn sessions_dir(&self) -> PathBuf {
        (*self.path).join("sessions")
    }

    fn tokens_dir(&self) -> PathBuf {
        (*self.path).join("tokens")
    }

    fn deployments_dir(&self) -> PathBuf {
        (*self.path).join("deployments")
    }

    fn deployments_path(&self, deployment_id: &str) -> PathBuf {
        self.deployments_dir().join(format!("{deployment_id}.json"))
    }

    fn save_deployment(
        &self,
        deployment_id: &str,
        deployment: &AgentDeployment,
    ) -> Result<(), FormaError> {
        Self::ensure_dir(&self.deployments_dir())?;
        let path = self.deployments_path(deployment_id);
        let json = serde_json::to_string(deployment)
            .map_err(|e| FormaError::Execution(format!("failed to serialize agent: {e}")))?;
        std::fs::write(&path, json).map_err(|e| {
            FormaError::Execution(format!(
                "failed to write agent file '{}': {e}",
                path.display()
            ))
        })
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    fn token_path(&self, user_id: &str) -> PathBuf {
        let key = sanitize_filename(user_id);
        self.tokens_dir().join(format!("{key}.json"))
    }

    fn index_path(&self) -> PathBuf {
        (*self.path).join("index.json")
    }

    fn msg_index_path(&self) -> PathBuf {
        (*self.path).join("msg_index.json")
    }

    fn ensure_dir(dir: &Path) -> Result<(), FormaError> {
        std::fs::create_dir_all(dir).map_err(|e| {
            FormaError::Execution(format!(
                "failed to create directory '{}': {e}",
                dir.display()
            ))
        })
    }

    fn load_session(&self, session_id: &str) -> Result<SessionData, FormaError> {
        let path = self.session_path(session_id);
        let content = std::fs::read_to_string(&path)
            .map_err(|_| FormaError::SessionNotFound(session_id.to_string()))?;
        serde_json::from_str(&content).map_err(|e| {
            FormaError::Execution(format!("failed to parse session '{session_id}': {e}"))
        })
    }

    fn save_session(&self, session: &SessionData) -> Result<(), FormaError> {
        let session_id = session.id_string()?;
        Self::ensure_dir(&self.sessions_dir())?;
        let path = self.session_path(&session_id);
        let json = serde_json::to_string_pretty(session).map_err(|e| {
            FormaError::Execution(format!("failed to serialize session '{session_id}': {e}"))
        })?;
        std::fs::write(&path, json).map_err(|e| {
            FormaError::Execution(format!("failed to write session file '{session_id}': {e}"))
        })
    }

    fn load_index(&self) -> Result<HashMap<String, String>, FormaError> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| FormaError::Execution(format!("failed to read index: {e}")))?;
        serde_json::from_str(&content)
            .map_err(|e| FormaError::Execution(format!("failed to parse index: {e}")))
    }

    fn save_index(&self, index: &HashMap<String, String>) -> Result<(), FormaError> {
        let json = serde_json::to_string_pretty(index)
            .map_err(|e| FormaError::Execution(format!("failed to serialize index: {e}")))?;
        std::fs::write(self.index_path(), json)
            .map_err(|e| FormaError::Execution(format!("failed to write index: {e}")))
    }

    fn load_msg_index(&self) -> Result<HashMap<String, String>, FormaError> {
        let path = self.msg_index_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| FormaError::Execution(format!("failed to read msg_index: {e}")))?;
        serde_json::from_str(&content)
            .map_err(|e| FormaError::Execution(format!("failed to parse msg_index: {e}")))
    }

    fn save_msg_index(&self, index: &HashMap<String, String>) -> Result<(), FormaError> {
        let json = serde_json::to_string_pretty(index)
            .map_err(|e| FormaError::Execution(format!("failed to serialize msg_index: {e}")))?;
        std::fs::write(self.msg_index_path(), json)
            .map_err(|e| FormaError::Execution(format!("failed to write msg_index: {e}")))
    }

    // fn load_session_checking_owner(
    //     &self,
    //     session_id: &str,
    //     user_id: &str,
    // ) -> Result<SessionData, FormaError> {
    //     let session = self.load_session(session_id)?;
    //     if session.user_id != user_id {
    //         return Err(FormaError::SessionNotOwnedByUser(session_id.to_string()));
    //     }
    //     Ok(session)
    // }
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[async_trait::async_trait]
impl FormaDB for FileDB {
    fn persist(&self) -> bool {
        true
    }

    async fn get_agent_deployment(
        &self,
        deployment_id: &str,
    ) -> Result<AgentDeployment, FormaError> {
        let path = self.deployments_path(deployment_id);
        let content = std::fs::read_to_string(&path)
            .map_err(|_| FormaError::AgentDeploymentNotFound(deployment_id.to_string()))?;
        serde_json::from_str(&content).map_err(|e| {
            FormaError::Execution(format!("failed to parse agent '{deployment_id}': {e}"))
        })
    }

    async fn new_agent_deployment(
        &self,
        user_id: &str,
        agent: serde_json::Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        let agent_id = agent["id"]
            .as_str()
            .ok_or_else(|| FormaError::Input("agent missing 'id' field".to_string()))?
            .to_string();

        let now = chrono::Utc::now();
        let deployment_id = Uuid::new_v4().to_string();

        let owned = AgentDeployment {
            agent_id,
            owner_id: user_id.to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            agent,
            credentials,
        };

        self.save_deployment(&deployment_id, &owned)?;
        Ok(deployment_id)
    }

    async fn update_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
        agent: serde_json::Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        let mut deployment = self.get_agent_deployment(deployment_id).await?;
        // A deployment owned by someone else is treated as not found,
        // so its existence is not leaked to other users.
        if deployment.owner_id != user_id {
            return Err(FormaError::AgentDeploymentNotFound(
                deployment_id.to_string(),
            ));
        }

        // Keep the denormalized agent_id in sync with the new agent data.
        if let Some(agent_id) = agent["id"].as_str() {
            deployment.agent_id = agent_id.to_string();
        }
        deployment.agent = agent;
        if let Some(credentials) = credentials {
            deployment.replace_credentials(credentials);
        }
        deployment.version += 1;
        deployment.updated_at = chrono::Utc::now();

        self.save_deployment(deployment_id, &deployment)?;
        Ok(deployment_id.to_string())
    }

    async fn remove_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
    ) -> Result<String, FormaError> {
        // As in `update_agent_deployment`: someone else's is not found.
        let deployment = self.get_agent_deployment(deployment_id).await?;
        if deployment.owner_id != user_id {
            return Err(FormaError::AgentDeploymentNotFound(
                deployment_id.to_string(),
            ));
        }
        let path = self.deployments_path(deployment_id);
        std::fs::remove_file(&path)
            .map_err(|_| FormaError::AgentDeploymentNotFound(deployment_id.to_string()))?;
        Ok(deployment_id.to_string())
    }

    async fn new() -> Result<Self, FormaError> {
        tracing::info!("using FileDB backend");
        Ok(Self::new_with_path(DEFAULT_PATH.to_string())?)
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError> {
        let msg_index = self.load_msg_index()?;
        let session_id = msg_index
            .get(message_id)
            .ok_or_else(|| FormaError::MessageNotFound(message_id.to_string()))?;

        if session_id.is_empty() {
            return Err(FormaError::MessageNotFound(message_id.to_string()));
        }

        let session = self.load_session(session_id)?;
        session
            .messages
            .into_iter()
            .find(|m| m.id.as_ref().map(|id| id.0.as_str()) == Some(message_id))
            .ok_or_else(|| FormaError::MessageNotFound(message_id.to_string()))
    }

    async fn add_msg_reaction(
        &self,
        session_id: &str,
        message_id: &str,
        emoji: &Option<String>,
    ) -> Result<(), FormaError> {
        if let Some(emoji) = emoji {
            let mut session = self.load_session(session_id)?;
            let msg = session
                .messages
                .iter_mut()
                .find(|m| m.id.as_ref().map(|id| id.0.as_str()) == Some(message_id))
                .ok_or_else(|| FormaError::MessageNotFound(message_id.to_string()))?;
            msg.reactions.push(emoji.clone());
            self.save_session(&session)?;
        }
        Ok(())
    }

    async fn get_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        let index = self.load_index()?;
        let key = format!("{user_id}:{agent_id}");
        let session_id = index
            .get(&key)
            .ok_or_else(|| FormaError::SessionNotFound(format!("u-{user_id}:a-{agent_id}")))?;
        self.get_session_by_id(session_id, n_messages).await
    }

    async fn update_msg_status(
        &self,
        session_id: &str,
        message_id: &str,
        status: ChatMessageState,
    ) -> Result<(), FormaError> {
        let mut session = self.load_session(session_id)?;
        let msg = session
            .messages
            .iter_mut()
            .find(|m| m.id.as_ref().map(|id| id.0.as_str()) == Some(message_id))
            .ok_or_else(|| FormaError::MessageNotFound(message_id.to_string()))?;
        msg.status = status;
        self.save_session(&session)
    }

    async fn suspend_session(
        &self,
        session: Arc<RwLock<SessionData>>,
        call: &PendingToolCall,
    ) -> Result<(), FormaError> {
        let mut session = session.write().await;
        session.pending_tool_calls.push(call.clone());

        let session_id = session.id_string()?;
        let mut stored = self.load_session(&session_id)?;
        stored.pending_tool_calls = session.pending_tool_calls.clone();
        stored.cached_messages = session.cached_messages.clone();
        stored.tool_calls_results = session.tool_calls_results.clone();
        stored.variables = session.variables.clone();
        self.save_session(&stored)
    }

    async fn save_pending_tool_calls(&self, session: &SessionData) -> Result<(), FormaError> {
        let mut stored = self.load_session(&session.id_string()?)?;
        stored.pending_tool_calls = session.pending_tool_calls.clone();
        stored.tool_calls_results = session.tool_calls_results.clone();
        self.save_session(&stored)
    }

    async fn get_session_by_id(
        &self,
        session_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        let mut session = self.load_session(session_id)?;
        if n_messages > 0 && session.messages.len() > n_messages {
            let skip = session.messages.len() - n_messages;
            session.messages = session.messages.into_iter().skip(skip).collect();
        }
        Ok(session)
    }

    async fn start_run(
        &self,
        session_id: &str,
        stale_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, FormaError> {
        // Not atomic across processes, but a file database serves a single
        // local process, where two runs racing for one session is unlikely.
        let mut session = self.load_session(session_id)?;
        if session.running_since.is_some_and(|t| t >= stale_before) {
            return Ok(false);
        }
        session.running_since = Some(chrono::Utc::now());
        self.save_session(&session)?;
        Ok(true)
    }

    async fn finish_run(&self, session_id: &str) -> Result<(), FormaError> {
        let mut session = self.load_session(session_id)?;
        session.running_since = None;
        self.save_session(&session)
    }

    async fn clear_state(&self, session_id: &str) -> Result<(), FormaError> {
        let mut session = self.load_session(session_id)?;
        session.pending_tool_calls.clear();
        session.cached_messages.clear();
        session.tool_calls_results.clear();
        self.save_session(&session)
    }

    async fn __append_msg(&self, msg: FormaMessage) -> Result<FormaMessage, FormaError> {
        let id = msg.id.clone().expect("adding msg with no id").to_string();
        let session_id = &msg.session_id;

        if !msg.session_id.is_empty() {
            let session_path = self.session_path(session_id);
            if session_path.exists() {
                let mut session = self.load_session(session_id)?;
                session.messages.push(msg.clone());
                self.save_session(&session)?;
            }
        }

        let mut msg_index = self.load_msg_index()?;
        msg_index.insert(id, msg.session_id.clone());
        self.save_msg_index(&msg_index)?;

        Ok(msg)
    }

    fn init(&mut self) -> Result<(), FormaError> {
        let p: String = (*self.path)
            .to_str()
            .expect("could not turn path into string")
            .to_string();
        if !std::path::Path::new(&p).exists() {
            return Err(FormaError::Input(format!("path '{p}' does not exist")));
        }
        if !std::path::Path::new(&p).is_dir() {
            return Err(FormaError::Input(format!("path '{p}' is not a directory")));
        }
        Ok(())
    }

    async fn get_oauth_tokens(&self, user_id: &str) -> Result<EncryptedTokens, FormaError> {
        let path = self.token_path(user_id);
        if !path.exists() {
            return Ok(HashMap::new().into());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| FormaError::Execution(format!("failed to read token file: {e}")))?;
        let tokens: EncryptedTokens = serde_json::from_str(&content)
            .map_err(|e| FormaError::Execution(format!("failed to parse token file: {e}")))?;
        Ok(tokens)
    }

    async fn save_oauth_tokens(
        &self,
        user_id: &str,
        tokens: EncryptedTokens,
    ) -> Result<(), FormaError> {
        Self::ensure_dir(&self.tokens_dir())?;
        let path = self.token_path(user_id);
        let path_str = path.to_string_lossy();
        let json = serde_json::to_string(&tokens)
            .map_err(|e| FormaError::Execution(format!("failed to serialize tokens: {e}")))?;
        std::fs::write(&path, json).map_err(|e| {
            FormaError::Execution(format!("failed to write token file '{path_str}': {e}"))
        })?;
        eprintln!("Saved OAuth tokens in '{path_str}'.");
        Ok(())
    }

    async fn remove_oauth_tokens(&self, user_id: &str) -> Result<(), FormaError> {
        let path = self.token_path(user_id);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| FormaError::Execution(format!("failed to remove token file: {e}")))?;
        }
        Ok(())
    }

    async fn new_session(&self, agent_id: &str, user_id: &str) -> Result<SessionData, FormaError> {
        let session = SessionData {
            id: Some(SessionId(Uuid::new_v4().to_string())),
            user_id: user_id.to_string(),
            agent_id: agent_id.to_string(),
            created_at: Some(chrono::Utc::now()),
            ..Default::default()
        };
        self.save_session(&session)?;

        let mut index = self.load_index()?;
        index.insert(format!("{user_id}:{agent_id}"), session.id_string()?);
        self.save_index(&index)?;

        Ok(session)
    }

    async fn delete_session(&self, session_id: &str, user_id: &str) -> Result<(), FormaError> {
        let session = self.load_session(session_id)?;
        if session.user_id != user_id {
            return Err(FormaError::SessionNotFound(session_id.to_string()));
        }

        std::fs::remove_file(self.session_path(session_id))
            .map_err(|_| FormaError::SessionNotFound(session_id.to_string()))?;

        // The messages live inside the session file, so they are already gone;
        // what is left are the two maps that point at the session by id.
        let mut msg_index = self.load_msg_index()?;
        msg_index.retain(|_, sid| sid != session_id);
        self.save_msg_index(&msg_index)?;

        let mut index = self.load_index()?;
        index.retain(|_, sid| sid != session_id);
        self.save_index(&index)?;

        Ok(())
    }

    async fn rename_session(
        &self,
        session_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> Result<(), FormaError> {
        let mut session = self.load_session(session_id)?;
        if session.user_id != user_id {
            return Err(FormaError::SessionNotFound(session_id.to_string()));
        }
        session.name = name.map(str::to_string);
        self.save_session(&session)
    }

    async fn get_user_sessions(
        &self,
        agent_id: &str,
        user_id: &str,
        n_sessions: usize,
    ) -> Result<Vec<SessionSummary>, FormaError> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&dir).map_err(|e| {
            FormaError::Execution(format!("failed to read sessions directory: {e}"))
        })?;

        // No index covers "every session for this (user, agent) pair" — the
        // index.json map is one session_id per key — so this scans every
        // stored session. Fine for a local/dev-only backend.
        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                FormaError::Execution(format!("failed to read sessions directory entry: {e}"))
            })?;
            let Some(stem) = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str().map(str::to_string))
            else {
                continue;
            };
            let Ok(session) = self.load_session(&stem) else {
                continue; // skip anything that isn't a valid session file
            };
            // Web-channel /v1/init creates a session on every call, not just
            // when starting a real conversation, so plenty of stored sessions
            // never got a user message — not a real conversation to show.
            let has_user_message = session
                .messages
                .iter()
                .any(|m| matches!(m.content, protocols::FormaMessageBody::User { .. }));
            if session.user_id == user_id && session.agent_id == agent_id && has_user_message {
                sessions.push(session);
            }
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        sessions.truncate(n_sessions);

        Ok(sessions
            .into_iter()
            .filter_map(|session| {
                let id = session.id_string().ok()?;
                Some(SessionSummary {
                    display_name: session
                        .name
                        .clone()
                        .unwrap_or_else(|| session_display_name(&session.messages)),
                    created_at: session.created_at,
                    id,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> FileDB {
        let path = std::env::temp_dir().join(format!("forma-db-test-{}", Uuid::new_v4()));
        FileDB::new_with_path(path.to_string_lossy().to_string()).unwrap()
    }

    #[tokio::test]
    async fn a_session_takes_one_run_at_a_time() {
        let db = temp_db();
        let id = db
            .new_session("agent-a", "user-1")
            .await
            .unwrap()
            .id_string()
            .unwrap();
        let stale_before = chrono::Utc::now() - chrono::TimeDelta::hours(1);

        assert!(db.start_run(&id, stale_before).await.unwrap());
        assert!(
            !db.start_run(&id, stale_before).await.unwrap(),
            "a second run must not start while the first holds the session"
        );

        // A message stored mid-run must not drop the claim.
        db.push_msg(&id, "user-1", "hello".to_string())
            .await
            .unwrap();
        assert!(!db.start_run(&id, stale_before).await.unwrap());

        db.finish_run(&id).await.unwrap();
        assert!(db.start_run(&id, stale_before).await.unwrap());
    }

    #[tokio::test]
    async fn a_claim_older_than_the_cutoff_is_taken_over() {
        let db = temp_db();
        let id = db
            .new_session("agent-a", "user-1")
            .await
            .unwrap()
            .id_string()
            .unwrap();

        assert!(db.start_run(&id, chrono::Utc::now()).await.unwrap());
        // The run that holds it died without releasing it: once its claim is
        // older than the cutoff, the next run gets the session.
        let cutoff = chrono::Utc::now() + chrono::TimeDelta::seconds(1);
        assert!(db.start_run(&id, cutoff).await.unwrap());
    }

    #[tokio::test]
    async fn get_user_sessions_scopes_by_agent_and_user_and_sorts_most_recent_first() {
        let db = temp_db();

        let s1 = db.new_session("agent-a", "user-1").await.unwrap();
        db.push_msg(
            &s1.id_string().unwrap(),
            "user-1",
            "first question".to_string(),
        )
        .await
        .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let s2 = db.new_session("agent-a", "user-1").await.unwrap();
        db.push_msg(
            &s2.id_string().unwrap(),
            "user-1",
            "second question".to_string(),
        )
        .await
        .unwrap();

        // A session for a different user on the same agent — must not leak in.
        let other = db.new_session("agent-a", "user-2").await.unwrap();
        db.push_msg(
            &other.id_string().unwrap(),
            "user-2",
            "someone else's question".to_string(),
        )
        .await
        .unwrap();

        // A session for the same user on a different agent — must not leak in either.
        let other_agent = db.new_session("agent-b", "user-1").await.unwrap();
        db.push_msg(
            &other_agent.id_string().unwrap(),
            "user-1",
            "question for a different agent".to_string(),
        )
        .await
        .unwrap();

        let summaries = db.get_user_sessions("agent-a", "user-1", 10).await.unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].display_name, "second question");
        assert_eq!(summaries[1].display_name, "first question");

        std::fs::remove_dir_all(db.path.as_ref()).ok();
    }

    #[tokio::test]
    async fn delete_session_removes_it_and_refuses_someone_else_s() {
        let db = temp_db();

        let mine = db.new_session("agent-a", "user-1").await.unwrap();
        let mine_id = mine.id_string().unwrap();
        db.push_msg(&mine_id, "user-1", "my question".to_string())
            .await
            .unwrap();

        let theirs = db.new_session("agent-a", "user-2").await.unwrap();
        let theirs_id = theirs.id_string().unwrap();
        db.push_msg(&theirs_id, "user-2", "their question".to_string())
            .await
            .unwrap();

        // Someone else's session is not mine to delete, and stays put.
        assert!(matches!(
            db.delete_session(&theirs_id, "user-1").await,
            Err(FormaError::SessionNotFound(_))
        ));
        assert!(db.get_session_by_id(&theirs_id, 10).await.is_ok());

        db.delete_session(&mine_id, "user-1").await.unwrap();
        assert!(matches!(
            db.get_session_by_id(&mine_id, 10).await,
            Err(FormaError::SessionNotFound(_))
        ));
        assert!(
            db.get_user_sessions("agent-a", "user-1", 10)
                .await
                .unwrap()
                .is_empty()
        );

        // The deleted session was this user's current one on the agent, so
        // the next conversation starts fresh rather than resuming it.
        assert!(matches!(
            db.get_session_by_user_id("agent-a", "user-1", 10).await,
            Err(FormaError::SessionNotFound(_))
        ));

        std::fs::remove_dir_all(db.path.as_ref()).ok();
    }

    #[tokio::test]
    async fn a_renamed_session_keeps_its_name_until_it_is_cleared() {
        let db = temp_db();

        let mine = db.new_session("agent-a", "user-1").await.unwrap();
        let mine_id = mine.id_string().unwrap();
        db.push_msg(&mine_id, "user-1", "what the chat derives from".to_string())
            .await
            .unwrap();

        let derived = "what the chat derives from";
        let listed = |db: &FileDB| {
            let db = db.clone();
            async move { db.get_user_sessions("agent-a", "user-1", 10).await.unwrap() }
        };
        assert_eq!(listed(&db).await[0].display_name, derived);

        // Someone else's session is not mine to rename.
        assert!(matches!(
            db.rename_session(&mine_id, "user-2", Some("hijacked"))
                .await,
            Err(FormaError::SessionNotFound(_))
        ));
        assert_eq!(listed(&db).await[0].display_name, derived);

        db.rename_session(&mine_id, "user-1", Some("Budget talk"))
            .await
            .unwrap();
        assert_eq!(listed(&db).await[0].display_name, "Budget talk");

        // A name survives the conversation carrying on.
        db.push_msg(&mine_id, "user-1", "and another question".to_string())
            .await
            .unwrap();
        assert_eq!(listed(&db).await[0].display_name, "Budget talk");

        // Clearing it goes back to the derived name.
        db.rename_session(&mine_id, "user-1", None).await.unwrap();
        assert_eq!(listed(&db).await[0].display_name, derived);

        std::fs::remove_dir_all(db.path.as_ref()).ok();
    }

    #[tokio::test]
    async fn get_user_sessions_respects_n_sessions_cap() {
        let db = temp_db();
        for i in 0..3 {
            let s = db.new_session("agent-a", "user-1").await.unwrap();
            db.push_msg(&s.id_string().unwrap(), "user-1", format!("question {i}"))
                .await
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let summaries = db.get_user_sessions("agent-a", "user-1", 2).await.unwrap();
        assert_eq!(summaries.len(), 2);

        std::fs::remove_dir_all(db.path.as_ref()).ok();
    }
}
