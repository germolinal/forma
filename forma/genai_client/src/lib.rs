#![warn(clippy::dbg_macro)]

pub mod events;
pub mod telemetry;
pub mod vertex_ai;
use async_trait::async_trait;
use derive::FormaDocs;
pub use events::SSEvent;
use serde::{Deserialize, Serialize};
pub mod audio;
pub use audio::AudioFormat;
pub mod schema;
pub use schema::JsonSchema;
pub mod genai_client_trait;

pub mod groq;
pub use groq::Groq;

pub mod openai;

pub use genai_client_trait::{ByteStream, GenaiClientTrait, GenaiMessageTrait, GenaiRequestTrait};

pub mod ollama;
use crate::{
    genai_client_trait::LLMProvider,
    openai::v1::{
        OpenAIAudioResponse, OpenAILogProb, OpenAIMessageChoiceDelta, OpenAIMessagePart,
        OpenAITextPart, OpenAIToolChoice, OpenAIToolChoiceMode, OpenAIUsage,
    },
    vertex_ai::VertexAI,
};
pub use crate::{
    ollama::Ollama,
    openai::v1::{
        OpenAIAssistantMessage, OpenAIChatChunk, OpenAIChatMessage, OpenAIChatRequest,
        OpenAIChatResponse, OpenAIDeveloperMessage, OpenAIMessageContent, OpenAIRole,
        OpenAISystemMessage, OpenAITool, OpenAIToolMessage, OpenAIUserMessage, OpenaiV1,
    },
};

/// Defines the LLM Provider, the model and the settings
/// utilise to generate responses
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(tag = "provider")]
#[serde(rename_all = "kebab-case")]
pub enum GenaiClient {
    /// A client for [Ollama](https://ollama.com) inference.
    ///
    /// Ideal for running local processes in an easy manner during
    /// development
    Ollama(Ollama),

    /// A client for [OpenAI's](https://openai.com) v1 API
    OpenaiV1(OpenaiV1),

    /// Client for [Groq](https://groq.com)
    Groq(Groq),

    /// Client for Google's [Vertex AI](https://cloud.google.com/vertex-ai) platform
    VertexAi(VertexAI),
}

/// The reason the model stopped generating tokens.
/// This will be `stop` if the model hit a natural stop
/// point or a provided stop sequence, `length` if the
/// maximum number of tokens specified in the request
/// was reached, `content_filter` if content was omitted
/// due to a flag from our content filters, `tool_calls`
/// if the model called a tool, or `function_call` (deprecated)
/// if the model called a function.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LLMFinishReason {
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
    /// The model tried to call a tool but produced an invalid call (Gemini's
    /// `MALFORMED_FUNCTION_CALL`). A mistake of the model's: asking again
    /// usually works.
    MalformedToolCall,
    /// The finish reason is not known or not handled.
    Other(String),
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct MessageChoice {
    pub finish_reason: LLMFinishReason,
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<OpenAILogProb>,
    pub message: LLMMessage,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct ChatResponse {
    pub choices: Vec<MessageChoice>,
    /// The Unix timestamp (in seconds) of when the chat completion was created.
    pub created: usize,
    /// Unique identifier for the chat completion
    pub id: String,
    /// The model used
    pub model: String,
    /// The object type, which is always `chat.completion`
    pub object: String,
    /// Usage statistics for the completion request
    pub usage: OpenAIUsage,
}

impl ChatResponse {
    /// Retrieves the first message from all choices
    pub fn message(&self) -> Option<&LLMMessage> {
        if let Some(m) = self.choices.last() {
            Some(&m.message)
        } else {
            None
        }
    }
}

pub type LLMResponse = ChatResponse;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct LLMChunk {
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

/// User message (role: "user")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct UserMessage {
    #[serde(default)]
    pub content: Vec<OpenAIMessagePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Assistant message (role: "assistant")
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AssistantMessage {
    /// What the model said: its text, and the files it produced.
    #[serde(default)]
    pub content: Vec<OpenAIMessagePart>,
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
    /// The tool proposed made by the assistant
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAIToolChoice>,
    /// The thought signature of the part.
    // JSON has no bytes: serde writes them as an array of numbers.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Vec<u8>>))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<bytes::Bytes>,
}

impl From<&AssistantMessage> for String {
    fn from(val: &AssistantMessage) -> Self {
        let strings: Vec<String> = val.content.iter().map(|x| x.into()).collect();
        strings.join("\n")
    }
}

impl From<AssistantMessage> for String {
    fn from(val: AssistantMessage) -> Self {
        let strings: Vec<String> = val.content.iter().map(|x| x.into()).collect();
        strings.join("\n")
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "role")]
#[serde(rename_all = "snake_case")]
pub enum LLMMessage {
    System(OpenAISystemMessage),
    User(UserMessage),
    Developer(OpenAIDeveloperMessage),
    Assistant(AssistantMessage),
    Tool(OpenAIToolMessage),
}

impl From<LLMMessage> for serde_json::Value {
    fn from(message: LLMMessage) -> Self {
        serde_json::to_value(&message).expect("could not turn message into json value")
    }
}

impl std::fmt::Display for LLMMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let content = self.content_string().unwrap_or_default();
        write!(f, "{}: {}", self.role(), content)
    }
}

