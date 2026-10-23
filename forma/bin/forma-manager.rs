use agent::Callable;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use db::FormaDBClient;
use forma::{
    api::{
        AgentDefinition, AgentFeatures, McpToolDefinition, ObjectDefinition,
        generative::generate_agent,
    },
    observability::{Telemeter, TelemetryMode},
    server::expect_user_id,
};
use protocols::{
    EncryptedTokens, FormaDB, OauthToken, SessionData, SessionSummary, normalize_session_name,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tracing::{error, info};

#[derive(Debug, Deserialize)]
struct SaveTokenRequest {
    user_id: String,
    /// Key for the tokens map — the MCP server URL or provider identifier.
    mcp_url: String,
    token: OauthToken,
}

type AppState = Arc<FormaDBClient>;

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn save_token(
    State(db): State<AppState>,
    Json(req): Json<SaveTokenRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    tracing::info!("saving token for user {}", req.user_id);
    // Read existing tokens, merge the new one in, write back.
    let mut tokens = db.get_oauth_tokens(&req.user_id).await.unwrap_or_default();

    tokens.insert(req.mcp_url, req.token);

    db.save_oauth_tokens(&req.user_id, tokens).await?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
struct RemoveTokenRequest {
    user_id: String,
    /// Key to remove from the tokens map — the MCP server URL or provider identifier.
    mcp_url: String,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn remove_token(
    State(db): State<AppState>,
    Json(req): Json<RemoveTokenRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    tracing::info!("removing token for user {}", req.user_id);
    let mut tokens = db.get_oauth_tokens(&req.user_id).await.unwrap_or_default();

    tokens.remove(&req.mcp_url);

    db.save_oauth_tokens(&req.user_id, tokens).await?;

    Ok(StatusCode::OK)
}

/// What `POST /agents/check` reports about an agent that compiled and passed
/// its checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckAgentResponse {
    /// The MCP servers the author configured authentication for, as written in
    /// the definition — what a caller has to be connected to before the agent
    /// can be compiled at all.
    pub mcp_definitions: Vec<McpToolDefinition>,
    /// What the *compiled* agent turned out to use. Read from the finished
    /// agent rather than its definition, so it covers tools reached through
    /// nested workflows and servers that only name themselves on connection.
    pub features: AgentFeatures,
}

/// How `/agents/check` treats the agent's MCP servers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum McpCheck {
    /// Connect to each with the caller's tokens, as running the agent would.
    /// What a deploy needs.
    #[default]
    Connect,
    /// Connect to none (see `McpAccess::Unconnected`): everything else is
    /// checked exactly as it would be, by a caller who has not connected those
    /// servers yet. What saving a blueprint needs.
    Unconnected,
}

#[derive(Debug, Default, Deserialize)]
struct CheckAgentQuery {
    #[serde(default)]
    mcp: McpCheck,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn check_agent(
    headers: HeaderMap,
    State(db): State<AppState>,
    Query(query): Query<CheckAgentQuery>,
    Json(def): Json<AgentDefinition>,
) -> Result<Json<CheckAgentResponse>, (StatusCode, String)> {
    info!("check_agent: checking agent");
    // Everything the files themselves can be faulted for — bad ids, tools
    // fighting over a name — is caught here, before a single MCP server is
    // connected to on the way to compiling the agent.
    let def = def.check().map_err(|e| {
        info!("check_agent: malformed definition: error={e:?}");
        (StatusCode::BAD_REQUEST, e.to_string())
    })?;
    let mcp_definitions = def.mcp_definitions();
    let user_id = expect_user_id(&headers)?;
    info!("check_agent: user_id={user_id} mcp={:?}", query.mcp);
    let compiled = match query.mcp {
        McpCheck::Connect => {
            let auth = db.get_oauth_tokens(&user_id).await?;
            info!("check_agent: fetched auth");
            def.into_agent(&auth).await
        }
        McpCheck::Unconnected => def.into_agent_unconnected().await,
    };
    let agent = compiled.map_err(|e| {
        info!("check_agent: malformed agent: error={e:?}");
        (StatusCode::BAD_REQUEST, e.to_string())
    })?;
    <agent::Agent as Callable<FormaDBClient>>::check(&agent).map_err(|e| {
        info!("check_agent: agent did not pass checks: error={e:?}");
        (StatusCode::BAD_REQUEST, e.to_string())
    })?;
    let features = AgentFeatures::from(&agent);
    info!(
        mcp_servers = features.mcp_servers.len(),
        scrapbook = features.scrapbook,
        workspaces = features.workspaces.len(),
        "check_agent: agent passed checks"
    );
    Ok(Json(CheckAgentResponse {
        mcp_definitions,
        features,
    }))
}

async fn into_agent_value(
    user_id: &str,
    definition: AgentDefinition,
    auth: &EncryptedTokens,
) -> Result<serde_json::Value, (StatusCode, String)> {
    info!(user_id, "building agent from definition");
    let agent = definition.into_agent(auth).await.map_err(|e| {
        error!(user_id, error = %e, "failed to build agent from definition");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "something went wrong when compiling the agent".to_string(),
        )
    })?;

    // Turn into Value again.
    let agent_value = serde_json::to_value(&agent).map_err(|e| {
        error!(user_id, error = %e, "failed to serialize agent definition");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "something went wrong when formatting the agent".to_string(),
        )
    })?;
    Ok(agent_value)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployAgentRequest {
    pub definition: AgentDefinition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<HashMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployAgentResponse {
    pub deployment_id: String,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn deploy_agent(
    headers: HeaderMap,
    State(db): State<AppState>,
    Json(req): Json<DeployAgentRequest>,
) -> Result<Json<DeployAgentResponse>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, "deploy_agent: start");

    info!(user_id, "deploy_agent: fetching oauth tokens");
    let auth = db.get_oauth_tokens(&user_id).await.map_err(|e| {
        error!(user_id, error = %e, "deploy_agent: failed to fetch oauth tokens");
        e
    })?;

    let definition = req.definition;
    info!(user_id, "deploy_agent");
    let agent = into_agent_value(&user_id, definition, &auth).await?;
    let credentials = req.credentials.map(|creds| creds.into());

    info!(user_id, "deploy_agent: saving deployment to db");
    let deployment_id = db
        .new_agent_deployment(&user_id, agent, credentials)
        .await
        .map_err(|e| {
            error!(user_id, error = %e, "deploy_agent: failed to save deployment");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "something went wrong when pushing the agent to the database".to_string(),
            )
        })?;

    info!(user_id, deployment_id, "deploy_agent: done");
    Ok(Json(DeployAgentResponse { deployment_id }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeployAgentRequest {
    pub deployment_id: String,
    pub definition: AgentDefinition,
    /// Credentials to replace, by key; the deployment keeps every key not
    /// named here. A new definition can need different workspace grants, so
    /// the caller sends the `workspaces` it re-derived — and a channel's
    /// secrets, which it does not resend, survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<HashMap<String, Value>>,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn redeploy_agent(
    headers: HeaderMap,
    State(db): State<AppState>,
    Json(req): Json<RedeployAgentRequest>,
) -> Result<Json<DeployAgentResponse>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    let deployment_id = req.deployment_id;
    info!(user_id, deployment_id, "redeploy_agent: start");

    info!(user_id, "redeploy_agent: fetching oauth tokens");
    let auth = db.get_oauth_tokens(&user_id).await.map_err(|e| {
        error!(user_id, error = %e, "redeploy_agent: failed to fetch oauth tokens");
        e
    })?;

    info!(user_id, "redeploy_agent");
    let definition = req.definition;
    let agent = into_agent_value(&user_id, definition, &auth).await?;
    let credentials = req.credentials.map(|creds| creds.into());

    info!(
        user_id,
        deployment_id, "redeploy_agent: updating deployment in db"
    );
    let deployment_id = db
        .update_agent_deployment(&user_id, &deployment_id, agent, credentials)
        .await
        .map_err(|e| {
            error!(user_id, error = %e, "redeploy_agent: failed to update deployment");
            e
        })?;

    info!(user_id, deployment_id, "redeploy_agent: done");
    Ok(Json(DeployAgentResponse { deployment_id }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteDeploymentRequest {
    pub deployment_id: String,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn delete_deployment(
    State(db): State<AppState>,
    Json(req): Json<DeleteDeploymentRequest>,
) -> Result<Json<DeployAgentResponse>, (StatusCode, String)> {
    info!(
        deployment_id = req.deployment_id,
        "delete_deployment: deleting deployment"
    );
    db.remove_agent_deployment(&req.deployment_id)
        .await
        .map_err(|e| {
            error!( error = %e, "delete_deployment: failed to delete deployment");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
    info!(
        deployment_id = req.deployment_id,
        "delete_deployment: deployment deleted"
    );

    Ok(Json(DeployAgentResponse {
        deployment_id: req.deployment_id,
    }))
}

#[derive(Debug, Serialize, Deserialize)]
struct GenAgentRequest {
    prompt: String,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn magically_generate_agent(
    Json(def): Json<GenAgentRequest>,
) -> Result<Json<AgentDefinition>, (StatusCode, String)> {
    let r = generate_agent(&def.prompt).await?;
    Ok(Json(r))
}

/// A session is never trimmed for display purposes — this is just large
/// enough that no real conversation would ever hit it. (Passing `0` to
/// `get_session_by_id` does *not* mean "unlimited": Mongo only populates
/// messages when `n_messages > 0`.)
const FULL_HISTORY_LIMIT: usize = 10_000;

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    agent_id: String,
    #[serde(default = "default_n_sessions")]
    n: usize,
}
fn default_n_sessions() -> usize {
    20
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn list_sessions(
    headers: HeaderMap,
    State(db): State<AppState>,
    Query(q): Query<ListSessionsQuery>,
) -> Result<Json<Vec<SessionSummary>>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, agent_id = q.agent_id, "list_sessions");

    let sessions = db.get_user_sessions(&q.agent_id, &user_id, q.n).await?;
    Ok(Json(sessions))
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn get_session(
    headers: HeaderMap,
    State(db): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionData>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, session_id, "get_session");
    let session = db
        .get_session_by_id(&session_id, FULL_HISTORY_LIMIT)
        .await?;
    if session.user_id != user_id {
        // Owned by someone else: treated as not-found, same rationale as the
        // ownership check in update_agent_deployment — existence isn't leaked.
        return Err((StatusCode::NOT_FOUND, "session not found".to_string()));
    }
    Ok(Json(session))
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn delete_session(
    headers: HeaderMap,
    State(db): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, session_id, "delete_session");
    // The ownership check lives in the DB call: a session owned by someone
    // else does not match, and comes back as a plain "not found".
    db.delete_session(&session_id, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The body of a rename: what the conversation is called from now on. An
/// empty name — or a missing one — puts it back to the name derived from its
/// first message, which is what clearing the box in the UI sends.
#[derive(Debug, Deserialize)]
struct RenameSessionRequest {
    #[serde(default)]
    name: String,
}

#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn rename_session(
    headers: HeaderMap,
    State(db): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, session_id, "rename_session");
    let name = normalize_session_name(&req.name);
    // As in delete_session, the ownership check lives in the DB call.
    db.rename_session(&session_id, &user_id, name.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[tokio::main]
async fn main() {
    forma::install_crypto_provider();
    let tel = TelemetryMode::JustOtel;
    let telemeter = Telemeter::new("forma-manager", tel);

    // The manager deploys agents to multi-tenant servers, which read them
    // from MongoDB.
    let mut db = FormaDBClient::new_mongo()
        .await
        .expect("failed to connect to database");
    db.init().expect("failed to initialise database");

    let state: AppState = Arc::new(db);

    let app = Router::new()
        .route("/tokens", post(save_token))
        .route("/tokens", delete(remove_token))
        .route("/agents/check", post(check_agent))
        .route("/deployments", post(deploy_agent))
        .route("/deployments", put(redeploy_agent))
        .route("/deployments", delete(delete_deployment))
        .route("/agents/generate", post(magically_generate_agent))
        .route("/sessions", get(list_sessions))
        .route(
            "/sessions/{session_id}",
            get(get_session)
                .patch(rename_session)
                .delete(delete_session),
        )
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("forma-manager listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(forma::observability::shutdown_signal())
        .await
        .expect("server error");

    // Ensure all spans are flushed before exit
    telemeter.shutdown();
}
