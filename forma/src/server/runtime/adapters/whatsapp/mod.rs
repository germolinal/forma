mod cloud_api;
pub(crate) mod incomming_message;
pub(crate) mod other_types;
use std::collections::HashMap;

use cloud_api::{WhatsappOutgoingMessage, download_media, upload_media};
use other_types::{
    IncomingMessageValueGeneral, InteractiveContent, MessageWrapper, WhatsappChange,
    WhatsappContent, WhatsappStatusChange,
};

use crate::server::{
    runtime::{
        RuntimeTrait,
        adapters::chat::{self, Approval, ButtonOutcome, ChatApp, ChatMedia},
        default_context_window_messages,
    },
    state::RuntimeState,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use protocols::{CHAT_PATH, DeploymentCredentials, FormaDB, FormaError, envar::expect_env_var};

use derive::FormaDocs;
pub(crate) use incomming_message::WhatsappIncomingMessage;
pub(crate) use other_types::WhatsappWebhookPayload;

use serde::{Deserialize, Serialize};

use genai_client::{events::UsageTally, openai::v1::OpenAIMessagePart};
use tracing::{instrument, warn};

/// What Forma needs from Meta to run an agent on a WhatsApp number, named as
/// Meta's App Dashboard names it.
///
/// A single-tenant server reads it from the environment
/// (`WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`,
/// `WHATSAPP_VERIFY_TOKEN` and `WHATSAPP_APP_SECRET`); a multi-tenant one
/// keeps it with each deployment, under `whatsapp_api`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatsappAPICredentials {
    /// The **Phone number ID** of the number the agent speaks as (in API
    /// Setup). An ID, not the phone number itself.
    phone_number_id: String,

    /// An **access token** with the `whatsapp_business_messaging`
    /// permission, to send messages and download what people send.
    access_token: String,

    /// The **verify token**: a secret of your choosing, also typed into the
    /// webhook's settings, which Meta sends back when it checks the webhook.
    verify_token: String,

    /// The **App secret** (App settings > Basic), with which Meta signs every
    /// delivery.
    app_secret: String,
}

impl From<WhatsappAPICredentials> for DeploymentCredentials {
    fn from(val: WhatsappAPICredentials) -> Self {
        let cred = serde_json::to_value(val).unwrap();
        let mut map = HashMap::new();
        map.insert(WHATSAPP_CREDENTIAL_KEY.to_string(), cred);

        map.into()
    }
}

impl WhatsappAPICredentials {
    fn from_env() -> Result<Self, FormaError> {
        Self {
            phone_number_id: expect_env_var(WHATSAPP_PHONE_NUMBER_ID)?,
            access_token: expect_env_var(WHATSAPP_ACCESS_TOKEN)?,
            verify_token: expect_env_var(WHATSAPP_VERIFY_TOKEN)?,
            app_secret: expect_env_var(WHATSAPP_APP_SECRET)?,
        }
        .validated()
    }

    /// The credentials a request's deployment carries. A single-tenant
    /// server's are its own, read from the environment when it started.
    fn from_deployment(credentials: &DeploymentCredentials) -> Result<Self, FormaError> {
        let Some(value) = credentials.get(WHATSAPP_CREDENTIAL_KEY) else {
            return Err(FormaError::Execution(format!(
                "the deployment has no '{WHATSAPP_CREDENTIAL_KEY}' credentials"
            )));
        };
        let credentials: Self = serde_json::from_value(value.clone()).map_err(|e| {
            FormaError::Execution(format!(
                "the deployment's '{WHATSAPP_CREDENTIAL_KEY}' credentials are invalid: {e}"
            ))
        })?;
        credentials.validated()
    }

    /// None of them works empty, and an empty App secret would let anyone
    /// sign a delivery.
    fn validated(self) -> Result<Self, FormaError> {
        for (name, value) in [
            ("phone_number_id", &self.phone_number_id),
            ("access_token", &self.access_token),
            ("verify_token", &self.verify_token),
            ("app_secret", &self.app_secret),
        ] {
            if value.trim().is_empty() {
                return Err(FormaError::Execution(format!(
                    "the WhatsApp credential '{name}' is empty"
                )));
            }
        }
        Ok(self)
    }
}

