use std::sync::Arc;

use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse};

#[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
use crate::server::state::AppState;
use crate::{
    observability::{Telemeter, TelemetryMode},
    server::state::RuntimeState,
};

#[cfg(feature = "multi-tenant")]
use crate::server::state::MultitenantAppState;
#[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
use crate::server::state::SharedAgent;

use crate::server::runtime::adapters::{
    ai_sdk_v5::AiSdkV5Adapter, openai_stream::OpenAISSEAdapter, openai_sync::OpenAISyncAdapter,
    telegram::TelegramAdapter, whatsapp::WhatsappAdapter,
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, Response, StatusCode},
    middleware::Next,
};
use derive::FormaDocs;
use protocols::{
    DEFAULT_PORT, DeploymentCredentials, FORMA_AGENT_API_KEY, FORMA_KEY_ENV_VAR, FormaDB,
    FormaError,
};
use serde::{Deserialize, Serialize};
use tracing::{Level, info, instrument};

/// Defines the runtime trait for a Forma app
pub mod runtime_trait;

pub use runtime_trait::RuntimeTrait;
/// Adapters to make this work with
/// various clients
pub mod adapters;

/// Where a server keeps conversations and workspaces, and where it runs code.
pub mod services;
pub use services::{
    CloudRunJob, GcsWorkspaces, LocalDocker, LocalWorkspaces, Sandbox, Storage, Workspaces,
};

/// What a Forma server runs with: who talks to the agent (the `client`), where
/// conversations are kept, and what the agent's tools can reach. This is the
/// `runtime.yaml` file, which `forma serve`, `forma dev` and production servers
/// all read (pass another with `--runtime`). For instance:
///
/// ```yaml
/// client: sse
/// max_history_messages: 20
/// storage: mongo
/// workspaces:
///   store: gcs
/// code_execution:
///   sandbox: cloud-run-job
/// ```
///
/// Secrets, and the names of resources that change from one environment to
/// the next (the database's connection string, the bucket, the Cloud Run
/// job), are not written here: they are environment variables, which each
/// setting below names.
///
/// > **Note** that this file does not hot reload, meaning that; during
/// > development, you will need to stop and restart Forma for changes to be
/// > effective.
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
pub struct FormaRuntime {
    /// Who talks to the agent, and how. Its options are written next to it,
    /// at the top level of the file.
    #[serde(flatten)]
    pub client: Client,

    /// Where conversations are kept.
    pub storage: Storage,

    /// Where workspaces are kept. Without it, the server keeps none, and
    /// workspace tools say so when they are called.
    pub workspaces: Option<Workspaces>,

    /// Where code runs. Without it, the server runs none, and the `code` tool
    /// says so when it is called.
    pub code_execution: Option<Sandbox>,
}

/// Used to determine what kind of client will be communicating with this agent.
///
/// It can be used to switch from Streaming (SSE) or normal REST, and changing
/// the input/output schema.
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "client")]
#[serde(deny_unknown_fields)]
pub enum Client {
    /// Basic service-to-service communication, with no streaming.
    /// Client sends an `Open AI Message`, and receives another back.
    ///
    /// This is meant to work mostly for machine-to-machine communication, where
    /// the client is a server, that needs to submit tasks or questions to
    /// a Forma Agent.
    Sync(OpenAISyncAdapter),

    /// Basic service-to-service communication, with streaming of states (not tokens).
    /// Client sends an `Open AI Message`, and receives updates with respect
    /// to tool calls and other state changes.
    ///
    /// This is meant to work mostly for machine-to-machine communication, where
    /// the client is a server, that needs to submit tasks or questions to
    /// a Forma Agent.
    Sse(OpenAISSEAdapter),

    /// Streamed responses using Vercel's 'AI-SDK v5' Server Side Events (SSE).
    /// Client sends a single message (the last user message) and the Forma agent
    /// will take care of retrieving the full conversation from the sessions database,
    /// if session persistence is enabled.
    ///
    /// Note that sending only the last message is NOT AiSDK's normal behaviour.
    /// The default behaviour of the Vercel AI SDK v5 is to send the whole conversation
    /// on every interaction. Forma does not favour this behaviour.
    ///
    /// > Note: In trying to add compatibility with v6's Human in the loop,
    /// > we noticed that Vercel's docs seem inconsistent or out of date.
    /// > Therefore, human-in-the-loop compatibility is implemented without
    /// > streaming.
    AiSdkV5(AiSdkV5Adapter),

