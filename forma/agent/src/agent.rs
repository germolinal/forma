use std::collections::{HashMap, VecDeque};

use crate::{
    Callable, Eval, Node, Tool, ToolRunner, Workflow,
    callable::{CallContext, Identifiable},
    eval_agent::{Evaluatable, EvaluatableSet, SingleEvaluatable},
    template::is_valid_variable_name,
};
use async_trait::async_trait;

use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use tracing::info;

pub fn default_agent_id() -> String {
    // Make sure it does not start with a number
    let ret = uuid::Uuid::new_v4().to_string();
    let tail = &ret[1..];
    let ret = format!("a{tail}");
    ret
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    #[serde(default = "default_agent_id")]
    pub id: String,
    pub start: Workflow,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, serde_json::Value>,
    /// The time zone to assume when the user's client does not send one
    /// (an IANA name). Read by the `current-date` tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// The evaluations to run
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub evals: Vec<Eval>,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            id: default_agent_id(),
            start: Default::default(),
            evals: vec![],
            variables: HashMap::with_capacity(0),
            timezone: None,
        }
    }
}

impl Agent {
    /// Walks every [`Node`] the agent can reach.
    ///
    /// Useful for whole-agent questions that the per-node and per-workflow
    /// checks cannot answer on their own — such as whether two nodes anywhere
    /// in the agent share an ID.
    pub fn nodes(&self) -> Nodes<'_> {
        Nodes::new(&self.start)
    }

    /// Walks every [`Tool`] the agent can reach, i.e. the tools of every node
    /// [`Agent::nodes`] visits.
    ///
    /// This answers "does this agent use X?" without a match arm per tool
    /// kind at every call site — every MCP server it talks to:
    ///
    /// ```
    /// use agent::{Agent, ToolRunner};
    ///
    /// let agent = Agent::default();
    /// let servers: Vec<&str> = agent
    ///     .tools()
    ///     .filter_map(|tool| match &tool.tool {
    ///         ToolRunner::Mcp(mcp) => Some(mcp.server_url.as_str()),
    ///         _ => None,
    ///     })
    ///     .collect();
    /// assert!(servers.is_empty());
    /// ```
    ///
    /// ...or whether it keeps a scrapbook:
    ///
    /// ```
    /// use agent::{Agent, ToolRunner, tool::workspace::WorkspaceScope};
    ///
    /// let agent = Agent::default();
    /// let uses_scrapbook = agent.tools().any(|tool| {
    ///     matches!(&tool.tool, ToolRunner::Knowledgebase(kb) if kb.scope == WorkspaceScope::Scrapbook)
    /// });
    /// assert!(!uses_scrapbook);
    /// ```
    ///
    /// Tools arrive without saying which node they belong to. When that
    /// matters — an error message naming the offending node, say — iterate
    /// [`Agent::nodes`] and read each node's `tools` instead.
    pub fn tools(&self) -> Tools<'_> {
        Tools::new(&self.start)
    }
}

/// Iterator over the nodes reachable from a [`Workflow`], created by
/// [`Agent::nodes`] or [`Nodes::new`].
///
/// Yields the workflow's own nodes first, in plan order, and then the nodes of
/// every workflow reached through a tool — those are part of the agent in every
/// way that matters here: they call LLMs, carry tools of their own, and draw
/// their IDs from the same namespace as everything else.
///
/// Nested workflows are owned by the tool holding them, so the structure is a
/// tree: every node is visited exactly once and the walk always terminates.
pub struct Nodes<'a> {
    /// Nodes found but not yet returned. A node's nested workflows are only
    /// expanded once the node itself comes out, which keeps the walk lazy —
    /// stopping early (`.any()`, `.find()`) never pays for branches it did not
    /// reach.
    pending: VecDeque<&'a Node>,
}

impl<'a> Nodes<'a> {
    /// Starts a walk at `workflow`, rather than at an agent's `start`.
    pub fn new(workflow: &'a Workflow) -> Self {
        Self {
            pending: workflow.plan.iter().flatten().collect(),
        }
    }
}

impl<'a> Iterator for Nodes<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.pending.pop_front()?;
        for tool in &node.tools {
            if let ToolRunner::Workflow(w) = &tool.tool {
                self.pending.extend(w.plan.iter().flatten());
            }
        }
        Some(node)
    }
}

