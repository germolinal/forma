use genai_client::openai::v1::{OpenAIMessagePart, OpenAITextPart};
use protocols::ChatMessageState;
use serde::{Deserialize, Serialize};

use super::other_types::{TextBody, WhatsappContent, WhatsappMessageContext, WhatsappMsgStatus};

impl From<WhatsappMsgStatus> for ChatMessageState {
    fn from(value: WhatsappMsgStatus) -> Self {
        match value {
            WhatsappMsgStatus::Delivered => Self::Delivered,
            WhatsappMsgStatus::Failed => Self::Failed,
            WhatsappMsgStatus::Read => Self::Read,
            WhatsappMsgStatus::Sent => Self::Sent,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub(crate) struct WhatsappIncomingMessage {
    pub(crate) from: String,
    pub(crate) id: String,
    pub(crate) timestamp: String,
    #[serde(flatten)]
    pub(crate) content: WhatsappContent,
    /// Only included if message via a "Message business" button.
    #[serde(default)]
    pub(crate) context: WhatsappMessageContext,
    /// This is not part of the official Whatsapp API,
    /// but it is still compatible and it will help render
    /// messages in UIs
    #[serde(default)]
    pub(crate) reactions: Vec<String>,
    /// Also not part of the official API, but still compatible
    /// and helps rendering.
    #[serde(default)]
    pub(crate) status: WhatsappMsgStatus,
}

impl From<&TextBody> for OpenAIMessagePart {
    fn from(value: &TextBody) -> Self {
        OpenAIMessagePart::Text(OpenAITextPart {
            text: value.body.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_translation() {
        assert_eq!(
            Into::<ChatMessageState>::into(WhatsappMsgStatus::Delivered),
            ChatMessageState::Delivered
        );
        assert_eq!(
            Into::<ChatMessageState>::into(WhatsappMsgStatus::Failed),
            ChatMessageState::Failed
        );
        assert_eq!(
            Into::<ChatMessageState>::into(WhatsappMsgStatus::Read),
            ChatMessageState::Read
        );
        assert_eq!(
            Into::<ChatMessageState>::into(WhatsappMsgStatus::Sent),
            ChatMessageState::Sent
        );
    }
}