    /// Use your Forma Agent as a Webhook for a [Whatsapp Cloud API](https://developers.facebook.com/documentation/business-messaging/whatsapp/about-the-platform).
    /// Check the [guide](../how-to/whatsapp.md) to set it up.
    ///
    /// This runtime always persists sessions. Each phone number has one
    /// conversation with the agent, and the phone number is the user's ID.
    /// A message that arrives while the agent is still answering waits its
    /// turn, and one that Meta delivers again is answered only once.
    ///
    /// > *Note* that connecting to real webhooks while developing
    /// > your agent will require [Ngrok](https://ngrok.com/) or a similar product.
    /// > However—because the runtime does not change the behaviour of your agent—you
    /// > can test your agent using any other kind of client service
    ///
    /// ### Authentication
    ///
    /// Set the following environment variables, named as Meta's App
    /// Dashboard names them. Every delivery must be signed with the App
    /// secret (`X-Hub-Signature-256`), or it is refused.
    ///
    /// ```bash
    /// WHATSAPP_PHONE_NUMBER_ID=106540352242922 # API Setup: the number the agent speaks as
    /// WHATSAPP_ACCESS_TOKEN=whatsapp-long-token # API Setup: to send messages and download media
    /// WHATSAPP_VERIFY_TOKEN=a-secret # Your choice: Meta sends it back to verify the webhook
    /// WHATSAPP_APP_SECRET=0123456789abcdef # App settings > Basic: Meta signs deliveries with it
    /// ```
    ///
    /// A multi-tenant server reads them from each deployment instead, under
    /// `whatsapp_api` (`phone_number_id`, `access_token`, `verify_token` and
    /// `app_secret`), and needs a reverse proxy that adds `X-Deployment-ID`.
    Whatsapp(WhatsappAdapter),

    /// Use your Forma Agent as a Telegram Bot webhook.
    /// Check the [guide](../how-to/telegram.md) to set it up.
    ///
    /// It behaves exactly as the `whatsapp` runtime does: only the app
    /// changes. This runtime always persists sessions. Each Telegram chat
    /// has one conversation with the agent, and the chat's ID is the user's
    /// ID. A message that arrives while the agent is still answering waits
    /// its turn, and one that Telegram delivers again is answered only once.
    ///
    /// Register the webhook with Telegram once your server is reachable,
    /// with the same secret token:
    /// ```text
    /// https://api.telegram.org/bot{TOKEN}/setWebhook?url=https://your-host/v1/chat&secret_token={SECRET_TOKEN}
    /// ```
    ///
    /// ### Human in the loop
    ///
    /// Human in the loop is surfaced as a message with two inline
    /// buttons: Approve or Reject, which go away once one is pressed.
    ///
    /// ### Authentication
    ///
    /// Set the following environment variables. Every update must carry the
    /// secret token (`X-Telegram-Bot-Api-Secret-Token`), or it is refused.
    ///
    /// ```bash
    /// TELEGRAM_BOT_TOKEN=your-telegram-bot-token # the token obtained from @BotFather.
    /// TELEGRAM_SECRET_TOKEN=a-secret # Your choice: given to setWebhook, which Telegram sends back
    /// ```
    ///
    /// A multi-tenant server reads them from each deployment instead, under
    /// `telegram_api` (`bot_token` and `secret_token`), and needs a reverse
    /// proxy that adds `X-Deployment-ID`.
    Telegram(TelegramAdapter),
}

pub(crate) fn default_context_window_messages() -> usize {
    30
}

/// Searches for the Expected API Key within the FORMA_AGENT_KEY
/// variable. Fails if it is not found, thus make sure you call this
/// only when the agent has been defined to require Auth.
fn expected_key() -> Result<String, (StatusCode, &'static str)> {
    match std::env::var(FORMA_KEY_ENV_VAR) {
        Ok(k) => Ok(k),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Deployment error: 'FORMA_AGENT_KEY' not set as environment variable",
        )),
    }
}

