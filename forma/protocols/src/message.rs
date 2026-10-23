use std::collections::BTreeMap;
use std::fmt::{Debug, Display};

use chrono::{DateTime, Utc};
use genai_client::{
    AssistantMessage, AudioFormat, GenaiMessageTrait, LLMMessage, OpenAIDeveloperMessage,
    OpenAISystemMessage, OpenAIToolMessage, Role, SSEvent, UserMessage,
    openai::v1::{
        OpenAIAudioPart, OpenAIChatMessage, OpenAIFile, OpenAIFilePart, OpenAIFunctionToolChoice,
        OpenAIImagePart, OpenAIImageUrl, OpenAIInputAudio, OpenAIMessageContent, OpenAIMessagePart,
        OpenAITextPart, OpenAIToolChoice, OpenAIVisionDetail,
    },
};
use http::StatusCode;

use crate::FormaError;
use serde::{Deserialize, Serialize};

/// State of a message, as reflected in the Database
#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageState {
    /// The message has been sent
    #[default]
    Sent,
    /// The message has been delivered and not read
    /// by everyone in the conversation
    Delivered,
    /// The message has been read by everyone
    Read,
    /// There was some error delivering the message
    Failed,
}

/// Represents a text part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaTextPart {
    /// The text content of the part.
    pub text: String,
}
impl From<String> for FormaTextPart {
    fn from(item: String) -> Self {
        Self { text: item }
    }
}
/// Represents a URL of an image part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaImageUrl {
    /// The URL of the image.
    pub url: String,
    /// The detail level of the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<OpenAIVisionDetail>,
}
/// Represents an image part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaImagePart {
    /// The URL of the image.
    pub image_url: FormaImageUrl,
}

/// Represents audio data for an input audio part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaInputAudio {
    /// Base64 encoded audio data.
    pub data: String,
    /// The format of the encoded audio data. Currently supports "wav" and "mp3".
    pub format: AudioFormat,
}

/// Represents an audio part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaAudioPart {
    /// The audio data for the part.
    pub input_audio: FormaInputAudio,
}

/// Represents a file part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaFile {
    /// The base64 encoded file data, used when passing the file to the model as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<String>,
    /// The ID of an uploaded file to use as input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// The name of the file, used when passing the file to the model as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Represents a file part of a message.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaFilePart {
    /// The file data for the part.
    pub file: FormaFile,
}

/// A file in a workspace, named rather than carried.
///
/// This is what an `attach` puts into a conversation: the bytes are fetched
/// and inlined only when a request is built, so a stored session stays small
/// (a 10 MiB PDF as base64 would not even fit one Mongo document) and the
/// conversation keeps working when the file behind it changes.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FormaWorkspaceFilePart {
    /// The workspace the file belongs to.
    pub workspace_id: String,
    /// The file's path inside that workspace.
    pub path: String,
    /// What the file was when it was attached, as the store reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// One piece of a message, as Forma keeps it.
///
/// The provider-facing parts are mirrored one to one, with the same serde tags
/// and field names as [`OpenAIMessagePart`], so every message stored before
/// this type existed reads back unchanged. What this adds is the parts that
/// only mean something inside Forma, and that have to be resolved into
/// provider-facing ones before a request goes out — see
/// [`TryFrom<FormaMessagePart>`](OpenAIMessagePart).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum FormaMessagePart {
    /// Plain text.
    Text(FormaTextPart),
    /// An image, as a data URL or a URL the provider can reach.
    ImageUrl(FormaImagePart),
    /// A recording, base64 with its format named.
    InputAudio(FormaAudioPart),
    /// A file, base64 or by provider-side id.
    File(FormaFilePart),
    /// A file in a workspace, by reference. Never reaches a provider as it
    /// is: resolving it puts the bytes in an `ImageUrl` or `File` part.
    WorkspaceFile(FormaWorkspaceFilePart),
}

impl From<String> for FormaMessagePart {
    fn from(item: String) -> Self {
        Self::Text(item.into())
    }
}

impl From<&FormaMessagePart> for String {
    fn from(item: &FormaMessagePart) -> Self {
        match item {
            FormaMessagePart::Text(t) => t.text.clone(),
            FormaMessagePart::ImageUrl(_) => "[image-url]".to_string(),
            FormaMessagePart::InputAudio(_) => "[input-audio]".to_string(),
            FormaMessagePart::File(_) => "[file]".to_string(),
            FormaMessagePart::WorkspaceFile(f) => format!("[workspace-file {}]", f.path),
        }
    }
}