/// Use your Forma Agent as a webhook for the [WhatsApp Cloud
/// API](https://developers.facebook.com/documentation/business-messaging/whatsapp/about-the-platform).
///
/// Each phone number has one conversation with the agent, which never ends:
/// the agent reads its last `max_history_messages` messages. Messages that
/// arrive while the agent is still answering wait their turn, and every
/// delivery must be signed by Meta (`X-Hub-Signature-256`).
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct WhatsappAdapter {
    /// The maximum number of messages to retrieve from the
    /// sessions database in each user interaction. The purpose
    /// is to prevent (1) the system prompt from being diluted by
    /// a large number of messages, and (2) reduce the token usage.
    #[serde(default = "default_context_window_messages")]
    pub(crate) max_history_messages: usize,
}

impl Default for WhatsappAdapter {
    fn default() -> Self {
        Self {
            max_history_messages: default_context_window_messages(),
        }
    }
}

const WHATSAPP_PHONE_NUMBER_ID: &str = "WHATSAPP_PHONE_NUMBER_ID";
const WHATSAPP_ACCESS_TOKEN: &str = "WHATSAPP_ACCESS_TOKEN";
const WHATSAPP_VERIFY_TOKEN: &str = "WHATSAPP_VERIFY_TOKEN";
const WHATSAPP_APP_SECRET: &str = "WHATSAPP_APP_SECRET";
const WHATSAPP_CREDENTIAL_KEY: &str = "whatsapp_api";

/// The header Meta signs every delivery with: `sha256=`, followed by the hex
/// HMAC-SHA256 of the body, keyed by the app's App secret.
const SIGNATURE_HEADER: &str = "x-hub-signature-256";

#[derive(Deserialize)]
struct ChallengeQuery {
    #[serde(rename = "hub.mode")]
    hub_mode: String,
    #[serde(rename = "hub.challenge")]
    hub_challenge: String,
    #[serde(rename = "hub.verify_token")]
    hub_verify_token: String,
}

/// Checks that `body` was signed by Meta, with the App secret. Anyone who
/// finds the webhook's URL can post to it, and a forged delivery would have
/// the agent answer, and spend, on behalf of any phone number.
fn verify_signature(app_secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), FormaError> {
    let header = headers.get(SIGNATURE_HEADER).ok_or_else(|| {
        FormaError::Unauthorized("the delivery has no X-Hub-Signature-256 header".to_string())
    })?;
    let signature = header
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("sha256="))
        .and_then(decode_hex)
        .ok_or_else(|| {
            FormaError::Unauthorized("the delivery's X-Hub-Signature-256 is malformed".to_string())
        })?;

    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, app_secret.as_bytes());
    ring::hmac::verify(&key, body, &signature).map_err(|_| {
        FormaError::Unauthorized(
            "the delivery's X-Hub-Signature-256 does not match: it is not from Meta, or the App secret is wrong"
                .to_string(),
        )
    })
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// WhatsApp, as the conversation with the agent sees it (see [`chat`]): how
/// to send what the agent says.
struct WhatsappApp {
    credentials: WhatsappAPICredentials,
}

#[async_trait::async_trait]
impl ChatApp for WhatsappApp {
    const NAME: &'static str = "WhatsApp";
    const MAX_TEXT: usize = 4096;
    const MAX_CAPTION: usize = 1024;

    async fn send_text(&self, to: &str, text: &str) -> Result<(), FormaError> {
        WhatsappOutgoingMessage::new_text_to(to.to_string(), text.to_string())
            .send(&self.credentials)
            .await
    }

    /// WhatsApp will not take bytes inline, and the `link` form needs a public
    /// URL — which a freshly generated image does not have — so a picture is
    /// uploaded first, and sent by the id the upload returns.
    async fn send_image(
        &self,
        to: &str,
        image: &ChatMedia,
        caption: Option<&str>,
    ) -> Result<(), FormaError> {
        let filename = format!("image.{}", chat::media_extension(&image.mime_type));
        let media_id = upload_media(&self.credentials, image, &filename).await?;
        WhatsappOutgoingMessage::new_image_to(to.to_string(), media_id, caption.map(str::to_string))
            .send(&self.credentials)
            .await
    }

