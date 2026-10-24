use chrono::Utc;
use genai_client::JsonSchema;
use rmcp::{
    RoleClient, ServiceExt,
    model::JsonObject,
    service::RunningService,
    transport::{
        auth::{
            AuthClient, AuthorizationManager, CredentialStore, InMemoryCredentialStore,
            OAuthClientConfig, OAuthTokenResponse, StoredCredentials,
        },
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
    },
};
use serde_json::Value;
use std::{collections::HashMap, error::Error, sync::Arc};
#[cfg(feature = "cli")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use agent::{
    Tool, ToolRunner,
    tool::{McpTool, mcp_tool::UNCONNECTED_TOOL_PREFIX},
};

use derive::FormaDocs;
use protocols::{EncryptedTokens, FormaError, OauthToken};
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

struct BasicClientHandler;
impl rmcp::ClientHandler for BasicClientHandler {}

/// A running MCP session, wrapping a [`RunningService`] with [`RoleClient`] and [`BasicClientHandler`].
type McpSession = RunningService<RoleClient, BasicClientHandler>;

fn mcp_api_error<E: Error>(msg: &'static str, e: E, url: &str) -> FormaError {
    FormaError::ApiCall {
        url: Some(url.to_string()),
        msg: format!("{msg}:{e}"),
    }
}
#[cfg(feature = "cli")]
fn resolve_secret(value: &str) -> Result<String, FormaError> {
    std::env::var(value)
        .map_err(|_| FormaError::Input(format!("environment variable '{value}' is not set")))
}

/// Per-tool overrides for an MCP server tool.
///
/// When present, these values take precedence over the definition-level
/// `confirm_before_calling` and `just_for` defaults. A missing field means
/// "inherit the parent value."
#[derive(Clone, Debug, Default, Serialize, Deserialize, FormaDocs)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct McpToolOverride {
    /// Override `confirm_before_calling` for this specific tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_before_calling: Option<bool>,

    /// Override `just_for` for this specific tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub just_for: Option<Vec<String>>,
}

#[cfg(feature = "cli")]
fn parse_code_and_state(request: &str) -> Result<(String, String), FormaError> {
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("code"), Some(v)) => code = Some(decode_percent(v)),
            (Some("state"), Some(v)) => state = Some(decode_percent(v)),
            _ => {}
        }
    }
    if code.is_none() {
        return Err(FormaError::Input(
            "Missing code in OAuth callback".to_string(),
        ));
    }
    if state.is_none() {
        return Err(FormaError::Input(
            "Missing state in OAuth callback".to_string(),
        ));
    }
    Ok((
        code.expect("code is missing in OAuth callback"),
        state.expect("state is missing in OAuth callback"),
    ))
}

/// One-shot TCP server that catches the OAuth redirect and returns (code, state).
#[cfg(feature = "cli")]
async fn wait_for_oauth_callback(
    listener: tokio::net::TcpListener,
) -> Result<(String, String), FormaError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| FormaError::Input(format!("OAuth callback accept error: {e}")))?;

        let mut buf = vec![0u8; 4096];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| FormaError::Input(format!("OAuth callback read error: {e}")))?;

        // First line: "GET /callback?code=X&state=Y HTTP/1.1"
        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request.lines().next().unwrap_or("");
        if first_line.starts_with("GET /callback") {
            if let Ok((code, state)) = parse_code_and_state(&request) {
                let body = "<h1>Authentication successful!</h1><p>You can close this tab.</p>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return Ok((code, state));
            } else {
                let body = "<h1>Authentication failed.</h1><p>Missing code parameter.</p>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return Err(FormaError::Input(
                    "OAuth callback missing code parameter".to_string(),
                ));
            }
        } else {
            // Server is checking something... let's say this is the wrong door
            let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response.as_bytes()).await.ok();

            eprintln!("Gracefully ignored background probe: {}", first_line);
        }
    }
}

