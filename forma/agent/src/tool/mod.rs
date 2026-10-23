use async_trait::async_trait;
use protocols::{FormaDB, FormaError, envar::get_env_var};

pub mod code_execution;
pub mod current_date;
pub mod delegated_tool;
pub mod fetch_tool;
pub mod google_agent_search_tool;
pub(crate) mod google_auth;
pub mod google_gen_image_tool;
pub mod host;
pub(crate) mod http;
pub mod mailgun_tool;
pub mod tavily_search_tool;
pub mod workspace;
pub use delegated_tool::DelegatedTool;
pub mod api_call_tool;
pub use api_call_tool::ApiCallTool;
pub mod template_tool;
use protocols::CachedToolResult;
use serde::{Deserialize, Serialize};
pub use template_tool::TemplateTool;
pub mod mcp_tool;
pub mod workflow_tool;
pub use mcp_tool::McpTool;
use serde_json::Value;

use crate::{
    Workflow,
    callable::CallContext,
    template::is_valid_variable_name,
    tool::{
        code_execution::CodeExecutionTool,
        current_date::CurrentDateTool,
        fetch_tool::FetchTool,
        google_agent_search_tool::GoogleAgentSearchTool,
        google_gen_image_tool::GoogleGenImageTool,
        mailgun_tool::MailgunTool,
        tavily_search_tool::TavilySearchTool,
        workspace::{KnowledgebaseTool, WorkspaceScope},
    },
};
use std::fmt::Debug;

use genai_client::{
    JsonSchema,
    openai::v1::{FunctionToolFields, OpenAITool},
};

/// Names the Google Cloud project a tool falls back to when it does not carry
/// one of its own.
pub const GOOGLE_CLOUD_PROJECT: &str = "GOOGLE_CLOUD_PROJECT";

/// Resolves the Google Cloud project a call is billed to.
///
/// The tool's own `project_id` wins when it has one. Otherwise the environment
/// is read *here*, at call time, rather than when the agent was loaded: an
/// agent file is meant to be portable between deployments, so which project
/// pays for a call is a property of the machine running it and not of the file
/// describing it. Reading it any earlier bakes one deployment's project into
/// the agent — and makes merely *loading* an agent fail on a machine that has
/// no project set, even when nothing ever calls the tool.
pub fn resolve_project_id(tool: &str, configured: Option<&str>) -> Result<String, FormaError> {
    if let Some(project_id) = configured.map(str::trim).filter(|p| !p.is_empty()) {
        return Ok(project_id.to_string());
    }

    match get_env_var(GOOGLE_CLOUD_PROJECT)?.filter(|p| !p.trim().is_empty()) {
        Some(project_id) => Ok(project_id.trim().to_string()),
        // Both routes are named because either one is a legitimate way to
        // configure this, and the author has to know which one they are missing.
        None => Err(FormaError::Input(format!(
            "{tool}: no Google Cloud project — set `project_id` on the tool, or the \
             {GOOGLE_CLOUD_PROJECT} environment variable"
        ))),
    }
}

/// Sets `GOOGLE_CLOUD_PROJECT` for as long as it is alive, then puts back
/// whatever was there before.
///
/// The environment is process-wide while the test harness runs a thread per
/// test, so a test that writes to it has to exclude the others that read it —
/// hence the lock, which every user of this variable in this crate's tests goes
/// through. Poisoning is ignored: a panicking test says nothing about whether
/// the variable is safe to touch.
#[cfg(test)]
pub(crate) struct ProjectEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<String>,
}

#[cfg(test)]
impl ProjectEnvGuard {
    /// Sets the variable to `value`, or unsets it when given `None`.
    pub(crate) fn set(value: Option<&str>) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var(GOOGLE_CLOUD_PROJECT).ok();
        set_or_unset(value);
        Self {
            _lock: lock,
            previous,
        }
    }
}

#[cfg(test)]
impl Drop for ProjectEnvGuard {
    fn drop(&mut self) {
        set_or_unset(self.previous.as_deref());
    }
}

#[cfg(test)]
pub(crate) fn set_or_unset(value: Option<&str>) {
    // SAFETY: every writer of this variable in this crate's tests holds the
    // lock inside `ProjectEnvGuard`, so no other thread is reading it here.
    match value {
        Some(v) => unsafe { std::env::set_var(GOOGLE_CLOUD_PROJECT, v) },
        None => unsafe { std::env::remove_var(GOOGLE_CLOUD_PROJECT) },
    }
}

