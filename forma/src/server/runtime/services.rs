use agent::tool::{
    code_execution::sandbox::{DEFAULT_LOCAL_IMAGE, SandboxKind},
    workspace::StoreKind,
};
use db::FormaDBClient;
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

/// Where a server keeps conversations: their messages, the sessions they
/// belong to, the users' tokens and, in a multi-tenant server, the deployed
/// agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(rename_all = "kebab-case")]
pub enum Storage {
    /// A MongoDB-compatible database, in a database called `forma-db`, which
    /// the `SESSIONS_DB_URL` environment variable connects to.
    ///
    /// What most servers want: Forma servers are stateless, so several copies
    /// of one can share it. A multi-tenant server reads its agents from it.
    Mongo,

    /// Files, in a `./forma-db` directory (relative to where the server
    /// starts).
    ///
    /// Fine for a single server with a disk that outlives it, and what the
    /// CLI uses while you develop.
    File,

    /// Nowhere: every message starts a new conversation, with no memory.
    ///
    /// For agents that answer in a single turn (e.g., one that classifies or
    /// summarises text). A multi-tenant server cannot use it, as it has nowhere
    /// to read its agents from.
    None,
}

impl Storage {
    /// Connects to this storage.
    pub async fn open(&self) -> Result<FormaDBClient, FormaError> {
        match self {
            Self::Mongo => FormaDBClient::new_mongo().await,
            Self::File => FormaDBClient::new_file().await,
            Self::None => FormaDBClient::new_none().await,
        }
    }
}

/// Where a server keeps workspaces: the folders of files that agents read and
/// write through their workspace tools (shared workspaces, scrapbooks, and the
/// files of code runs).
///
/// It is the same for every agent on the server: an agent's definition names
/// a workspace, never where it is kept, so the same agent works with either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(tag = "store", rename_all = "kebab-case")]
pub enum Workspaces {
    /// Google Cloud Storage.
    ///
    /// ```yaml
    /// workspaces:
    ///   store: gcs
    /// ```
    Gcs(GcsWorkspaces),

    /// A folder on this machine, which stands in for the bucket.
    ///
    /// ```yaml
    /// workspaces:
    ///   store: local
    ///   dir: ./workspaces
    /// ```
    Local(LocalWorkspaces),
}

/// Workspaces kept in Google Cloud Storage, reached with the server's Google
/// credentials. Each workspace is a folder in the bucket its definition names
/// or, if it names none, in the one the `WORKSPACES_BUCKET` environment
/// variable does.
///
/// There is nothing else to set. (A unit variant would quietly accept, and
/// ignore, a misplaced setting; an empty struct refuses it.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct GcsWorkspaces {}

/// Workspaces kept in a folder on this machine, which stands in for the
/// bucket.
///
/// It cannot sign URLs, which code execution needs: a server that runs code
/// keeps its workspaces in `gcs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct LocalWorkspaces {
    /// The folder workspaces are kept in, relative to where the server
    /// starts. A workspace's files are under `{dir}/{workspace}/`, at the
    /// paths their objects would have in the bucket.
    pub dir: String,
}

impl Workspaces {
    /// The store the agent's tools use.
    pub(crate) fn store(&self) -> Result<StoreKind, FormaError> {
        match self {
            Self::Gcs(_) => Ok(StoreKind::Gcs),
            Self::Local(local) if local.dir.trim().is_empty() => Err(FormaError::Input(
                "workspaces: a `local` store needs a `dir` to keep workspaces in".to_string(),
            )),
            Self::Local(local) => Ok(StoreKind::Local(local.dir.trim().into())),
        }
    }
}

/// Where a server runs code, for agents that use the `code` tool.
///
/// It is the same for every agent on the server. A run reaches its files only
/// through signed URLs, so a server that runs code keeps its workspaces in
/// `gcs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(tag = "sandbox", rename_all = "kebab-case")]
pub enum Sandbox {
    /// An execution of a Cloud Run Job per run.
    ///
    /// ```yaml
    /// code_execution:
    ///   sandbox: cloud-run-job
    /// ```
    CloudRunJob(CloudRunJob),

    /// A container per run, in this machine's Docker.
    ///
    /// ```yaml
    /// code_execution:
    ///   sandbox: local-docker
    ///   image: forma-sandbox:dev
    /// ```
    LocalDocker(LocalDocker),
}

/// Code runs in executions of a Cloud Run Job, one per run, which the
/// `CODE_EXECUTION_JOB` environment variable names: its full name
/// (`projects/<project>/locations/<region>/jobs/<job>`), or just the job's, in
/// the project and the region (`GOOGLE_CLOUD_LOCATION`) the server runs in.
///
/// There is nothing else to set: the job names a resource that changes from
/// one environment to the next, so it is not written in this file. (A unit
/// variant would quietly accept, and ignore, a misplaced setting; an empty
/// struct refuses it.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct CloudRunJob {}

/// Code runs in containers on this machine's Docker, one per run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct LocalDocker {
    /// The image each run starts from. Defaults to `forma-sandbox:dev`, the
    /// image `docker build -t forma-sandbox:dev forma/sandbox` builds.
    #[serde(default = "default_local_image")]
    pub image: String,
}

fn default_local_image() -> String {
    DEFAULT_LOCAL_IMAGE.to_string()
}

impl Sandbox {
    /// The sandbox the agent's tools use.
    pub(crate) fn kind(&self) -> SandboxKind {
        match self {
            Self::CloudRunJob(_) => SandboxKind::CloudRunJob,
            Self::LocalDocker(docker) => SandboxKind::LocalDocker {
                image: docker.image.trim().to_string(),
            },
        }
    }
}