/// Minimal percent-decoding for OAuth callback query params.
#[cfg(feature = "cli")]
fn decode_percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
                && let Ok(byte) = u8::from_str_radix(hex, 16)
            {
                out.push(byte as char);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Runs the full browser-based OAuth2 authorization_code + PKCE flow and returns
/// an [`OauthToken`] populated with the resulting credentials.
#[cfg(feature = "cli")]
async fn authenticate_oauth(
    mcp_url: &str,
    client_id: String,
    client_secret: Option<String>,
    scopes: Vec<String>,
    callback_port: Option<u16>,
) -> Result<OauthToken, FormaError> {
    let callback_port = callback_port.unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{callback_port}"))
        .await
        .map_err(|e| mcp_api_error("failed to bind OAuth callback server", e, mcp_url))?;
    let port = listener
        .local_addr()
        .map_err(|e| mcp_api_error("failed to get callback port", e, mcp_url))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    let mut manager = AuthorizationManager::new(mcp_url)
        .await
        .map_err(|e| mcp_api_error("OAuth init failed", e, mcp_url))?;
    let metadata = manager
        .resolve_metadata()
        .await
        .map_err(|e| mcp_api_error("OAuth metadata discovery failed", e, mcp_url))?;
    manager.set_metadata(metadata.metadata.clone());

    let mut cfg = OAuthClientConfig::new(&client_id, &redirect_uri);
    if let Some(ref secret) = client_secret {
        cfg = cfg.with_client_secret(secret.clone());
    }
    manager
        .configure_client(cfg)
        .map_err(|e| mcp_api_error("OAuth client config failed", e, mcp_url))?;

    let effective_scopes: Vec<String> = if scopes.is_empty() {
        manager.select_scopes(None, &[])
    } else {
        scopes
    };

    let scope_refs: Vec<&str> = effective_scopes.iter().map(|s| s.as_str()).collect();
    let auth_url = manager
        .get_authorization_url(&scope_refs)
        .await
        .map_err(|e| mcp_api_error("failed to get OAuth auth URL", e, mcp_url))?;

    eprintln!(
        "\n\
         ╔══════════════════════════════════════════╗\n\
         ║       MCP Authentication Required        ║\n\
         ╚══════════════════════════════════════════╝\n\
         Open this URL in your browser:\n\n  {auth_url}\n\n\
         Waiting…\n"
    );

    let (code, csrf_token) = wait_for_oauth_callback(listener).await?;

    let token_response = manager
        .exchange_code_for_token(&code, &csrf_token)
        .await
        .map_err(|e| mcp_api_error("OAuth token exchange failed", e, mcp_url))?;

    eprintln!("Authentication successful!.\n");

    // Extract token fields via serde_json to avoid a direct oauth2 crate dependency.
    let token_json = serde_json::to_value(&token_response)
        .map_err(|e| FormaError::Input(format!("failed to serialize token response: {e}")))?;

    let access_token = token_json["access_token"]
        .as_str()
        .ok_or_else(|| FormaError::Input("missing access_token in OAuth response".to_string()))?
        .to_string();

    let refresh_token = token_json["refresh_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let expires_in_secs = token_json["expires_in"].as_u64().unwrap_or(3600);
    let expires_at = Utc::now() + chrono::Duration::seconds(expires_in_secs as i64);

    Ok(OauthToken {
        client_id,
        client_secret: client_secret.unwrap_or_default(),
        access_token,
        refresh_token,
        expires_at,
        token_endpoint: metadata.metadata.token_endpoint,
    })
}

/// Connects to an MCP server using a pre-obtained OAuth token.
///
/// Seeds an [`AuthorizationManager`] from the stored token so that automatic
/// token refresh is handled transparently for the lifetime of the session.
async fn connect_with_token(
    url: &str,
    transport_config: StreamableHttpClientTransportConfig,
    token: OauthToken,
) -> Result<McpSession, FormaError> {
    // Compute remaining lifetime so the manager's expiry check stays correct
    // even for partially-consumed tokens.
    let remaining_secs = token
        .expires_at
        .signed_duration_since(Utc::now())
        .num_seconds()
        .max(0) as u64;

    let token_json = serde_json::json!({
        "access_token": token.access_token,
        "token_type": "bearer",
        "refresh_token": token.refresh_token,
        "expires_in": remaining_secs,
    });
    let token_response: OAuthTokenResponse = serde_json::from_value(token_json)
        .map_err(|e| FormaError::Input(format!("invalid stored token: {e}")))?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let store = InMemoryCredentialStore::new();
    store
        .save(StoredCredentials::new(
            token.client_id.clone(),
            Some(token_response),
            vec![],
            Some(now_secs),
        ))
        .await
        .map_err(|e| FormaError::Input(format!("credential store error: {e}")))?;

    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|e| mcp_api_error("OAuth init failed", e, url))?;
    let metadata = manager
        .resolve_metadata()
        .await
        .map_err(|e| mcp_api_error("OAuth metadata discovery failed", e, url))?;
    manager.set_metadata(metadata.metadata);
    manager.set_credential_store(store);

    // redirect_uri is unused for token refresh; a placeholder satisfies the type.
    let mut cfg = OAuthClientConfig::new(&token.client_id, "http://localhost");
    if !token.client_secret.is_empty() {
        cfg = cfg.with_client_secret(token.client_secret);
    }
    manager
        .configure_client(cfg)
        .map_err(|e| mcp_api_error("OAuth client config failed", e, url))?;

    let auth_client = AuthClient::new(reqwest::Client::new(), manager);
    let transport = StreamableHttpClientTransport::<AuthClient<reqwest::Client>>::with_client(
        auth_client,
        transport_config,
    );
    BasicClientHandler
        .serve(transport)
        .await
        .map_err(|e| mcp_api_error("failed to serve MCP transport", e, url))
}

async fn connect_no_auth(
    url: &str,
    transport_config: StreamableHttpClientTransportConfig,
) -> Result<McpSession, FormaError> {
    let transport = StreamableHttpClientTransport::<reqwest::Client>::with_client(
        reqwest::Client::new(),
        transport_config,
    );

    let r = BasicClientHandler
        .serve(transport)
        .await
        .map_err(|e| FormaError::ApiCall {
            url: Some(url.to_string()),
            msg: e.to_string(),
        })?;
    Ok(r)
}

fn into_schema(value: Arc<JsonObject>) -> JsonSchema {
    let mut json_value = Value::Object((*value).clone());
    normalize_schema(&mut json_value);
    serde_json::from_value(json_value).expect("could not normalize MCP tool schema")
}

fn normalize_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get("additionalProperties")
                && v.is_object()
            {
                map.insert("additionalProperties".to_string(), Value::Bool(true));
            }
            if map.contains_key("properties") && !map.contains_key("required") {
                map.insert("required".to_string(), Value::Array(vec![]));
            }
            if let Some(Value::Object(props)) = map.get_mut("properties") {
                props.retain(|_, v| v.is_object());
                for prop_schema in props.values_mut() {
                    normalize_schema(prop_schema);
                }
            }
            for (key, v) in map.iter_mut() {
                if key != "properties" {
                    normalize_schema(v);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                normalize_schema(v);
            }
        }
        _ => {}
    }
}

