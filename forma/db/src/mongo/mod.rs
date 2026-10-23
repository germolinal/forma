use chrono::DateTime as ChronoDateTime;
use futures::stream::TryStreamExt;
use mongodb::Client;
use mongodb::options::FindOptions;
use mongodb::{
    Collection,
    bson::to_bson,
    bson::{Bson, DateTime, Document, doc, oid::ObjectId},
};

use protocols::{
    AgentDeployment, ChatMessageState, DB_CONNECTION_STRING_ENV, DeploymentCredentials,
    FORMA_DATABASE_ID, FormaDB, FormaError, PendingToolCall, SessionData, SessionSummary,
    credentials::{EncryptedTokens, UserTokens},
    envar::get_env_var,
    message::{FormaMessage, GenericFormaMessage},
    session::GenericSessionData,
    session_display_name,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::instrument;

const SESSIONS_COLLECTION: &str = "sessions";
const MESSAGES_COLLECTION: &str = "messages";
const TOKENS_COLLECTION: &str = "oauth_tokens";
const DEPLOYED_AGENTS_COLLECTION: &str = "deployments";

type MongoMessage = GenericFormaMessage<DateTime>;

/// A deployment as stored: the compiled agent as a JSON string rather than as
/// nested documents. Firestore refuses anything nested more than 20 levels
/// deep, and a compiled agent passes that easily — an MCP tool keeps the
/// input schema of every tool its server offers, and a schema nests twice for
/// every level of the data it describes.
fn deployment_to_doc(deployment: &AgentDeployment) -> Result<Document, FormaError> {
    let agent =
        serde_json::to_string(&deployment.agent).map_err(|e| FormaError::Serde(e.to_string()))?;
    let mut doc =
        mongodb::bson::to_document(deployment).map_err(|e| FormaError::Serde(e.to_string()))?;
    doc.insert("agent", agent);
    Ok(doc)
}

/// The inverse of [`deployment_to_doc`]. Deployments saved before the agent was
/// stored as a string are read as they are, and written as a string the next
/// time they are redeployed.
fn deployment_from_doc(mut doc: Document) -> Result<AgentDeployment, FormaError> {
    doc.remove("_id");
    let agent: Value = match doc.insert("agent", Bson::Null) {
        Some(Bson::String(json)) => {
            serde_json::from_str(&json).map_err(|e| FormaError::Serde(e.to_string()))?
        }
        Some(nested) => {
            mongodb::bson::from_bson(nested).map_err(|e| FormaError::Serde(e.to_string()))?
        }
        None => Value::Null,
    };
    let mut deployment: AgentDeployment =
        mongodb::bson::from_document(doc).map_err(|e| FormaError::Serde(e.to_string()))?;
    deployment.agent = agent;
    Ok(deployment)
}
type MongoSessionData = GenericSessionData<ObjectId, DateTime>;

fn to_mongo_date(dt: ChronoDateTime<chrono::Utc>) -> DateTime {
    DateTime::from_millis(dt.timestamp_millis())
}
fn from_mongo_date(dt: DateTime) -> ChronoDateTime<chrono::Utc> {
    let milis = dt.timestamp_millis();
    ChronoDateTime::from_timestamp_millis(milis).expect("could not turn milis into datetime")
}

fn from_mongo_session(session: MongoSessionData) -> SessionData {
    let id = session.id.map(|id| id.to_string().into());
    let created_at = session.created_at.map(from_mongo_date);
    SessionData {
        id,
        created_at,
        pending_tool_calls: session.pending_tool_calls,
        running_since: session.running_since.map(from_mongo_date),
        user_id: session.user_id,
        agent_id: session.agent_id,
        channel_id: session.channel_id,
        name: session.name,
        messages: session.messages,
        variables: session.variables,
        cached_messages: session.cached_messages,
        tool_calls_results: session.tool_calls_results,
        // Never stored: only ever filled during a turn.
        produced_files: Vec::new(),
    }
}

fn from_forma_message(msg: FormaMessage) -> Result<MongoMessage, FormaError> {
    let created_at = msg.created_at.map(to_mongo_date);
    Ok(MongoMessage {
        id: msg.id,
        user_id: msg.user_id,
        session_id: msg.session_id,
        reactions: msg.reactions,
        status: msg.status,
        content: msg.content,
        created_at,
    })
}

fn from_mongo_message(msg: MongoMessage) -> Result<FormaMessage, FormaError> {
    let id = msg.id.map(|id| id.to_string().into());
    let created_at = msg.created_at.map(from_mongo_date);
    Ok(FormaMessage {
        id,
        user_id: msg.user_id,
        session_id: msg.session_id,
        reactions: msg.reactions,
        status: msg.status,
        content: msg.content,
        created_at,
    })
}

fn str_to_id(s: &str) -> Result<ObjectId, FormaError> {
    ObjectId::parse_str(s)
        .map_err(|x| x.to_string())
        .map_err(|x| FormaError::Input(format!("could not format MongoDB id: '{x}'")))
}

/// Exchanges data with a MongoDB-compatible database
#[derive(Clone)]
pub struct MongoDB {
    connection_string: Arc<str>,
    tokens_collection: Collection<UserTokens>,
    sessions_collection: Collection<MongoSessionData>,
    messages_collection: Collection<MongoMessage>,
    deployments_collection: Collection<Document>,
}

/// Leaves the connection string out: it holds the database's password, and
/// anything that holds the database (a call's context, a span recording its
/// arguments) may end up formatted into a log or a trace.
impl std::fmt::Debug for MongoDB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MongoDB").finish_non_exhaustive()
    }
}

impl MongoDB {
    /// Returns a MongoDB filter that matches the document for the given user
    fn by_user(user_id: &str) -> Document {
        doc! { "user_id": user_id }
    }

    /// Returns a MongoDB filter that matches the document for the given user and agent.
    fn by_user_and_agent(user_id: &str, agent_id: &str) -> Document {
        doc! { "user_id": user_id, "agent_id": agent_id }
    }