/// Iterator over the tools reachable from a [`Workflow`], created by
/// [`Agent::tools`] or [`Tools::new`].
///
/// Follows [`Nodes`], returning each node's tools in declaration order. A
/// workflow tool is returned like any other, before the tools of the workflow
/// it wraps.
pub struct Tools<'a> {
    nodes: Nodes<'a>,
    current: std::slice::Iter<'a, Tool>,
}

impl<'a> Tools<'a> {
    /// Starts a walk at `workflow`, rather than at an agent's `start`.
    pub fn new(workflow: &'a Workflow) -> Self {
        Self {
            nodes: Nodes::new(workflow),
            current: [].iter(),
        }
    }
}

impl<'a> Iterator for Tools<'a> {
    type Item = &'a Tool;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(tool) = self.current.next() {
                return Some(tool);
            }
            // Nodes without tools are ordinary, so keep pulling until one has
            // tools rather than ending the walk at the first empty node.
            self.current = self.nodes.next()?.tools.iter();
        }
    }
}

impl Identifiable for Agent {
    const CLASS: &str = "agent";

    fn get_id(&self) -> &str {
        &self.id
    }
}

impl Evaluatable for Agent {
    fn get_evals(&self) -> &[Eval] {
        &self.evals
    }
    fn evals_mut(&mut self) -> &mut Vec<Eval> {
        &mut self.evals
    }
    fn collect_evals(&self, callable: &mut EvaluatableSet) -> Result<(), String> {
        // Insert agent
        callable.root = Some((self.id.clone(), SingleEvaluatable::new(self)));
        // And main workflow
        // insert contents
        self.start.collect_evals(callable)
    }
}

#[async_trait]
impl<DB: FormaDB> Callable<DB> for Agent {
    fn timezone(&self) -> Option<&str> {
        self.timezone.as_deref()
    }

    fn check(&self) -> Result<(), FormaError> {
        info!(agent = %self.id, "checking agent");
        is_valid_variable_name(&self.id)?;
        <Workflow as Callable<DB>>::check(&self.start)
    }

    async fn run(
        &self,
        ctx: CallContext<DB>,
        is_root: bool,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        info!(agent = %self.id, is_root, "running agent");
        self.start.run(ctx, is_root).await
    }

    fn inner_ids(&self) -> Vec<String> {
        let mut ret = Vec::new();
        ret.push(self.id.clone());
        ret.extend(<Workflow as Callable<DB>>::inner_ids(&self.start));
        ret
    }
}

#[cfg(test)]
mod tests {
    use db::NoneDB;

    use super::*;
    use crate::tool::TemplateTool;
    use crate::tool::mcp_tool::McpTool;
    use crate::tool::workspace::gcs::GcsDocConfig;
    use crate::tool::workspace::{
        DocStoreBackend, KnowledgeBaseOperation, KnowledgebaseTool, WorkspaceScope,
    };
    use genai_client::JsonSchema;
    use std::collections::HashSet;

    #[test]
    fn test_default_agent_id_format() {
        let id = default_agent_id();
        assert!(id.starts_with('a'));
        assert_eq!(id.len(), 36);
    }

    #[test]
    fn test_agent_default_initialization() {
        let agent = Agent::default();
        assert!(agent.id.starts_with('a'));
        assert!(agent.evals.is_empty());
    }

    #[test]
    fn test_agent_identifiable() {
        let agent = Agent::default();
        assert_eq!(agent.get_id(), &agent.id);
    }

    #[test]
    fn test_agent_evaluatable_evals() {
        let agent = Agent::default();
        assert!(agent.get_evals().is_empty());
    }

    #[test]
    fn test_collect_evals_sets_root() {
        let mut agent = Agent::default();
        agent.id = "test-agent".to_string();

        let mut set = EvaluatableSet::default();
        let res = agent.collect_evals(&mut set);

        assert!(
            res.is_ok(),
            "collect_evals should return Ok on default workflow"
        );
        assert!(
            set.root.is_some(),
            "collect_evals should set the root entry"
        );
        let (root_id, _single) = set.root.as_ref().unwrap();
        assert_eq!(root_id, &agent.id);
    }