impl From<OpenAIMessagePart> for FormaMessagePart {
    fn from(part: OpenAIMessagePart) -> Self {
        match part {
            OpenAIMessagePart::Text(p) => Self::Text(FormaTextPart { text: p.text }),
            OpenAIMessagePart::ImageUrl(p) => Self::ImageUrl(FormaImagePart {
                image_url: FormaImageUrl {
                    url: p.image_url.url,
                    detail: p.image_url.detail,
                },
            }),
            OpenAIMessagePart::InputAudio(p) => Self::InputAudio(FormaAudioPart {
                input_audio: FormaInputAudio {
                    data: p.input_audio.data,
                    format: p.input_audio.format,
                },
            }),
            OpenAIMessagePart::File(p) => Self::File(FormaFilePart {
                file: FormaFile {
                    file_data: p.file.file_data,
                    file_id: p.file.file_id,
                    filename: p.file.filename,
                },
            }),
        }
    }
}

/// Turns a part into the shape a provider is sent.
///
/// Fallible on purpose: a [`FormaMessagePart::WorkspaceFile`] carries no bytes,
/// so a request built without resolving it first would otherwise reach a
/// provider naming a file it cannot read. This way that bug fails loudly, at
/// the boundary, instead of turning into a puzzling provider error.
impl TryFrom<FormaMessagePart> for OpenAIMessagePart {
    type Error = FormaError;

    fn try_from(part: FormaMessagePart) -> Result<Self, Self::Error> {
        Ok(match part {
            FormaMessagePart::Text(p) => Self::Text(OpenAITextPart { text: p.text }),
            FormaMessagePart::ImageUrl(p) => Self::ImageUrl(OpenAIImagePart {
                image_url: OpenAIImageUrl {
                    url: p.image_url.url,
                    detail: p.image_url.detail,
                },
            }),
            FormaMessagePart::InputAudio(p) => Self::InputAudio(OpenAIAudioPart {
                input_audio: OpenAIInputAudio {
                    data: p.input_audio.data,
                    format: p.input_audio.format,
                },
            }),
            FormaMessagePart::File(p) => Self::File(OpenAIFilePart {
                file: OpenAIFile {
                    file_data: p.file.file_data,
                    file_id: p.file.file_id,
                    filename: p.file.filename,
                },
            }),
            FormaMessagePart::WorkspaceFile(f) => {
                return Err(FormaError::Execution(format!(
                    "workspace file '{}' reached the provider unresolved; its contents were never fetched",
                    f.path
                )));
            }
        })
    }
}

/// Takes in a whole message's worth of provider-facing parts.
pub fn from_provider_parts(parts: Vec<OpenAIMessagePart>) -> Vec<FormaMessagePart> {
    parts.into_iter().map(FormaMessagePart::from).collect()
}

/// Converts a whole message's worth of parts, failing on the first that cannot
/// be sent as it is.
pub fn into_provider_parts(
    parts: Vec<FormaMessagePart>,
) -> Result<Vec<OpenAIMessagePart>, FormaError> {
    parts.into_iter().map(OpenAIMessagePart::try_from).collect()
}

/// Role-dependent content of a message
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "role")]
#[serde(rename_all = "snake_case")]
pub enum FormaMessageBody {
    /// The message was sent by the user
    User {
        /// The content of the message
        content: Vec<FormaMessagePart>,
    },
    /// The message was sent by the developer
    Developer {
        /// The content of the message
        content: Vec<FormaMessagePart>,
    },
    /// The message was sent by the system
    System {
        /// The content of the message
        content: Vec<FormaMessagePart>,
    },
    /// The message was sent by the assistant
    Assistant {
        /// The content of the message
        content: Vec<FormaMessagePart>,
        /// The tool calls proposed by the assistant
        tool_calls: Vec<OpenAIToolChoice>,
        /// The thought signature of the part.
        thought_signature: Option<bytes::Bytes>,
        /// The bundle of each tool call that has one, by tool call ID. Only
        /// there so a reopened session renders its tool calls as the live
        /// stream did; never sent to the LLM.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        tool_bundles: BTreeMap<String, String>,
        /// What each tool call's tool is (see `SSEvent::ToolInputStart`), by
        /// tool call ID, for the same reason.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        tool_types: BTreeMap<String, String>,
    },
    /// Message contains information about the result of a tool call
    Tool {
        /// The ID of the tool call
        tool_call_id: String,
        /// The result of the tool call
        content: Vec<FormaMessagePart>,
    },
}

