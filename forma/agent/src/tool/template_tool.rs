use std::collections::HashMap;

use crate::{
    callable::CallContext,
    template::{extract_template_schema, render_template},
    tool::ToolTrait,
};

use async_trait::async_trait;
use genai_client::JsonSchema;

use protocols::{FormaDB, FormaError};
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info;

/// This tool asks an LLM to fill the missing fields on a template.
///
/// This tool receives one or more templates, all of which share the same
/// field specifications (e.g., they all have the same fields with the same
/// types and descriptions). At runtime, it randomly selects one of the
/// templates and asks the LLM to populate it.
///
/// This tool is useful for defining pre-defined responses that should
/// not feel too robotic (e.g., "The current time is {{time: string}}" and "It is
/// {{time: string}}") and for mocking up responses (e.g., "I checked your
/// email, there are no new messages from {{address}}" vs "You do not have
/// any new messages from {{address}}")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateTool {
    /// The templates to choose from. For instance,
    ///
    /// > hello {{name: string "the name of the user" }}, nice to meet you! The current time is {{time: string "the current time"}}
    pub templates: Vec<String>,
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for TemplateTool {
    fn check(&self) -> Result<(), FormaError> {
        if self.templates.is_empty() {
            return Err(FormaError::Input(
                "TemplateTool must have at least one template".to_string(),
            ));
        }

        let first_schema = extract_template_schema(&self.templates[0])?;

        for template in &self.templates[1..] {
            let schema = extract_template_schema(template)?;
            if schema != first_schema {
                return Err(FormaError::Input(
                    "all templates in a TemplateTool must have the same schema".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        let template = self
            .templates
            .choose(&mut rand::rng())
            .ok_or_else(|| FormaError::Input("TemplateTool has no templates".to_string()))?;

        info!(template = %template, "executing template tool");
        if let Value::Object(v) = input {
            let mut data: HashMap<String, Value> = HashMap::new();
            for (key, value) in v {
                data.insert(key, value);
            }
            let ret = render_template(template, &data)?;
            let ret = serde_json::to_value(&ret).expect("could not turn string into value");
            Ok(Some(ret))
        } else {
            tracing::error!("template tool received non-object input");
            Err("value received in template tool is not a map"
                .to_string()
                .into())
        }
    }
}

impl TryFrom<TemplateTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: TemplateTool) -> Result<Self, FormaError> {
        let first = item
            .templates
            .first()
            .ok_or_else(|| FormaError::Input("TemplateTool has no templates".to_string()))?;
        extract_template_schema(first)
    }
}

#[cfg(test)]
mod tests {
    use db::NoneDB;
    use genai_client::SSEvent;
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::sync::{RwLock, mpsc};

    use crate::callable::CallContext;

    use super::*;

    fn tool(templates: &[&str]) -> TemplateTool {
        TemplateTool {
            templates: templates.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_check_empty_templates_is_err() {
        let t = tool(&[]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        let err = check.unwrap_err().to_string();
        assert!(err.contains("at least one template"));
    }

    #[test]
    fn test_check_single_valid_template() {
        let t = tool(&["hello {{name: string \"the name\"}}"]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_single_template_no_variables() {
        let t = tool(&["Hello world"]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_single_template_invalid_type() {
        let t = tool(&["Hello {{name: array \"desc\"}}"]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_multiple_templates_same_schema() {
        let t = tool(&[
            "Hi {{name: string \"the name\"}}!",
            "Hey there, {{name: string \"the name\"}}.",
        ]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_multiple_templates_different_fields_is_err() {
        let t = tool(&[
            "Hello {{name: string \"the name\"}}",
            "Hello {{age: number \"the age\"}}",
        ]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_multiple_templates_same_field_different_description_is_err() {
        let t = tool(&[
            "Hello {{name: string \"full name\"}}",
            "Hello {{name: string \"first name\"}}",
        ]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_multiple_templates_field_order_independent() {
        // Same fields in different order within the template text — schemas are equivalent
        let t = tool(&[
            "Hi {{name: string \"the name\"}}, you are {{age: number \"the age\"}}!",
            "Age: {{age: number \"the age\"}}, name: {{name: string \"the name\"}}.",
        ]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_later_template_invalid_type_is_err() {
        let t = tool(&[
            "Hello {{name: string \"the name\"}}",
            "Hello {{name: object \"the name\"}}",
        ]);
        let check = <TemplateTool as ToolTrait<NoneDB>>::check(&t);
        assert!(check.is_err());
    }

    // ── execute ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_execute_renders_template() {
        let t = tool(&["Hello {{name}}!"]);
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        match t
            .execute(serde_json::json!({"name": "Alice"}), ctx)
            .await
            .unwrap()
        {
            Some(result) => assert_eq!(result, serde_json::json!("Hello Alice!")),
            None => unreachable!(),
        }
    }

    #[tokio::test]
    async fn test_execute_non_object_input_is_err() {
        let t = tool(&["Hello {{name}}!"]);
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        let result = t.execute(serde_json::json!("not an object"), ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_multiple_templates_picks_valid_one() {
        let t = tool(&["Hi {{name}}!", "Hey {{name}}!"]);
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        match t
            .execute(serde_json::json!({"name": "Bob"}), ctx)
            .await
            .unwrap()
        {
            Some(result) => {
                let s = result.as_str().unwrap();
                assert!(s == "Hi Bob!" || s == "Hey Bob!");
            }
            None => unreachable!(),
        }
    }
}
