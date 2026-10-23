use async_trait::async_trait;

use genai_client::JsonSchema;
use protocols::{FormaDB, FormaError};
use reqwest::{
    RequestBuilder, Response, StatusCode,
    header::{ACCEPT, HeaderValue},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    callable::CallContext,
    tool::{ToolTrait, http},
};
use tracing::info;

/// What a client says it accepts on every request to an MCP server (the
/// streamable HTTP transport): the server may answer with JSON or with a
/// stream of events, and some refuse a request that does not say so.
const ACCEPT_JSON_AND_EVENTS: &str = "application/json, text/event-stream";

/// Where a server names the session `initialize` opened. Every later request
/// of that session has to carry it.
const SESSION_ID_HEADER: &str = "mcp-session-id";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: JsonSchema,
    pub server_url: String,
}

/// How the tool an MCP server stands in as starts, when an agent is compiled
/// without connecting to its servers (forma's `McpAccess::Unconnected`): the
/// server's URL follows, with every character a tool name cannot hold made a
/// `-`. Such an agent is checked and described, never run.
pub const UNCONNECTED_TOOL_PREFIX: &str = "unconnected-mcp-";

impl McpTool {
    /// Whether this is the placeholder an unconnected server stands in as,
    /// rather than a tool the server offered.
    pub fn is_unconnected(&self) -> bool {
        self.name.starts_with(UNCONNECTED_TOOL_PREFIX)
    }
}

impl TryFrom<McpTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: McpTool) -> Result<JsonSchema, FormaError> {
        Ok(item.input_schema)
    }
}

