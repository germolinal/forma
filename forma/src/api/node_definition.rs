use agent::{
    Eval, Node, Tool,
    node::{default_max_iterations, default_node_id, default_system_prompt},
    template::{extract_placeholder_names, is_valid_variable_name},
};
use derive::FormaDocs;
use genai_client::{
    GenaiClient, GenaiClientTrait, JsonSchema,
    openai::v1::{OpenAIResponseFormat, OpenAIResponseFormatContent, OpenAISchemaType},
};
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use std::collections::HashSet;
use tracing::warn;

use crate::api::{
    ObjectDefinition, eval_definition::EvalDefinition, mcp_tool_definition::McpAccess,
};

use super::{
    template_field::{TemplateField, expand_string},
    tool_definition::ToolDefinition,
};

/// A Node is the key of Forma Agents' execution. They are the
/// ones that call LLMs, process their response, and decide whether
/// tools should be called. It always does the same thing:
///
/// 1. **Triage** – an LLM reads the whole conversation and decides what to do next: reply right away, or call tools.
/// 2. **Tools (optional)** – the node runs the tools the LLM chose, and triage runs again with their results. This repeats until triage replies without calling a tool, and that reply is the node's answer, or until `max_iterations` rounds of tools have run.
/// 3. **Summarisation (only at the limit)** – if `max_iterations` runs out while the LLM is still calling tools, the node calls an LLM once more, without tools. It is told it reached the limit, so it says what it found and what is missing, and asks the user, in the language of the conversation, whether to go on. A simple yes starts a new turn. `summarization_prompt` and `summarization_llm` shape this call.
///
/// **Load time checks**
///
/// - ID is well-formed
/// - Tools are all well formed
/// - It does not have output format AND tools simultaneously
/// - Does not have summarization prompt or summarization LLM if it does not have tools
#[derive(Clone, Deserialize, Serialize, Debug, FormaDocs)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeDefinition {
    /// The ID of the node. If absent, one will be provided.
    /// The value given here is used to identify the output
    /// of this node in subsequent nodes within a workflow,
    /// allowing for template interpolation
    #[serde(default = "default_node_id")]
    pub(crate) id: String,

    /// The main LLM and settings, used for triage and, unless
    /// `summarization_llm` is given, for summarisation.
    #[serde(default)]
    pub(crate) llm: GenaiClient,

    /// The main instruction given to the LLM.
    #[serde(default = "default_system_prompt")]
    pub(crate) system_prompt: String,

    /// An optional LLM for summarisation, the reply written when
    /// `max_iterations` runs out. If absent, the main `llm` is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summarization_llm: Option<GenaiClient>,

    /// An optional instruction for summarisation, the reply written when
    /// `max_iterations` runs out while the LLM is still calling tools. If
    /// absent, the `system_prompt` is used. Either way, the node adds a note
    /// saying the limit was reached and the user should be asked whether to
    /// go on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summarization_prompt: Option<String>,

    /// The tools that the LLM can decide to call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) tools: Vec<TemplateField<ToolDefinition>>,

    /// The evaluations used to test this specific node as a
    /// whole (different from Triage- and Summarize-specific evals)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) evals: Vec<EvalDefinition>,

    /// The evaluations used to test the triage LLM specifically
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub triage_evals: Vec<EvalDefinition>,

    /// The evaluations used to test the summarize LLM specifically
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub summarize_evals: Vec<EvalDefinition>,

    /// An optional output schema for the node.
    ///
    /// A node that has tools cannot have output format. If you want
    /// to combine tool calling and output format, you should
    /// chain two nodes (one with the tools and the next with
    /// the output schema)
    ///
    /// Internally, the output schema will be assigned to the node's
    /// LLM.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub output_format: Option<TemplateField<JsonSchema>>,

    /// The maximum number of rounds of tool calls in one turn. When they run
    /// out, the node summarises instead of calling more tools.
    ///
    /// Defaults to 10.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// Whether to add tool results to the summarization prompt.
    ///
    /// This can be false for nodes that use LLMs like those from OpenAI, Anthropic or
    /// Google, which use special kinds of messages to communicate tool results
    /// and calls. However, other LLMs may need this to be true.
    #[serde(default)]
    pub add_tool_results_in_summarization_prompt: bool,

    /// Whether to stream tokens to the user as they are generated.
    ///
    /// When `false` (the default), the node generates the whole
    /// response before sending it to the user. When `true`, tokens
    /// are streamed as they arrive.
    #[serde(default)]
    pub stream: bool,

    /// Phrases the chat shows, one at a time and at random, while this node
    /// works (e.g., `["Revisando tus facturas…", "Sumando el IVA…"]`). If
    /// there are none, the chat shows the node's ID.
    ///
    /// They do not change the node's behavior and are not part of the
    /// compiled Agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) waiting_phrases: Vec<String>,
}