    async fn update_single_message(
        &self,
        session_id: &str,
        message_id: &str,
        update: Document,
    ) -> Result<(), FormaError> {
        let query = doc! {
            "id": message_id,
            "session_id": session_id
        };

        let result = self
            .messages_collection
            .update_one(query, update)
            .await
            .map_err(|e| FormaError::Execution(e.to_string()))?;

        // If the value remains the same, modified_count is Zero
        if result.matched_count == 0 {
            Err(FormaError::MessageNotFound(message_id.to_string()))
        } else if result.modified_count == 1 || result.modified_count == 0 {
            Ok(())
        } else {
            Err(FormaError::Execution(format!(
                "Unexpected modified count '{}' when updating session",
                result.modified_count
            )))
        }
    }

    /// Creates a new `MongoDBClient` with the specified URI.
    pub async fn with_uri_str(uri: &str) -> Result<Self, FormaError> {
        let client = Client::with_uri_str(uri).await.map_err(|x| x.to_string())?;
        let sessions_db = client.database(FORMA_DATABASE_ID);

        Ok(Self {
            connection_string: Arc::from(uri),
            tokens_collection: sessions_db.collection(TOKENS_COLLECTION),
            sessions_collection: sessions_db.collection(SESSIONS_COLLECTION),
            messages_collection: sessions_db.collection(MESSAGES_COLLECTION),
            deployments_collection: sessions_db.collection(DEPLOYED_AGENTS_COLLECTION),
        })
    }

    async fn get_native_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
    ) -> Result<Option<MongoSessionData>, FormaError> {
        let filter = Self::by_user_and_agent(user_id, agent_id);

        let session: Option<MongoSessionData> = self
            .sessions_collection
            .find_one(filter)
            .await
            .map_err(|e| FormaError::ApiCall {
                url: None,
                msg: e.to_string(),
            })?;
        Ok(session)
    }
    async fn get_native_message_by_id(&self, message_id: &str) -> Result<MongoMessage, FormaError> {
        let filter = doc! {
            "id": &message_id
        };

        let msg = self
            .messages_collection
            .find_one(filter)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?
            .ok_or_else(|| FormaError::MessageNotFound(message_id.to_string()))?;
        Ok(msg)
    }

    async fn get_native_session_by_id(
        &self,
        session_id: &str,
        n_messages: usize,
    ) -> Result<MongoSessionData, FormaError> {
        let id = str_to_id(session_id)?;

        let filter = doc! {
            "_id": &id
        };

        // The session and its messages are read at once: neither query
        // needs the other's answer, so waiting for one before sending the
        // other only added a round trip.
        let session = async {
            self.sessions_collection
                .find_one(filter)
                .await
                // error reaching out to DB
                .map_err(|x| FormaError::ApiCall {
                    url: None,
                    msg: x.to_string(),
                })?
                // Not found
                .ok_or_else(|| FormaError::SessionNotFound(session_id.to_string()))
        };

        let messages = async {
            if n_messages == 0 {
                return Ok(vec![]);
            }
            let msg_filter = doc! {
                "session_id": &session_id
            };
            // `_id` breaks ties: messages stored within the same millisecond
            // share a `created_at`, and the driver numbers documents in the
            // order it inserts them.
            let find_options = FindOptions::builder()
                .sort(doc! { "created_at": -1, "_id": -1 })
                .limit(n_messages as i64)
                .build();
            self.messages_collection
                .find(msg_filter)
                .with_options(find_options)
                .await
                .map_err(|x| FormaError::ApiCall {
                    url: None,
                    msg: x.to_string(),
                })?
                .try_collect::<Vec<MongoMessage>>()
                .await
                .map_err(|x| FormaError::ApiCall {
                    url: None,
                    msg: x.to_string(),
                })
        };

        let (mut session, mut messages) = tokio::try_join!(session, messages)?;
        if n_messages > 0 {
            messages.reverse();

            session.messages = Vec::with_capacity(messages.len());
            for m in messages {
                session.messages.push(from_mongo_message(m)?)
            }
        }
        Ok(session)
    }
}

#[async_trait::async_trait]
impl FormaDB for MongoDB {
    fn persist(&self) -> bool {
        true
    }

    async fn get_agent_deployment(&self, id: &str) -> Result<AgentDeployment, FormaError> {
        let id = str_to_id(id)?;
        let doc = self
            .deployments_collection
            .find_one(doc! { "_id": id })
            .await
            .map_err(|e| {
                tracing::error!("Failed to get agent deployment: {}", e);
                FormaError::Execution("could not get agent deployment".to_string())
            })?
            .ok_or_else(|| FormaError::AgentDeploymentNotFound(id.to_string()))?;

        deployment_from_doc(doc).map_err(|e| {
            tracing::error!("Failed to deserialize agent deployment: {}", e);
            FormaError::Serde("could not deserialize agent deployment".to_string())
        })
    }

    async fn new_agent_deployment(
        &self,
        user_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        let agent_id = agent["id"]
            .as_str()
            .ok_or_else(|| FormaError::Input("agent missing 'id' field".to_string()))?
            .to_string();

        let now = chrono::Utc::now();
        let owned = AgentDeployment {
            agent_id: agent_id.clone(),
            owner_id: user_id.to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
            agent,
            credentials,
        };
        let doc = deployment_to_doc(&owned)?;

        let inserted = self
            .deployments_collection
            .insert_one(&doc)
            .await
            .map_err(|e| {
                FormaError::Execution(format!("failed to save agent '{agent_id}': {e}"))
            })?;

        Ok(inserted
            .inserted_id
            .as_object_id()
            .expect("inserted id is not an ObjectId")
            .to_string())
    }

    async fn update_agent_deployment(
        &self,
        user_id: &str,
        deployment_id: &str,
        agent: Value,
        credentials: Option<DeploymentCredentials>,
    ) -> Result<String, FormaError> {
        let id = str_to_id(deployment_id)?;
        let existing = self
            .deployments_collection
            .find_one(doc! { "_id": id })
            .await
            .map_err(|e| FormaError::Execution(e.to_string()))?
            .ok_or_else(|| FormaError::AgentDeploymentNotFound(deployment_id.to_string()))?;

        let mut deployment = deployment_from_doc(existing)?;
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

        let doc = deployment_to_doc(&deployment)?;
        self.deployments_collection
            .replace_one(doc! { "_id": id }, &doc)
            .await
            .map_err(|e| {
                FormaError::Execution(format!(
                    "failed to update deployment '{deployment_id}': {e}"
                ))
            })?;

        Ok(deployment_id.to_string())
    }