fn get_port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn get_host_url() -> String {
    // Expose as server
    #[cfg(not(feature = "cli"))]
    let ret = format!("0.0.0.0:{}", get_port());
    // Only requests from this machine
    #[cfg(feature = "cli")]
    let ret = format!("127.0.0.1:{}", get_port());
    ret
}

/// Checks that an appropriate API Key was provided in
/// the request. Will return an error if not, of if it is
/// incorrectly formatted. Thus, make sure to call this function
/// only if the Agent was defined as requiring an API Key
fn provided_key(headers: &HeaderMap) -> Result<String, (StatusCode, &'static str)> {
    let k = headers.get(FORMA_AGENT_API_KEY);
    let k = match k {
        Some(v) => v,
        None => {
            info!("No API Key found");
            return Err((StatusCode::UNAUTHORIZED, "No API Key found"));
        }
    };
    let k = match k.to_str() {
        Ok(v) => v,
        Err(_s) => {
            info!("API Key could not be parsed");
            return Err((StatusCode::UNAUTHORIZED, "API Key could not be parsed"));
        }
    };

    Ok(k.to_string())
}

/// Validates that the API key in `headers` matches the expected key from the environment.
/// Returns `Ok(())` on success, or an appropriate `(StatusCode, message)` error.
fn check_api_key(headers: &HeaderMap) -> Result<(), (StatusCode, &'static str)> {
    let expected = expected_key()?;
    let provided = provided_key(headers)?;

    if provided == expected {
        #[cfg(debug_assertions)]
        println!("API Key verified");

        Ok(())
    } else {
        #[cfg(debug_assertions)]
        println!("Incorrect API Key");

        Err((StatusCode::FORBIDDEN, "Incorrect API Key"))
    }
}

async fn api_key_middleware(req: Request, next: Next) -> Response<Body> {
    match check_api_key(req.headers()) {
        Ok(()) => next.run(req).await,
        Err((code, e)) => Response::builder().status(code).body(e.into()).unwrap(),
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for Client {
    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        match self {
            Client::Sync(s) => RuntimeTrait::<DB, T>::credentials_from_env(s),
            Client::Sse(s) => RuntimeTrait::<DB, T>::credentials_from_env(s),
            Client::AiSdkV5(s) => RuntimeTrait::<DB, T>::credentials_from_env(s),
            Client::Whatsapp(s) => RuntimeTrait::<DB, T>::credentials_from_env(s),
            Client::Telegram(s) => RuntimeTrait::<DB, T>::credentials_from_env(s),
        }
    }

    fn init(&mut self) -> Result<(), FormaError> {
        match self {
            Client::Sync(s) => RuntimeTrait::<DB, T>::init(s),
            Client::Sse(s) => RuntimeTrait::<DB, T>::init(s),
            Client::AiSdkV5(s) => RuntimeTrait::<DB, T>::init(s),
            Client::Whatsapp(s) => RuntimeTrait::<DB, T>::init(s),
            Client::Telegram(s) => RuntimeTrait::<DB, T>::init(s),
        }
    }

    fn build_app(&self, mut app: Router<T>) -> Result<Router<T>, FormaError> {
        app = match self {
            Client::Sync(s) => s.build_app(app)?,
            Client::Sse(s) => s.build_app(app)?,
            Client::AiSdkV5(s) => s.build_app(app)?,
            Client::Whatsapp(s) => s.build_app(app)?,
            Client::Telegram(s) => s.build_app(app)?,
        };

        if !self.no_api_key() {
            app = app.layer(axum::middleware::from_fn(api_key_middleware));
        }

        // Add the Axum basic events (e.g., 404, 401, etc.) logging
        let request_response_logging = tower_http::trace::TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO));
        app = app.layer(request_response_logging);

        Ok(app)
    }
}

impl Client {
    fn no_api_key(&self) -> bool {
        match self {
            Client::Sync(s) => s.no_api_key,
            Client::Sse(s) => s.no_api_key,
            Client::AiSdkV5(s) => s.no_api_key,
            Client::Whatsapp(_) => true,
            Client::Telegram(_) => true,
        }
    }
}

