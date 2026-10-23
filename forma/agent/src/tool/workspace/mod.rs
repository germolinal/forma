//! Agentic knowledgebase retrieval.
//!
//! A single knowledgebase (a [`DocStoreBackend`]) is exposed to the LLM as a
//! *set* of small tools — `list`, `read` and `search` — rather than one opaque
//! "search" call. The agent navigates the corpus the way a person would: list
//! what is there, search for the relevant document, then read it (in slices, if
//! it is large). This is the "agentic search" alternative to embedding-based
//! RAG: no vectors, no ingestion pipeline, and exact citations by path.
//!
//! The storage is abstracted behind [`protocols::DocStore`], and which store
//! holds the workspaces is the server's choice, not the agent's ([`StoreKind`],
//! from the server's `runtime.yaml`; see [`crate::tool::host`]): Cloud Storage
//! ([`gcs::GcsDocStore`]) or a folder ([`local::LocalDocStore`]). The same
//! definition works with either, so agents stay portable.

pub mod attach;
pub mod changes;
pub mod conversation;
pub mod gcs;
pub(crate) mod gcs_signing;
pub mod local;

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{AttachedDoc, DocContent, DocStore, FormaDB, FormaError, envar::expect_env_var};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    CallContext, Tool, ToolRunner, ToolTrait,
    tool::{host, mailgun_tool::string_schema},
};

/// Default cap on how many characters [`KbOp::Read`] returns in one call.
const DEFAULT_READ_LIMIT: usize = 20_000;
/// The bucket workspaces live in when a definition does not name one.
///
/// Returns an empty string when `WORKSPACES_BUCKET` is unset, which
/// [`GcsDocConfig::check`](gcs::GcsDocConfig::check) then rejects with a proper
/// error. It must not panic: this is a serde `default`, so it runs while
/// deserializing agent definitions — including untrusted ones.
pub fn default_workspace_bucket() -> String {
    expect_env_var("WORKSPACES_BUCKET").unwrap_or_default()
}

/// Which store holds this server's workspaces — every one of them:
/// knowledgebases, workspaces, scrapbooks, and code-execution runs.
///
/// Like the sandbox code runs in, it is chosen by whoever runs the server, in
/// its `runtime.yaml`; an agent's definition says *which* workspace, never
/// where it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreKind {
    /// Cloud Storage: the bucket a workspace names, or `WORKSPACES_BUCKET`.
    Gcs,
    /// A folder on this machine, standing in for the bucket.
    Local(PathBuf),
}

/// Root prefix under which every scrapbook lives, keeping them in their own
/// corner of the bucket so a lifecycle rule or a user-deletion job can target
/// them without touching shared workspaces.
const SCRAPBOOK_ROOT: &str = "scrapbooks";

/// Root prefix under which every code-execution run keeps its files, one
/// folder per run: `runs/{owner}/{user}/{run_id}/`. Kept apart from everything
/// else in the bucket so a lifecycle rule that deletes runs (e.g., after a
/// day) can never reach a workspace or a scrapbook.
pub(crate) const RUNS_ROOT: &str = "runs";

/// Where a docstore tool's prefix comes from, and what it takes to reach it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceScope {
    /// A fixed workspace chosen by the agent's author and shared by everyone
    /// talking to it. It is an organisation-owned resource, so reaching it
    /// requires a matching entry in the deployment's `workspaces` credential.
    #[default]
    Shared,
    /// A private store belonging to the deployed agent and user making the
    /// call. Its prefix is *derived* from that pair at call time, so an agent
    /// cannot name anybody else's scrapbook and there is nothing for a
    /// deployment to be granted: authority comes from the derivation itself.
    /// See [`scrapbook_prefix`].
    Scrapbook,
    /// The files of this user's code-execution runs (see
    /// [`crate::tool::code_execution`]): their outputs and logs. Derived from
    /// the caller exactly like a scrapbook — `runs/{owner}/{user}` — and so
    /// needs no grant either. Read-only: only a run writes here.
    Runs,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Scope {
    Read,
    Write,
}

#[derive(Debug, Serialize, Deserialize)]
struct KnowledgebaseCredentials {
    path: String,
    scopes: Vec<Scope>,
}

impl KnowledgeBaseOperation {
    fn scope(&self) -> Scope {
        match self {
            Self::List | Self::Read => Scope::Read,
            Self::Write | Self::Append | Self::Edit | Self::Move | Self::Delete => Scope::Write,
        }
    }
}

impl KnowledgebaseCredentials {
    fn can_access(&self, id: &str, operation: KnowledgeBaseOperation) -> bool {
        self.path == id && self.scopes.contains(&operation.scope())
    }
}

/// Percent-encodes one segment of a scrapbook prefix.
///
/// A scrapbook's isolation rests entirely on its prefix being derived from the
/// caller's identity, so no identifier may contribute a `/` (which would widen
/// or move the prefix) or anything else that is awkward in an object name.
/// Anything outside a conservative allowlist is escaped rather than dropped, so
/// two distinct IDs can never collapse onto the same scrapbook.
///
/// Empty identifiers are rejected: falling back to a placeholder would quietly
/// hand every anonymous caller the *same* scrapbook.
fn sanitize_segment(kind: &str, value: &str) -> Result<String, FormaError> {
    if value.trim().is_empty() {
        return Err(FormaError::Input(format!(
            "scrapbook: cannot derive a private workspace without a {kind}"
        )));
    }
    // `.` survives the allowlist below because it is common in real IDs, which
    // leaves `.`/`..` as whole segments. Object names are literal in GCS, so
    // these cannot traverse anything here — but they do mean something to
    // anything downstream that normalises paths (a cleanup job, a local sync),
    // so refuse them rather than leave that landmine.
    if value.chars().all(|c| c == '.') {
        return Err(FormaError::Input(format!(
            "scrapbook: {kind} must not consist only of dots"
        )));
    }
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' => out.push(c),
            _ => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    Ok(out)
}

/// Derives the object prefix of the scrapbook belonging to the caller of `ctx`:
/// `scrapbooks/{owner}/{user}`.
///
/// `owner` identifies the deployed agent — its deployment ID where there is
/// one, and otherwise the agent's own ID, for single-tenant setups that serve
/// exactly one agent per process. A deployment currently hosts exactly one
/// agent, so one segment names both.
///
/// The deployment ID is *preferred* rather than merely accepted, even though
/// the two are the same string today: multi-tenant looks an agent up by its
/// deployment ID and then overwrites `agent.id` with it. Were that assignment
/// ever dropped, `agent_id` would fall back to the name the author wrote in
/// their YAML, and two tenants that both called their agent `assistant` would
/// start sharing scrapbooks. Keying on the deployment ID — a globally unique
/// document ID — means that refactor cannot silently join two tenants' data.
///
/// The user comes last, and is never the whole key: user IDs arrive in a
/// per-request header and are only unique *within* a deployment. Ordering it
/// this way also keeps both deletions that matter to a single prefix — one user
/// is `scrapbooks/{owner}/{user}/`, and a whole deployment is
/// `scrapbooks/{owner}/`.
fn scrapbook_prefix<DB: FormaDB>(ctx: &CallContext<DB>) -> Result<String, FormaError> {
    caller_prefix(SCRAPBOOK_ROOT, ctx)
}

/// Derives the prefix holding the code-execution runs of the caller of `ctx`:
/// `runs/{owner}/{user}`. Same derivation, and same reasoning, as
/// [`scrapbook_prefix`]: a run is only ever reachable by the deployment and
/// user that started it.
pub(crate) fn runs_prefix<DB: FormaDB>(ctx: &CallContext<DB>) -> Result<String, FormaError> {
    caller_prefix(RUNS_ROOT, ctx)
}

/// `{root}/{owner}/{user}` for the caller of `ctx`; see [`scrapbook_prefix`].
fn caller_prefix<DB: FormaDB>(root: &str, ctx: &CallContext<DB>) -> Result<String, FormaError> {
    let owner = match &ctx.deployment_id {
        Some(deployment) => sanitize_segment("deployment id", deployment)?,
        None => sanitize_segment("agent id", &ctx.agent_id)?,
    };
    let user = sanitize_segment("user id", &ctx.user_id)?;
    Ok(format!("{root}/{owner}/{user}"))
}