/// How long a waiting phrase may be, in characters: it shares a line with a
/// spinner.
const MAX_WAITING_PHRASE_CHARS: usize = 100;

impl ObjectDefinition for JsonSchema {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl ObjectDefinition for NodeDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        let summarisation_prompt = match self.summarization_prompt {
            Some(v) => Some(expand_string(v, parent_dir)?),
            None => None,
        };

        let mut tools = Vec::with_capacity(self.tools.len());
        for t in self.tools {
            let e = t.expand(parent_dir)?;
            tools.push(e);
        }
        let output_format = match self.output_format {
            Some(e) => Some(e.expand(parent_dir)?),
            None => None,
        };

        Ok(Self {
            id: self.id,
            llm: self.llm,
            summarization_llm: self.summarization_llm,
            // expanded stuff
            system_prompt: expand_string(self.system_prompt, parent_dir)?,
            summarization_prompt: summarisation_prompt,
            tools,
            evals: self
                .evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
            triage_evals: self
                .triage_evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
            summarize_evals: self
                .summarize_evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
            output_format,
            max_iterations: self.max_iterations,
            add_tool_results_in_summarization_prompt: self.add_tool_results_in_summarization_prompt,
            stream: self.stream,
            waiting_phrases: self.waiting_phrases,
        })
    }

    /// The same rules `Node::check` applies at runtime, as far as a definition
    /// can see them: an `output_format` here becomes the LLM's output schema
    /// there, and the tools written here become the tools counted there.
    fn check(mut self) -> Result<Self, FormaError> {
        is_valid_variable_name(&self.id)?;
        if self.max_iterations < 1 {
            return Err(FormaError::Input(
                "max_iterations must be at least 1".to_string(),
            ));
        }

        for (i, phrase) in self.waiting_phrases.iter().enumerate() {
            if phrase.trim().is_empty() {
                return Err(FormaError::Input(format!(
                    "waiting phrase {} of node '{}' is empty",
                    i + 1,
                    self.id
                )));
            }
            if phrase.chars().count() > MAX_WAITING_PHRASE_CHARS {
                return Err(FormaError::Input(format!(
                    "waiting phrase {} of node '{}' is longer than {MAX_WAITING_PHRASE_CHARS} characters",
                    i + 1,
                    self.id
                )));
            }
        }

        let mut seen_tool_names: HashSet<String> = HashSet::new();
        for tool in self.tools.iter() {
            let TemplateField::Object(tool) = tool else {
                continue;
            };
            // A `None` here is an MCP server: it decides its own tool names
            // when it is connected to, so this node's names can only be told
            // apart once the agent is compiled.
            let Some(names) = tool.tool_names() else {
                continue;
            };
            for name in names {
                if !seen_tool_names.insert(name.clone()) {
                    return Err(FormaError::Input(format!(
                        "node '{}' has duplicate tool name '{name}'",
                        self.id
                    )));
                }
            }
        }

        if self.tools.is_empty() {
            if self.summarization_prompt.is_some() {
                return Err(FormaError::Input(format!(
                    "node '{}' has a summarization_prompt but no tools",
                    self.id
                )));
            }
        } else {
            if self.output_format.is_some() {
                return Err(FormaError::Input(format!(
                    "node '{}' has an output schema and also tools",
                    self.id
                )));
            }
            if let Some(llm) = &self.summarization_llm
                && llm.has_output_schema()
            {
                return Err(FormaError::Input(format!(
                    "node '{}' has a summarization_llm with an output schema",
                    self.id
                )));
            }
        }

        // Taken out and put back so each tool can be checked by value, without
        // moving the twelve other fields around it.
        let tools = std::mem::take(&mut self.tools);
        self.tools = tools
            .into_iter()
            .map(|t| t.check())
            .collect::<Result<_, _>>()?;
        Ok(self)
    }
}