impl FormaRuntime {
    /// Reads a runtime file (YAML or JSON), and checks it.
    #[instrument]
    pub fn load(filename: &str) -> Result<Self, FormaError> {
        #[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
        type State = AppState<db::FormaDBClient>;
        #[cfg(all(feature = "multi-tenant", not(feature = "cli")))]
        type State = MultitenantAppState<db::FormaDBClient>;

        let contents = std::fs::read_to_string(filename).map_err(|e| {
            FormaError::Input(format!("could not read the runtime file '{filename}': {e}"))
        })?;
        let mut runtime: Self = if filename.ends_with(".json") {
            serde_json::from_str(&contents)
                .map_err(|e| FormaError::Input(format!("runtime file '{filename}': {e}")))?
        } else if filename.ends_with(".yaml") || filename.ends_with(".yml") {
            serde_yaml::from_str(&contents)
                .map_err(|e| FormaError::Input(format!("runtime file '{filename}': {e}")))?
        } else {
            return Err(FormaError::Input(format!(
                "runtime file '{filename}': it should be either `json` or `yaml`"
            )));
        };
        runtime.host()?.check()?;
        RuntimeTrait::<db::FormaDBClient, State>::init(&mut runtime.client)?;
        Ok(runtime)
    }

    /// What this server gives its agents' tools: where workspaces are kept,
    /// and where code runs.
    fn host(&self) -> Result<agent::tool::host::Host, FormaError> {
        Ok(agent::tool::host::Host {
            workspaces: self
                .workspaces
                .as_ref()
                .map(Workspaces::store)
                .transpose()?,
            sandbox: self.code_execution.as_ref().map(Sandbox::kind),
        })
    }

    /// Hands this server's workspaces and sandbox to the agents' tools. Once
    /// per process, before an agent is loaded.
    pub fn install(&self) -> Result<(), FormaError> {
        agent::tool::host::install(self.host()?)
    }

    /// Serves the client's routes
    ///
    /// `extra` is merged after [`RuntimeTrait::build_app`] has already layered
    /// on the API key check, and axum only applies a layer to the routes
    /// registered before it — so the extra routes are deliberately left
    /// unauthenticated. That is what `forma dev` wants for its own chat UI,
    /// and why nothing reaching the network in production passes anything here.
    async fn start<DB: FormaDB, T: RuntimeState<DB>>(
        &self,
        telemetry: TelemetryMode,
        service_name: &str,
        state: T,
        extra: Option<Router<T>>,
    ) -> Result<(), FormaError> {
        let app = Router::new();
        let telemeter = Telemeter::new(service_name, telemetry);
        let mut app = RuntimeTrait::<DB, T>::build_app(&self.client, app)?;

        if let Some(extra) = extra {
            app = app.merge(extra);
        }

        let app = app.with_state(state);

        let bind_to = get_host_url();
        let listener = tokio::net::TcpListener::bind(bind_to)
            .await
            .expect("Failed to bind TCP listener");

        let addr = listener.local_addr().expect("Failed to get local address");
        info!("Listening on {addr}",);
        axum::serve(listener, app)
            .with_graceful_shutdown(crate::observability::shutdown_signal())
            .await
            .expect("Failed to start server");

        // Ensure all spans are flushed before exit
        telemeter.shutdown();

        Ok(())
    }

    /// Builds the state a single-tenant server runs on
    #[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
    async fn single_tenant_state<DB: FormaDB>(
        &self,
        agent: SharedAgent,
        db: Arc<DB>,
    ) -> Result<AppState<DB>, FormaError> {
        let mut credentials =
            <Client as RuntimeTrait<DB, AppState<DB>>>::credentials_from_env(&self.client)?;
        // Read workspaces from .workspaces.json
        let workspaces = ".workspaces.json";
        if std::path::Path::new(workspaces).exists() {
            let str = std::fs::read_to_string(workspaces).map_err(|x| {
                FormaError::Input(format!("could not read file '{}' : {}", workspaces, x))
            })?;
            let workspaces: serde_json::Value = serde_json::from_str(&str)?;
            credentials.insert("workspaces".to_string(), workspaces);
        }
        AppState::new(agent, db, credentials).await
    }

    /// Start the server for an agent
    #[cfg(any(not(feature = "multi-tenant"), feature = "cli"))]
    pub async fn run<DB: FormaDB>(
        &self,
        telemetry: TelemetryMode,
        service_name: &str,
        agent: SharedAgent,
        db: Arc<DB>,
    ) -> Result<(), FormaError> {
        let state = self.single_tenant_state(agent, db).await?;
        self.start(telemetry, service_name, state, None).await
    }