/// Configuration selecting which storage backend holds a knowledgebase, along
/// with the details needed to reach it.
///
/// This is plain, serializable data (no live connection), so it round-trips
/// through agent definitions unchanged. [`DocStoreBackend::connect`] turns it
/// into a live [`DocStore`] at call time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "kebab-case")]
pub enum DocStoreBackend {
    /// Documents stored as objects in a Google Cloud Storage bucket.
    GoogleKnowledgebase(gcs::GcsDocConfig),
}

impl DocStoreBackend {
    /// Validates the backend configuration without opening a connection,
    /// against the store this server keeps workspaces in.
    ///
    /// Where no store is known — a server that keeps no workspaces, or one that
    /// runs no agents, like `forma-manager` — there is nothing to check it
    /// against, and the agent still loads: as with code execution, a tool
    /// that cannot work says so when it is called.
    pub fn check(&self) -> Result<(), FormaError> {
        let store = host::installed().and_then(|h| h.workspaces.as_ref());
        match (self, store) {
            // A folder needs no bucket.
            (Self::GoogleKnowledgebase(_), Some(StoreKind::Local(_))) => Ok(()),
            (Self::GoogleKnowledgebase(c), Some(StoreKind::Gcs)) => c.check(),
            (Self::GoogleKnowledgebase(_), None) => Ok(()),
        }
    }

    /// Opens a live [`DocStore`] for this workspace, in whichever store this
    /// server keeps workspaces ([`StoreKind`]).
    pub async fn connect(&self) -> Result<Box<dyn DocStore>, FormaError> {
        self.connect_to(host::workspaces()?.clone())
    }

    /// Opens this workspace in the store `kind`.
    fn connect_to(&self, kind: StoreKind) -> Result<Box<dyn DocStore>, FormaError> {
        match (self, kind) {
            (Self::GoogleKnowledgebase(c), StoreKind::Gcs) => {
                Ok(Box::new(gcs::GcsDocStore::new(c.clone())))
            }
            (Self::GoogleKnowledgebase(c), StoreKind::Local(root)) => {
                Ok(Box::new(local::LocalDocStore::new(&root, c)?))
            }
        }
    }

    /// The path this backend is granted against, as matched by the
    /// deployment's `workspaces` credential.
    fn resource_path(&self) -> String {
        match self {
            Self::GoogleKnowledgebase(c) => c.knowledgebase_id().to_string(),
        }
    }

    /// The same storage, re-pointed at `workspace_id`.
    ///
    /// Used to aim a backend at a prefix that is only known at call time — a
    /// scrapbook's, derived from who is calling — while leaving the rest of the
    /// configuration (the bucket) as the author wrote it.
    fn scoped_to(&self, workspace_id: String) -> Self {
        match self {
            Self::GoogleKnowledgebase(c) => Self::GoogleKnowledgebase(gcs::GcsDocConfig {
                bucket: c.bucket.clone(),
                workspace_id,
            }),
        }
    }
}

/// Opens the store that `workspace` names, among the ones these tools reach.
///
/// This is how a reference kept in a conversation is turned back into storage,
/// and it deliberately goes through the tools rather than around them: the
/// backend is re-authorized against the deployment's grants every time, so a
/// reference in an old message stops resolving once the agent loses access to
/// what it names.
pub async fn store_for_workspace<DB: FormaDB>(
    tools: &[Tool],
    workspace: &str,
    ctx: &CallContext<DB>,
) -> Result<Box<dyn DocStore>, FormaError> {
    for tool in tools {
        let ToolRunner::Knowledgebase(kb) = &tool.tool else {
            continue;
        };
        if kb.op.scope() != Scope::Read {
            continue;
        }
        let backend = kb.resolve_backend(ctx)?;
        if backend.resource_path() == workspace {
            return backend.connect().await;
        }
    }
    Err(FormaError::Input(format!(
        "this agent has no access to the workspace '{workspace}'"
    )))
}

/// Which agentic-retrieval operation a given [`KnowledgebaseTool`] exposes. One
/// knowledgebase (or workspace) expands into one tool per operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KnowledgeBaseOperation {
    /// List the documents in the knowledgebase (optionally under a prefix).
    List,
    /// Read one document: its text (optionally a slice of it), or — for a
    /// picture or a PDF — the file itself, attached to the conversation. The
    /// tool decides which, so the model never has to.
    Read,
    /// Create or overwrite a document. Requires a write-scoped credential, as
    /// does every operation below.
    Write,
    /// Add text at the end of a document, creating it if it is not there.
    Append,
    /// Replace one exact passage of a text document.
    Edit,
    /// Move or rename a document.
    Move,
    /// Delete a document.
    Delete,
}

impl KnowledgeBaseOperation {
    /// The prefix prepended to the base tool name to form this operation's tool
    /// name (e.g. base `handbook` → `read-from-handbook`).
    pub fn prefix(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read-from",
            Self::Write => "write-to",
            Self::Append => "append-to",
            Self::Edit => "edit-in",
            Self::Move => "move-in",
            Self::Delete => "delete-from",
        }
    }

    /// A description of this operation, tailored to the LLM. `kb` is a
    /// human-readable name/description of the knowledgebase, when available.
    pub fn describe(self, kb: Option<&str>, scope: WorkspaceScope) -> String {
        match scope {
            WorkspaceScope::Scrapbook => return self.describe_scrapbook(kb),
            WorkspaceScope::Runs => return self.describe_runs(kb),
            WorkspaceScope::Shared => {}
        }
        let about = kb
            .map(|k| format!(" in the \"{k}\" knowledgebase"))
            .unwrap_or_default();
        match self {
            Self::List => {
                format!("List the documents{about}, with what each one is (its media type).")
            }
            Self::Read => {
                format!("Read a single document{about}, identified by its `path`. {READ_HOW}")
            }
            Self::Write => format!(
                "Create or overwrite a document{about} in its persistent backend. Documents are identified by their `path`, with \
                 the given `content`. If a document already exists at that path it is \
                 replaced in full, so to change part of it use `edit-in` instead. \
                 {SAVE_HOW} {REVIEWED}"
            ),
            Self::Append | Self::Edit | Self::Move | Self::Delete => self.describe_change(&about),
        }
    }

    /// Description of an operation that changes a document, the same wherever
    /// the document is: `about` says where.
    fn describe_change(self, about: &str) -> String {
        match self {
            Self::Append => format!(
                "Add text at the end of a document{about}, identified by its `path`, \
                 without rewriting it — for a log, a list, an index. The document is \
                 created if it is not there. A line break is added before your text \
                 when the document does not already end with one. {REVIEWED}"
            ),
            Self::Edit => format!(
                "Change part of a text document{about}: replace the passage `old` with \
                 `new`. `old` must appear in the document exactly once, character for \
                 character — include enough of the surrounding text to make it unique. \
                 To delete a passage, leave `new` empty. Prefer this to rewriting a \
                 whole document. {REVIEWED}"
            ),
            Self::Move => format!(
                "Move or rename a document{about}, from `from` to `to` (both paths). \
                 Folders need no creating: a path puts the document in them. Refused \
                 if something is already at `to`. {REVIEWED}"
            ),
            Self::Delete => {
                format!("Delete a document{about}, identified by its `path`. {REVIEWED}")
            }
            Self::List | Self::Read | Self::Write => unreachable!("not a change"),
        }
    }

    /// Description used when the store is a [`WorkspaceScope::Scrapbook`].
    ///
    /// Deliberately different copy from the shared case: a scrapbook is not a
    /// corpus to research, it is a notebook this agent and this user keep
    /// together across conversations. Every operation restates that the user
    /// can read it — the model would otherwise be free to assume it is private
    /// scratch space, and might either hide from the user what it wrote or
    /// write things it would not be willing to show them.
    fn describe_scrapbook(self, kb: Option<&str>) -> String {
        let about = kb.map(|k| format!(" (\"{k}\")")).unwrap_or_default();
        match self {
            Self::List => format!(
                "List the documents in your scrapbook{about}: the private, persistent \
                 notebook you share with this user (both can read/write and edit)."
            ),
            Self::Read => format!(
                "Read a document from your scrapbook{about}: the private, persistent \
                 notebook you share with this user (both can read/write and edit) — identified by its `path`. \
                 {READ_HOW}"
            ),
            Self::Write => format!(
                "Save a document to your scrapbook{about}: the private, persistent \
                 notebook you share with this user (both can read/write and edit). If a document already exists at that path it is \
                 replaced in full, so to change part of it use `edit-in` instead. \
                 {SAVE_HOW} {REVIEWED}"
            ),
            Self::Append | Self::Edit | Self::Move | Self::Delete => {
                self.describe_change(&format!(" in your scrapbook{about}"))
            }
        }
    }
    /// Description used when the store is a [`WorkspaceScope::Runs`].
    fn describe_runs(self, kb: Option<&str>) -> String {
        let about = kb.map(|k| format!(" (\"{k}\")")).unwrap_or_default();
        let paths = "Paths look like `<run_id>/out/<file>`, `<run_id>/stdout.txt` and \
                     `<run_id>/stderr.txt`, with the `run_id` the run reported. Runs are \
                     deleted after about a day.";
        match self {
            Self::List => format!("List the files left by your code runs{about}. {paths}"),
            Self::Read => format!(
                "Read a file left by one of your code runs{about}: an output (a chart \
                 too), or the full stdout/stderr when the excerpt in the run's result is \
                 not enough. {paths} {READ_HOW}"
            ),
            Self::Write | Self::Append | Self::Edit | Self::Move | Self::Delete => {
                format!("Code runs{about} cannot be written to.")
            }
        }
    }
}