impl Default for FormaMessageBody {
    fn default() -> Self {
        Self::User { content: vec![] }
    }
}

impl FormaMessageBody {
    /// Checks if the message is empty
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Assistant {
                content,
                tool_calls,
                thought_signature,
                ..
            } => content.is_empty() && tool_calls.is_empty() && thought_signature.is_none(),
            _ => self.content().is_empty(),
        }
    }

    /// Gathers the content of the message as a slice of [`FormaMessagePart`]
    pub fn content(&self) -> &[FormaMessagePart] {
        match self {
            Self::User { content } => content,
            Self::System { content } => content,
            Self::Developer { content } => content,
            Self::Assistant { content, .. } => content,
            Self::Tool { content, .. } => content,
        }
    }

    /// The content of the message, to change in place: what resolving a part
    /// that names something into one that carries it needs.
    pub fn content_mut(&mut self) -> &mut Vec<FormaMessagePart> {
        match self {
            Self::User { content } => content,
            Self::System { content } => content,
            Self::Developer { content } => content,
            Self::Assistant { content, .. } => content,
            Self::Tool { content, .. } => content,
        }
    }

    /// Gets the role of the message
    pub fn role(&self) -> Role {
        match self {
            Self::User { .. } => Role::User,
            Self::System { .. } => Role::System,
            Self::Developer { .. } => Role::Developer,
            Self::Assistant { .. } => Role::Assistant,
            Self::Tool { .. } => Role::Tool,
        }
    }
}

/// The ID of a message
#[derive(Debug, Serialize, Deserialize, Clone, Default, Eq, PartialEq)]
pub struct MessageId(pub String);

impl Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for MessageId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A user, tool, assistant, or developer message
pub type FormaMessage = GenericFormaMessage<DateTime<Utc>>;

/// A user, tool, assistant, or developer message, generic
/// over the potential DB id and date types
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenericFormaMessage<DATE> {
    /// The content of the message
    #[serde(flatten)]
    pub content: FormaMessageBody,

    /// The ID of the message itslf
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<MessageId>,

    /// The user that owns this message
    pub user_id: String,
    /// The session this message belongs to
    pub session_id: String,

    /// The time of creation of this
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DATE>,

    /// A collection of all the reactions
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub reactions: Vec<String>,
    /// The state of the message
    #[serde(default)]
    pub status: ChatMessageState,
}