impl Default for NodeDefinition {
    fn default() -> Self {
        Self {
            id: default_node_id(),
            llm: GenaiClient::default(),
            system_prompt: default_system_prompt(),
            summarization_llm: None,
            summarization_prompt: None,
            tools: vec![],
            evals: vec![],
            triage_evals: vec![],
            summarize_evals: vec![],
            output_format: None,
            max_iterations: default_max_iterations(),
            add_tool_results_in_summarization_prompt: false,
            stream: false,
            waiting_phrases: vec![],
        }
    }
}

impl NodeDefinition {
    /// Checks on the `system_prompt` and the `summarisation_prompt` for the values that should be replaced
    ///
    /// Each value within the templates (e.g., {{some_value}}) corresponds
    /// to the output of that node. This method collects all the values that should
    /// be substituted.
    pub fn get_dependencies(&self) -> Result<HashSet<String>, FormaError> {
        let mut deps = extract_placeholder_names(&self.system_prompt)?;
        if let Some(prompt) = &self.summarization_prompt {
            let new_deps = extract_placeholder_names(prompt)?;
            deps.extend(new_deps);
        }
        Ok(deps)
    }
}

fn translate_evals(definitions: &[EvalDefinition]) -> Result<Vec<Eval>, FormaError> {
    let mut evals = Vec::with_capacity(definitions.len());
    for e in definitions.iter().cloned() {
        let n = e.try_into()?;
        evals.push(n);
    }
    Ok(evals)
}

impl NodeDefinition {
    pub(crate) async fn into_node(
        self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> Result<Node, FormaError> {
        let mut tools: Vec<Tool> = Vec::with_capacity(self.tools.len());
        for t in self.tools.into_iter() {
            let available_variables = available_variables.clone();
            if let TemplateField::Object(t) = t {
                let new_tools: Vec<Tool> = t.into_tools(mcp, available_variables).await?;
                tools.extend(new_tools);
            } else {
                return Err(
                    "Nodes' tools should be expanded before turning them into Node objects... found a non-expanded tool"
                        .to_string()
                        .into(),
                );
            }
        }
        // A code-execution tool reaches workspaces only through the node's own
        // workspace tools, which exist only now.
        agent::tool::code_execution::link_workspaces(&mut tools);

        let mut llm = self.llm.clone();
        if let Some(schema) = self.output_format {
            if let TemplateField::Object(schema) = schema {
                llm.set_output_schema(OpenAIResponseFormat {
                    schema_type: OpenAISchemaType::JsonSchema,
                    json_schema: OpenAIResponseFormatContent {
                        name: format!("{}-out-schema", self.id),
                        schema,
                    },
                });
            } else {
                return Err(
                    "Nodes' tools should be expanded before turning them into Node objects... found a non-expanded output format"
                        .to_string()
                        .into(),
                );
            }
        }
        let summarization_llm = match self.summarization_llm {
            Some(s) => s.clone(),
            None => {
                let mut r = llm.clone();
                // Sumarized does not use tools
                r.set_tools(None);
                r.set_tool_choice_mode(None);
                r
            }
        };

        if llm.count_tools() > 0 {
            warn!(
                "llm in node '{}' has tool. These will be removed. Tools should be defined in the Node level, not at LLM level.",
                &self.id
            );
            llm.set_tools(None);
        }
        if !tools.is_empty() {
            let mut llm_tools = Vec::with_capacity(tools.len());
            for t in tools.clone().into_iter() {
                llm_tools.push(t.try_into()?);
            }
            llm.set_tools(Some(llm_tools));
        }

        let evals = translate_evals(&self.evals)?;
        let triage_evals = translate_evals(&self.triage_evals)?;
        let summarize_evals = translate_evals(&self.summarize_evals)?;

        let ret = Node {
            id: self.id.clone(),
            llm,
            system_prompt: self.system_prompt.clone(), // should have been expanded
            summarization_llm,
            summarization_prompt: self.summarization_prompt.clone(), // should have been expanded
            evals,
            tools,
            triage_evals,
            summarize_evals,
            max_iterations: self.max_iterations,
            add_tool_results_in_summarization_prompt: self.add_tool_results_in_summarization_prompt,
            stream: self.stream,
        };
        Ok(ret)
    }
}

#[cfg(test)]
mod testing {

