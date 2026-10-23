use agent::Tool;
use agent::ToolRunner;
use agent::template::is_valid_variable_name;
use agent::tool::workspace::KnowledgeBaseOperation;
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::GoogleAgentSearchDefinition;
use crate::api::GoogleGenImageDefinition;
use crate::api::ObjectDefinition;
use crate::api::TavilyWebSearchDefinition;
use crate::api::api_call_tool_definition::ApiCallDefinition;
use crate::api::code_execution_tool_definition::{
    CodeExecutionDefinition, code_execution_tool_names, code_execution_tools,
};
use crate::api::current_date_tool_definition::CurrentDateToolDefinition;
use crate::api::delegated_tool_definition::DelegatedToolDefinition;
use crate::api::fetch_tool_definition::FetchToolDefinition;
use crate::api::mailgun_tool_definition::MailgunToolDefinition;
use crate::api::mcp_tool_definition::{McpAccess, McpToolDefinition};
use crate::api::template_tool_definition::TemplateToolDefinition;
use crate::api::workflow_definition::WorkflowToolDefinition;
use crate::api::workspace_tool_definition::{
    GoogleWorkspaceDefinition, KNOWLEDGEBASE_OPS, SCRAPBOOK_OPS, ScrapbookDefinition,
    WORKSPACE_OPS, docstore_tool_name, knowledgebase_tools, scrapbook_tools, workspace_tools,
};

/// A tool runner lets you select one of the several tools
/// that are built into Forma. They will handle the communication
/// with the LLM (in terms of inputs and outputs)
#[derive(Debug, Serialize, Deserialize, Clone, FormaDocs)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum ToolRunnerDefinition {
    /// Takes a template, asks the LLM to generate
    /// its fields, and returns a String with the rendered
    /// template
    Template(TemplateToolDefinition),
    /// Runs a workflow as if it was a tool, using a fresh
    /// state (i.e., does not receive the whole conversation)
    Workflow(WorkflowToolDefinition),
    /// Makes an HTTP REST API call. The LLM fills in URL path parameters
    /// and request body fields based on their schemas. Use `$VAR` in
    /// header values to reference environment variables for secrets.
    ApiCall(ApiCallDefinition),
    /// Asks the LLM to populate the fields that will then be sent
    /// to the client not only to validate the inputs but also to execute
    /// it. The session will be suspended until the agent receives the
    /// output of this tool.
    Delegated(DelegatedToolDefinition),
    /// Expands an MCP server into its tools.
    Mcp(McpToolDefinition),
    /// Sends an email using the Mailgun API.
    /// Requires the following Env variable
    /// - MAILGUN_API_KEY
    Mailgun(MailgunToolDefinition),

    /// Searches the web
    ///
    /// Expects env:
    /// - TAVILY_API_KEY
    TavilySearch(TavilyWebSearchDefinition),

    /// Searches a Google Vertex AI Search (Agent Builder / Discovery Engine)
    /// data store. Uses Google Cloud application-default credentials; no API
    /// key env var is required.
    GoogleAgentSearch(GoogleAgentSearchDefinition),

    /// Generates an image with a Gemini image model on Vertex AI, streaming
    /// the picture to the client while telling the LLM only that it worked.
    /// Uses Google Cloud application-default credentials; no API key env var
    /// is required.
    GoogleGenImage(GoogleGenImageDefinition),

    /// Pulls the contents of a URL
    Fetch(FetchToolDefinition),

    /// A knowledgebase stored in a Google Cloud Storage bucket. Expands into
    /// two tools — `list-<name>` and `read-from-<name>` — that let the agent
    /// explore the documents agentically. Uses Google Cloud
    /// application-default credentials; no API key env var is required. When
    /// the server keeps workspaces in a local folder (see its
    /// [runtime file](./auto-formaruntime.md)), the documents are files there
    /// instead, with no change to the definition.
    GoogleKnowledgebase(GoogleWorkspaceDefinition),

    /// A read/write workspace stored in a Google Cloud Storage bucket. Expands
    /// into `list-<name>` and `read-from-<name>`, and the tools that change
    /// documents — `write-to-<name>`, `append-to-<name>`, `edit-in-<name>`,
    /// `move-in-<name>` and `delete-from-<name>` — letting the agent explore
    /// *and persist* documents. Its changes wait for the user's review.
    /// Writing requires a write-scoped credential. Uses Google Cloud
    /// application-default credentials; no API key env var is required. When
    /// the server keeps workspaces in a local folder (see its
    /// [runtime file](./auto-formaruntime.md)), the documents are files there
    /// instead, with no change to the definition.
    GoogleWorkspace(GoogleWorkspaceDefinition),

    /// A private, persistent scratchpad belonging to the user talking to the
    /// agent. Expands into the same tools as a workspace, which behave just
    /// like a workspace's.
    ///
    /// Unlike a workspace, a scrapbook is not a place the author picks: it is
    /// located from the user making the call and the deployed agent they are
    /// talking to, so it is opt-in rather than granted — defining it is enough,
    /// and the deployment needs no `workspaces` permission, because the agent
    /// cannot address anyone else's scrapbook. The user can read and edit their
    /// own, and the agent is told so.
    ///
    /// Stored in the bucket named by `WORKSPACES_BUCKET` (application-default
    /// credentials, no API key needed), or in the local folder the server's
    /// [runtime file](./auto-formaruntime.md) names.
    Scrapbook(ScrapbookDefinition),

    /// Runs shell commands and Python scripts in an isolated sandbox: a fresh
    /// container per run, with no internet access. Expands into two tools —
    /// `run-in-<name>` and `read-from-<name>` — to run code and to read what
    /// the runs left behind.
    ///
    /// A run can take files from, and save outputs into, the workspaces the
    /// same node can reach through its own workspace tools, with the same
    /// permissions. Where the code runs is the server's to decide
    /// (`code_execution` in its [runtime file](./auto-formaruntime.md)), the
    /// same for every agent on it; runs keep their files in `WORKSPACES_BUCKET`.
    CodeExecution(CodeExecutionDefinition),

    /// Tells the agent today's date, weekday and time in the user's time
    /// zone: the one their client sends (`x-time-zone`), one the user told
    /// the agent, or the agent's `timezone`. Without any, it asks the agent to
    /// ask the user where they are.
    CurrentDate(CurrentDateToolDefinition),
}