impl<DATE> GenericFormaMessage<DATE> {
    /// Generates a new empty User message
    pub fn new_user() -> Self {
        Self {
            content: FormaMessageBody::User { content: vec![] },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }

    /// Generates a new empty Assistant message
    pub fn new_assistant() -> Self {
        Self {
            content: FormaMessageBody::Assistant {
                content: vec![],
                tool_calls: Vec::with_capacity(3),
                thought_signature: None,
                tool_bundles: BTreeMap::new(),
                tool_types: BTreeMap::new(),
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }
    /// Generates a new empty Tool message
    pub fn new_tool() -> Self {
        Self {
            content: FormaMessageBody::Tool {
                tool_call_id: String::new(),
                content: vec![],
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }

    /// Generates a new Tool message with the given content
    pub fn new_tool_with_content<I: Into<FormaMessagePart>>(
        tool_call_id: &str,
        content: I,
    ) -> Self {
        Self {
            content: FormaMessageBody::Tool {
                tool_call_id: tool_call_id.to_string(),
                content: vec![content.into()],
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }

    /// Generates a new empty User message
    pub fn new_user_with_content<I: Into<FormaMessagePart>>(content: I) -> Self {
        Self::new_user_with_parts(vec![content.into()])
    }

    /// Generates a User message from several content parts, as a channel that
    /// carries a picture and a caption in one turn produces.
    pub fn new_user_with_parts(content: Vec<FormaMessagePart>) -> Self {
        Self {
            content: FormaMessageBody::User { content },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }
    /// Generates a new empty User message
    pub fn new_system_with_content<I: Into<FormaMessagePart>>(content: I) -> Self {
        Self {
            content: FormaMessageBody::System {
                content: vec![content.into()],
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }

    /// Generates a new empty Assistant message with content
    pub fn new_assistant_with_content<I: Into<FormaMessagePart>>(content: I) -> Self {
        Self::new_assistant_with_parts(vec![content.into()])
    }

    /// Generates an Assistant message from several content parts.
    pub fn new_assistant_with_parts(content: Vec<FormaMessagePart>) -> Self {
        Self {
            content: FormaMessageBody::Assistant {
                content,
                tool_calls: Vec::with_capacity(3),
                thought_signature: None,
                tool_bundles: BTreeMap::new(),
                tool_types: BTreeMap::new(),
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }

    /// Retrieves the ID of the message. Returns an Internal Server error
    /// if the session does not have na ID
    pub fn id_string(&self) -> Result<String, (StatusCode, String)> {
        match &self.id {
            Some(id) => Ok(id.to_string()),
            None => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "when retrieving Whatsapp message: message has no id".to_string(),
            )),
        }
    }

    /// Checks if the message is empty (i.e. has no content or tool calls)
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

impl<DATE> From<String> for GenericFormaMessage<DATE> {
    fn from(value: String) -> Self {
        Self {
            content: FormaMessageBody::User {
                content: vec![FormaMessagePart::Text(FormaTextPart { text: value })],
            },
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
        }
    }
}

/// Takes in a message as an OpenAI-compatible caller sent it.
impl<DATE> From<OpenAIChatMessage> for GenericFormaMessage<DATE>
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn from(value: OpenAIChatMessage) -> Self {
        LLMMessage::from(value).into()
    }
}

impl<DATE> From<LLMMessage> for GenericFormaMessage<DATE>
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn from(value: LLMMessage) -> Self {
        let mut ret = Self {
            id: None,
            user_id: String::default(),
            session_id: String::default(),
            created_at: None,
            reactions: Vec::with_capacity(0),
            status: ChatMessageState::default(),
            content: FormaMessageBody::default(),
        };
        match value {
            LLMMessage::System(m) => {
                ret.content = FormaMessageBody::System {
                    content: from_provider_parts(m.content.as_parts()),
                };
            }
            LLMMessage::User(m) => {
                ret.content = FormaMessageBody::User {
                    content: from_provider_parts(m.content),
                };
            }
            LLMMessage::Developer(m) => {
                ret.content = FormaMessageBody::Developer {
                    content: from_provider_parts(m.content.as_parts()),
                };
            }
            LLMMessage::Assistant(m) => {
                ret.content = FormaMessageBody::Assistant {
                    content: from_provider_parts(m.content),
                    tool_calls: m.tool_calls,
                    thought_signature: None,
                    tool_bundles: BTreeMap::new(),
                    tool_types: BTreeMap::new(),
                };
            }
            LLMMessage::Tool(m) => {
                ret.content = FormaMessageBody::Tool {
                    content: from_provider_parts(m.content.as_parts()),
                    tool_call_id: m.tool_call_id,
                };
            }
        };
        ret
    }
}

/// Builds the message a provider is sent.
///
/// Fallible because a part may still name something instead of carrying it —
/// see [`TryFrom<FormaMessagePart>`](OpenAIMessagePart).
impl<DATE> TryFrom<GenericFormaMessage<DATE>> for LLMMessage
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    type Error = FormaError;

    fn try_from(value: GenericFormaMessage<DATE>) -> Result<Self, Self::Error> {
        Ok(match value.content {
            FormaMessageBody::System { content } => Self::System(OpenAISystemMessage {
                content: OpenAIMessageContent::Parts(into_provider_parts(content)?),
                name: None,
            }),
            FormaMessageBody::User { content } => Self::User(UserMessage {
                content: into_provider_parts(content)?,
                name: None,
            }),
            FormaMessageBody::Developer { content } => Self::Developer(OpenAIDeveloperMessage {
                content: OpenAIMessageContent::Parts(into_provider_parts(content)?),
            }),
            FormaMessageBody::Assistant {
                content,
                tool_calls,
                thought_signature,
                ..
            } => Self::Assistant(AssistantMessage {
                content: into_provider_parts(content)?,
                tool_calls,
                thought_signature,
                ..Default::default()
            }),
            FormaMessageBody::Tool {
                tool_call_id,
                content,
            } => Self::Tool(OpenAIToolMessage {
                tool_call_id,
                content: OpenAIMessageContent::Parts(into_provider_parts(content)?),
            }),
        })
    }
}

impl From<&FormaMessageBody> for String {
    fn from(val: &FormaMessageBody) -> Self {
        let strings: Vec<String> = val.content().iter().map(|x| x.into()).collect();
        strings.join("\n")
    }
}

impl std::fmt::Display for FormaMessageBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for x in self.content().iter() {
            let s: String = x.into();
            writeln!(f, "{s}")?;
        }

        Ok(())
    }
}

impl<DATE> From<&GenericFormaMessage<DATE>> for String
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn from(val: &GenericFormaMessage<DATE>) -> Self {
        (&val.content).into()
    }
}

impl<DATE> std::fmt::Display for GenericFormaMessage<DATE>
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content)
    }
}

impl<DATE> GenaiMessageTrait for GenericFormaMessage<DATE>
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn role(&self) -> Role {
        self.content.role()
    }
    fn content_string(&self) -> Option<String> {
        if self.content.is_empty() {
            return None;
        }

        Some(self.to_string())
    }
    fn has_content(&self) -> bool {
        !self.content.is_empty()
    }
}

impl std::ops::AddAssign<SSEvent> for FormaMessageBody {
    fn add_assign(&mut self, event: SSEvent) {
        if let FormaMessageBody::Assistant {
            content,
            tool_calls,
            tool_bundles,
            tool_types,
            ..
        } = self
        {
            match event {
                SSEvent::ToolInputStart {
                    tool_call_id,
                    tool_type,
                    bundle,
                    ..
                } => {
                    if !tool_type.is_empty() {
                        tool_types.insert(tool_call_id.clone(), tool_type);
                    }
                    if let Some(bundle) = bundle {
                        tool_bundles.insert(tool_call_id, bundle);
                    }
                }
                SSEvent::TextStart { .. } => {
                    let empty_part = FormaMessagePart::Text(FormaTextPart::default());
                    content.push(empty_part);
                }
                SSEvent::TextDelta { delta, .. } => {
                    let mut appended = false;
                    for p in content.iter_mut().rev() {
                        if let FormaMessagePart::Text(FormaTextPart { text }) = p {
                            text.push_str(&delta);
                            appended = true;
                            break;
                        }
                    }
                    if !appended {
                        panic!("trying to append text to a message that has no text parts")
                    }
                }
                // Kept as a real content part rather than a note about one, so
                // the document is still in the history the next turn is built
                // from and the model can be asked to revise it.
                SSEvent::FileAvailable { part, .. } => {
                    content.push(part.into());
                }
                SSEvent::ToolInputAvailable {
                    tool_call_id,
                    tool_name,
                    input,
                    ..
                } => {
                    let arguments =
                        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                    tool_calls.push(OpenAIToolChoice::Function {
                        id: tool_call_id,
                        function: OpenAIFunctionToolChoice {
                            name: tool_name,
                            arguments,
                        },
                    });
                }
                _ => { /* Nothing to do about these */ }
            }
        } else {
            unreachable!("adding event to non-assistant messages is not supported ")
        }
    }
}

impl<DATE> std::ops::AddAssign<SSEvent> for GenericFormaMessage<DATE>
where
    DATE: Serialize + for<'a> Deserialize<'a> + Clone + Debug,
{
    fn add_assign(&mut self, event: SSEvent) {
        self.content += event;
    }
}

#[cfg(test)]
mod testing {
    use super::*;
    use genai_client::openai::v1::OpenAIFunctionToolChoice;