    use super::*;
    use protocols::EncryptedTokens;

    #[test]
    fn get_node_dependencies() -> Result<(), FormaError> {
        let node = NodeDefinition {
            llm: GenaiClient::default(),
            system_prompt: "Behave like {{character}} and use emojies".to_string(),
            summarization_prompt: Some("Summarise the information about {{world}}".to_string()),
            ..Default::default()
        };
        let dependencies = node.get_dependencies()?;
        // make sure 'character' and 'world' are included, in whatever order
        if dependencies.len() == 2 {
            assert!(dependencies.contains(&"character".to_string()));
            assert!(dependencies.contains(&"world".to_string()));
        } else {
            panic!("Expected 2 dependencies, got {}", dependencies.len());
        }
        Ok(())
    }
    #[test]
    fn get_template_dependencies() -> Result<(), FormaError> {
        let template = r"Hello, {{world}}".to_string();
        let dependencies: HashSet<String> = extract_placeholder_names(&template)?;
        assert_eq!(dependencies, ["world".to_string()].into());
        Ok(())
    }

    #[tokio::test]
    async fn try_from_with_expanded_tools_applies_tools_to_llm_and_clears_summarization_tools() {
        use crate::api::template_field::TemplateField;
        use crate::api::tool_definition::ToolDefinition;

        let nd = NodeDefinition {
            tools: vec![TemplateField::Object(ToolDefinition::default())],
            ..Default::default()
        };
        let mcp = McpAccess::Connect(EncryptedTokens::default());

        let node = nd
            .into_node(&mcp, vec![])
            .await
            .expect("TryFrom should succeed for expanded tools");
        // Node should keep the tools
        assert_eq!(node.tools.len(), 1);
        // The node's main llm should have the tools applied
        assert_eq!(node.llm.count_tools(), 1);
        // The summarization LLM should have no tools (it is cloned from llm before tools are set)
        assert_eq!(node.summarization_llm.count_tools(), 0);
    }

    #[tokio::test]
    async fn try_from_translates_evals_counts() -> Result<(), FormaError> {
        use crate::api::eval_definition::EvalDefinition;

        let nd = NodeDefinition {
            evals: vec![EvalDefinition::default()],
            triage_evals: vec![EvalDefinition::default()],
            summarize_evals: vec![EvalDefinition::default()],
            ..Default::default()
        };
        let mcp = McpAccess::Connect(EncryptedTokens::default());
        let node = nd.into_node(&mcp, vec![]).await?;
        assert_eq!(node.evals.len(), 1);
        assert_eq!(node.triage_evals.len(), 1);
        assert_eq!(node.summarize_evals.len(), 1);
        Ok(())
    }

    // ── NodeDefinition::check ────────────────────────────────────────────────

    /// Parses a tool the way an agent file would, so the check runs over what
    /// serde produces rather than over a hand-built struct.
    fn tool(yaml: &str) -> TemplateField<ToolDefinition> {
        TemplateField::Object(serde_yaml::from_str(yaml).expect("could not parse tool"))
    }

    fn template_tool(name: &str) -> TemplateField<ToolDefinition> {
        tool(&format!(
            "name: {name}\ntool:\n  type: template\n  templates: ['hello']"
        ))
    }

    fn node_with(tools: Vec<TemplateField<ToolDefinition>>) -> NodeDefinition {
        NodeDefinition {
            id: "node_1".to_string(),
            tools,
            ..Default::default()
        }
    }

    #[test]
    fn check_accepts_unique_tool_names() {
        let node = node_with(vec![template_tool("t1"), template_tool("t2")]);
        assert!(node.check().is_ok());
    }

    #[test]
    fn check_rejects_duplicate_tool_names() {
        let node = node_with(vec![template_tool("t1"), template_tool("t1")]);
        let err = node
            .check()
            .expect_err("two tools named 't1' should be rejected")
            .to_string();
        assert!(err.contains("duplicate tool name 't1'"), "{err}");
    }

    #[test]
    fn check_rejects_invalid_tool_name() {
        let node = node_with(vec![template_tool("'not a name'")]);
        let err = node
            .check()
            .expect_err("a tool name with spaces should be rejected")
            .to_string();
        assert!(err.contains("not a name"), "{err}");
    }

