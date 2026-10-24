//! The manager's HTTP API (`forma-manager`): deploys agents to multi-tenant
//! servers, keeps users' OAuth tokens, and lists and edits their
//! conversations.
//!
//! The routes and their OpenAPI description are built together
//! ([`OpenApiRouter`]), so what is served is what is described.

use crate::{
    api::{
        AgentDefinition, AgentFeatures, McpToolDefinition, ObjectDefinition,
        generative::generate_agent,
    },
    server::expect_user_id,
};
use agent::Callable;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use db::FormaDBClient;
use protocols::{
    EncryptedTokens, FormaDB, OauthToken, SessionData, SessionSummary, normalize_session_name,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tracing::{error, info};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

/// What the manager's handlers share: the database.
pub type ManagerState = Arc<FormaDBClient>;

/// The tag of each group of operations.
const TOKENS: &str = "Tokens";
const AGENTS: &str = "Agents";
const DEPLOYMENTS: &str = "Deployments";
const SESSIONS: &str = "Sessions";

/// The manager's API, as described before any route is added.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Forma Manager API",
        description = "The API of `forma-manager`, which deploys agents to multi-tenant Forma \
servers and keeps what they share: users' OAuth tokens (for MCP servers) and their \
conversations.

**The manager does not authenticate its callers.** It trusts `X-User-ID`, and anyone who \
can reach it can deploy, change and delete agents. It is meant to be called by your own \
back-end, on a private network, and must never be exposed to the internet. Your back-end \
authenticates the user, and sends their id.

Agents are sent as their *expanded* definition (`forma expand -f json`), which the manager \
compiles itself: with the deploying user's OAuth tokens, so that MCP servers can be reached. \
A multi-tenant runtime then serves each deployment to requests whose `X-Deployment-ID` names \
it.",
        license(name = "MIT", identifier = "MIT"),
    ),
    tags(
        (name = "Tokens", description = "OAuth tokens a user obtained for MCP servers. Agents that use \
those servers are compiled, and run, with the tokens of the user they act for."),
        (name = "Agents", description = "Agent definitions that are not deployed: checking and generating them."),
        (name = "Deployments", description = "Agents deployed to the multi-tenant runtimes, which read them \
from the database. A deployment's id is what requests to a runtime send as `X-Deployment-ID`."),
        (name = "Sessions", description = "Users' conversations, as the runtimes stored them. In a \
multi-tenant runtime, a conversation's `agent_id` is the id of the deployment it was held with."),
    ),
    external_docs(
        url = "https://germolinal.github.io/forma/docs/api/manager.html",
        description = "The manager's API, explained"
    )
)]
pub struct ManagerApi;

/// The manager's routes, with their OpenAPI description.
pub fn routes() -> OpenApiRouter<ManagerState> {
    OpenApiRouter::with_openapi(ManagerApi::openapi())
        .routes(routes!(save_token, remove_token))
        .routes(routes!(check_agent))
        .routes(routes!(generate))
        .routes(routes!(deploy_agent, redeploy_agent, delete_deployment))
        .routes(routes!(list_sessions))
        .routes(routes!(get_session, rename_session, delete_session))
}

/// The manager's server, on `db`.
pub fn router(db: ManagerState) -> Router {
    let (router, _) = routes().split_for_parts();
    router.with_state(db)
}

/// The manager's OpenAPI description.
pub fn openapi() -> utoipa::openapi::OpenApi {
    routes().into_openapi()
}

/// The token a user obtained for an MCP server, to keep.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SaveTokenRequest {
    /// The user the token belongs to.
    user_id: String,
    /// The MCP server the token is for: its URL, as the agent definition
    /// names it (`url`). Agents look tokens up by it.
    mcp_url: String,
    /// The token. It is stored encrypted (`FORMA_ENCRYPTION_KEY`), and
    /// refreshed when it expires.
    token: OauthToken,
}