    /// Start a development server for an agent, serving `extra` — the embedded
    /// chat UI — next to the agent's own routes.
    #[cfg(feature = "cli")]
    pub(crate) async fn run_dev<DB: FormaDB>(
        &self,
        telemetry: TelemetryMode,
        service_name: &str,
        agent: SharedAgent,
        db: Arc<DB>,
        extra: Router<AppState<DB>>,
    ) -> Result<(), FormaError> {
        let bind_to = get_host_url();
        println!("______________________________________________________");
        println!("| Chat with '{service_name}' at http://{bind_to}");
        println!("------------------------------------------------------");
        let state = self.single_tenant_state(agent, db).await?;
        self.start(telemetry, service_name, state, Some(extra))
            .await
    }

    /// Start the server for a multitenant agent
    #[cfg(feature = "multi-tenant")]
    pub async fn run_multitenant<DB: FormaDB>(
        &self,
        telemetry: TelemetryMode,
        service_name: &str,
        db: Arc<DB>,
    ) -> Result<(), FormaError> {
        if self.storage == Storage::None {
            return Err(FormaError::Input(
                "a multi-tenant server reads its agents from its storage, so it cannot use \
                 `storage: none`"
                    .to_string(),
            ));
        }
        let state = MultitenantAppState::new(db);
        self.start(telemetry, service_name, state, None).await
    }
}

#[cfg(test)]
mod test {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{expected_key, provided_key};

    use protocols::FORMA_KEY_ENV_VAR;

    // Tests that read/write env vars must hold this lock for the duration of the test
    // to prevent races when cargo runs tests in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn parse(yaml: &str) -> Result<super::FormaRuntime, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    #[test]
    fn test_runtime_file() {
        use super::{Client, Sandbox, Storage, Workspaces};

        let runtime = parse(
            "client: sync\nmax_history_messages: 10\nstorage: mongo\n\
             workspaces:\n  store: gcs\ncode_execution:\n  sandbox: cloud-run-job\n",
        )
        .unwrap();
        let Client::Sync(sync) = &runtime.client else {
            panic!("not sync: {runtime:?}");
        };
        assert_eq!(sync.max_history_messages, 10);
        assert_eq!(runtime.storage, Storage::Mongo);
        assert_eq!(
            runtime.workspaces,
            Some(Workspaces::Gcs(Default::default()))
        );
        assert_eq!(
            runtime.code_execution,
            Some(Sandbox::CloudRunJob(Default::default()))
        );

        // Only the client and the storage are required.
        let runtime = parse("client: telegram\nstorage: file\n").unwrap();
        assert!(matches!(runtime.client, Client::Telegram(_)));
        assert_eq!(runtime.workspaces, None);
        assert_eq!(runtime.code_execution, None);
        let err = parse("client: sse\n").unwrap_err();
        assert!(err.contains("missing field `storage`"), "{err}");

        // A folder for the workspaces, and Docker's default image.
        let runtime = parse(
            "client: sse\nstorage: none\nworkspaces:\n  store: local\n  dir: ./ws\n\
             code_execution:\n  sandbox: local-docker\n",
        )
        .unwrap();
        let Some(Workspaces::Local(local)) = &runtime.workspaces else {
            panic!("not local: {runtime:?}");
        };
        assert_eq!(local.dir, "./ws");
        let Some(Sandbox::LocalDocker(docker)) = &runtime.code_execution else {
            panic!("not docker: {runtime:?}");
        };
        assert_eq!(docker.image, "forma-sandbox:dev");
    }

    #[test]
    fn test_unknown_runtime_fields_are_rejected() {
        // The client's options sit next to the other settings, and a typo in
        // any of them is still refused.
        let err = parse("client: sync\nstorage: none\nmax_history_mesages: 10\n").unwrap_err();
        assert!(err.contains("unknown field `max_history_mesages`"), "{err}");
        let err = parse("client: sync\nstorage: none\nworkspace:\n  store: gcs\n").unwrap_err();
        assert!(err.contains("unknown field `workspace`"), "{err}");
        let err = parse("client: sync\nstorage: nowhere\n").unwrap_err();
        assert!(err.contains("unknown variant `nowhere`"), "{err}");
        let err = parse("client: sync\nstorage: none\nworkspaces:\n  store: local\n  folder: x\n")
            .unwrap_err();
        assert!(err.contains("unknown field `folder`"), "{err}");
        let err = parse("client: sync\nstorage: none\nworkspaces:\n  store: gcs\n  bucket: x\n")
            .unwrap_err();
        assert!(err.contains("unknown field `bucket`"), "{err}");
        let err = parse(
            "client: sync\nstorage: none\ncode_execution:\n  sandbox: cloud-run-job\n  job: x\n",
        )
        .unwrap_err();
        assert!(err.contains("unknown field `job`"), "{err}");
    }

