use crate::{
    cli::path_file::{PathAndFile, assume_file_format},
    observability::TelemetryMode,
    server::{FormaRuntime, load_agent},
};
use agent::Agent;
use clap::Parser;

use notify::{
    Error, Event, EventKind, RecursiveMode, Result as NotifyResult, Watcher,
    event::{CreateKind, ModifyKind},
};
use protocols::{FormaDB, FormaError};

use std::path::Path;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc, oneshot};

use tracing::info;

const FILES_TO_IGNORE: &[&str] = &[".DS_Store", ".gitignore"];

fn should_reload(e: Event) -> bool {
    if e.paths.len() == 1
        && FILES_TO_IGNORE.contains(&e.paths[0].file_name().unwrap().to_str().unwrap())
    {
        return false;
    }
    match e.kind {
        EventKind::Create(s) => match s {
            CreateKind::File => {
                info!("... created file");
                true
            }
            _ => false,
        },
        EventKind::Modify(s) => match s {
            ModifyKind::Name(_) => {
                info!("... renamed file");
                true
            }
            ModifyKind::Data(_) => {
                info!("... modified file");
                true
            }
            ModifyKind::Any | ModifyKind::Metadata(_) | ModifyKind::Other => false,
        },
        EventKind::Remove(_) => {
            eprint!("... removed fole");
            false
        }
        EventKind::Any | EventKind::Other | EventKind::Access(_) => false,
    }
}

/// A handle for stopping the Hot-reloading loop
pub struct WatcherHandle {
    tx: oneshot::Sender<()>,
}

impl WatcherHandle {
    /// Use it to stop the hot-reloading thread
    pub fn stop(self) {
        let _ = self.tx.send(());
    }
}

async fn handle_event<DB: FormaDB>(
    res: Result<Event, Error>,
    filename: &str,
    user_id: &str,
    agent: Arc<RwLock<Agent>>,
    db: Arc<DB>,
) {
    match res {
        Ok(event) => {
            if should_reload(event) {
                match load_agent(filename, user_id, db).await {
                    Ok(new) => {
                        let mut a = agent.write().await;
                        *a = new;
                    }
                    Err(e) => {
                        eprintln!("[Error] {e}")
                    }
                }
            }
        }
        Err(e) => {
            println!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "hot-reload")]
pub(crate) async fn watch_files<DB: FormaDB>(
    agent: Arc<RwLock<Agent>>,
    filename: String,
    user_id: String,
    path: String,
    db: Arc<DB>,
) -> WatcherHandle {
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let (tx, mut rx) = mpsc::channel::<NotifyResult<Event>>(100);
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.blocking_send(res);
    })
    .unwrap();
    watcher
        .watch(Path::new(&path), RecursiveMode::Recursive)
        .unwrap();

    tokio::task::spawn(async move {
        let _watcher = watcher;
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!("Stopping file watcher...");
                    break;
                }
                Some(res) = rx.recv() => {
                    handle_event(res, &filename,&user_id, agent.clone(), db.clone()).await;
                }

            }
        }
    });
    WatcherHandle { tx: stop_tx }
}

/// Arguments used to start the development server
#[derive(Debug, Parser)]
pub struct ServeArgs {
    /// Whether to send logs to the console, or emit tracing data to an OpenTelemetry collector.
    #[arg(short, long, default_value_t, value_enum)]
    pub telemetry: TelemetryMode,

    /// The file describing the runtime settings
    #[arg(short, long, default_value = "runtime.yaml")]
    pub runtime: String,

    /// The path where the agent and its dependencies are stored
    #[arg(short, long, default_value = "./src")]
    pub path: String,

    /// The name of the agent file within the given path
    #[arg(short, long, default_value = "agent")]
    pub agent: String,

    /// The user ID to associate with the agent
    #[arg(short, long, default_value = "forma-user")]
    pub user: String,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            telemetry: Default::default(),
            path: "./src".to_string(),
            agent: "agent".to_string(),
            runtime: "runtime.yaml".to_string(),
            user: "test-user".to_string(),
        }
    }
}

impl ServeArgs {
    async fn load_agent<DB: FormaDB>(&self, db: Arc<DB>) -> Result<Agent, FormaError> {
        let file = PathAndFile {
            filename: self.agent.clone(),
            path: self.path.clone(),
        };
        let file = file.validate();
        let ret = load_agent(&file.filename, &self.user, db).await?;

        Ok(ret)
    }

    fn load_runtime(&self) -> Result<FormaRuntime, FormaError> {
        FormaRuntime::load(&assume_file_format(&self.runtime))
    }

    /// Start the server
    pub async fn serve(&self) -> Result<(), FormaError> {
        // Before the agent, whose tools are checked against what the server
        // gives them.
        let runtime = self.load_runtime()?;
        runtime.install()?;
        let db = Arc::new(runtime.storage.open().await?);

        let telemetry = self.telemetry;

        let agent = self.load_agent(db.clone()).await?;
        let agent_id = agent.id.clone();

        let agent = Arc::new(RwLock::new(agent));
        #[cfg(not(feature = "hot-reload"))]
        let agent = Arc::new(agent);

        let file = PathAndFile {
            filename: self.agent.clone(),
            path: self.path.clone(),
        };

        let file = file.validate();

        #[cfg(feature = "hot-reload")]
        let handler = watch_files(
            agent.clone(),
            file.filename.clone(),
            self.user.clone(),
            file.path,
            db.clone(),
        )
        .await;

        runtime.run(telemetry, &agent_id, agent, db).await?;

        #[cfg(feature = "hot-reload")]
        handler.stop(); // stop watching files
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "cli")]
    use super::*;
    #[cfg(feature = "cli")]
    use crate::cli::init::InitArgs;
    #[cfg(feature = "cli")]
    use db::NoneDB;
    #[cfg(feature = "cli")]
    use tempfile::tempdir;

    /// Let's make sure `forma init` and then `forma serve` works out of the box
    #[tokio::test]
    #[cfg(feature = "cli")]
    async fn test_files_in_path() {
        // 1. Create a temporary directory that is automatically deleted when dropped
        let dir = tempdir().expect("Failed to create temp dir");
        let root_path = dir.path().join("test-agent");
        let root_path_str = root_path.to_str().unwrap().to_string();

        // 2. Generate a basic agent
        // This creates the directory structure inside the temp folder

        let init_args = InitArgs {
            path: root_path_str.clone(),
        };
        init_args.init().expect("could not init agent");
        let src_path = root_path.join("src"); // Init puts these to in here.
        let src_path_str = src_path.to_str().unwrap().to_string();

        // 3. Create a defult 'serve' command
        let args = ServeArgs {
            path: src_path_str.clone(),
            runtime: format!("{}/runtime.yaml", &root_path_str),
            ..Default::default()
        };
        // 3. Verify files exist before loading to be sure init() worked
        let settings = root_path.join(&args.runtime);
        assert!(settings.exists());

        let agent = src_path.join(format!("{}.yaml", &args.agent));
        assert!(agent.exists());
        let db = Arc::new(NoneDB {});
        args.load_agent(db).await.expect("could not load agent");
        args.load_runtime().expect("could not load settings");
    }
}