/// Authentication configuration for an MCP server.
///
/// Credentials are read from environment variables at runtime and never appear
/// in config files or logs. Set the field to the name of the environment variable
/// that holds the value (e.g. `token: MY_MCP_TOKEN`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum McpAuth {
    /// Interactive OAuth 2.0 authorization_code + PKCE flow.
    ///
    /// Call [`McpToolDefinition::authenticate`] once to complete the browser login
    /// and obtain an [`OauthToken`]. Store that token and pass it to
    /// [`McpToolDefinition::connect`] on every subsequent run.
    ///
    /// Use this for MCP servers that act on behalf of a human user (e.g. Gmail,
    /// Google Calendar, GitHub). The `client_id` must match a credential registered
    /// with the authorization server and configured with `http://localhost` as an
    /// allowed redirect URI.
    ///
    /// Example:
    /// ```yaml
    /// auth:
    ///   type: oauth
    ///   client_id: GMAIL_CLIENT_ID
    ///   client_secret: GMAIL_CLIENT_SECRET   # omit for public clients
    ///   scopes:
    ///     - https://www.googleapis.com/auth/gmail.readonly
    /// ```
    Oauth {
        /// Environment variable holding the OAuth 2.0 client ID.
        client_id: String,
        /// Environment variable holding the client secret.
        /// Omit for public clients that rely on dynamic registration.
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
        /// Scopes to request. If omitted, the server's advertised scopes are used.
        #[serde(default)]
        scopes: Vec<String>,
        /// The port to use for the OAuth 2.0 callback URL.
        /// If not specified, a random port will be chosen.
        #[serde(default)]
        callback_port: Option<u16>,
    },
}