impl Default for ToolRunnerDefinition {
    fn default() -> Self {
        Self::Template(TemplateToolDefinition::default())
    }
}

impl ObjectDefinition for ToolRunnerDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        match self {
            Self::Template(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Template(t))
            }
            Self::Workflow(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Workflow(t))
            }
            Self::ApiCall(t) => Ok(Self::ApiCall(t.expand(parent_dir)?)),
            Self::Delegated(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Delegated(t))
            }
            Self::Mcp(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Mcp(t))
            }
            Self::Mailgun(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Mailgun(t))
            }
            Self::TavilySearch(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::TavilySearch(t))
            }
            Self::GoogleAgentSearch(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::GoogleAgentSearch(t))
            }
            Self::GoogleGenImage(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::GoogleGenImage(t))
            }
            Self::Fetch(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Fetch(t))
            }
            Self::GoogleKnowledgebase(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::GoogleKnowledgebase(t))
            }
            Self::GoogleWorkspace(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::GoogleWorkspace(t))
            }
            Self::Scrapbook(t) => {
                let t = t.expand(parent_dir)?;
                Ok(Self::Scrapbook(t))
            }
            Self::CodeExecution(t) => Ok(Self::CodeExecution(t.expand(parent_dir)?)),
            Self::CurrentDate(t) => Ok(Self::CurrentDate(t.expand(parent_dir)?)),
        }
    }

    fn check(self) -> Result<Self, FormaError> {
        match self {
            // A workflow tool carries a whole workflow, whose nodes and tools
            // are checked like any other. Every other runner is a leaf: what it
            // needs is checked when it is compiled, or when it is called.
            Self::Workflow(t) => Ok(Self::Workflow(t.check()?)),
            other => Ok(other),
        }
    }
}

