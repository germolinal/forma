//! What a compiled agent *uses*, as opposed to what it does.
//!
//! An agent's definition says which tools it has, but only a compiled agent
//! knows what they turned out to be: an MCP server contributes tools whose
//! names — and existence — arrive with the connection. This module walks the
//! finished article and reports the handful of things the rest of the product
//! has to act on.

use std::collections::{BTreeMap, BTreeSet};

use agent::{Agent, ToolRunner, tool::workspace::WorkspaceScope};
use genai_client::GenaiClientTrait;
use serde::{Deserialize, Serialize};

/// The capabilities a compiled agent draws on.
///
/// Recorded on a deployment so that everything downstream can act on it without
/// compiling the agent again: the front end badges an agent that talks to MCP
/// servers, and starring one that keeps a scrapbook gives the user somewhere to
/// read what it wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFeatures {
    /// Every MCP server the agent talks to, in the order they are first
    /// reached.
    ///
    /// One server contributes a tool per capability it reports, and several
    /// nodes may list the same one, so servers are deduplicated: what matters
    /// downstream is which servers a user would have to be connected to, not
    /// how many tools came from each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,

    /// Whether the agent keeps a scrapbook — the private store, one per user
    /// per deployment, that outlives any single conversation.
    #[serde(default)]
    pub scrapbook: bool,

    /// Every workspace the agent's tools reach, and how — exactly what its
    /// deployment has to be granted, and all it should be.
    ///
    /// Derived from the tools, not from what anybody asks for: a knowledgebase
    /// needs `read`, a workspace `read` and `write`. An agent can therefore have
    /// less access than the person deploying it — a workspace they may edit,
    /// reached by the agent only through a knowledgebase tool, is granted
    /// read-only. Scrapbooks and code-execution runs are not listed: the
    /// runtime derives them from whoever is calling, and grants nothing.
    ///
    /// Sorted by path, scopes `read` before `write`, so the same agent always
    /// reports the same list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceAccess>,

    /// The kinds of tool the agent uses (see [`agent::Tool::kind`]), each once, sorted:
    /// what it can do, for whoever shows that in words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_kinds: Vec<String>,

    /// Whether the agent asks the user before calling some of its tools.
    #[serde(default)]
    pub asks_first: bool,

    /// Every tool the agent can call, first-seen order, with its kind and its
    /// bundle — the MCP server or workspace it comes from. For listing;
    /// [`AgentFeatures::tool_kinds`] is the summary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSummary>,

    /// The language models the agent's steps run on, first-seen order: which
    /// provider sees the conversation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelSummary>,
}

/// One tool an agent can call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    /// The name the model calls it by.
    pub name: String,
    /// Its kind; see [`agent::Tool::kind`].
    pub kind: String,
    /// The group it belongs to: the MCP server it came from, or the workspace
    /// definition that expanded into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
}

/// One language model an agent runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSummary {
    /// The provider, as usage is reported under it (`openai`, `gcp.vertex_ai`…).
    pub provider: String,
    /// The model's name at that provider.
    pub model: String,
}

/// One workspace an agent reaches, and the access its tools need to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAccess {
    /// The workspace, as a deployment's `workspaces` grant names it:
    /// `<bucket>/<workspace id>`.
    pub path: String,
    /// `read`, `write`, or both.
    pub scopes: Vec<String>,
}