    async fn send_approval(&self, to: &str, approval: &Approval) -> Result<(), FormaError> {
        WhatsappOutgoingMessage::new_interactive_approval(to.to_string(), approval)
            .send(&self.credentials)
            .await
    }
}

impl WhatsappAdapter {
    /// Verifies the challenge sent by Meta during the webhook setup process.
    ///
    /// From the [Whatsapp documentation](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/create-webhook-endpoint/):
    ///
    /// **`GET` requests**
    ///
    /// `GET` requests are used to verify your webhook endpoint. Anytime you set or edit the Callback URL field or the Verify token field in the App Dashboard, Meta will send a GET request to your webhook endpoint. You must validate and respond to this request.
    ///
    /// **Request syntax**
    ///
    /// ```bash
    /// GET <CALLBACK_URL>
    ///     ?hub.mode=subscribe
    ///     &hub.challenge=<HUB.CHALLENGE>
    ///     &hub.verify_token=<HUB.VERIFY_TOKEN>
    /// ```
    ///
    /// Where
    /// - `<CALLBACK_URL>` is your webhook endpoint URL.
    /// - `<HUB.CHALLENGE>` is a random string that Meta will generate.
    /// - `<HUB.VERIFY_TOKEN>` is a verification string of your own choosing. Store this string on your server.
    ///
    /// **Validation**
    ///
    /// To validate GET requests, compare the hub.verify_token value in the request to the verification string you have stored on your server. If the values match, the request is valid, otherwise it is invalid.
    ///
    /// **Response**
    ///
    /// If the request is valid, respond with HTTP status 200 and the hub.challenge value. If the request is invalid, respond with a 400-level HTTP status code, or anything other than status 200.
    /// When you configure webhooks, Meta will send a GET request to your webhook endpoint. If it returns status 200 and the hub.challenge value included in the request, Meta will consider your webhook endpoint verified, and begin sending you webhooks. If your webhook endpoint responds with anything else, however, Meta will consider your webhook endpoint unverified, and webhooks will not be sent to your endpoint.
    async fn verify_challenge<DB: FormaDB, T: RuntimeState<DB>>(
        headers: HeaderMap,
        State(state): State<T>,
        Query(params): Query<ChallengeQuery>,
    ) -> Result<String, (StatusCode, String)> {
        let credentials = state.deployment_credentials(&headers).await?;
        let credentials = WhatsappAPICredentials::from_deployment(&credentials)?;

        if params.hub_mode != "subscribe" {
            return Err(FormaError::Input("Invalid hub.mode".to_string()).into());
        }

        if params.hub_verify_token != credentials.verify_token {
            return Err(FormaError::Unauthorized("Invalid verify token".to_string()).into());
        }

        Ok(params.hub_challenge)
    }

    /// What the agent is given for a message, or `None` for the kinds it
    /// does not handle (videos, stickers, locations...).
    ///
    /// Media arrives as a handle, so pictures, audio and documents are
    /// downloaded first.
    async fn message_parts(
        credentials: &WhatsappAPICredentials,
        content: &WhatsappContent,
    ) -> Result<Option<Vec<OpenAIMessagePart>>, FormaError> {
        let parts = match content {
            WhatsappContent::Text { text } => vec![text.into()],
            WhatsappContent::Image { image } => {
                let media = download_media(credentials, &image.id, &image.mime_type).await?;
                let mut parts = vec![chat::image_part(&media)];
                if !image.caption.trim().is_empty() {
                    parts.push(chat::text_part(&image.caption));
                }
                parts
            }
            WhatsappContent::Audio { audio } => {
                let media = download_media(credentials, &audio.id, &audio.mime_type).await?;
                let filename = format!("voice-note.{}", chat::media_extension(&media.mime_type));
                vec![chat::file_part(&media, filename)]
            }
            WhatsappContent::Document { document } => {
                let media = download_media(credentials, &document.id, &document.mime_type).await?;
                let filename = if document.filename.trim().is_empty() {
                    format!("document.{}", chat::media_extension(&media.mime_type))
                } else {
                    document.filename.clone()
                };
                let mut parts = vec![chat::file_part(&media, filename)];
                if !document.caption.trim().is_empty() {
                    parts.push(chat::text_part(&document.caption));
                }
                parts
            }
            _ => return Ok(None),
        };
        Ok(Some(parts))
    }

