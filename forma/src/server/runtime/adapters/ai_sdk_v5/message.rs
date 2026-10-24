use std::fmt::Display;

use genai_client::{
    AssistantMessage, GenaiMessageTrait, LLMMessage, Role, UserMessage,
    openai::v1::{
        OpenAIDeveloperMessage, OpenAIFile, OpenAIFilePart, OpenAIImagePart, OpenAIImageUrl,
        OpenAIMessageContent, OpenAIMessagePart, OpenAISystemMessage, OpenAIToolMessage,
    },
};
use protocols::FormaError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::sse::file_part_url;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A message, as Vercel's AI SDK (v5) sends it (a `UIMessage`): an id, who
/// wrote it, and its parts.
pub struct AiSdkV5Message {
    /// What it says: the AI SDK's `text` and `file` parts, or OpenAI's
    /// (`image_url`, `input_audio`, `file`).
    parts: Vec<AiSdkV5Part>,
    /// Who wrote it: `user`, for what is sent to the agent.
    role: Role,
    /// The message's id. Required, and not kept.
    id: String,
}

/// One part of an AI SDK message.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum AiSdkV5Part {
    /// The AI SDK's own file part, which is how `useChat` sends attachments,
    /// and how Forma sends files back.
    File(AiSdkV5FilePart),
    /// One of OpenAI's parts. The AI SDK's `text` part is the same as
    /// OpenAI's.
    OpenAi(OpenAIMessagePart),
}

/// The tag of the AI SDK's file part.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum FileTag {
    /// A file.
    File,
}

/// The AI SDK's file part: a picture or a document, by URL.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiSdkV5FilePart {
    /// Always `file`.
    #[serde(rename = "type")]
    tag: FileTag,
    /// What the file is (`image/png`, `application/pdf`, ...).
    #[serde(rename = "mediaType")]
    media_type: String,
    /// A `data:` URL; or, for a picture, a URL the model's provider can
    /// reach. Other files must be inline: Forma does not fetch them.
    url: String,
    /// The file's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
}

impl AiSdkV5FilePart {
    /// The OpenAI part the agent receives: a picture as an image, anything
    /// else as an inline file.
    fn to_openai(&self) -> Result<OpenAIMessagePart, FormaError> {
        if self.media_type.starts_with("image/") {
            return Ok(OpenAIMessagePart::ImageUrl(OpenAIImagePart {
                image_url: OpenAIImageUrl {
                    url: self.url.clone(),
                    detail: None,
                },
            }));
        }
        if !self.url.starts_with("data:") {
            return Err(FormaError::Input(format!(
                "the file '{}' ({}) is a link: send files other than pictures inline, as a `data:` URL",
                self.filename.as_deref().unwrap_or("without a name"),
                self.media_type
            )));
        }
        Ok(OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(self.url.clone()),
                file_id: None,
                filename: self.filename.clone(),
            },
        }))
    }
}

impl AiSdkV5Part {
    fn to_openai(&self) -> Result<OpenAIMessagePart, FormaError> {
        match self {
            Self::File(f) => f.to_openai(),
            Self::OpenAi(p) => Ok(p.clone()),
        }
    }

    /// The part the AI SDK renders: files as its own file parts, text as it
    /// is.
    fn from_openai(part: OpenAIMessagePart) -> Self {
        if let OpenAIMessagePart::Text(_) = part {
            return Self::OpenAi(part);
        }
        let filename = match &part {
            OpenAIMessagePart::File(f) => f.file.filename.clone(),
            _ => None,
        };
        match file_part_url(&part) {
            Some((url, media_type)) => Self::File(AiSdkV5FilePart {
                tag: FileTag::File,
                media_type,
                url,
                filename,
            }),
            None => Self::OpenAi(part),
        }
    }
}

impl AiSdkV5Message {
    /// Checks that every part can reach the agent: a file other than a
    /// picture has to be inline.
    pub(crate) fn check(&self) -> Result<(), FormaError> {
        self.openai_parts().map(|_| ())
    }

    fn openai_parts(&self) -> Result<Vec<OpenAIMessagePart>, FormaError> {
        self.parts.iter().map(AiSdkV5Part::to_openai).collect()
    }
}

impl Default for AiSdkV5Message {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            role: Role::default(),
            id: Uuid::new_v4().to_string(),
        }
    }
}

impl From<&AiSdkV5Part> for String {
    fn from(part: &AiSdkV5Part) -> Self {
        match part {
            AiSdkV5Part::File(_) => "[file]".to_string(),
            AiSdkV5Part::OpenAi(p) => String::from(p),
        }
    }
}

impl Display for AiSdkV5Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.role)?;
        for m in self.parts.iter() {
            write!(f, "{}", String::from(m))?;
        }
        Ok(())
    }
}

impl GenaiMessageTrait for AiSdkV5Message {
    fn role(&self) -> Role {
        self.role
    }

    fn has_content(&self) -> bool {
        !self.parts.is_empty()
    }