/// How reading works, said the same way by every read tool.
const READ_HOW: &str = "A text document comes back as text; large ones can be read in \
     slices with `offset` and `limit`. A picture or a PDF is attached to the \
     conversation instead, so you see it as it is. Anything else (a spreadsheet, a \
     presentation, a Word document) cannot be read, and the tool says so.";

/// How writing saves a file that is already in the conversation.
const SAVE_HOW: &str = "To save a file that is in this conversation — a picture the user \
     sent, or one you generated — give `from_conversation` with its handle (shown \
     as [file-…] next to the file, or returned by the tool that made it) instead \
     of `content`.";

/// What every changing operation tells the model about review.
const REVIEWED: &str = "The change takes effect at once, but the user reviews it and may \
     revert it.";

/// A tool that exposes exactly one [`KbOp`] over a knowledgebase [`DocStoreBackend`].
///
/// Several of these (sharing a `backend` but differing in `op`) are produced
/// from a single knowledgebase tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgebaseTool {
    /// The storage backend the operation runs against.
    pub backend: DocStoreBackend,
    /// The operation this particular tool performs.
    pub op: KnowledgeBaseOperation,
    /// Whether the store is shared (and permissioned) or private to the caller.
    ///
    /// Defaults to [`WorkspaceScope::Shared`] so that tools serialized before
    /// scrapbooks existed keep their permission checks when read back.
    #[serde(default)]
    pub scope: WorkspaceScope,
}

/// Helper: an integer property schema.
fn integer_schema(description: &str) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Integer),
        description: Some(description.to_string()),
        ..Default::default()
    }
}

fn object_schema(props: HashMap<String, JsonSchema>, required: Vec<String>) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Object),
        properties: Some(props),
        required: Some(required),
        additional_properties: false,
        ..Default::default()
    }
}

impl TryFrom<KnowledgebaseTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: KnowledgebaseTool) -> Result<JsonSchema, FormaError> {
        let schema = match item.op {
            KnowledgeBaseOperation::List => {
                let props = HashMap::new();
                // props.insert(
                //     "prefix".to_string(),
                //     string_schema(
                //         "Optional path prefix; only documents whose path starts with it \
                //          are returned. Omit to list everything.",
                //     ),
                // );
                object_schema(props, vec![])
            }
            KnowledgeBaseOperation::Read => {
                let mut props = HashMap::new();
                props.insert(
                    "path".to_string(),
                    string_schema("The path of the document to read, as returned by list."),
                );
                props.insert(
                    "offset".to_string(),
                    integer_schema(
                        "Character offset to start reading a text document from. Defaults to \
                         0 (the start). Ignored for an attached file.",
                    ),
                );
                props.insert(
                    "limit".to_string(),
                    integer_schema(
                        "Maximum number of characters of text to return. Use with `offset` \
                         to page through a long document. Ignored for an attached file.",
                    ),
                );
                object_schema(props, vec!["path".to_string()])
            }
            KnowledgeBaseOperation::Write => {
                let mut props = HashMap::new();
                props.insert(
                    "path".to_string(),
                    string_schema(
                        "The path of the document to create or overwrite (e.g. \
                         `notes/summary.md`).",
                    ),
                );
                props.insert(
                    "content".to_string(),
                    string_schema(
                        "The full text content to store at `path`. Give exactly one of \
                         `content` or `from_conversation`.",
                    ),
                );
                props.insert(
                    "from_conversation".to_string(),
                    string_schema(
                        "The handle of a file in this conversation (`file-…`), to store \
                         that file at `path` as it is. Give exactly one of `content` or \
                         `from_conversation`.",
                    ),
                );
                object_schema(props, vec!["path".to_string()])
            }
            KnowledgeBaseOperation::Append => {
                let mut props = HashMap::new();
                props.insert(
                    "path".to_string(),
                    string_schema("The path of the document to add to (e.g. `notes/log.md`)."),
                );
                props.insert(
                    "content".to_string(),
                    string_schema("The text to add at the end."),
                );
                object_schema(props, vec!["path".to_string(), "content".to_string()])
            }
            KnowledgeBaseOperation::Edit => {
                let mut props = HashMap::new();
                props.insert(
                    "path".to_string(),
                    string_schema("The path of the text document to change."),
                );
                props.insert(
                    "old".to_string(),
                    string_schema(
                        "The passage to replace, exactly as it is in the document. It must \
                         appear exactly once.",
                    ),
                );
                props.insert(
                    "new".to_string(),
                    string_schema("What to put in its place. Empty to delete the passage."),
                );
                object_schema(
                    props,
                    vec!["path".to_string(), "old".to_string(), "new".to_string()],
                )
            }
            KnowledgeBaseOperation::Move => {
                let mut props = HashMap::new();
                props.insert(
                    "from".to_string(),
                    string_schema("The path of the document to move."),
                );
                props.insert(
                    "to".to_string(),
                    string_schema("Its new path (e.g. `archive/2025/notes.md`)."),
                );
                object_schema(props, vec!["from".to_string(), "to".to_string()])
            }
            KnowledgeBaseOperation::Delete => {
                let mut props = HashMap::new();
                props.insert(
                    "path".to_string(),
                    string_schema("The path of the document to delete."),
                );
                object_schema(props, vec!["path".to_string()])
            }
        };
        Ok(schema)
    }
}

/// Input the LLM supplies for [`KbOp::List`].
#[derive(Debug, Deserialize)]
struct ListInput {
    #[serde(default)]
    prefix: Option<String>,
}

/// Input the LLM supplies for [`KbOp::Read`].
#[derive(Debug, Deserialize)]
struct ReadInput {
    path: String,
    /// Floats, because models sometimes send whole numbers as `6000.0`; see
    /// [`ReadInput::offset`] and [`ReadInput::limit`] for the rounding.
    #[serde(default)]
    offset: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
}

impl ReadInput {
    /// Rounded down, so reading starts no later than the model asked.
    /// Negative values saturate to 0.
    fn offset(&self) -> usize {
        self.offset.map_or(0, |n| n.floor() as usize)
    }

    /// Rounded up, so the model gets no less text than it asked for.
    fn limit(&self) -> usize {
        self.limit.map_or(DEFAULT_READ_LIMIT, |n| n.ceil() as usize)
    }
}

/// Input the LLM supplies for [`KbOp::Write`].
#[derive(Debug, Deserialize)]
struct WriteInput {
    path: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    from_conversation: Option<String>,
}

/// Input the LLM supplies for [`KbOp::Append`].
#[derive(Debug, Deserialize)]
struct AppendInput {
    path: String,
    content: String,
}

/// Input the LLM supplies for [`KbOp::Edit`].
#[derive(Debug, Deserialize)]
struct EditInput {
    path: String,
    old: String,
    new: String,
}

/// Input the LLM supplies for [`KbOp::Move`].
#[derive(Debug, Deserialize)]
struct MoveInput {
    from: String,
    to: String,
}

/// Input the LLM supplies for [`KbOp::Delete`].
#[derive(Debug, Deserialize)]
struct DeleteInput {
    path: String,
}

