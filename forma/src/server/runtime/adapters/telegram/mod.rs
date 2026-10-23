mod bot_api;
pub(crate) mod types;

use std::collections::HashMap;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use bot_api::{InlineButton, TelegramBot};
use derive::FormaDocs;
use genai_client::{events::UsageTally, openai::v1::OpenAIMessagePart};
use protocols::{CHAT_PATH, DeploymentCredentials, FormaDB, FormaError, envar::expect_env_var};
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};
use types::{TelegramCallbackQuery, TelegramMessage, TelegramUpdate};

use crate::server::{
    runtime::{
        RuntimeTrait,
        adapters::chat::{
            self, APPROVAL_PROMPT, APPROVE_LABEL, Approval, ButtonOutcome, ChatApp, ChatMedia,
            REJECT_LABEL,
        },
        default_context_window_messages,
    },
    state::RuntimeState,
};

/// What Forma needs from Telegram to run an agent as a bot, named as
/// Telegram names it.
///
/// A single-tenant server reads it from the environment (`TELEGRAM_BOT_TOKEN`
/// and `TELEGRAM_SECRET_TOKEN`); a multi-tenant one keeps it with each
/// deployment, under `telegram_api`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramAPICredentials {
    /// The bot's **token**, from @BotFather.
    bot_token: String,

    /// The **secret token** the webhook was registered with (`setWebhook`'s
    /// `secret_token`), which Telegram sends with every update.
    secret_token: String,
}

impl From<TelegramAPICredentials> for DeploymentCredentials {
    fn from(val: TelegramAPICredentials) -> Self {
        let cred = serde_json::to_value(val).unwrap();
        let mut map = HashMap::new();
        map.insert(TELEGRAM_CREDENTIAL_KEY.to_string(), cred);

        map.into()
    }
}

impl TelegramAPICredentials {
    fn from_env() -> Result<Self, FormaError> {
        Self {
            bot_token: expect_env_var(TELEGRAM_BOT_TOKEN)?,
            secret_token: expect_env_var(TELEGRAM_SECRET_TOKEN)?,
        }
        .validated()
    }

    /// The credentials a request's deployment carries. A single-tenant
    /// server's are its own, read from the environment when it started.
    fn from_deployment(credentials: &DeploymentCredentials) -> Result<Self, FormaError> {
        let Some(value) = credentials.get(TELEGRAM_CREDENTIAL_KEY) else {
            return Err(FormaError::Execution(format!(
                "the deployment has no '{TELEGRAM_CREDENTIAL_KEY}' credentials"
            )));
        };
        let credentials: Self = serde_json::from_value(value.clone()).map_err(|e| {
            FormaError::Execution(format!(
                "the deployment's '{TELEGRAM_CREDENTIAL_KEY}' credentials are invalid: {e}"
            ))
        })?;
        credentials.validated()
    }

    /// Neither works empty, and an empty secret token would let anyone post
    /// updates.
    fn validated(self) -> Result<Self, FormaError> {
        for (name, value) in [
            ("bot_token", &self.bot_token),
            ("secret_token", &self.secret_token),
        ] {
            if value.trim().is_empty() {
                return Err(FormaError::Execution(format!(
                    "the Telegram credential '{name}' is empty"
                )));
            }
        }
        Ok(self)
    }

    /// The bot's own id: the part of its token before the colon.
    fn bot_id(&self) -> &str {
        self.bot_token
            .split_once(':')
            .map_or(self.bot_token.as_str(), |(id, _)| id)
    }
}

/// Use your Forma Agent as a [Telegram bot](https://core.telegram.org/bots).
///
/// Each chat has one conversation with the agent, which never ends: the
/// agent reads its last `max_history_messages` messages. Messages that
/// arrive while the agent is still answering wait their turn, and every
/// update must carry the webhook's secret token
/// (`X-Telegram-Bot-Api-Secret-Token`).
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct TelegramAdapter {
    /// The maximum number of messages to retrieve from the
    /// sessions database in each user interaction. The purpose
    /// is to prevent (1) the system prompt from being diluted by
    /// a large number of messages, and (2) reduce the token usage.
    #[serde(default = "default_context_window_messages")]
    pub(crate) max_history_messages: usize,
}