    #[test]
    fn test_is_empty() {
        let mut msg = FormaMessage::new_user();
        assert!(msg.is_empty());

        msg.content = FormaMessageBody::User {
            content: vec![FormaMessagePart::Text(FormaTextPart {
                text: "hello".to_string(),
            })],
        };
        assert!(!msg.is_empty());

        let assistant_msg = FormaMessage::new_assistant();
        assert!(assistant_msg.is_empty());

        let mut assistant_msg_with_tool = FormaMessage::new_assistant();
        if let FormaMessageBody::Assistant { tool_calls, .. } = &mut assistant_msg_with_tool.content
        {
            tool_calls.push(OpenAIToolChoice::Function {
                id: "123".to_string(),
                function: OpenAIFunctionToolChoice::default(),
            });
        }
        assert!(!assistant_msg_with_tool.is_empty());
    }

    #[test]
    fn test_new_methods() {
        let user = FormaMessage::new_user();
        assert_eq!(user.role(), Role::User);
        assert!(user.is_empty());

        let assistant = FormaMessage::new_assistant();
        assert_eq!(assistant.role(), Role::Assistant);
        assert!(assistant.is_empty());

        let user_content = FormaMessage::new_user_with_content("hello".to_string());
        assert_eq!(user_content.role(), Role::User);
        assert!(!user_content.is_empty());

        let system_content = FormaMessage::new_system_with_content("system".to_string());
        assert_eq!(system_content.role(), Role::System);
        assert!(!system_content.is_empty());

        let assistant_content = FormaMessage::new_assistant_with_content("assistant".to_string());
        assert_eq!(assistant_content.role(), Role::Assistant);
        assert!(!assistant_content.is_empty());
    }

