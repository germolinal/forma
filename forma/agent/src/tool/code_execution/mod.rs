//! Running code in a sandbox.
//!
//! One call is one run: the runtime stages the files the agent asked for, has
//! the stores holding them sign the URLs the run may use, starts a fresh
//! container, waits for it, and answers with the outcome. The container is gone
//! by then; everything it left is in the run's folder,
//! `runs/{owner}/{user}/{run_id}/` in the workspaces store, which the agent
//! reads through the `read-from-*` tool of
//! [`WorkspaceScope::Runs`].
//!
//! Two things are pluggable, independently:
//!
//! - **Where files live** is a [`DocStore`]: every input, output and log is
//!   moved through the trait, and each store signs its own URLs
//!   ([`DocStore::signed_url`], [`DocStore::signed_upload`]). A workspace in one
//!   store can feed a run whose folder is in another.
//! - **Where code runs** is the server's one [`sandbox::Sandbox`], chosen by
//!   whoever runs it (`code_execution` in its `runtime.yaml`, see
//!   [`crate::tool::host`]) and the same for every agent on it. Agents do not
//!   choose.
//!
//! What keeps this safe:
//!
//! - The container's service account has no roles. All it can touch is what
//!   the run's manifest grants: a signed GET per input, a signed PUT per log
//!   (size-limited), and a POST policy for its own `out/` folder.
//! - The container has no internet (infra/code-execution.tf).
//! - Inside it, the command runs as an unprivileged user that never sees the
//!   manifest (sandbox/entrypoint.py).
//! - Run folders are derived from the caller, like scrapbooks, so nobody can
//!   name another user's runs.
//! - Files come from, and go to, workspaces only through the agent's own
//!   workspace tools, with the same grants and role restrictions.
//! - Nothing the command prints reaches the platform's logs; failures are
//!   logged as metadata only.

pub mod sandbox;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use genai_client::{JsonSchema, SSEvent, schema::SchemaType};
use protocols::{DocStore, FormaDB, FormaError, SignedAccess, envar::expect_env_var};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use self::sandbox::SandboxExit;
use crate::tool::workspace::{
    DocStoreBackend, KnowledgeBaseOperation, KnowledgebaseTool, WorkspaceScope, changes,
    gcs::GcsDocConfig, runs_prefix,
};
use crate::{
    CallContext, Tool, ToolRunner, ToolTrait,
    tool::{host, mailgun_tool::string_schema},
};

/// How long the command itself may run, enforced inside the container.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the container may run, enforced by the platform. Longer than the
/// command's, so a command that times out still leaves its logs behind.
const TASK_TIMEOUT: Duration = Duration::from_secs(360);
/// How long the runtime waits for a run, startup included, before giving up
/// and cancelling it.
const RUN_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// How long the signed URLs live: past the deadline, so a run that started
/// late can still upload, and not much longer.
const URL_EXPIRY: Duration = Duration::from_secs(20 * 60);

const MAX_COMMAND_CHARS: usize = 20_000;
const MAX_PATH_BYTES: usize = 400;
const MAX_INPUT_FILES: usize = 50;
const MAX_INPUT_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_INPUT_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
const MAX_OUTPUT_FILES: usize = 50;
const MAX_OUTPUT_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_OUTPUT_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_RESULT_BYTES: u64 = 256 * 1024;
const MAX_PERSIST: usize = 20;
/// How much of stdout/stderr comes back in the tool result.
const EXCERPT_CHARS: usize = 2_000;
const EXCERPT_BYTES: usize = 8 * 1024;

/// What the sandbox image offers; keep in sync with sandbox/requirements.txt.
const SANDBOX_CONTENTS: &str = "bash, coreutils, grep, sed, gawk and Python 3.12 with numpy, \
     pandas, scipy, statsmodels, matplotlib, openpyxl (so Excel .xlsx files can be read, \
     edited and written with pandas or openpyxl) and python-pptx (so PowerPoint .pptx \
     decks can be read, edited and written)";

/// The tool that starts a run. Reading what a run left is done by ordinary
/// [`KnowledgebaseTool`]s over [`WorkspaceScope::Runs`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodeExecutionTool {
    /// The workspaces this agent can also reach through its own tools, by the
    /// name the agent knows them by. Filled in by [`link_workspaces`] once all
    /// of a node's tools exist; a run can take files from these and persist
    /// outputs into them, and from nowhere else.
    #[serde(default)]
    pub workspaces: BTreeMap<String, LinkedWorkspace>,
}

/// How a run reaches one workspace: through the agent's own tools for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinkedWorkspace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<LinkedAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<LinkedAccess>,
}

/// One of the agent's workspace tools, with the restriction it came with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedAccess {
    pub tool: KnowledgebaseTool,
    /// The roles allowed to use the original tool; a run is held to the same.
    #[serde(default)]
    pub just_for: Vec<String>,
}

impl LinkedAccess {
    /// The workspace's store, if this caller may use it: the role limit of the
    /// original tool, then its own grant check.
    fn resolve<DB: FormaDB>(
        &self,
        name: &str,
        ctx: &CallContext<DB>,
    ) -> Result<DocStoreBackend, FormaError> {
        let allowed =
            self.just_for.is_empty() || ctx.user_roles.iter().any(|r| self.just_for.contains(r));
        if !allowed {
            return Err(FormaError::Input(format!(
                "you cannot use the workspace '{name}' for this user"
            )));
        }
        self.tool.resolve_backend(ctx)
    }
}

