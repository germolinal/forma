// Tool based on https://documentation.mailgun.com/docs/mailgun/api-reference/api-overview

use std::collections::HashMap;

use async_trait::async_trait;
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CallContext, ToolTrait};

#[cfg(not(feature = "airtight"))]
use tracing::{info, warn};

/// Name of the environment variable holding the Mailgun private API key.
const API_KEY_ENV: &str = "MAILGUN_API_KEY";

/// Defines the endpoint to use
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailgunLocation {
    /// Points to https://api.mailgun.net/
    Us,
    /// Points to https://api.eu.mailgun.net/
    Eu,
}

impl MailgunLocation {
    fn endpoint(&self) -> &'static str {
        match self {
            Self::Eu => "api.eu.mailgun.net",
            Self::Us => "api.mailgun.net",
        }
    }
}

/// Sends an email using the Mailgun API.
/// Requires the following Env variable
/// - MAILGUN_API_KEY
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailgunTool {
    /// Whether to point to the US or EU Mailgun endpoint.
    pub location: MailgunLocation,
    /// The name and email of the sender
    /// Can be something like 'Bob <bob@my_company.com>'
    pub from: String,
    /// The domain (eg., my_company.com).
    /// Must match the one in the email set in
    /// `from`
    pub domain: String,
}

/// The inputs the LLM fills in when calling the tool. Mirrors the schema
/// produced by `TryFrom<MailgunTool> for JsonSchema`.
#[derive(Debug, Clone, Deserialize)]
struct SendEmailRequest {
    /// List of recipients
    to: Vec<String>,
    /// List of carbon-copied users
    #[serde(default)]
    cc: Vec<String>,
    /// The subject of the email
    subject: String,
    /// The content of the email
    content: String,
}

/// Helper: a `{ "type": "string", "description": ... }` schema.
pub(crate) fn string_schema(description: &str) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::String),
        description: Some(description.to_string()),
        ..Default::default()
    }
}

/// Helper: an array-of-strings schema.
pub(crate) fn string_array_schema(description: &str) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Array),
        description: Some(description.to_string()),
        items: Some(Box::new(JsonSchema {
            schema_type: Some(SchemaType::String),
            ..Default::default()
        })),
        ..Default::default()
    }
}