impl Default for TelegramAdapter {
    fn default() -> Self {
        Self {
            max_history_messages: default_context_window_messages(),
        }
    }
}

const TELEGRAM_BOT_TOKEN: &str = "TELEGRAM_BOT_TOKEN";
const TELEGRAM_SECRET_TOKEN: &str = "TELEGRAM_SECRET_TOKEN";
const TELEGRAM_CREDENTIAL_KEY: &str = "telegram_api";

/// The header Telegram sends the webhook's secret token in.
const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

/// Checks that an update comes from Telegram: only Telegram knows the secret
/// token the webhook was registered with. Anyone who finds the webhook's URL
/// can post to it, and a forged update would have the agent answer, and
/// spend, on behalf of any chat.
fn verify_secret(secret_token: &str, headers: &HeaderMap) -> Result<(), FormaError> {
    let sent = headers.get(SECRET_HEADER).ok_or_else(|| {
        FormaError::Unauthorized(
            "the update has no X-Telegram-Bot-Api-Secret-Token header".to_string(),
        )
    })?;
    // Compared in constant time, so the answer's timing says nothing about
    // how much of a guess was right.
    let (sent, expected) = (sent.as_bytes(), secret_token.as_bytes());
    let differences = sent
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if sent.len() != expected.len() || differences != 0 {
        return Err(FormaError::Unauthorized(
            "the update's X-Telegram-Bot-Api-Secret-Token does not match".to_string(),
        ));
    }
    Ok(())
}

/// The id a message is stored under, so that one Telegram delivers again is
/// recognised. Telegram numbers messages per chat, and a person's chat with
/// one bot has the same id as their chat with another, so both go in.
fn message_id(bot_id: &str, message: &TelegramMessage) -> String {
    format!(
        "telegram:{bot_id}:{}:{}",
        message.chat.id, message.message_id
    )
}

/// Telegram, as the conversation with the agent sees it (see [`chat`]): how
/// to send what the agent says.
struct TelegramApp {
    bot: TelegramBot,
}

#[async_trait::async_trait]
impl ChatApp for TelegramApp {
    const NAME: &'static str = "Telegram";
    const MAX_TEXT: usize = 4096;
    const MAX_CAPTION: usize = 1024;

    async fn send_text(&self, to: &str, text: &str) -> Result<(), FormaError> {
        Ok(self.bot.send_message(to, text, None).await?)
    }

    async fn send_image(
        &self,
        to: &str,
        image: &ChatMedia,
        caption: Option<&str>,
    ) -> Result<(), FormaError> {
        Ok(self.bot.send_photo(to, image, caption).await?)
    }

    async fn send_approval(&self, to: &str, approval: &Approval) -> Result<(), FormaError> {
        let text = format!(
            "*{}*\n\n{}\n\n_{APPROVAL_PROMPT}_",
            approval.title, approval.details
        );
        let buttons = vec![
            InlineButton {
                text: APPROVE_LABEL.to_string(),
                callback_data: approval.approve.clone(),
            },
            InlineButton {
                text: REJECT_LABEL.to_string(),
                callback_data: approval.reject.clone(),
            },
        ];
        Ok(self.bot.send_message(to, &text, Some(buttons)).await?)
    }
}