/// Lets every code-execution tool in `tools` reach the workspaces the same
/// tools can, and tells the model which those are.
///
/// Called once a node's tools are compiled. A workspace whose tool asks for
/// confirmation before it is called is left out: a run would otherwise read or
/// write it without the confirmation its author asked for.
pub fn link_workspaces(tools: &mut [Tool]) {
    let mut linked: BTreeMap<String, LinkedWorkspace> = BTreeMap::new();
    for tool in tools.iter() {
        let ToolRunner::Knowledgebase(kb) = &tool.tool else {
            continue;
        };
        if kb.scope == WorkspaceScope::Runs || tool.confirm_before_calling {
            continue;
        }
        let Some(name) = tool.name.strip_prefix(&format!("{}-", kb.op.prefix())) else {
            continue;
        };
        let access = LinkedAccess {
            tool: kb.clone(),
            just_for: tool.just_for.clone(),
        };
        let entry = linked.entry(name.to_string()).or_default();
        match kb.op {
            KnowledgeBaseOperation::Read => entry.read = Some(access),
            KnowledgeBaseOperation::Write => entry.write = Some(access),
            KnowledgeBaseOperation::List
            | KnowledgeBaseOperation::Append
            | KnowledgeBaseOperation::Edit
            | KnowledgeBaseOperation::Move
            | KnowledgeBaseOperation::Delete => {}
        }
    }
    linked.retain(|_, w| w.read.is_some() || w.write.is_some());

    let readable: Vec<&str> = linked
        .iter()
        .filter(|(_, w)| w.read.is_some())
        .map(|(n, _)| n.as_str())
        .collect();
    let writable: Vec<&str> = linked
        .iter()
        .filter(|(_, w)| w.write.is_some())
        .map(|(n, _)| n.as_str())
        .collect();
    let mut note = String::new();
    if !readable.is_empty() {
        note.push_str(&format!(
            " Workspaces you can take files from (`from_workspace`): {}.",
            readable.join(", ")
        ));
    }
    if !writable.is_empty() {
        note.push_str(&format!(
            " Workspaces you can persist outputs into (`to_workspace`): {}.",
            writable.join(", ")
        ));
    }

    for tool in tools.iter_mut() {
        if let ToolRunner::CodeExecution(t) = &mut tool.tool {
            t.workspaces = linked.clone();
            if !note.is_empty()
                && let Some(description) = &mut tool.description
            {
                description.push_str(&note);
            }
        }
    }
}

/// The run tool's description. `reader` is the name of the tool that reads
/// what runs leave behind.
pub fn describe_run(reader: &str) -> String {
    format!(
        "Run a shell command in a fresh, isolated Linux sandbox, and get its result when it \
         finishes. Use it for exact, deterministic work: counting, find-and-replace and other \
         text processing (grep/sed/awk), or Python scripts (calculations, statistics, \
         simulations, data analysis, charts). The sandbox has {SANDBOX_CONTENTS}. It has NO \
         internet access and nothing else can be installed. The command runs in /work, where \
         the `files` you pass are placed — nothing else is there: a file from a workspace \
         (including a ready-made script) is only available if you pass it with \
         `from_workspace`. Write everything you want back into /work/out/ (also $OUT). \
         Limits: {} minutes per run; up to {MAX_INPUT_FILES} input files of {} MB each ({} MB \
         in total); up to {MAX_OUTPUT_FILES} output files ({} MB in total). Every run starts \
         empty. A run's outputs can be passed to a later run (`from_run`), or read with \
         `{reader}`, for about a day; to keep one, list it in `persist` to copy \
         it into a workspace (done only if the run succeeds). The result's `status` is `ok`; \
         `failed` (the command exited with an error: read `stderr` and fix it); `timeout`; or \
         `error` (the sandbox itself failed: tell the user something went wrong). Check \
         `status`, `exit_code` and `stderr` before trusting the outputs. The result only \
         carries the last {EXCERPT_CHARS} characters of stdout and stderr: do not print \
         anything longer (a table, a whole spreadsheet, a report). Write it to a file in \
         /work/out/ and read that file with `{reader}`, which reads long files in parts; \
         do not run the code again just to see more of what it printed (the full stdout is \
         also in `<run_id>/stdout.txt`).",
        COMMAND_TIMEOUT.as_secs() / 60,
        MAX_INPUT_FILE_BYTES / (1024 * 1024),
        MAX_INPUT_TOTAL_BYTES / (1024 * 1024),
        MAX_OUTPUT_TOTAL_BYTES / (1024 * 1024),
    )
}

// ---- input ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RunInput {
    command: String,
    #[serde(default)]
    files: Vec<FileInput>,
    #[serde(default)]
    persist: Vec<PersistInput>,
}

#[derive(Debug, Deserialize)]
struct FileInput {
    path: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    from_workspace: Option<String>,
    #[serde(default)]
    from_run: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PersistInput {
    from: String,
    to_workspace: String,
    path: String,
}

/// Checks a path the agent gave, relative to the sandbox's work directory or
/// to a store. Mirrors `validate_rel_path` in sandbox/entrypoint.py.
///
/// Refuses rather than normalises: `..`, `.`, empty segments, absolute paths,
/// backslashes and control characters. Object names are literal in Cloud
/// Storage, so none of these could escape a prefix there, but in the sandbox
/// they would be resolved against a real file system.
fn validate_rel_path(path: &str) -> Result<(), FormaError> {
    let bad = |why: &str| Err(FormaError::Input(format!("'{path}': {why}")));
    if path.is_empty() {
        return bad("a path cannot be empty");
    }
    if path.len() > MAX_PATH_BYTES {
        return bad("the path is too long");
    }
    if path.starts_with('/') {
        return bad("paths must be relative (e.g. `data/ventas.csv`)");
    }
    if path.chars().any(|c| c.is_control() || c == '\\') {
        return bad("the path contains characters that are not allowed");
    }
    if path
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return bad("the path must be a plain relative path, without `.` or `..`");
    }
    Ok(())
}

