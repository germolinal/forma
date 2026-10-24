use crate::{
    AssistantMessage, AudioFormat, ByteStream, ChatResponse, GenaiClientTrait, JsonSchema,
    LLMChunk, LLMFinishReason, LLMMessage, MessageChoice, Role, UserMessage,
    genai_client_trait::{
        GenaiMessageTrait, GenaiRequestTrait, LLMProvider, post, post_stream, rest_request,
    },
};
use async_trait::async_trait;
use derive::FormaDocs;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};

/// The voice the model uses to respond. Supported voices are `alloy`,
/// `ash`, `ballad`, `coral`, `echo`, `fable`, `nova`, `onyx`,
/// `sage`, and `shimmer`.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone, FormaDocs)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIVoice {
    #[default]
    Allow,
    Ash,
    Ballad,
    Coral,
    Echo,
    Fable,
    Nova,
    Onyx,
    Sage,
    Shimer,
}

/// Output types that you would like the model to generate.
/// Most models are capable of generating text, which is the default:
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIModality {
    #[default]
    Text,
    Audio,
}
/// Parameters for audio output.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
pub struct OpenAIAudioSettings {
    pub format: AudioFormat,
    pub voice: OpenAIVoice,
}

/// Constrains effort on reasoning for reasoning models.
/// Currently supported values are `minimal`, `low`, `medium`, and `high`.
/// Reducing reasoning effort can result in faster responses
/// and fewer tokens used on reasoning in a response.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIReasoningEffort {
    #[default]
    Minimal,
    Low,
    Medium,
    High,
}

#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
pub struct OpenAIStreamOption {
    /// When true, stream obfuscation will be enabled. Stream
    /// obfuscation adds random characters to an obfuscation field
    /// on streaming delta events to normalize payload sizes as a
    /// mitigation to certain side-channel attacks. These obfuscation
    /// fields are included by default, but add a small amount of overhead
    /// to the data stream. You can set include_obfuscation to false to optimize
    /// for bandwidth if you trust the network links between your application
    /// and the OpenAI API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_obfuscation: Option<bool>,

    /// If set, an additional chunk will be streamed before the data: [DONE] message.
    /// The usage field on this chunk shows the token usage statistics for the entire
    /// request, and the choices field will always be an empty array.
    ///
    /// All other chunks will also include a usage field, but with a null value.
    /// **NOTE**: If the stream is interrupted, you may not receive the final usage
    /// chunk which contains the total token usage for the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

/// Specifies the processing type used for serving the request.
///
/// * If set to `auto`, then the request will be processed with the service tier configured in the Project settings. Unless otherwise configured, the Project will use `default`.
/// * If set to `default`, then the request will be processed with the standard pricing and performance for the selected model.
/// * If set to `flex` or `priority`, then the request will be processed with the corresponding service tier.
/// * When not set, the default behavior is 'auto'.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIServiceTier {
    #[default]
    Auto,
    Flex,
    Priority,
    Default,
    OnDemand,
}

#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIVerbosityLevel {
    Low,
    #[default]
    Medium,
    High,
}

/// Constrains the verbosity of the model's response. Lower values
/// will result in more concise responses, while higher values will
/// result in more verbose responses. Currently supported values are
/// `low`, `medium`, and `high`.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
pub struct OpenAITextField {
    /// Constrains the verbosity of the model's response. Lower values
    /// will result in more concise responses, while higher values will
    /// result in more verbose responses. Currently supported values are
    /// `low`, `medium`, and `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<OpenAIVerbosityLevel>,
}

/// Controls which (if any) tool is called by the model. `none` means
/// the model will not call any tool and instead generates a message.
/// `auto` means the model can pick between generating a message or calling
/// one or more tools. `required` means the model must call one or more tools.
#[derive(Debug, Default, Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIToolChoiceMode {
    /// Do not call tools, just produce a message
    None,
    /// Calling tools is optional: the model can
    /// call a tool or generate a message
    #[default]
    Auto,
    /// The model must choose a tool
    Required,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct FunctionToolFields {
    /// The name of the function to be called. Must be a-z,
    /// A-Z, 0-9, or contain underscores and dashes, with
    /// a maximum length of 64.
    pub name: String,

    /// A description of what the function does, used by the model to choose when and how to call the function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The parameters the functions accepts, described as a JSON Schema object. See the guide for examples, and the JSON Schema reference for documentation about the format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<JsonSchema>,
    /// Whether to enable strict schema adherence when
    /// generating the function call. If set to true,
    /// the model will follow the exact schema defined
    /// in the parameters field. Only a subset of JSON
    /// Schema is supported when strict is true
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct CustomToolFields {
    pub name: String,
    pub description: String,
    pub format: serde_json::Value,
}

/// A custom tool that processes input using a specified format.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAICustomTool {
    pub custom: CustomToolFields,
}

/// A tool that can be given to the model for calling
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum OpenAITool {
    Custom(OpenAICustomTool),
    Function { function: FunctionToolFields },
}

impl OpenAITool {
    pub fn name(&self) -> &String {
        match self {
            OpenAITool::Function { function } => &function.name,
            OpenAITool::Custom(c) => &c.custom.name,
        }
    }
    pub fn description(&self) -> String {
        match self {
            OpenAITool::Function { function } => match &function.description {
                Some(v) => v.clone(),
                None => "".to_string(),
            },
            OpenAITool::Custom(c) => c.custom.description.clone(),
        }
    }
    pub fn parameters(&self) -> &Option<JsonSchema> {
        match self {
            OpenAITool::Function { function } => &function.parameters,
            OpenAITool::Custom(_c) => todo!("OpenAI Custom functions are unsupported"),
        }
    }
}

/// A call to a function tool, as the model made it.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAIFunctionToolChoice {
    /// The arguments, as a JSON string.
    pub arguments: String,
    /// The tool's name.
    pub name: String,
}

/// A call to a custom tool, as the model made it.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAICustomToolChoice {
    /// What the tool was given.
    pub input: String,
    /// The tool's name.
    pub name: String,
}

/// A tool call the model made: its `type` says what kind of tool.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum OpenAIToolChoice {
    /// A call to a function tool: every tool Forma offers.
    Function {
        /// The call's id, which its result (a `tool` message) repeats.
        id: String,
        /// What is called, and with what.
        function: OpenAIFunctionToolChoice,
    },
    /// A call to a custom tool.
    Custom {
        /// The call's id.
        id: String,
        /// What is called, and with what.
        function: OpenAICustomToolChoice,
    },
}

impl OpenAIToolChoice {
    pub fn id(&self) -> String {
        match self {
            OpenAIToolChoice::Function { id, .. } => id.clone(),
            OpenAIToolChoice::Custom { id, .. } => id.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            OpenAIToolChoice::Function { function, .. } => function.name.clone(),
            OpenAIToolChoice::Custom { function, .. } => function.name.clone(),
        }
    }

    pub fn arguments(&self) -> String {
        match self {
            OpenAIToolChoice::Function { function, .. } => function.arguments.clone(),
            OpenAIToolChoice::Custom { function, .. } => function.input.clone(),
        }
    }
}

/// The partial `function` fields of a streaming tool-call delta.
///
/// Both fields are optional: the first chunk of a tool call usually carries
/// the `name` with an empty `arguments`, and subsequent chunks carry
/// `arguments` fragments that must be concatenated in order.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIFunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// A single tool call as it arrives in a streaming response.
///
/// Unlike [`OpenAIToolChoice`], every field is optional and the fragments are
/// keyed by `index` so they can be merged across chunks (see
/// [`ToolCallAccumulator`]). The first fragment for a given `index` carries the
/// `id`, `type` and function `name`; later fragments only carry `arguments`
/// pieces.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIToolCallDelta {
    /// Which tool call in the response this fragment belongs to.
    #[serde(default)]
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAIFunctionCallDelta>,
}

/// Builds up a single tool call from streaming [`OpenAIToolCallDelta`] fragments.
#[derive(Debug, Default, Clone)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
    /// Whether any fragment has been applied to this slot.
    seen: bool,
}

