//! Where a run's container is started.
//!
//! A [`Sandbox`] starts one fresh container for one run, hands it the signed
//! URL of the run's manifest, and waits for it to finish. Everything else —
//! inputs, outputs, logs — travels through signed URLs to the run's store, so a
//! backend knows nothing about files, and any backend works with any store.
//!
//! A server has one sandbox ([`SandboxKind`]), chosen by whoever runs it in its
//! `runtime.yaml` (see [`crate::tool::host`]), and every agent on that server
//! runs there. Runs are stateless, so sharing it is not a problem. An
//! installation that wants a different sandbox — in another cloud, say —
//! configures it there, for all of its agents at once; an agent's definition
//! has no say.

use std::time::Duration;

use async_trait::async_trait;
use protocols::{FormaError, envar::get_env_var};
use serde_json::{Value, json};
use tracing::warn;

use crate::tool::google_auth::{CLOUD_PLATFORM_SCOPE, auth_headers};
use crate::tool::resolve_project_id;

/// The Cloud Run Job to run: a full resource name
/// (`projects/p/locations/r/jobs/j`), or just its name, in which case the
/// project and `GOOGLE_CLOUD_LOCATION` fill in the rest. An environment
/// variable, not a setting, because it names a resource that differs from one
/// environment (project) to the next.
pub const JOB_ENV: &str = "CODE_EXECUTION_JOB";
/// The variable the container reads its manifest URL from.
pub const MANIFEST_ENV: &str = "CODE_EXEC_MANIFEST_URL";

/// The image `local-docker` runs when the server names none.
pub const DEFAULT_LOCAL_IMAGE: &str = "forma-sandbox:dev";
const POLL_EVERY: Duration = Duration::from_secs(2);

/// Which sandbox a server runs code in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxKind {
    /// An execution of the Cloud Run Job that [`JOB_ENV`] names, per run.
    CloudRunJob,
    /// A container of `image` in this machine's Docker, per run.
    LocalDocker { image: String },
}

impl SandboxKind {
    /// Refuses an image that is not one.
    ///
    /// Only whoever runs the server sets it, but it becomes an argument to
    /// `docker`, where a leading `-` would be read as an option: a mistake
    /// there should fail plainly, not run something else. The job is checked
    /// when it is read (see [`open`]).
    pub fn check(&self) -> Result<(), FormaError> {
        match self {
            Self::CloudRunJob => Ok(()),
            Self::LocalDocker { image } if is_image_name(image) => Ok(()),
            Self::LocalDocker { image } => Err(FormaError::Input(format!(
                "code execution: '{image}' is not a Docker image name"
            ))),
        }
    }
}

/// How a run's container ended, as far as the platform can tell.
#[derive(Debug, Clone, PartialEq)]
pub enum SandboxExit {
    /// The container finished on its own. Whether the *command* worked is in
    /// the run's `result.json`.
    Completed,
    /// The container did not finish properly: it ran out of memory or time,
    /// could not start, or the platform failed. `reason` is for operators.
    Failed { reason: String },
}

/// What a run's container did: how it ended, and for how long the platform
/// bills it.
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxRun {
    pub exit: SandboxExit,
    /// The time the platform charges for, when it says so (Cloud Run's
    /// `startTime` to `completionTime`). `None` when it does not, in which
    /// case the caller's own clock stands in.
    pub billed: Option<Duration>,
}

impl From<SandboxExit> for SandboxRun {
    fn from(exit: SandboxExit) -> Self {
        Self { exit, billed: None }
    }
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Starts the container for `run_id` and waits for it, at most `deadline`.
    async fn run(
        &self,
        run_id: &str,
        manifest_url: &str,
        task_timeout: Duration,
        deadline: Duration,
    ) -> Result<SandboxRun, FormaError>;

    /// Names the backend in logs.
    fn describe(&self) -> String;
}

/// Opens this server's sandbox, `kind`.
///
/// The job is checked even though only whoever runs the server sets it: its
/// name becomes a path on the Cloud Run API, called with the server's
/// credentials, and a mistake there should fail plainly, not call something
/// else.
pub fn open(kind: &SandboxKind) -> Result<Box<dyn Sandbox>, FormaError> {
    kind.check()?;
    match kind {
        SandboxKind::CloudRunJob => Ok(Box::new(CloudRunJob {
            job: job_from_env()?,
        })),
        SandboxKind::LocalDocker { image } => Ok(Box::new(LocalDocker {
            image: image.clone(),
        })),
    }
}

