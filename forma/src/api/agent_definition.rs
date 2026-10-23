use super::workflow_definition::WorkflowDefinition;
use super::{
    mcp_tool_definition::McpToolDefinition, template_field::TemplateField,
    tool_definition::ToolRunnerDefinition,
};
use crate::api::{ObjectDefinition, eval_definition::EvalDefinition};

use agent::{Agent, default_agent_id, template::is_valid_variable_name};

use derive::FormaDocs;
use protocols::{EncryptedTokens, FormaError};

use crate::api::mcp_tool_definition::McpAccess;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::path::Path;

/// Represents an input variable for the Agent.
/// These are values that will be pre-loaded into the session,
/// and thus can be referenced in system prompts in nodes of the agent.
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct InputVariable {
    /// Name of the variable. Cannot have spaces
    pub name: String,
    /// Type of the variable: `string`, `long-text`, `number` or `boolean`.
    ///
    /// `long-text` behaves exactly like `string`; it only tells the UI to
    /// offer a text area instead of a single-line field.
    #[serde(rename = "type")]
    pub type_: String,
    /// The value that will be passed to the Agent when it is invoked.
    ///
    /// Leave it empty if you want to share the Agent.
    #[serde(default)]
    pub value: Value,
    /// An example of how the variable should look like (e.g., +569..., some@email.com, etc.)
    pub placeholder: Value,
}

/// Defines the entry points, evaluations and other settings
/// for the Forma agent.
#[derive(Serialize, Deserialize, Clone, Debug, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct AgentDefinition {
    /// The ID. If absent, one will be provided.
    #[serde(default = "default_agent_id")]
    pub(crate) id: String,

    /// The name of the Agent.
    pub(crate) name: String,

    /// The description of the Agent, used for searching purposes
    pub(crate) description: String,

    /// The Workflow that serves as a starting point of this Agent.
    pub(crate) start: WorkflowDefinition,

    /// A set of input variables that can then be referenced
    /// in system prompts in nodes of the agent
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) variables: Vec<InputVariable>,

    /// The evaluations that will be used to test the
    /// Agent as a whole.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) evals: Vec<EvalDefinition>,

    /// Examples of what a user could ask the Agent first, shown as cards
    /// when a conversation starts. Add as many as you like; the apps show a
    /// few at random.
    ///
    /// They do not change the Agent's behavior and are not part of the
    /// compiled Agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) suggestions: Vec<String>,

    /// The time zone to assume for users whose client does not send theirs
    /// (WhatsApp, Telegram…), as an IANA name such as `America/Santiago`.
    /// Read by the `current-date` tool; leave it out if the agent's users may
    /// be anywhere, and that tool will ask them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timezone: Option<String>,
}

/// How long a suggestion may be, in characters: a question, not a prompt.
const MAX_SUGGESTION_CHARS: usize = 300;

impl Default for AgentDefinition {
    fn default() -> Self {
        Self {
            id: default_agent_id(),
            name: "".to_string(),
            description: "".to_string(),
            start: WorkflowDefinition::default(),
            evals: vec![],
            variables: vec![],
            suggestions: vec![],
            timezone: None,
        }
    }
}

impl AgentDefinition {
    /// Converts this definition into a runtime [`Agent`].
    pub async fn into_agent(self, oauth: &EncryptedTokens) -> Result<Agent, FormaError> {
        self.compile(&McpAccess::Connect(oauth.clone())).await
    }

    /// Compiles the agent without connecting to its MCP servers, each standing
    /// in as a placeholder tool (see [`McpAccess::Unconnected`]): everything
    /// else about it is checked as [`AgentDefinition::into_agent`] would, by
    /// someone who has not connected those servers. The result is for checking
    /// and describing, never for running.
    pub async fn into_agent_unconnected(self) -> Result<Agent, FormaError> {
        self.compile(&McpAccess::Unconnected).await
    }

    async fn compile(self, mcp: &McpAccess) -> Result<Agent, FormaError> {
        let mut evals = Vec::with_capacity(self.evals.len());
        for e in self.evals.into_iter() {
            let n = e.try_into()?;
            evals.push(n);
        }
        let variable_names: Vec<String> =
            self.variables.clone().into_iter().map(|v| v.name).collect();
        let variables = self
            .variables
            .clone()
            .into_iter()
            .map(|v| (v.name, v.value))
            .collect();
        Ok(Agent {
            id: self.id.clone(),
            start: self
                .start
                .clone()
                .into_workflow(mcp, variable_names)
                .await?,
            evals,
            variables,
            timezone: self.timezone.clone(),
        })
    }
}

impl AgentDefinition {
    /// Returns all [`McpToolDefinition`]s that have `auth` configured,
    /// collected from every node in the agent's workflow.
    pub fn mcp_definitions(&self) -> Vec<McpToolDefinition> {
        self.start
            .nodes
            .iter()
            .flat_map(|node| node.tools.iter())
            .filter_map(|tf| match tf {
                TemplateField::Object(td) => Some(td),
                TemplateField::File(_) => None,
            })
            .filter_map(|td| match &td.tool {
                ToolRunnerDefinition::Mcp(mcp) if mcp.auth.is_some() => Some(mcp.clone()),
                _ => None,
            })
            .collect()
    }
}