    #[test]
    fn test_inner_ids_default_agent() {
        let agent = Agent::default();
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        // Agent id + default workflow id + default workflow's node id = 3
        assert!(ids.contains(&agent.id));
        assert!(ids.contains(&agent.start.id));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_inner_ids_agent_id_comes_first() {
        let agent = Agent {
            id: "agent_1".to_string(),
            ..Default::default()
        };
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        assert_eq!(ids[0], "agent_1");
    }

    #[test]
    fn test_inner_ids_empty_workflow_plan() {
        let agent = Agent {
            id: "agent_1".to_string(),
            start: Workflow {
                id: "workflow_1".to_string(),
                plan: vec![],
                output: HashSet::new(),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        assert!(ids.contains(&"agent_1".to_string()));
        assert!(ids.contains(&"workflow_1".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_inner_ids_with_nodes_in_workflow() {
        let node_a = Node {
            id: "node_a".to_string(),
            ..Default::default()
        };
        let node_b = Node {
            id: "node_b".to_string(),
            ..Default::default()
        };
        let agent = Agent {
            id: "agent_1".to_string(),
            start: Workflow {
                id: "workflow_1".to_string(),
                plan: vec![vec![node_a, node_b]],
                output: HashSet::from(["node_b".to_string()]),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        assert!(ids.contains(&"agent_1".to_string()));
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_a".to_string()));
        assert!(ids.contains(&"node_b".to_string()));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn test_inner_ids_template_tools_excluded() {
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![Tool {
                name: "t1".to_string(),
                description: None,
                tool: ToolRunner::Template(TemplateTool {
                    templates: vec!["result".to_string()],
                }),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let agent = Agent {
            id: "agent_1".to_string(),
            start: Workflow {
                id: "workflow_1".to_string(),
                plan: vec![vec![node]],
                output: HashSet::from(["node_1".to_string()]),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        // Template tools are not included
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        assert!(ids.contains(&"agent_1".to_string()));
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_1".to_string()));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_inner_ids_nested_workflow_tool() {
        let inner_node = Node {
            id: "inner_node".to_string(),
            ..Default::default()
        };
        let inner_workflow = Workflow {
            id: "inner_workflow".to_string(),
            plan: vec![vec![inner_node]],
            output: HashSet::from(["inner_node".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![Tool {
                name: "wf_tool".to_string(),
                description: None,
                tool: ToolRunner::Workflow(inner_workflow),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let agent = Agent {
            id: "agent_1".to_string(),
            start: Workflow {
                id: "workflow_1".to_string(),
                plan: vec![vec![node]],
                output: HashSet::from(["node_1".to_string()]),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        let ids = <Agent as Callable<NoneDB>>::inner_ids(&agent);
        assert!(ids.contains(&"agent_1".to_string()));
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_1".to_string()));
        assert!(ids.contains(&"inner_workflow".to_string()));
        assert!(ids.contains(&"inner_node".to_string()));
        assert_eq!(ids.len(), 5);
    }

    // ── Agent::check ─────────────────────────────────────────────────────────

    #[test]
    fn test_check_valid_agent() {
        let agent = Agent {
            id: "my_agent".to_string(),
            start: Workflow {
                id: "wf_1".to_string(),
                plan: vec![vec![Node {
                    id: "node_1".to_string(),
                    ..Default::default()
                }]],
                output: HashSet::from(["node_1".to_string()]),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_agent_invalid_id() {
        let agent = Agent {
            id: "1invalid".to_string(),
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("1invalid"));
    }

    #[test]
    fn test_check_agent_invalid_id_with_space() {
        let agent = Agent {
            id: "my agent".to_string(),
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("my agent"));
    }

    #[test]
    fn test_check_agent_empty_id() {
        let agent = Agent {
            id: String::new(),
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_agent_propagates_workflow_error() {
        // A workflow whose output references a non-existent node ID
        let agent = Agent {
            id: "valid_agent".to_string(),
            start: Workflow {
                id: "wf_1".to_string(),
                plan: vec![vec![Node {
                    id: "real_node".to_string(),
                    ..Default::default()
                }]],
                output: HashSet::from(["ghost_node".to_string()]),
                input_prompt: None,
                evals: vec![],
            },
            evals: vec![],
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("ghost_node"));
    }

    #[test]
    fn test_check_agent_propagates_node_error() {
        // A node inside the workflow has an invalid ID
        let agent = Agent {
            id: "valid_agent".to_string(),
            start: Workflow {
                id: "wf_1".to_string(),
                plan: vec![vec![Node {
                    id: "1bad_node".to_string(),
                    ..Default::default()
                }]],
                output: HashSet::from(["1bad_node".to_string()]),
                input_prompt: None,
                ..Default::default()
            },
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("1bad_node"));
    }

    #[test]
    fn test_check_agent_valid_id_with_underscore_and_hyphen() {
        let agent = Agent {
            id: "my_agent-v2".to_string(),
            start: Workflow {
                id: "wf_1".to_string(),
                plan: vec![vec![Node {
                    id: "node_1".to_string(),
                    ..Default::default()
                }]],
                output: HashSet::from(["node_1".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };
        let check = <Agent as Callable<NoneDB>>::check(&agent);
        assert!(check.is_ok());
    }

    // ── Agent::nodes and Agent::tools ────────────────────────────────────────

    fn tool(name: &str, runner: ToolRunner) -> Tool {
        Tool {
            name: name.to_string(),
            description: None,
            tool: runner,
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        }
    }

    fn template_tool(name: &str) -> Tool {
        tool(
            name,
            ToolRunner::Template(TemplateTool {
                templates: vec!["result".to_string()],
            }),
        )
    }

    fn mcp_tool(name: &str, server_url: &str) -> Tool {
        tool(
            name,
            ToolRunner::Mcp(McpTool {
                name: name.to_string(),
                description: None,
                input_schema: JsonSchema::default(),
                server_url: server_url.to_string(),
            }),
        )
    }

    fn knowledgebase_tool(name: &str, scope: WorkspaceScope) -> Tool {
        tool(
            name,
            ToolRunner::Knowledgebase(KnowledgebaseTool {
                backend: DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                    bucket: "a_bucket".to_string(),
                    workspace_id: "notes".to_string(),
                }),
                op: KnowledgeBaseOperation::Read,
                scope,
            }),
        )
    }

    fn node_with(id: &str, tools: Vec<Tool>) -> Node {
        Node {
            id: id.to_string(),
            tools,
            ..Default::default()
        }
    }

    fn workflow_with(id: &str, plan: Vec<Vec<Node>>) -> Workflow {
        let output = plan.iter().flatten().map(|n| n.id.clone()).collect();
        Workflow {
            id: id.to_string(),
            plan,
            output,
            input_prompt: None,
            evals: vec![],
        }
    }

    fn agent_with(start: Workflow) -> Agent {
        Agent {
            id: "agent_1".to_string(),
            start,
            ..Default::default()
        }
    }

    fn node_ids(agent: &Agent) -> Vec<&str> {
        agent.nodes().map(|n| n.id.as_str()).collect()
    }

    fn tool_names(agent: &Agent) -> Vec<&str> {
        agent.tools().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn test_nodes_default_agent() {
        let agent = Agent::default();
        assert_eq!(node_ids(&agent), vec![agent.start.plan[0][0].id.as_str()]);
    }

    #[test]
    fn test_nodes_empty_plan() {
        let agent = agent_with(workflow_with("wf_1", vec![]));
        assert_eq!(node_ids(&agent).len(), 0);
    }

    #[test]
    fn test_nodes_follow_plan_order() {
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![
                vec![node_with("node_a", vec![]), node_with("node_b", vec![])],
                vec![node_with("node_c", vec![])],
            ],
        ));
        assert_eq!(node_ids(&agent), vec!["node_a", "node_b", "node_c"]);
    }

    #[test]
    fn test_nodes_descend_into_workflow_tools() {
        let inner = workflow_with("inner_wf", vec![vec![node_with("inner_node", vec![])]]);
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![
                node_with("node_a", vec![tool("wf_tool", ToolRunner::Workflow(inner))]),
                node_with("node_b", vec![]),
            ]],
        ));
        // The outer workflow is exhausted before descending into the nested one.
        assert_eq!(node_ids(&agent), vec!["node_a", "node_b", "inner_node"]);
    }

    #[test]
    fn test_nodes_descend_recursively() {
        let deepest = workflow_with("deepest_wf", vec![vec![node_with("deepest_node", vec![])]]);
        let deep = workflow_with(
            "deep_wf",
            vec![vec![node_with(
                "deep_node",
                vec![tool("deepest_tool", ToolRunner::Workflow(deepest))],
            )]],
        );
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![tool("deep_tool", ToolRunner::Workflow(deep))],
            )]],
        ));
        assert_eq!(
            node_ids(&agent),
            vec!["node_a", "deep_node", "deepest_node"]
        );
    }

    #[test]
    fn test_nodes_ignore_non_workflow_tools() {
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![template_tool("t1"), mcp_tool("t2", "https://mcp.example")],
            )]],
        ));
        assert_eq!(node_ids(&agent), vec!["node_a"]);
    }

    #[test]
    fn test_nodes_can_start_at_any_workflow() {
        let inner = workflow_with("inner_wf", vec![vec![node_with("inner_node", vec![])]]);
        let ids: Vec<&str> = Nodes::new(&inner).map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["inner_node"]);
    }

    #[test]
    fn test_tools_default_agent_has_none() {
        let agent = Agent::default();
        assert_eq!(agent.tools().count(), 0);
    }

    #[test]
    fn test_tools_follow_declaration_order() {
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![
                node_with("node_a", vec![template_tool("t1"), template_tool("t2")]),
                node_with("node_b", vec![template_tool("t3")]),
            ]],
        ));
        assert_eq!(tool_names(&agent), vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn test_tools_skip_nodes_without_tools() {
        // Nodes with no tools sit both before and after the ones that have
        // them, so an empty node can neither end the walk nor be skipped over.
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![
                node_with("node_a", vec![]),
                node_with("node_b", vec![template_tool("t1")]),
                node_with("node_c", vec![]),
                node_with("node_d", vec![template_tool("t2")]),
                node_with("node_e", vec![]),
            ]],
        ));
        assert_eq!(tool_names(&agent), vec!["t1", "t2"]);
    }

    #[test]
    fn test_tools_include_nested_workflow_tools() {
        let inner = workflow_with(
            "inner_wf",
            vec![vec![node_with(
                "inner_node",
                vec![template_tool("inner_t")],
            )]],
        );
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![tool("wf_tool", ToolRunner::Workflow(inner))],
            )]],
        ));
        // The workflow tool itself is returned too, before what it wraps.
        assert_eq!(tool_names(&agent), vec!["wf_tool", "inner_t"]);
    }

    /// Tool names are only checked for duplicates within a node; the iterator
    /// is what makes the same question answerable for a whole agent.
    #[test]
    fn test_tools_expose_duplicates_across_nodes() {
        let inner = workflow_with(
            "inner_wf",
            vec![vec![node_with("inner_node", vec![template_tool("shared")])]],
        );
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![
                node_with(
                    "node_a",
                    vec![
                        template_tool("shared"),
                        tool("wf_tool", ToolRunner::Workflow(inner)),
                    ],
                ),
                node_with("node_b", vec![template_tool("unique")]),
            ]],
        ));