    fn content_string(&self) -> Option<String> {
        let r: String = self.parts.iter().map(String::from).collect();
        Some(r)
    }
}

/// Takes in a message as the AI SDK sent it. Goes through [`LLMMessage`],
/// which is the shape this adapter already speaks.
impl From<AiSdkV5Message> for protocols::FormaMessage {
    fn from(value: AiSdkV5Message) -> Self {
        LLMMessage::from(value).into()
    }
}

/// Parts that cannot reach the agent are left out: [`AiSdkV5Message::check`]
/// refuses a message that has one before it gets here.
impl From<AiSdkV5Message> for LLMMessage {
    fn from(value: AiSdkV5Message) -> Self {
        let parts = value
            .parts
            .iter()
            .filter_map(|p| p.to_openai().ok())
            .collect();
        let content = OpenAIMessageContent::Parts(parts);
        match value.role {
            Role::System => Self::System(OpenAISystemMessage {
                content,
                name: None,
            }),
            Role::User => Self::User(UserMessage {
                content: content.as_parts(),
                name: None,
            }),
            Role::Developer => Self::Developer(OpenAIDeveloperMessage { content }),
            Role::Assistant => Self::Assistant(AssistantMessage {
                content: content.as_parts(),
                ..Default::default()
            }),
            Role::Tool => Self::Tool(OpenAIToolMessage {
                tool_call_id: String::new(),
                content,
            }),
        }
    }
}

impl From<LLMMessage> for AiSdkV5Message {
    fn from(value: LLMMessage) -> Self {
        let (parts, role) = match value {
            LLMMessage::System(msg) => (msg.content.as_parts(), Role::System),
            LLMMessage::User(msg) => (msg.content.clone(), Role::User),
            LLMMessage::Developer(msg) => (msg.content.as_parts(), Role::Developer),
            LLMMessage::Assistant(msg) => (msg.content.clone(), Role::Assistant),
            LLMMessage::Tool(msg) => (msg.content.as_parts(), Role::Tool),
        };

        Self {
            role,
            parts: parts.into_iter().map(AiSdkV5Part::from_openai).collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    fn user_parts(message: serde_json::Value) -> Vec<OpenAIMessagePart> {
        let message: AiSdkV5Message = serde_json::from_value(message).unwrap();
        message.check().unwrap();
        match LLMMessage::from(message) {
            LLMMessage::User(m) => m.content,
            other => panic!("not the user's: {other:?}"),
        }
    }

    #[test]
    fn the_ai_sdks_file_parts_reach_the_agent() {
        let parts = user_parts(json!({
            "id": "m1",
            "role": "user",
            "parts": [
                {"type": "text", "text": "What are these?"},
                {"type": "file", "mediaType": "image/png", "url": "https://example.com/a.png"},
                {"type": "file", "mediaType": "application/pdf", "url": "data:application/pdf;base64,JVBERi0=", "filename": "a.pdf"},
            ],
        }));
        assert!(matches!(&parts[0], OpenAIMessagePart::Text(t) if t.text == "What are these?"));
        assert!(
            matches!(&parts[1], OpenAIMessagePart::ImageUrl(p) if p.image_url.url == "https://example.com/a.png")
        );
        match &parts[2] {
            OpenAIMessagePart::File(f) => {
                assert_eq!(f.file.filename.as_deref(), Some("a.pdf"));
                assert!(f.file.file_data.as_deref().unwrap().starts_with("data:"));
            }
            other => panic!("not a file: {other:?}"),
        }
    }

    #[test]
    fn openais_parts_still_work() {
        let parts = user_parts(json!({
            "id": "m1",
            "role": "user",
            "parts": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}],
        }));
        assert!(matches!(&parts[0], OpenAIMessagePart::ImageUrl(_)));
    }

    #[test]
    fn a_linked_document_is_refused() {
        let message: AiSdkV5Message = serde_json::from_value(json!({
            "id": "m1",
            "role": "user",
            "parts": [{"type": "file", "mediaType": "application/pdf", "url": "https://example.com/a.pdf"}],
        }))
        .unwrap();
        assert!(matches!(message.check(), Err(FormaError::Input(_))));
    }

    #[test]
    fn files_go_back_as_the_ai_sdks_file_parts() {
        let answer = LLMMessage::Assistant(AssistantMessage {
            content: vec![
                OpenAIMessagePart::Text("Here".to_string().into()),
                OpenAIMessagePart::ImageUrl(OpenAIImagePart {
                    image_url: OpenAIImageUrl {
                        url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                        detail: None,
                    },
                }),
            ],
            ..Default::default()
        });
        let message = serde_json::to_value(AiSdkV5Message::from(answer)).unwrap();
        assert_eq!(
            message["parts"],
            json!([
                {"type": "text", "text": "Here"},
                {"type": "file", "mediaType": "image/png", "url": "data:image/png;base64,iVBORw0KGgo="},
            ])
        );
    }
}