/// A run ID as this module makes them: 32 lower-case hex characters.
fn validate_run_id(id: &str) -> Result<(), FormaError> {
    if id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(FormaError::Input(format!("'{id}' is not a run id")))
    }
}

/// One input, ready: where the sandbox puts it, and the signed URL it is
/// downloaded from.
struct StagedInput {
    path: String,
    url: String,
}

/// One output to copy into a workspace after the run.
struct PersistPlan {
    from: String,
    workspace: String,
    path: String,
    store: DocStoreBackend,
}

// ---- the result the sandbox leaves ------------------------------------------------

/// `result.json`, as written by the entrypoint. Read defensively: every field
/// has a default, and the status is checked against the ones that exist.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SandboxResult {
    status: String,
    error: Option<String>,
    exit_code: Option<i64>,
    timed_out: bool,
    duration_ms: Option<u64>,
    stdout_bytes: u64,
    stdout_truncated: bool,
    stderr_bytes: u64,
    stderr_truncated: bool,
    outputs: Vec<OutputMeta>,
    skipped_outputs: Vec<Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct OutputMeta {
    path: String,
    size: u64,
    media_type: String,
}

// ---- schema -----------------------------------------------------------------------

fn object_schema(props: HashMap<String, JsonSchema>, required: Vec<&str>) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Object),
        properties: Some(props),
        required: Some(required.into_iter().map(str::to_string).collect()),
        additional_properties: false,
        ..Default::default()
    }
}

fn array_schema(items: JsonSchema, description: &str) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Array),
        items: Some(Box::new(items)),
        description: Some(description.to_string()),
        ..Default::default()
    }
}

impl TryFrom<CodeExecutionTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_: CodeExecutionTool) -> Result<JsonSchema, FormaError> {
        let mut file = HashMap::new();
        file.insert(
            "path".to_string(),
            string_schema(
                "Where the file goes, relative to /work (e.g. `analysis.py`, `data/ventas.csv`).",
            ),
        );
        file.insert(
            "content".to_string(),
            string_schema("The file's full text, when you write it yourself (e.g. a script). Give exactly one of `content`, `from_workspace` or `from_run`."),
        );
        file.insert(
            "from_workspace".to_string(),
            string_schema("The workspace to copy the file from; `source` is its path there."),
        );
        file.insert(
            "from_run".to_string(),
            string_schema("The `run_id` of an earlier run to copy the file from; `source` is its path in that run (e.g. `out/result.csv`)."),
        );
        file.insert(
            "source".to_string(),
            string_schema("The file's path in `from_workspace` or `from_run`."),
        );

        let mut persist = HashMap::new();
        persist.insert(
            "from".to_string(),
            string_schema(
                "The output to keep, as the sandbox sees it under /work (e.g. `out/ledger.json`).",
            ),
        );
        persist.insert(
            "to_workspace".to_string(),
            string_schema("The workspace to copy it into."),
        );
        persist.insert(
            "path".to_string(),
            string_schema("Its path in that workspace. An existing file there is replaced."),
        );

        let mut props = HashMap::new();
        // Only for the person watching: the run ignores it, so a call
        // without it still works.
        props.insert(
            "purpose".to_string(),
            string_schema("What this run is for, in a few words and in the language you are speaking with the user (e.g. `Sumar las ventas por mes`). It is shown to them while the code runs, so say what it achieves, not how."),
        );
        props.insert(
            "command".to_string(),
            string_schema("The shell command to run in /work with bash (e.g. `python analysis.py` or `grep -c fresa notas.txt`)."),
        );
        props.insert(
            "files".to_string(),
            array_schema(
                object_schema(file, vec!["path"]),
                "The files to place in /work before running.",
            ),
        );
        props.insert(
            "persist".to_string(),
            array_schema(
                object_schema(persist, vec!["from", "to_workspace", "path"]),
                "Outputs to copy into a workspace if the run succeeds.",
            ),
        );
        Ok(object_schema(props, vec!["purpose", "command"]))
    }
}

// ---- running ------------------------------------------------------------------------

/// Lossy text of the end of a log, at most [`EXCERPT_CHARS`] characters.
fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(EXCERPT_CHARS);
    chars[start..].iter().collect()
}

