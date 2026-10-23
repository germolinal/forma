// https://core.telegram.org/bots/api#available-types
//
// Only what the adapter reads: serde skips every other field.

use serde::{Deserialize, Serialize};

/// A chat: a private one with a person, or a group.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramChat {
    pub(crate) id: i64,
}

/// A Telegram user (or bot).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramUser {
    pub(crate) id: i64,
}

/// One size of a photo. A photo arrives in several, smallest first.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramPhotoSize {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) width: u32,
    #[serde(default)]
    pub(crate) height: u32,
}

/// A voice note, an audio file or a document: a file with a media type,
/// which Telegram does not always state.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramFile {
    pub(crate) file_id: String,
    #[serde(default)]
    pub(crate) mime_type: Option<String>,
    /// Audio files and documents only.
    #[serde(default)]
    pub(crate) file_name: Option<String>,
}

/// A message. Inside a callback query, it is the bot's message that carried
/// the buttons, which may be one Telegram no longer gives the content of
/// (`MaybeInaccessibleMessage`): its chat and id are always there.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramMessage {
    pub(crate) message_id: i64,
    pub(crate) chat: TelegramChat,
    #[serde(default)]
    pub(crate) text: Option<String>,
    /// What goes with a photo, an audio file or a document.
    #[serde(default)]
    pub(crate) caption: Option<String>,
    #[serde(default)]
    pub(crate) photo: Vec<TelegramPhotoSize>,
    #[serde(default)]
    pub(crate) voice: Option<TelegramFile>,
    #[serde(default)]
    pub(crate) audio: Option<TelegramFile>,
    #[serde(default)]
    pub(crate) document: Option<TelegramFile>,
}

/// A press on one of the bot's buttons.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramCallbackQuery {
    /// Must be answered (`answerCallbackQuery`), or the button keeps spinning.
    pub(crate) id: String,
    pub(crate) from: TelegramUser,
    /// The message the button was on.
    #[serde(default)]
    pub(crate) message: Option<TelegramMessage>,
    /// The button's id.
    #[serde(default)]
    pub(crate) data: Option<String>,
}

/// What Telegram posts to the webhook: at most one of its fields is set.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TelegramUpdate {
    pub(crate) update_id: i64,
    #[serde(default)]
    pub(crate) message: Option<TelegramMessage>,
    #[serde(default)]
    pub(crate) callback_query: Option<TelegramCallbackQuery>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_message() {
        let update: TelegramUpdate = serde_json::from_str(
            r#"{
                "update_id": 123456789,
                "message": {
                    "message_id": 1,
                    "date": 1234567890,
                    "chat": {"id": 123456789, "type": "private", "first_name": "Ana"},
                    "from": {"id": 123456789, "is_bot": false, "first_name": "Ana"},
                    "text": "Hello bot"
                }
            }"#,
        )
        .unwrap();
        let message = update.message.unwrap();
        assert_eq!(message.text.as_deref(), Some("Hello bot"));
        assert_eq!(message.chat.id, 123456789);
        assert!(update.callback_query.is_none());
    }

    #[test]
    fn a_photo_with_a_caption() {
        let update: TelegramUpdate = serde_json::from_str(
            r#"{
                "update_id": 1,
                "message": {
                    "message_id": 2, "date": 0, "chat": {"id": 7, "type": "private"},
                    "photo": [
                        {"file_id": "small", "file_unique_id": "a", "width": 90, "height": 60},
                        {"file_id": "large", "file_unique_id": "b", "width": 1280, "height": 853}
                    ],
                    "caption": "What is this?"
                }
            }"#,
        )
        .unwrap();
        let message = update.message.unwrap();
        assert_eq!(message.photo.len(), 2);
        assert_eq!(message.caption.as_deref(), Some("What is this?"));
    }

    #[test]
    fn a_voice_note_and_a_document() {
        let update: TelegramUpdate = serde_json::from_str(
            r#"{
                "update_id": 1,
                "message": {
                    "message_id": 3, "date": 0, "chat": {"id": 7, "type": "private"},
                    "voice": {"file_id": "v", "file_unique_id": "v1", "duration": 3, "mime_type": "audio/ogg"}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            update.message.unwrap().voice.unwrap().mime_type.as_deref(),
            Some("audio/ogg")
        );

        let update: TelegramUpdate = serde_json::from_str(
            r#"{
                "update_id": 2,
                "message": {
                    "message_id": 4, "date": 0, "chat": {"id": 7, "type": "private"},
                    "document": {"file_id": "d", "file_unique_id": "d1", "file_name": "report.pdf", "mime_type": "application/pdf"}
                }
            }"#,
        )
        .unwrap();
        let document = update.message.unwrap().document.unwrap();
        assert_eq!(document.file_name.as_deref(), Some("report.pdf"));
    }

    #[test]
    fn a_button_press_on_a_message_telegram_no_longer_shows() {
        // An inaccessible message only keeps its chat, its id, and a zero date.
        let update: TelegramUpdate = serde_json::from_str(
            r#"{
                "update_id": 123456790,
                "callback_query": {
                    "id": "query_id_123",
                    "chat_instance": "-1",
                    "from": {"id": 123456789, "is_bot": false, "first_name": "Ana"},
                    "message": {"message_id": 2, "date": 0, "chat": {"id": 123456789, "type": "private"}},
                    "data": "approve|call-1"
                }
            }"#,
        )
        .unwrap();
        let press = update.callback_query.unwrap();
        assert_eq!(press.data.as_deref(), Some("approve|call-1"));
        assert_eq!(press.message.unwrap().chat.id, 123456789);
    }

    #[test]
    fn an_update_the_adapter_does_not_read() {
        let update: TelegramUpdate = serde_json::from_str(
            r#"{"update_id": 999, "edited_message": {"message_id": 1, "date": 0, "chat": {"id": 7, "type": "private"}, "text": "edited"}}"#,
        )
        .unwrap();
        assert!(update.message.is_none());
        assert!(update.callback_query.is_none());
    }
}
