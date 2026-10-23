//! Tool definition for running code in a sandbox.
//!
//! A code-execution definition expands into two tools: `run-in-<name>`, which
//! runs a command in a fresh, isolated container and waits for it, and
//! `read-from-<name>`, which reads what runs left behind (their outputs and
//! full logs) for about a day.

use agent::{
    Tool, ToolRunner,
    tool::{
        code_execution::{CodeExecutionTool, describe_run},
        workspace::{KnowledgeBaseOperation, KnowledgebaseTool, WorkspaceScope, gcs::GcsDocConfig},
    },
};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::{ObjectDefinition, workspace_tool_definition::docstore_tool_name};

/// Lets the agent run shell commands and Python scripts in an isolated sandbox.
///
/// Every run starts a fresh container with no internet access, bash, the usual
/// text tools (grep, sed, gawk) and Python 3.12 with numpy, pandas, scipy,
/// statsmodels, matplotlib and openpyxl. The agent passes the files it
/// needs — written by itself, taken from one of its workspaces, or produced by
/// an earlier run — and gets back the exit status, the end of stdout/stderr and
/// the list of output files. Outputs can be copied into a workspace the agent
/// can write to.
///
/// Where runs execute is up to whoever runs the server (`code_execution` in
/// its [runtime file](./auto-formaruntime.md)), and is the same for every agent
/// on it; a definition cannot choose, which keeps agents portable. Run folders
/// are kept in the server's `WORKSPACES_BUCKET`.
///
/// This definition expands into two tools named after the definition:
/// `run-in-<name>` and `read-from-<name>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct CodeExecutionDefinition {}

impl ObjectDefinition for CodeExecutionDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

/// The operations over past runs that come with a code-execution tool.
const RUN_STORE_OPS: &[KnowledgeBaseOperation] = &[KnowledgeBaseOperation::Read];

/// The name of the tool that starts a run.
pub(crate) fn run_tool_name(base_name: &str) -> String {
    format!("run-in-{base_name}")
}

/// Every tool name a code-execution definition called `base_name` produces.
pub(crate) fn code_execution_tool_names(base_name: &str) -> Vec<String> {
    let mut names = vec![run_tool_name(base_name)];
    names.extend(
        RUN_STORE_OPS
            .iter()
            .map(|op| docstore_tool_name(*op, base_name)),
    );
    names
}

fn append_guidance(description: &mut String, about: Option<&str>) {
    if let Some(about) = about {
        description.push_str("<content-description>");
        description.push_str(about.replace('\n', " ").trim());
        description.push_str("</content-description>");
    }
}

/// Expands a code-execution definition into its two tools, grouped under one
/// bundle.
pub(crate) fn code_execution_tools(
    _definition: CodeExecutionDefinition,
    base_name: &str,
    description: Option<&str>,
    confirm_before_calling: bool,
    just_for: &[String],
    bundle: Option<String>,
) -> Vec<Tool> {
    let bundle = bundle.or_else(|| Some(base_name.to_string()));
    let reader = docstore_tool_name(KnowledgeBaseOperation::Read, base_name);

    let mut run_description = describe_run(&reader);
    append_guidance(&mut run_description, description);
    let mut tools = vec![Tool {
        name: run_tool_name(base_name),
        description: Some(run_description),
        tool: ToolRunner::CodeExecution(CodeExecutionTool::default()),
        just_for: just_for.to_vec(),
        confirm_before_calling,
        bundle: bundle.clone(),
    }];

    // Reading back is part of running: the same people may do it, and asking
    // for confirmation to read a run's own output would only get in the way.
    for op in RUN_STORE_OPS {
        tools.push(Tool {
            name: docstore_tool_name(*op, base_name),
            description: Some(op.describe(Some(base_name), WorkspaceScope::Runs)),
            tool: ToolRunner::Knowledgebase(KnowledgebaseTool {
                // The prefix is derived from the caller at call time, like a
                // scrapbook's; the bucket is the operator's.
                backend: agent::tool::workspace::DocStoreBackend::GoogleKnowledgebase(
                    GcsDocConfig {
                        bucket: agent::tool::workspace::default_workspace_bucket(),
                        workspace_id: String::new(),
                    },
                ),
                op: *op,
                scope: WorkspaceScope::Runs,
            }),
            just_for: just_for.to_vec(),
            confirm_before_calling: false,
            bundle: bundle.clone(),
        });
    }
    tools
}

#[cfg(test)]
mod tests {
    use crate::api::mcp_tool_definition::McpAccess;
    use crate::api::tool_definition::ToolDefinition;
    use agent::ToolRunner;
    use agent::tool::workspace::WorkspaceScope;
    use protocols::EncryptedTokens;

    #[tokio::test]
    async fn test_code_execution_expands_into_run_and_read() {
        let def: ToolDefinition = serde_yaml::from_str(
            r#"
            name: sandbox
            tool:
              type: code-execution
            "#,
        )
        .unwrap();
        assert_eq!(
            def.tool_names().unwrap(),
            vec!["run-in-sandbox", "read-from-sandbox"]
        );

        let tools = def
            .into_tools(&McpAccess::Connect(EncryptedTokens::default()), vec![])
            .await
            .unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["run-in-sandbox", "read-from-sandbox"]);
        assert!(matches!(tools[0].tool, ToolRunner::CodeExecution(_)));
        for tool in &tools[1..] {
            let ToolRunner::Knowledgebase(kb) = &tool.tool else {
                panic!("{} should read runs", tool.name);
            };
            assert_eq!(kb.scope, WorkspaceScope::Runs);
        }
        for tool in &tools {
            assert_eq!(tool.bundle.as_deref(), Some("sandbox"));
        }
    }

    #[test]
    fn test_the_definition_cannot_choose_where_code_runs() {
        // The sandbox is the runtime's, the same for every agent on it. An
        // author who tries to pick one is told so rather than silently ignored.
        for yaml in [
            "name: s\ntool:\n  type: code-execution\n  backend: local-docker\n",
            "name: s\ntool:\n  type: code-execution\n  backend:\n    type: cloud-run-job\n    job: projects/p/locations/r/jobs/j\n",
            "name: s\ntool:\n  type: code-execution\n  image: my/image\n",
        ] {
            assert!(
                serde_yaml::from_str::<ToolDefinition>(yaml).is_err(),
                "{yaml}"
            );
        }
    }
}
