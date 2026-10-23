use std::collections::HashMap;

use async_trait::async_trait;
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    callable::CallContext,
    template::{extract_placeholder_names, extract_template_schema, render_template},
    tool::ToolTrait,
};
#[cfg(not(feature = "airtight"))]
use tracing::info;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
}

impl TryFrom<&str> for HttpMethod {
    type Error = FormaError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "DELETE" => Ok(Self::Delete),
            "PATCH" => Ok(Self::Patch),
            "HEAD" => Ok(Self::Head),
            other => Err(FormaError::Input(format!("unknown HTTP method '{other}'"))),
        }
    }
}

/// Calls an external REST API endpoint. The LLM fills in URL path parameters
/// and the request body based on their schemas.
///
/// URL parameters use the same `{{name: type "description"}}` template syntax
/// as the rest of Forma. For example:
/// `https://api.example.com/users/{{user_id: string "the user's ID"}}/posts`
///
/// Headers are static and may reference environment variables for secrets
/// using `$VAR` or `${VAR}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCallTool {
    /// Endpoint URL. May contain `{{param: type "description"}}` placeholders
    /// for path parameters the LLM will fill.
    pub url: String,
    /// HTTP method to use.
    pub method: HttpMethod,
    /// Optional object schema describing the request body fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<JsonSchema>,
    /// Static headers. Values may use `$VAR` or `${VAR}` for env-var secrets.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn resolve_header_value(value: &str) -> Result<String, FormaError> {
    if let Some(rest) = value.strip_prefix('$') {
        let var_name = rest.trim_matches(|c| c == '{' || c == '}');
        std::env::var(var_name)
            .map_err(|_| FormaError::Input(format!("environment variable '{var_name}' is not set")))
    } else {
        Ok(value.to_string())
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for ApiCallTool {
    fn check(&self) -> Result<(), FormaError> {
        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Err(FormaError::Input(
                "ApiCallTool URL must start with http:// or https://".to_string(),
            ));
        }
        // Validates template syntax and param types in one shot
        extract_template_schema(&self.url)?;

        if let Some(body) = &self.body
            && body.schema_type != Some(SchemaType::Object)
        {
            return Err(FormaError::Input(
                "ApiCall body schema must be of type 'object'".to_string(),
            ));
        }

        // URL param names must not collide with body field names
        let url_params = extract_placeholder_names(&self.url)?;
        if let Some(body) = &self.body
            && let Some(props) = &body.properties
        {
            for param in &url_params {
                if props.contains_key(param) {
                    return Err(FormaError::Input(format!(
                        "URL parameter '{param}' conflicts with a body field of the same name"
                    )));
                }
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let Value::Object(input_obj) = input else {
            warn!("ApiCallTool received non-object input");
            return Err(FormaError::Input(
                "ApiCallTool input must be a JSON object".to_string(),
            ));
        };

        let session: HashMap<String, Value> = input_obj.clone().into_iter().collect();
        let url = render_template(&self.url, &session)
            .map_err(|e| FormaError::Input(format!("URL template error: {e}")))?;

        let url_params = extract_placeholder_names(&self.url)?;

        let mut header_map = HeaderMap::new();
        for (name, value) in &self.headers {
            let resolved = resolve_header_value(value)?;
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| FormaError::Input(format!("invalid header name '{name}': {e}")))?;
            let header_value = HeaderValue::from_str(&resolved).map_err(|e| {
                FormaError::Input(format!("invalid value for header '{name}': {e}"))
            })?;
            header_map.insert(header_name, header_value);
        }

        let client = crate::tool::http::client();
        let req_builder = match self.method {
            HttpMethod::Get => client.get(&url),
            HttpMethod::Post => client.post(&url),
            HttpMethod::Put => client.put(&url),
            HttpMethod::Delete => client.delete(&url),
            HttpMethod::Patch => client.patch(&url),
            HttpMethod::Head => client.head(&url),
        };
        let req_builder = req_builder.headers(header_map);

        let _req_builder = if self.body.is_some() {
            let body_obj: serde_json::Map<String, Value> = input_obj
                .into_iter()
                .filter(|(k, _)| !url_params.contains(k))
                .collect();
            req_builder.json(&Value::Object(body_obj))
        } else {
            req_builder
        };

        #[cfg(not(feature = "airtight"))]
        {
            info!(url = %url, method = ?self.method, "calling external API");
            let resp = _req_builder.send().await?;
            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                warn!(url = %url, %status, error = %error_body, "external API returned error");
                return Err(FormaError::ApiCall {
                    url: Some(url),
                    msg: format!("HTTP {status}: {error_body}"),
                });
            }
            let body = resp.json::<Value>().await.unwrap_or(Value::Null);
            return Ok(Some(body));
        }
        #[cfg(feature = "airtight")]
        Ok(Some(Value::Null))
    }
}

