// https://core.telegram.org/bots/api#available-methods

use std::sync::LazyLock;

use protocols::FormaError;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};

use crate::server::runtime::adapters::chat::ChatMedia;
#[cfg(not(feature = "airtight"))]
use crate::server::runtime::adapters::chat::media_extension;

/// The client every call to the Bot API goes through, so its connection is
/// kept and reused, instead of being opened (and its TLS handshake made)
/// again for every message.
#[cfg_attr(feature = "airtight", allow(dead_code))]
static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

/// Why a call failed, as Telegram put it.
#[derive(Debug)]
pub(crate) struct BotApiError {
    description: String,
}

impl BotApiError {
    /// Telegram could not read the message's Markdown: an unpaired `*` or
    /// `_`, which an answer written for any other app can easily contain.
    fn is_bad_markdown(&self) -> bool {
        self.description.contains("can't parse entities")
    }
}

impl From<BotApiError> for FormaError {
    fn from(e: BotApiError) -> Self {
        // Never the URL: it carries the bot's token.
        FormaError::ApiCall {
            url: Some("https://api.telegram.org/bot.../...".to_string()),
            msg: e.description,
        }
    }
}

impl From<reqwest::Error> for BotApiError {
    fn from(e: reqwest::Error) -> Self {
        Self {
            description: e.without_url().to_string(),
        }
    }
}

/// A button under a message.
#[derive(Serialize)]
pub(crate) struct InlineButton {
    pub(crate) text: String,
    pub(crate) callback_data: String,
}

/// A bot, by its token.
pub(crate) struct TelegramBot {
    token: String,
}

#[cfg_attr(feature = "airtight", allow(unused_variables))]
impl TelegramBot {
    pub(crate) fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
        }
    }

    #[cfg_attr(feature = "airtight", allow(dead_code))]
    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.token)
    }

    /// Reads the envelope every method answers with: `{"ok": true, "result":
    /// ...}`, or `{"ok": false, "description": ...}`.
    #[cfg(not(feature = "airtight"))]
    async fn result(response: reqwest::Response) -> Result<Value, BotApiError> {
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| BotApiError {
            description: format!("HTTP {status}, and an answer that is not JSON: {e}"),
        })?;
        if body["ok"].as_bool() == Some(true) {
            return Ok(body["result"].clone());
        }
        Err(BotApiError {
            description: body["description"]
                .as_str()
                .map_or_else(|| format!("HTTP {status}"), str::to_string),
        })
    }

    /// Calls `method` with a JSON body. Airtight builds make no calls.
    async fn call(&self, method: &str, body: &Value) -> Result<Value, BotApiError> {
        #[cfg(feature = "airtight")]
        return Ok(Value::Null);

        #[cfg(not(feature = "airtight"))]
        Self::result(CLIENT.post(self.url(method)).json(body).send().await?).await
    }

    /// Sends `text` to `chat_id`, with buttons under it if any.
    ///
    /// Written with Telegram's (legacy) Markdown, whose `*bold*` and
    /// `_italics_` are WhatsApp's too. A message Telegram cannot read as
    /// Markdown is sent again as plain text, rather than not at all.
    pub(crate) async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        buttons: Option<Vec<InlineButton>>,
    ) -> Result<(), BotApiError> {
        let mut body = json!({ "chat_id": chat_id, "text": text, "parse_mode": "Markdown" });
        if let Some(buttons) = buttons {
            body["reply_markup"] = json!({ "inline_keyboard": [buttons] });
        }
        match self.call("sendMessage", &body).await {
            Err(e) if e.is_bad_markdown() => {
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("parse_mode");
                }
                self.call("sendMessage", &body).await.map(drop)
            }
            other => other.map(drop),
        }
    }

    /// Uploads a picture to `chat_id`, with its caption if any.
    pub(crate) async fn send_photo(
        &self,
        chat_id: &str,
        image: &ChatMedia,
        caption: Option<&str>,
    ) -> Result<(), BotApiError> {
        #[cfg(feature = "airtight")]
        return Ok(());

        #[cfg(not(feature = "airtight"))]
        {
            let send = |markdown: bool| async move {
                let photo = reqwest::multipart::Part::bytes(image.bytes.clone())
                    .file_name(format!("image.{}", media_extension(&image.mime_type)))
                    .mime_str(&image.mime_type)
                    .map_err(|e| BotApiError {
                        description: format!("invalid media type for upload: {e}"),
                    })?;
                let mut form = reqwest::multipart::Form::new()
                    .text("chat_id", chat_id.to_string())
                    .part("photo", photo);
                if let Some(caption) = caption {
                    form = form.text("caption", caption.to_string());
                    if markdown {
                        form = form.text("parse_mode", "Markdown");
                    }
                }
                let response = CLIENT
                    .post(self.url("sendPhoto"))
                    .multipart(form)
                    .send()
                    .await?;
                Self::result(response).await.map(drop)
            };
            match send(true).await {
                Err(e) if e.is_bad_markdown() => send(false).await,
                other => other,
            }
        }
    }

    /// Stops the spinner on a button that was pressed.
    pub(crate) async fn answer_callback_query(&self, id: &str) -> Result<(), BotApiError> {
        self.call("answerCallbackQuery", &json!({ "callback_query_id": id }))
            .await
            .map(drop)
    }

    /// Takes the buttons away from one of the bot's messages.
    pub(crate) async fn remove_buttons(
        &self,
        chat_id: &str,
        message_id: i64,
    ) -> Result<(), BotApiError> {
        self.call(
            "editMessageReplyMarkup",
            &json!({ "chat_id": chat_id, "message_id": message_id }),
        )
        .await
        .map(drop)
    }

    /// Downloads a file someone sent.
    ///
    /// A message carries only the file's id: `getFile` turns it into a path,
    /// valid for an hour, which is then fetched. Bots can download files of up
    /// to 20 MB.
    pub(crate) async fn download(
        &self,
        file_id: &str,
        mime_type: &str,
    ) -> Result<ChatMedia, BotApiError> {
        #[cfg(feature = "airtight")]
        return Ok(ChatMedia {
            mime_type: mime_type.to_string(),
            bytes: Vec::new(),
        });

        #[cfg(not(feature = "airtight"))]
        {
            let file = self.call("getFile", &json!({ "file_id": file_id })).await?;
            let path = file["file_path"].as_str().ok_or_else(|| BotApiError {
                description: format!("Telegram gave no path for file '{file_id}'"),
            })?;
            let response = CLIENT
                .get(format!(
                    "https://api.telegram.org/file/bot{}/{path}",
                    self.token
                ))
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(BotApiError {
                    description: format!(
                        "downloading file '{file_id}': HTTP {}",
                        response.status()
                    ),
                });
            }
            Ok(ChatMedia {
                mime_type: mime_type.to_string(),
                bytes: response.bytes().await?.to_vec(),
            })
        }
    }
}