impl CodeExecutionTool {
    fn workspace(&self, name: &str) -> Result<&LinkedWorkspace, FormaError> {
        self.workspaces.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.workspaces.keys().map(String::as_str).collect();
            FormaError::Input(format!(
                "there is no workspace called '{name}' here{}",
                if known.is_empty() {
                    String::new()
                } else {
                    format!("; you can use: {}", known.join(", "))
                }
            ))
        })
    }

    /// Checks every `persist` entry before anything runs, so a mistake there
    /// costs nothing.
    fn plan_persist<DB: FormaDB>(
        &self,
        persist: &[PersistInput],
        ctx: &CallContext<DB>,
    ) -> Result<Vec<PersistPlan>, FormaError> {
        if persist.len() > MAX_PERSIST {
            return Err(FormaError::Input(format!(
                "at most {MAX_PERSIST} outputs can be persisted per run"
            )));
        }
        let mut plans = Vec::with_capacity(persist.len());
        for p in persist {
            validate_rel_path(&p.from)?;
            if !p.from.starts_with("out/") {
                return Err(FormaError::Input(format!(
                    "'{}': only files under out/ can be persisted",
                    p.from
                )));
            }
            validate_rel_path(&p.path)?;
            changes::check_visible(&p.path)?;
            let access = self
                .workspace(&p.to_workspace)?
                .write
                .as_ref()
                .ok_or_else(|| {
                    FormaError::Input(format!(
                        "you cannot write to the workspace '{}'",
                        p.to_workspace
                    ))
                })?;
            plans.push(PersistPlan {
                from: p.from.clone(),
                workspace: p.to_workspace.clone(),
                path: p.path.clone(),
                store: access.resolve(&p.to_workspace, ctx)?,
            });
        }
        Ok(plans)
    }

    /// Locates every input — uploading the inline ones to the run's folder —
    /// and has the store holding it sign its download, enforcing the limits on
    /// count and size.
    async fn stage_inputs<DB: FormaDB>(
        &self,
        files: Vec<FileInput>,
        run_store: &dyn DocStore,
        runs_store: &dyn DocStore,
        ctx: &CallContext<DB>,
    ) -> Result<Vec<StagedInput>, FormaError> {
        if files.len() > MAX_INPUT_FILES {
            return Err(FormaError::Input(format!(
                "at most {MAX_INPUT_FILES} files can be passed to a run"
            )));
        }
        let mut seen = BTreeSet::new();
        for f in &files {
            validate_rel_path(&f.path)?;
            if !seen.insert(f.path.as_str()) {
                return Err(FormaError::Input(format!("'{}' is given twice", f.path)));
            }
            let sources = [
                f.content.is_some(),
                f.from_workspace.is_some(),
                f.from_run.is_some(),
            ];
            if sources.iter().filter(|s| **s).count() != 1 {
                return Err(FormaError::Input(format!(
                    "'{}': give exactly one of `content`, `from_workspace` or `from_run`",
                    f.path
                )));
            }
        }

        let mut total: u64 = 0;
        let mut staged = Vec::with_capacity(files.len());
        for f in files {
            let url = if let Some(content) = f.content {
                let size = content.len() as u64;
                check_input_size(&f.path, size)?;
                total += size;
                check_input_total(total)?;
                let path = format!("in/{}", f.path);
                run_store
                    .write_bytes(&path, content.into_bytes(), "text/plain; charset=utf-8")
                    .await?;
                run_store
                    .signed_url(&path, SignedAccess::Read, URL_EXPIRY)
                    .await?
                    .url
            } else {
                let source = f.source.as_deref().ok_or_else(|| {
                    FormaError::Input(format!("'{}': `source` is missing", f.path))
                })?;
                validate_rel_path(source)?;
                changes::check_visible(source)?;
                let workspace_store: Box<dyn DocStore>;
                let (store, path): (&dyn DocStore, String) = if let Some(name) = &f.from_workspace {
                    let access = self.workspace(name)?.read.as_ref().ok_or_else(|| {
                        FormaError::Input(format!("you cannot read from the workspace '{name}'"))
                    })?;
                    workspace_store = access.resolve(name, ctx)?.connect().await?;
                    (workspace_store.as_ref(), source.to_string())
                } else {
                    let run = f.from_run.as_deref().unwrap_or_default();
                    validate_run_id(run)?;
                    // Through the store of the caller's own runs, so
                    // another user's run cannot even be named.
                    (runs_store, format!("{run}/{source}"))
                };
                let size = store
                    .stat(&path)
                    .await?
                    .and_then(|m| m.size_bytes)
                    .ok_or_else(|| {
                        FormaError::Input(format!(
                            "there is no file at '{source}' to pass as '{}'",
                            f.path
                        ))
                    })?;
                check_input_size(&f.path, size)?;
                total += size;
                check_input_total(total)?;
                store
                    .signed_url(&path, SignedAccess::Read, URL_EXPIRY)
                    .await?
                    .url
            };
            staged.push(StagedInput { path: f.path, url });
        }
        Ok(staged)
    }
}