/// Parses the input of `op`, saying which operation it was for when it fails.
fn input_for<T: serde::de::DeserializeOwned>(op: &str, input: Value) -> Result<T, FormaError> {
    serde_json::from_value(input)
        .map_err(|e| FormaError::Input(format!("knowledgebase {op}: invalid input: {e}")))
}

/// The path an agent named, ready to change: relative, and outside the
/// workspace's own records of what changed.
fn change_path(raw: &str) -> Result<String, FormaError> {
    let path = raw.trim().trim_start_matches('/');
    if path.is_empty() {
        return Err(FormaError::Input("Empty document path".to_string()));
    }
    changes::check_visible(path)?;
    Ok(path.to_string())
}

/// The text document at `path`, with what it is stored as, or why it cannot
/// be changed as text. Exact: a lenient match on a name would change the
/// wrong file.
async fn text_document(store: &dyn DocStore, path: &str) -> Result<(String, String), FormaError> {
    if store.stat(path).await?.is_none() {
        return Err(FormaError::Input(format!(
            "there is no document at '{path}'"
        )));
    }
    let doc = store.attach(path).await?;
    match readable(&doc)? {
        Readable::Text(text) => Ok((text, doc.media_type)),
        Readable::Attach => Err(FormaError::Input(format!(
            "'{path}' is not a text document, so it cannot be changed in part"
        ))),
    }
}

impl KnowledgebaseTool {
    /// The grant this tool needs from its deployment, as `(path, scope)` —
    /// `scope` being `read` or `write`, matching [`KnowledgebaseCredentials`].
    ///
    /// `None` for the stores derived from the caller (scrapbooks, code-execution
    /// runs): no deployment is granted those, because nobody can name anybody
    /// else's.
    pub fn required_grant(&self) -> Option<(String, &'static str)> {
        match self.scope {
            WorkspaceScope::Shared => {
                let scope = match self.op.scope() {
                    Scope::Read => "read",
                    Scope::Write => "write",
                };
                Some((self.backend.resource_path(), scope))
            }
            WorkspaceScope::Scrapbook | WorkspaceScope::Runs => None,
        }
    }

    /// Checks the deployment was granted this operation on this store.
    ///
    /// Only meaningful for [`WorkspaceScope::Shared`] stores; see
    /// [`KnowledgebaseTool::resolve_backend`].
    fn authorize<DB: FormaDB>(&self, ctx: &CallContext<DB>) -> Result<(), FormaError> {
        let Some(credentials) = ctx.deployment_credentials.get("workspaces") else {
            tracing::info!("no workspace credentials found... no access");
            return Err(FormaError::Input(
                "agent does not have access to knowledgebase".to_string(),
            ));
        };

        match serde_json::from_value::<Vec<KnowledgebaseCredentials>>(credentials.clone()) {
            Ok(credentials) => {
                let knowledgebase_id = self.backend.resource_path();
                let access = credentials
                    .iter()
                    .find(|x| x.can_access(&knowledgebase_id, self.op));
                if access.is_none() {
                    tracing::info!(
                        "agent does not have access to knowledgebase {knowledgebase_id}"
                    );
                    return Err(FormaError::Input(format!(
                        "agent does not have access to knowledgebase {knowledgebase_id}"
                    )));
                }
                Ok(())
            }
            Err(e) => {
                tracing::warn!("knowledgebase: invalid credentials: {e}");
                Err(FormaError::Input(format!(
                    "knowledgebase: invalid credentials: {e}"
                )))
            }
        }
    }

