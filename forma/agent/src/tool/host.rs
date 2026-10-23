//! What the server running an agent gives its tools: where workspaces are
//! kept, and where code runs.
//!
//! Both are chosen by whoever runs the server, in its `runtime.yaml`, and are
//! the same for every agent on it: a definition says *which* workspace a tool
//! reads, or that it runs code, never where. A server installs its choice once,
//! when it starts and before it loads an agent ([`install`]), and tools read it
//! when they run. Where nothing was installed (e.g., `forma-manager`, which
//! compiles agents but runs none) nothing is available, and the checks that
//! depend on it are left to the server that runs the agent.

use std::sync::OnceLock;

use protocols::FormaError;

use crate::tool::{code_execution::sandbox::SandboxKind, workspace::StoreKind};

/// What a server gives its tools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    /// Where workspaces are kept; `None` if this server keeps none.
    pub workspaces: Option<StoreKind>,
    /// Where code runs; `None` if this server runs none.
    pub sandbox: Option<SandboxKind>,
}

impl Host {
    /// Refuses what cannot work, so that a server fails when it starts rather
    /// than when an agent first needs it.
    pub fn check(&self) -> Result<(), FormaError> {
        let Some(sandbox) = &self.sandbox else {
            return Ok(());
        };
        sandbox.check()?;
        // A run reaches its files only through signed URLs.
        match &self.workspaces {
            Some(StoreKind::Gcs) => Ok(()),
            Some(StoreKind::Local(_)) => Err(FormaError::Input(
                "code execution needs workspaces in Cloud Storage: a local folder cannot sign \
                 the URLs a run reads and writes its files through"
                    .to_string(),
            )),
            None => Err(FormaError::Input(
                "code execution needs workspaces, where a run's files are kept".to_string(),
            )),
        }
    }
}

static HOST: OnceLock<Host> = OnceLock::new();

/// Installs what this server gives its tools. Once per process: a server has
/// one set of choices.
pub fn install(host: Host) -> Result<(), FormaError> {
    host.check()?;
    HOST.set(host).map_err(|_| {
        FormaError::Execution("this server's workspaces and sandbox are already set".to_string())
    })
}

/// What this server installed, if anything.
pub fn installed() -> Option<&'static Host> {
    HOST.get()
}

/// Where this server keeps workspaces.
pub fn workspaces() -> Result<&'static StoreKind, FormaError> {
    installed()
        .and_then(|h| h.workspaces.as_ref())
        .ok_or_else(|| {
            FormaError::Input(
                "workspaces are not available here: the server's runtime.yaml sets no `workspaces`"
                    .to_string(),
            )
        })
}

/// Where this server runs code.
pub fn sandbox() -> Result<&'static SandboxKind, FormaError> {
    installed().and_then(|h| h.sandbox.as_ref()).ok_or_else(|| {
        FormaError::Input(
            "code execution is not available here: the server's runtime.yaml sets no \
             `code_execution`"
                .to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_code_runs_only_beside_signing_workspaces() {
        let local = |dir: &str| StoreKind::Local(PathBuf::from(dir));
        let docker = SandboxKind::LocalDocker {
            image: "forma-sandbox:dev".to_string(),
        };

        // Nothing, or workspaces alone, are fine.
        assert!(Host::default().check().is_ok());
        let host = Host {
            workspaces: Some(local("/data/ws")),
            sandbox: None,
        };
        assert!(host.check().is_ok());

        // A run needs workspaces that can sign URLs.
        let host = Host {
            workspaces: Some(StoreKind::Gcs),
            sandbox: Some(docker.clone()),
        };
        assert!(host.check().is_ok());
        let host = Host {
            workspaces: Some(local("/data/ws")),
            sandbox: Some(docker.clone()),
        };
        assert!(host.check().unwrap_err().to_string().contains("sign"));
        let host = Host {
            workspaces: None,
            sandbox: Some(docker),
        };
        assert!(host.check().is_err());

        // And an image that cannot be mistaken for an option.
        let host = Host {
            workspaces: Some(StoreKind::Gcs),
            sandbox: Some(SandboxKind::LocalDocker {
                image: "--privileged".to_string(),
            }),
        };
        assert!(host.check().is_err());
    }
}