#[async_trait]
pub trait ToolTrait<DB: FormaDB>:
    TryInto<JsonSchema> + Clone + Debug + Send + Sync + 'static
{
    /// Checks that the tool is well formed.
    /// What we check will depend on the specific tool type.
    ///
    /// Should return an `FormaError::Input` if the element is not well formed.
    fn check(&self) -> Result<(), FormaError>;

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "kebab-case")]
pub enum ToolRunner {
    Template(TemplateTool),
    Workflow(Workflow),
    ApiCall(ApiCallTool),
    Delegated(DelegatedTool),
    Mcp(McpTool),
    Mailgun(MailgunTool),
    TavilyWebSearch(TavilySearchTool),
    GoogleAgentSearch(GoogleAgentSearchTool),
    GoogleGenImage(GoogleGenImageTool),
    Fetch(FetchTool),
    Knowledgebase(KnowledgebaseTool),
    CodeExecution(CodeExecutionTool),
    CurrentDate(CurrentDateTool),
}

impl ToolRunner {
    /// The runner's kind, as the string used in the `type` serde tag above.
    ///
    /// Billing keys on this rather than on `Tool::name`: the name is whatever
    /// the agent author called it, while the kind says which machinery runs and
    /// therefore what it costs — a `tavily-web-search` bills through to a paid
    /// API, a `template` is free. Kept in sync with the `rename_all =
    /// "kebab-case"` tag by hand, since serde offers no way to ask for a
    /// variant's tag without serialising the whole tool.
    pub fn kind(&self) -> &'static str {
        match self {
            ToolRunner::Template(_) => "template",
            ToolRunner::Workflow(_) => "workflow",
            ToolRunner::ApiCall(_) => "api-call",
            ToolRunner::Delegated(_) => "delegated",
            ToolRunner::Mcp(_) => "mcp",
            ToolRunner::Mailgun(_) => "mailgun",
            ToolRunner::TavilyWebSearch(_) => "tavily-web-search",
            ToolRunner::GoogleAgentSearch(_) => "google-agent-search",
            ToolRunner::GoogleGenImage(_) => "google-gen-image",
            ToolRunner::Fetch(_) => "fetch",
            ToolRunner::Knowledgebase(_) => "knowledgebase",
            ToolRunner::CodeExecution(_) => "code-execution",
            ToolRunner::CurrentDate(_) => "current-date",
        }
    }
}