        let mut seen = HashSet::new();
        let duplicates: Vec<&str> = agent
            .tools()
            .map(|t| t.name.as_str())
            .filter(|name| !seen.insert(*name))
            .collect();
        assert_eq!(duplicates, vec!["shared"]);

        // Every node passes its own check, so the duplicate is invisible there.
        for node in agent.nodes() {
            assert!(<Node as Callable<NoneDB>>::check(node).is_ok());
        }
    }

    #[test]
    fn test_tools_collect_mcp_servers() {
        let inner = workflow_with(
            "inner_wf",
            vec![vec![node_with(
                "inner_node",
                vec![mcp_tool("nested", "https://nested.example/mcp")],
            )]],
        );
        let agent = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![
                    template_tool("t1"),
                    mcp_tool("m1", "https://mcp.example"),
                    tool("wf_tool", ToolRunner::Workflow(inner)),
                ],
            )]],
        ));

        let servers: Vec<&str> = agent
            .tools()
            .filter_map(|t| match &t.tool {
                ToolRunner::Mcp(mcp) => Some(mcp.server_url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            servers,
            vec!["https://mcp.example", "https://nested.example/mcp"]
        );
    }

    #[test]
    fn test_tools_detect_scrapbook() {
        let uses_scrapbook = |agent: &Agent| {
            agent.tools().any(|t| {
                matches!(&t.tool, ToolRunner::Knowledgebase(kb)
                    if kb.scope == WorkspaceScope::Scrapbook)
            })
        };

        let with_scrapbook = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![knowledgebase_tool("notes", WorkspaceScope::Scrapbook)],
            )]],
        ));
        assert!(uses_scrapbook(&with_scrapbook));

        // A shared knowledgebase is the same tool kind but not a scrapbook.
        let with_shared_kb = agent_with(workflow_with(
            "wf_1",
            vec![vec![node_with(
                "node_a",
                vec![knowledgebase_tool("handbook", WorkspaceScope::Shared)],
            )]],
        ));
        assert!(!uses_scrapbook(&with_shared_kb));
    }
}
