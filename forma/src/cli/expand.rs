use crate::api::ObjectDefinition;
use crate::cli::{login::read_definition, path_file::PathAndFile};
use agent::Callable;
use clap::Parser;
use db::file::FileDB;
use protocols::{EncryptedTokens, FormaDB, FormaError};
use serde::Serialize;

#[derive(Default, Copy, Clone, PartialEq, Eq, Debug, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
enum AgentFormat {
    #[default]
    Yaml,
    Json,
}

impl AgentFormat {
    fn print_agent<T: Serialize>(&self, agent: &T) -> Result<(), FormaError> {
        let s = match self {
            AgentFormat::Yaml => serde_yaml::to_string(agent).map_err(|e| format!("{e}"))?,
            AgentFormat::Json => serde_json::to_string_pretty(agent).map_err(|e| format!("{e}"))?,
        };
        println!("{s}");
        Ok(())
    }
}

/// Arguments for expanding an Agent. This means, replacing
/// all the TemplateFields by their actual values.
#[derive(Debug, Parser)]
pub struct ExpandArgs {
    /// The path where the agent and its dependencies are stored
    #[arg(short, long, default_value = "./src")]
    pub path: String,

    /// The name of the agent file within the given path
    #[arg(short, long, default_value = "agent")]
    pub agent: String,

    #[arg(long, short, default_value_t, value_enum)]
    /// The format of the
    format: AgentFormat,

    /// Compile the agent
    #[arg(long, short)]
    compile: bool,

    /// The user ID who owns the tokens used to connect
    /// the agent to MCP servers. Only relevant when
    /// compiling the agent.
    #[arg(short, long, default_value = "forma-user")]
    pub user: String,
}

impl ExpandArgs {
    /// Expand the agent
    pub async fn expand(&self) -> Result<(), FormaError> {
        let file = PathAndFile {
            filename: self.agent.clone(),
            path: self.path.clone(),
        }
        .validate();
        // Check definition... then we re-check if this is expanded
        let agent = read_definition(&file.filename)?.check()?;

        if self.compile {
            let mcps = agent.mcp_definitions();
            let oauth = if mcps.is_empty() {
                // Will be ignored... no need for Auth.
                EncryptedTokens::default()
            } else {
                let db = FileDB::new().await?;
                db.get_oauth_tokens(&self.user).await?
            };
            let agent = agent.into_agent(&oauth).await?;
            // Now that the MCP servers have been connected to and every tool
            // has its final name, the runtime's own checks can run — the same
            // ones `forma serve` runs before serving the agent.
            Callable::<FileDB>::check(&agent)?;
            self.format.print_agent(&agent)
        } else {
            self.format.print_agent(&agent)
        }
    }
}

#[cfg(test)]
mod tests {}