impl TelegramAdapter {
    /// What the agent is given for a message, or `None` for the kinds it
    /// does not handle (videos, stickers, locations...).
    ///
    /// A message carries only its files' ids, so photos, voice notes, audio
    /// files and documents are downloaded first.
    async fn message_parts(
        bot: &TelegramBot,
        message: &TelegramMessage,
    ) -> Result<Option<Vec<OpenAIMessagePart>>, FormaError> {
        let caption = message
            .caption
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .map(chat::text_part);

        let mut parts = if let Some(text) = &message.text {
            vec![chat::text_part(text)]
        } else if let Some(photo) = message
            .photo
            .iter()
            .max_by_key(|p| u64::from(p.width) * u64::from(p.height))
        {
            // Telegram sends a photo in several sizes; the largest is the
            // one the person sent.
            let media = bot.download(&photo.file_id, "image/jpeg").await?;
            vec![chat::image_part(&media)]
        } else if let Some(file) = message.voice.as_ref().or(message.audio.as_ref()) {
            let mime_type = file.mime_type.as_deref().unwrap_or("audio/ogg");
            let media = bot.download(&file.file_id, mime_type).await?;
            let filename = file.file_name.clone().unwrap_or_else(|| {
                format!("voice-note.{}", chat::media_extension(&media.mime_type))
            });
            vec![chat::file_part(&media, filename)]
        } else if let Some(document) = &message.document {
            let mime_type = document
                .mime_type
                .as_deref()
                .unwrap_or("application/octet-stream");
            let media = bot.download(&document.file_id, mime_type).await?;
            let filename = document
                .file_name
                .clone()
                .unwrap_or_else(|| format!("document.{}", chat::media_extension(&media.mime_type)));
            vec![chat::file_part(&media, filename)]
        } else {
            return Ok(None);
        };
        // The caption is the question about the file, so it reads last.
        parts.extend(caption);
        Ok(Some(parts))
    }

    #[instrument(skip_all)]
    async fn handle_message<DB: FormaDB, T: RuntimeState<DB>>(
        message: &TelegramMessage,
        credentials: &TelegramAPICredentials,
        max_history_messages: usize,
        headers: &HeaderMap,
        state: &T,
    ) -> Result<Vec<UsageTally>, FormaError> {
        let app = TelegramApp {
            bot: TelegramBot::new(&credentials.bot_token),
        };
        let Some(parts) = Self::message_parts(&app.bot, message).await? else {
            warn!("received unsupported Telegram content type; ignoring");
            return Ok(vec![]);
        };
        chat::answer_message::<DB, T, _>(
            &app,
            &message.chat.id.to_string(),
            &message_id(credentials.bot_id(), message),
            parts,
            max_history_messages,
            headers,
            state,
        )
        .await
    }

    /// A press on one of an approval's buttons. The button stops spinning,
    /// and the buttons go away, so that the call is answered once.
    #[instrument(skip_all)]
    async fn handle_button<DB: FormaDB, T: RuntimeState<DB>>(
        press: &TelegramCallbackQuery,
        credentials: &TelegramAPICredentials,
        max_history_messages: usize,
        headers: &HeaderMap,
        state: &T,
    ) -> Result<Vec<UsageTally>, FormaError> {
        let app = TelegramApp {
            bot: TelegramBot::new(&credentials.bot_token),
        };
        if let Err(e) = app.bot.answer_callback_query(&press.id).await {
            warn!("could not answer a Telegram button press: {e:?}");
        }
        // The chat the buttons were sent to; a private chat's id is also the
        // person's.
        let chat_id = press
            .message
            .as_ref()
            .map_or(press.from.id, |m| m.chat.id)
            .to_string();
        if let Some(message) = &press.message
            && let Err(e) = app.bot.remove_buttons(&chat_id, message.message_id).await
        {
            warn!("could not take a Telegram approval's buttons away: {e:?}");
        }

        let Some(button) = &press.data else {
            return Ok(vec![]);
        };
        let outcome = chat::answer_button::<DB, T, _>(
            &app,
            &chat_id,
            button,
            max_history_messages,
            headers,
            state,
        )
        .await?;
        match outcome {
            ButtonOutcome::Answered(usage) => Ok(usage),
            // Its buttons are gone now, too.
            ButtonOutcome::NotWaiting => Ok(vec![]),
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
        let credentials = TelegramAPICredentials::from_deployment(&credentials)?;
        // Before anything is read from it.
        verify_secret(&credentials.secret_token, &headers)?;

        #[cfg(debug_assertions)]
        println!("{}", String::from_utf8_lossy(&body));

        let update: TelegramUpdate = serde_json::from_slice(&body)
            .map_err(|e| FormaError::Input(format!("not a Telegram update: {e}")))?;

        let usage = if let Some(message) = &update.message {
            Self::handle_message(
                message,
                &credentials,
                max_history_messages,
                &headers,
                &state,
            )
            .await?
        } else if let Some(press) = &update.callback_query {
            Self::handle_button(press, &credentials, max_history_messages, &headers, &state).await?
        } else {
            warn!("received unhandled Telegram update type; ignoring");
            vec![]
        };
        Ok(Json(usage))
    }
}

impl<DB: FormaDB, T: RuntimeState<DB>> RuntimeTrait<DB, T> for TelegramAdapter {
    fn init(&mut self) -> Result<(), FormaError> {
        Ok(())
    }