/// Represents a tool definition for an MCP (Model-Client Protocol) server.
///
/// The server's tools are discovered at load time. `confirm_before_calling`
/// and `just_for` are applied to every tool; use the `tools` map to override
/// either value for specific tools by name.
#[derive(Clone, Debug, Serialize, Deserialize, FormaDocs)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct McpToolDefinition {
    /// The URL of the MCP server.
    pub url: String,

    /// Authentication configuration. Omit for unauthenticated servers.
    ///
    /// The one strategy is `oauth`: OAuth 2.0 with a client registered with
    /// the server's authorization server, on behalf of the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpAuth>,

    /// Per-tool overrides keyed by tool name.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tools: HashMap<String, McpToolOverride>,
}

impl ObjectDefinition for McpToolDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

/// How compiling an agent reaches the MCP servers it uses.
#[derive(Debug, Clone)]
pub enum McpAccess {
    /// Connect to each one, with these tokens, and take the tools it offers.
    /// What an agent that will run is compiled with.
    Connect(EncryptedTokens),
    /// Connect to none. Each server stands in as one placeholder tool, so that
    /// every node keeps the tools-or-none shape it would have connected — and
    /// with it every check that depends on it — and nothing about the agent
    /// besides its MCP servers can fail or pass differently. What cannot be
    /// checked is what only a server knows: the names of its tools, and so
    /// whether they clash. For checking and describing an agent by someone
    /// who has not connected its servers; never for running it.
    Unconnected,
}

impl McpToolDefinition {
    /// The one tool this server stands in as when it is not connected to.
    ///
    /// Named after the server's URL, so two copies of one server in a node
    /// clash here just as their real tools would.
    fn placeholder(&self, confirm_before_calling: bool, just_for: &[String]) -> Tool {
        let url: String = self
            .url
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let name = format!("{UNCONNECTED_TOOL_PREFIX}{url}");
        Tool {
            name: name.clone(),
            description: Some(format!("The tools of {}, not connected to.", self.url)),
            tool: ToolRunner::Mcp(McpTool {
                name,
                description: None,
                input_schema: JsonSchema {
                    schema_type: Some(genai_client::schema::SchemaType::Object),
                    ..Default::default()
                },
                server_url: self.url.clone(),
            }),
            confirm_before_calling,
            just_for: just_for.to_vec(),
            bundle: Some(self.url.clone()),
        }
    }
}

impl McpToolDefinition {
    /// Initiates a browser-based OAuth2 login for this server and returns the resulting token.
    ///
    /// The token must be persisted by the caller (e.g. to a file or database) and passed
    /// to [`connect`][McpToolDefinition::connect] on every subsequent run. Returns `None`
    /// for servers that require no authentication.
    #[cfg(feature = "cli")]
    pub async fn authenticate(&self) -> Result<Option<OauthToken>, FormaError> {
        let Some(ref auth) = self.auth else {
            return Ok(None);
        };
        match auth.clone() {
            McpAuth::Oauth {
                client_id,
                client_secret,
                scopes,
                callback_port,
            } => {
                let client_id = resolve_secret(&client_id)?;
                let client_secret = match client_secret {
                    Some(secret) => Some(resolve_secret(&secret)?),
                    None => None,
                };
                let token =
                    authenticate_oauth(&self.url, client_id, client_secret, scopes, callback_port)
                        .await?;
                Ok(Some(token))
            }
        }
    }

