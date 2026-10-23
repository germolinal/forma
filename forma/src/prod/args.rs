use crate::{observability::TelemetryMode, server::FormaRuntime};
#[cfg(not(feature = "multi-tenant"))]
use agent::Agent;
use clap::Parser;
use protocols::FormaError;
#[cfg(all(feature = "hot-reload", not(feature = "multi-tenant")))]
use tokio::sync::RwLock;

/// Runs a server in production
#[derive(Debug, Parser)]
pub struct ProdServeArgs {
    /// Whether to send logs to the console, or emit tracing data to an OpenTelemetry collector.
    #[arg(short, long, default_value = "otel", value_enum)]
    pub telemetry: TelemetryMode,

    /// The file describing the runtime settings: who talks to the agent,
    /// where conversations and workspaces are kept, and where code runs.
    ///
    /// If no extension is provided, (e.g., `runtime`), `.yaml` is assumed.
    /// Otherwise, the provided extension is used as-is.
    ///
    /// Only `.yaml` and `.json` are supported.
    #[arg(short, long, default_value = "runtime.yaml")]
    pub runtime: String,

    /// Service name, used as `service.name` on every span and log record.
    ///
    /// Defaults to the Cloud Run service name. It cannot be baked into the
    /// image: the Dockerfile's exec-form CMD does no variable expansion, and a
    /// build ARG would not be in scope at runtime anyway.
    #[cfg(feature = "multi-tenant")]
    #[arg(short, long, env = "K_SERVICE", default_value = "forma-multi-tenant")]
    pub service: String,

    /// The name of the expanded agent file within the given path.
    ///
    /// If no extension is provided, (e.g., `agent`), `.yaml` is assumed.
    /// Otherwise, the provided extension is used as-is.
    ///
    /// Only `.yaml` and `.json` are supported.
    #[cfg(not(feature = "multi-tenant"))]
    #[arg(short, long, default_value = "agent")]
    pub agent: String,
}

/// `name`, or `name.yaml` if it has no extension.
fn assume_yaml(name: &str) -> String {
    if std::path::Path::new(name).extension().is_none() {
        format!("{name}.yaml")
    } else {
        name.to_string()
    }
}

impl ProdServeArgs {
    /// Reads the runtime file, and hands what it gives the agents' tools to
    /// them.
    fn runtime(&self) -> Result<FormaRuntime, FormaError> {
        let runtime = FormaRuntime::load(&assume_yaml(&self.runtime))?;
        runtime.install()?;
        Ok(runtime)
    }

    /// Runs the server.
    #[cfg(not(feature = "multi-tenant"))]
    pub async fn run(&self) -> Result<(), FormaError> {
        let runtime = self.runtime()?;
        let path = assume_yaml(&self.agent);
        let agent_str = std::fs::read_to_string(&path)
            .map_err(|e| FormaError::Input(format!("could not load agent in '{path}': {e}")))?;

        let agent: Agent = if path.ends_with(".yaml") || path.ends_with(".yml") {
            serde_yaml::from_str(&agent_str)
                .map_err(|e| FormaError::Input(format!("could not parse agent in '{path}': {e}")))?
        } else {
            serde_json::from_str(&agent_str)
                .map_err(|e| FormaError::Input(format!("could not parse agent in '{path}': {e}")))?
        };
        // An agent definition (the output of `forma expand` without `-c`)
        // also parses as an `Agent`, but with no nodes at all.
        if agent.start.plan.is_empty() {
            return Err(FormaError::Input(format!(
                "the agent in '{path}' has no nodes. Production servers need a compiled agent: create it with `forma expand -c`"
            )));
        }
        let agent_id = agent.id.clone();
        #[cfg(feature = "hot-reload")]
        let agent = RwLock::new(agent);
        let agent = std::sync::Arc::new(agent);
        let db = std::sync::Arc::new(runtime.storage.open().await?);
        runtime.run(self.telemetry, &agent_id, agent, db).await?;

        Ok(())
    }

    /// Runs the server for multi-tenant agents
    ///
    /// Assumes the agent is stored as JSON in the `db`
    #[cfg(feature = "multi-tenant")]
    pub async fn run_multi_tenant(&self) -> Result<(), FormaError> {
        let runtime = self.runtime()?;
        let db = std::sync::Arc::new(runtime.storage.open().await?);
        runtime
            .run_multitenant(self.telemetry, &self.service, db)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::assume_yaml;

    #[test]
    fn test_files_without_an_extension_are_yaml() {
        assert_eq!(assume_yaml("runtime"), "runtime.yaml");
        assert_eq!(assume_yaml("/app/runtime"), "/app/runtime.yaml");
        assert_eq!(assume_yaml("runtime.json"), "runtime.json");
        assert_eq!(assume_yaml("./deploy/runtime.yml"), "./deploy/runtime.yml");
    }
}