    fn credentials_from_env(&self) -> Result<DeploymentCredentials, FormaError> {
        let credentials = TelegramAPICredentials::from_env()?;
        let mut map = HashMap::with_capacity(1);
        map.insert(
            TELEGRAM_CREDENTIAL_KEY.to_string(),
            serde_json::to_value(credentials)?,
        );
        Ok(map.into())
    }

    fn build_app(&self, app: Router<T>) -> Result<Router<T>, FormaError> {
        let adapter = self.clone();
        Ok(app.route(
            CHAT_PATH,
            post(move |h, s, b| Self::webhook::<DB, T>(adapter.max_history_messages, h, s, b)),
        ))
    }
}

#[cfg(all(test, any(not(feature = "multi-tenant"), feature = "cli")))]
mod tests {
    use std::sync::Arc;

    use agent::Agent;
    use axum::http::HeaderValue;
    #[cfg(feature = "hot-reload")]
    use tokio::sync::RwLock;
    use uuid::Uuid;

    use crate::server::state::AppState;

    use super::*;

    const TEST_SECRET: &str = "the-secret-token";

    fn test_credentials() -> TelegramAPICredentials {
        TelegramAPICredentials {
            bot_token: "7412345678:AAHf3x9Qz-test".to_string(),
            secret_token: TEST_SECRET.to_string(),
        }
    }

    fn with_secret(secret: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(SECRET_HEADER, HeaderValue::from_str(secret).unwrap());
        headers
    }

    // ── what an update has to prove ────────────────────────────────────────

    #[test]
    fn an_update_with_the_secret_token_is_accepted() {
        verify_secret(TEST_SECRET, &with_secret(TEST_SECRET)).unwrap();
    }

    #[test]
    fn an_update_that_cannot_prove_it_is_from_telegram_is_refused() {
        let unauthorized =
            |r: Result<(), FormaError>| matches!(r, Err(FormaError::Unauthorized(_)));
        assert!(unauthorized(verify_secret(TEST_SECRET, &HeaderMap::new())));
        assert!(unauthorized(verify_secret(
            TEST_SECRET,
            &with_secret("someone-elses")
        )));
        // A prefix of the secret is not the secret.
        assert!(unauthorized(verify_secret(
            TEST_SECRET,
            &with_secret("the-secret")
        )));
    }

    #[test]
    fn credentials_use_telegrams_names() {
        let deployment: DeploymentCredentials = test_credentials().into();
        let credentials = TelegramAPICredentials::from_deployment(&deployment).unwrap();
        assert_eq!(credentials.bot_id(), "7412345678");

        // Without a secret token, nobody could tell Telegram's updates from
        // anyone else's.
        let mut old = HashMap::new();
        old.insert(
            TELEGRAM_CREDENTIAL_KEY.to_string(),
            serde_json::json!({ "bot_token": "1:a" }),
        );
        let old: DeploymentCredentials = old.into();
        assert!(TelegramAPICredentials::from_deployment(&old).is_err());

        let empty: DeploymentCredentials = TelegramAPICredentials {
            secret_token: " ".to_string(),
            ..test_credentials()
        }
        .into();
        assert!(TelegramAPICredentials::from_deployment(&empty).is_err());
    }

