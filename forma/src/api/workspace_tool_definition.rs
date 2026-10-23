//! Tool definitions for agentic knowledgebase retrieval.
//!
//! A knowledgebase definition does not become a single tool: it *expands* into
//! the tools an agent needs to explore a corpus agentically — `list-<name>`
//! and `read-from-<name>`, plus the tools that change documents for a
//! workspace — all grouped under one bundle. The agent navigates the way a
//! person would: list what is there, then read what is relevant (in slices,
//! if large; a picture or a PDF is attached instead).
//!
//! Two storage backends are available: `google-knowledgebase` (a Google Cloud
//! Storage bucket) and `mongo-docstore` (a MongoDB collection).

use agent::{
    Tool, ToolRunner,
    tool::workspace::{
        DocStoreBackend, KnowledgeBaseOperation, KnowledgebaseTool, WorkspaceScope,
        default_workspace_bucket, gcs::GcsDocConfig,
    },
};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// Exposes a knowledgebase stored in a Google Cloud Storage bucket.
///
/// Documents are plain-text objects (Markdown, txt, etc.) in the bucket,
/// optionally under a `prefix` that scopes the knowledgebase. Authentication
/// uses Google Cloud application-default credentials — like the
/// `google-agent-search` tool, no API key env var is required. The caller needs
/// read access to the bucket (`roles/storage.objectViewer` should do).
///
/// Where the documents are kept is the server's choice, not the definition's
/// (`workspaces` in its [runtime file](./auto-formaruntime.md)): Cloud Storage,
/// or files under `<dir>/<workspace_id>/` in a local folder, in which case
/// `bucket` is not used. The same definition works with both.
///
/// This definition expands into tools named after the definition:
/// `list-<name>` and `read-from-<name>`, plus the tools that change documents
/// (`write-to-<name>`, `append-to-<name>`, `edit-in-<name>`, `move-in-<name>`,
/// `delete-from-<name>`) for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
pub struct GoogleWorkspaceDefinition {
    /// The bucket that holds the documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,

    /// An optional object-name prefix scoping this knowledgebase within the
    /// bucket (e.g. `handbook/`). Paths shown to the agent are relative to it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,

    /// Optional instructions for the AI to know how/when to use
    /// this workspace.
    pub usage_guidelines: Option<String>,
}

impl ObjectDefinition for GoogleWorkspaceDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl From<GoogleWorkspaceDefinition> for DocStoreBackend {
    fn from(value: GoogleWorkspaceDefinition) -> Self {
        let bucket = value.bucket.unwrap_or_else(default_workspace_bucket);
        DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
            bucket,
            workspace_id: value.workspace_id,
        })
    }
}

/// A private, per-user scratchpad the agent can read and write.
///
/// A scrapbook behaves exactly like a workspace, but it is not a place the
/// agent's author picks: it belongs to the user talking to the agent, and its
/// location is derived at call time from that user and the deployed agent they
/// are talking to. That makes it opt-in rather than granted — an agent has a
/// scrapbook because its definition asks for one, and it needs no entry in the
/// deployment's `workspaces` credential, because there is no way for it to
/// address anybody else's.
///
/// It always lives in the server's workspace store — the bucket named by the
/// `WORKSPACES_BUCKET` environment variable, or the local folder its
/// [runtime file](./auto-formaruntime.md) names; unlike a workspace, the author
/// cannot choose one.
///
/// This definition expands into three tools named after the definition:
/// `list-<name>`, `read-from-<name>`, and the tools that change documents:
/// `write-to-<name>`, `append-to-<name>`, `edit-in-<name>`, `move-in-<name>`
/// and `delete-from-<name>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, FormaDocs)]
pub struct ScrapbookDefinition {
    /// Optional instructions for the AI to know how/when to use
    /// this workspace.
    pub usage_guidelines: Option<String>,
}

impl ObjectDefinition for ScrapbookDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl From<ScrapbookDefinition> for DocStoreBackend {
    fn from(_value: ScrapbookDefinition) -> Self {
        // `workspace_id` is a placeholder: a scrapbook's prefix is derived from
        // the caller at call time and overwrites whatever is set here.
        DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
            bucket: default_workspace_bucket(),
            workspace_id: String::new(),
        })
    }
}