    /// Connects to the MCP server and returns one [`McpTool`] per tool it advertises.
    ///
    /// `token` must be `Some` for servers with `auth` configured; pass the token
    /// previously returned by [`authenticate`][McpToolDefinition::authenticate].
    /// Returns an error immediately if auth is configured but no token is supplied.
    async fn connect(&self, token: Option<OauthToken>) -> Result<Vec<McpTool>, FormaError> {
        let transport_config = StreamableHttpClientTransportConfig::with_uri(self.url.clone());

        let session: McpSession = match (self.auth.as_ref(), token) {
            (None, _) => connect_no_auth(&self.url, transport_config).await?,
            (Some(_), None) => {
                return Err(FormaError::Input(format!(
                    "MCP server '{}' requires authentication — call authenticate() first",
                    self.url
                )));
            }
            (Some(_), Some(t)) => connect_with_token(&self.url, transport_config, t).await?,
        };

        let peer = session.peer().clone();
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| FormaError::ApiCall {
                url: Some(self.url.clone()),
                msg: format!("failed to list MCP tools: {e}"),
            })?;

        Ok(tools
            .into_iter()
            .map(|t| McpTool {
                server_url: self.url.clone(),
                name: t.name.into_owned(),
                description: t.description.map(|desc| desc.into_owned()),
                input_schema: into_schema(t.input_schema),
            })
            .collect())
    }

    /// The runtime [`Tool`]s this server offers, with the given defaults
    /// applied — learnt by connecting to it, unless `mcp` says not to, in
    /// which case it is one placeholder (see [`McpAccess::Unconnected`]).
    pub async fn into_tools(
        self,
        confirm_before_calling: bool,
        just_for: &[String],
        mcp: &McpAccess,
    ) -> Result<Vec<Tool>, FormaError> {
        let oauth = match mcp {
            McpAccess::Connect(oauth) => oauth.get(&self.url).cloned(),
            McpAccess::Unconnected => {
                return Ok(vec![self.placeholder(confirm_before_calling, just_for)]);
            }
        };
        let mcp_tools = self.connect(oauth).await?;

        let tools = mcp_tools
            .into_iter()
            .map(|runner| {
                let ov = self.tools.get(&runner.name);
                Tool {
                    name: runner.name.clone(),
                    description: runner.description.clone(),
                    tool: ToolRunner::Mcp(runner),
                    confirm_before_calling: ov
                        .and_then(|o| o.confirm_before_calling)
                        .unwrap_or(confirm_before_calling),
                    just_for: ov
                        .and_then(|o| o.just_for.clone())
                        .unwrap_or_else(|| just_for.to_vec()),
                    bundle: Some(self.url.clone()),
                }
            })
            .collect();

        Ok(tools)
    }
}

#[cfg(test)]
mod tests {

    use genai_client::JsonSchema;

    use super::*;
    #[test]
    #[cfg(feature = "cli")]
    fn test_parse_code_and_state() {
        let exp_code = "na1-efde-0aee-446a-a0b0-dd9524beeebe";
        let exp_state = "Wej3nLlPf0ZrFxHCSTj86Q";
        let request = format!(
            "GET /callback?code={exp_code}&state={exp_state} HTTP/1.1\r\nHost: localhost:6417\r\nConnection: keep-alive\r\nsec-ch-ua: \"Chromium\";v=\"148\", \"Google Chrome\";v=\"148\", \"Not/A)Brand\";v=\"99\"\r\nsec-ch-ua-mobile: ?0\r\nsec-ch-ua-platform: \"macOS\"\r\nUpgrade-Insecure-Requests: 1\r\nUser-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36\r\nAccept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7\r\nSec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-User: ?1\r\nSec-Fetch-Dest: document\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nAccept-Language: es,en-US;q=0.9,en;q=0.8\r\nCookie: _ga=G1.1.17467aa.1ccsd67640104; _ga_ois0N4K3=GS2.1.s17sad79470757$o5$g1$t177947123jxxx\r\n\r\n"
        );
        let result = parse_code_and_state(&request);
        assert!(result.is_ok());
        let (code, state) = result.unwrap();
        assert_eq!(code, exp_code);
        assert_eq!(state, exp_state);
    }