    #[instrument(skip_all)]
    async fn handle_incoming_message<DB: FormaDB, T: RuntimeState<DB>>(
        m: &WhatsappIncomingMessage,
        credentials: &WhatsappAPICredentials,
        max_history_messages: usize,
        headers: &HeaderMap,
        state: &T,
    ) -> Result<Vec<UsageTally>, FormaError> {
        let app = WhatsappApp {
            credentials: credentials.clone(),
        };
        match &m.content {
            WhatsappContent::Reaction { reaction } => {
                chat::store_reaction::<DB, T>(
                    &m.from,
                    &reaction.message_id,
                    reaction.emoji.as_deref(),
                    max_history_messages,
                    headers,
                    state,
                )
                .await?;
                Ok(vec![])
            }
            WhatsappContent::Interactive { interactive } => {
                let button = match interactive {
                    InteractiveContent::ButtonReply { button_reply } => &button_reply.id,
                    InteractiveContent::ListReply { list_reply } => &list_reply.id,
                };
                let outcome = chat::answer_button::<DB, T, _>(
                    &app,
                    &m.from,
                    button,
                    max_history_messages,
                    headers,
                    state,
                )
                .await?;
                match outcome {
                    ButtonOutcome::Answered(usage) => Ok(usage),
                    // WhatsApp cannot take a button back, so one pressed too
                    // late is just ignored.
                    ButtonOutcome::NotWaiting => Ok(vec![]),
                }
            }
            content => match Self::message_parts(credentials, content).await? {
                Some(parts) => {
                    chat::answer_message::<DB, T, _>(
                        &app,
                        &m.from,
                        &m.id,
                        parts,
                        max_history_messages,
                        headers,
                        state,
                    )
                    .await
                }
                None => {
                    warn!("received unsupported WhatsApp content type; ignoring");
                    Ok(vec![])
                }
            },
        }
    }

    /// Statuses (sent, delivered, read, failed) are acknowledged, not stored:
    /// nothing reads them. A failed one is the only place Meta says why a
    /// message did not arrive, so it is reported.
    fn handle_status(status: &WhatsappStatusChange) {
        for e in &status.errors {
            warn!(message_id = %status.id, "WhatsApp could not deliver a message: {e}");
        }
    }

    #[instrument(skip_all)]
    async fn handle_general_message<DB: FormaDB, T: RuntimeState<DB>>(
        value: &IncomingMessageValueGeneral,
        credentials: &WhatsappAPICredentials,
        max_history_messages: usize,
        headers: &HeaderMap,
        state: &T,
    ) -> Result<Vec<UsageTally>, FormaError> {
        // One Meta app can send several numbers' messages to the same
        // webhook, and a message for another number must not be answered
        // from this one.
        if value.metadata.phone_number_id != credentials.phone_number_id {
            warn!(
                phone_number_id = %value.metadata.phone_number_id,
                "ignoring a delivery for another WhatsApp number"
            );
            return Ok(vec![]);
        }

        let mut results = Vec::with_capacity(value.messages.len());
        for m in &value.messages {
            results.extend(
                Self::handle_incoming_message(m, credentials, max_history_messages, headers, state)
                    .await?,
            );
        }
        for status in &value.statuses {
            Self::handle_status(status);
        }
        Ok(results)
    }

