use crate::{
    cli::path_file::{PathAndFile, assume_file_format},
    observability::TelemetryMode,
    server::{
        Client, FormaRuntime, RuntimeTrait, load_agent,
        runtime::adapters::openai_stream::OpenAISSEAdapter, state::AppState,
    },
};
use agent::{Agent, ToolRunner, Workflow};
use axum::{
    Router,
    body::Bytes,
    http::{StatusCode, header},
    routing::get,
};
use clap::Parser;
use db::FormaDBClient;
use protocols::{FormaDB, FormaError};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::RwLock;

#[cfg(feature = "hot-reload")]
use crate::cli::serve::watch_files;

/// The chat UI, embedded so that a `forma` binary is the only thing anyone
/// needs to talk to their agent. It is a single dependency-free file, which
/// keeps a JavaScript toolchain out of Forma's build.
const CHAT_UI: &str = include_str!("ui/index.html");

/// The marker the UI leaves for the server to hand it its settings.
const CONFIG_PLACEHOLDER: &str = "<!--forma-config-->";

/// Settings the UI cannot know on its own, injected at start up.
#[derive(Serialize)]
struct UiConfig<'a> {
    /// Chat sessions and OAuth tokens are both keyed by user, so the UI has to
    /// use the same identity the agent was loaded with — otherwise the tokens
    /// stored by `forma login` are invisible to the conversation.
    user_id: &'a str,
    agent_id: &'a str,
    /// The roles the agent's `just_for` lists name, offered as a hint in the
    /// UI's roles field.
    roles: Vec<String>,
}

/// Every role named in a `just_for` list, in `workflow` and in the workflows
/// its tools run, which see the same roles.
fn roles(workflow: &Workflow, found: &mut BTreeSet<String>) {
    for node in workflow.plan.iter().flatten() {
        for tool in &node.tools {
            found.extend(tool.just_for.iter().cloned());
            if let ToolRunner::Workflow(inner) = &tool.tool {
                roles(inner, found);
            }
        }
    }
}

/// Renders the chat UI with `config` baked in.
fn chat_ui(config: &UiConfig) -> Result<String, FormaError> {
    let json = serde_json::to_string(config)?
        // `serde_json` escapes quotes and backslashes but leaves `<` alone,
        // which would let an agent id containing `</script>` close the tag.
        .replace('<', "\\u003c");

    Ok(CHAT_UI.replace(
        CONFIG_PLACEHOLDER,
        &format!("<script>window.__FORMA_CONFIG__={json};</script>"),
    ))
}

/// Serves the chat UI at the root of the agent's own server.
fn ui_router<DB: FormaDB>(html: String) -> Router<AppState<DB>> {
    let page = Bytes::from(html);

    Router::new()
        .route(
            "/",
            get(move || {
                let page = page.clone();
                async move {
                    (
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        page.clone(),
                    )
                }
            }),
        )
        // Browsers ask for this unprompted; answering keeps the log readable.
        .route("/favicon.ico", get(async || StatusCode::NO_CONTENT))
}

/// Arguments used to start the development server and its chat UI
#[derive(Debug, Parser)]
pub struct DevArgs {
    /// Whether to send logs to the console, or emit tracing data to an OpenTelemetry collector.
    #[arg(short, long, default_value_t, value_enum)]
    pub telemetry: TelemetryMode,

    /// The file describing the runtime settings. Its `client` is ignored:
    /// the chat UI speaks Server Sent Events.
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

impl Default for DevArgs {
    fn default() -> Self {
        Self {
            telemetry: Default::default(),
            runtime: "runtime.yaml".to_string(),
            path: "./src".to_string(),
            agent: "agent".to_string(),
            user: "forma-user".to_string(),
        }
    }
}

impl DevArgs {
    async fn load_agent<DB: FormaDB>(&self, db: Arc<DB>) -> Result<Agent, FormaError> {
        let file = PathAndFile {
            filename: self.agent.clone(),
            path: self.path.clone(),
        };
        let file = file.validate();
        load_agent(&file.filename, &self.user, db).await
    }