impl Default for LLMMessage {
    fn default() -> Self {
        Self::User(UserMessage::default())
    }
}

impl From<String> for AssistantMessage {
    fn from(text: String) -> Self {
        Self {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart { text })],
            name: None,
            ..Default::default()
        }
    }
}
impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        Self {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart { text })],
            name: None,
        }
    }
}

impl From<String> for LLMMessage {
    fn from(text: String) -> Self {
        Self::User(text.into())
    }
}

impl GenaiMessageTrait for LLMMessage {
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
        match self {
            Self::Developer(m) => Some((&m.content).into()),
            Self::System(m) => Some((&m.content).into()),
            Self::User(m) => m.content.iter().find_map(|p| {
                if let OpenAIMessagePart::Text(t) = p {
                    Some(t.text.clone())
                } else {
                    None
                }
            }),
            Self::Assistant(m) => m.content.iter().find_map(|p| {
                if let OpenAIMessagePart::Text(t) = p {
                    Some(t.text.clone())
                } else {
                    None
                }
            }),
            Self::Tool(m) => Some((&m.content).into()),
        }
    }

    fn has_content(&self) -> bool {
        match self {
            Self::System(m) => !m.content.is_empty(),
            Self::User(m) => !m.content.is_empty(),
            Self::Developer(m) => !m.content.is_empty(),
            Self::Assistant(m) => !m.content.is_empty(),
            Self::Tool(m) => !m.content.is_empty(),
        }
    }
}

pub type LLMTool = OpenAITool;
pub type Role = OpenAIRole;

pub struct ChatEndpoint;

#[async_trait]
impl GenaiClientTrait for GenaiClient {
    type Chunk = LLMChunk;
    type Response = LLMResponse;
    type Message = LLMMessage;
    type Tool = LLMTool;

    const PROVIDER: LLMProvider = LLMProvider::Generic;

    fn otel_provider(&self) -> &str {
        match self {
            Self::OpenaiV1(c) => c.otel_provider(),
            Self::Ollama(c) => c.otel_provider(),
            Self::Groq(c) => c.otel_provider(),
            Self::VertexAi(c) => c.otel_provider(),
        }
    }

    fn openinference_provider(&self) -> &str {
        match self {
            Self::OpenaiV1(c) => c.openinference_provider(),
            Self::Ollama(c) => c.openinference_provider(),
            Self::Groq(c) => c.openinference_provider(),
            Self::VertexAi(c) => c.openinference_provider(),
        }
    }
    fn openinference_system(&self) -> &str {
        match self {
            Self::OpenaiV1(c) => c.openinference_system(),
            Self::Ollama(c) => c.openinference_system(),
            Self::Groq(c) => c.openinference_system(),
            Self::VertexAi(c) => c.openinference_system(),
        }
    }

    fn frequency_penalty(&self) -> Option<f64> {
        match self {
            Self::OpenaiV1(c) => c.frequency_penalty(),
            Self::Ollama(c) => c.frequency_penalty(),
            Self::Groq(c) => c.frequency_penalty(),
            Self::VertexAi(c) => c.frequency_penalty(),
        }
    }
    fn max_tokens(&self) -> Option<usize> {
        match self {
            Self::OpenaiV1(c) => c.max_tokens(),
            Self::Ollama(c) => c.max_tokens(),
            Self::Groq(c) => c.max_tokens(),
            Self::VertexAi(c) => c.max_tokens(),
        }
    }
    fn presence_penalty(&self) -> Option<f64> {
        match self {
            Self::OpenaiV1(c) => c.presence_penalty(),
            Self::Ollama(c) => c.presence_penalty(),
            Self::Groq(c) => c.presence_penalty(),
            Self::VertexAi(c) => c.presence_penalty(),
        }
    }
    fn temperature(&self) -> Option<f64> {
        match self {
            Self::OpenaiV1(c) => c.temperature(),
            Self::Ollama(c) => c.temperature(),
            Self::Groq(c) => c.temperature(),
            Self::VertexAi(c) => c.temperature(),
        }
    }
    fn top_p(&self) -> Option<f64> {
        match self {
            Self::OpenaiV1(c) => c.top_p(),
            Self::Ollama(c) => c.top_p(),
            Self::Groq(c) => c.top_p(),
            Self::VertexAi(c) => c.top_p(),
        }
    }

