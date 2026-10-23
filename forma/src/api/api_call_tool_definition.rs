use std::collections::HashMap;

use agent::tool::ApiCallTool;
use derive::FormaDocs;
use genai_client::JsonSchema;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

fn default_method() -> String {
    "GET".to_string()
}

/// A tool that makes HTTP REST API calls.
///
/// Forma sends the request on behalf of the LLM once it has decided the
/// argument values. The LLM fills in:
///
/// - URL path parameters — declared inline in `url` using the same
///   `{{name: type "description"}}` template syntax used elsewhere in Forma
/// - Request body fields — declared via the `body` JSON schema
///
/// Secrets can be passed as header values using `$VAR_NAME` or `${VAR_NAME}`:
/// they are resolved from environment variables at runtime so they never need
/// to appear in config files.
///
/// Example (YAML):
/// ```yaml
/// type: api-call
/// url: 'https://api.example.com/users/{{user_id: string "the user to look up"}}'
/// method: GET
/// headers:
///   Authorization: "$API_TOKEN"
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, FormaDocs)]
pub struct ApiCallDefinition {
    /// The endpoint URL. Path parameters the LLM should fill are declared
    /// using `{{name: type "description"}}` — the same template syntax used
    /// elsewhere in Forma. For example:
    /// `https://api.example.com/users/{{user_id: string "the user's ID"}}`
    ///
    /// Supported types for URL parameters: `string`, `integer`, `number`, `boolean`.
    pub url: String,

    /// HTTP method. Case-insensitive (e.g. `GET`, `post`, `Put`).
    /// Supported values: GET, POST, PUT, DELETE, PATCH, HEAD.
    ///
    /// > Defaults to `GET`.
    #[serde(default = "default_method")]
    pub method: String,

    /// HTTP headers to attach to every request.
    /// Values starting with `$` are resolved from environment variables:
    /// `"$TOKEN"` or `"${TOKEN}"` → value of the `TOKEN` env var.
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Schema for the JSON request body. Must be an `object` schema.
    /// Omit for requests that carry no body (e.g. GET).
    /// The LLM will provide values for all fields described here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<JsonSchema>,
}

impl ObjectDefinition for ApiCallDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<ApiCallDefinition> for ApiCallTool {
    type Error = FormaError;

    fn try_from(value: ApiCallDefinition) -> Result<Self, Self::Error> {
        let method = value.method.as_str().try_into()?;
        Ok(ApiCallTool {
            url: value.url,
            method,
            body: value.body,
            headers: value.headers,
        })
    }
}