/// Tools are instruments that allow AI Agents do things.
/// They way they operate is that we send an LLM a request,
/// and in that request we include the possible actions we
/// have available for taking. The LLM will *not* execute any
/// action (Forma does it afterwards) but it can decide *which*
/// tool and its arguments.
///
/// The purpose of this Tool object is to describe the possible
/// actions to the LLM, so it can decide.
#[derive(Default, Clone, Deserialize, Serialize, Debug, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    /// The name of the tool. Should be descriptive to the LLM
    /// so it can discriminate between tools.
    ///
    /// (Misleading names just cause confusion)
    pub name: String,
    /// A more detailed description of the tool, for the LLM to
    /// discriminate between options.
    ///
    /// Note that, in the case of Scrapbooks, Workspaces and Knowledgebases,
    /// this field should describe the contents of the collection (e.g., "documents
    /// with information about algebra" or "notes on the progress of the student").
    pub description: Option<String>,

    /// The Tool that will be executed if the this tool
    /// is selected.
    pub tool: ToolRunnerDefinition,

    /// The roles that are allowed to use this tool.
    /// If empty, everyone is allowed to use it; if not,
    /// only the specific roles in here are allowed to use it.
    #[serde(default)]
    pub just_for: Vec<String>,

    /// Whether to ask the client to confirm, reject and eventually
    /// modify the inputs proposed by the Agent before calling the tool.
    ///
    /// If `true`, Forma will send the client a confirmation request before
    /// calling the tool, and the session will be suspended until the client
    /// has either rejected or confirmed the inputs.
    ///
    /// > Note: This has no effect with delegated_tools, as they are always
    /// > sent to the client for both confirmation and running it.
    #[serde(default)]
    pub confirm_before_calling: bool,

    /// An identifier used to group tools together.
    /// This has no impact on the tool's behavior, but
    /// the client will be informed about the bundle of
    /// the tool when sending a `tool_input_start` event.
    /// This can help show the client some icon or label
    /// to provide better guidance.
    ///
    /// When tools are derived from MCP Servers, this bundle is
    /// the name of the MCP Server.
    #[serde(default)]
    pub bundle: Option<String>,
}

impl ObjectDefinition for ToolDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        let tool = self.tool.expand(parent_dir)?;

        Ok(Self {
            name: self.name.clone(),
            description: self.description,
            tool,
            just_for: self.just_for.clone(),
            confirm_before_calling: self.confirm_before_calling,
            bundle: self.bundle.clone(),
        })
    }

    fn check(self) -> Result<Self, FormaError> {
        // The same rule `Tool::check` applies at runtime, over the names this
        // definition will actually produce rather than the one it was written
        // with — they are only the same for the tools that compile one-to-one.
        for name in self.tool_names().unwrap_or_default() {
            is_valid_variable_name(&name)?;
        }
        let Self {
            name,
            description,
            tool,
            just_for,
            confirm_before_calling,
            bundle,
        } = self;
        Ok(Self {
            name,
            description,
            tool: tool.check()?,
            just_for,
            confirm_before_calling,
            bundle,
        })
    }
}

impl ToolDefinition {
    /// The names the tools compiled from this definition will answer to, or
    /// `None` when the file cannot say.
    ///
    /// Most definitions become a single tool called exactly what the author
    /// named it. A docstore becomes one tool per operation, each name derived
    /// from that one. An MCP server becomes whatever tools it reports when
    /// connected to — the name written next to it is not used for anything —
    /// so its tools cannot be known, or checked, until the agent is compiled.
    pub(crate) fn tool_names(&self) -> Option<Vec<String>> {
        let docstore_names = |ops: &[KnowledgeBaseOperation]| {
            ops.iter()
                .map(|op| docstore_tool_name(*op, &self.name))
                .collect()
        };
        let names = match &self.tool {
            ToolRunnerDefinition::Mcp(_) => return None,
            ToolRunnerDefinition::GoogleKnowledgebase(_) => docstore_names(KNOWLEDGEBASE_OPS),
            ToolRunnerDefinition::GoogleWorkspace(_) => docstore_names(WORKSPACE_OPS),
            ToolRunnerDefinition::Scrapbook(_) => docstore_names(SCRAPBOOK_OPS),
            ToolRunnerDefinition::CodeExecution(_) => code_execution_tool_names(&self.name),
            ToolRunnerDefinition::Template(_)
            | ToolRunnerDefinition::Workflow(_)
            | ToolRunnerDefinition::ApiCall(_)
            | ToolRunnerDefinition::Delegated(_)
            | ToolRunnerDefinition::Mailgun(_)
            | ToolRunnerDefinition::TavilySearch(_)
            | ToolRunnerDefinition::GoogleAgentSearch(_)
            | ToolRunnerDefinition::GoogleGenImage(_)
            | ToolRunnerDefinition::Fetch(_)
            | ToolRunnerDefinition::CurrentDate(_) => vec![self.name.clone()],
        };
        Some(names)
    }
}