impl McpTool {
    /// Opens a session with the server, as the user. This is where their
    /// credentials are checked: a server that does not accept them answers 401
    /// or 403, and the call stops here, before the tool is asked for anything,
    /// saying that the user has to connect to the server again.
    ///
    /// Returns the session the server opened, if it named one.
    async fn init_server(&self, token: &str) -> Result<Option<HeaderValue>, FormaError> {
        info!(server_url = %self.server_url, "initializing external MCP server");
        let init_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "forma", "version": "1.0" }
            }
        });

        let resp = self
            .request(token, None)
            .json(&init_body)
            .send()
            .await
            .map_err(|e| FormaError::ApiCall {
                url: Some(self.server_url.clone()),
                msg: format!("MCP initialize failed: {e}"),
            })?;
        let resp = self.accepted(resp, "initialize").await?;
        let session = resp.headers().get(SESSION_ID_HEADER).cloned();

        // initialized notification (fire and forget, no response expected)
        let _ = self
            .request(token, session.as_ref())
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .send()
            .await;
        Ok(session)
    }

    /// A request to the server, as the user, within `session` when there is
    /// one. It goes out on the client every tool shares: the connection is
    /// shared, the credentials on it are the user's own.
    fn request(&self, token: &str, session: Option<&HeaderValue>) -> RequestBuilder {
        let request = http::client()
            .post(&self.server_url)
            .bearer_auth(token)
            .header(ACCEPT, ACCEPT_JSON_AND_EVENTS);
        match session {
            Some(id) => request.header(SESSION_ID_HEADER, id.clone()),
            None => request,
        }
    }

    /// `resp`, when the server answered `step`. A refusal of the user's
    /// credentials stops the call saying so, so that they can connect again;
    /// any other failure stops it with what the server said.
    async fn accepted(&self, resp: Response, step: &str) -> Result<Response, FormaError> {
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(FormaError::OAuthError {
                url: self.server_url.clone(),
                msg: format!(
                    "the MCP server refused the user's credentials (HTTP {status}): \
                     they need to connect to it again"
                ),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body: String = body.chars().take(500).collect();
            return Err(FormaError::ApiCall {
                url: Some(self.server_url.clone()),
                msg: format!("MCP {step} failed: HTTP {status}: {body}"),
            });
        }
        Ok(resp)
    }

    fn build_call_body(&self, input: Value) -> Result<Value, FormaError> {
        let arguments = match input {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                return Err(FormaError::Input(format!(
                    "MCP tool '{}' expected an object input, got: {other}",
                    self.name
                )));
            }
        };

        let mut params = serde_json::json!({ "name": self.name });
        if let Some(args) = arguments {
            params["arguments"] = Value::Object(args);
        }

        let call_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": params
        });
        Ok(call_body)
    }

    fn extract_result(&self, response: Value) -> Result<Option<Value>, FormaError> {
        let result = response.get("result").ok_or_else(|| FormaError::ApiCall {
            url: None,
            msg: format!(
                "MCP response missing 'result' field: {}",
                response
                    .get("error")
                    .and_then(|e| e["message"].as_str())
                    .unwrap_or("unknown error")
            ),
        })?;

        // Note: spec uses camelCase "isError", not snake_case
        if result.get("isError") == Some(&Value::Bool(true)) {
            let msg = result["content"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(FormaError::ApiCall {
                url: None,
                msg: format!("MCP tool '{}' returned error: {msg}", self.name),
            });
        }

        let value = if let Some(structured) = result.get("structuredContent") {
            structured.clone()
        } else {
            let texts: Vec<String> = result["content"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|c| c["text"].as_str().map(String::from))
                .collect();
            Value::String(texts.join("\n"))
        };

        Ok(Some(value))
    }

    async fn call(
        &self,
        token: &str,
        session: Option<&HeaderValue>,
        call_body: &Value,
    ) -> Result<Value, FormaError> {
        info!(tool = %self.name, server_url = %self.server_url, "invoking external MCP tool");
        let response = self
            .request(token, session)
            .json(&call_body)
            .send()
            .await
            .map_err(|e| FormaError::ApiCall {
                url: Some(self.server_url.clone()),
                msg: format!("MCP tools/call '{}' failed: {e}", self.name),
            })?;
        let response = self.accepted(response, "tools/call").await?;

        let result: Value = response.json().await.map_err(|e| FormaError::ApiCall {
            url: Some(self.server_url.clone()),
            msg: format!("MCP response parse failed: {e}"),
        })?;

        Ok(result)
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for McpTool {
    fn check(&self) -> Result<(), FormaError> {
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        if self.is_unconnected() {
            // Only an agent compiled for checking holds one, and those are
            // never run; refused anyway rather than sent to the server.
            return Err(FormaError::Input(format!(
                "the MCP server at {} was not connected when this agent was compiled",
                self.server_url
            )));
        }
        info!(tool = %self.name, server_url = %self.server_url, "executing MCP tool");
        let token = ctx.get_oauth_token(&self.server_url).await?;

        // The user's credentials are checked first, by the server, when the
        // session opens: nothing is called with credentials it refuses.
        let session = self.init_server(&token).await?;

        let params = self.build_call_body(input)?;

        let response = self.call(&token, session.as_ref(), &params).await?;

        self.extract_result(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> McpTool {
        McpTool {
            name: "lookup".to_string(),
            description: None,
            input_schema: JsonSchema::default(),
            server_url: "https://mcp.example.com/mcp".to_string(),
        }
    }

    fn answer(status: u16, body: &'static str) -> Response {
        // The `http` crate, not the tools' client module of the same name.
        ::http::Response::builder()
            .status(status)
            .body(body)
            .expect("a valid response")
            .into()
    }

    #[tokio::test]
    async fn refused_credentials_stop_the_call() {
        for status in [401, 403] {
            match tool().accepted(answer(status, ""), "initialize").await {
                Err(FormaError::OAuthError { url, msg }) => {
                    assert_eq!(url, "https://mcp.example.com/mcp");
                    assert!(msg.contains("connect to it again"), "{msg}");
                }
                other => panic!("HTTP {status} gave {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn another_failure_says_what_the_server_said() {
        match tool()
            .accepted(answer(404, "session not found"), "tools/call")
            .await
        {
            Err(FormaError::ApiCall { msg, .. }) => {
                assert!(msg.contains("tools/call"), "{msg}");
                assert!(msg.contains("404"), "{msg}");
                assert!(msg.contains("session not found"), "{msg}");
            }
            other => panic!("HTTP 404 gave {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_answer_goes_through() {
        let resp = tool().accepted(answer(200, "{}"), "initialize").await;
        assert_eq!(resp.expect("accepted").status(), StatusCode::OK);
    }

    #[test]
    fn every_request_carries_the_users_token_and_the_session() {
        let request = tool()
            .request("user-token", Some(&HeaderValue::from_static("session-1")))
            .build()
            .expect("a valid request");
        let headers = request.headers();
        assert_eq!(headers["authorization"], "Bearer user-token");
        assert_eq!(headers["accept"], ACCEPT_JSON_AND_EVENTS);
        assert_eq!(headers[SESSION_ID_HEADER], "session-1");

        // Before the session is open, there is none to carry.
        let request = tool().request("user-token", None).build().unwrap();
        assert!(request.headers().get(SESSION_ID_HEADER).is_none());
    }
}