/// The operations a workspace exposes.
pub(crate) const WORKSPACE_OPS: &[KnowledgeBaseOperation] = &[
    KnowledgeBaseOperation::List,
    KnowledgeBaseOperation::Read,
    KnowledgeBaseOperation::Write,
    KnowledgeBaseOperation::Append,
    KnowledgeBaseOperation::Edit,
    KnowledgeBaseOperation::Move,
    KnowledgeBaseOperation::Delete,
];

/// The operations a scrapbook exposes: the same as a workspace's.
pub(crate) const SCRAPBOOK_OPS: &[KnowledgeBaseOperation] = WORKSPACE_OPS;

/// The operations a knowledgebase exposes. Read-only, unlike a workspace.
pub(crate) const KNOWLEDGEBASE_OPS: &[KnowledgeBaseOperation] =
    &[KnowledgeBaseOperation::List, KnowledgeBaseOperation::Read];

/// The name the tool for `op` over the docstore called `base_name` answers to.
///
/// Split out from [`docstore_tools`] so that a definition can be asked what it
/// will be called without being compiled — the tools themselves need a backend,
/// which needs a bucket, which a check should not have to invent.
pub(crate) fn docstore_tool_name(op: KnowledgeBaseOperation, base_name: &str) -> String {
    format!("{}-{base_name}", op.prefix())
}

/// Expands one workspace backend into the four agentic tools
/// (`<base_name>_list`, `<base_name>_read`, `<base_name>_search`,
/// `<base_name>_write`), grouped under a single bundle so clients can render
/// them together.
pub(crate) fn workspace_tools(
    backend: DocStoreBackend,
    base_name: &str,
    description: Option<&str>,
    usage_guidelines: Option<&str>,
    confirm_before_calling: bool,
    just_for: &[String],
    bundle: Option<String>,
) -> Vec<Tool> {
    docstore_tools(
        WORKSPACE_OPS,
        WorkspaceScope::Shared,
        backend,
        base_name,
        description,
        usage_guidelines,
        confirm_before_calling,
        just_for,
        bundle,
    )
}

/// Expands one scrapbook into the same read/write tool set as a workspace
/// (`<base_name>_list`, `<base_name>_read`, `<base_name>_write`), scoped to the
/// caller instead of to a configured path.
pub(crate) fn scrapbook_tools(
    backend: DocStoreBackend,
    base_name: &str,
    description: Option<&str>,
    usage_guidelines: Option<&str>,
    confirm_before_calling: bool,
    just_for: &[String],
    bundle: Option<String>,
) -> Vec<Tool> {
    docstore_tools(
        SCRAPBOOK_OPS,
        WorkspaceScope::Scrapbook,
        backend,
        base_name,
        description,
        usage_guidelines,
        confirm_before_calling,
        just_for,
        bundle,
    )
}

pub(crate) fn knowledgebase_tools(
    backend: DocStoreBackend,
    base_name: &str,
    description: Option<&str>,
    usage_guidelines: Option<&str>,
    confirm_before_calling: bool,
    just_for: &[String],
    bundle: Option<String>,
) -> Vec<Tool> {
    docstore_tools(
        KNOWLEDGEBASE_OPS,
        WorkspaceScope::Shared,
        backend,
        base_name,
        description,
        usage_guidelines,
        confirm_before_calling,
        just_for,
        bundle,
    )
}