impl TryFrom<ApiCallTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: ApiCallTool) -> Result<Self, FormaError> {
        // URL template already encodes param names, types, and descriptions
        let url_schema = extract_template_schema(&item.url)?;
        let mut properties = url_schema.properties.unwrap_or_default();
        let mut required = url_schema.required.unwrap_or_default();

        if let Some(body) = item.body {
            if let Some(body_props) = body.properties {
                for (k, v) in body_props {
                    properties.insert(k, v);
                }
            }
            if let Some(body_required) = body.required {
                for r in body_required {
                    if !required.contains(&r) {
                        required.push(r);
                    }
                }
            }
        }

        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: if properties.is_empty() {
                None
            } else {
                Some(properties)
            },
            required: if required.is_empty() {
                None
            } else {
                Some(required)
            },
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::callable::CallContext;

    use super::*;
    use db::NoneDB;
    use genai_client::JsonSchema;
    use genai_client::SSEvent;
    use protocols::DeploymentCredentials;
    use protocols::EncryptedTokens;
    use protocols::SessionData;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock, mpsc};
    fn make_ctx() -> CallContext<NoneDB> {
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        CallContext {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "test".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        }
    }

    fn simple_get(url: &str) -> ApiCallTool {
        ApiCallTool {
            url: url.to_string(),
            method: HttpMethod::Get,
            body: None,
            headers: HashMap::new(),
        }
    }

    fn object_body(fields: &[(&str, &str)]) -> JsonSchema {
        let mut properties = HashMap::new();
        let mut required = Vec::new();
        for (name, desc) in fields {
            properties.insert(
                name.to_string(),
                JsonSchema {
                    schema_type: Some(SchemaType::String),
                    description: Some(desc.to_string()),
                    ..Default::default()
                },
            );
            required.push(name.to_string());
        }
        JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(properties),
            required: Some(required),
            ..Default::default()
        }
    }

    // ── HttpMethod ────────────────────────────────────────────────────────────

    #[test]
    fn http_method_from_str_case_insensitive() {
        assert_eq!(HttpMethod::try_from("get").unwrap(), HttpMethod::Get);
        assert_eq!(HttpMethod::try_from("POST").unwrap(), HttpMethod::Post);
        assert_eq!(HttpMethod::try_from("Put").unwrap(), HttpMethod::Put);
        assert_eq!(HttpMethod::try_from("delete").unwrap(), HttpMethod::Delete);
        assert_eq!(HttpMethod::try_from("PATCH").unwrap(), HttpMethod::Patch);
        assert_eq!(HttpMethod::try_from("head").unwrap(), HttpMethod::Head);
    }

    #[test]
    fn http_method_unknown_is_err() {
        assert!(HttpMethod::try_from("OPTIONS").is_err());
        assert!(HttpMethod::try_from("").is_err());
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn check_valid_get_no_params() {
        let t = simple_get("https://api.example.com/users");
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&t).is_ok());
    }

    #[test]
    fn check_valid_get_with_template_param() {
        let t = simple_get(r#"https://api.example.com/users/{{user_id: string "the user's ID"}}"#);
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&t).is_ok());
    }

    #[test]
    fn check_non_http_url_is_err() {
        let t = simple_get("ftp://example.com");
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    #[test]
    fn check_http_url_allowed() {
        let t = simple_get("http://localhost:3000/api");
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&t).is_ok());
    }

    #[test]
    fn check_body_must_be_object_type() {
        let tool = ApiCallTool {
            url: "https://api.example.com".to_string(),
            method: HttpMethod::Post,
            body: Some(JsonSchema {
                schema_type: Some(SchemaType::String),
                ..Default::default()
            }),
            headers: HashMap::new(),
        };
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&tool).is_err());
    }

    #[test]
    fn check_url_param_conflict_with_body_field_is_err() {
        let tool = ApiCallTool {
            url: r#"https://api.example.com/users/{{id: string "the id"}}"#.to_string(),
            method: HttpMethod::Post,
            body: Some(object_body(&[("id", "some id")])),
            headers: HashMap::new(),
        };
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&tool).is_err());
    }

    #[test]
    fn check_url_param_and_body_no_conflict_is_ok() {
        let tool = ApiCallTool {
            url: r#"https://api.example.com/users/{{user_id: string "the user's ID"}}"#.to_string(),
            method: HttpMethod::Post,
            body: Some(object_body(&[("name", "user name")])),
            headers: HashMap::new(),
        };
        assert!(<ApiCallTool as ToolTrait<NoneDB>>::check(&tool).is_ok());
    }

    // ── TryFrom<ApiCallTool> for JsonSchema ──────────────────────────────────────

    #[test]
    fn schema_no_params_no_body() {
        let schema = JsonSchema::try_from(simple_get("https://api.example.com")).unwrap();
        assert_eq!(schema.schema_type, Some(SchemaType::Object));
        assert!(schema.properties.is_none());
        assert!(schema.required.is_none());
    }

    #[test]
    fn schema_url_params_extracted_with_types() {
        let tool = simple_get(
            r#"https://api.example.com/{{user_id: string "user ID"}}/posts/{{post_id: integer "post ID"}}"#,
        );
        let schema = JsonSchema::try_from(tool).unwrap();
        let props = schema.properties.unwrap();
        assert_eq!(props["user_id"].schema_type, Some(SchemaType::String));
        assert_eq!(props["post_id"].schema_type, Some(SchemaType::Integer));
        let req = schema.required.unwrap();
        assert!(req.contains(&"user_id".to_string()));
        assert!(req.contains(&"post_id".to_string()));
    }

    #[test]
    fn schema_body_fields_merged() {
        let tool = ApiCallTool {
            url: r#"https://api.example.com/users/{{user_id: string "user ID"}}"#.to_string(),
            method: HttpMethod::Post,
            body: Some(object_body(&[("name", "the name"), ("email", "the email")])),
            headers: HashMap::new(),
        };
        let schema = JsonSchema::try_from(tool).unwrap();
        let props = schema.properties.unwrap();
        assert!(props.contains_key("user_id"));
        assert!(props.contains_key("name"));
        assert!(props.contains_key("email"));
        let req = schema.required.unwrap();
        assert!(req.contains(&"user_id".to_string()));
        assert!(req.contains(&"name".to_string()));
        assert!(req.contains(&"email".to_string()));
    }

    // ── resolve_header_value ─────────────────────────────────────────────────

    #[test]
    fn resolve_static_header() {
        assert_eq!(
            resolve_header_value("Bearer abc123").unwrap(),
            "Bearer abc123"
        );
    }

    #[test]
    fn resolve_env_var_header() {
        unsafe { std::env::set_var("TEST_FORMA_TOKEN", "secret") };
        assert_eq!(resolve_header_value("$TEST_FORMA_TOKEN").unwrap(), "secret");
        assert_eq!(
            resolve_header_value("${TEST_FORMA_TOKEN}").unwrap(),
            "secret"
        );
    }

    #[test]
    fn resolve_missing_env_var_is_err() {
        assert!(resolve_header_value("$FORMA_DEFINITELY_NOT_SET_VAR_XYZ").is_err());
    }

    // ── execute ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_non_object_input_is_err() {
        let tool = simple_get("https://httpbin.org/get");
        let result = tool
            .execute(serde_json::json!("not an object"), make_ctx())
            .await;
        assert!(result.is_err());
    }

    // ── execute (airtight — no real HTTP calls) ───────────────────────────────

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_valid_get_returns_ok() {
        let result = simple_get("https://api.example.com/users")
            .execute(serde_json::json!({}), make_ctx())
            .await;
        assert_eq!(result.unwrap(), Some(Value::Null));
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_url_param_filled_from_input() {
        let tool = simple_get(r#"https://api.example.com/users/{{user_id: string "user id"}}"#);
        let result = tool
            .execute(serde_json::json!({"user_id": "42"}), make_ctx())
            .await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_missing_url_param_is_err() {
        let tool = simple_get(r#"https://api.example.com/users/{{user_id: string "user id"}}"#);
        let result = tool.execute(serde_json::json!({}), make_ctx()).await;
        assert!(result.is_err());
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_missing_env_var_header_is_err() {
        let tool = ApiCallTool {
            url: "https://api.example.com/data".to_string(),
            method: HttpMethod::Get,
            body: None,
            headers: [(
                "authorization".to_string(),
                "$FORMA_MISSING_TOKEN_XYZ".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let result = tool.execute(serde_json::json!({}), make_ctx()).await;
        assert!(result.is_err());
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_invalid_header_name_is_err() {
        let tool = ApiCallTool {
            url: "https://api.example.com/data".to_string(),
            method: HttpMethod::Get,
            body: None,
            headers: [("x bad header name".to_string(), "value".to_string())]
                .into_iter()
                .collect(),
        };
        let result = tool.execute(serde_json::json!({}), make_ctx()).await;
        assert!(result.is_err());
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_post_with_body_and_url_param_ok() {
        let tool = ApiCallTool {
            url: r#"https://api.example.com/users/{{user_id: string "user id"}}"#.to_string(),
            method: HttpMethod::Post,
            body: Some(object_body(&[
                ("name", "user name"),
                ("email", "user email"),
            ])),
            headers: HashMap::new(),
        };
        let result = tool
            .execute(
                serde_json::json!({"user_id": "42", "name": "Alice", "email": "a@example.com"}),
                make_ctx(),
            )
            .await;
        assert_eq!(result.unwrap(), Some(Value::Null));
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_all_http_methods_ok() {
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Delete,
            HttpMethod::Patch,
            HttpMethod::Head,
        ] {
            let tool = ApiCallTool {
                url: "https://api.example.com/data".to_string(),
                method,
                body: None,
                headers: HashMap::new(),
            };
            let result = tool.execute(serde_json::json!({}), make_ctx()).await;
            assert!(result.is_ok());
        }
    }
}