impl From<&Agent> for AgentFeatures {
    fn from(agent: &Agent) -> Self {
        let mut features = Self::default();
        let mut workspaces: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
        let mut kinds: BTreeSet<&'static str> = BTreeSet::new();
        for tool in agent.tools() {
            let kind = tool.kind();
            kinds.insert(kind);
            features.asks_first |= tool.confirm_before_calling;
            // A server compiled without connecting to it stands in as one
            // placeholder: it counts as a server, and as the `mcp` kind, but
            // what its tools are called is not known.
            let placeholder = matches!(&tool.tool, ToolRunner::Mcp(m) if m.is_unconnected());
            if !placeholder && !features.tools.iter().any(|t| t.name == tool.name) {
                features.tools.push(ToolSummary {
                    name: tool.name.clone(),
                    kind: kind.to_string(),
                    bundle: tool.bundle.clone(),
                });
            }
            match &tool.tool {
                ToolRunner::Mcp(mcp) => {
                    if !features.mcp_servers.iter().any(|s| s == &mcp.server_url) {
                        features.mcp_servers.push(mcp.server_url.clone());
                    }
                }
                ToolRunner::Knowledgebase(kb) => {
                    if kb.scope == WorkspaceScope::Scrapbook {
                        features.scrapbook = true;
                    }
                    if let Some((path, scope)) = kb.required_grant() {
                        workspaces.entry(path).or_default().insert(scope);
                    }
                }
                _ => {}
            }
        }
        features.tool_kinds = kinds.into_iter().map(str::to_string).collect();
        for node in agent.nodes() {
            for llm in [&node.llm, &node.summarization_llm] {
                let model = ModelSummary {
                    provider: llm.otel_provider().to_string(),
                    model: llm.model().to_string(),
                };
                if !model.model.is_empty() && !features.models.contains(&model) {
                    features.models.push(model);
                }
            }
        }
        features.workspaces = workspaces
            .into_iter()
            .map(|(path, scopes)| WorkspaceAccess {
                path,
                // `read` sorts before `write` already; spelled out anyway.
                scopes: ["read", "write"]
                    .into_iter()
                    .filter(|s| scopes.contains(s))
                    .map(str::to_string)
                    .collect(),
            })
            .collect();
        features
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::tool::TemplateTool;
    use agent::tool::mcp_tool::McpTool;
    use agent::tool::workspace::gcs::GcsDocConfig;
    use agent::tool::workspace::{DocStoreBackend, KnowledgeBaseOperation, KnowledgebaseTool};
    use agent::{Node, Tool, Workflow};
    use genai_client::JsonSchema;
    use std::collections::HashSet;

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

    fn docstore_tool(name: &str, scope: WorkspaceScope) -> Tool {
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

    fn template_tool(name: &str) -> Tool {
        tool(
            name,
            ToolRunner::Template(TemplateTool {
                templates: vec!["result".to_string()],
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

    fn workflow_with(id: &str, nodes: Vec<Node>) -> Workflow {
        let output = nodes.iter().map(|n| n.id.clone()).collect::<HashSet<_>>();
        Workflow {
            id: id.to_string(),
            plan: vec![nodes],
            output,
            input_prompt: None,
            evals: vec![],
        }
    }

    fn agent_with(nodes: Vec<Node>) -> Agent {
        Agent {
            id: "agent_1".to_string(),
            start: workflow_with("wf_1", nodes),
            ..Default::default()
        }
    }

    #[test]
    fn plain_agent_has_no_features() {
        // Nothing to act on or warn about: no servers, scrapbook, workspaces or
        // confirmations. Its tool, its kind and its model are still listed.
        let agent = agent_with(vec![node_with("node_a", vec![template_tool("t1")])]);
        let features = AgentFeatures::from(&agent);
        assert!(features.mcp_servers.is_empty());
        assert!(!features.scrapbook);
        assert!(features.workspaces.is_empty());
        assert!(!features.asks_first);
        assert_eq!(features.tool_kinds, vec!["template"]);
        assert_eq!(features.tools.len(), 1);
        assert_eq!(features.models.len(), 1);
    }

    #[test]
    fn collects_mcp_servers_in_order_without_repeats() {
        let agent = agent_with(vec![
            node_with(
                "node_a",
                vec![
                    mcp_tool("first", "https://mcp.example"),
                    mcp_tool("second", "https://mcp.example"),
                ],
            ),
            node_with("node_b", vec![mcp_tool("third", "https://other.example")]),
        ]);
        let features = AgentFeatures::from(&agent);
        assert_eq!(
            features.mcp_servers,
            vec!["https://mcp.example", "https://other.example"]
        );
    }

    #[test]
    fn finds_a_scrapbook() {
        let agent = agent_with(vec![node_with(
            "node_a",
            vec![docstore_tool("read-from-notes", WorkspaceScope::Scrapbook)],
        )]);
        assert!(AgentFeatures::from(&agent).scrapbook);
    }

    /// A shared workspace is the same kind of tool, but it belongs to the
    /// agent's author rather than to whoever is talking to it.
    #[test]
    fn a_shared_workspace_is_not_a_scrapbook() {
        let agent = agent_with(vec![node_with(
            "node_a",
            vec![docstore_tool("read-from-handbook", WorkspaceScope::Shared)],
        )]);
        assert!(!AgentFeatures::from(&agent).scrapbook);
    }

    /// Tools reached through a workflow tool are the agent's too — a scrapbook
    /// two workflows down is still a scrapbook the user will be given.
    #[test]
    fn reaches_tools_inside_nested_workflows() {
        let inner = workflow_with(
            "inner_wf",
            vec![node_with(
                "inner_node",
                vec![
                    mcp_tool("nested", "https://nested.example"),
                    docstore_tool("read-from-notes", WorkspaceScope::Scrapbook),
                ],
            )],
        );
        let agent = agent_with(vec![node_with(
            "node_a",
            vec![tool("wf_tool", ToolRunner::Workflow(inner))],
        )]);
        let features = AgentFeatures::from(&agent);
        assert_eq!(features.mcp_servers, vec!["https://nested.example"]);
        assert!(features.scrapbook);
    }

    fn store_tool(
        name: &str,
        workspace: &str,
        op: KnowledgeBaseOperation,
        scope: WorkspaceScope,
    ) -> Tool {
        tool(
            name,
            ToolRunner::Knowledgebase(KnowledgebaseTool {
                backend: DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                    bucket: "a_bucket".to_string(),
                    workspace_id: workspace.to_string(),
                }),
                op,
                scope,
            }),
        )
    }

    #[test]
    fn workspaces_need_what_their_tools_do() {
        use KnowledgeBaseOperation::*;
        let agent = agent_with(vec![node_with(
            "main",
            vec![
                // A knowledgebase: read only, however many tools read it.
                store_tool("list-handbook", "handbook", List, WorkspaceScope::Shared),
                store_tool(
                    "read-from-handbook",
                    "handbook",
                    Read,
                    WorkspaceScope::Shared,
                ),
                // A workspace: read and write.
                store_tool("read-from-ledger", "ledger", Read, WorkspaceScope::Shared),
                store_tool("write-to-ledger", "ledger", Write, WorkspaceScope::Shared),
                // Derived from the caller: never granted.
                store_tool("write-to-notes", "", Write, WorkspaceScope::Scrapbook),
                store_tool("read-from-runs", "", Read, WorkspaceScope::Runs),
            ],
        )]);
        let features = AgentFeatures::from(&agent);
        assert_eq!(
            features.workspaces,
            vec![
                WorkspaceAccess {
                    path: "a_bucket/handbook".to_string(),
                    scopes: vec!["read".to_string()],
                },
                WorkspaceAccess {
                    path: "a_bucket/ledger".to_string(),
                    scopes: vec!["read".to_string(), "write".to_string()],
                },
            ]
        );
        assert!(features.scrapbook);
    }

    #[test]
    fn workspaces_inside_nested_workflows_are_counted() {
        use KnowledgeBaseOperation::*;
        let inner = workflow_with(
            "inner",
            vec![node_with(
                "deep",
                vec![store_tool(
                    "write-to-ledger",
                    "ledger",
                    Write,
                    WorkspaceScope::Shared,
                )],
            )],
        );
        let agent = agent_with(vec![node_with(
            "main",
            vec![tool("sub", ToolRunner::Workflow(inner))],
        )]);
        let features = AgentFeatures::from(&agent);
        assert_eq!(features.workspaces.len(), 1);
        assert_eq!(features.workspaces[0].scopes, vec!["write".to_string()]);
    }

    #[test]
    fn tool_kinds_are_the_runners_own_but_stores_say_whose() {
        use KnowledgeBaseOperation::*;
        let mut asks = mcp_tool("play", "https://mcp.spotify.com");
        asks.confirm_before_calling = true;
        let agent = agent_with(vec![node_with(
            "main",
            vec![
                asks,
                store_tool("write-to-ledger", "ledger", Write, WorkspaceScope::Shared),
                store_tool("read-from-ledger", "ledger", Read, WorkspaceScope::Shared),
                store_tool("write-to-notes", "", Write, WorkspaceScope::Scrapbook),
                // Reading a run's output counts as running code.
                store_tool("read-from-sandbox", "", Read, WorkspaceScope::Runs),
                template_tool("answer"),
            ],
        )]);
        let features = AgentFeatures::from(&agent);
        // `mcp` and `template` as the runners report them; the store tools as
        // whose store they reach. Sorted, each once.
        assert_eq!(
            features.tool_kinds,
            vec![
                "code-execution",
                "knowledgebase",
                "mcp",
                "scrapbook",
                "template",
                "workspace"
            ]
        );
        assert!(features.asks_first);
        // Every tool is listed once, with its kind.
        assert_eq!(features.tools.len(), 6);
        let kind = |name: &str| {
            features
                .tools
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.kind.clone())
                .unwrap()
        };
        assert_eq!(kind("play"), "mcp");
        assert_eq!(kind("write-to-ledger"), "workspace");
        assert_eq!(kind("read-from-ledger"), "knowledgebase");
        assert_eq!(kind("read-from-sandbox"), "code-execution");
    }

    #[test]
    fn a_tool_in_two_nodes_is_listed_once() {
        let agent = agent_with(vec![
            node_with("a", vec![template_tool("answer")]),
            node_with("b", vec![template_tool("answer")]),
        ]);
        assert_eq!(AgentFeatures::from(&agent).tools.len(), 1);
    }

    #[test]
    fn models_are_listed_once_each() {
        // The default node runs on one model for both answering and
        // summarising; it is reported once.
        let agent = agent_with(vec![node_with("a", vec![]), node_with("b", vec![])]);
        let models = AgentFeatures::from(&agent).models;
        let mut seen = models.clone();
        seen.dedup();
        assert_eq!(models, seen);
        assert!(
            models
                .iter()
                .all(|m| !m.provider.is_empty() && !m.model.is_empty())
        );
    }
}