    #[test]
    fn test_id_string() {
        let mut msg = FormaMessage::new_user();
        assert!(msg.id_string().is_err());

        let id: MessageId = String::from("message id").into();
        msg.id = Some(id.clone());
        assert_eq!(msg.id_string().unwrap(), id.to_string());
    }

    #[test]
    fn test_from_string() {
        let msg = FormaMessage::from("hello".to_string());
        assert_eq!(msg.role(), Role::User);
        assert_eq!(msg.content_string(), Some("hello\n".to_string()));
    }

    #[test]
    fn test_conversions_llm_message() {
        // Test System
        let system_llm = LLMMessage::System(OpenAISystemMessage {
            content: OpenAIMessageContent::String("sys".to_string()),
            name: None,
        });
        let system_forma = FormaMessage::from(system_llm.clone());
        assert_eq!(system_forma.role(), Role::System);
        let back_llm: LLMMessage = system_forma
            .try_into()
            .expect("parts are all provider-ready");
        assert!(matches!(back_llm, LLMMessage::System(_)));

        // Test User
        let user_llm = LLMMessage::User(UserMessage {
            content: vec!["usr".to_string().into()],
            name: None,
        });
        let user_forma = FormaMessage::from(user_llm.clone());
        assert_eq!(user_forma.role(), Role::User);
        let back_llm: LLMMessage = user_forma.try_into().expect("parts are all provider-ready");
        assert!(matches!(back_llm, LLMMessage::User(_)));

        // Test Developer
        let dev_llm = LLMMessage::Developer(OpenAIDeveloperMessage {
            content: OpenAIMessageContent::String("dev".to_string()),
        });
        let dev_forma = FormaMessage::from(dev_llm.clone());
        assert_eq!(dev_forma.role(), Role::Developer);
        let back_llm: LLMMessage = dev_forma.try_into().expect("parts are all provider-ready");
        assert!(matches!(back_llm, LLMMessage::Developer(_)));

        // Test Assistant
        let assistant_llm = LLMMessage::Assistant(AssistantMessage {
            content: vec!["ast".to_string().into()],
            tool_calls: vec![],
            ..Default::default()
        });
        let assistant_forma = FormaMessage::from(assistant_llm.clone());
        assert_eq!(assistant_forma.role(), Role::Assistant);
        let back_llm: LLMMessage = assistant_forma
            .try_into()
            .expect("parts are all provider-ready");
        assert!(matches!(back_llm, LLMMessage::Assistant(_)));

        // Test Tool
        let tool_llm = LLMMessage::Tool(OpenAIToolMessage {
            tool_call_id: "id".to_string(),
            content: OpenAIMessageContent::String("res".to_string()),
        });
        let tool_forma = FormaMessage::from(tool_llm.clone());
        assert_eq!(tool_forma.role(), Role::Tool);
        let back_llm: LLMMessage = tool_forma.try_into().expect("parts are all provider-ready");
        assert!(matches!(back_llm, LLMMessage::Tool(_)));
    }