    /// Works out which store this call actually runs against, enforcing
    /// whatever that scope requires.
    ///
    /// The two scopes are secured differently rather than one being a weakened
    /// version of the other. A shared store is named by the author, so the
    /// deployment must hold a grant for it. A scrapbook is *not* named by
    /// anyone: its prefix is computed from the caller's identity, so the agent
    /// has no way to express another user's scrapbook and a grant would have
    /// nothing to add.
    pub(crate) fn resolve_backend<DB: FormaDB>(
        &self,
        ctx: &CallContext<DB>,
    ) -> Result<DocStoreBackend, FormaError> {
        match self.scope {
            WorkspaceScope::Shared => {
                self.authorize(ctx)?;
                Ok(self.backend.clone())
            }
            WorkspaceScope::Scrapbook => Ok(self.backend.scoped_to(scrapbook_prefix(ctx)?)),
            WorkspaceScope::Runs => {
                // Only a run writes its own folder, through the URLs it was
                // signed; nothing the model calls may.
                if self.op.scope() == Scope::Write {
                    return Err(FormaError::Input(
                        "code runs cannot be written to".to_string(),
                    ));
                }
                Ok(self.backend.scoped_to(runs_prefix(ctx)?))
            }
        }
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for KnowledgebaseTool {
    fn check(&self) -> Result<(), FormaError> {
        self.backend.check()
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let backend = self.resolve_backend(&ctx)?;
        let store = backend.connect().await?;
        self.run(&backend, store.as_ref(), input, &ctx).await
    }
}

impl KnowledgebaseTool {
    /// Runs this tool's operation on `store`, the one `backend` opened.
    async fn run<DB: FormaDB>(
        &self,
        backend: &DocStoreBackend,
        store: &dyn DocStore,
        input: Value,
        ctx: &CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        match self.op {
            KnowledgeBaseOperation::List => {
                let args: ListInput = serde_json::from_value(input).map_err(|e| {
                    FormaError::Input(format!("knowledgebase list: invalid input: {e}"))
                })?;
                let mut docs = store.list(args.prefix.as_deref()).await?;
                docs.retain(|d| !changes::is_hidden(&d.path));
                Ok(Some(json!({ "documents": docs })))
            }
            KnowledgeBaseOperation::Read => {
                let args: ReadInput = serde_json::from_value(input).map_err(|e| {
                    FormaError::Input(format!("knowledgebase read: invalid input: {e}"))
                })?;
                if args.path.trim().is_empty() {
                    return Err(FormaError::Input("Empty document path".to_string()));
                }
                changes::check_visible(&args.path)?;
                let doc = store.attach(&args.path).await?;
                match readable(&doc)? {
                    Readable::Text(full) => {
                        let (content, truncated) =
                            gcs::slice_content(&full, args.offset(), Some(args.limit()));
                        Ok(Some(serde_json::to_value(DocContent {
                            path: doc.path,
                            total_len: full.chars().count(),
                            content,
                            truncated,
                        })?))
                    }
                    // Only a reference goes in the result; the file itself is
                    // fetched again whenever a request is built. Checked now
                    // so that a file too large to send is an answer this call
                    // can give, rather than an error while the next request is
                    // being built, which the model could neither see nor
                    // recover from.
                    Readable::Attach => Ok(Some(json!({
                        // What `attached` means to the conversation: see
                        // `push_in_memory_tool_result`, which turns it into the
                        // part carrying the file.
                        "attached": {
                            "workspace": backend.resource_path(),
                            "path": doc.path,
                            "media_type": doc.media_type,
                            "size_bytes": doc.size_bytes(),
                        },
                        "note": "The document is in front of you in this conversation; read it from there."
                    }))),
                }
            }
            KnowledgeBaseOperation::Write => {
                let args: WriteInput = input_for("write", input)?;
                let path = change_path(&args.path)?;
                let meta = match (args.content, args.from_conversation) {
                    (Some(content), None) => {
                        let tracked = changes::track(store, &path).await?;
                        let meta = store.write(&path, &content).await?;
                        changes::save(store, tracked).await?;
                        meta
                    }
                    (None, Some(handle)) => {
                        let bytes = {
                            let session = ctx.session.read().await;
                            let url = conversation::find(
                                &session.messages,
                                &session.produced_files,
                                handle.trim(),
                            )
                            .ok_or_else(|| {
                                FormaError::Input(format!(
                                    "there is no file '{handle}' in this conversation; use \
                                     a handle shown as [file-…] next to a file"
                                ))
                            })?;
                            conversation::decode(url)?
                        };
                        let media_type = savable_type(&path, &bytes)?;
                        let tracked = changes::track(store, &path).await?;
                        let meta = store.write_bytes(&path, bytes, &media_type).await?;
                        changes::save(store, tracked).await?;
                        meta
                    }
                    _ => {
                        return Err(FormaError::Input(
                            "give exactly one of `content` or `from_conversation`".to_string(),
                        ));
                    }
                };
                Ok(Some(serde_json::to_value(meta)?))
            }
            // Each change is checked before it is tracked, so one that is
            // refused leaves no copy behind.
            KnowledgeBaseOperation::Append => {
                let args: AppendInput = input_for("append", input)?;
                let path = change_path(&args.path)?;
                let (text, media_type) = match store.stat(&path).await? {
                    None => (String::new(), None),
                    Some(_) => {
                        let (mut text, media_type) = text_document(store, &path).await?;
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        (text, Some(media_type))
                    }
                };
                let tracked = changes::track(store, &path).await?;
                let updated = text + &args.content;
                let meta = match media_type {
                    Some(media_type) => {
                        store
                            .write_bytes(&path, updated.into_bytes(), &media_type)
                            .await?
                    }
                    None => store.write(&path, &updated).await?,
                };
                changes::save(store, tracked).await?;
                Ok(Some(serde_json::to_value(meta)?))
            }
            KnowledgeBaseOperation::Edit => {
                let args: EditInput = input_for("edit", input)?;
                let path = change_path(&args.path)?;
                if args.old.is_empty() {
                    return Err(FormaError::Input(
                        "`old` cannot be empty; to add text, use the append tool".to_string(),
                    ));
                }
                let (text, media_type) = text_document(store, &path).await?;
                match text.matches(&args.old).count() {
                    1 => {}
                    0 => {
                        return Err(FormaError::Input(format!(
                            "`old` was not found in '{path}'; read it again and copy the \
                             passage exactly"
                        )));
                    }
                    n => {
                        return Err(FormaError::Input(format!(
                            "`old` appears {n} times in '{path}'; include more of the \
                             surrounding text so that it appears once"
                        )));
                    }
                }
                let updated = text.replacen(&args.old, &args.new, 1);
                let tracked = changes::track(store, &path).await?;
                let meta = store
                    .write_bytes(&path, updated.into_bytes(), &media_type)
                    .await?;
                changes::save(store, tracked).await?;
                Ok(Some(serde_json::to_value(meta)?))
            }
            KnowledgeBaseOperation::Move => {
                let args: MoveInput = input_for("move", input)?;
                let from = change_path(&args.from)?;
                let to = change_path(&args.to)?;
                if from == to {
                    return Err(FormaError::Input(
                        "`from` and `to` are the same".to_string(),
                    ));
                }
                if store.stat(&from).await?.is_none() {
                    return Err(FormaError::Input(format!(
                        "there is no document at '{from}'"
                    )));
                }
                if store.stat(&to).await?.is_some() {
                    return Err(FormaError::Input(format!(
                        "there is already a document at '{to}'"
                    )));
                }
                let doc = store.attach(&from).await?;
                let mut tracked = changes::track(store, &from).await?;
                store.write_bytes(&to, doc.bytes, &doc.media_type).await?;
                store.delete(&from).await?;
                tracked.current = Some(to.clone());
                changes::save(store, tracked).await?;
                Ok(Some(json!({ "moved": { "from": from, "to": to } })))
            }
            KnowledgeBaseOperation::Delete => {
                let args: DeleteInput = input_for("delete", input)?;
                let path = change_path(&args.path)?;
                if store.stat(&path).await?.is_none() {
                    return Err(FormaError::Input(format!(
                        "there is no document at '{path}'"
                    )));
                }
                let mut tracked = changes::track(store, &path).await?;
                store.delete(&path).await?;
                tracked.current = None;
                changes::save(store, tracked).await?;
                Ok(Some(json!({ "deleted": path })))
            }
        }
    }
}

/// What a file from the conversation is stored as, judged by its bytes — the
/// type the conversation declared for it is not trusted — or why it cannot be
/// saved. Held to what a workspace takes from an upload: something it can
/// recognise, within the size a model could be given back.
fn savable_type(path: &str, bytes: &[u8]) -> Result<String, FormaError> {
    if bytes.is_empty() {
        return Err(FormaError::Input("that file is empty".to_string()));
    }
    if bytes.len() > attach::MAX_ATTACHMENT_BYTES {
        return Err(FormaError::Input(format!(
            "that file is too large to save; the limit is {} MiB",
            attach::MAX_ATTACHMENT_BYTES / (1024 * 1024)
        )));
    }
    let head = &bytes[..bytes.len().min(512)];
    let media_type = local::media_type_of(path, head);
    if media_type == "application/octet-stream" {
        tracing::warn!("'{path}': refused to save an unrecognised file");
        return Err(FormaError::Input(
            "that file is not one a workspace can hold (a picture, a PDF or text)".to_string(),
        ));
    }
    Ok(media_type)
}

/// What reading a document gives the model.
#[derive(Debug, PartialEq)]
enum Readable {
    /// Its text, whole; the caller slices it.
    Text(String),
    /// The file itself, attached to the conversation.
    Attach,
}

/// Decides how a document reaches the model, or says why it cannot.
///
/// A picture or a PDF is attached: decoded as UTF-8 it would be a page of
/// replacement characters, which a model cannot tell from a document that
/// really is written that way. Text is read. Anything else — a spreadsheet, a
/// Word document, an archive — is neither, and the refusal says so, so the
/// model can tell the user rather than guess at the contents.
fn readable(doc: &AttachedDoc) -> Result<Readable, FormaError> {
    if let Some(refusal) = office_refusal(&doc.path, &doc.media_type) {
        return Err(refusal);
    }
    if attach::is_attachable(&doc.media_type) {
        attach::check_attachable(doc)?;
        return Ok(Readable::Attach);
    }
    match std::str::from_utf8(&doc.bytes) {
        Ok(text) => Ok(Readable::Text(text.to_string())),
        Err(_) => {
            let kind = doc.media_type.split(';').next().unwrap_or("").trim();
            tracing::warn!("'{}' ({kind}) can neither be read nor attached", doc.path);
            Err(FormaError::Input(format!(
                "'{}' is {kind}, which can neither be read as text nor attached, so its \
                 contents cannot be shown to you. Tell the user; if you have a \
                 code-execution tool, you may be able to open it there by passing it in \
                 `files` with `from_workspace`.",
                doc.path
            )))
        }
    }
}

/// What an Excel workbook is stored under.
pub(crate) const XLSX_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// What a PowerPoint deck is stored under.
pub(crate) const PPTX_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";

/// Says why a spreadsheet or a deck cannot be read or attached, or `None`
/// when `path` is neither. Models take neither as input, but a sandbox opens
/// them (pandas or openpyxl, python-pptx), so the refusal names that way
/// instead.
pub(crate) fn office_refusal(path: &str, media_type: &str) -> Option<FormaError> {
    let kind = media_type.split(';').next().unwrap_or("").trim();
    let lower = path.to_ascii_lowercase();
    let is = |media: &str, exts: &[&str]| kind == media || exts.iter().any(|e| lower.ends_with(e));
    let (what, library) = if is(XLSX_TYPE, &[".xlsx", ".xlsm", ".xls"]) {
        ("an Excel workbook", "pandas or openpyxl")
    } else if is(PPTX_TYPE, &[".pptx", ".pptm", ".ppt"]) {
        ("a PowerPoint deck", "python-pptx")
    } else {
        return None;
    };
    Some(FormaError::Input(format!(
        "'{path}' is {what}, which cannot be read as text or attached. Open it with a \
         code-execution tool instead, if you have one: pass it in `files` with \
         `from_workspace` and read it with {library}."
    )))
}

#[cfg(test)]
mod tests {
    /// Issue #257: `"limit": 6000.0` was refused as "expected usize".
    #[test]
    fn read_accepts_whole_numbers_sent_as_floats() {
        let args: super::ReadInput = super::input_for(
            "read",
            serde_json::json!({ "path": "a.md", "offset": 0.0, "limit": 6000.0 }),
        )
        .unwrap();
        assert_eq!((args.offset(), args.limit()), (0, 6000));
    }

    #[test]
    fn read_rounds_offset_down_and_limit_up() {
        let args: super::ReadInput = super::input_for(
            "read",
            serde_json::json!({ "path": "a.md", "offset": 10.7, "limit": 99.2 }),
        )
        .unwrap();
        assert_eq!((args.offset(), args.limit()), (10, 100));

        let args: super::ReadInput =
            super::input_for("read", serde_json::json!({ "path": "a.md", "offset": -5 })).unwrap();
        assert_eq!(
            (args.offset(), args.limit()),
            (0, super::DEFAULT_READ_LIMIT)
        );
    }

    #[test]
    fn test_spreadsheets_are_sent_to_code_execution() {
        let refusal = super::office_refusal("ventas.xlsx", "application/zip").unwrap();
        assert!(refusal.to_string().contains("from_workspace"));
        assert!(super::office_refusal("data", super::XLSX_TYPE).is_some());
        assert!(super::office_refusal("old.XLS", "application/octet-stream").is_some());
        assert!(super::office_refusal("notes.md", "text/markdown").is_none());
        // Reading one says so, rather than failing on the bytes.
        let err =
            super::readable(&doc("ventas.xlsx", super::XLSX_TYPE, b"PK\x03\x04")).unwrap_err();
        assert!(err.to_string().contains("Excel workbook"));
    }

    #[test]
    fn test_decks_are_sent_to_code_execution() {
        let refusal = super::office_refusal("resumen.pptx", "application/zip").unwrap();
        assert!(refusal.to_string().contains("python-pptx"));
        assert!(super::office_refusal("data", super::PPTX_TYPE).is_some());
        let err =
            super::readable(&doc("resumen.pptx", super::PPTX_TYPE, b"PK\x03\x04")).unwrap_err();
        assert!(err.to_string().contains("PowerPoint deck"));
    }

    fn doc(path: &str, media_type: &str, bytes: &[u8]) -> protocols::AttachedDoc {
        protocols::AttachedDoc {
            path: path.to_string(),
            media_type: media_type.to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn test_a_conversation_file_is_saved_as_what_its_bytes_are() {
        use super::savable_type;
        // Judged by the bytes, whatever the name or the declared type said.
        assert_eq!(
            savable_type("logo.png", b"\x89PNG\r\n\x1a\nrest").unwrap(),
            "image/png"
        );
        assert_eq!(savable_type("doc", b"%PDF-1.7").unwrap(), "application/pdf");
        assert!(
            savable_type("notes.md", b"# Notes")
                .unwrap()
                .starts_with("text/markdown")
        );
        // Bytes it cannot recognise, or nothing at all, are refused.
        assert!(savable_type("x.png", b"\x00\x01\x02\xff").is_err());
        assert!(savable_type("x.png", b"").is_err());
        let huge = vec![b'a'; super::attach::MAX_ATTACHMENT_BYTES + 1];
        assert!(savable_type("big.txt", &huge).is_err());
    }

    #[test]
    fn test_write_takes_text_or_a_conversation_file() {
        let schema: JsonSchema = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::Write,
            scope: WorkspaceScope::Shared,
        }
        .try_into()
        .unwrap();
        assert_eq!(schema.required, Some(vec!["path".to_string()]));
        assert!(schema.properties.unwrap().contains_key("from_conversation"));
    }

    #[test]
    fn test_read_decides_between_text_and_attachment() {
        use super::{Readable, readable};
        // Text is read, whatever the store calls it.
        assert_eq!(
            readable(&doc("notes.md", "text/markdown", b"# Hi")).unwrap(),
            Readable::Text("# Hi".to_string())
        );
        assert_eq!(
            readable(&doc("data.csv", "application/octet-stream", b"a,b")).unwrap(),
            Readable::Text("a,b".to_string())
        );
        // Pictures and PDFs are attached.
        assert_eq!(
            readable(&doc("chart.png", "image/png", b"\x89PNG")).unwrap(),
            Readable::Attach
        );
        assert_eq!(
            readable(&doc("report.pdf", "application/pdf", b"%PDF-1.7")).unwrap(),
            Readable::Attach
        );
        // An empty picture cannot be sent.
        assert!(readable(&doc("chart.png", "image/png", b"")).is_err());
        // Binary that is neither is refused, saying so.
        let err =
            readable(&doc("memo.docx", "application/msword", b"\xd0\xcf\x11\xe0")).unwrap_err();
        assert!(
            err.to_string()
                .contains("neither be read as text nor attached")
        );
    }

    use super::*;
    use crate::tool::workspace::gcs::GcsDocConfig;
    use db::NoneDB;

    fn gcs_backend() -> DocStoreBackend {
        DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
            bucket: "my-bucket".to_string(),
            workspace_id: "".to_string(),
        })
    }

    #[test]
    fn test_backend_serde_kebab_tags() {
        let backend = gcs_backend();
        let json = serde_json::to_value(&backend).unwrap();
        assert_eq!(json["backend"], "google-knowledgebase");
        let parsed: DocStoreBackend = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, DocStoreBackend::GoogleKnowledgebase(_)));
    }

    #[test]
    fn test_check_valid_gcs_backend() {
        let tool = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::List,
            scope: WorkspaceScope::Shared,
        };
        assert!(<KnowledgebaseTool as ToolTrait<NoneDB>>::check(&tool).is_ok());
    }

    #[test]
    fn test_schema_per_op() {
        // list: no required fields
        let schema: JsonSchema = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::List,
            scope: WorkspaceScope::Shared,
        }
        .try_into()
        .unwrap();
        assert_eq!(schema.required, Some(vec![]));
        assert!(!schema.properties.unwrap().contains_key("prefix"));

        // read: requires `path`
        let schema: JsonSchema = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::Read,
            scope: WorkspaceScope::Shared,
        }
        .try_into()
        .unwrap();
        assert_eq!(schema.required, Some(vec!["path".to_string()]));
        let props = schema.properties.unwrap();
        assert!(props.contains_key("offset"));
        assert!(props.contains_key("limit"));

        // write: requires `path`
        let schema: JsonSchema = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::Write,
            scope: WorkspaceScope::Shared,
        }
        .try_into()
        .unwrap();
        let required = schema.required.unwrap();
        assert!(required.contains(&"path".to_string()));
        // `content` or `from_conversation`: neither is required on its own.
        assert!(!required.contains(&"content".to_string()));
    }

    #[test]
    fn test_op_suffixes() {
        assert_eq!(KnowledgeBaseOperation::List.prefix(), "list");
        assert_eq!(KnowledgeBaseOperation::Read.prefix(), "read-from");
        assert_eq!(KnowledgeBaseOperation::Write.prefix(), "write-to");
    }

    #[test]
    fn test_op_scopes_gate_write_behind_write_credential() {
        // Read-family operations are reachable with a read credential; write is
        // not — it needs a write-scoped credential for the same path.
        let read_cred = KnowledgebaseCredentials {
            path: "my-bucket".to_string(),
            scopes: vec![Scope::Read],
        };
        assert!(read_cred.can_access("my-bucket", KnowledgeBaseOperation::Read));
        assert!(read_cred.can_access("my-bucket", KnowledgeBaseOperation::List));
        assert!(!read_cred.can_access("my-bucket", KnowledgeBaseOperation::Write));

        let write_cred = KnowledgebaseCredentials {
            path: "my-bucket".to_string(),
            scopes: vec![Scope::Write],
        };
        assert!(write_cred.can_access("my-bucket", KnowledgeBaseOperation::Write));
        assert!(!write_cred.can_access("my-bucket", KnowledgeBaseOperation::Read));
    }

    // --- scrapbooks ---------------------------------------------------------

    /// A context for `user`/`agent`, optionally inside a deployment.
    fn ctx_for(user: &str, agent: &str, deployment: Option<&str>) -> CallContext<NoneDB> {
        use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
        use std::sync::Arc;
        use tokio::sync::{Mutex, RwLock};

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Deliberately leaked so the receiver never closes mid-test.
        std::mem::forget(_rx);
        CallContext {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "job".into(),
            user_roles: Arc::new(vec![]),
            user_id: user.into(),
            agent_id: agent.into(),
            deployment_id: deployment.map(Arc::from),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            db: Arc::new(NoneDB::default()),
        }
    }

    fn scrapbook_tool(op: KnowledgeBaseOperation) -> KnowledgebaseTool {
        KnowledgebaseTool {
            backend: gcs_backend(),
            op,
            scope: WorkspaceScope::Scrapbook,
        }
    }

    /// The prefix of the scrapbook `tool` would use for this caller.
    fn resolved_prefix(tool: &KnowledgebaseTool, ctx: &CallContext<NoneDB>) -> String {
        match tool.resolve_backend(ctx).unwrap() {
            DocStoreBackend::GoogleKnowledgebase(c) => c.workspace_id,
        }
    }

    #[test]
    fn test_scrapbook_prefix_is_owner_then_user() {
        let tool = scrapbook_tool(KnowledgeBaseOperation::Read);

        // Single-tenant: one agent per process, so the agent names the owner.
        let base = resolved_prefix(&tool, &ctx_for("alice", "helper", None));
        assert_eq!(base, "scrapbooks/helper/alice");

        // Multi-tenant: the deployment names it. A deployment hosts exactly one
        // agent, so the agent ID adds no segment of its own.
        let deployed = resolved_prefix(&tool, &ctx_for("alice", "helper", Some("depl-1")));
        assert_eq!(deployed, "scrapbooks/depl-1/alice");

        // Both parts of the key move the prefix.
        assert_ne!(
            base,
            resolved_prefix(&tool, &ctx_for("bob", "helper", None))
        );
        assert_ne!(
            base,
            resolved_prefix(&tool, &ctx_for("alice", "coach", None))
        );

        // The same caller always lands on the same scrapbook, so what was
        // written in one conversation is found in the next.
        assert_eq!(
            base,
            resolved_prefix(&tool, &ctx_for("alice", "helper", None))
        );
    }

    #[test]
    fn test_scrapbook_prefers_the_deployment_id_over_the_agent_id() {
        // The two are the same string today, because multi-tenant overwrites
        // `agent.id` with the deployment ID it looked the agent up by. Keying
        // on the deployment ID regardless means that if the assignment were
        // ever dropped — leaving `agent_id` as the author's YAML name — two
        // tenants that both named their agent `assistant` still could not end
        // up sharing a scrapbook.
        let tool = scrapbook_tool(KnowledgeBaseOperation::Read);
        let acme = resolved_prefix(&tool, &ctx_for("123", "assistant", Some("acme")));
        let globex = resolved_prefix(&tool, &ctx_for("123", "assistant", Some("globex")));
        assert_ne!(acme, globex);
        assert_eq!(acme, "scrapbooks/acme/123");
    }

    #[test]
    fn test_scrapbook_requires_an_owner_and_a_user() {
        // Falling back to a placeholder for any of these would silently pool
        // callers into one shared scrapbook, so they are hard errors.
        let tool = scrapbook_tool(KnowledgeBaseOperation::Read);
        // No user.
        assert!(tool.resolve_backend(&ctx_for("", "helper", None)).is_err());
        assert!(
            tool.resolve_backend(&ctx_for("   ", "helper", None))
                .is_err()
        );
        // No owner: single-tenant falls back to the agent ID, which must exist.
        assert!(tool.resolve_backend(&ctx_for("alice", "", None)).is_err());
        // ... and an empty deployment ID is not a licence to skip the check.
        assert!(
            tool.resolve_backend(&ctx_for("alice", "helper", Some("")))
                .is_err()
        );
    }

    #[test]
    fn test_scrapbook_identity_cannot_escape_its_prefix() {
        // An ID containing separators must not be able to widen the prefix or
        // climb into a sibling's; it is escaped, not stripped, so that two
        // distinct IDs cannot collapse onto one scrapbook either.
        let tool = scrapbook_tool(KnowledgeBaseOperation::Write);
        let sneaky = resolved_prefix(&tool, &ctx_for("../../bob", "helper", None));
        // The separators are escaped, so the ID stays one segment and the
        // prefix keeps its exact depth instead of climbing out.
        assert!(sneaky.starts_with("scrapbooks/"));
        assert_eq!(sneaky.matches('/').count(), 2);
        assert_eq!(sneaky, "scrapbooks/helper/..%2F..%2Fbob");

        // Escaping rather than stripping keeps distinct identities distinct: a
        // user who could collide with another by having their separators
        // dropped would land in that other user's scrapbook.
        assert_ne!(
            resolved_prefix(&tool, &ctx_for("a/b", "helper", None)),
            resolved_prefix(&tool, &ctx_for("b", "helper/a", None)),
        );

        // Whole-segment dots are refused outright.
        assert!(
            tool.resolve_backend(&ctx_for("..", "helper", None))
                .is_err()
        );
        assert!(tool.resolve_backend(&ctx_for(".", "helper", None)).is_err());
    }

    #[test]
    fn test_scrapbook_needs_no_deployment_credential() {
        // `ctx_for` carries empty deployment credentials — a shared workspace
        // would be refused, while a scrapbook resolves, including for writes.
        let ctx = ctx_for("alice", "helper", None);
        for op in [
            KnowledgeBaseOperation::List,
            KnowledgeBaseOperation::Read,
            KnowledgeBaseOperation::Write,
        ] {
            assert!(scrapbook_tool(op).resolve_backend(&ctx).is_ok());

            let shared = KnowledgebaseTool {
                backend: gcs_backend(),
                op,
                scope: WorkspaceScope::Shared,
            };
            assert!(shared.resolve_backend(&ctx).is_err());
        }
    }

    #[test]
    fn test_scrapbook_keeps_the_configured_bucket() {
        // Only the prefix is derived; the bucket stays as configured.
        let ctx = ctx_for("alice", "helper", None);
        match scrapbook_tool(KnowledgeBaseOperation::Read)
            .resolve_backend(&ctx)
            .unwrap()
        {
            DocStoreBackend::GoogleKnowledgebase(c) => assert_eq!(c.bucket, "my-bucket"),
        }
    }

    #[test]
    fn test_runs_are_scoped_to_the_caller_and_read_only() {
        let tool = |op| KnowledgebaseTool {
            backend: gcs_backend(),
            op,
            scope: WorkspaceScope::Runs,
        };
        let prefix = |ctx: &CallContext<NoneDB>| match tool(KnowledgeBaseOperation::Read)
            .resolve_backend(ctx)
            .unwrap()
        {
            DocStoreBackend::GoogleKnowledgebase(c) => c.workspace_id,
        };
        // Derived like a scrapbook, under its own root, with no grant needed.
        assert_eq!(
            prefix(&ctx_for("alice", "helper", Some("depl-1"))),
            "runs/depl-1/alice"
        );
        assert_ne!(
            prefix(&ctx_for("alice", "helper", Some("depl-1"))),
            prefix(&ctx_for("bob", "helper", Some("depl-1")))
        );
        assert!(
            tool(KnowledgeBaseOperation::Read)
                .resolve_backend(&ctx_for("", "helper", None))
                .is_err()
        );
        // Only a run writes its folder.
        assert!(
            tool(KnowledgeBaseOperation::Write)
                .resolve_backend(&ctx_for("alice", "helper", None))
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_without_a_store_a_workspace_loads_but_does_not_open() {
        // Nothing in these tests installs what a server gives its tools, but
        // the ignored end-to-end run of code execution.
        if host::installed().is_some() {
            return;
        }
        let backend = gcs_backend();
        assert!(backend.check().is_ok());
        let err = backend.connect().await.err().unwrap().to_string();
        assert!(err.contains("runtime.yaml"), "{err}");
    }

    #[tokio::test]
    async fn test_a_local_store_keeps_a_workspace_where_the_bucket_would() {
        // The same address — a caller's scrapbook — lands in the folder at the
        // path its objects would have in the bucket.
        let root = std::env::temp_dir().join(format!("forma-ws-{}", uuid::Uuid::new_v4().simple()));
        let ctx = ctx_for("alice", "helper", Some("depl-1"));
        let scrapbook = KnowledgebaseTool {
            backend: gcs_backend(),
            op: KnowledgeBaseOperation::Write,
            scope: WorkspaceScope::Scrapbook,
        };
        let store = scrapbook
            .resolve_backend(&ctx)
            .unwrap()
            .connect_to(StoreKind::Local(root.clone()))
            .unwrap();
        store.write("notes.md", "remember").await.unwrap();
        let file = root.join("scrapbooks/depl-1/alice/notes.md");
        let written = std::fs::read_to_string(&file);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(written.unwrap(), "remember");
    }

    // --- changing documents, and what waits for review ----------------------

    /// A shared workspace kept in a fresh folder, with a tool for each
    /// operation over it.
    struct Folder {
        root: PathBuf,
        backend: DocStoreBackend,
        store: Box<dyn DocStore>,
    }

    impl Folder {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("forma-ws-{}", uuid::Uuid::new_v4().simple()));
            let backend = DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                bucket: String::new(),
                workspace_id: "kb".to_string(),
            });
            let store = backend.connect_to(StoreKind::Local(root.clone())).unwrap();
            Self {
                root,
                backend,
                store,
            }
        }

        async fn call(
            &self,
            op: KnowledgeBaseOperation,
            input: Value,
        ) -> Result<Value, FormaError> {
            let tool = KnowledgebaseTool {
                backend: self.backend.clone(),
                op,
                scope: WorkspaceScope::Shared,
            };
            let ctx = ctx_for("alice", "helper", None);
            Ok(tool
                .run(&self.backend, self.store.as_ref(), input, &ctx)
                .await?
                .unwrap_or_default())
        }

        async fn text(&self, path: &str) -> String {
            String::from_utf8(self.store.attach(path).await.unwrap().bytes).unwrap()
        }

        async fn pending(&self) -> Vec<changes::Change> {
            changes::pending(self.store.as_ref()).await.unwrap()
        }

        async fn base(&self, change: &changes::Change) -> String {
            self.text(&format!(".history/base/{}", change.id)).await
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn test_a_file_changed_twice_is_reviewed_against_the_approved_version() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.store.write("plan.md", "v1").await.unwrap();

        ws.call(Write, json!({"path": "plan.md", "content": "v2"}))
            .await
            .unwrap();
        ws.call(Write, json!({"path": "plan.md", "content": "v3"}))
            .await
            .unwrap();

        let pending = ws.pending().await;
        assert_eq!(pending.len(), 1, "one file, one change");
        assert_eq!(pending[0].original.as_deref(), Some("plan.md"));
        assert_eq!(pending[0].current.as_deref(), Some("plan.md"));
        // Not the agent's own first draft.
        assert_eq!(ws.base(&pending[0]).await, "v1");
        assert_eq!(ws.text("plan.md").await, "v3");
    }

    #[tokio::test]
    async fn test_edit_replaces_one_exact_passage() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.store.write("a.md", "one two two").await.unwrap();

        let twice = ws
            .call(Edit, json!({"path": "a.md", "old": "two", "new": "2"}))
            .await;
        assert!(twice.unwrap_err().to_string().contains("2 times"));
        let missing = ws
            .call(Edit, json!({"path": "a.md", "old": "three", "new": "3"}))
            .await;
        assert!(missing.unwrap_err().to_string().contains("not found"));
        // Refusals leave nothing to review.
        assert!(ws.pending().await.is_empty());

        ws.call(
            Edit,
            json!({"path": "a.md", "old": "one two", "new": "1 2"}),
        )
        .await
        .unwrap();
        assert_eq!(ws.text("a.md").await, "1 2 two");
        assert_eq!(ws.pending().await.len(), 1);
    }

    #[tokio::test]
    async fn test_append_adds_a_line_or_creates_the_document() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.store.write("log.md", "- first").await.unwrap();
        ws.call(Append, json!({"path": "log.md", "content": "- second"}))
            .await
            .unwrap();
        assert_eq!(ws.text("log.md").await, "- first\n- second");

        ws.call(Append, json!({"path": "new.md", "content": "hello"}))
            .await
            .unwrap();
        assert_eq!(ws.text("new.md").await, "hello");
        let created = ws
            .pending()
            .await
            .into_iter()
            .find(|c| c.current.as_deref() == Some("new.md"))
            .unwrap();
        assert_eq!(created.original, None);
    }

    #[tokio::test]
    async fn test_move_follows_the_file_and_refuses_to_overwrite() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.store.write("a.md", "a").await.unwrap();
        ws.store.write("b.md", "b").await.unwrap();

        let onto = ws.call(Move, json!({"from": "a.md", "to": "b.md"})).await;
        assert!(onto.unwrap_err().to_string().contains("already"));

        ws.call(Move, json!({"from": "a.md", "to": "notes/a.md"}))
            .await
            .unwrap();
        ws.call(Edit, json!({"path": "notes/a.md", "old": "a", "new": "A"}))
            .await
            .unwrap();

        let pending = ws.pending().await;
        assert_eq!(pending.len(), 1, "moved and edited is one change");
        assert_eq!(pending[0].original.as_deref(), Some("a.md"));
        assert_eq!(pending[0].current.as_deref(), Some("notes/a.md"));
        assert_eq!(ws.base(&pending[0]).await, "a");
        assert!(ws.store.stat("a.md").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_deleting_what_the_agent_created_leaves_nothing_to_review() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.store.write("keep.md", "keep").await.unwrap();

        ws.call(Write, json!({"path": "tmp.md", "content": "x"}))
            .await
            .unwrap();
        ws.call(Delete, json!({"path": "tmp.md"})).await.unwrap();
        assert!(ws.pending().await.is_empty());

        ws.call(Delete, json!({"path": "keep.md"})).await.unwrap();
        let pending = ws.pending().await;
        assert_eq!(pending[0].current, None);
        assert_eq!(ws.base(&pending[0]).await, "keep");
    }

    #[tokio::test]
    async fn test_the_records_are_out_of_the_agents_reach() {
        use KnowledgeBaseOperation::*;
        let ws = Folder::new();
        ws.call(Write, json!({"path": "a.md", "content": "a"}))
            .await
            .unwrap();

        let listed = ws.call(List, json!({})).await.unwrap();
        let paths: Vec<&str> = listed["documents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["a.md"]);

        let record = format!(".history/changes/{}.json", ws.pending().await[0].id);
        assert!(ws.call(Read, json!({"path": record})).await.is_err());
        assert!(ws.call(Delete, json!({"path": record})).await.is_err());
        assert!(
            ws.call(Write, json!({"path": ".history/x", "content": "x"}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_a_change_left_unreviewed_is_approved_after_three_days() {
        let ws = Folder::new();
        ws.store.write("a.md", "a").await.unwrap();
        let mut change = changes::track(ws.store.as_ref(), "a.md").await.unwrap();
        changes::save(ws.store.as_ref(), change.clone())
            .await
            .unwrap();
        assert_eq!(ws.pending().await.len(), 1);

        // As if it had last been touched four days ago.
        change.last_at = chrono::Utc::now() - chrono::Duration::days(4);
        let json = serde_json::to_vec(&change).unwrap();
        ws.store
            .write_bytes(
                &format!(".history/changes/{}.json", change.id),
                json,
                "application/json",
            )
            .await
            .unwrap();

        assert!(ws.pending().await.is_empty());
        // Its copy goes with it.
        let left = ws.store.list(Some(".history/")).await.unwrap();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn test_a_workspace_without_a_bucket_is_named_by_its_id() {
        // What a grant names: unchanged in Cloud Storage, and bucket-free in a
        // folder, where no bucket is configured.
        let named = |bucket: &str, id: &str| {
            GcsDocConfig {
                bucket: bucket.to_string(),
                workspace_id: id.to_string(),
            }
            .knowledgebase_id()
        };
        assert_eq!(named("b", "kb"), "b/kb");
        assert_eq!(named("b", ""), "b");
        assert_eq!(named("", "kb"), "kb");
    }

    #[test]
    fn test_each_tool_says_which_grant_it_needs() {
        let tool = |op, scope| KnowledgebaseTool {
            backend: DocStoreBackend::GoogleKnowledgebase(GcsDocConfig {
                bucket: "bucket".to_string(),
                workspace_id: "kb-1".to_string(),
            }),
            op,
            scope,
        };
        use KnowledgeBaseOperation::*;
        for op in [List, Read] {
            assert_eq!(
                tool(op, WorkspaceScope::Shared).required_grant(),
                Some(("bucket/kb-1".to_string(), "read"))
            );
        }
        assert_eq!(
            tool(Write, WorkspaceScope::Shared).required_grant(),
            Some(("bucket/kb-1".to_string(), "write"))
        );
        // Derived from the caller: nothing to grant.
        assert_eq!(
            tool(Write, WorkspaceScope::Scrapbook).required_grant(),
            None
        );
        assert_eq!(tool(Read, WorkspaceScope::Runs).required_grant(), None);
    }

    #[test]
    fn test_tool_without_scope_deserializes_as_shared() {
        // Tools serialized before scrapbooks existed must keep their permission
        // checks rather than silently becoming unpermissioned scrapbooks.
        let tool: KnowledgebaseTool = serde_json::from_value(json!({
            "backend": { "backend": "google-knowledgebase", "bucket": "b", "workspace_id": "kb" },
            "op": "read",
        }))
        .unwrap();
        assert_eq!(tool.scope, WorkspaceScope::Shared);
    }
}
