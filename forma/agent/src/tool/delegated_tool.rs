use crate::{callable::CallContext, tool::ToolTrait};
use async_trait::async_trait;

use genai_client::JsonSchema;
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};

/// A tool that delegates execution to a remote API endpoint.
/// When calling this tool, Forma will send a request asking the
/// client not only to validate the inputs but also to execute
/// it. The session will be suspended until the agent receives the
/// output of this tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegatedTool {
    pub input_schema: JsonSchema,
}

impl TryFrom<DelegatedTool> for JsonSchema {
    type Error = FormaError;
    fn try_from(item: DelegatedTool) -> Result<JsonSchema, Self::Error> {
        Ok(item.input_schema.clone())
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for DelegatedTool {
    fn check(&self) -> Result<(), FormaError> {
        Ok(())
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        unreachable!("delegated tools should be executed at client side")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::NoneDB;
    use genai_client::schema::SchemaType;
    use std::collections::HashMap;

    fn object_schema() -> JsonSchema {
        let mut properties = HashMap::new();
        properties.insert(
            "query".to_string(),
            JsonSchema {
                schema_type: Some(SchemaType::String),
                description: Some("the search query".to_string()),
                ..Default::default()
            },
        );
        JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(properties),
            required: Some(vec!["query".to_string()]),
            ..Default::default()
        }
    }

    #[test]
    fn check_always_ok() {
        let tool = DelegatedTool {
            input_schema: object_schema(),
        };
        assert!(<DelegatedTool as ToolTrait<NoneDB>>::check(&tool).is_ok());
    }

    #[test]
    fn check_ok_with_empty_schema() {
        let tool = DelegatedTool {
            input_schema: JsonSchema::default(),
        };
        assert!(<DelegatedTool as ToolTrait<NoneDB>>::check(&tool).is_ok());
    }

    #[test]
    fn try_from_returns_input_schema() {
        let schema = object_schema();
        let tool = DelegatedTool {
            input_schema: schema.clone(),
        };
        let result = JsonSchema::try_from(tool);
        assert!(result.is_ok());
        let got = result.unwrap();
        assert_eq!(got.schema_type, schema.schema_type);
        assert_eq!(got.required, schema.required);
        assert_eq!(
            got.properties.as_ref().map(|p| p.contains_key("query")),
            Some(true)
        );
    }
}