/// Keeps the OAuth token a user obtained for an MCP server.
///
/// The token replaces the one the user had for the same server, if any;
/// their tokens for other servers are kept. Agents that use the server are
/// compiled and run with it.
#[utoipa::path(
    post,
    path = "/tokens",
    tag = TOKENS,
    request_body = SaveTokenRequest,
    responses(
        (status = OK, description = "The token is kept"),
        (status = INTERNAL_SERVER_ERROR, description = "The token could not be stored", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn save_token(
    State(db): State<ManagerState>,
    Json(req): Json<SaveTokenRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    tracing::info!("saving token for user {}", req.user_id);
    // Read existing tokens, merge the new one in, write back.
    let mut tokens = db.get_oauth_tokens(&req.user_id).await.unwrap_or_default();

    tokens.insert(req.mcp_url, req.token);

    db.save_oauth_tokens(&req.user_id, tokens).await?;

    Ok(StatusCode::OK)
}

/// Which of a user's tokens to forget.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RemoveTokenRequest {
    /// The user the token belongs to.
    user_id: String,
    /// The MCP server whose token is forgotten, as in `POST /tokens`.
    mcp_url: String,
}

/// Forgets the token a user had for an MCP server.
///
/// Forgetting a token the user does not have is not an error. Agents
/// deployed already keep working until they are compiled again (redeployed):
/// a running agent reads the user's tokens when it is called.
#[utoipa::path(
    delete,
    path = "/tokens",
    tag = TOKENS,
    request_body = RemoveTokenRequest,
    responses(
        (status = OK, description = "The token is forgotten"),
        (status = INTERNAL_SERVER_ERROR, description = "The tokens could not be stored", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn remove_token(
    State(db): State<ManagerState>,
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum McpCheck {
    /// Connect to each with the caller's tokens, as running the agent would.
    /// What a deploy needs.
    #[default]
    Connect,
    /// Connect to none (see `McpAccess::Unconnected`): everything else is
    /// checked exactly as it would be, by a caller who has not connected those
    /// servers yet. What saving a blueprint needs.
    Unconnected,
}

/// The options of `POST /agents/check`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CheckAgentQuery {
    /// How the agent's MCP servers are treated: `connect` (the default) to
    /// each with the caller's tokens, as running the agent would; or
    /// `unconnected`, to check everything else without them, for a caller who
    /// has not connected those servers yet. Without connecting, the names of
    /// a server's tools are unknown, so whether they clash is not checked.
    #[serde(default)]
    #[param(inline)]
    mcp: McpCheck,
}

/// Checks an agent definition without deploying it.
///
/// The definition is checked (ids, names), compiled (connecting to its MCP
/// servers with the tokens of `X-User-ID`, unless `mcp=unconnected`), and the
/// compiled agent checked as a runtime would before serving it. What it
/// reports is what the agent uses: what a deployment of it needs to be
/// granted, and what a user needs to be connected to.
#[utoipa::path(
    post,
    path = "/agents/check",
    tag = AGENTS,
    params(
        CheckAgentQuery,
        ("X-User-ID" = String, Header, description = "The user who checks the agent. Their OAuth tokens are used to reach its MCP servers."),
    ),
    request_body(
        content = Object,
        description = "The agent's expanded definition (`forma expand -f json`): an [Agent](https://germolinal.github.io/forma/docs/reference_docs/auto-agent.html), with every `$file` inlined."
    ),
    responses(
        (status = OK, description = "The agent compiled, and passed its checks", body = CheckAgentResponse),
        (status = BAD_REQUEST, description = "The definition is malformed, does not compile, or does not pass the checks; or `X-User-ID` is missing. The message says why.", body = String, content_type = "text/plain"),
        (status = UNPROCESSABLE_ENTITY, description = "The body is not an agent definition (an unknown field, a missing one, a wrong type)", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn check_agent(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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

/// An agent to deploy.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeployAgentRequest {
    /// The agent's expanded definition (`forma expand -f json`): an
    /// [Agent](https://germolinal.github.io/forma/docs/reference_docs/auto-agent.html),
    /// with every `$file` inlined. It must have an `id`.
    #[schema(value_type = Object)]
    pub definition: AgentDefinition,
    /// What the deployment needs to reach what is outside it, by key; stored
    /// encrypted (`FORMA_ENCRYPTION_KEY`). `whatsapp_api`
    /// (`phone_number_id`, `access_token`, `verify_token`, `app_secret`) and
    /// `telegram_api` (`bot_token`, `secret_token`) for the chat apps a
    /// runtime serves it on, and `workspaces`: the workspaces its tools may
    /// reach, as a list of `{"path": "<bucket>/<workspace id>", "scopes":
    /// ["read", "write"]}` (what `POST /agents/check` reports under
    /// `features.workspaces`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<HashMap<String, Object>>)]
    pub credentials: Option<HashMap<String, Value>>,
}

/// A deployment, as the manager reports it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeployAgentResponse {
    /// The deployment's id: what requests to a multi-tenant runtime send as
    /// `X-Deployment-ID`, and what conversations held with it have as their
    /// `agent_id`.
    pub deployment_id: String,
}

/// Deploys an agent.
///
/// The definition is compiled with the OAuth tokens of `X-User-ID` (who owns
/// the deployment from now on), and stored. Multi-tenant runtimes serve it,
/// with no restart, to requests whose `X-Deployment-ID` is the id this
/// returns. Each call creates a new deployment, even for the same agent:
/// update one with `PUT /deployments`.
#[utoipa::path(
    post,
    path = "/deployments",
    tag = DEPLOYMENTS,
    params(
        ("X-User-ID" = String, Header, description = "The user who deploys the agent, and owns the deployment. Their OAuth tokens are used to reach its MCP servers."),
    ),
    request_body = DeployAgentRequest,
    responses(
        (status = OK, description = "The agent is deployed", body = DeployAgentResponse),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or the definition has no `id`", body = String, content_type = "text/plain"),
        (status = UNPROCESSABLE_ENTITY, description = "The body is not a deployment (an unknown field, a missing one, a wrong type)", body = String, content_type = "text/plain"),
        (status = INTERNAL_SERVER_ERROR, description = "The agent did not compile, or could not be stored. Check it first with `POST /agents/check`, which says why.", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn deploy_agent(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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

/// A new version of a deployed agent.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RedeployAgentRequest {
    /// The deployment to update.
    pub deployment_id: String,
    /// The agent's new expanded definition, as in `POST /deployments`.
    #[schema(value_type = Object)]
    pub definition: AgentDefinition,
    /// Credentials to replace, by key; the deployment keeps every key not
    /// named here. A new definition can need different workspace grants, so
    /// the caller sends the `workspaces` it re-derived — and a channel's
    /// secrets, which it does not resend, survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<HashMap<String, Object>>)]
    pub credentials: Option<HashMap<String, Value>>,
}

/// Updates a deployed agent.
///
/// The new definition is compiled with the OAuth tokens of `X-User-ID`, who
/// must own the deployment, and replaces the old one: the runtimes serve it
/// from the next request on, in the same conversations. Credentials are
/// replaced only under the keys the request names.
#[utoipa::path(
    put,
    path = "/deployments",
    tag = DEPLOYMENTS,
    params(
        ("X-User-ID" = String, Header, description = "The user who owns the deployment. Their OAuth tokens are used to reach its MCP servers."),
    ),
    request_body = RedeployAgentRequest,
    responses(
        (status = OK, description = "The deployment is updated", body = DeployAgentResponse),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or `deployment_id` is not a valid id", body = String, content_type = "text/plain"),
        (status = NOT_FOUND, description = "There is no such deployment, or it belongs to another user (which is not told apart, so as not to reveal it exists)", body = String, content_type = "text/plain"),
        (status = UNPROCESSABLE_ENTITY, description = "The body is not a redeployment", body = String, content_type = "text/plain"),
        (status = INTERNAL_SERVER_ERROR, description = "The agent did not compile, or could not be stored", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn redeploy_agent(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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

/// A deployment to delete.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeleteDeploymentRequest {
    /// The deployment to delete.
    pub deployment_id: String,
}

/// Deletes a deployment.
///
/// `X-User-ID` must own it. Runtimes stop serving it at once: requests that
/// name it are answered `404`. Its conversations are kept.
#[utoipa::path(
    delete,
    path = "/deployments",
    tag = DEPLOYMENTS,
    params(
        ("X-User-ID" = String, Header, description = "The user who owns the deployment."),
    ),
    request_body = DeleteDeploymentRequest,
    responses(
        (status = OK, description = "The deployment is deleted", body = DeployAgentResponse),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or `deployment_id` is not a valid id", body = String, content_type = "text/plain"),
        (status = NOT_FOUND, description = "There is no such deployment, or it belongs to another user (which is not told apart, so as not to reveal it exists)", body = String, content_type = "text/plain"),
        (status = UNPROCESSABLE_ENTITY, description = "The body is not a deployment to delete", body = String, content_type = "text/plain"),
        (status = INTERNAL_SERVER_ERROR, description = "The database failed", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn delete_deployment(
    headers: HeaderMap,
    State(db): State<ManagerState>,
    Json(req): Json<DeleteDeploymentRequest>,
) -> Result<Json<DeployAgentResponse>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(
        user_id,
        deployment_id = req.deployment_id,
        "delete_deployment: deleting deployment"
    );
    db.remove_agent_deployment(&user_id, &req.deployment_id)
        .await
        .map_err(|e| {
            error!(error = %e, "delete_deployment: failed to delete deployment");
            e
        })?;
    info!(
        deployment_id = req.deployment_id,
        "delete_deployment: deployment deleted"
    );

    Ok(Json(DeployAgentResponse {
        deployment_id: req.deployment_id,
    }))
}

/// What an agent should do, in words.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GenAgentRequest {
    /// A description of the agent, as a person would write it.
    prompt: String,
}

/// Writes an agent definition from a description.
///
/// A language model (Gemini, on Vertex AI: the manager needs Google Cloud
/// credentials) writes the definition. Nothing is checked or deployed: check
/// it with `POST /agents/check` before deploying it.
#[utoipa::path(
    post,
    path = "/agents/generate",
    tag = AGENTS,
    request_body = GenAgentRequest,
    responses(
        (status = OK, description = "The agent's definition: an [Agent](https://germolinal.github.io/forma/docs/reference_docs/auto-agent.html)", body = Object),
        (status = UNPROCESSABLE_ENTITY, description = "The body has no `prompt`", body = String, content_type = "text/plain"),
        (status = INTERNAL_SERVER_ERROR, description = "The model could not be reached, or did not write a definition", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn generate(
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

/// Which conversations to list.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListSessionsQuery {
    /// The agent the conversations were held with: in a multi-tenant
    /// runtime, the deployment's id; in a single-tenant one, the agent's
    /// `id`.
    agent_id: String,
    /// How many to list, at most.
    #[serde(default = "default_n_sessions")]
    #[param(default = 20, minimum = 0)]
    n: usize,
}
fn default_n_sessions() -> usize {
    20
}

/// Lists a user's conversations with an agent.
///
/// The most recent first, without their messages. Conversations nobody wrote
/// in (every `/v1/init` starts one) are left out.
#[utoipa::path(
    get,
    path = "/sessions",
    tag = SESSIONS,
    params(
        ListSessionsQuery,
        ("X-User-ID" = String, Header, description = "Whose conversations to list."),
    ),
    responses(
        (status = OK, description = "The conversations, the most recent first", body = Vec<SessionSummary>),
        (status = BAD_REQUEST, description = "`X-User-ID` or `agent_id` is missing", body = String, content_type = "text/plain"),
        (status = INTERNAL_SERVER_ERROR, description = "The database failed", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn list_sessions(
    headers: HeaderMap,
    State(db): State<ManagerState>,
    Query(q): Query<ListSessionsQuery>,
) -> Result<Json<Vec<SessionSummary>>, (StatusCode, String)> {
    let user_id = expect_user_id(&headers)?;
    info!(user_id, agent_id = q.agent_id, "list_sessions");

    let sessions = db.get_user_sessions(&q.agent_id, &user_id, q.n).await?;
    Ok(Json(sessions))
}

/// Reads a conversation, with every message.
///
/// What the runtime stored: the messages (user, assistant, tool), the calls
/// it waits on (`pending_tool_calls`), and what it keeps to resume them.
#[utoipa::path(
    get,
    path = "/sessions/{session_id}",
    tag = SESSIONS,
    params(
        ("session_id" = String, Path, description = "The conversation."),
        ("X-User-ID" = String, Header, description = "The user who owns the conversation."),
    ),
    responses(
        (status = OK, description = "The conversation", body = SessionData),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or the id is not valid", body = String, content_type = "text/plain"),
        (status = NOT_FOUND, description = "There is no such conversation, or it belongs to another user", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn get_session(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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

/// Deletes a conversation, with its messages.
#[utoipa::path(
    delete,
    path = "/sessions/{session_id}",
    tag = SESSIONS,
    params(
        ("session_id" = String, Path, description = "The conversation."),
        ("X-User-ID" = String, Header, description = "The user who owns the conversation."),
    ),
    responses(
        (status = NO_CONTENT, description = "The conversation is deleted"),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or the id is not valid", body = String, content_type = "text/plain"),
        (status = NOT_FOUND, description = "There is no such conversation, or it belongs to another user", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn delete_session(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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
#[derive(Debug, Deserialize, ToSchema)]
pub struct RenameSessionRequest {
    /// The conversation's new name, cut to 100 characters. Empty, or absent,
    /// to go back to the name derived from its first message.
    #[serde(default)]
    name: String,
}

/// Renames a conversation.
///
/// `GET /sessions` lists it under the new name (`display_name`).
#[utoipa::path(
    patch,
    path = "/sessions/{session_id}",
    tag = SESSIONS,
    params(
        ("session_id" = String, Path, description = "The conversation."),
        ("X-User-ID" = String, Header, description = "The user who owns the conversation."),
    ),
    request_body = RenameSessionRequest,
    responses(
        (status = NO_CONTENT, description = "The conversation is renamed"),
        (status = BAD_REQUEST, description = "`X-User-ID` is missing, or the id is not valid", body = String, content_type = "text/plain"),
        (status = NOT_FOUND, description = "There is no such conversation, or it belongs to another user", body = String, content_type = "text/plain"),
    )
)]
#[tracing::instrument(skip_all)]
#[axum::debug_handler]
async fn rename_session(
    headers: HeaderMap,
    State(db): State<ManagerState>,
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