    /// The runtime `forma dev` serves: the one in `runtime.yaml`, but for its
    /// client.
    ///
    /// The embedded UI speaks the SSE protocol, and pinning the client is
    /// what lets the UI work against an agent destined for WhatsApp or
    /// Telegram. The API key check is off because the only client is a page
    /// served from this same process. Everything else — where conversations
    /// and workspaces are kept, where code runs — is what the file says, as
    /// in `forma serve`.
    fn runtime(&self) -> Result<FormaRuntime, FormaError> {
        let mut runtime = FormaRuntime::load(&assume_file_format(&self.runtime))?;
        // Everything but the missing key is the adapter's own default, which
        // is also where the chat UI gets a body limit big enough for the
        // files it can now attach.
        runtime.client = Client::Sse(OpenAISSEAdapter {
            no_api_key: true,
            ..Default::default()
        });
        RuntimeTrait::<FormaDBClient, AppState<FormaDBClient>>::init(&mut runtime.client)?;
        Ok(runtime)
    }

    /// Start the development server and serve the chat UI
    pub async fn dev(&self) -> Result<(), FormaError> {
        // Before the agent, whose tools are checked against what the server
        // gives them.
        let runtime = self.runtime()?;
        runtime.install()?;
        let db = Arc::new(runtime.storage.open().await?);

        let agent = self.load_agent(db.clone()).await?;
        let agent_id = agent.id.clone();
        let mut agent_roles = BTreeSet::new();
        roles(&agent.start, &mut agent_roles);

        #[cfg(feature = "hot-reload")]
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

        let html = chat_ui(&UiConfig {
            user_id: &self.user,
            agent_id: &agent_id,
            roles: agent_roles.into_iter().collect(),
        })?;

        runtime
            .run_dev(self.telemetry, &agent_id, agent, db, ui_router(html))
            .await?;

        #[cfg(feature = "hot-reload")]
        handler.stop(); // stop watching files

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_has_a_config_placeholder() {
        assert!(
            CHAT_UI.contains(CONFIG_PLACEHOLDER),
            "the embedded UI must keep the '{CONFIG_PLACEHOLDER}' marker for the server to fill in"
        );
    }

    #[test]
    fn config_is_injected() {
        let html = chat_ui(&UiConfig {
            user_id: "someone",
            agent_id: "my-agent",
            roles: vec!["admin".to_string()],
        })
        .expect("could not render the UI");

        assert!(!html.contains(CONFIG_PLACEHOLDER));
        assert!(html.contains(
            r#"window.__FORMA_CONFIG__={"user_id":"someone","agent_id":"my-agent","roles":["admin"]};"#
        ));
    }

    /// Roles come from every tool, including those of workflow tools, once.
    #[test]
    fn roles_are_collected_from_nested_workflows() {
        let tool = |name: &str, runner: ToolRunner, just_for: &[&str]| agent::Tool {
            name: name.to_string(),
            description: None,
            tool: runner,
            just_for: just_for.iter().map(|r| r.to_string()).collect(),
            confirm_before_calling: false,
            bundle: None,
        };
        let node = |tools: Vec<agent::Tool>| agent::Node {
            tools,
            ..Default::default()
        };
        let template = || {
            ToolRunner::Template(agent::tool::TemplateTool {
                templates: vec!["done".to_string()],
            })
        };

        let inner = Workflow {
            plan: vec![vec![node(vec![tool(
                "audit",
                template(),
                &["auditor", "admin"],
            )])]],
            ..Default::default()
        };
        let start = Workflow {
            plan: vec![vec![node(vec![
                tool("refund", template(), &["admin"]),
                tool("everyone", template(), &[]),
                tool("deep", ToolRunner::Workflow(inner), &[]),
            ])]],
            ..Default::default()
        };

        let mut found = BTreeSet::new();
        roles(&start, &mut found);
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            vec!["admin".to_string(), "auditor".to_string()]
        );
    }

    /// An id is free-form text, and it ends up inside a `<script>` tag.
    #[test]
    fn config_cannot_close_the_script_tag() {
        let html = chat_ui(&UiConfig {
            user_id: "</script><script>alert(1)</script>",
            agent_id: "my-agent",
            roles: vec![],
        })
        .expect("could not render the UI");

        let injected = html
            .split("window.__FORMA_CONFIG__=")
            .nth(1)
            .expect("config was not injected");
        let injected = injected.split("</script>").next().unwrap();
        assert!(!injected.contains('<'));
        assert!(injected.contains("\\u003c/script"));
    }
}