impl ToolDefinition {
    pub(crate) async fn into_tools(
        self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> Result<Vec<Tool>, FormaError> {
        let name = self.name.clone();
        let description = self.description.clone();
        let tool = match self.tool {
            ToolRunnerDefinition::Template(v) => {
                let t = v.try_into()?;
                ToolRunner::Template(t)
            }
            ToolRunnerDefinition::Workflow(v) => {
                let t = v.into_workflow(mcp, available_variables).await?;
                ToolRunner::Workflow(t)
            }
            ToolRunnerDefinition::ApiCall(v) => {
                let t = v.try_into()?;
                ToolRunner::ApiCall(t)
            }
            ToolRunnerDefinition::Delegated(v) => {
                let t = v.try_into()?;
                ToolRunner::Delegated(t)
            }
            ToolRunnerDefinition::Mcp(v) => {
                return v
                    .into_tools(self.confirm_before_calling, &self.just_for, mcp)
                    .await;
            }
            ToolRunnerDefinition::Mailgun(v) => {
                let t = v.try_into()?;
                ToolRunner::Mailgun(t)
            }
            ToolRunnerDefinition::TavilySearch(v) => {
                let t = v.try_into()?;
                ToolRunner::TavilyWebSearch(t)
            }
            ToolRunnerDefinition::GoogleAgentSearch(v) => {
                let t = v.try_into()?;
                ToolRunner::GoogleAgentSearch(t)
            }
            ToolRunnerDefinition::GoogleGenImage(v) => {
                let t = v.try_into()?;
                ToolRunner::GoogleGenImage(t)
            }
            ToolRunnerDefinition::Fetch(v) => {
                let t = v.try_into()?;
                ToolRunner::Fetch(t)
            }
            ToolRunnerDefinition::CurrentDate(v) => {
                let t = v.try_into()?;
                ToolRunner::CurrentDate(t)
            }
            ToolRunnerDefinition::GoogleKnowledgebase(v) => {
                return Ok(knowledgebase_tools(
                    v.clone().into(),
                    &name,
                    description.as_deref(),
                    v.usage_guidelines.as_deref(),
                    self.confirm_before_calling,
                    &self.just_for,
                    self.bundle,
                ));
            }
            ToolRunnerDefinition::GoogleWorkspace(v) => {
                return Ok(workspace_tools(
                    v.clone().into(),
                    &name,
                    description.as_deref(),
                    v.usage_guidelines.as_deref(),
                    self.confirm_before_calling,
                    &self.just_for,
                    self.bundle,
                ));
            }
            ToolRunnerDefinition::CodeExecution(v) => {
                return Ok(code_execution_tools(
                    v,
                    &name,
                    description.as_deref(),
                    self.confirm_before_calling,
                    &self.just_for,
                    self.bundle,
                ));
            }
            ToolRunnerDefinition::Scrapbook(v) => {
                return Ok(scrapbook_tools(
                    v.clone().into(),
                    &name,
                    description.as_deref(),
                    v.usage_guidelines.as_deref(),
                    self.confirm_before_calling,
                    &self.just_for,
                    self.bundle,
                ));
            }
        };

        let v = vec![Tool {
            name,
            description,
            tool,
            just_for: self.just_for,
            confirm_before_calling: self.confirm_before_calling,
            bundle: self.bundle.clone(),
        }];
        Ok(v)
    }
}