/// Accumulates streaming [`OpenAIToolCallDelta`] fragments (keyed by their
/// `index`) into complete [`OpenAIToolChoice`] values.
///
/// Streaming tool calls arrive across many chunks: the first fragment for an
/// index carries the `id` and function `name`, while the JSON `arguments` are
/// delivered as a sequence of string pieces that must be concatenated. This
/// accumulator merges those fragments so the caller can reconstruct the full
/// tool call once the stream completes.
#[derive(Debug, Default, Clone)]
pub struct ToolCallAccumulator {
    /// Builders indexed by the tool call's `index`.
    calls: Vec<ToolCallBuilder>,
}

impl ToolCallAccumulator {
    /// Merges a chunk's worth of tool-call fragments into the accumulator.
    pub fn apply(&mut self, deltas: Vec<OpenAIToolCallDelta>) {
        for delta in deltas {
            let index = delta.index;
            if index >= self.calls.len() {
                self.calls.resize_with(index + 1, ToolCallBuilder::default);
            }
            let slot = &mut self.calls[index];
            slot.seen = true;

            if let Some(id) = delta.id
                && !id.is_empty()
            {
                slot.id = id;
            }
            if let Some(function) = delta.function {
                if let Some(name) = function.name
                    && !name.is_empty()
                {
                    slot.name = name;
                }
                if let Some(arguments) = function.arguments {
                    slot.arguments.push_str(&arguments);
                }
            }
        }
    }

    /// Returns `true` if no tool-call fragment has been seen.
    pub fn is_empty(&self) -> bool {
        !self.calls.iter().any(|call| call.seen)
    }

    /// Consumes the accumulator and produces the reconstructed tool calls,
    /// preserving their original order.
    ///
    /// All calls are reconstructed as [`OpenAIToolChoice::Function`]; streamed
    /// custom tool calls are not currently supported.
    pub fn finish(self) -> Vec<OpenAIToolChoice> {
        self.calls
            .into_iter()
            .filter(|call| call.seen)
            .map(|call| OpenAIToolChoice::Function {
                id: call.id,
                function: OpenAIFunctionToolChoice {
                    name: call.name,
                    arguments: call.arguments,
                },
            })
            .collect()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Who wrote a message.
pub enum OpenAIRole {
    Assistant,
    #[default]
    User,
    Developer,
    System,
    Tool,
}

impl Display for OpenAIRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assistant => write!(f, "assistant"),
            Self::User => write!(f, "user"),
            Self::Developer => write!(f, "developer"),
            Self::System => write!(f, "system"),
            Self::Tool => write!(f, "tool"),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Audio the model answered with.
pub struct OpenAIAudioResponse {
    /// The audio, base64.
    pub data: String,
    /// When the audio stops being available to refer back to (Unix seconds).
    pub expires_at: usize,
    /// The audio's id.
    pub id: String,
    /// What the audio says.
    pub transcript: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy)]
pub struct OpenAICompletionTokenDetails {
    /// When using Predicted Outputs, the number of tokens in the
    /// prediction that appeared in the completion.
    ///
    /// GROQ does not seem to support this field.
    #[serde(default)]
    pub accepted_prediction_tokens: usize,

    /// Audio input tokens generated by the model.
    #[serde(default)]
    pub audio_tokens: usize,

    /// Tokens generated by the model for reasoning.
    #[serde(default)]
    pub reasoning_tokens: usize,

    /// When using Predicted Outputs, the number of tokens in
    /// the prediction that did not appear in the completion.
    /// However, like reasoning tokens, these tokens are still
    /// counted in the total completion tokens for purposes of
    /// billing, output, and context window limits.
    #[serde(default)]
    pub rejected_prediction_tokens: usize,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy)]
pub struct OpenAIPromptTokenDetails {
    /// Audio input tokens present in the prompt.
    ///
    /// GROQ does not seem to support this field.
    #[serde(default)]
    pub audio_tokens: usize,