impl TryFrom<ToolRunner> for JsonSchema {
    type Error = FormaError;
    fn try_from(item: ToolRunner) -> Result<JsonSchema, Self::Error> {
        match item {
            ToolRunner::Workflow(tool) => tool.try_into(),
            ToolRunner::Template(tool) => tool.try_into(),
            ToolRunner::ApiCall(tool) => tool.try_into(),
            ToolRunner::Delegated(tool) => tool.try_into(),
            ToolRunner::Mcp(tool) => tool.try_into(),
            ToolRunner::Mailgun(tool) => tool.try_into(),
            ToolRunner::TavilyWebSearch(tool) => tool.try_into(),
            ToolRunner::GoogleAgentSearch(tool) => tool.try_into(),
            ToolRunner::GoogleGenImage(tool) => tool.try_into(),
            ToolRunner::Fetch(tool) => tool.try_into(),
            ToolRunner::Knowledgebase(tool) => tool.try_into(),
            ToolRunner::CodeExecution(tool) => tool.try_into(),
            ToolRunner::CurrentDate(tool) => tool.try_into(),
        }
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for ToolRunner {
    fn check(&self) -> Result<(), FormaError> {
        match self {
            ToolRunner::Workflow(w) => <Workflow as ToolTrait<DB>>::check(w),
            ToolRunner::Template(w) => <TemplateTool as ToolTrait<DB>>::check(w),
            ToolRunner::ApiCall(w) => <ApiCallTool as ToolTrait<DB>>::check(w),
            ToolRunner::Delegated(w) => <DelegatedTool as ToolTrait<DB>>::check(w),
            ToolRunner::Mcp(w) => <McpTool as ToolTrait<DB>>::check(w),
            ToolRunner::Mailgun(w) => <MailgunTool as ToolTrait<DB>>::check(w),
            ToolRunner::TavilyWebSearch(w) => <TavilySearchTool as ToolTrait<DB>>::check(w),
            ToolRunner::GoogleAgentSearch(w) => <GoogleAgentSearchTool as ToolTrait<DB>>::check(w),
            ToolRunner::GoogleGenImage(w) => <GoogleGenImageTool as ToolTrait<DB>>::check(w),
            ToolRunner::Fetch(w) => <FetchTool as ToolTrait<DB>>::check(w),
            ToolRunner::Knowledgebase(w) => <KnowledgebaseTool as ToolTrait<DB>>::check(w),
            ToolRunner::CodeExecution(w) => <CodeExecutionTool as ToolTrait<DB>>::check(w),
            ToolRunner::CurrentDate(w) => <CurrentDateTool as ToolTrait<DB>>::check(w),
        }
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        match self {
            ToolRunner::Workflow(tool) => tool.execute(input, ctx).await,
            ToolRunner::Template(tool) => tool.execute(input, ctx).await,
            ToolRunner::ApiCall(tool) => tool.execute(input, ctx).await,
            ToolRunner::Delegated(tool) => tool.execute(input, ctx).await,
            ToolRunner::Mcp(tool) => tool.execute(input, ctx).await,
            ToolRunner::Mailgun(tool) => tool.execute(input, ctx).await,
            ToolRunner::TavilyWebSearch(tool) => tool.execute(input, ctx).await,
            ToolRunner::GoogleAgentSearch(tool) => tool.execute(input, ctx).await,
            ToolRunner::GoogleGenImage(tool) => tool.execute(input, ctx).await,
            ToolRunner::Fetch(tool) => tool.execute(input, ctx).await,
            ToolRunner::Knowledgebase(tool) => tool.execute(input, ctx).await,
            ToolRunner::CodeExecution(tool) => tool.execute(input, ctx).await,
            ToolRunner::CurrentDate(tool) => tool.execute(input, ctx).await,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// The name of the tool
    pub name: String,
    /// A description to give the LLM, to help it choose tools.
    pub description: Option<String>,
    /// The tool itself
    pub tool: ToolRunner,
    /// The roles that are allowed to use this tool.
    /// If empty, everyone is allowed to use it; if not,
    /// only the specific roles in here are allowed to use it.
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
    pub confirm_before_calling: bool,

    /// The name of the bundle this tool belongs to,
    /// if any.
    pub bundle: Option<String>,
}

impl Tool {
    /// What the tool is, for whoever shows it: its [`ToolRunner::kind`],
    /// except for store tools.
    ///
    /// Every store shares the one `knowledgebase` runner, so a store tool says
    /// whose store it reaches instead, in the names the definitions use: a
    /// `knowledgebase` (read), a `workspace` (written to), the caller's
    /// `scrapbook`, or — reading what a run left — `code-execution`.
    pub fn kind(&self) -> &'static str {
        match &self.tool {
            ToolRunner::Knowledgebase(kb) => match kb.scope {
                WorkspaceScope::Shared => match kb.required_grant() {
                    Some((_, "write")) => "workspace",
                    _ => "knowledgebase",
                },
                WorkspaceScope::Scrapbook => "scrapbook",
                WorkspaceScope::Runs => "code-execution",
            },
            runner => runner.kind(),
        }
    }

    /// Checks that it has a valid ID and that the runner is valid
    pub fn check<DB: FormaDB>(&self) -> Result<(), FormaError> {
        is_valid_variable_name(&self.name)?;
        <ToolRunner as ToolTrait<DB>>::check(&self.tool)?;
        Ok(())
    }

    /// Registers a result into the tool_calls, adding at as `Value`
    /// indexex by its `id`
    pub async fn register_result<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        tool_call_id: &str,
        result: &Value,
        from_root: bool,
    ) -> Result<(), FormaError> {
        let mut session = ctx.session.write().await;
        let id = &self.name;
        let result = CachedToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: id.to_string(),
            result: result.clone(),
            from_root,
        };
        session
            .tool_calls_results
            .insert(tool_call_id.to_string(), result);
        Ok(())
    }

    /// Retrieves the result from the tool_calls.
    pub async fn cached_result<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
        tool_call_id: &str,
    ) -> Option<Value> {
        let session = ctx.session.read().await;
        if let Some(result) = session.tool_calls_results.get(tool_call_id) {
            if result.tool_name == self.name {
                Some(result.result.clone())
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl TryFrom<Tool> for OpenAITool {
    type Error = FormaError;

    fn try_from(item: Tool) -> Result<Self, Self::Error> {
        let ret = Self::Function {
            function: FunctionToolFields {
                name: item.name,
                description: item.description,
                parameters: Some(item.tool.try_into()?),
                strict: None,
            },
        };
        Ok(ret)
    }
}

impl Tool {
    /// Checks whether the given roles are allowed to use this tool
    pub fn is_allowed(&self, roles: &[String]) -> bool {
        // Not restricted
        if self.just_for.is_empty() {
            return true;
        }
        // Check the given roles
        for role in roles {
            // if any role is allowed, then yes.
            if self.just_for.contains(role) {
                return true;
            }
        }
        false
    }

    /// Exectutes the tool runner code
    pub async fn run<DB: FormaDB>(
        &self,
        input: serde_json::Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        self.tool.execute(input, ctx).await
    }
}

#[cfg(test)]
mod tests {

    use std::{collections::HashMap, sync::Arc};

    use db::NoneDB;
    use genai_client::SSEvent;
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use tokio::sync::{Mutex, RwLock, mpsc};

    use crate::callable::CallContext;

    use super::*;

    // ── project resolution ────────────────────────────────────────────────

    #[test]
    fn a_stated_project_is_used_and_the_environment_is_not_read() {
        // Set to something recognisably wrong: if it ever leaks into the
        // result, the assertion says so rather than passing by coincidence.
        let _env = ProjectEnvGuard::set(Some("from-the-environment"));
        assert_eq!(
            resolve_project_id("SomeTool", Some("from-the-agent")).unwrap(),
            "from-the-agent"
        );
    }

    #[test]
    fn an_unstated_project_falls_back_to_the_environment() {
        let _env = ProjectEnvGuard::set(Some("from-the-environment"));
        assert_eq!(
            resolve_project_id("SomeTool", None).unwrap(),
            "from-the-environment"
        );
    }

    /// The environment is read on every call, not once — the same tool value
    /// has to work on whichever machine ends up running it.
    #[test]
    fn the_environment_is_read_afresh_each_time() {
        let _env = ProjectEnvGuard::set(Some("first"));
        assert_eq!(resolve_project_id("SomeTool", None).unwrap(), "first");

        set_or_unset(Some("second"));
        assert_eq!(resolve_project_id("SomeTool", None).unwrap(), "second");
    }

    #[test]
    fn neither_a_stated_nor_an_environment_project_is_an_error() {
        let _env = ProjectEnvGuard::set(None);
        let err = resolve_project_id("SomeTool", None).unwrap_err();
        let FormaError::Input(msg) = err else {
            panic!("expected an Input error, got {err:?}");
        };
        // Both routes are named, so the reader knows which one they are missing.
        assert!(msg.contains("SomeTool"), "{msg}");
        assert!(msg.contains("project_id"), "{msg}");
        assert!(msg.contains(GOOGLE_CLOUD_PROJECT), "{msg}");
    }

    /// An env var set to whitespace is as good as unset, and must not produce
    /// a URL with an empty project segment in it.
    #[test]
    fn a_blank_project_counts_as_no_project() {
        let _env = ProjectEnvGuard::set(Some("   "));
        assert!(resolve_project_id("SomeTool", None).is_err());
        assert!(resolve_project_id("SomeTool", Some("  ")).is_err());
    }

    #[test]
    fn a_stated_project_is_trimmed() {
        let _env = ProjectEnvGuard::set(Some(" from-the-environment \n"));
        assert_eq!(
            resolve_project_id("SomeTool", Some(" from-the-agent ")).unwrap(),
            "from-the-agent"
        );
        assert_eq!(
            resolve_project_id("SomeTool", None).unwrap(),
            "from-the-environment"
        );
    }

    #[tokio::test]
    async fn test_resolved_tool() {
        use genai_client::SSEvent;
        use std::{collections::HashMap, sync::Arc};
        use tokio::sync::{RwLock, mpsc};

        // Create a tool
        let tool_call_id = "call123123".to_string();
        let tool = Tool {
            name: "test_tool".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["some template".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };

        // Create a session with the tool's result already in tool_calls
        let expected_result = serde_json::json!({"output": "cached_tool"});
        let r = CachedToolResult {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool.name.clone(),
            result: expected_result.clone(),
            from_root: true,
        };
        let mut tool_calls_results = HashMap::new();
        tool_calls_results.insert(tool_call_id.clone(), r);
        let session = SessionData {
            tool_calls_results,
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(session));

        // Create a context
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "test_job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        // Check the result
        let result = tool.cached_result(&ctx, &tool_call_id).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap(), expected_result);
    }

    #[tokio::test]
    async fn test_cached_result_tool_name_mismatch() {
        let tool_call_id = "call123123".to_string();
        let tool = Tool {
            name: "test_tool".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["some template".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };

        // Store a result keyed by the correct tool_call_id, but with a different tool name
        let r = CachedToolResult {
            tool_call_id: tool_call_id.clone(),
            tool_name: "other_tool".to_string(),
            result: serde_json::json!({"output": "cached_tool"}),
            from_root: false,
        };
        let mut tool_calls_results = HashMap::new();
        tool_calls_results.insert(tool_call_id.clone(), r);
        let session = SessionData {
            tool_calls_results,
            ..Default::default()
        };
        let session = Arc::new(RwLock::new(session));

        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: session.clone(),
            tx,
            job_id: "test_job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let result = tool.cached_result(&ctx, &tool_call_id).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cached_result_missing_tool_call_id() {
        let tool = Tool {
            name: "test_tool".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["some template".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };

        let session = Arc::new(RwLock::new(SessionData::default()));
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session,
            tx,
            job_id: "test_job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let result = tool.cached_result(&ctx, "nonexistent_call_id").await;
        assert!(result.is_none());
    }

    // ── Tool::check / ToolRunner::check ───────────────────────────────────────

    fn template_tool(template: &str) -> Tool {
        Tool {
            name: "valid_name".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec![template.to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        }
    }

    fn workflow_tool(input_prompt: Option<&str>) -> Tool {
        Tool {
            name: "valid_name".to_string(),
            description: None,
            tool: ToolRunner::Workflow(Workflow {
                id: "workflow_1".to_string(),
                input_prompt: input_prompt.map(|s| s.to_string()),
                ..Default::default()
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        }
    }

    #[test]
    fn test_check_valid_template_tool() {
        let tool = template_tool("Hello {{name: string \"the name\"}}");
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_template_tool_no_variables() {
        // A fixed-string template with no placeholders is valid
        let tool = template_tool("Hello world");
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_template_tool_invalid_type() {
        // "array" is not a supported field type
        let tool = template_tool("Hello {{name: array \"desc\"}}");
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_valid_workflow_tool_no_prompt() {
        let tool = workflow_tool(None);
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_valid_workflow_tool_with_prompt() {
        let tool = workflow_tool(Some("Do {{task: string \"the task\"}}"));
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_workflow_tool_invalid_prompt_type() {
        let tool = workflow_tool(Some("Do {{task: object \"desc\"}}"));
        let check = <ToolRunner as ToolTrait<NoneDB>>::check(&tool.tool);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_name_starts_with_digit_is_err() {
        let mut tool = template_tool("ok");
        tool.name = "1invalid".to_string();
        let check = tool.check::<NoneDB>();
        assert!(check.is_err());
    }

    #[test]
    fn test_check_name_with_space_is_err() {
        let mut tool = template_tool("ok");
        tool.name = "my tool".to_string();
        let check = tool.check::<NoneDB>();
        assert!(check.is_err());
    }

    #[test]
    fn test_check_name_empty_is_err() {
        let mut tool = template_tool("ok");
        tool.name = String::new();
        let check = tool.check::<NoneDB>();
        assert!(check.is_err());
    }

    #[test]
    fn test_check_name_with_underscore_prefix_is_ok() {
        let mut tool = template_tool("ok");
        tool.name = "_private_tool".to_string();
        let check = tool.check::<NoneDB>();
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_name_with_hyphen_is_ok() {
        let mut tool = template_tool("ok");
        tool.name = "my-tool".to_string();
        let check = tool.check::<NoneDB>();
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_name_error_reported_before_runner_error() {
        // Invalid name AND invalid template: name error should come first
        let tool = Tool {
            name: "1bad".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["{{x: array}}".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };
        let err = tool.check::<NoneDB>().unwrap_err().to_string();
        assert!(err.contains("1bad"));
    }
}