/// Expands one docstore `backend` into one [`Tool`] per requested `op`, all
/// grouped under a single bundle. Shared by the read-only knowledgebase
/// definition and the read/write workspace definition; the caller decides which
/// operations to expose.
#[allow(clippy::too_many_arguments)]
pub(crate) fn docstore_tools(
    ops: &[KnowledgeBaseOperation],
    scope: WorkspaceScope,
    backend: DocStoreBackend,
    base_name: &str,
    description: Option<&str>,
    usage_guidelines: Option<&str>,
    confirm_before_calling: bool,
    just_for: &[String],
    bundle: Option<String>,
) -> Vec<Tool> {
    ops.iter()
        .copied()
        .map(|op| {
            let mut tool_description = op.describe(Some(base_name), scope);
            if let Some(about) = description {
                // replace newlines by spaces
                let about = about.replace('\n', " ");
                let about = about.trim();
                tool_description.push_str("<content-description>");
                tool_description.push_str(about);
                tool_description.push_str("</content-description>");
            }
            if let Some(guidelines) = usage_guidelines {
                let guidelines = guidelines.replace('\n', " ");
                let guidelines = guidelines.trim();
                tool_description.push_str("<usage-guidelines>");
                tool_description.push_str(guidelines);
                tool_description.push_str("</usage-guidelines>");
            }
            Tool {
                name: docstore_tool_name(op, base_name),
                description: Some(tool_description),
                tool: ToolRunner::Knowledgebase(KnowledgebaseTool {
                    backend: backend.clone(),
                    op,
                    scope,
                }),
                just_for: just_for.to_vec(),
                confirm_before_calling,
                bundle: bundle.clone().or_else(|| Some(base_name.to_string())),
            }
        })
        .collect()
}

/// Expands one knowledgebase backend into the three agentic-retrieval tools
/// (`<base_name>_list`, `<base_name>_read`, `<base_name>_search`), grouped
/// under a single bundle so clients can render them together.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::mcp_tool_definition::McpAccess;
    use crate::api::tool_definition::{ToolDefinition, ToolRunnerDefinition};
    use protocols::EncryptedTokens;

    #[tokio::test]
    async fn test_google_docstore_expands_into_three_tools() {
        let def: ToolDefinition = serde_yaml::from_str(
            r#"
            name: handbook
            description: The company handbook
            tool:
              type: google-knowledgebase
              bucket: my-kb-bucket
              prefix: handbook/
            "#,
        )
        .unwrap();

        let tools = def
            .into_tools(&McpAccess::Connect(EncryptedTokens::default()), vec![])
            .await
            .unwrap();

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // Read attaches what it cannot give as text.
        assert_eq!(names, vec!["list-handbook", "read-from-handbook"]);
        for tool in &tools {
            // All grouped under the definition's name by default
            assert_eq!(tool.bundle.as_deref(), Some("handbook"));
            // The user's description is surfaced to the LLM on every tool
            assert!(
                tool.description
                    .as_deref()
                    .unwrap()
                    .contains("The company handbook")
            );
            assert!(matches!(tool.tool, ToolRunner::Knowledgebase(_)));
        }
    }

    #[test]
    fn test_runner_definition_kebab_tags() {
        let def: ToolRunnerDefinition =
            serde_yaml::from_str("type: google-knowledgebase\nbucket: b").unwrap();
        assert!(matches!(def, ToolRunnerDefinition::GoogleKnowledgebase(_)));

        let def: ToolRunnerDefinition = serde_yaml::from_str("type: scrapbook").unwrap();
        assert!(matches!(def, ToolRunnerDefinition::Scrapbook(_)));
    }

    #[tokio::test]
    async fn test_scrapbook_expands_into_read_write_tools() {
        // A scrapbook is opt-in and self-describing: the definition names no
        // bucket and no path, because both come from elsewhere at call time.
        let def: ToolDefinition = serde_yaml::from_str(
            r#"
            name: notes
            description: Where you keep track of what matters to this user
            tool:
              type: scrapbook
            "#,
        )
        .unwrap();

        let tools = def
            .into_tools(&McpAccess::Connect(EncryptedTokens::default()), vec![])
            .await
            .unwrap();

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "list-notes",
                "read-from-notes",
                "write-to-notes",
                "append-to-notes",
                "edit-in-notes",
                "move-in-notes",
                "delete-from-notes"
            ]
        );

        for tool in &tools {
            assert_eq!(tool.bundle.as_deref(), Some("notes"));
            let ToolRunner::Knowledgebase(kb) = &tool.tool else {
                panic!("{} is not a knowledgebase tool", tool.name);
            };
            assert_eq!(kb.scope, WorkspaceScope::Scrapbook);
        }
    }
}