    #[instrument(skip_all)]
    async fn handle_change<DB: FormaDB, T: RuntimeState<DB>>(
        change: &WhatsappChange,
        credentials: &WhatsappAPICredentials,
        max_history_messages: usize,
        headers: &HeaderMap,
        state: &T,
    ) -> Result<Vec<UsageTally>, FormaError> {
        match change {
            WhatsappChange::Messages(MessageWrapper::General(value)) => {
                Self::handle_general_message(
                    value,
                    credentials,
                    max_history_messages,
                    headers,
                    state,
                )
                .await
            }
            WhatsappChange::GroupLifecycle(_)
            | WhatsappChange::GroupSettings(_)
            | WhatsappChange::GroupParticipant(_)
            | WhatsappChange::MessageTemplateStatusUpdate(_)
            | WhatsappChange::AccountReviewUpdate(_)
            | WhatsappChange::AccountUpdate(_)
            | WhatsappChange::TemplateCategoryUpdate(_)
            | WhatsappChange::MessageTemplateQualityUpdate(_)
            | WhatsappChange::PhoneNumberNameUpdate(_)
            | WhatsappChange::PhoneNumberQualityUpdate(_)
            | WhatsappChange::BusinessCapabilityUpdate(_) => {
                warn!("received unhandled WhatsApp change type; ignoring");
                Ok(vec![])
            }
            WhatsappChange::Unknown => {
                warn!("received an unrecognised WhatsApp change field; ignoring");
                Ok(vec![])
            }
        }
    }

    #[instrument(skip_all)]
    async fn webhook<DB: FormaDB, T: RuntimeState<DB>>(
        max_history_messages: usize,
        headers: HeaderMap,
        State(state): State<T>,
        body: Bytes,
    ) -> Result<Json<Vec<UsageTally>>, (StatusCode, String)> {
        let credentials = state.deployment_credentials(&headers).await?;
        let credentials = WhatsappAPICredentials::from_deployment(&credentials)?;
        // Before anything is read from it.
        verify_signature(&credentials.app_secret, &headers, &body)?;

        #[cfg(debug_assertions)]
        println!("{}", String::from_utf8_lossy(&body));

        let message: WhatsappWebhookPayload = serde_json::from_slice(&body)
            .map_err(|e| FormaError::Input(format!("not a WhatsApp delivery: {e}")))?;

        let mut usage_tallies: Vec<UsageTally> = Vec::new();
        for change in message.entry.iter().flat_map(|e| &e.changes) {
            let ts =
                Self::handle_change(change, &credentials, max_history_messages, &headers, &state)
                    .await?;
            usage_tallies.extend(ts);
        }
        Ok(Json(usage_tallies))
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for WhatsappAdapter {
    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }

    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        let credentials = WhatsappAPICredentials::from_env()?;
        let mut map = HashMap::with_capacity(1);
        map.insert(
            WHATSAPP_CREDENTIAL_KEY.to_string(),
            serde_json::to_value(credentials)?,
        );
        Ok(map.into())
    }

    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError> {
        let adapter = self.clone();
        let app = app.route(
            CHAT_PATH,
            post(move |h, s, b| Self::webhook::<DB, T>(adapter.max_history_messages, h, s, b)),
        );
        let app = app.route(
            CHAT_PATH,
            get(move |h, s, m| Self::verify_challenge::<DB, T>(h, s, m)),
        );
        Ok(app)
    }
}

#[cfg(all(test, any(not(feature = "multi-tenant"), feature = "cli")))]
mod test {
    use std::{sync::Arc, time::Duration};

    use agent::Agent;
    use axum::http::HeaderValue;
    use protocols::FormaMessage;

    use db::NoneDB;
    #[cfg(feature = "hot-reload")]
    use tokio::sync::RwLock;
    use tokio::time::sleep;
    use uuid::Uuid;

    use crate::server::state::AppState;
    use other_types::create_test_payload;

    use super::*;

