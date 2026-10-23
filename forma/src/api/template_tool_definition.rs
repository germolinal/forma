use agent::{template::extract_placeholder_names, tool::TemplateTool};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::{ObjectDefinition, template_field::expand_string};

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
#[derive(Debug, Serialize, Deserialize, Clone, FormaDocs)]
pub struct TemplateToolDefinition {
    /// The templates to choose from. For instance,
    ///
    /// > hello {{name: string "the name of the user" }}, nice to meet you! The current time is {{time: string "the current time"}}
    pub templates: Vec<String>,
}

impl Default for TemplateToolDefinition {
    fn default() -> Self {
        Self {
            templates: vec!["Hello! How can I help?".to_string()],
        }
    }
}

impl ObjectDefinition for TemplateToolDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        let mut templates = Vec::with_capacity(self.templates.len());
        for t in self.templates.into_iter() {
            extract_placeholder_names(&t)?;
            templates.push(expand_string(t, parent_dir)?);
        }
        Ok(Self { templates })
    }
}

impl TryFrom<TemplateToolDefinition> for TemplateTool {
    type Error = FormaError;
    fn try_from(value: TemplateToolDefinition) -> Result<Self, Self::Error> {
        Ok(TemplateTool {
            templates: value.templates,
        })
    }
}
