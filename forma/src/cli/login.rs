use crate::{
    api::{AgentDefinition, McpToolDefinition, ObjectDefinition},
    cli::path_file::PathAndFile,
};
use clap::Parser;

use db::file::FileDB;
use protocols::OauthToken;
use protocols::{FormaDB, FormaError, credentials::EncryptedTokenSet};
use std::collections::HashMap;

pub(crate) fn read_definition(filename: &str) -> Result<AgentDefinition, FormaError> {
    let contents = std::fs::read_to_string(filename)
        .map_err(|_| FormaError::Execution(format!("Failed to read agent file '{filename}'")))?;
    let def: AgentDefinition = if filename.ends_with(".json") {
        serde_json::from_str(&contents).map_err(|e| FormaError::Execution(e.to_string()))?
    } else if filename.ends_with(".yaml") || filename.ends_with(".yml") {
        serde_yaml::from_str(&contents).map_err(|e| FormaError::Execution(e.to_string()))?
    } else {
        return Err("agent file must be .json or .yaml".into());
    };
    def.expand(filename)
}

/// Arguments for `forma login`
#[derive(Debug, Parser)]
pub struct LoginArgs {
    /// The path where the agent and its dependencies are stored
    #[arg(short, long, default_value = "./src")]
    pub path: String,

    /// The name of the agent file within the given path
    #[arg(short, long, default_value = "agent")]
    pub agent: String,

    /// The user ID whose tokens are being created or updated.
    #[arg(short, long, default_value = "forma-user")]
    pub user: String,
}

async fn auth(mcps: &[McpToolDefinition]) -> Result<EncryptedTokenSet<OauthToken>, FormaError> {
    let mut tokens: HashMap<String, OauthToken> = HashMap::new();
    for mcp in mcps {
        eprintln!("Authenticating MCP server: {}", mcp.url);
        if let Some(token) = mcp.authenticate().await? {
            tokens.insert(mcp.url.clone(), token);
        }
    }
    Ok(EncryptedTokenSet(tokens))
}

impl LoginArgs {
    /// Run the interactive OAuth2 login for every authenticated MCP server in the agent
    /// and persist the resulting tokens under the given user ID.
    pub async fn login(&self) -> Result<(), FormaError> {
        let file = PathAndFile {
            filename: self.agent.clone(),
            path: self.path.clone(),
        }
        .validate();

        let def = read_definition(&file.filename)?;
        let mcps = def.mcp_definitions();

        let user_id = &self.user;

        if mcps.is_empty() {
            eprintln!("No authenticated MCP servers found in agent — nothing to do.");
            return Ok(());
        }

        let tokens = auth(&mcps).await?;
        let db = FileDB::new().await?;

        db.save_oauth_tokens(user_id, tokens).await?;
        Ok(())
    }
}