impl ObjectDefinition for AgentDefinition {
    fn expand(self, filename: &str) -> Result<Self, FormaError> {
        is_valid_variable_name(&self.id)?;

        let path = Path::new(filename);
        let parent_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or("could not read file")?;

        let parent_dir = parent_dir.as_path().to_str().unwrap();

        let ret = Self {
            name: self.name,
            description: self.description,
            id: self.id,
            start: self.start.expand(parent_dir)?,
            evals: self
                .evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
            variables: self.variables,
            suggestions: self.suggestions,
            timezone: self.timezone,
        };

        Ok(ret)
    }

    fn check(self) -> Result<Self, FormaError> {
        let Self {
            id,
            name,
            description,
            start,
            variables,
            evals,
            suggestions,
            timezone,
        } = self;
        if let Some(tz) = &timezone {
            agent::tool::current_date::parse_timezone(tz)?;
        }
        is_valid_variable_name(&id)?;
        for variable in variables.iter() {
            is_valid_variable_name(&variable.name)?;
        }
        for (i, suggestion) in suggestions.iter().enumerate() {
            if suggestion.trim().is_empty() {
                return Err(FormaError::Input(format!("suggestion {} is empty", i + 1)));
            }
            if suggestion.chars().count() > MAX_SUGGESTION_CHARS {
                return Err(FormaError::Input(format!(
                    "suggestion {} is longer than {MAX_SUGGESTION_CHARS} characters",
                    i + 1
                )));
            }
        }
        Ok(Self {
            id,
            name,
            description,
            start: start.check()?,
            variables,
            evals,
            suggestions,
            timezone,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::eval_definition::MetricDefinition;

    fn agent(yaml: &str) -> AgentDefinition {
        serde_yaml::from_str(yaml).expect("could not parse agent")
    }

    /// The whole point of checking a definition: a name collision inside a node
    /// is reported without connecting to anything or compiling the agent.
    #[test]
    fn check_reaches_the_nodes_of_the_start_workflow() {
        let def = agent(
            r#"
            id: an_agent
            name: An agent
            description: An agent that repeats itself
            start:
              nodes:
                - id: node_1
                  tools:
                    - name: repeated
                      tool:
                        type: template
                        templates: ['hello']
                    - name: repeated
                      tool:
                        type: template
                        templates: ['hello']
            "#,
        );
        let err = def
            .check()
            .expect_err("the duplicate should be rejected")
            .to_string();
        assert!(err.contains("duplicate tool name 'repeated'"), "{err}");
        assert!(err.contains("node_1"), "{err}");
    }

    /// The time zone is an IANA name, and a typo is caught when the agent
    /// loads, not when a user first asks for the date.
    #[test]
    fn check_validates_the_agent_time_zone() {
        let yaml = |tz: &str| {
            format!(
                "id: an_agent\nname: An agent\ndescription: d\ntimezone: {tz}\nstart:\n  nodes:\n    - id: node_1\n      tools:\n        - name: fecha-actual\n          tool:\n            type: current-date\n"
            )
        };
        agent(&yaml("America/Santiago"))
            .check()
            .expect("an IANA zone should be accepted");
        let err = agent(&yaml("Santiago"))
            .check()
            .expect_err("a city alone is not a zone")
            .to_string();
        assert!(err.contains("IANA"), "{err}");
    }

    #[test]
    fn check_accepts_a_well_formed_agent() {
        let def = agent(
            r#"
            id: an_agent
            name: An agent
            description: An agent that behaves
            start:
              nodes:
                - id: node_1
                  tools:
                    - name: greet
                      tool:
                        type: template
                        templates: ['hello']
            "#,
        );
        assert!(def.check().is_ok());
    }

    #[test]
    fn check_rejects_invalid_agent_id() {
        let def = AgentDefinition {
            id: "1invalid".to_string(),
            ..Default::default()
        };
        let err = def.check().expect_err("bad id").to_string();
        assert!(err.contains("1invalid"), "{err}");
    }

    #[test]
    fn check_rejects_invalid_variable_name() {
        let def = AgentDefinition {
            id: "an_agent".to_string(),
            variables: vec![InputVariable {
                name: "not a name".to_string(),
                type_: "string".to_string(),
                value: Value::Null,
                placeholder: Value::Null,
            }],
            ..Default::default()
        };
        let err = def.check().expect_err("bad variable name").to_string();
        assert!(err.contains("not a name"), "{err}");
    }

    /// Suggestions are read and checked, but never reach the compiled agent.
    #[test]
    fn suggestions_are_checked() {
        let def = agent(
            r#"
            id: an_agent
            name: An agent
            description: An agent that suggests
            suggestions:
              - What can you do?
              - Summarise my last report
            start:
              nodes:
                - id: node_1
                  tools:
                    - name: greet
                      tool:
                        type: template
                        templates: ['hello']
            "#,
        );
        assert_eq!(def.suggestions.len(), 2);
        assert!(def.check().is_ok());

        let blank = AgentDefinition {
            id: "an_agent".to_string(),
            suggestions: vec!["  ".to_string()],
            ..Default::default()
        };
        let err = blank.check().expect_err("blank suggestion").to_string();
        assert!(err.contains("suggestion 1 is empty"), "{err}");

        let long = AgentDefinition {
            id: "an_agent".to_string(),
            suggestions: vec!["ok".to_string(), "a".repeat(MAX_SUGGESTION_CHARS + 1)],
            ..Default::default()
        };
        let err = long.check().expect_err("long suggestion").to_string();
        assert!(err.contains("suggestion 2"), "{err}");
    }

    #[test]
    fn expand_rejects_missing_agent_metric_template() {
        let dir = tempfile::tempdir().unwrap();
        let def = AgentDefinition {
            evals: vec![EvalDefinition {
                dataset: "dataset".to_string(),
                metrics: vec![MetricDefinition {
                    id: "politeness".to_string(),
                    judge: Default::default(),
                    template: "$metrics/politeness_TYPO.md".to_string(),
                    required_score: None,
                }],
            }],
            ..Default::default()
        };

        assert!(
            def.expand(dir.path().join("agent.yaml").to_str().unwrap())
                .is_err()
        );
    }

    // --- compiling without connecting to MCP servers ---------------------------

    use crate::api::agent_features::AgentFeatures;
    use agent::Callable;

    /// Nobody can reach this: an agent that uses it compiles only unconnected.
    const UNREACHABLE: &str = "http://127.0.0.1:1/mcp";

    /// An agent whose one node is `node` (YAML, indented under `nodes:`).
    fn agent_with_node(node: &str) -> AgentDefinition {
        agent(&format!(
            "id: an_agent\nname: An agent\ndescription: d\nstart:\n  nodes:\n{node}"
        ))
    }

    fn mcp_tool(name: &str) -> String {
        format!(
            "        - name: {name}\n          tool:\n            type: mcp\n            url: {UNREACHABLE}\n"
        )
    }

    fn compiled_check(agent: &agent::Agent) -> Result<(), FormaError> {
        <agent::Agent as Callable<db::NoneDB>>::check(agent)
    }

    /// The point of it: an author who has not connected a server can still have
    /// everything else checked — and a node whose only tools come from that
    /// server keeps its shape, so a rule about having tools does not fail.
    #[tokio::test]
    async fn an_unconnected_compile_checks_everything_but_the_servers() {
        let def = agent_with_node(&format!(
            "    - id: node_1\n      summarization_prompt: Summarise what the tools found.\n      tools:\n{}",
            mcp_tool("music")
        ));
        let agent = def
            .into_agent_unconnected()
            .await
            .expect("compiles without connecting");
        compiled_check(&agent).expect("a node with only MCP tools still has tools");

        // Described as far as it can be: the server, and that it uses one.
        let features = AgentFeatures::from(&agent);
        assert_eq!(features.mcp_servers, vec![UNREACHABLE]);
        assert_eq!(features.tool_kinds, vec!["mcp"]);
        // What the server's tools are called is not known, so none are listed.
        assert!(features.tools.is_empty());
    }

    /// Nothing an unconnected server stands in for can hide a problem with
    /// the rest: the errors that would show connected show unconnected too.
    #[tokio::test]
    async fn an_unconnected_server_never_hides_another_error() {
        // A rule broken elsewhere in the node.
        let def = agent_with_node(&format!(
            "    - id: node_1\n      max_iterations: 0\n      tools:\n{}",
            mcp_tool("music")
        ));
        let agent = def.into_agent_unconnected().await.unwrap();
        let err = compiled_check(&agent).unwrap_err().to_string();
        assert!(err.contains("max_iterations"), "{err}");

        // A rule about having tools, which the server's placeholder counts for
        // exactly as its real tools would.
        let def = agent_with_node(&format!(
            "    - id: node_1\n      output_format:\n        type: object\n        properties:\n          a:\n            type: string\n      tools:\n{}",
            mcp_tool("music")
        ));
        let agent = def.into_agent_unconnected().await.unwrap();
        let err = compiled_check(&agent).unwrap_err().to_string();
        assert!(err.contains("output schema and also tools"), "{err}");
    }

    /// One server listed twice in a node gives the same tools twice once
    /// connected, which clash; its placeholders clash the same way.
    #[tokio::test]
    async fn one_server_twice_in_a_node_clashes_unconnected_too() {
        let def = agent_with_node(&format!(
            "    - id: node_1\n      tools:\n{}{}",
            mcp_tool("music"),
            mcp_tool("more_music")
        ));
        let agent = def.into_agent_unconnected().await.unwrap();
        let err = compiled_check(&agent).unwrap_err().to_string();
        assert!(err.contains("duplicate tool name"), "{err}");
    }
}