    const HUBSPOT_SCHEMA: &str = r#"{
      "type": "object",
      "properties": {
        "objectType": {
          "type": "string",
          "description": "Mandatory field specifying ObjectType to search for objects for."
        },
        "query": {
          "type": "string",
          "description": "Optional text to search for within the default searchable properties of the specified object type.\nUses simple text matching (contains)\nEach object type has different searchable properties.\ncontacts - firstname, lastname, email, phone, company),\ncompanies - name, website, domain, phone),\ndeals - dealname, pipeline, dealstage, description, dealtype)\nticket - subject, content, hs_pipeline_stage, hs_ticket_category, hs_ticket_id)\n\n[Important] Common stop words (e.g., \"the\", \"a\", \"of\", \"and\", \"in\") may not be indexed for some object types. If a query returns no results, retry by removing stop words or using property filters instead of free-text search.\n\nMax length - 200 chars"
        },
        "properties": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Optional list of property names to include in results. Returns default set if empty. Specify the minimum set of properties needed for the task for best results. Leave it empty only if you explicitly need to discover the full default property set."
        },
        "sorts": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "propertyName": {
                "type": "string",
                "description": "The name of the property to sort by"
              },
              "direction": {
                "type": "string",
                "enum": ["ASCENDING", "DESCENDING"],
                "description": "The order to sort by"
              }
            },
            "required": ["propertyName", "direction"],
            "additionalProperties": false
          },
          "description": "Optional sorting rules for results. Only one sort rule can be applied"
        },
        "limit": {
          "type": "integer",
          "description": "Maximum number of results per page. Max: 200, Default: 100"
        },
        "offset": {
          "type": "integer",
          "description": "Paging cursor token for pagination"
        },
        "filterGroups": {
          "type": "array",
          "items": {
            "type": "object",
            "properties": {
              "filters": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "propertyName": {
                      "type": "string",
                      "description": "Specify the name of the Crm Object Property to filter on.\n\nDO NOT use the pseudo-property `associations.{objectType}` or `associated_{objectType}` for associations.\n\nYou can use the following properties to filter by the number of associated records:\n\nTickets - hs_num_associated_companies\nCompanies - num_associated_contacts, num_associated_deals\nContacts - num_associated_deals\nDeals - num_associated_contacts"
                    },
                    "operator": {
                      "type": "string",
                      "description": "Filter operator:\n  EQ, NEQ, LT, LTE, GT, GTE, BETWEEN, IN, NOT_IN, HAS_PROPERTY, NOT_HAS_PROPERTY, CONTAINS_TOKEN, NOT_CONTAINS_TOKEN\nUsage:\n  No additional parameters required for HAS_PROPERTY and NOT_HAS_PROPERTY"
                    },
                    "value": {
                      "type": "string",
                      "description": "Value parameter for single-value operators (EQ, NEQ, LT, LTE, GT, GTE, CONTAINS_TOKEN, NOT_CONTAINS_TOKEN)"
                    },
                    "highValue": {
                      "type": "string",
                      "description": "High value for range-based operators like BETWEEN"
                    },
                    "values": {
                      "type": "array",
                      "items": { "type": "string" },
                      "description": "Set of values for multi-value operators like IN and NOT_IN"
                    }
                  },
                  "required": ["propertyName", "operator"],
                  "additionalProperties": false,
                  "description": " Represents a filter condition for CRM object searches.\n Filters specify criteria that property values must meet to be included in search results.\n The available operators are defined in {@link PropertyFilterOperator}.\n"
                },
                "description": "Set of filters in this group. All filters combined with logical ANDThe filter is used with associatedWith for AND logic in the same FilterGroup."
              },
              "associatedWith": {
                "type": "array",
                "items": {
                  "type": "object",
                  "properties": {
                    "objectType": {
                      "type": "string",
                      "description": "Object type to find associations with.\n\nExample: \"companies\" to find records associated with a company"
                    },
                    "operator": {
                      "type": "string",
                      "enum": ["EQUAL", "IN", "NOT_EQUAL", "NOT_IN"],
                      "description": "Operator for association filtering:\n- EQUAL/NOT_EQUAL: Associated with the specified object.\n- IN/NOT_IN: Associated with any of the specified objects."
                    },
                    "objectIds": {
                      "type": "array",
                      "items": { "type": "string" },
                      "description": "DEPRECATED: Use objectIdValues instead.\nList of object IDs as strings to filter by.\nIf both objectIds and objectIdValues are provided, objectIdValues takes precedence.\nValid IDs are string representations of positive longs (e.g., '123', '456').\n\nFor EQUAL and NOT_EQUAL operators, provide exactly one ID eg. [\"123\"]\nFor IN and NOT_IN operators, provide multiple IDs eg. [\"123\", \"456\", \"789\"]"
                    },
                    "objectIdValues": {
                      "type": "array",
                      "items": { "type": "integer" },
                      "description": "List of object IDs to filter by.\n\nAt least one valid object ID must be provided.\nEach object ID greater than 0.\nMaximum of 100 object IDs allowed per association filter.\n\nFor EQUAL and NOT_EQUAL operators, provide exactly one ID eg. [123]\nFor IN and NOT_IN operators, provide multiple IDs eg. [123, 456, 789]"
                    }
                  },
                  "required": ["objectType", "operator"],
                  "additionalProperties": false
                },
                "description": "Association filters for finding records related to specific objects (e.g., \"contacts for company X\").\n\nWithin a single FilterGroup, all association filters are combined with AND logic.\nAssociation filters are also ANDed with any property filters in the same FilterGroup.\n\nExamples:\n- Find contacts associated with company 123:\n  [{\"objectType\": \"companies\", \"operator\": \"EQUAL\", \"objectIdValues\": [123]}]\n\n- Find contacts associated with company 123 OR 456 (use IN operator):\n  [{\"objectType\": \"companies\", \"operator\": \"IN\", \"objectIdValues\": [123, 456]}]\n\n- Find records associated with BOTH company 123 AND contact 456 (multiple filters):\n  [{\"objectType\": \"companies\", \"operator\": \"EQUAL\", \"objectIdValues\": [123]},\n   {\"objectType\": \"contacts\", \"operator\": \"EQUAL\", \"objectIdValues\": [456]}]\n\n- To search for company A OR company B, use separate FilterGroups (FilterGroups are ORed together)."
              }
            },
            "required": [],
            "additionalProperties": false,
            "description": " Represents a group of filters that are logically connected with AND operators.\n Multiple filter groups in a search request are combined with OR operators.\n"
          },
          "description": "* The filter groups that define the search criteria.\n * Filters that match ALL of several conditions (AND logic) should be put in the same filterGroup.\n * Filters that match AT LEAST ONE of several conditions (OR logic) should be put in a separate filterGroup."
        },
        "chatInsights": {
          "type": "object",
          "properties": {
            "userIntent": {
              "type": "string",
              "description": "The original intent or goal the user was trying to accomplish.\nSet to \"<unchanged>\" if the intent has already been submitted and hasn't changed.\nUse generic or anonymized references instead of specific names, emails, or other PII.\nReplace specific names with generalized references:\n- Company/Partner names: \"Bluleadz partner\" → \"partner A\"\n- Deal names: \"Tesla deal\" → \"deal A\"\n- Customer names: \"Acme Corp customer\" → \"company X\"\nImportant: Avoid adding extraneous information other than what the user provides in the current chat.\nMaximum 40 words."
            },
            "satisfaction": {
              "type": "string",
              "enum": ["DISSATISFIED", "NEUTRAL", "SATISFIED"],
              "description": "The user's satisfaction during the conversation.\n  Set to DISSATISFIED when the user struggled with a feature, mentioned an unmet need or showed dissatisfaction.\n  <dissatisfied_signals>\n  - User provides explicit correction: \"No, I meant...\", \"Actually, I want...\"\n  - User repeats query with different phrasing\n  - User states something is wrong: \"That's wrong\", \"That's not right\"\n  - User clarifies because agent misunderstood\n  </dissatisfied_signals>\n  Set to SATISFIED when the user expresses satisfaction, gratitude, or approval.\n  Set to NEUTRAL when the conversation flowed smoothly without strong emotional signals."
            }
          },
          "required": ["userIntent", "satisfaction"],
          "additionalProperties": false,
          "description": "Insights about the chat including user input and satisfaction level.\nIMPORTANT: MUST be provided for all scenarios."
        },
        "required": []
      },
      "required": ["objectType"],
      "additionalProperties": false,
      "$defs": {},
      "definitions": {}
    }"#;
    #[test]
    fn test_json() {
        let mut json_value = serde_json::from_str(HUBSPOT_SCHEMA).unwrap();
        normalize_schema(&mut json_value);
        let _sch: JsonSchema = serde_json::from_value(json_value).unwrap();
    }
}