/// `projects/<project>/locations/<region>/jobs/<job>`, each segment made of the
/// characters Google Cloud allows in it (lower-case letters, digits, hyphens),
/// and nothing else — no `?`, no `/..`, no `:`.
fn is_job_name(job: &str) -> bool {
    let parts: Vec<&str> = job.split('/').collect();
    let segment = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    parts.len() == 6
        && parts[0] == "projects"
        && parts[2] == "locations"
        && parts[4] == "jobs"
        && segment(parts[1])
        && segment(parts[3])
        && segment(parts[5])
}

/// A Docker image reference, as `registry/name:tag` or `name@sha256:…`, that
/// cannot be mistaken for an option.
fn is_image_name(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 255
        && image.starts_with(|c: char| c.is_ascii_alphanumeric())
        && image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:@".contains(&b))
}

/// The runtime's job, from `CODE_EXECUTION_JOB`: a full name, or a bare one
/// completed with the project and `GOOGLE_CLOUD_LOCATION`.
fn job_from_env() -> Result<String, FormaError> {
    let configured = get_env_var(JOB_ENV)?
        .filter(|j| !j.trim().is_empty())
        .ok_or_else(|| {
            FormaError::Input(format!(
                "code execution is not available here: {JOB_ENV} is not set"
            ))
        })?;
    let configured = configured.trim();
    let job = if configured.starts_with("projects/") {
        configured.to_string()
    } else {
        let project = resolve_project_id("code execution", None)?;
        let region = get_env_var("GOOGLE_CLOUD_LOCATION")?
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| {
                FormaError::Input(format!(
                    "code execution: {JOB_ENV} is a bare name, so GOOGLE_CLOUD_LOCATION must say its region"
                ))
            })?;
        format!(
            "projects/{project}/locations/{}/jobs/{configured}",
            region.trim()
        )
    };
    if !is_job_name(&job) {
        return Err(FormaError::Input(format!(
            "code execution: '{job}' is not a Cloud Run job name \
             (projects/<project>/locations/<region>/jobs/<job>)"
        )));
    }
    Ok(job)
}

// ---- Cloud Run Jobs -----------------------------------------------------------

/// One execution of a Cloud Run Job per run.
///
/// The job itself (image, service account with no roles, network without
/// internet) is defined in infra/code-execution.tf; a run only overrides the
/// manifest URL and the timeout.
#[derive(Debug, Clone)]
pub struct CloudRunJob {
    job: String,
}

/// A failed call to the Cloud Run API, keeping the HTTP status when there was
/// one so a caller can tell a refusal from a hiccup.
#[derive(Debug)]
struct ApiError {
    status: Option<reqwest::StatusCode>,
    error: FormaError,
}