    #[test]
    fn a_message_is_stored_under_its_bot_chat_and_number() {
        let message: TelegramMessage = serde_json::from_str(
            r#"{"message_id": 42, "date": 0, "chat": {"id": -100123, "type": "group"}}"#,
        )
        .unwrap();
        assert_eq!(
            message_id("7412345678", &message),
            "telegram:7412345678:-100123:42"
        );
    }

    /// Make sure the adapter loads if the
    /// env variables are there.
    #[test]
    fn test_init() {
        let mut adapter = TelegramAdapter::default();
        <TelegramAdapter as RuntimeTrait<db::NoneDB, AppState<db::NoneDB>>>::init(&mut adapter)
            .unwrap();
    }

    // ── updates, end to end ────────────────────────────────────────────────

    /// Posts `update` to the webhook of a single-tenant server running
    /// `agent_id`, with the secret token, as Telegram would.
    async fn test_hook(
        update: &str,
        agent_id: &str,
        secret: &str,
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
        let db = Arc::new(MongoDB::new().await.unwrap());
        let state = AppState::new(agent, db, test_credentials().into())
            .await
            .unwrap();

        TelegramAdapter::webhook(
            TelegramAdapter::default().max_history_messages,
            with_secret(secret),
            State(state),
            Bytes::from(update.to_string()),
        )
        .await
    }

    fn text_update(chat_id: i64, message_id: i64) -> String {
        format!(
            r#"{{
                "update_id": {message_id},
                "message": {{
                    "message_id": {message_id},
                    "date": 1761070000,
                    "chat": {{"id": {chat_id}, "type": "private"}},
                    "from": {{"id": {chat_id}, "is_bot": false, "first_name": "Ana"}},
                    "text": "Hello from the test!"
                }}
            }}"#
        )
    }

    fn a_chat() -> i64 {
        // Distinct per test run, so tests sharing the database do not share
        // a conversation.
        (Uuid::new_v4().as_u128() % 1_000_000_000) as i64 + 1
    }

    #[tokio::test]
    async fn an_update_without_the_secret_token_is_refused() {
        let res = test_hook(&text_update(a_chat(), 1), "tg-unsigned", "nope").await;
        assert_eq!(res.unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn what_the_agent_cannot_read_is_ignored() {
        let sticker = format!(
            r#"{{"update_id": 1, "message": {{"message_id": 1, "date": 0,
                "chat": {{"id": {}, "type": "private"}},
                "sticker": {{"file_id": "s", "file_unique_id": "s1", "type": "regular",
                             "width": 512, "height": 512, "is_animated": false, "is_video": false}}}}}}"#,
            a_chat()
        );
        let res = test_hook(&sticker, "tg-sticker", TEST_SECRET)
            .await
            .unwrap();
        assert!(res.0.is_empty());
    }

    #[tokio::test]
    async fn a_button_for_a_call_that_is_not_waiting_is_ignored() {
        let chat_id = a_chat();
        let press = format!(
            r#"{{"update_id": 2, "callback_query": {{
                "id": "q1", "chat_instance": "-1",
                "from": {{"id": {chat_id}, "is_bot": false, "first_name": "Ana"}},
                "message": {{"message_id": 5, "date": 0, "chat": {{"id": {chat_id}, "type": "private"}}}},
                "data": "approve|call-that-is-not-waiting"
            }}}}"#
        );
        let res = test_hook(&press, "tg-stale-button", TEST_SECRET)
            .await
            .unwrap();
        assert!(res.0.is_empty());
    }
}