    fn model(&self) -> &str {
        match self {
            Self::OpenaiV1(c) => c.model(),
            Self::Ollama(c) => c.model(),
            Self::Groq(c) => c.model(),
            Self::VertexAi(c) => c.model(),
        }
    }

    fn set_tools(&mut self, tools: Option<Vec<Self::Tool>>) {
        match self {
            Self::OpenaiV1(c) => c.set_tools(tools),
            Self::Ollama(c) => c.set_tools(tools),
            Self::Groq(c) => c.set_tools(tools),
            Self::VertexAi(c) => {
                let tools = match tools {
                    Some(t) => {
                        let t = t.into_iter().map(|x| x.into()).collect();
                        Some(t)
                    }
                    None => None,
                };
                c.set_tools(tools)
            }
        }
    }

    fn set_tool_choice_mode(&mut self, tool_choice: Option<OpenAIToolChoiceMode>) {
        match self {
            Self::OpenaiV1(c) => c.set_tool_choice_mode(tool_choice),
            Self::Ollama(c) => c.set_tool_choice_mode(tool_choice),
            Self::Groq(c) => c.set_tool_choice_mode(tool_choice),
            Self::VertexAi(c) => c.set_tool_choice_mode(tool_choice),
        }
    }

    fn set_output_schema(&mut self, schema: openai::v1::OpenAIResponseFormat) {
        match self {
            Self::OpenaiV1(c) => c.set_output_schema(schema),
            Self::Ollama(c) => c.set_output_schema(schema),
            Self::Groq(c) => c.set_output_schema(schema),
            Self::VertexAi(c) => c.set_output_schema(schema),
        }
    }

    fn has_output_schema(&self) -> bool {
        match self {
            Self::OpenaiV1(c) => c.has_output_schema(),
            Self::Ollama(c) => c.has_output_schema(),
            Self::Groq(c) => c.has_output_schema(),
            Self::VertexAi(c) => c.has_output_schema(),
        }
    }

    fn count_tools(&self) -> usize {
        match self {
            Self::OpenaiV1(c) => c.count_tools(),
            Self::Ollama(c) => c.count_tools(),
            Self::Groq(c) => c.count_tools(),
            Self::VertexAi(c) => c.count_tools(),
        }
    }

    fn chat_endpoint(&self) -> Result<String, String> {
        match &self {
            Self::Ollama(c) => c.chat_endpoint(),
            Self::OpenaiV1(c) => c.chat_endpoint(),
            Self::Groq(c) => c.chat_endpoint(),
            Self::VertexAi(c) => c.chat_endpoint(),
        }
    }

    fn server_address(&self) -> String {
        match &self {
            Self::Ollama(c) => c.server_address(),
            Self::OpenaiV1(c) => c.server_address(),
            Self::Groq(c) => c.server_address(),
            Self::VertexAi(c) => c.server_address(),
        }
    }

    /// Generate a response from the LLM client
    /// If `allowed_tools` is `None`, all tools are made available;
    /// if it is `Some(vector)`, only those within that list are allowed.
    /// If some IDs in `allowed_tools` are not found, they are simply ignored.
    async fn stream<
        M: Into<Self::Message> + Send + 'static,
        O: From<Self::Chunk> + Send + 'static,
    >(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<ByteStream<O>, String> {
        use futures::StreamExt;

        let messages: Vec<Self::Message> = messages.into_iter().map(|x| x.into()).collect();

        // Every backend's chunk type implements `Into<LLMChunk>`, so we ask each
        // client for a `ByteStream<LLMChunk>` and then map that single stream into
        // the caller-requested `O` (guaranteed by `O: From<Self::Chunk> = From<LLMChunk>`).
        let stream: ByteStream<Self::Chunk> = match &self {
            Self::Ollama(c) => c.stream(messages, allowed_tools).await?,
            Self::OpenaiV1(c) => c.stream(messages, allowed_tools).await?,
            Self::Groq(c) => c.stream(messages, allowed_tools).await?,
            Self::VertexAi(c) => {
                c.stream(vertex_ai::resolve_tool_call_names(messages), allowed_tools)
                    .await?
            }
        };

        Ok(Box::pin(stream.map(Into::into)))
    }

    /// Generate a response from the LLM client
    /// If `allowed_tools` is `None`, all tools are made available;
    /// if it is `Some(vector)`, only those within that list are allowed.
    /// If some IDs in `allowed_tools` are not found, they are simply ignored.
    async fn generate<M: Into<Self::Message> + Send + 'static>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<Self::Response, String> {
        let messages: Vec<Self::Message> = messages.into_iter().map(|x| x.into()).collect();
        match &self {
            Self::Ollama(c) => {
                let r = c.generate(messages, allowed_tools).await?;
                Ok(r.into())
            }
            Self::OpenaiV1(c) => {
                let messages: Vec<OpenAIChatMessage> =
                    messages.into_iter().map(|x| x.into()).collect();
                let r = c.generate(messages, allowed_tools).await?;
                Ok(r.into())
            }
            Self::Groq(c) => {
                let r = c.generate(messages, allowed_tools).await?;
                Ok(r.into())
            }
            Self::VertexAi(c) => {
                let messages = vertex_ai::resolve_tool_call_names(messages);
                let r = c.generate(messages, allowed_tools).await?;
                Ok(r.into())
            }
        }
    }
}