impl ApiError {
    /// A refusal that asking again will not change: not signed in, not
    /// allowed, or no such execution. Anything else may pass.
    fn is_permanent(&self) -> bool {
        matches!(
            self.status.map(|s| s.as_u16()),
            Some(401) | Some(403) | Some(404)
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl From<FormaError> for ApiError {
    fn from(error: FormaError) -> Self {
        Self {
            status: None,
            error,
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        FormaError::from(e).into()
    }
}

impl From<ApiError> for FormaError {
    fn from(e: ApiError) -> Self {
        e.error
    }
}

impl CloudRunJob {
    async fn api(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        let headers = auth_headers(&[CLOUD_PLATFORM_SCOPE]).await?;
        let mut req = crate::tool::http::client()
            .request(method, url)
            .headers(headers)
            .timeout(Duration::from_secs(30));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError {
                status: Some(status),
                error: FormaError::ApiCall {
                    url: Some(url.to_string()),
                    msg: format!("HTTP {status}: {body}"),
                },
            });
        }
        Ok(resp.json().await.unwrap_or(Value::Null))
    }
}

#[async_trait]
impl Sandbox for CloudRunJob {
    async fn run(
        &self,
        run_id: &str,
        manifest_url: &str,
        task_timeout: Duration,
        deadline: Duration,
    ) -> Result<SandboxRun, FormaError> {
        let started = tokio::time::Instant::now();
        let operation = self
            .api(
                reqwest::Method::POST,
                &format!("https://run.googleapis.com/v2/{}:run", self.job),
                Some(json!({
                    "overrides": {
                        "containerOverrides": [{
                            "env": [{ "name": MANIFEST_ENV, "value": manifest_url }]
                        }],
                        "taskCount": 1,
                        "timeout": format!("{}s", task_timeout.as_secs()),
                    }
                })),
            )
            .await?;
        // The operation's metadata is the execution it started.
        let Some(execution) = operation
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(SandboxExit::Failed {
                reason: format!("run {run_id}: the job started no execution we can follow"),
            }
            .into());
        };

        loop {
            tokio::time::sleep(POLL_EVERY).await;
            let state = match self
                .api(
                    reqwest::Method::GET,
                    &format!("https://run.googleapis.com/v2/{execution}"),
                    None,
                )
                .await
            {
                Ok(state) => state,
                // A refusal will not pass by waiting: polling on would hold the
                // turn until the deadline, and the request long before that.
                Err(e) if e.is_permanent() => {
                    return Ok(SandboxExit::Failed {
                        reason: format!("cannot follow execution {execution}: {}", e.error),
                    }
                    .into());
                }
                // One failed poll is not a failed run.
                Err(e) => {
                    warn!(run_id, execution = %execution, error = %e, "code execution: poll failed");
                    Value::Null
                }
            };
            if let Some(exit) = execution_exit(&state) {
                return Ok(SandboxRun {
                    exit,
                    billed: execution_duration(&state),
                });
            }
            if started.elapsed() > deadline {
                let cancelled = self
                    .api(
                        reqwest::Method::POST,
                        &format!("https://run.googleapis.com/v2/{execution}:cancel"),
                        Some(json!({})),
                    )
                    .await;
                if let Err(e) = cancelled {
                    warn!(run_id, execution = %execution, error = %e, "code execution: cancel failed");
                }
                return Ok(SandboxExit::Failed {
                    reason: format!(
                        "execution {execution} did not finish within {}s and was cancelled",
                        deadline.as_secs()
                    ),
                }
                .into());
            }
        }
    }

    fn describe(&self) -> String {
        format!("cloud-run-job {}", self.job)
    }
}

/// How long a finished Cloud Run execution ran, from its own timestamps:
/// what the platform bills, startup included.
fn execution_duration(execution: &Value) -> Option<Duration> {
    let time = |field: &str| {
        execution
            .get(field)
            .and_then(Value::as_str)
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    (time("completionTime")? - time("startTime")?).to_std().ok()
}

/// Reads a Cloud Run execution: `None` while it is still running.
fn execution_exit(execution: &Value) -> Option<SandboxExit> {
    execution.get("completionTime")?;
    let count = |field: &str| execution.get(field).and_then(Value::as_i64).unwrap_or(0);
    if count("succeededCount") >= 1 {
        return Some(SandboxExit::Completed);
    }
    let conditions = execution
        .get("conditions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let reason = conditions
        .iter()
        .filter(|c| c.get("state").and_then(Value::as_str) == Some("CONDITION_FAILED"))
        .filter_map(|c| {
            let kind = c.get("type").and_then(Value::as_str).unwrap_or("?");
            let message = c.get("message").and_then(Value::as_str)?;
            Some(format!("{kind}: {message}"))
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(SandboxExit::Failed {
        reason: format!(
            "{} (failed: {}, cancelled: {})",
            if reason.is_empty() {
                "the execution failed"
            } else {
                &reason
            },
            count("failedCount"),
            count("cancelledCount"),
        ),
    })
}

// ---- local Docker -------------------------------------------------------------

/// `docker run` on the machine the runtime runs on, for development.
///
/// Not a production backend: the container needs the network to reach the
/// store, so it is not cut off from the internet the way the Cloud Run Job is.
/// Everything else is kept as close as Docker allows: a fresh container per
/// run, no capabilities beyond what the entrypoint needs to drop privileges,
/// and limits on memory, CPU and processes.
#[derive(Debug, Clone)]
pub struct LocalDocker {
    image: String,
}

#[async_trait]
impl Sandbox for LocalDocker {
    async fn run(
        &self,
        run_id: &str,
        manifest_url: &str,
        _task_timeout: Duration,
        deadline: Duration,
    ) -> Result<SandboxRun, FormaError> {
        let name = format!("forma-sandbox-{run_id}");
        let mut cmd = tokio::process::Command::new("docker");
        cmd.args(["run", "--rm", "--pull=never", "--name", &name])
            // The value comes from this process's environment, so the signed
            // URL never shows up in `ps`.
            .args(["-e", MANIFEST_ENV])
            .env(MANIFEST_ENV, manifest_url)
            .args(["--memory", "2g", "--cpus", "1", "--pids-limit", "512"])
            .args(["--cap-drop", "ALL"])
            .args([
                "--cap-add",
                "CHOWN",
                "--cap-add",
                "FOWNER",
                "--cap-add",
                "DAC_OVERRIDE",
            ])
            .args([
                "--cap-add",
                "SETUID",
                "--cap-add",
                "SETGID",
                "--cap-add",
                "KILL",
            ])
            .args(["--security-opt", "no-new-privileges"])
            .arg(&self.image)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| {
            FormaError::Input(format!("code execution: could not start docker: {e}"))
        })?;
        // Docker says nothing about billing; the caller's clock is the measure.
        let exit = match tokio::time::timeout(deadline, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => Ok(SandboxExit::Completed),
            Ok(Ok(output)) => Ok(SandboxExit::Failed {
                // The entrypoint prints metadata only, never the command's
                // output, so its last lines are safe to report.
                reason: format!(
                    "{}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                        .lines()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
            }),
            Ok(Err(e)) => Ok(SandboxExit::Failed {
                reason: format!("docker failed: {e}"),
            }),
            Err(_) => {
                let _ = tokio::process::Command::new("docker")
                    .args(["kill", &name])
                    .output()
                    .await;
                Ok(SandboxExit::Failed {
                    reason: format!("container did not finish within {}s", deadline.as_secs()),
                })
            }
        };
        exit.map(SandboxRun::from)
    }

    fn describe(&self) -> String {
        format!("local-docker {}", self.image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_names_are_held_to_real_ones() {
        assert!(is_job_name(
            "projects/clonsult-dev/locations/southamerica-west1/jobs/code-execution"
        ));
        for bad in [
            "code-execution",
            "projects/p/locations/r/jobs/j/extra",
            "projects/p/locations/r/jobs/j:run",
            "projects/p/locations/r/jobs/j?x=1",
            "projects/../locations/r/jobs/j",
            "projects/P/locations/r/jobs/j",
            "projects/p/locations/r/jobs/",
            "projects/p/locations/r/services/j",
        ] {
            assert!(!is_job_name(bad), "{bad}");
        }
    }

    #[test]
    fn test_images_cannot_be_options() {
        assert!(is_image_name(DEFAULT_LOCAL_IMAGE));
        assert!(is_image_name(
            "us-docker.pkg.dev/p/repo/forma-sandbox@sha256:abc123"
        ));
        for bad in [
            "--privileged",
            "-v/:/host",
            "",
            "img name",
            "img;rm",
            "img\nx",
        ] {
            assert!(!is_image_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn test_execution_duration_is_what_cloud_run_reports() {
        let e = json!({
            "startTime": "2026-10-08T00:00:00.250Z",
            "completionTime": "2026-10-08T00:01:30.750Z",
        });
        assert_eq!(execution_duration(&e), Some(Duration::from_millis(90_500)));
        assert_eq!(
            execution_duration(&json!({ "startTime": "2026-10-08T00:00:00Z" })),
            None
        );
    }

    #[test]
    fn test_only_refusals_stop_the_polling() {
        let with = |code: u16| ApiError {
            status: Some(reqwest::StatusCode::from_u16(code).unwrap()),
            error: FormaError::Execution("x".to_string()),
        };
        assert!(with(403).is_permanent());
        assert!(with(404).is_permanent());
        assert!(with(401).is_permanent());
        assert!(!with(429).is_permanent());
        assert!(!with(503).is_permanent());
        assert!(!ApiError::from(FormaError::Execution("timeout".to_string())).is_permanent());
    }

    #[test]
    fn test_execution_still_running() {
        assert_eq!(execution_exit(&json!({ "runningCount": 1 })), None);
        assert_eq!(execution_exit(&Value::Null), None);
    }

    #[test]
    fn test_execution_succeeded() {
        let e = json!({ "completionTime": "2026-10-08T00:00:00Z", "succeededCount": 1 });
        assert_eq!(execution_exit(&e), Some(SandboxExit::Completed));
    }

    #[test]
    fn test_execution_failed_reports_the_failed_conditions() {
        let e = json!({
            "completionTime": "2026-10-08T00:00:00Z",
            "failedCount": 1,
            "conditions": [
                { "type": "Ready", "state": "CONDITION_SUCCEEDED" },
                { "type": "Completed", "state": "CONDITION_FAILED",
                  "message": "Task exceeded memory limit" }
            ]
        });
        let Some(SandboxExit::Failed { reason }) = execution_exit(&e) else {
            panic!("should have failed");
        };
        assert!(reason.contains("Completed: Task exceeded memory limit"));
        assert!(reason.contains("failed: 1"));
    }
}