impl TryFrom<MailgunTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_item: MailgunTool) -> Result<JsonSchema, FormaError> {
        // The `from`, `domain` and `location` are fixed configuration on the
        // tool itself, so they are not part of the schema the LLM fills in.
        let mut props = HashMap::new();
        props.insert(
            "to".to_string(),
            string_array_schema("List of recipient email addresses. Each entry may use the 'Name <email@domain.com>' form."),
        );
        props.insert(
            "cc".to_string(),
            string_array_schema("Optional list of carbon-copy email addresses."),
        );
        props.insert("subject".to_string(), string_schema("The email subject."));
        props.insert(
            "content".to_string(),
            string_schema("The plain-text body of the email."),
        );

        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props),
            required: Some(vec![
                "to".to_string(),
                "subject".to_string(),
                "content".to_string(),
            ]),
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for MailgunTool {
    fn check(&self) -> Result<(), FormaError> {
        if self.domain.trim().is_empty() {
            return Err(FormaError::Input(
                "MailgunTool: 'domain' must not be empty".to_string(),
            ));
        }
        if self.from.trim().is_empty() {
            return Err(FormaError::Input(
                "MailgunTool: 'from' must not be empty".to_string(),
            ));
        }
        // The sending domain must appear in the `from` address, per Mailgun's
        // requirement that you may only send from your configured domain.
        if !self.from.contains(&self.domain) {
            return Err(FormaError::Input(format!(
                "MailgunTool: 'from' ({}) must use the configured domain '{}'",
                self.from, self.domain
            )));
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let req: SendEmailRequest = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("MailgunTool: invalid input: {e}")))?;

        if req.to.is_empty() {
            return Err(FormaError::Input(
                "MailgunTool: at least one 'to' recipient is required".to_string(),
            ));
        }

        // Auth is HTTP Basic with username `api` and the private key as password.
        let api_key = std::env::var(API_KEY_ENV).map_err(|e| FormaError::MissingEnvVar {
            key: API_KEY_ENV.to_string(),
            msg: e.to_string(),
        })?;

        let url = format!(
            "https://{}/v3/{}/messages",
            self.location.endpoint(),
            self.domain
        );

        // urlencoded form. Repeating the `to`/`cc` keys sends multiple
        // recipients, which Mailgun accepts.
        let mut form: Vec<(&str, String)> = Vec::with_capacity(req.to.len() + req.cc.len() + 3);
        form.push(("from", self.from.clone()));
        for to in &req.to {
            form.push(("to", to.clone()));
        }
        for cc in &req.cc {
            form.push(("cc", cc.clone()));
        }
        form.push(("subject", req.subject.clone()));
        form.push(("text", req.content.clone()));

        #[cfg(not(feature = "airtight"))]
        {
            info!(url = %url, recipients = req.to.len(), "sending email via Mailgun");
            let client = crate::tool::http::client();
            let resp = client
                .post(&url)
                .basic_auth("api", Some(api_key))
                .form(&form)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                warn!(url = %url, %status, error = %error_body, "Mailgun returned error");
                return Err(FormaError::ApiCall {
                    url: Some(url),
                    msg: format!("HTTP {status}: {error_body}"),
                });
            }
            let body = resp.json::<Value>().await.unwrap_or(Value::Null);
            return Ok(Some(body));
        }
        #[cfg(feature = "airtight")]
        {
            // Airtight builds make no network calls.
            let _ = (url, form, api_key);
            Ok(Some(Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callable::CallContext;
    use db::NoneDB;
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock, mpsc};

    fn make_ctx() -> CallContext<NoneDB> {
        let (tx, _rx) = mpsc::unbounded_channel::<genai_client::SSEvent>();
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

    fn tool() -> MailgunTool {
        MailgunTool {
            location: MailgunLocation::Us,
            from: "Bob <bob@my_company.com>".to_string(),
            domain: "my_company.com".to_string(),
        }
    }

    // ── endpoint ──────────────────────────────────────────────────────────────

    #[test]
    fn endpoint_maps_by_location() {
        assert_eq!(MailgunLocation::Us.endpoint(), "api.mailgun.net");
        assert_eq!(MailgunLocation::Eu.endpoint(), "api.eu.mailgun.net");
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn check_valid() {
        assert!(<MailgunTool as ToolTrait<NoneDB>>::check(&tool()).is_ok());
    }

    #[test]
    fn check_empty_domain_is_err() {
        let mut t = tool();
        t.domain = "  ".to_string();
        assert!(<MailgunTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    #[test]
    fn check_from_without_domain_is_err() {
        let mut t = tool();
        t.from = "Bob <bob@other.com>".to_string();
        assert!(<MailgunTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    // ── schema ────────────────────────────────────────────────────────────────

    #[test]
    fn schema_has_expected_fields() {
        let schema = JsonSchema::try_from(tool()).unwrap();
        assert_eq!(schema.schema_type, Some(SchemaType::Object));
        let props = schema.properties.unwrap();
        assert_eq!(props["to"].schema_type, Some(SchemaType::Array));
        assert_eq!(
            props["to"].items.as_ref().unwrap().schema_type,
            Some(SchemaType::String)
        );
        assert_eq!(props["cc"].schema_type, Some(SchemaType::Array));
        assert_eq!(props["subject"].schema_type, Some(SchemaType::String));
        assert_eq!(props["content"].schema_type, Some(SchemaType::String));

        let required = schema.required.unwrap();
        assert!(required.contains(&"to".to_string()));
        assert!(required.contains(&"subject".to_string()));
        assert!(required.contains(&"content".to_string()));
        // cc is optional
        assert!(!required.contains(&"cc".to_string()));
    }

    // ── execute (input validation — no network / env needed) ──────────────────

    #[tokio::test]
    async fn execute_non_object_input_is_err() {
        let result = tool()
            .execute(serde_json::json!("not an object"), make_ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_empty_recipients_is_err() {
        let result = tool()
            .execute(
                serde_json::json!({"to": [], "subject": "hi", "content": "hello"}),
                make_ctx(),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_missing_content_is_err() {
        let result = tool()
            .execute(
                serde_json::json!({"to": ["a@b.com"], "subject": "hi"}),
                make_ctx(),
            )
            .await;
        assert!(result.is_err());
    }

    // ── execute (airtight — no real HTTP calls) ───────────────────────────────

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_valid_returns_ok() {
        unsafe { std::env::set_var(API_KEY_ENV, "test-key") };
        let result = tool()
            .execute(
                serde_json::json!({
                    "to": ["alice@example.com"],
                    "cc": ["carol@example.com"],
                    "subject": "Hi",
                    "content": "Hello there"
                }),
                make_ctx(),
            )
            .await;
        assert_eq!(result.unwrap(), Some(Value::Null));
    }
}
