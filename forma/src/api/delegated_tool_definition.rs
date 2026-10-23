use agent::tool::DelegatedTool;
use derive::FormaDocs;
use genai_client::JsonSchema;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::{ObjectDefinition, template_field::TemplateField};

/// A tool that delegates execution to a remote API endpoint.
/// When calling this tool, Forma will send a request asking the
/// client not only to validate the inputs but also to execute
/// it. The session will be suspended until the agent receives the
/// output of this tool.
#[derive(Debug, Serialize, Deserialize, Clone, FormaDocs)]
pub struct DelegatedToolDefinition {
    pub(crate) input_schema: TemplateField<JsonSchema>,
}

impl ObjectDefinition for DelegatedToolDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        let input_schema = self.input_schema.expand(parent_dir)?;
        Ok(DelegatedToolDefinition { input_schema })
    }
}

impl TryFrom<DelegatedToolDefinition> for DelegatedTool {
    type Error = FormaError;

    fn try_from(value: DelegatedToolDefinition) -> Result<Self, Self::Error> {
        if let TemplateField::Object(ref schema) = value.input_schema {
            let ret = Self {
                input_schema: schema.clone(),
            };
            Ok(ret)
        } else {
            unreachable!(
                "DelegatedToolDefinition should be expanded before turning it into a DelegatedTool"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai_client::schema::SchemaType;
    use std::{collections::HashMap, fs};

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

    // ── ObjectDefinition::expand ──────────────────────────────────────────────

    #[test]
    fn expand_object_variant_is_identity() {
        let def = DelegatedToolDefinition {
            input_schema: TemplateField::Object(object_schema()),
        };
        let result = def.expand("/irrelevant");
        assert!(result.is_ok());
        match result.unwrap().input_schema {
            TemplateField::Object(schema) => {
                assert_eq!(schema.schema_type, Some(SchemaType::Object));
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn expand_file_variant_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("schema.json");
        fs::write(
            &file_path,
            r#"{"type":"object","properties":{"city":{"type":"string"}}}"#,
        )
        .unwrap();
        let parent = dir.path().to_str().unwrap();

        let def = DelegatedToolDefinition {
            input_schema: TemplateField::File("$schema.json".to_string()),
        };
        let result = def.expand(parent);
        assert!(result.is_ok());
        match result.unwrap().input_schema {
            TemplateField::Object(schema) => {
                assert_eq!(schema.schema_type, Some(SchemaType::Object));
                assert!(
                    schema
                        .properties
                        .as_ref()
                        .map_or(false, |p| p.contains_key("city"))
                );
            }
            _ => panic!("expected Object variant after expand"),
        }
    }

    #[test]
    fn expand_file_variant_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("schema.yaml");
        fs::write(
            &file_path,
            "type: object\nproperties:\n  city:\n    type: string\n",
        )
        .unwrap();
        let parent = dir.path().to_str().unwrap();

        let def = DelegatedToolDefinition {
            input_schema: TemplateField::File("$schema.yaml".to_string()),
        };
        let result = def.expand(parent);
        assert!(result.is_ok());
        match result.unwrap().input_schema {
            TemplateField::Object(schema) => {
                assert_eq!(schema.schema_type, Some(SchemaType::Object));
            }
            _ => panic!("expected Object variant after expand"),
        }
    }

    #[test]
    fn expand_missing_file_returns_err() {
        let def = DelegatedToolDefinition {
            input_schema: TemplateField::File("$nonexistent.json".to_string()),
        };
        assert!(def.expand("/tmp").is_err());
    }

    // ── TryFrom ───────────────────────────────────────────────────────────────

    #[test]
    fn try_from_object_variant_extracts_schema() {
        let schema = object_schema();
        let def = DelegatedToolDefinition {
            input_schema: TemplateField::Object(schema.clone()),
        };
        let result = DelegatedTool::try_from(def);
        assert!(result.is_ok());
        let tool = result.unwrap();
        assert_eq!(tool.input_schema.schema_type, schema.schema_type);
        assert_eq!(tool.input_schema.required, schema.required);
        assert_eq!(
            tool.input_schema
                .properties
                .as_ref()
                .map(|p| p.contains_key("query")),
            Some(true)
        );
    }
}