    #[test]
    fn test_what_tools_get_is_checked_when_loaded() {
        use agent::tool::workspace::StoreKind;

        let runtime =
            parse("client: sse\nstorage: file\nworkspaces:\n  store: local\n  dir: ./ws\n")
                .unwrap();
        let host = runtime.host().unwrap();
        assert_eq!(host.workspaces, Some(StoreKind::Local("./ws".into())));
        assert_eq!(host.sandbox, None);
        assert!(host.check().is_ok());

        // A run's files need a store that signs URLs...
        let runtime = parse(
            "client: sse\nstorage: file\nworkspaces:\n  store: local\n  dir: ./ws\n\
             code_execution:\n  sandbox: local-docker\n",
        )
        .unwrap();
        assert!(runtime.host().unwrap().check().is_err());

        // ...and a folder must be named.
        let runtime =
            parse("client: sse\nstorage: file\nworkspaces:\n  store: local\n  dir: ' '\n").unwrap();
        assert!(runtime.host().is_err());
    }

    #[test]
    fn test_provided_key() {
        let key = "some-fake-key";
        let bearer = "some-fake-key";

        // No key
        let headers = HeaderMap::new();
        let r = provided_key(&headers);
        assert!(r.is_err());
        let mut headers = HeaderMap::new();
        headers.insert("Auth", HeaderValue::from_static(key)); // not empty, but not in the right header
        let r = provided_key(&headers);
        assert!(r.is_err());

        // Not a string case....??
        // ??

        // Bad format
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_static(key)); // No 'bearer'
        let r = provided_key(&headers);
        assert!(r.is_err());

        // Good

        let mut headers = HeaderMap::new();
        headers.insert("X-API-Key", HeaderValue::from_static(&bearer)); // No 'bearer'
        let r = provided_key(&headers);
        match r {
            Ok(v) => assert_eq!(v, key),
            Err(_) => panic!("Wrong key"),
        };
    }

    #[test]
    fn test_expected_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var(FORMA_KEY_ENV_VAR) }
        assert!(expected_key().is_err());
        let key = "some-fake-key";
        unsafe {
            std::env::set_var(FORMA_KEY_ENV_VAR, key);
        }
        match expected_key() {
            Ok(k) => assert_eq!(key, k),
            Err(_e) => panic!("could not get variable"),
        }
    }

    #[test]
    fn test_check_api_key() {
        use super::check_api_key;

        let _lock = ENV_LOCK.lock().unwrap();

        // No env var set → INTERNAL_SERVER_ERROR regardless of headers
        unsafe { std::env::remove_var(FORMA_KEY_ENV_VAR) }
        let headers = HeaderMap::new();
        let r = check_api_key(&headers);
        assert_eq!(
            r.unwrap_err().0,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        let key = "correct-key";
        unsafe { std::env::set_var(FORMA_KEY_ENV_VAR, key) }

        // Env var set but no Authorization header → UNAUTHORIZED
        let headers = HeaderMap::new();
        let r = check_api_key(&headers);
        assert_eq!(r.unwrap_err().0, axum::http::StatusCode::UNAUTHORIZED);

        // Env var set, wrong key → FORBIDDEN
        let mut headers = HeaderMap::new();
        headers.insert("X-API-KEY", HeaderValue::from_static("wrong-key"));
        let r = check_api_key(&headers);
        assert_eq!(r.unwrap_err().0, axum::http::StatusCode::FORBIDDEN);

        // Env var set, correct key → Ok
        let mut headers = HeaderMap::new();
        headers.insert("X-API-KEY", HeaderValue::from_static("correct-key"));
        assert!(check_api_key(&headers).is_ok());
    }
}