impl Default for GenaiClient {
    fn default() -> Self {
        Self::Ollama(Ollama::default())
    }
}
#[cfg(test)]
mod tests {
    use crate::openai::v1::FunctionToolFields;

    use super::*;

    // Helper to build one of each enum variant (requires Default for inner types)
    fn all_variants() -> Vec<GenaiClient> {
        vec![
            GenaiClient::Ollama(Ollama::default()),
            GenaiClient::OpenaiV1(OpenaiV1::default()),
            GenaiClient::Groq(Groq::default()),
            GenaiClient::VertexAi(VertexAI::default()),
        ]
    }

    #[test]
    fn test_sync_dispatch_basic_infos() {
        unsafe {
            std::env::set_var("GOOGLE_CLOUD_PROJECT", "FAKE_GCP_PROJECT");
        }
        for client in all_variants() {
            // Ensure provider strings are accessible and non-empty
            let otel = client.otel_provider();
            assert!(!otel.is_empty(), "otel_provider empty for {:?}", client);

            let openinf = client.openinference_provider();
            assert!(
                !openinf.is_empty(),
                "openinference_provider empty for {:?}",
                client
            );

            let openinf_sys = client.openinference_system();
            assert!(
                !openinf_sys.is_empty(),
                "openinference_system empty for {:?}",
                client
            );

            // model and chat endpoint should be accessible
            let model = client.model();
            assert!(!model.is_empty(), "model empty for {:?}", client);

            let endpoint = client.chat_endpoint().expect("no endpoint");
            assert!(!endpoint.is_empty(), "chat_endpoint empty for {:?}", client);

            // Numeric option getters should not panic; accept any Option
            let _ = client.frequency_penalty();
            let _ = client.max_tokens();
            let _ = client.presence_penalty();
            let _ = client.temperature();
            let _ = client.top_p();

            // Counting tools should return a usize without panicking
            let _ = client.count_tools();
        }
    }

    #[test]
    fn test_set_tools_none_and_count() {
        for mut client in all_variants() {
            // setting None should be supported for all implementations
            client.set_tools(None);
            // after setting None, count_tools must still return
            let i = client.count_tools();
            assert_eq!(i, 0);
        }
    }

    #[test]
    fn test_set_tools_and_count() {
        let tools = vec![OpenAITool::Function {
            function: FunctionToolFields::default(),
        }];
        for mut client in all_variants() {
            // setting None should be supported for all implementations
            client.set_tools(Some(tools.clone()));
            // after setting None, count_tools must still return
            let i = client.count_tools();
            assert_eq!(i, 1);
        }
    }

    #[test]
    fn default() {
        if let GenaiClient::Ollama(_) = GenaiClient::default() {
            println!("ok")
        } else {
            panic!("ollama is not default!")
        }
    }

    /// A default client serializes to `provider` + `model` only, so switching
    /// the provider tag yields a valid client for every other provider.
    #[test]
    fn test_default_clients_are_interchangeable() {
        let providers = ["ollama", "openai-v1", "groq", "vertex-ai"];
        for client in all_variants() {
            let json = serde_json::to_value(&client).unwrap();
            let obj = json.as_object().unwrap();
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort();
            assert_eq!(keys, ["model", "provider"], "{json}");

            for provider in providers {
                let mut other = json.clone();
                other["provider"] = provider.into();
                let parsed: GenaiClient = serde_json::from_value(other)
                    .unwrap_or_else(|e| panic!("{json} as {provider}: {e}"));
                assert_eq!(parsed.model(), client.model());
            }
        }
    }

    #[test]
    fn test_set_output_schema_accepts_value() {
        for mut client in all_variants() {
            // Attempt to set a default OpenAI response format; if the concrete
            // type doesn't provide a Default, constructing a simple value via
            // the public API should be possible in most implementations.
            //
            // Use a minimal / fallback value via Default if available.
            let schema = crate::openai::v1::OpenAIResponseFormat::default();
            client.set_output_schema(schema);
        }
    }
}