    #[test]
    fn test_display_and_string_conversions() {
        let msg = FormaMessage::new_user_with_content("hello world".to_string());
        assert_eq!(msg.to_string(), "hello world\n");
        let s: String = (&msg).into();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn test_add_assign_ssevent() {
        let mut msg = FormaMessage::new_assistant();

        msg += SSEvent::TextStart {
            id: "123".to_string(),
        };
        msg += SSEvent::TextDelta {
            id: "123".to_string(),
            delta: "hello ".to_string(),
        };
        msg += SSEvent::TextDelta {
            id: "123".to_string(),
            delta: "world".to_string(),
        };

        assert_eq!(msg.content_string(), Some("hello world\n".to_string()));
    }

    #[test]
    #[should_panic(expected = "adding event to non-assistant messages is not supported")]
    fn test_add_assign_ssevent_panic() {
        let mut msg = FormaMessage::new_user();
        msg += SSEvent::TextStart {
            id: "123".to_string(),
        };
    }

    #[test]
    fn test_genai_message_trait() {
        let mut msg = FormaMessage::new_user();
        assert!(!msg.has_content());
        assert_eq!(msg.role(), Role::User);
        assert!(msg.content_string().is_none());

        msg.content = FormaMessageBody::User {
            content: vec!["hello".to_string().into()],
        };
        assert!(msg.has_content());
        assert_eq!(msg.content_string(), Some("hello\n".to_string()));
    }

    #[test]
    fn test_deserialize() {
        let json = r#"{
            "role":"assistant",
            "user_id":"123123",
            "session_id":"123123",
            "id":"12312k3jh",
            "content":[
                {
                    "type": "text",
                    "text":"Hello, how can I help you today?"
                }
            ],
            "tool_calls": [],
            "tool_call_id": ""
        }"#;
        let msg: FormaMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role(), Role::Assistant);
        assert_eq!(
            msg.content_string(),
            Some("Hello, how can I help you today?\n".to_string())
        );
        if let FormaMessageBody::Assistant { tool_calls, .. } = &msg.content {
            assert!(tool_calls.is_empty());
        }
    }

    /// Messages stored before Forma had parts of its own must still read back,
    /// so the wire shape of every provider-facing part is unchanged.
    #[test]
    fn test_parts_keep_their_stored_shape() {
        let json = r#"[
            {"type":"text","text":"look at this"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,iVBORw0KGgo="}},
            {"type":"file","file":{"file_data":"data:application/pdf;base64,JVBERi0=","filename":"a.pdf"}},
            {"type":"input_audio","input_audio":{"data":"AAAA","format":"wav"}}
        ]"#;
        let parts: Vec<FormaMessagePart> = serde_json::from_str(json).expect("reads back");
        assert!(matches!(parts[0], FormaMessagePart::Text(_)));
        assert!(matches!(parts[1], FormaMessagePart::ImageUrl(_)));
        assert!(matches!(parts[2], FormaMessagePart::File(_)));
        assert!(matches!(parts[3], FormaMessagePart::InputAudio(_)));

        // And each one still goes out as what it came in as.
        let out = into_provider_parts(parts).expect("all are provider-ready");
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }

    /// A workspace file names a file instead of carrying it, so it must never
    /// reach a provider: the conversion refuses, naming the file.
    #[test]
    fn test_unresolved_workspace_file_is_refused() {
        let part = FormaMessagePart::WorkspaceFile(FormaWorkspaceFilePart {
            workspace_id: "kb-1".to_string(),
            path: "invoices/march.pdf".to_string(),
            media_type: Some("application/pdf".to_string()),
        });
        // It survives a round trip through storage, under a tag of its own.
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains(r#""type":"workspace_file""#), "{json}");
        let back: FormaMessagePart = serde_json::from_str(&json).unwrap();

        let err = into_provider_parts(vec![back]).expect_err("must not reach a provider");
        assert!(
            err.to_string().contains("invoices/march.pdf"),
            "the error should name the file: {err}"
        );

        let msg = FormaMessage::new_user_with_parts(vec![part]);
        assert!(
            LLMMessage::try_from(msg).is_err(),
            "a message carrying one must be refused too"
        );
    }
}