fn check_input_total(total: u64) -> Result<(), FormaError> {
    if total > MAX_INPUT_TOTAL_BYTES {
        return Err(FormaError::Input(format!(
            "the files passed to a run may add up to {} MB at most",
            MAX_INPUT_TOTAL_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

fn check_input_size(path: &str, size: u64) -> Result<(), FormaError> {
    if size > MAX_INPUT_FILE_BYTES {
        return Err(FormaError::Input(format!(
            "'{path}' is larger than the {} MB a run accepts per file",
            MAX_INPUT_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Copies one output from the run's folder into its workspace, through both
/// stores — which need not be the same kind.
async fn persist_one(run_store: &dyn DocStore, plan: &PersistPlan) -> Result<(), FormaError> {
    let doc = run_store.attach(&plan.from).await?;
    if doc.bytes.len() as u64 > MAX_OUTPUT_FILE_BYTES {
        return Err(FormaError::Input(format!(
            "'{}' is larger than an output may be",
            plan.from
        )));
    }
    let workspace = plan.store.connect().await?;
    // A change like any other the agent makes, so it waits for a review too.
    let tracked = changes::track(workspace.as_ref(), &plan.path).await?;
    workspace
        .write_bytes(&plan.path, doc.bytes, &doc.media_type)
        .await?;
    changes::save(workspace.as_ref(), tracked).await?;
    Ok(())
}

/// What a run is billed by (see `toolUnitPrices` in back/pricing.go).
const BILLED_UNIT: &str = "second";

/// Whole seconds, rounded up: a run that started is never free.
fn billed_seconds(d: Duration) -> u64 {
    d.as_millis().div_ceil(1000).max(1) as u64
}

/// What the model is told when the sandbox itself failed.
fn platform_failure(run_id: &str) -> Value {
    json!({
        "run_id": run_id,
        "status": "error",
        "message": "The code execution environment failed before the run could finish (for \
                    example, it ran out of memory or time, or could not start). This is a \
                    problem with the platform, not necessarily with the code. Tell the user \
                    that something went wrong while running the code. You may try again \
                    once, with a smaller or faster job.",
    })
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for CodeExecutionTool {
    fn check(&self) -> Result<(), FormaError> {
        // What runs the code, and where its files go, is the server's to
        // configure and is read at call time, so that an agent loads anywhere.
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let args: RunInput = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("code execution: invalid input: {e}")))?;
        if args.command.trim().is_empty() {
            return Err(FormaError::Input("the command is empty".to_string()));
        }
        if args.command.chars().count() > MAX_COMMAND_CHARS {
            return Err(FormaError::Input(format!(
                "the command is longer than {MAX_COMMAND_CHARS} characters; put it in a file instead"
            )));
        }

        // Everything that can be refused is refused before anything is staged.
        //
        // A run reaches its files only through signed URLs, which a folder on
        // this machine cannot sign: a server that runs code keeps its
        // workspaces in Cloud Storage, which installing its sandbox insisted on
        // (see `host::Host::check`).
        let sandbox = sandbox::open(host::sandbox()?)?;
        let bucket = expect_env_var("WORKSPACES_BUCKET").map_err(|_| {
            FormaError::Input(
                "code execution is not available here: WORKSPACES_BUCKET is not set".to_string(),
            )
        })?;
        let persist = self.plan_persist(&args.persist, &ctx)?;

        // The caller's runs, and this run's own folder within them. Derived
        // from caller, like a scrapbook; nothing the model wrote is in it.
        let runs_root = runs_prefix(&ctx)?;
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let store_at = |prefix: String| {
            DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                bucket: bucket.clone(),
                workspace_id: prefix,
            })
        };
        let runs_store = store_at(runs_root.clone()).connect().await?;
        let run_store = store_at(format!("{runs_root}/{run_id}")).connect().await?;

        let inputs = self
            .stage_inputs(args.files, run_store.as_ref(), runs_store.as_ref(), &ctx)
            .await?;

        // The manifest: every grant the run gets, and nothing else.
        let log_url = |name: &'static str, max_bytes: u64| {
            run_store.signed_url(name, SignedAccess::Write { max_bytes }, URL_EXPIRY)
        };
        let manifest = json!({
            "version": 1,
            "command": args.command,
            "timeout_seconds": COMMAND_TIMEOUT.as_secs(),
            "inputs": inputs
                .iter()
                .map(|i| json!({ "path": i.path, "url": i.url }))
                .collect::<Vec<_>>(),
            "outputs": run_store
                .signed_upload("out/", MAX_OUTPUT_FILE_BYTES, URL_EXPIRY)
                .await?,
            "logs": {
                "stdout": log_url("stdout.txt", MAX_LOG_BYTES).await?,
                "stderr": log_url("stderr.txt", MAX_LOG_BYTES).await?,
                "result": log_url("result.json", MAX_RESULT_BYTES).await?,
            },
            "limits": {
                "max_input_file_bytes": MAX_INPUT_FILE_BYTES,
                "max_input_total_bytes": MAX_INPUT_TOTAL_BYTES,
                "max_output_files": MAX_OUTPUT_FILES,
                "max_output_file_bytes": MAX_OUTPUT_FILE_BYTES,
                "max_output_total_bytes": MAX_OUTPUT_TOTAL_BYTES,
                "max_log_bytes": MAX_LOG_BYTES,
            },
        });
        // The manifest goes through the store rather than the job's overrides:
        // overrides are kept in the execution's history, and the command can
        // carry the user's data.
        run_store
            .write_bytes(
                "manifest.json",
                manifest.to_string().into_bytes(),
                "application/json",
            )
            .await?;
        let manifest_url = run_store
            .signed_url("manifest.json", SignedAccess::Read, URL_EXPIRY)
            .await?;

        let started = std::time::Instant::now();
        let run = sandbox
            .run(&run_id, &manifest_url.url, TASK_TIMEOUT, RUN_DEADLINE)
            .await;
        // Billed whatever happened: a container that failed or timed out still
        // ran. By the platform's own clock when it gives one, else by ours.
        let billed = run
            .as_ref()
            .ok()
            .and_then(|r| r.billed)
            .unwrap_or_else(|| started.elapsed());
        ctx.tx.send(SSEvent::ToolUsage {
            tool_kind: "code-execution".to_string(),
            unit: BILLED_UNIT.to_string(),
            quantity: billed_seconds(billed),
        })?;
        let exit = run.map(|r| r.exit);
        // It holds live credentials and the run folder is readable by the
        // model, so it does not outlive the run.
        if let Err(e) = run_store.delete("manifest.json").await {
            warn!(run_id = %run_id, error = %e, "code execution: could not delete the manifest");
        }

        let log_failure = |reason: &str| {
            // Metadata only: never the command, the files or the output.
            error!(
                run_id = %run_id,
                deployment = ctx.deployment_id.as_deref().unwrap_or("-"),
                agent = %ctx.agent_id,
                backend = %sandbox.describe(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                reason = %reason,
                "code execution: the sandbox failed",
            );
        };
        match exit {
            Ok(SandboxExit::Completed) => {}
            Ok(SandboxExit::Failed { reason }) => {
                log_failure(&reason);
                return Ok(Some(platform_failure(&run_id)));
            }
            Err(e) => {
                log_failure(&e.to_string());
                return Ok(Some(platform_failure(&run_id)));
            }
        }

        // What a run leaves was written by code nobody vetted: its sizes are
        // bounded by the signatures it got, and checked again here.
        let raw = match run_store.attach("result.json").await {
            Ok(doc) if doc.bytes.len() as u64 <= MAX_RESULT_BYTES => doc.bytes,
            Ok(_) => {
                log_failure("result.json is larger than allowed");
                return Ok(Some(platform_failure(&run_id)));
            }
            Err(e) => {
                log_failure(&format!("the run left no readable result.json: {e}"));
                return Ok(Some(platform_failure(&run_id)));
            }
        };
        let result: SandboxResult = match serde_json::from_slice(&raw) {
            Ok(r) => r,
            Err(e) => {
                log_failure(&format!("result.json is not valid: {e}"));
                return Ok(Some(platform_failure(&run_id)));
            }
        };
        if !matches!(
            result.status.as_str(),
            "ok" | "failed" | "timeout" | "error"
        ) {
            log_failure(&format!(
                "result.json has an unknown status '{}'",
                result.status
            ));
            return Ok(Some(platform_failure(&run_id)));
        }
        info!(
            run_id = %run_id,
            status = %result.status,
            exit_code = result.exit_code.unwrap_or(-1),
            duration_ms = result.duration_ms.unwrap_or(0),
            outputs = result.outputs.len(),
            "code execution: run finished",
        );

        if result.status == "error" {
            // The run never started its command: an input could not be placed.
            return Ok(Some(json!({
                "run_id": run_id,
                "status": "error",
                "message": result.error.unwrap_or_else(|| "the run could not start".to_string()),
            })));
        }

        let tail_of = |name: &'static str| {
            let run_store = &run_store;
            async move {
                match run_store.attach(name).await {
                    Ok(doc) if doc.bytes.len() as u64 <= MAX_LOG_BYTES => {
                        let start = doc.bytes.len().saturating_sub(EXCERPT_BYTES);
                        excerpt(&doc.bytes[start..])
                    }
                    Ok(_) => String::new(),
                    Err(e) => {
                        warn!(error = %e, "code execution: could not read {name}");
                        String::new()
                    }
                }
            }
        };
        let stdout = tail_of("stdout.txt").await;
        let stderr = tail_of("stderr.txt").await;

        // Persist only what a successful run produced: a failed run's outputs
        // may be half-written, and would replace good files.
        let mut persisted = Vec::new();
        let mut persist_errors = Vec::new();
        for plan in &persist {
            if result.status != "ok" {
                persist_errors.push(json!({
                    "from": plan.from,
                    "error": "not copied, because the run did not succeed",
                }));
                continue;
            }
            match persist_one(run_store.as_ref(), plan).await {
                Ok(()) => persisted.push(json!({
                    "from": plan.from, "workspace": plan.workspace, "path": plan.path,
                })),
                Err(e) => persist_errors.push(json!({ "from": plan.from, "error": e.to_string() })),
            }
        }

        let outputs: Vec<Value> = result
            .outputs
            .iter()
            .map(|o| json!({ "path": format!("{run_id}/{}", o.path), "media_type": o.media_type, "size": o.size }))
            .collect();
        let mut answer = json!({
            "run_id": run_id,
            "status": result.status,
            "exit_code": result.exit_code,
            "timed_out": result.timed_out,
            "duration_ms": result.duration_ms,
            "stdout": stdout,
            "stdout_total_bytes": result.stdout_bytes,
            "stderr": stderr,
            "stderr_total_bytes": result.stderr_bytes,
            "outputs": outputs,
        });
        if result.stdout_truncated || result.stderr_truncated {
            answer["logs_note"] = json!(format!(
                "Only the first {} MB of stdout/stderr was kept.",
                MAX_LOG_BYTES / (1024 * 1024)
            ));
        }
        if !result.skipped_outputs.is_empty() {
            answer["skipped_outputs"] = Value::Array(result.skipped_outputs);
        }
        if !persisted.is_empty() {
            answer["persisted"] = Value::Array(persisted);
        }
        if !persist_errors.is_empty() {
            answer["persist_errors"] = Value::Array(persist_errors);
        }
        Ok(Some(answer))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_billed_seconds_round_up() {
        use std::time::Duration;
        assert_eq!(super::billed_seconds(Duration::from_millis(90_500)), 91);
        assert_eq!(super::billed_seconds(Duration::from_secs(60)), 60);
        assert_eq!(super::billed_seconds(Duration::ZERO), 1);
    }

    use super::*;

    fn kb_tool(name: &str, op: KnowledgeBaseOperation, scope: WorkspaceScope) -> Tool {
        Tool {
            name: format!("{}-{name}", op.prefix()),
            description: Some(String::new()),
            tool: ToolRunner::Knowledgebase(KnowledgebaseTool {
                backend: DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                    bucket: "b".to_string(),
                    workspace_id: name.to_string(),
                }),
                op,
                scope,
            }),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        }
    }

    fn run_tool() -> Tool {
        Tool {
            name: "run-in-sandbox".to_string(),
            description: Some("Run.".to_string()),
            tool: ToolRunner::CodeExecution(CodeExecutionTool::default()),
            just_for: vec![],
            confirm_before_calling: false,
            bundle: None,
        }
    }

    fn linked(tools: &[Tool]) -> &BTreeMap<String, LinkedWorkspace> {
        tools
            .iter()
            .find_map(|t| match &t.tool {
                ToolRunner::CodeExecution(c) => Some(&c.workspaces),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn test_link_workspaces_follows_the_agents_own_tools() {
        use KnowledgeBaseOperation::*;
        let mut tools = vec![
            run_tool(),
            kb_tool("contabilidad", List, WorkspaceScope::Shared),
            kb_tool("contabilidad", Read, WorkspaceScope::Shared),
            kb_tool("contabilidad", Write, WorkspaceScope::Shared),
            kb_tool("handbook", Read, WorkspaceScope::Shared),
            kb_tool("notes", Read, WorkspaceScope::Scrapbook),
            // The runs store is reached with from_run, never as a workspace.
            kb_tool("sandbox", Read, WorkspaceScope::Runs),
        ];
        link_workspaces(&mut tools);
        let ws = linked(&tools);
        assert_eq!(
            ws.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["contabilidad", "handbook", "notes"]
        );
        assert!(ws["contabilidad"].read.is_some() && ws["contabilidad"].write.is_some());
        assert!(ws["handbook"].write.is_none());

        let description = tools[0].description.as_deref().unwrap();
        assert!(description.contains("(`from_workspace`): contabilidad, handbook, notes."));
        assert!(description.contains("(`to_workspace`): contabilidad."));
    }

    #[test]
    fn test_link_workspaces_keeps_role_limits_and_skips_confirmed_tools() {
        use KnowledgeBaseOperation::*;
        let mut admin_write = kb_tool("ledger", Write, WorkspaceScope::Shared);
        admin_write.just_for = vec!["admin".to_string()];
        let mut confirmed = kb_tool("ledger", Read, WorkspaceScope::Shared);
        confirmed.confirm_before_calling = true;
        let mut tools = vec![run_tool(), admin_write, confirmed];
        link_workspaces(&mut tools);
        let ws = linked(&tools);
        assert!(ws["ledger"].read.is_none());
        assert_eq!(ws["ledger"].write.as_ref().unwrap().just_for, vec!["admin"]);
    }

    #[test]
    fn test_validate_rel_path() {
        for ok in ["a.py", "data/ventas.csv", "out/x/y.png", "ñandú.txt"] {
            assert!(validate_rel_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            ".",
            "a\\b",
            "a\nb",
        ] {
            assert!(validate_rel_path(bad).is_err(), "{bad:?}");
        }
        assert!(validate_rel_path(&"a".repeat(MAX_PATH_BYTES + 1)).is_err());
    }

    #[test]
    fn test_validate_run_id() {
        let id = uuid::Uuid::new_v4().simple().to_string();
        assert!(validate_run_id(&id).is_ok());
        assert!(validate_run_id("../other-user/abc").is_err());
        assert!(validate_run_id(&id.to_uppercase()).is_err());
        assert!(validate_run_id(&format!("{id}/x")).is_err());
    }

    #[test]
    fn test_excerpt_keeps_the_end() {
        let long = "a".repeat(EXCERPT_CHARS) + "THE END";
        let e = excerpt(long.as_bytes());
        assert!(e.ends_with("THE END"));
        assert_eq!(e.chars().count(), EXCERPT_CHARS);
        // A cut through a multi-byte character does not fail.
        assert_eq!(excerpt(&[0xC3]), "\u{FFFD}");
    }

    #[test]
    fn test_result_is_read_defensively() {
        let r: SandboxResult = serde_json::from_str(r#"{"status":"ok","unexpected":1}"#).unwrap();
        assert_eq!(r.status, "ok");
        assert!(r.outputs.is_empty());
    }

    #[test]
    fn test_schema_requires_the_purpose_and_the_command() {
        let schema: JsonSchema = CodeExecutionTool::default().try_into().unwrap();
        assert_eq!(
            schema.required,
            Some(vec!["purpose".to_string(), "command".to_string()])
        );
        let props = schema.properties.unwrap();
        assert!(props.contains_key("files") && props.contains_key("persist"));
    }

    /// The purpose is for the person watching; a run is the same with or
    /// without it.
    #[test]
    fn test_the_purpose_is_optional_to_the_run() {
        let with: RunInput =
            serde_json::from_value(json!({"purpose": "Sumar ventas", "command": "ls"})).unwrap();
        let without: RunInput = serde_json::from_value(json!({"command": "ls"})).unwrap();
        assert_eq!(with.command, without.command);
    }

    #[test]
    fn test_persist_is_checked_before_anything_runs() {
        use crate::tool::workspace::KnowledgeBaseOperation::*;
        let mut tools = vec![
            run_tool(),
            kb_tool("handbook", Read, WorkspaceScope::Shared),
            kb_tool("notes", Write, WorkspaceScope::Scrapbook),
        ];
        link_workspaces(&mut tools);
        let ToolRunner::CodeExecution(tool) = &tools[0].tool else {
            unreachable!()
        };
        let ctx = test_ctx();
        let plan = |from: &str, ws: &str, path: &str| {
            tool.plan_persist(
                &[PersistInput {
                    from: from.to_string(),
                    to_workspace: ws.to_string(),
                    path: path.to_string(),
                }],
                &ctx,
            )
        };
        // Read-only workspace, unknown workspace, outside out/, bad paths.
        assert!(plan("out/a.csv", "handbook", "a.csv").is_err());
        assert!(plan("out/a.csv", "nope", "a.csv").is_err());
        assert!(plan("in/a.csv", "notes", "a.csv").is_err());
        assert!(plan("out/../manifest.json", "notes", "a.csv").is_err());
        assert!(plan("out/a.csv", "notes", "../a.csv").is_err());
        // A scrapbook resolves to the caller's own prefix.
        let ok = plan("out/a.csv", "notes", "a.csv").unwrap();
        let DocStoreBackend::GoogleKnowledgebase(config) = &ok[0].store;
        assert_eq!(
            config.object_name(&ok[0].path),
            "scrapbooks/helper/alice/a.csv"
        );
    }

    /// A real run, start to finish: real signatures, a real bucket, the real
    /// sandbox image in local Docker. Needs `WORKSPACES_BUCKET`,
    /// application-default credentials,
    /// `GCS_SIGNING_ACCOUNT` off Google Cloud, and
    /// `docker build -t forma-sandbox:dev forma/sandbox`. Deletes what it wrote.
    /// Run with:
    ///
    /// `cargo test -p agent --features integration_test end_to_end -- --ignored --nocapture`
    #[cfg(feature = "integration_test")]
    #[tokio::test]
    #[ignore = "needs Docker, the sandbox image, a bucket and credentials"]
    async fn test_code_execution_end_to_end() {
        // Failures are reported through tracing; show them.
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        host::install(host::Host {
            workspaces: Some(crate::tool::workspace::StoreKind::Gcs),
            sandbox: Some(sandbox::SandboxKind::LocalDocker {
                image: sandbox::DEFAULT_LOCAL_IMAGE.to_string(),
            }),
        })
        .unwrap();
        let tool = CodeExecutionTool::default();
        // Its own user, so cleaning up cannot touch anybody's real runs.
        let mut ctx = test_ctx();
        ctx.user_id = format!("e2e-{}", uuid::Uuid::new_v4().simple()).into();
        let run = |input: Value| {
            let tool = tool.clone();
            let ctx = ctx.clone();
            async move {
                <CodeExecutionTool as ToolTrait<db::NoneDB>>::execute(&tool, input, ctx)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };

        let first = run(json!({
            "command": "mkdir -p out && grep -o r notes.txt | wc -l | tr -d ' ' | tee out/count.txt",
            "files": [{ "path": "notes.txt", "content": "strawberry" }],
        }))
        .await;
        println!("{first:#}");
        let run_id = first["run_id"].as_str().unwrap_or_default().to_string();

        // A second run takes the first one's output.
        let second = run(json!({
            "command": "cat count.txt; echo oops >&2; exit 4",
            "files": [{ "path": "count.txt", "from_run": run_id, "source": "out/count.txt" }],
        }))
        .await;
        println!("{second:#}");

        // The agent reads what the run left through the runs store's tool.
        let reader = KnowledgebaseTool {
            backend: DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                bucket: crate::tool::workspace::default_workspace_bucket(),
                workspace_id: String::new(),
            }),
            op: KnowledgeBaseOperation::Read,
            scope: WorkspaceScope::Runs,
        };
        let read = <KnowledgebaseTool as ToolTrait<db::NoneDB>>::execute(
            &reader,
            json!({ "path": format!("{run_id}/out/count.txt") }),
            ctx.clone(),
        )
        .await;

        // Clean up before judging, so a failure leaves nothing behind.
        let runs = DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
            bucket: crate::tool::workspace::default_workspace_bucket(),
            workspace_id: runs_prefix(&ctx).unwrap(),
        })
        .connect()
        .await
        .unwrap();
        let left = runs.list(None).await.unwrap();
        for doc in &left {
            runs.delete(&doc.path).await.unwrap();
        }

        assert_eq!(first["status"], "ok", "{first:#}");
        assert_eq!(first["stdout"].as_str().unwrap().trim(), "3");
        assert_eq!(
            first["outputs"][0]["path"],
            format!("{run_id}/out/count.txt")
        );
        assert_eq!(second["status"], "failed");
        assert_eq!(second["exit_code"], 4);
        assert_eq!(second["stdout"].as_str().unwrap().trim(), "3");
        assert_eq!(second["stderr"].as_str().unwrap().trim(), "oops");
        let read = read.unwrap().unwrap();
        assert_eq!(read["content"].as_str().unwrap().trim(), "3");
        // The manifests, with their live URLs, were gone before cleanup.
        assert!(left.iter().all(|d| !d.path.ends_with("manifest.json")));
    }

    fn test_ctx() -> CallContext<db::NoneDB> {
        use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
        use std::sync::Arc;
        use tokio::sync::{Mutex, RwLock};
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(rx);
        CallContext {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "job".into(),
            user_roles: Arc::new(vec![]),
            user_id: "alice".into(),
            agent_id: "helper".into(),
            deployment_id: None,
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            db: Arc::new(db::NoneDB::default()),
        }
    }
}