    /// Cached tokens present in the prompt.
    #[serde(default)]
    pub cached_tokens: usize,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIUsage {
    /// Number of tokens in the generated completion
    pub completion_tokens: usize,
    /// Number of tokens in the prompt
    pub prompt_tokens: usize,
    /// Total number of tokens used in the request (prompt + completion)
    pub total_tokens: usize,
    /// Breakdown of tokens used in completion
    pub completion_tokens_details: Option<OpenAICompletionTokenDetails>,
    /// Breakdown of tokens used in prompt
    pub prompt_tokens_details: Option<OpenAIPromptTokenDetails>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Text.
pub struct OpenAITextPart {
    /// The text.
    pub text: String,
}

impl From<String> for OpenAITextPart {
    fn from(item: String) -> Self {
        Self { text: item }
    }
}

/// Tells the model what level of detail to use when processing and understanding the image
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum OpenAIVisionDetail {
    ///  lets the model process the image with a budget of 85 tokens.
    /// The model receives a low-resolution 512px x 512px version of the
    /// image. This is fine if your use case doesn't require the model to
    /// see with high-resolution detail (for example, if you're asking
    /// about the dominant shape or color in the image).
    Low,
    #[default]
    Auto,
    /// For when you want the model to have a better understanding of the image.
    High,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// Where an image is.
pub struct OpenAIImageUrl {
    /// A `data:` URL (`data:image/png;base64,...`), or a URL the model's
    /// provider can reach.
    pub url: String,
    /// How closely the model should look at it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<OpenAIVisionDetail>,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// An image.
pub struct OpenAIImagePart {
    /// Where it is.
    pub image_url: OpenAIImageUrl,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A recording, inline.
pub struct OpenAIInputAudio {
    /// Base64 encoded audio data.
    pub data: String,
    /// The format of the encoded audio data. Currently supports "wav" and "mp3".
    pub format: AudioFormat,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A recording.
pub struct OpenAIAudioPart {
    /// The recording.
    pub input_audio: OpenAIInputAudio,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A file: a document, or audio in any container.
pub struct OpenAIFile {
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
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A file.
pub struct OpenAIFilePart {
    /// The file.
    pub file: OpenAIFile,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// One piece of a message's content, its `type` saying what it is.
pub enum OpenAIMessagePart {
    /// Text.
    Text(OpenAITextPart),
    /// An image.
    ImageUrl(OpenAIImagePart),
    /// A recording, base64 with its format named.
    InputAudio(OpenAIAudioPart),
    /// A file, base64 in a `data:` URL.
    File(OpenAIFilePart),
}

impl From<&OpenAIMessagePart> for String {
    fn from(item: &OpenAIMessagePart) -> Self {
        match item {
            OpenAIMessagePart::Text(t) => t.text.clone(),
            OpenAIMessagePart::ImageUrl(_) => "[image-url]".to_string(),
            OpenAIMessagePart::InputAudio(_) => "[input-audio]".to_string(),
            OpenAIMessagePart::File(_) => "[file]".to_string(),
        }
    }
}

impl From<String> for OpenAIMessagePart {
    fn from(item: String) -> Self {
        Self::Text(item.into())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// What a message says: text, or a list of parts.
pub enum OpenAIMessageContent {
    /// Just text.
    String(String),
    /// Text, images, recordings and files.
    Parts(Vec<OpenAIMessagePart>),
}

impl OpenAIMessageContent {
    pub fn as_parts(&self) -> Vec<OpenAIMessagePart> {
        match self {
            OpenAIMessageContent::String(s) => {
                vec![OpenAIMessagePart::Text(OpenAITextPart { text: s.clone() })]
            }
            OpenAIMessageContent::Parts(v) => v.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            OpenAIMessageContent::String(s) => s.is_empty(),
            OpenAIMessageContent::Parts(v) => v.is_empty(),
        }
    }
}

impl Default for OpenAIMessageContent {
    fn default() -> Self {
        Self::String("".into())
    }
}

impl From<String> for OpenAIMessageContent {
    fn from(item: String) -> Self {
        Self::String(item)
    }
}

impl From<&OpenAIMessageContent> for String {
    fn from(val: &OpenAIMessageContent) -> Self {
        match val {
            OpenAIMessageContent::String(s) => s.clone(),
            OpenAIMessageContent::Parts(p) => {
                let strings: Vec<String> = p
                    .iter()
                    .map(|x| {
                        let s: String = x.into();
                        s
                    })
                    .collect();
                strings.join("\n")
            }
        }
    }
}

/// System message (role: "system")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAISystemMessage {
    /// What it says.
    pub content: OpenAIMessageContent,
    /// Who wrote it, among several of the same role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// User message (role: "user")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAIUserMessage {
    /// What it says: text, or text, images, recordings and files.
    pub content: OpenAIMessageContent,
    /// Who wrote it, among several of the same role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Developer message (role: "developer")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAIDeveloperMessage {
    /// What it says.
    pub content: OpenAIMessageContent,
}

/// Assistant message (role: "assistant")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAIAssistantMessage {
    /// What the agent said: its text, and the files it produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIMessageContent>,
    /// Why the model refused to answer, if it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Who wrote it, among several of the same role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    // pub annotations: .. not supported yet
    /// Audio the model answered with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<OpenAIAudioResponse>,
    /// The tools the model called.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAIToolChoice>,
}

/// Tool result message (role: "tool")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OpenAIToolMessage {
    /// The ID of the tool call this message is responding to
    pub tool_call_id: String,
    /// What the tool returned.
    pub content: OpenAIMessageContent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "role")]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A message, in the shape of OpenAI's chat messages: its `role` says who
/// wrote it.
pub enum OpenAIChatMessage {
    /// Instructions to the model.
    System(OpenAISystemMessage),
    /// What the user wrote.
    User(OpenAIUserMessage),
    /// Instructions to the model.
    Developer(OpenAIDeveloperMessage),
    /// What the agent said.
    Assistant(OpenAIAssistantMessage),
    /// A tool call's result.
    Tool(OpenAIToolMessage),
}

impl Default for OpenAIChatMessage {
    fn default() -> Self {
        Self::User(OpenAIUserMessage::default())
    }
}

impl From<String> for OpenAIChatMessage {
    fn from(item: String) -> Self {
        Self::User(OpenAIUserMessage {
            content: OpenAIMessageContent::String(item),
            name: None,
        })
    }
}
impl From<String> for OpenAISystemMessage {
    fn from(item: String) -> Self {
        Self {
            content: OpenAIMessageContent::String(item),
            name: None,
        }
    }
}

impl std::fmt::Display for OpenAIChatMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let content: Option<&OpenAIMessageContent> = match self {
            Self::System(m) => Some(&m.content),
            Self::User(m) => Some(&m.content),
            Self::Developer(m) => Some(&m.content),
            Self::Assistant(m) => m.content.as_ref(),
            Self::Tool(m) => Some(&m.content),
        };
        if let Some(c) = content {
            match c {
                OpenAIMessageContent::String(s) => {
                    writeln!(f, "{s}")?;
                }
                OpenAIMessageContent::Parts(p) => {
                    for x in p.iter() {
                        let s: String = x.into();
                        writeln!(f, "{s}")?;
                    }
                }
            }
        }
        Ok(())
    }
}
impl From<UserMessage> for OpenAIUserMessage {
    fn from(value: UserMessage) -> Self {
        Self {
            content: OpenAIMessageContent::Parts(value.content),
            name: value.name,
        }
    }
}

impl From<AssistantMessage> for OpenAIAssistantMessage {
    fn from(value: AssistantMessage) -> Self {
        let content = if value.content.is_empty() {
            None
        } else {
            Some(OpenAIMessageContent::Parts(value.content))
        };
        Self {
            content,
            refusal: value.refusal,
            name: value.name,
            audio: value.audio,
            tool_calls: value.tool_calls,
        }
    }
}

impl From<LLMMessage> for OpenAIChatMessage {
    fn from(value: LLMMessage) -> Self {
        match value {
            LLMMessage::System(m) => Self::System(m),
            LLMMessage::User(m) => Self::User(m.into()),
            LLMMessage::Developer(m) => Self::Developer(m),
            LLMMessage::Assistant(m) => Self::Assistant(m.into()),
            LLMMessage::Tool(m) => Self::Tool(m),
        }
    }
}

impl From<OpenAIUserMessage> for UserMessage {
    fn from(value: OpenAIUserMessage) -> Self {
        Self {
            content: value.content.as_parts(),
            name: value.name,
        }
    }
}

impl From<OpenAIAssistantMessage> for AssistantMessage {
    fn from(value: OpenAIAssistantMessage) -> Self {
        let content = if let Some(content) = value.content {
            content.as_parts()
        } else {
            vec![]
        };
        Self {
            content,
            name: value.name,
            refusal: value.refusal,
            audio: value.audio,
            tool_calls: value.tool_calls,
            thought_signature: None,
        }
    }
}

impl From<OpenAIChatMessage> for LLMMessage {
    fn from(value: OpenAIChatMessage) -> Self {
        match value {
            OpenAIChatMessage::System(m) => Self::System(m),
            OpenAIChatMessage::User(m) => Self::User(m.into()),
            OpenAIChatMessage::Developer(m) => Self::Developer(m),
            OpenAIChatMessage::Assistant(m) => Self::Assistant(m.into()),
            OpenAIChatMessage::Tool(m) => Self::Tool(m),
        }
    }
}

impl GenaiMessageTrait for OpenAIChatMessage {
    fn role(&self) -> Role {
        match self {
            Self::System(_) => OpenAIRole::System,
            Self::User(_) => OpenAIRole::User,
            Self::Developer(_) => OpenAIRole::Developer,
            Self::Assistant(_) => OpenAIRole::Assistant,
            Self::Tool(_) => OpenAIRole::Tool,
        }
    }

    fn content_string(&self) -> Option<String> {
        let content: Option<&OpenAIMessageContent> = match self {
            Self::System(m) => Some(&m.content),
            Self::User(m) => Some(&m.content),
            Self::Developer(m) => Some(&m.content),
            Self::Assistant(m) => m.content.as_ref(),
            Self::Tool(m) => Some(&m.content),
        };
        content.map(|c| c.into())
    }

    fn has_content(&self) -> bool {
        let content: Option<&OpenAIMessageContent> = match self {
            Self::System(m) => Some(&m.content),
            Self::User(m) => Some(&m.content),
            Self::Developer(m) => Some(&m.content),
            Self::Assistant(m) => m.content.as_ref(),
            Self::Tool(m) => Some(&m.content),
        };
        if let Some(c) = content {
            match c {
                OpenAIMessageContent::String(s) => !s.is_empty(),
                OpenAIMessageContent::Parts(v) => !v.is_empty(),
            }
        } else {
            false
        }
    }
}

/// Partial message used in streaming deltas. Unlike [`OpenAIChatMessage`], all fields
/// are optional because a streaming chunk may arrive as `{}` or with only some fields.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<OpenAIRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<OpenAIAudioResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCallDelta>>,
    /// Gemini "thinking" models return a thought signature that must be echoed
    /// back with the assistant turn on subsequent requests. Providers without
    /// thought signatures leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<bytes::Bytes>,
}

/// The reason the model stopped generating tokens.
/// This will be `stop` if the model hit a natural stop
/// point or a provided stop sequence, `length` if the
/// maximum number of tokens specified in the request
/// was reached, `content_filter` if content was omitted
/// due to a flag from our content filters, `tool_calls`
/// if the model called a tool, or `function_call` (deprecated)
/// if the model called a function.
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIFinishReason {
    /// the model hit a natural stop point or a provided stop sequence
    #[default]
    Stop,
    /// the maximum number of tokens specified in the request was reached
    Length,
    ///  content was omitted due to a flag from OpenAI's content filters
    ContentFilter,
    ///  the model called a tool
    ToolCalls,
    /// (deprecated) if the model called a function.
    FunctionCalls,
    /// Not sent by OpenAI: lets other providers' streams report
    /// [`LLMFinishReason::MalformedToolCall`].
    MalformedToolCall,
}
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAILogProbInfo {
    /// A list of integers representing the UTF-8 bytes representation of the token. Useful in instances where characters are represented by multiple tokens and their byte representations must be combined to generate the correct text representation. Can be null if there is no bytes representation for the token.
    pub bytes: Vec<usize>,

    /// The log probability of this token, if it is within the top 20 most likely tokens. Otherwise, the value -9999.0 is used to signify that the token is very unlikely.
    pub logprob: f64,

    /// The token
    pub token: String,
    /// List of the most likely tokens and their
    /// log probability, at this token position. In rare cases,
    /// there may be fewer than the number of requested
    /// top_logprobs returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<Box<Self>>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAILogProb {
    pub content: OpenAILogProbInfo,
    pub refusal: OpenAILogProbInfo,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIMessageChoice {
    pub finish_reason: OpenAIFinishReason,
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<OpenAILogProb>,
    pub message: OpenAIChatMessage,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIMessageChoiceDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<OpenAIFinishReason>,
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<OpenAILogProb>,
    pub delta: OpenAIChatDelta,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIChatChunk {
    pub choices: Vec<OpenAIMessageChoiceDelta>,
    /// The Unix timestamp (in seconds) of when the chat completion was created.
    pub created: usize,
    /// Unique identifier for the chat completion
    pub id: String,
    /// The model used
    pub model: String,
    /// The object type, which is always `chat.completion`
    pub object: String,
    /// Specifies the processing type used for serving the request.
    pub system_fingerprint: Option<String>,
    /// Token usage for the whole request. This is only present (non-null) on
    /// the final chunk when `stream_options.include_usage` was requested; every
    /// other chunk carries a `null` usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIChatResponse {
    pub choices: Vec<OpenAIMessageChoice>,
    /// The Unix timestamp (in seconds) of when the chat completion was created.
    pub created: usize,
    /// Unique identifier for the chat completion
    pub id: String,
    /// The model used
    pub model: String,
    /// The object type, which is always `chat.completion`
    pub object: String,
    /// Specifies the processing type used for serving the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<OpenAIServiceTier>,
    /// Usage statistics for the completion request
    pub usage: OpenAIUsage,
}

impl OpenAIChatResponse {
    /// Retrieves the first message from all choices
    pub fn message(&self) -> Option<&OpenAIChatMessage> {
        if let Some(m) = self.choices.last() {
            Some(&m.message)
        } else {
            None
        }
    }
}

impl GenaiRequestTrait for OpenAIChatRequest {
    fn set_streaming(&mut self, streaming: bool) {
        self.stream = Some(streaming);
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum OpenAISchemaType {
    #[default]
    JsonSchema,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIResponseFormatContent {
    pub name: String,
    pub schema: JsonSchema,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIResponseFormat {
    #[serde(rename = "type")]
    pub schema_type: OpenAISchemaType,
    pub json_schema: OpenAIResponseFormatContent,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct OpenAIChatRequest {
    /// A list of messages comprising the conversation
    /// so far. Depending on the model you use, different
    /// message types (modalities) are supported, like
    /// text, images, and audio.
    pub messages: Vec<OpenAIChatMessage>,

    /// Model ID used to generate the response, like gpt-4o or o3.
    /// OpenAI offers a wide range of models with different capabilities,
    /// performance characteristics, and price points. Refer to the model
    /// guide to browse and compare available models.
    pub model: String,

    /// Required when audio output is
    /// requested with `modalities: ["audio"]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<OpenAIAudioSettings>,

    /// Number between -2.0 and 2.0. Positive values penalize new tokens
    /// based on their existing frequency in the text so far, decreasing
    /// the model's likelihood to repeat the same line verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    // logit_bias // not supported... not sure what it is for.
    // function_call --> deprecated in favour of `tool_choice`
    // functions --> deprecated in favour of `tools`
    /// Whether to return log probabilities of the output tokens or not.
    /// If true, returns the log probabilities of each output token
    /// returned in the content of message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,

    /// An upper bound for the number of tokens that can be generated for a
    /// completion, including visible output tokens and reasoning tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<usize>,
    // max_tokens --> deprecated in favour of above
    /// Set of 16 key-value pairs that can be attached to an object. This can
    /// be useful for storing additional information about the object in a
    /// structured format, and querying for objects via API or the dashboard.
    ///
    /// Keys are strings with a maximum length of 64 characters. Values are
    /// strings with a maximum length of 512 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// Output types that you would like the model to generate. Most models are
    /// capable of generating text, which is the default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<OpenAIModality>>,

    /// How many chat completion choices to generate for each input message.
    /// Note that you will be charged based on the number of generated tokens
    /// across all of the choices. Keep `n` as `1` to minimize costs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,

    /// Whether to enable [parallel function calling](https://platform.openai.com/docs/guides/function-calling#configuring-parallel-function-calling)
    /// during tool use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,

    // prediction: todo
    /// Number between -2.0 and 2.0. Positive values penalize new tokens
    /// based on whether they appear in the text so far, increasing the
    /// model's likelihood to talk about new topics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,

    /// Used by OpenAI to cache responses for similar requests to optimize
    /// your cache hit rates. Replaces the user field. [Learn more](https://platform.openai.com/docs/guides/prompt-caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,

    /// Constrains effort on reasoning for reasoning models. Currently supported
    /// values are minimal, low, medium, and high. Reducing reasoning effort
    /// can result in faster responses and fewer tokens used on reasoning in a response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<OpenAIReasoningEffort>,

    /// An object specifying the format that the model must output.
    ///
    /// Setting to `{ "type": "json_schema", "json_schema": {...} }`
    /// enables Structured Outputs which ensures the model will match your
    /// supplied JSON schema.
    ///
    /// Setting to `{ "type": "json_object" }` enables the older JSON mode,
    /// which ensures the message the model generates is valid JSON.
    ///
    /// Using `json_schema` is preferred for models that support it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<OpenAIResponseFormat>,

    /// A stable identifier used to help detect users of your application
    /// that may be violating OpenAI's usage policies. The IDs should be a
    /// string that uniquely identifies each user. We recommend hashing their
    /// username or email address, in order to avoid sending us any identifying
    /// information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,

    /// When the `service_tier` parameter is set, the response body will
    /// include the `service_tier` value based on the processing mode actually
    /// used to serve the request. This response value may be different from
    /// the value set in the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<OpenAIServiceTier>,

    // stop: not supported with latest reasoning models.
    /// Whether or not to store the output of this chat completion request
    /// for use in our model distillation or evals products.
    ///
    /// Supports text and image inputs.
    /// **Note**: image inputs over 10MB will be dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,

    /// If set to true, the model response data will be streamed
    /// to the client as it is generated using server-sent events.
    /// See the Streaming section below for more information,
    /// along with the streaming responses guide for more information
    /// on how to handle the streaming events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OpenAIStreamOption>,

    /// What sampling temperature to use, between 0 and 2. Higher values like 0.8 will make
    /// the output more random, while lower values like 0.2 will make it more focused
    /// and deterministic. We generally recommend altering this or top_p but not both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Constrains the verbosity of the model's response. Lower values
    /// will result in more concise responses, while higher values will
    /// result in more verbose responses. Currently supported values are
    /// `low`, `medium`, and `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<OpenAITextField>,

    /// Controls which (if any) tool is called by the model. `none` means
    /// the model will not call any tool and instead generates a message.
    /// `auto` means the model can pick between generating a message or calling
    /// one or more tools. `required` means the model must call one or more tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoiceMode>,

    /// A list of tools the model may select as appropriate to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,

    /// An integer between 0 and 20 specifying the number of most
    /// likely tokens to return at each token position, each with an
    /// associated log probability. logprobs must be set to true if
    /// this parameter is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<usize>,

    /// An alternative to sampling with temperature, called nucleus sampling,
    /// where the model considers the results of the tokens with top_p probability
    /// mass. So 0.1 means only the tokens comprising the top 10% probability
    /// mass are considered.
    ///
    /// We generally recommend altering this or temperature but not both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>, // web_search_options: todo
}

/// A client for OpenAI's v1 api
///
/// It attempts to adhere as strictly as possible (even if
/// some features are not supported by Forma).
///
/// > 🔑 **Authentication**: uses the `OPENAI_API_KEY` API Key
///
/// > Note: the documentation below has been copied nearly verbatim
/// > from OpenAI's one, as reference
#[derive(Serialize, Deserialize, Debug, Clone, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct OpenaiV1 {
    /// The endpoint to reach for the API. Defaults to `https://api.openai.com`
    #[serde(
        default = "default_openai_endpoint",
        skip_serializing_if = "is_default_openai_endpoint"
    )]
    pub endpoint: String,

    /// The model to use. Defaults to `gpt-3.5-turbo`
    ///
    /// Common models are (source https://developers.openai.com/api/docs/pricing):
    /// - `gpt-6-astra`
    /// - `gpt-5.6-sol`
    /// - `gpt-5.6-terra`
    /// - `gpt-5.6-luna`
    /// - `gpt-5.5`
    /// - `gpt-5.5-pro`
    /// - `gpt-5.4`
    /// - `gpt-5.4-pro`
    /// - `gpt-4-turbo`
    /// - `gpt-4`
    /// - `gpt-3.5-turbo`
    #[serde(default = "default_openai_model")]
    pub model: String,
    /// Required when audio output is
    /// requested with `modalities: ["audio"]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<OpenAIAudioSettings>,

    /// Number between -2.0 and 2.0. Positive values penalize new tokens
    /// based on their existing frequency in the text so far, decreasing
    /// the model's likelihood to repeat the same line verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    // logit_bias // not supported... not sure what it is for.
    // function_call --> deprecated in favour of `tool_choice`
    // functions --> deprecated in favour of `tools`
    /// Whether to return log probabilities of the output tokens or not.
    /// If true, returns the log probabilities of each output token
    /// returned in the content of message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,

    /// An upper bound for the number of tokens that can be generated for a
    /// completion, including visible output tokens and reasoning tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<usize>,
    // max_tokens --> deprecated in favour of above
    /// Set of 16 key-value pairs that can be attached to an object. This can
    /// be useful for storing additional information about the object in a
    /// structured format, and querying for objects via API or the dashboard.
    ///
    /// Keys are strings with a maximum length of 64 characters. Values are
    /// strings with a maximum length of 512 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// Output types that you would like the model to generate. Most models are
    /// capable of generating text, which is the default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<OpenAIModality>>,

    /// How many chat completion choices to generate for each input message.
    /// Note that you will be charged based on the number of generated tokens
    /// across all of the choices. Keep `n` as `1` to minimize costs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,

    /// Whether to enable [parallel function calling](https://platform.openai.com/docs/guides/function-calling#configuring-parallel-function-calling)
    /// during tool use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,

    // prediction: todo
    /// Number between -2.0 and 2.0. Positive values penalize new tokens
    /// based on whether they appear in the text so far, increasing the
    /// model's likelihood to talk about new topics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,

    /// Used by OpenAI to cache responses for similar requests to optimize
    /// your cache hit rates. Replaces the user field. [Learn more](https://platform.openai.com/docs/guides/prompt-caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,

    /// Constrains effort on reasoning for reasoning models. Currently supported
    /// values are minimal, low, medium, and high. Reducing reasoning effort
    /// can result in faster responses and fewer tokens used on reasoning in a response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<OpenAIReasoningEffort>,

    /// An object specifying the format that the model must output.
    ///
    /// Setting to `{ "type": "json_schema", "json_schema": {...} }`
    /// enables Structured Outputs which ensures the model will match your
    /// supplied JSON schema.
    ///
    /// Setting to `{ "type": "json_object" }` enables the older JSON mode,
    /// which ensures the message the model generates is valid JSON.
    ///
    /// Using `json_schema` is preferred for models that support it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<OpenAIResponseFormat>,

    /// A stable identifier used to help detect users of your application
    /// that may be violating OpenAI's usage policies. The IDs should be a
    /// string that uniquely identifies each user. We recommend hashing their
    /// username or email address, in order to avoid sending us any identifying
    /// information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,

    /// When the `service_tier` parameter is set, the response body will
    /// include the `service_tier` value based on the processing mode actually
    /// used to serve the request. This response value may be different from
    /// the value set in the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<OpenAIServiceTier>,

    // stop: not supported with latest reasoning models.
    /// Whether or not to store the output of this chat completion request
    /// for use in our model distillation or evals products.
    ///
    /// Supports text and image inputs.
    /// **Note**: image inputs over 10MB will be dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,

    /// If set to true, the model response data will be streamed
    /// to the client as it is generated using server-sent events.
    /// See the Streaming section below for more information,
    /// along with the streaming responses guide for more information
    /// on how to handle the streaming events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OpenAIStreamOption>,

    /// What sampling temperature to use, between 0 and 2. Higher values like 0.8 will make
    /// the output more random, while lower values like 0.2 will make it more focused
    /// and deterministic. We generally recommend altering this or top_p but not both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Constrains the verbosity of the model's response. Lower values
    /// will result in more concise responses, while higher values will
    /// result in more verbose responses. Currently supported values are
    /// `low`, `medium`, and `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<OpenAITextField>,

    /// Controls which (if any) tool is called by the model. `none` means
    /// the model will not call any tool and instead generates a message.
    /// `auto` means the model can pick between generating a message or calling
    /// one or more tools. `any` means the model must call one or more tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoiceMode>,

    /// A list of tools the model may call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,

    /// An integer between 0 and 20 specifying the number of most
    /// likely tokens to return at each token position, each with an
    /// associated log probability. logprobs must be set to true if
    /// this parameter is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<usize>,

    /// An alternative to sampling with temperature, called nucleus sampling,
    /// where the model considers the results of the tokens with top_p probability
    /// mass. So 0.1 means only the tokens comprising the top 10% probability
    /// mass are considered.
    ///
    /// We generally recommend altering this or temperature but not both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>, // web_search_options: todo
}

fn default_openai_endpoint() -> String {
    "https://api.openai.com".to_string()
}

// Left out when serializing, so a default client comes out as just
// `provider` + `model`, which every other provider also accepts.
fn is_default_openai_endpoint(v: &String) -> bool {
    *v == default_openai_endpoint()
}

fn default_openai_model() -> String {
    "gpt-3.5-turbo".to_string()
}

impl OpenaiV1 {
    /// Builds the request body for the OpenAI v1 chat endpoint
    /// If `allowed_tools` is `None`, all tools are made available;
    /// if it is `Some(vector)`, only those within that list are allowed.
    /// If some IDs in `allowed_tools` are not found, they are simply ignored.
    fn get_request_body<M: Into<OpenAIChatMessage>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> OpenAIChatRequest {
        let tools = match &self.tools {
            Some(ts) => {
                let r = if let Some(allowed) = allowed_tools {
                    ts.iter()
                        .filter(|t| allowed.contains(t.name()))
                        .cloned()
                        .collect()
                } else {
                    ts.clone()
                };
                Some(r)
            }
            None => None,
        };
        let messages = messages.into_iter().map(|x| x.into()).collect();
        OpenAIChatRequest {
            model: self.model.clone(),
            messages,
            audio: self.audio,
            frequency_penalty: self.frequency_penalty,
            logprobs: self.logprobs,
            max_completion_tokens: self.max_completion_tokens,
            metadata: self.metadata.clone(),
            modalities: self.modalities.clone(),
            n: self.n,
            parallel_tool_calls: self.parallel_tool_calls,
            presence_penalty: self.presence_penalty,
            prompt_cache_key: self.prompt_cache_key.clone(),
            reasoning_effort: self.reasoning_effort,
            response_format: self.response_format.clone(),
            safety_identifier: self.safety_identifier.clone(),
            service_tier: self.service_tier,
            store: self.store,
            stream: self.stream,
            stream_options: self.stream_options,
            temperature: self.temperature,
            text: self.text,
            tool_choice: self.tool_choice,
            tools,
            top_logprobs: self.top_logprobs,
            top_p: self.top_p,
        }
    }
}

impl Default for OpenaiV1 {
    fn default() -> Self {
        Self {
            endpoint: default_openai_endpoint(),
            model: default_openai_model(),
            audio: None,
            frequency_penalty: None,
            logprobs: None,
            max_completion_tokens: None,
            metadata: None,
            modalities: None,
            n: None,
            parallel_tool_calls: None,
            presence_penalty: None,
            prompt_cache_key: None,
            reasoning_effort: None,
            response_format: None,
            safety_identifier: None,
            service_tier: None,
            store: None,
            stream: None,
            stream_options: None,
            temperature: None,
            text: None,
            tool_choice: None,
            tools: None,
            top_logprobs: None,
            top_p: None,
        }
    }
}

impl From<OpenAIFinishReason> for LLMFinishReason {
    fn from(reason: OpenAIFinishReason) -> Self {
        match reason {
            OpenAIFinishReason::Stop => LLMFinishReason::Stop,
            OpenAIFinishReason::Length => LLMFinishReason::Length,
            OpenAIFinishReason::ContentFilter => LLMFinishReason::ContentFilter,
            OpenAIFinishReason::ToolCalls => LLMFinishReason::ToolCalls,
            OpenAIFinishReason::FunctionCalls => LLMFinishReason::FunctionCalls,
            OpenAIFinishReason::MalformedToolCall => LLMFinishReason::MalformedToolCall,
        }
    }
}

impl From<OpenAIMessageChoice> for MessageChoice {
    fn from(choice: OpenAIMessageChoice) -> Self {
        MessageChoice {
            finish_reason: choice.finish_reason.into(),
            index: choice.index,
            logprobs: choice.logprobs,
            message: choice.message.into(),
        }
    }
}

impl From<OpenAIChatResponse> for ChatResponse {
    fn from(response: OpenAIChatResponse) -> Self {
        ChatResponse {
            choices: response.choices.into_iter().map(|c| c.into()).collect(),
            created: response.created,
            id: response.id,
            model: response.model,
            object: response.object,
            usage: response.usage,
        }
    }
}

impl From<OpenAIChatChunk> for LLMChunk {
    fn from(value: OpenAIChatChunk) -> Self {
        Self {
            choices: value.choices,
            created: value.created,
            id: value.id,
            model: value.model,
            object: value.object,
            system_fingerprint: value.system_fingerprint,
            usage: value.usage,
        }
    }
}

#[async_trait]
impl GenaiClientTrait for OpenaiV1 {
    type Chunk = OpenAIChatChunk;
    type Response = OpenAIChatResponse;
    type Message = OpenAIChatMessage;
    type Tool = OpenAITool;

    const PROVIDER: LLMProvider = LLMProvider::OpenAI;

    const API_KEY_NAME: Option<&'static str> = Some("OPENAI_API_KEY");

    fn model(&self) -> &str {
        &self.model
    }

    fn set_tools(&mut self, tools: Option<Vec<Self::Tool>>) {
        self.tools = tools;
    }

    fn set_tool_choice_mode(&mut self, tool_choice: Option<OpenAIToolChoiceMode>) {
        self.tool_choice = tool_choice;
    }

    fn set_output_schema(&mut self, schema: OpenAIResponseFormat) {
        self.response_format = Some(schema);
    }

    fn has_output_schema(&self) -> bool {
        self.response_format.is_some()
    }

    fn count_tools(&self) -> usize {
        match &self.tools {
            Some(v) => v.len(),
            None => 0,
        }
    }

    fn frequency_penalty(&self) -> Option<f64> {
        self.frequency_penalty
    }
    fn max_tokens(&self) -> Option<usize> {
        self.max_completion_tokens
    }
    fn presence_penalty(&self) -> Option<f64> {
        self.presence_penalty
    }
    fn temperature(&self) -> Option<f64> {
        self.temperature
    }
    fn top_p(&self) -> Option<f64> {
        self.top_p
    }

    fn chat_endpoint(&self) -> Result<String, String> {
        if self.endpoint.is_empty() {
            return Err("endpoint empty".to_string());
        }
        Ok(format!("{}/v1/chat/completions", self.endpoint))
    }

    async fn generate<M: Into<Self::Message> + Send + 'static>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<Self::Response, String> {
        let body = self.get_request_body(messages, allowed_tools);
        let request = rest_request(self, &body)?;
        post::<Self::Response>(request).await
    }
    async fn stream<M: Into<Self::Message> + Send + 'static, O: From<Self::Chunk>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<ByteStream<O>, String> {
        let mut body = self.get_request_body(messages, allowed_tools);
        body.stream = Some(true);
        // Ask for a final usage chunk so token counts are available while streaming.
        body.stream_options = Some(OpenAIStreamOption {
            include_usage: Some(true),
            ..Default::default()
        });
        let request = rest_request(self, &body)?;
        post_stream::<Self::Chunk, O>(request).await
    }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {
    use super::GenaiClientTrait;
    use crate::{OpenAIChatChunk, OpenAIChatMessage, openai::v1::OpenAIFinishReason};
    use futures::StreamExt;

    use super::OpenaiV1;

    #[tokio::test]
    async fn test_openai_v1_stream() {
        let llm = OpenaiV1::default();

        let messages: Vec<OpenAIChatMessage> = vec!["hello!".to_string().into()];
        let mut r = llm
            .stream::<OpenAIChatMessage, OpenAIChatChunk>(messages, None)
            .await
            .expect("Failed to stream");
        let mut count = 0;
        let mut done: Option<OpenAIFinishReason> = None;
        let mut usage_seen = false;
        while let Some(e) = r.next().await {
            count += 1;
            // The final chunk (requested via stream_options.include_usage) carries
            // the token usage and an empty `choices` array.
            if e.choices.is_empty() {
                assert!(e.usage.is_some());
                usage_seen = true;
                continue;
            }
            assert!(done.is_none());
            done = e.choices[0].finish_reason;
        }
        assert!(done.is_some());
        // assert that the reason is Stop
        assert_eq!(done, Some(OpenAIFinishReason::Stop));
        assert!(count > 0, "Expected at least one chunk");
        assert!(usage_seen, "Expected a final usage chunk");
    }

    #[tokio::test]
    async fn test_openai_v1_generate() {
        let llm = OpenaiV1::default();
        let messages: Vec<OpenAIChatMessage> = vec!["hello!".to_string().into()];
        let _r = llm
            .generate(messages, None)
            .await
            .expect("Failed to generate response");
    }
}

#[cfg(test)]
mod testing {

    use super::*;

    #[test]
    fn test_get_request_body_with_allowed_tools() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let tool1 = OpenAITool::Function {
            function: FunctionToolFields {
                name: "tool1".to_string(),
                description: Some("first tool".to_string()),
                parameters: Some(JsonSchema::default()),
                strict: None,
            },
        };
        let tool2 = OpenAITool::Function {
            function: FunctionToolFields {
                name: "tool2".to_string(),
                description: Some("second tool".to_string()),
                parameters: Some(JsonSchema::default()),
                strict: None,
            },
        };
        let mut client = OpenaiV1::default();
        client.set_tools(Some(vec![tool1, tool2]));

        let allowed = vec!["tool1".to_string()];
        let body = client.get_request_body(messages, Some(allowed));

        assert!(body.tools.is_some());
        let tools = body.tools.unwrap();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn test_get_request_body_no_tools_filter() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let tool = OpenAITool::Function {
            function: FunctionToolFields {
                name: "tool1".to_string(),
                description: Some("tool".to_string()),
                parameters: Some(JsonSchema::default()),
                strict: None,
            },
        };
        let mut client = OpenaiV1::default();
        client.set_tools(Some(vec![tool]));

        let body = client.get_request_body(messages, None);

        assert!(body.tools.is_some());
        assert_eq!(body.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_get_request_body_no_tools() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let client = OpenaiV1::default();

        let body = client.get_request_body(messages, None);

        assert!(body.tools.is_none());
    }

    #[test]
    fn test_get_request_body_preserves_parameters() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let temperature = 0.5;
        let top_p = 0.9;
        let mut client = OpenaiV1::default();
        client.temperature = Some(temperature);
        client.top_p = Some(top_p);

        let body = client.get_request_body(messages, None);

        assert_eq!(body.temperature, Some(temperature));
        assert_eq!(body.top_p, Some(top_p));
        assert_eq!(body.model, client.model);
    }

    #[test]
    fn test_params() {
        let messages: Vec<OpenAIChatMessage> = vec![];
        let temperature = 0.2;
        let frequency_penalty = 1.2;
        let presence_penalty = 32.1;
        let top_p = 2.0;
        let client = OpenaiV1 {
            temperature: Some(temperature),
            top_p: Some(top_p),
            frequency_penalty: Some(frequency_penalty),
            presence_penalty: Some(presence_penalty),
            response_format: None,
            tools: None,
            ..Default::default()
        };

        let b = client.get_request_body(messages, None);
        assert_eq!(&client.model, &b.model);
        assert_eq!(
            b.frequency_penalty.expect("frequency_penalty"),
            frequency_penalty
        );
        assert_eq!(
            b.presence_penalty.expect("presence_penalty"),
            presence_penalty
        );
        assert!(b.response_format.is_none());
        assert_eq!(b.temperature.expect("temperature"), temperature);
        assert!(b.tools.is_none());
        assert_eq!(client.count_tools(), 0);
        assert_eq!(b.top_p.expect("top_p"), top_p);
        assert!(b.messages.is_empty());

        assert_eq!(&client.model(), &b.model);
        assert_eq!(
            client.frequency_penalty().expect("frequency_penalty"),
            frequency_penalty
        );
        assert_eq!(
            client.presence_penalty().expect("presence_penalty"),
            presence_penalty
        );
        assert_eq!(client.temperature().expect("temperature"), temperature);
        assert_eq!(client.top_p().expect("top_p"), top_p);
        assert!(client.max_tokens().is_none());
        assert!(b.messages.is_empty());
        assert_eq!(client.count_tools(), 0);
    }

    #[test]
    fn set_streaming() {
        let mut req = OpenAIChatRequest::default();
        assert!(req.stream.is_none());
        req.set_streaming(true);
        assert!(req.stream.unwrap());
        req.set_streaming(false);
        assert!(!req.stream.unwrap());
    }

    #[test]
    fn response_msg() {
        let txt = "Hey!";
        let mut res = OpenAIChatResponse::default();
        assert!(res.message().is_none());
        res.choices.push(OpenAIMessageChoice {
            message: OpenAIChatMessage::from(txt.to_string()),
            ..Default::default()
        });
        assert!(res.message().is_some());
        let found = res.message().unwrap().content_string().unwrap();
        assert_eq!(txt, found);
    }

    #[test]
    fn msg_trait() {
        // Empty developer message
        let msg = OpenAIChatMessage::Developer(OpenAIDeveloperMessage::default());
        assert_eq!(msg.role(), OpenAIRole::Developer);
        assert!(!msg.has_content());
        assert!(msg.content_string().is_some()); // default content is empty string

        // String content in system message
        let txt1 = "hello!";
        let c = OpenAIMessageContent::String(txt1.to_string());
        let msg = OpenAIChatMessage::System(OpenAISystemMessage {
            content: c,
            name: None,
        });
        assert_eq!(msg.role(), OpenAIRole::System);
        assert!(msg.has_content());
        assert_eq!(msg.content_string().expect("content!"), txt1);

        // Parts content in user message
        let txt2 = "how is tha banana doing?";
        let c = OpenAIMessageContent::Parts(vec![
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt1.to_string(),
            }),
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt2.to_string(),
            }),
        ]);
        let msg = OpenAIChatMessage::User(OpenAIUserMessage {
            content: c,
            name: None,
        });
        assert_eq!(msg.role(), OpenAIRole::User);
        assert!(msg.has_content());
        assert_eq!(
            msg.content_string().expect("content!"),
            format!("{txt1}\n{txt2}")
        );
    }

    #[test]
    fn message_to_string() {
        let txt1 = "hello!";
        let c = OpenAIMessageContent::String(txt1.to_string());
        let msg = OpenAIChatMessage::User(OpenAIUserMessage {
            content: c,
            name: None,
        });
        let s = msg.to_string();
        assert_eq!(format!("{txt1}\n"), s); // println

        let txt2 = "how is tha banana doing?";
        let c = OpenAIMessageContent::Parts(vec![
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt1.to_string(),
            }),
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt2.to_string(),
            }),
        ]);
        let msg = OpenAIChatMessage::User(OpenAIUserMessage {
            content: c,
            name: None,
        });
        let s = msg.to_string();
        assert_eq!(s, format!("{txt1}\n{txt2}\n"));
    }

    #[test]
    fn content_to_string() {
        let txt1 = "hello!";
        let txt2 = "how is tha banana doing?";
        let c = OpenAIMessageContent::String(txt1.to_string());
        let s = String::from(&c);
        assert_eq!(&s, txt1);

        let c = OpenAIMessageContent::Parts(vec![
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt1.to_string(),
            }),
            OpenAIMessagePart::Text(OpenAITextPart {
                text: txt2.to_string(),
            }),
        ]);
        let s = String::from(&c);
        assert_eq!(s, format!("{txt1}\n{txt2}"));
    }

    #[test]
    fn default_msg_content() {
        let c = OpenAIMessageContent::default();
        if let OpenAIMessageContent::String(s) = c {
            assert!(s.is_empty());
        } else {
            panic!("expecting empty String part")
        }
    }

    #[test]
    fn msg_part_to_str() {
        let txt = "some text";
        let p = OpenAIMessagePart::Text(txt.to_string().into());
        let s = String::from(&p);
        assert_eq!(&s, txt);

        let p = OpenAIMessagePart::ImageUrl(OpenAIImagePart::default());
        let s = String::from(&p);
        assert_eq!(&s, "[image-url]");

        let p = OpenAIMessagePart::InputAudio(OpenAIAudioPart::default());
        let s = String::from(&p);
        assert_eq!(&s, "[input-audio]");

        let p = OpenAIMessagePart::File(OpenAIFilePart::default());
        let s = String::from(&p);
        assert_eq!(&s, "[file]");
    }

    #[test]
    fn text_part_from_str() {
        let txt = "some text";
        let part = OpenAITextPart::from(txt.to_string());
        assert_eq!(txt, part.text);

        let msgprt = OpenAIMessagePart::from(txt.to_string());
        if let OpenAIMessagePart::Text(part) = &msgprt {
            assert_eq!(txt, part.text);
        } else {
            panic!("oh no! Expecting message part::text")
        }
    }

    #[test]
    fn roles_str() {
        assert_eq!(OpenAIRole::Assistant.to_string(), "assistant");
        assert_eq!(OpenAIRole::User.to_string(), "user");
        assert_eq!(OpenAIRole::Developer.to_string(), "developer");
        assert_eq!(OpenAIRole::System.to_string(), "system");
        assert_eq!(OpenAIRole::Tool.to_string(), "tool");
    }

    #[test]
    fn tool_choice_custom() {
        let id = "some-id";
        let name = "tool-name";
        let args = "{\"the_input\": \"yes!\"}";
        let choice = OpenAIToolChoice::Custom {
            id: id.to_string(),
            function: super::OpenAICustomToolChoice {
                input: args.to_string(),
                name: name.to_string(),
            },
        };
        assert_eq!(choice.name(), name);
        assert_eq!(choice.id(), id);
        assert_eq!(choice.arguments(), args);
    }

    #[test]
    fn tool_choice_function() {
        let id = "some-id";
        let name = "tool-name";
        let args = "{\"the_input\": \"yes!\"}";
        let choice = OpenAIToolChoice::Function {
            id: id.to_string(),
            function: super::OpenAIFunctionToolChoice {
                arguments: args.to_string(),
                name: name.to_string(),
            },
        };
        assert_eq!(choice.name(), name);
        assert_eq!(choice.id(), id);
        assert_eq!(choice.arguments(), args);
    }

    #[test]
    fn tool_function_descriptions() {
        let name = "some-tool";
        let description = "the tool description";
        let tool = OpenAITool::Function {
            function: FunctionToolFields {
                name: name.to_string(),
                description: Some(description.to_string()),
                parameters: Some(crate::JsonSchema::default()),
                strict: None,
            },
        };
        assert_eq!(tool.name(), name);
        assert_eq!(tool.description(), description);
        let exp_params = serde_json::to_string(&JsonSchema::default()).unwrap();
        let p = tool.parameters().clone().unwrap();
        let params = serde_json::to_string(&p).unwrap();
        assert_eq!(params, exp_params);
    }

    #[test]
    fn tool_custom_descriptions() {
        let name = "some-tool";
        let description = "the tool description";
        let fmt = JsonSchema::default();
        let tool = OpenAITool::Custom(OpenAICustomTool {
            custom: CustomToolFields {
                name: name.to_string(),
                description: description.to_string(),
                format: serde_json::to_value(&fmt).unwrap(),
            },
        });

        assert_eq!(tool.name(), name);
        assert_eq!(tool.description(), description);
        // let exp_params = serde_json::to_string(&JsonSchema::default()).unwrap();
        // let p = tool.parameters().clone().unwrap();
        // let params = serde_json::to_string(&p).unwrap();
        // assert_eq!(params, exp_params);
    }

    #[test]
    fn serde_openai_voice() {
        let voice = OpenAIVoice::Echo;
        let s = serde_json::to_string(&voice).expect("could not serialise");
        assert_eq!("\"echo\"", s);
        let v2 = serde_json::from_str(&s).expect("could not deserialise");
        match v2 {
            OpenAIVoice::Echo => {}
            _ => panic!("Wrong!"),
        }
    }

    #[test]
    fn deserialise_openai_chat_request() {
        let json = r#"{
            "model": "gpt-5",
            "messages": [
              {
                "role": "developer",
                "content": "You are a helpful assistant."
              },
              {
                "role": "user",
                "content": "Hello!"
              }
            ]
          }"#;
        let _v: OpenAIChatRequest = serde_json::from_str(json).expect("could not deserialise");
    }
    #[test]
    fn deserialise_openai_chat_response() {
        let json = r#"{
                "id": "chatcmpl-B9MHDbslfkBeAs8l4bebGdFOJ6PeG",
                "object": "chat.completion",
                "created": 1741570283,
                "model": "gpt-4o-2024-08-06",
                "choices": [
                    {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "The image shows a wooden boardwalk path running through a lush green field or meadow. The sky is bright blue with some scattered clouds, giving the scene a serene and peaceful atmosphere. Trees and shrubs are visible in the background.",
                        "refusal": null,
                        "annotations": []
                    },
                    "logprobs": null,
                    "finish_reason": "stop"
                    }
                ],
                "usage": {
                    "prompt_tokens": 1117,
                    "completion_tokens": 46,
                    "total_tokens": 1163,
                    "prompt_tokens_details": {
                    "cached_tokens": 0,
                    "audio_tokens": 0
                    },
                    "completion_tokens_details": {
                    "reasoning_tokens": 0,
                    "audio_tokens": 0,
                    "accepted_prediction_tokens": 0,
                    "rejected_prediction_tokens": 0
                    }
                },
                "service_tier": "default",
                "system_fingerprint": "fp_fc9f1d7035"
                }
        "#;
        let _v: OpenAIChatResponse = serde_json::from_str(json).expect("could not deserialise");
    }

    #[test]
    fn deserialise_chunk() {
        let json = r#"{
          "choices":[{
                "finish_reason":null,
                "index":0,
                "logprobs":null,
                "delta":{
                    "role":"assistant",
                    "content":""
                }
            }],
          "created":1694268190,
          "id":"chatcmpl-123",
          "model":"gpt-4o-mini",
          "object":"chat.completion.chunk",
          "system_fingerprint": "fp_44709d6fcb"
        }"#;
        let _: OpenAIChatChunk =
            serde_json::from_str(&json).expect("Failed to deserialize OpenAIChatChunk");
    }

    #[test]
    fn deserialise_chunk2() {
        let json = r#"{
            "id": "chatcmpl-C7Huxa7gmbvE4Vr9M8SfQ8lo25LWk",
            "object": "chat.completion.chunk",
            "created": 1755853207,
            "model": "gpt-3.5-turbo-0125",
            "service_tier": "default",
            "system_fingerprint": null,
            "choices": [
                {
                    "index": 0,
                    "delta": {},
                    "logprobs": null,
                    "finish_reason": "stop"
                }
            ],
            "obfuscation": "t"
        }"#;
        let _: OpenAIChatChunk =
            serde_json::from_str(&json).expect("Failed to deserialize OpenAIChatChunk");
    }

    #[test]
    fn deserialise_final_usage_chunk() {
        // The chunk emitted when stream_options.include_usage is set: empty
        // choices, populated usage.
        let json = r#"{
            "id": "chatcmpl-usage",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o",
            "choices": [],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 22,
                "total_tokens": 33
            }
        }"#;
        let chunk: OpenAIChatChunk =
            serde_json::from_str(json).expect("Failed to deserialize usage chunk");
        assert!(chunk.choices.is_empty());
        let usage = chunk.usage.expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 11);
        assert_eq!(usage.completion_tokens, 22);
        assert_eq!(usage.total_tokens, 33);
    }

    #[test]
    fn deserialise_chunk_with_tool_call_fragments() {
        // First fragment: carries id, type and function name with empty args.
        let first = r#"{
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": ""}
                    }]
                },
                "finish_reason": null
            }]
        }"#;
        // Second fragment: only carries an arguments piece.
        let second = r#"{
            "id": "chatcmpl-tool",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {"arguments": "{\"city\":\"Paris\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;

        let first: OpenAIChatChunk = serde_json::from_str(first).expect("first fragment");
        let second: OpenAIChatChunk = serde_json::from_str(second).expect("second fragment");

        let mut acc = ToolCallAccumulator::default();
        assert!(acc.is_empty());
        acc.apply(first.choices[0].delta.tool_calls.clone().unwrap());
        acc.apply(second.choices[0].delta.tool_calls.clone().unwrap());
        assert!(!acc.is_empty());

        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id(), "call_abc");
        assert_eq!(calls[0].name(), "get_weather");
        assert_eq!(calls[0].arguments(), r#"{"city":"Paris"}"#);
    }

    #[test]
    fn tool_call_accumulator_merges_multiple_calls_by_index() {
        let mut acc = ToolCallAccumulator::default();
        // Two calls interleaved across fragments, keyed by index.
        acc.apply(vec![
            OpenAIToolCallDelta {
                index: 0,
                id: Some("call_0".to_string()),
                call_type: Some("function".to_string()),
                function: Some(OpenAIFunctionCallDelta {
                    name: Some("a".to_string()),
                    arguments: Some("{\"x\":".to_string()),
                }),
            },
            OpenAIToolCallDelta {
                index: 1,
                id: Some("call_1".to_string()),
                call_type: Some("function".to_string()),
                function: Some(OpenAIFunctionCallDelta {
                    name: Some("b".to_string()),
                    arguments: Some("{\"y\":".to_string()),
                }),
            },
        ]);
        acc.apply(vec![
            OpenAIToolCallDelta {
                index: 1,
                function: Some(OpenAIFunctionCallDelta {
                    name: None,
                    arguments: Some("2}".to_string()),
                }),
                ..Default::default()
            },
            OpenAIToolCallDelta {
                index: 0,
                function: Some(OpenAIFunctionCallDelta {
                    name: None,
                    arguments: Some("1}".to_string()),
                }),
                ..Default::default()
            },
        ]);

        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id(), "call_0");
        assert_eq!(calls[0].arguments(), r#"{"x":1}"#);
        assert_eq!(calls[1].id(), "call_1");
        assert_eq!(calls[1].arguments(), r#"{"y":2}"#);
    }

    #[test]
    fn deserialise_message_parts() {
        let json = r#"{
            "model": "gpt-4.1",
            "messages": [
              {
                "role": "user",
                "content": [
                  {
                    "type": "text",
                    "text": "What is in this image?"
                  },
                  {
                    "type": "image_url",
                    "image_url": {
                      "url": "https://upload.wikimedia.org/wikipedia/commons/thumb/d/dd/Gfp-wisconsin-madison-the-nature-boardwalk.jpg/2560px-Gfp-wisconsin-madison-the-nature-boardwalk.jpg"
                    }
                  }
                ]
              }
            ],
            "max_tokens": 300
          }"#;
        let _: OpenAIChatRequest =
            serde_json::from_str(&json).expect("Failed to deserialize OpenAIChatRequest");
    }

    #[test]
    fn deserialise_message_text() {
        let json = r#"{
            "model": "gpt-4.1",
            "messages": [
              {
                "role": "user",
                "content": "Hello"
              }
            ],
            "max_tokens": 300
          }"#;
        let _d: OpenAIChatRequest =
            serde_json::from_str(&json).expect("Failed to deserialize OpenAIChatRequest");
    }

    #[test]
    fn deserialise_function_tool_params() {
        let tool = OpenAITool::Function {
            function: FunctionToolFields {
                name: "get_horoscope".to_string(),
                description: Some("Get today's horoscope for an astrological sign.".to_string()),
                parameters: serde_json::from_str(
                    r#"{
                "type": "object",
                "properties": {
                    "sign": {
                        "type": "string",
                        "description": "An astrological sign like Taurus or Aquarius"
                    }
                },
                "required": ["sign"]
            }"#,
                )
                .expect("Failed to deserialize FunctionToolFields"),
                strict: None,
            },
        };

        println!(
            "{}",
            serde_json::to_string(&tool).expect("Failed to serialize OpenAITool")
        );

        let json = r#"{
                "type": "function",
                "function": {
                    "name": "get_horoscope",
                    "description": "Get today's horoscope for an astrological sign.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "sign": {
                                "type": "string",
                                "description": "An astrological sign like Taurus or Aquarius"
                            }
                        },
                        "required": ["sign"]
                    }
                }
            }"#;

        let tool: OpenAITool =
            serde_json::from_str(&json).expect("Failed to deserialize OpenAITool");
        if let OpenAITool::Function { function } = tool {
            assert_eq!(function.name, "get_horoscope".to_string());
        } else {
            panic!("Wrong tool!")
        }
    }
}