    async fn remove_agent_deployment(&self, deployment_id: &str) -> Result<String, FormaError> {
        let id = str_to_id(deployment_id)?;
        let result = self
            .deployments_collection
            .delete_one(doc! { "_id": id })
            .await
            .map_err(|e| FormaError::Execution(e.to_string()))?;

        if result.deleted_count == 0 {
            return Err(FormaError::AgentDeploymentNotFound(
                deployment_id.to_string(),
            ));
        }
        Ok(deployment_id.to_string())
    }

    async fn new() -> Result<Self, FormaError> {
        tracing::info!("using MongoDB backend");
        eprint!("using MongoDB backend");
        let uri = protocols::envar::expect_env_var(DB_CONNECTION_STRING_ENV)?;
        Self::with_uri_str(&uri).await
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<FormaMessage, FormaError> {
        let m = self.get_native_message_by_id(message_id).await?;
        from_mongo_message(m)
    }

    async fn get_session_by_user_id(
        &self,
        agent_id: &str,
        user_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        let r = self
            .get_native_session_by_user_id(agent_id, user_id)
            .await?;
        if let Some(session) = r {
            if let Some(id) = session.id {
                let session_id = id.to_string();
                self.get_session_by_id(&session_id, n_messages).await
            } else {
                unreachable!("retrieved a session without ID in mongo?")
            }
        } else {
            Err(FormaError::SessionNotFound(format!(
                "u-{user_id}:a-{agent_id}"
            )))
        }
    }
    async fn update_msg_status(
        &self,
        session_id: &str,
        message_id: &str,
        status: ChatMessageState,
    ) -> Result<(), FormaError> {
        let state = to_bson(&status).map_err(|e| FormaError::Execution(e.to_string()))?;

        let update = doc! {"$set": {"status": state}};
        self.update_single_message(session_id, message_id, update)
            .await
    }

    #[instrument(skip(self, session))]
    async fn suspend_session(
        &self,
        session: Arc<RwLock<SessionData>>,
        call: &PendingToolCall,
    ) -> Result<(), FormaError> {
        let mut session = session.write().await;
        let call_bson = to_bson(&call).map_err(|x| FormaError::Execution(x.to_string()))?;
        session.pending_tool_calls.push(call.clone());

        let id = session
            .id
            .clone()
            .ok_or_else(|| FormaError::Input("cannot suspend session without id".to_string()))?;

        let findid = str_to_id(&id.0)?;
        let query = doc! {"_id": findid};

        let cached_messages =
            to_bson(&session.cached_messages).map_err(|x| FormaError::Execution(x.to_string()))?;
        let tool_calls_results = to_bson(&session.tool_calls_results)
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        let variables =
            to_bson(&session.variables).map_err(|x| FormaError::Execution(x.to_string()))?;
        let update = doc! {
            "$set": {
                "cached_messages": cached_messages,
                "tool_calls_results": tool_calls_results,
                "variables": variables,
            },
            "$push": {
                "pending_tool_calls": call_bson,
            }
        };

        let result = self
            .sessions_collection
            .update_one(query, update)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        if result.matched_count == 0 {
            Err(FormaError::SessionNotFound(id.to_string()))
        } else {
            Ok(())
        }
    }

    async fn get_session_by_id(
        &self,
        session_id: &str,
        n_messages: usize,
    ) -> Result<SessionData, FormaError> {
        let session = self
            .get_native_session_by_id(session_id, n_messages)
            .await?;

        Ok(from_mongo_session(session))
    }
    async fn add_msg_reaction(
        &self,
        session_id: &str,
        message_id: &str,
        reaction: &Option<String>,
    ) -> Result<(), FormaError> {
        if let Some(reaction) = reaction {
            let update = doc! {"$push": {"reactions": &reaction}};
            self.update_single_message(session_id, message_id, update)
                .await
        } else {
            unreachable!("reacting with empty emoji is not yet supported");
        }
    }

    fn init(&mut self) -> Result<(), FormaError> {
        if self.connection_string.is_empty() {
            if let Some(url) = get_env_var(DB_CONNECTION_STRING_ENV)? {
                self.connection_string = url.to_string().into();
            } else {
                return Err(FormaError::MissingEnvVar {
                    key: DB_CONNECTION_STRING_ENV.to_string(),
                    msg: "no value found".to_string(),
                });
            }
        }
        Ok(())
    }

    async fn start_run(
        &self,
        session_id: &str,
        stale_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, FormaError> {
        let id = str_to_id(session_id)?;
        // One conditional update, so the check and the claim cannot be split by
        // a second run claiming in between.
        let query = doc! {
            "_id": id,
            "$or": [
                { "running_since": null },
                { "running_since": { "$lt": to_mongo_date(stale_before) } },
            ],
        };
        let update = doc! { "$set": { "running_since": to_mongo_date(chrono::Utc::now()) } };
        let result = self
            .sessions_collection
            .update_one(query, update)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        Ok(result.matched_count == 1)
    }

    async fn finish_run(&self, session_id: &str) -> Result<(), FormaError> {
        let id = str_to_id(session_id)?;
        self.sessions_collection
            .update_one(
                doc! { "_id": id },
                doc! { "$unset": { "running_since": "" } },
            )
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        Ok(())
    }

    async fn save_pending_tool_calls(&self, session: &SessionData) -> Result<(), FormaError> {
        let session_id = session.id_string()?;
        let pending_tool_calls = to_bson(&session.pending_tool_calls)
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        let tool_calls_results = to_bson(&session.tool_calls_results)
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        let update = doc! {
            "$set": {
                "pending_tool_calls": pending_tool_calls,
                "tool_calls_results": tool_calls_results,
            }
        };
        let result = self
            .sessions_collection
            .update_one(doc! { "_id": str_to_id(&session_id)? }, update)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        if result.matched_count == 0 {
            Err(FormaError::SessionNotFound(session_id))
        } else {
            Ok(())
        }
    }

    async fn clear_state(&self, session_id: &str) -> Result<(), FormaError> {
        let id = str_to_id(session_id)?;
        let update = doc! {
            "$set": {
                "state": Document::new(),
                "tool_calls": Document::new(),
                "pending_tool_calls": mongodb::bson::Bson::Array(vec![]),
                "cached_messages": Document::new(),
                "tool_calls_results": Document::new(),
            }
        };
        let result = self
            .sessions_collection
            .update_one(doc! { "_id": id }, update)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;
        if result.matched_count == 0 {
            Err(FormaError::SessionNotFound(session_id.to_string()))
        } else {
            Ok(())
        }
    }

    async fn get_oauth_tokens(&self, user_id: &str) -> Result<EncryptedTokens, FormaError> {
        let query = Self::by_user(user_id);
        let result = self.tokens_collection.find_one(query).await.map_err(|e| {
            tracing::error!("failed to get tokens for user '{user_id}': {e}");
            FormaError::Execution(format!("failed to get tokens for user: {e}"))
        })?;

        Ok(result.map(|doc| doc.tokens).unwrap_or_default())
    }

    async fn save_oauth_tokens(
        &self,
        user_id: &str,
        tokens: EncryptedTokens,
    ) -> Result<(), FormaError> {
        let doc = UserTokens {
            user_id: user_id.to_string(),
            tokens,
        };

        let query = Self::by_user(user_id);
        self.tokens_collection
            .replace_one(query, &doc)
            .upsert(true)
            .await
            .map_err(|e| {
                FormaError::Execution(format!("failed to save tokens for user '{user_id}': {e}"))
            })?;
        Ok(())
    }

    async fn remove_oauth_tokens(&self, user_id: &str) -> Result<(), FormaError> {
        self.tokens_collection
            .delete_one(Self::by_user(user_id))
            .await
            .map_err(|e| {
                FormaError::Execution(format!("failed to remove tokens for user '{user_id}': {e}"))
            })?;
        Ok(())
    }

    async fn new_session(&self, agent_id: &str, user_id: &str) -> Result<SessionData, FormaError> {
        let mut session = MongoSessionData {
            user_id: user_id.to_string(),
            agent_id: agent_id.to_string(),
            created_at: Some(to_mongo_date(chrono::Utc::now())),
            ..Default::default()
        };
        let res = self
            .sessions_collection
            .insert_one(&session)
            .await
            .map_err(|x| FormaError::ApiCall {
                url: None,
                msg: x.to_string(),
            })?;

        session.id = res.inserted_id.as_object_id();
        Ok(from_mongo_session(session))
    }

    async fn __append_msg(&self, msg: FormaMessage) -> Result<FormaMessage, FormaError> {
        let input = from_forma_message(msg.clone())?;

        self.messages_collection
            .insert_one(input)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        Ok(msg)
    }

    async fn __append_msgs(
        &self,
        msgs: Vec<FormaMessage>,
    ) -> Result<Vec<FormaMessage>, FormaError> {
        if msgs.is_empty() {
            return Ok(msgs);
        }
        let input = msgs
            .iter()
            .cloned()
            .map(from_forma_message)
            .collect::<Result<Vec<_>, _>>()?;

        // Ordered (the default), so they are numbered, and read back, in the
        // order given.
        self.messages_collection
            .insert_many(input)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        Ok(msgs)
    }

    async fn delete_session(&self, session_id: &str, user_id: &str) -> Result<(), FormaError> {
        let id = str_to_id(session_id)?;
        // Matching on the owner as well means a session belonging to someone
        // else simply does not match, and is reported as not found.
        let result = self
            .sessions_collection
            .delete_one(doc! { "_id": id, "user_id": user_id })
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        if result.deleted_count == 0 {
            return Err(FormaError::SessionNotFound(session_id.to_string()));
        }

        // Messages are a separate collection, so dropping the session document
        // would otherwise leave the whole conversation behind.
        self.messages_collection
            .delete_many(doc! { "session_id": session_id })
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        Ok(())
    }

    async fn rename_session(
        &self,
        session_id: &str,
        user_id: &str,
        name: Option<&str>,
    ) -> Result<(), FormaError> {
        let id = str_to_id(session_id)?;
        // Clearing the name unsets the field rather than storing an empty
        // string, so a cleared session is stored exactly like one that was
        // never renamed, and derives its name again.
        let update = match name {
            Some(name) => doc! { "$set": { "name": name } },
            None => doc! { "$unset": { "name": "" } },
        };

        // Matching on the owner too means somebody else's session simply does
        // not match, and is reported as not found.
        let result = self
            .sessions_collection
            .update_one(doc! { "_id": id, "user_id": user_id }, update)
            .await
            .map_err(|x| FormaError::Execution(x.to_string()))?;

        if result.matched_count == 0 {
            return Err(FormaError::SessionNotFound(session_id.to_string()));
        }
        Ok(())
    }

    async fn get_user_sessions(
        &self,
        agent_id: &str,
        user_id: &str,
        n_sessions: usize,
    ) -> Result<Vec<SessionSummary>, FormaError> {
        let filter = Self::by_user_and_agent(user_id, agent_id);
        // The web channel's /v1/init creates a session on every call, not just
        // when starting a real conversation, so recent sessions can include
        // ones nobody ever sent a message in. Over-fetch a bounded candidate
        // window and filter those out below, rather than a hard n_sessions
        // limit that could under-fill the list.
        let candidate_limit = n_sessions.saturating_mul(3).clamp(20, 200);
        let find_options = FindOptions::builder()
            .sort(doc! { "created_at": -1 })
            .limit(candidate_limit as i64)
            .build();

        let sessions: Vec<MongoSessionData> = self
            .sessions_collection
            .find(filter)
            .with_options(find_options)
            .await
            .map_err(|x| FormaError::ApiCall {
                url: None,
                msg: x.to_string(),
            })?
            .try_collect()
            .await
            .map_err(|x| FormaError::ApiCall {
                url: None,
                msg: x.to_string(),
            })?;

        let mut summaries = Vec::with_capacity(n_sessions);
        for session in sessions {
            if summaries.len() >= n_sessions {
                break;
            }
            let Some(id) = session.id else { continue };
            let session_id = id.to_string();
            let created_at = session.created_at.map(from_mongo_date);

            // Only the first user message is needed for the display name, so
            // this is a targeted, indexed-by-nothing-but-bounded lookup rather
            // than pulling the whole conversation for every session listed.
            let msg_options = FindOptions::builder()
                .sort(doc! { "created_at": 1 })
                .limit(1)
                .build();
            let first_user_msg: Vec<MongoMessage> = self
                .messages_collection
                .find(doc! { "session_id": &session_id, "role": "user" })
                .with_options(msg_options)
                .await
                .map_err(|x| FormaError::ApiCall {
                    url: None,
                    msg: x.to_string(),
                })?
                .try_collect()
                .await
                .map_err(|x| FormaError::ApiCall {
                    url: None,
                    msg: x.to_string(),
                })?;
            let messages: Vec<FormaMessage> = first_user_msg
                .into_iter()
                .map(from_mongo_message)
                .collect::<Result<_, _>>()?;
            if messages.is_empty() {
                continue; // no user message yet — not a real conversation to show
            }

            summaries.push(SessionSummary {
                id: session_id,
                created_at,
                display_name: session
                    .name
                    .unwrap_or_else(|| session_display_name(&messages)),
            });
        }

        Ok(summaries)
    }
}

#[cfg(test)]
mod stored_deployment_tests {
    use super::*;

    /// An agent nested far deeper than Firestore allows: as an MCP tool's
    /// schemas make it.
    fn deep_agent() -> Value {
        let mut schema = serde_json::json!({ "type": "string" });
        for _ in 0..30 {
            schema = serde_json::json!({ "type": "object", "properties": { "x": schema } });
        }
        serde_json::json!({ "id": "an_agent", "start": { "nodes": [{ "schema": schema }] } })
    }

    fn deployment(agent: Value) -> AgentDeployment {
        AgentDeployment {
            agent_id: "an_agent".to_string(),
            owner_id: "owner".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 3,
            agent,
            credentials: None,
        }
    }

    #[test]
    fn the_agent_is_stored_flat_and_read_back_whole() {
        let doc = deployment_to_doc(&deployment(deep_agent())).unwrap();
        assert!(matches!(doc.get("agent"), Some(Bson::String(_))));

        let read = deployment_from_doc(doc).unwrap();
        assert_eq!(read.agent, deep_agent());
        assert_eq!(read.version, 3);
    }

    /// Saved before the agent was a string, and not redeployed since.
    #[test]
    fn a_nested_agent_is_still_read() {
        let agent = serde_json::json!({ "id": "an_agent", "start": { "nodes": [{ "id": "n", "max_iterations": 2 }] } });
        let mut doc = mongodb::bson::to_document(&deployment(agent.clone())).unwrap();
        doc.insert("_id", ObjectId::new());

        assert_eq!(deployment_from_doc(doc).unwrap().agent, agent);
    }
}

#[cfg(test)]
mod test {

    use std::time::Duration;

    use mongodb::bson::Uuid;
    use protocols::{
        AssistantMessage, CachedToolResult, LLMMessage, OpenAIMessagePart, OpenAITextPart,
        PendingToolCall,
    };
    use tokio::{sync::RwLock, time::sleep};

    use super::*;

    #[tokio::test]
    async fn db_sessions_checks() {
        let agent_id = "test-agent";
        let user_id = "the-user";
        let db = MongoDB::new().await.unwrap();
        // Check new
        let session = db.new_session(&agent_id, user_id).await.unwrap();
        let id = session.id.clone().unwrap().0;
        println!("ID = {id}");
        assert!(session.id.is_some());
        assert_eq!(session.user_id, user_id);
        assert_eq!(session.agent_id, agent_id);
        // Check update
        let content = "hellooooossss.".to_string();
        let msg: FormaMessage = content.into();
        let user_id = "some-user";

        db.push_msg(&id, user_id, msg).await.unwrap();

        // Check find
        let _found_session = db.get_session_by_id(&id, 1).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_takes_the_messages_with_it() {
        let agent_id = "test-agent-delete";
        let db = MongoDB::new().await.unwrap();

        let session = db.new_session(agent_id, "owner-uid").await.unwrap();
        let id = session.id.clone().unwrap().0;
        let msg = db
            .push_msg(&id, "owner-uid", "please forget this".to_string())
            .await
            .unwrap();
        let msg_id = msg.id.clone().unwrap().0;

        // Someone else's delete matches nothing, and leaves the session alone.
        assert!(matches!(
            db.delete_session(&id, "someone-else").await,
            Err(FormaError::SessionNotFound(_))
        ));
        assert!(db.get_session_by_id(&id, 10).await.is_ok());

        db.delete_session(&id, "owner-uid").await.unwrap();
        assert!(matches!(
            db.get_session_by_id(&id, 10).await,
            Err(FormaError::SessionNotFound(_))
        ));
        // The conversation lives in a collection of its own, so dropping the
        // session document alone would leave every message behind.
        assert!(matches!(
            db.get_message_by_id(&msg_id).await,
            Err(FormaError::MessageNotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_renamed_session_keeps_its_name_until_it_is_cleared() {
        let agent_id = "test-agent-rename";
        let db = MongoDB::new().await.unwrap();

        let session = db.new_session(agent_id, "owner-uid").await.unwrap();
        let id = session.id.clone().unwrap().0;
        db.push_msg(&id, "owner-uid", "the derived one".to_string())
            .await
            .unwrap();

        let listed = async || {
            db.get_user_sessions(agent_id, "owner-uid", 10)
                .await
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .expect("the session should be listed")
                .display_name
        };
        assert_eq!(listed().await, "the derived one");

        // Someone else's session is not theirs to rename.
        assert!(matches!(
            db.rename_session(&id, "someone-else", Some("hijacked"))
                .await,
            Err(FormaError::SessionNotFound(_))
        ));
        assert_eq!(listed().await, "the derived one");

        db.rename_session(&id, "owner-uid", Some("Budget talk"))
            .await
            .unwrap();
        assert_eq!(listed().await, "Budget talk");

        // Clearing it unsets the field, so the name is derived again.
        db.rename_session(&id, "owner-uid", None).await.unwrap();
        assert_eq!(listed().await, "the derived one");

        db.delete_session(&id, "owner-uid").await.unwrap();
    }

    #[tokio::test]
    async fn db_sessions_respects_context_window() {
        let agent_id = "test-agent-slice";
        let user_id = "test-user";
        let db = MongoDB::new().await.unwrap();
        // Create new session
        let session = db.new_session(agent_id, user_id).await.unwrap();

        let id = session.id.clone().unwrap().0;

        // Insert more messages than the window
        let total_messages = 5;
        let user_id = "some-user";
        for i in 0..total_messages {
            let content = format!("message-{i}");
            let msg: LLMMessage = content.into();
            db.push_msg(&id, user_id, msg).await.unwrap();
        }

        // Load session with a capped window
        let window = 2;
        let capped_session = db.get_session_by_id(&id, window).await.unwrap();

        // Assert only last N messages are returned
        assert_eq!(capped_session.messages.len(), window);

        // Assert they are the *last* ones
        let contents: Vec<String> = capped_session
            .messages
            .iter()
            .map(|m| m.to_string())
            .collect();

        assert_eq!(
            contents,
            vec!["message-3\n".to_string(), "message-4\n".to_string(),]
        );
    }

    #[tokio::test]
    async fn db_session_find_or_create() {
        let agent_id = "test-agent-find-or-create";
        let user_id = "the-user-find-or-create";
        let db = MongoDB::new().await.unwrap();

        // 1. Session does not exist, should be created.
        let session1 = db
            .get_or_create_session(agent_id, user_id, 1)
            .await
            .unwrap();

        assert!(session1.id.is_some());
        assert_eq!(session1.agent_id, agent_id);
        assert_eq!(session1.user_id, user_id);

        // 2. Session exists, should be found.
        let session2 = db
            .get_or_create_session(agent_id, user_id, 1)
            .await
            .unwrap();

        // Make sure it is the same session
        assert_eq!(session1.id, session2.id);
        assert_eq!(session2.agent_id, agent_id);
        assert_eq!(session2.user_id, user_id);
    }

    #[test]
    fn test_suspended_defaults_to_false() {
        // When the `suspended` field is absent (e.g. older DB documents), it must deserialize as false.
        let json = serde_json::json!({
            "user_id": "u1",
            "agent_id": "a1",
            "state": {},
            "tool_calls_results": {},
        });
        let session: SessionData = serde_json::from_value(json).unwrap();
        assert!(
            session.pending_tool_calls.is_empty(),
            "no pending tool calls should be present when suspended is absent"
        );
    }

    #[test]
    fn test_str_to_id() {
        assert!(str_to_id("698d9ee801bb63e92e9ed57b").is_ok());
        assert!(str_to_id("non-hex-id").is_err());
    }

    #[test]
    fn test_id_string() {
        let d = SessionData {
            id: None,
            ..Default::default()
        };
        assert!(d.id_string().is_err());

        let d = SessionData {
            id: Some("the-id".to_string().into()),
            ..Default::default()
        };
        assert!(d.id_string().is_ok());
    }

    #[tokio::test]
    async fn test_push_msg() {
        let session_id = "the-session";
        let user_id = "the-user";
        let db = MongoDB::new().await.unwrap();
        let msg = FormaMessage {
            ..Default::default()
        };
        let msg = db.push_msg(session_id, user_id, msg).await.unwrap();
        assert!(msg.id.is_some());
        assert_eq!(&msg.user_id, user_id);
        assert_eq!(&msg.session_id, session_id);
    }

    #[tokio::test]
    async fn test_react() {
        let agent_id = "test-agent-react-to-msg";
        let db = MongoDB::new().await.unwrap();

        let message_id = "698d9ee801bb63e92e9ed57b";
        let session_id = "698d9ee801bb63e92e9ed57b";

        // Session does not exist.
        let reaction = Some("❤️".to_string());
        let an_err = db
            .add_msg_reaction(session_id, &message_id, &reaction)
            .await;
        assert!(an_err.is_err());

        let user_id = "the-user-react";
        let session = db.new_session(agent_id, user_id).await.unwrap();

        sleep(Duration::from_millis(900)).await;
        let session_id = session.id_string().unwrap();

        let an_err = db
            .add_msg_reaction(&session_id, &message_id, &reaction)
            .await;
        assert!(an_err.is_err());

        // Succeed?
        let msg = FormaMessage {
            ..Default::default()
        };
        let user_id = "some-user";
        let msg = db.push_msg(&session_id, user_id, msg).await.unwrap();
        let message_id = msg.id.unwrap().to_string();

        let not_err = db
            .add_msg_reaction(&session_id, &message_id, &reaction)
            .await;
        assert!(not_err.is_ok());
    }

    #[tokio::test]
    async fn test_status() {
        let agent_id = "test-agent-msg-status";
        let user_id = "test-user-msg-status";
        let db = MongoDB::new().await.unwrap();

        let message_id = Uuid::new().to_string();
        let session = db.new_session(agent_id, user_id).await.unwrap();
        let session_id = &session.id.unwrap().to_string();
        // Session does not exist.
        let an_err = db
            .update_msg_status(&session_id, &message_id, ChatMessageState::Read)
            .await;
        assert!(an_err.is_err());

        let user_id = "the-user-find-or-create";
        let session = db.new_session(agent_id, user_id).await.unwrap();
        sleep(Duration::from_millis(900)).await;
        let session_id = session.id_string().unwrap();

        assert!(an_err.is_err());

        // Succeed?
        let msg = FormaMessage {
            ..Default::default()
        };
        let user_id = "some-user";
        let msg = db.push_msg(&session_id, user_id, msg).await.unwrap();
        let message_id = msg.id.unwrap().to_string();

        let not_err = db
            .update_msg_status(&session_id, &message_id, ChatMessageState::Delivered)
            .await;
        assert!(not_err.is_ok());
    }

    #[tokio::test]
    async fn test_get_message() {
        let db = MongoDB::new().await.unwrap();

        assert!(db.get_message_by_id("inexistent-msg").await.is_err());

        // No session
        let session_id = "tha-session";
        let user_id = "tha-user";
        let msg = FormaMessage::default();
        let msg = db.push_msg(session_id, user_id, msg).await.unwrap();
        let id = msg.id_string().unwrap();
        assert!(db.get_message_by_id(&id).await.is_ok());
    }

    #[tokio::test]
    async fn a_session_takes_one_run_at_a_time() {
        let db = MongoDB::new().await.unwrap();
        let id = db
            .new_session("test-agent-claim", "test-user-claim")
            .await
            .unwrap()
            .id_string()
            .unwrap();
        let stale_before = chrono::Utc::now() - chrono::TimeDelta::hours(1);

        // Two runs racing for the session: exactly one gets it.
        let (a, b) = tokio::join!(
            db.start_run(&id, stale_before),
            db.start_run(&id, stale_before)
        );
        assert!(
            a.unwrap() ^ b.unwrap(),
            "exactly one run must get the session"
        );

        let session = db.get_session_by_id(&id, 0).await.unwrap();
        assert!(session.running_since.is_some());

        db.finish_run(&id).await.unwrap();
        let session = db.get_session_by_id(&id, 0).await.unwrap();
        assert!(session.running_since.is_none());
        assert!(db.start_run(&id, stale_before).await.unwrap());

        // A claim older than the cutoff belongs to a run that died holding it.
        let cutoff = chrono::Utc::now() + chrono::TimeDelta::seconds(1);
        assert!(db.start_run(&id, cutoff).await.unwrap());
    }

    #[tokio::test]
    async fn test_suspend() {
        let agent_id = "test-agent-suspend";
        let user_id = "test-user-suspend";
        let db = MongoDB::new().await.unwrap();

        // Suspending without an ID should fail
        let no_id_session = Arc::new(RwLock::new(SessionData::default()));
        let call = PendingToolCall::default();
        assert!(db.suspend_session(no_id_session, &call).await.is_err());

        // Create a real session in the DB
        let mut session_data = db.new_session(agent_id, user_id).await.unwrap();

        let msg = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "Hi!".to_string(),
                ..Default::default()
            })],
            ..Default::default()
        };
        session_data.cached_messages.insert("key1".to_string(), msg);
        let tool_name = "tool_1".to_string();
        let tool_call_id = "call123".to_string();
        let result = serde_json::json!({"result": "done"});
        session_data.tool_calls_results.insert(
            tool_call_id.clone(),
            CachedToolResult {
                tool_name,
                tool_call_id: tool_call_id.clone(),
                result,
                from_root: true,
            },
        );

        let session_id = session_data.id_string().unwrap();
        let session = Arc::new(RwLock::new(session_data));
        let call = PendingToolCall::default();
        db.suspend_session(Arc::clone(&session), &call)
            .await
            .unwrap();

        // In-memory flag must be set immediately after suspend
        assert!(
            !session.read().await.pending_tool_calls.is_empty(),
            "pending tool calls should be present in-memory after suspend"
        );

        // Reload from DB and verify suspended flag and state were persisted
        let mut loaded = db.get_session_by_id(&session_id, 1).await.unwrap();

        assert_eq!(
            loaded.pending_tool_calls.len(),
            1,
            "one pending tool call should be present in the database after suspend"
        );
        let msg = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "Hi!".to_string(),
                ..Default::default()
            })],
            ..Default::default()
        };
        loaded.cached_messages.insert("key1".to_string(), msg);
        assert_eq!(loaded.cached_messages.len(), 1);
        let cached_result = loaded
            .tool_calls_results
            .get(&tool_call_id)
            .expect("expecting_some");
        assert_eq!(
            &cached_result.result,
            &serde_json::json!({"result": "done"}),
            "result should be stored in tool_calls_results"
        );
    }

    // async fn setup_suspended_session(
    //     agent_id: &str,
    //     user_id: &str,
    // ) -> (
    //     MongoDB,
    //     String,
    //     String, // tool_call_id
    // ) {
    //     let db = MongoDB::new().await.unwrap();

    //     let session_data = db.new_session(agent_id, user_id).await.unwrap();
    //     let session_id = session_data.id_string().unwrap();

    //     let call = PendingToolCall {
    //         tool_call_id: "tool-call-1".to_string(),
    //         tool_name: "some_tool".to_string(),
    //         arguments: serde_json::json!({"x": 1}),
    //         ..Default::default()
    //     };
    //     let tool_call_id = call.tool_call_id.clone();
    //     let session = Arc::new(RwLock::new(session_data));
    //     db.suspend_session(Arc::clone(&session), &call)
    //         .await
    //         .unwrap();

    //     (db, session_id, tool_call_id)
    // }

    // #[tokio::test]
    // async fn test_process_resume_session_not_found() {
    //     let db = MongoDB::new().await.unwrap();

    //     // Valid ObjectId format, but no such document in the DB
    //     let req = ResumeRequest {
    //         tool_call_id: "any-call-id".to_string(),
    //         reject: false,
    //         arguments: Some(serde_json::json!({})),
    //         result: None,
    //     };
    //     let err = db
    //         .handle_resume("000000000000000000000000", "any-user", &req)
    //         .await
    //         .unwrap_err();

    //     assert!(
    //         matches!(err, FormaError::SessionNotFound(_)),
    //         "expected SessionNotFound, got {err:?}"
    //     );
    // }

    // #[tokio::test]
    // async fn test_process_resume_not_owned() {
    //     let agent_id = "test-resume-not-owned";
    //     let user_id = "owner-user";

    //     let (db, session_id, tool_call_id) = setup_suspended_session(agent_id, user_id).await;

    //     let req = ResumeRequest {
    //         tool_call_id: tool_call_id,
    //         reject: false,
    //         arguments: Some(serde_json::json!({})),
    //         result: None,
    //     };
    //     let err = db
    //         .handle_resume(&session_id, "different-user", &req)
    //         .await
    //         .unwrap_err();

    //     assert!(
    //         matches!(err, FormaError::SessionNotOwnedByUser(_)),
    //         "expected SessionNotOwnedByUser, got {err:?}"
    //     );
    // }

    // #[tokio::test]
    // async fn test_process_resume_unknown_tool_call_id() {
    //     let agent_id = "test-resume-unknown-call";
    //     let user_id = "test-user-unknown-call";
    //     let (db, session_id, _) = setup_suspended_session(agent_id, user_id).await;

    //     let req = ResumeRequest {
    //         tool_call_id: "nonexistent-call-id".to_string(),
    //         reject: false,
    //         arguments: Some(serde_json::json!({})),
    //         result: None,
    //     };
    //     let err = db
    //         .handle_resume(&session_id, user_id, &req)
    //         .await
    //         .unwrap_err();

    //     assert!(
    //         matches!(err, FormaError::Input(_)),
    //         "expected Input error for unknown tool_call_id, got {err:?}"
    //     );
    // }

    // #[tokio::test]
    // async fn test_accept_tool_call_success() {
    //     let agent_id = "test-accept-tool-call";
    //     let user_id = "test-user-accept";
    //     let (db, session_id, tool_call_id) = setup_suspended_session(agent_id, user_id).await;

    //     let arguments = serde_json::json!({"x": 42});
    //     let req = ResumeRequest {
    //         tool_call_id: tool_call_id.clone(),
    //         reject: false,
    //         arguments: Some(arguments.clone()),
    //         result: None,
    //     };
    //     let still_pending = db.handle_resume(&session_id, user_id, &req).await.unwrap();

    //     assert!(
    //         !still_pending,
    //         "no pending calls should remain after accepting the only one"
    //     );

    //     // Reload and verify: pending has the updated call with approved=true
    //     let loaded = db.get_session_by_id(&session_id, 10).await.unwrap();

    //     assert!(!loaded.pending_tool_calls.is_empty());
    //     let call = loaded
    //         .pending_tool_calls
    //         .iter()
    //         .find(|c| c.tool_call_id == tool_call_id)
    //         .expect("tool call should still be in pending list");
    //     assert!(call.approved);
    //     assert_eq!(call.arguments, arguments);

    //     // Tool calls should NOT be populated yet (Node does that)
    //     assert!(loaded.tool_calls_results.get(&tool_call_id).is_none());
    // }

    // #[tokio::test]
    // async fn test_accept_tool_call_result_success() {
    //     let agent_id = "test-accept-tool-call-result";
    //     let user_id = "test-user-accept-result";
    //     let (db, session_id, tool_call_id) = setup_suspended_session(agent_id, user_id).await;

    //     let result = serde_json::json!({"output": "42"});
    //     let req = ResumeRequest {
    //         tool_call_id: tool_call_id.clone(),
    //         reject: false,
    //         arguments: None,
    //         result: Some(result.clone()),
    //     };
    //     let still_pending = db.handle_resume(&session_id, user_id, &req).await.unwrap();

    //     assert!(
    //         !still_pending,
    //         "no pending calls should remain after providing a result for the only one"
    //     );

    //     let loaded = db.get_session_by_id(&session_id, 10).await.unwrap();

    //     assert!(!loaded.pending_tool_calls.is_empty());
    //     let call = loaded
    //         .pending_tool_calls
    //         .iter()
    //         .find(|c| c.tool_call_id == tool_call_id)
    //         .expect("tool call should still be in pending list");
    //     assert!(call.approved, "pending tool call should be marked approved");
    //     let cached_result = loaded.tool_calls_results.get(&tool_call_id);
    //     let cached_result = cached_result.expect("expecting Some(result)");
    //     assert_eq!(
    //         &cached_result.result, &result,
    //         "result should be stored in tool_calls_results"
    //     );
    //     assert_eq!(
    //         cached_result.tool_call_id, tool_call_id,
    //         "tool_call_id should be stored in cached result"
    //     );
    //     assert_eq!(
    //         cached_result.tool_name, "some_tool",
    //         "tool_name should be stored in cached result"
    //     );
    // }

    // #[tokio::test]
    // async fn test_process_resume_both_fields_fails() {
    //     let agent_id = "test-resume-both-fail";
    //     let user_id = "test-user-both-fail";
    //     let (db, session_id, tool_call_id) = setup_suspended_session(agent_id, user_id).await;

    //     let req = ResumeRequest {
    //         tool_call_id: tool_call_id.clone(),
    //         reject: false,
    //         arguments: Some(serde_json::json!({"x": 1})),
    //         result: Some(serde_json::json!({"output": "42"})),
    //     };
    //     let err = db
    //         .handle_resume(&session_id, user_id, &req)
    //         .await
    //         .unwrap_err();

    //     assert!(
    //         matches!(err, FormaError::Input(_)),
    //         "expected Input error when both arguments and result are provided, got {err:?}"
    //     );
    // }

    // #[tokio::test]
    // async fn test_reject_tool_call_success() {
    //     let agent_id = "test-reject-tool-call";
    //     let user_id = "test-user-reject";
    //     let (db, session_id, tool_call_id) = setup_suspended_session(agent_id, user_id).await;

    //     let req = ResumeRequest {
    //         tool_call_id: tool_call_id.clone(),
    //         reject: true,
    //         arguments: None,
    //         result: None,
    //     };
    //     let still_pending = db.handle_resume(&session_id, user_id, &req).await.unwrap();

    //     assert!(
    //         !still_pending,
    //         "no pending calls should remain after rejecting the only one"
    //     );

    //     // Reload and verify: pending cleared, nothing added to tool_calls
    //     let loaded = db.get_session_by_id(&session_id, 10).await.unwrap();

    //     assert!(loaded.pending_tool_calls.is_empty());
    //     assert!(loaded.tool_calls_results.get(&tool_call_id).is_none());
    // }

    // #[tokio::test]
    // async fn test_process_resume_still_pending() {
    //     let agent_id = "test-resume-still-pending";
    //     let user_id = "test-user-still-pending";
    //     let db = MongoDB::new().await.unwrap();

    //     let session_data = db.new_session(agent_id, user_id).await.unwrap();
    //     let session_id = session_data.id_string().unwrap();
    //     let session = Arc::new(RwLock::new(session_data));

    //     // Suspend twice with two different calls
    //     let call1 = PendingToolCall {
    //         tool_call_id: "call-id-1".to_string(),
    //         ..Default::default()
    //     };
    //     let call2 = PendingToolCall {
    //         tool_call_id: "call-id-2".to_string(),
    //         ..Default::default()
    //     };
    //     db.suspend_session(Arc::clone(&session), &call1)
    //         .await
    //         .unwrap();
    //     db.suspend_session(Arc::clone(&session), &call2)
    //         .await
    //         .unwrap();

    //     // Accept the first — second should still be pending
    //     let req = ResumeRequest {
    //         tool_call_id: "call-id-1".to_string(),
    //         reject: false,
    //         arguments: Some(serde_json::json!({})),
    //         result: None,
    //     };
    //     let still_pending = db.handle_resume(&session_id, user_id, &req).await.unwrap();

    //     assert!(
    //         still_pending,
    //         "one pending call should remain after accepting the first of two"
    //     );
    // }
}
