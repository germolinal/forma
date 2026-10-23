use std::fmt::Display;

use genai_client::{
    AssistantMessage, GenaiMessageTrait, LLMMessage, Role, UserMessage,
    openai::v1::{
        OpenAIDeveloperMessage, OpenAIMessageContent, OpenAIMessagePart, OpenAISystemMessage,
        OpenAIToolMessage,
    },
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct AiSdkV5Message {
    parts: Vec<OpenAIMessagePart>,
    role: Role,
    id: String,
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

impl From<AiSdkV5Message> for LLMMessage {
    fn from(value: AiSdkV5Message) -> Self {
        let content = OpenAIMessageContent::Parts(value.parts);
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
            parts,
            ..Default::default()
        }
    }
}