    /// A docstore is not one tool but several, named after it. Two of them
    /// sharing a base name means every one of their tools collides.
    #[test]
    fn check_rejects_docstores_sharing_a_base_name() {
        let node = node_with(vec![
            tool("name: notes\ntool:\n  type: scrapbook"),
            tool("name: notes\ntool:\n  type: google-workspace"),
        ]);
        let err = node
            .check()
            .expect_err("two docstores called 'notes' should be rejected")
            .to_string();
        assert!(err.contains("duplicate tool name 'list-notes'"), "{err}");
    }

    /// ...but a docstore's own name never becomes a tool name, so an ordinary
    /// tool may still use it.
    #[test]
    fn check_accepts_tool_named_after_a_docstore() {
        let node = node_with(vec![
            template_tool("notes"),
            tool("name: notes\ntool:\n  type: scrapbook"),
        ]);
        assert!(node.check().is_ok());
    }

    /// An MCP server names its own tools when it is connected to, and the name
    /// written beside it is never used, so nothing here can be compared.
    #[test]
    fn check_ignores_mcp_tool_names() {
        let node = node_with(vec![
            tool("name: server\ntool:\n  type: mcp\n  url: https://mcp.example"),
            tool("name: server\ntool:\n  type: mcp\n  url: https://other.example"),
        ]);
        assert!(node.check().is_ok());
    }

    #[test]
    fn check_rejects_invalid_node_id() {
        let node = NodeDefinition {
            id: "1bad_node".to_string(),
            ..Default::default()
        };
        let err = node.check().expect_err("bad id").to_string();
        assert!(err.contains("1bad_node"), "{err}");
    }

    /// Waiting phrases are checked, but never reach the compiled node.
    #[tokio::test]
    async fn waiting_phrases_are_checked_but_not_compiled() {
        let node = NodeDefinition {
            waiting_phrases: vec!["Thinking...".to_string(), "Almost there...".to_string()],
            ..Default::default()
        };
        let node = node.check().expect("well-formed phrases");
        let compiled = node
            .into_node(&McpAccess::Unconnected, vec![])
            .await
            .unwrap();
        let compiled = serde_json::to_string(&compiled).unwrap();
        assert!(!compiled.contains("Almost there"), "{compiled}");

        let blank = NodeDefinition {
            waiting_phrases: vec!["ok".to_string(), "  ".to_string()],
            ..Default::default()
        };
        let err = blank.check().expect_err("blank phrase").to_string();
        assert!(err.contains("waiting phrase 2"), "{err}");

        let long = NodeDefinition {
            waiting_phrases: vec!["a".repeat(MAX_WAITING_PHRASE_CHARS + 1)],
            ..Default::default()
        };
        let err = long.check().expect_err("long phrase").to_string();
        assert!(err.contains("longer than"), "{err}");
    }

    #[test]
    fn check_rejects_zero_max_iterations() {
        let node = NodeDefinition {
            max_iterations: 0,
            ..Default::default()
        };
        let err = node.check().expect_err("zero iterations").to_string();
        assert!(err.contains("max_iterations"), "{err}");
    }

    #[test]
    fn check_rejects_output_format_with_tools() {
        let node = NodeDefinition {
            output_format: Some(TemplateField::Object(JsonSchema::default())),
            ..node_with(vec![template_tool("t1")])
        };
        let err = node.check().expect_err("schema and tools").to_string();
        assert!(err.contains("output schema and also tools"), "{err}");
    }

    #[test]
    fn check_rejects_summarization_prompt_without_tools() {
        let node = NodeDefinition {
            id: "node_1".to_string(),
            summarization_prompt: Some("summarise this".to_string()),
            ..Default::default()
        };
        let err = node.check().expect_err("prompt without tools").to_string();
        assert!(err.contains("summarization_prompt but no tools"), "{err}");
    }

    /// A workflow tool holds nodes of its own, and they are checked too.
    #[test]
    fn check_descends_into_workflow_tools() {
        let node = node_with(vec![tool(
            r#"
            name: wf_tool
            tool:
              type: workflow
              id: inner_wf
              input_prompt: do the thing
              nodes:
                - id: inner_node
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
        )]);
        let err = node
            .check()
            .expect_err("the nested node's duplicate should be rejected")
            .to_string();
        assert!(err.contains("duplicate tool name 'repeated'"), "{err}");
        assert!(err.contains("inner_node"), "{err}");
    }
}