    fn test_host_number() -> &'static str {
        "912164528653259"
    }

    const TEST_APP_SECRET: &str = "the-app-secret";

    fn test_credentials() -> WhatsappAPICredentials {
        WhatsappAPICredentials {
            phone_number_id: test_host_number().to_string(),
            access_token: "tha-key".to_string(),
            verify_token: "kaljshdaos89".to_string(),
            app_secret: TEST_APP_SECRET.to_string(),
        }
    }

    /// The `X-Hub-Signature-256` Meta would send with `body`.
    fn sign(secret: &str, body: &[u8]) -> String {
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        let tag = ring::hmac::sign(&key, body);
        let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256={hex}")
    }

    fn signed(secret: &str, body: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            SIGNATURE_HEADER,
            HeaderValue::from_str(&sign(secret, body)).unwrap(),
        );
        headers
    }

    // ── what a delivery has to prove ───────────────────────────────────────

    #[test]
    fn a_delivery_signed_with_the_app_secret_is_accepted() {
        let body = br#"{"object":"whatsapp_business_account","entry":[]}"#;
        verify_signature(TEST_APP_SECRET, &signed(TEST_APP_SECRET, body), body).unwrap();
    }

    #[test]
    fn a_delivery_that_cannot_prove_it_is_from_meta_is_refused() {
        let body = br#"{"object":"whatsapp_business_account","entry":[]}"#;
        let unauthorized =
            |r: Result<(), FormaError>| matches!(r, Err(FormaError::Unauthorized(_)));

        // No signature at all.
        assert!(unauthorized(verify_signature(
            TEST_APP_SECRET,
            &HeaderMap::new(),
            body
        )));
        // Signed with another secret.
        assert!(unauthorized(verify_signature(
            TEST_APP_SECRET,
            &signed("someone-else", body),
            body
        )));
        // Signed, but the body was changed on the way.
        assert!(unauthorized(verify_signature(
            TEST_APP_SECRET,
            &signed(TEST_APP_SECRET, body),
            br#"{"object":"whatsapp_business_account","entry":[{}]}"#
        )));
        // Malformed: no prefix, and not hex.
        for value in ["abcdef", "sha256=zz", "sha256=abc"] {
            let mut headers = HeaderMap::new();
            headers.insert(SIGNATURE_HEADER, HeaderValue::from_static(value));
            assert!(unauthorized(verify_signature(
                TEST_APP_SECRET,
                &headers,
                body
            )));
        }
    }

    #[test]
    fn credentials_use_metas_names() {
        let deployment: DeploymentCredentials = test_credentials().into();
        let credentials = WhatsappAPICredentials::from_deployment(&deployment).unwrap();
        assert_eq!(credentials.phone_number_id, test_host_number());

        // The names used before are refused, not guessed at: the deployment
        // has to be redeployed with Meta's names (and an App secret).
        let mut old = HashMap::new();
        old.insert(
            WHATSAPP_CREDENTIAL_KEY.to_string(),
            serde_json::json!({
                "host_phone_number_id": "1",
                "whatsapp_api_key": "2",
                "verify_token": "3",
            }),
        );
        let old: DeploymentCredentials = old.into();
        assert!(WhatsappAPICredentials::from_deployment(&old).is_err());

        // An empty App secret would let anyone sign a delivery.
        let empty: DeploymentCredentials = WhatsappAPICredentials {
            app_secret: " ".to_string(),
            ..test_credentials()
        }
        .into();
        assert!(WhatsappAPICredentials::from_deployment(&empty).is_err());
    }

    /// Make sure the adapter loads if the
    /// env variables are there.
    #[test]
    fn test_init() {
        let mut adapter = WhatsappAdapter::default();
        <WhatsappAdapter as RuntimeTrait<NoneDB, AppState<NoneDB>>>::init(&mut adapter).unwrap();
    }

    // ── deliveries, end to end ─────────────────────────────────────────────

    /// Posts `payload` to the webhook of a single-tenant server running
    /// `agent_id`, signed as Meta would.
    async fn test_hook(
        payload: &str,
        agent_id: &str,
    ) -> Result<Json<Vec<UsageTally>>, (StatusCode, String)> {
        use db::mongo::MongoDB;

        let agent = Agent {
            id: agent_id.to_string(),
            ..Default::default()
        };
        #[cfg(feature = "hot-reload")]
        let agent = Arc::new(RwLock::new(agent));
        #[cfg(not(feature = "hot-reload"))]
        let agent = Arc::new(agent);
        let adapter = WhatsappAdapter::default();
        let db = Arc::new(MongoDB::new().await.unwrap());
        let state = AppState::new(agent, db, test_credentials().into())
            .await
            .unwrap();

        let headers = signed(TEST_APP_SECRET, payload.as_bytes());
        WhatsappAdapter::webhook(
            adapter.max_history_messages,
            headers,
            State(state),
            Bytes::from(payload.to_string()),
        )
        .await
    }

    /// A text message from `from`, with WhatsApp's message id `id`.
    fn text_message(from: &str, id: &str) -> String {
        create_test_payload(&format!(
            r#"{{
                "from": "{from}",
                "id": "{id}",
                "timestamp": "1767994843",
                "type": "text",
                "text": {{ "body": "Hello from the test!" }}
            }}"#
        ))
    }

    #[tokio::test]
    async fn an_unsigned_delivery_is_refused() {
        use db::mongo::MongoDB;

        let db = Arc::new(MongoDB::new().await.unwrap());
        let state = AppState::new(
            Arc::new(Agent::default().into()),
            db,
            test_credentials().into(),
        )
        .await
        .unwrap();
        let payload = text_message("64272815246", "wamid.unsigned");

        let res =
            WhatsappAdapter::webhook(30, HeaderMap::new(), State(state), Bytes::from(payload))
                .await;
        assert_eq!(res.unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_delivery_for_another_number_is_ignored() {
        let agent_id = format!("wa-other-number-{}", Uuid::new_v4());
        let payload = text_message("64272815246", "wamid.other-number")
            .replace(test_host_number(), "111111111111111");

        let res = test_hook(&payload, &agent_id).await.unwrap();
        assert!(
            res.0.is_empty(),
            "nothing is answered from the wrong number"
        );
    }

    #[tokio::test]
    async fn reactions_are_stored_on_the_message_they_react_to() {
        use db::mongo::MongoDB;

        let agent_id = format!("wa-reaction-{}", Uuid::new_v4());
        let user_id = Uuid::new_v4().simple().to_string();
        let message_id = Uuid::new_v4().to_string();
        let reaction = create_test_payload(&format!(
            r#"{{
            "from": "{user_id}",
            "id": "wamid.HBgPMjY1MzY4OTMzMjY5NTgxFQIAERgSN0VBRUZCMjVBMDRGN0QzQUY5AB==",
            "timestamp": "1767994843",
            "type": "reaction",
            "reaction": {{
                "message_id": "{message_id}",
                "emoji": "❤️"
            }}
        }}"#
        ));

        // A reaction to a message that is not stored (one of the agent's
        // answers, say) is acknowledged: Meta redelivering it would not help.
        _ = test_hook(&reaction, &agent_id).await.unwrap();

        // One to a stored message is kept with it.
        let db = MongoDB::new().await.unwrap();
        let session = db
            .get_or_create_session(&agent_id, &user_id, 4)
            .await
            .unwrap();
        let session_id = session.id_string().unwrap();
        let reacted_to = FormaMessage {
            id: Some(message_id.clone().into()),
            ..Default::default()
        };
        db.push_msg(&session_id, &user_id, reacted_to)
            .await
            .unwrap();
        sleep(Duration::from_millis(900)).await;

        _ = test_hook(&reaction, &agent_id).await.unwrap();
        let stored = db.get_message_by_id(&message_id).await.unwrap();
        assert_eq!(stored.reactions, vec!["❤️".to_string()]);
    }

    #[tokio::test]
    async fn statuses_are_acknowledged() {
        // Sent, delivered and failed statuses refer to the agent's answers,
        // which are not stored under WhatsApp's ids: acknowledging them is
        // all there is to do (a failure is logged).
        let agent_id = format!("wa-status-{}", Uuid::new_v4());
        for payload in [
            include_str!("./types/incoming_status_delivered.json"),
            include_str!("./types/incoming_status_failed.json"),
            include_str!("./types/incoming_status_sent.json"),
        ] {
            let res = test_hook(payload, &agent_id).await.unwrap();
            assert!(res.0.is_empty());
        }
    }

    #[cfg(all(test, feature = "integration_test"))]
    mod integration_test {
        use super::*;

        #[tokio::test]
        async fn test_text_message() {
            let payload = text_message(
                "64272815246",
                "wamid.HBgPMjY1MzY4OTMzMjY5NTgxFQIAERgSN0VBRUZCMjVBMDRGN0QzQUY5AA==",
            );
            _ = test_hook(&payload, "wa-text-message-agent").await.unwrap();
        }
    }
}
