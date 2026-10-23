use crate::{
    LLMChunk, LLMMessage, LLMResponse, LLMTool, Role,
    openai::v1::{OpenAIResponseFormat, OpenAIToolChoiceMode},
};
use async_trait::async_trait;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::{Client, Error as ReqwestError, RequestBuilder};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt::Debug, pin::Pin, sync::LazyLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_stream::wrappers::LinesStream;
use tokio_util::io::StreamReader;

pub type ByteStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

/// A list of known LLM Providers supported
/// by Forma. These are defined as expectd by
/// the [Opentelemetry semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/)
/// and can be transforemd into [OpenInference ones](https://arize-ai.github.io/openinference/spec/semantic_conventions.html)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LLMProvider {
    /// A value for clients that allow for multiple providers
    Generic,
    /// Ollama
    Ollama,
    /// OpenAI Provider
    OpenAI,
    /// Groq
    Groq,
    /// Vertex AI
    VertexAI,
    /// Gooogle GenAI
    GoogleGenAI,
    /// Anthropic
    Anthropic,
    // MistralAI
    // Cohere
    // Azure
    // Aws
}

impl LLMProvider {
    /// Returns the string value in opentelemetry
    /// terms
    pub fn otel(&self) -> &str {
        match self {
            Self::Generic => "generic",
            Self::Ollama => "ollama",
            Self::OpenAI => "openai",
            Self::Groq => "groq",
            Self::VertexAI => "gcp.vertex_ai",
            Self::GoogleGenAI => "gcp.gen_ai",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn openinference_system(&self) -> &str {
        match self {
            Self::Generic => "generic",
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::VertexAI => "vertexai",
            Self::GoogleGenAI => "vertexai",
            // Self::MistralAI => "mistralai"
            // Self::Cohere => "cohere"
            Self::Ollama => "ollama",
            Self::Groq => "groq",
        }
    }
    pub fn openinference_provider(&self) -> &str {
        match self {
            Self::Generic => "generic",
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            // Self::Cohere => "cohere",
            // Self::MistralAI => "mistralai",
            // Self::Azure => "azure",
            Self::VertexAI => "google",
            Self::GoogleGenAI => "google",
            // Self::Aws => "aws",
            Self::Groq => "groq",
            Self::Ollama => "ollama",
        }
    }
}

/// Necessary methods a that any ChatMessage should comply with
pub trait GenaiMessageTrait:
    Serialize + for<'a> Deserialize<'a> + Debug + Clone + From<LLMMessage>
{
    /// Gets the message role in our common
    fn role(&self) -> Role;

    /// Checks if the message has any content in it
    fn has_content(&self) -> bool;

    /// Retrieves the String content from a message, if any
    fn content_string(&self) -> Option<String>;
}

pub trait GenaiRequestTrait: Serialize + for<'a> Deserialize<'a> + Debug + Clone {
    fn set_streaming(&mut self, streaming: bool);
}

/// The HTTP client every request to a provider goes through, so that its
/// connections are kept and reused. A client per request opened a new
/// connection per LLM call: a TCP and a TLS handshake every time, and, under
/// load, every local port left waiting in TIME_WAIT until no call could
/// connect at all.
static HTTP_CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

/// Sends a rest_request
pub(crate) fn rest_request<C: GenaiClientTrait, B: Serialize>(
    client: &C,
    body: &B,
) -> Result<RequestBuilder, String> {
    // Let reqwest set Content-Type automatically
    let url = client.chat_endpoint()?;
    let mut c = HTTP_CLIENT.post(url).json(body);

    if let Some(api_key_name) = C::API_KEY_NAME {
        let errmsg = format!("could not find API key in environment variable '{api_key_name}'");
        let key = std::env::var(api_key_name).expect(&errmsg);
        c = c.header("Authorization", format!("Bearer {key}"));
    }

    Ok(c)
}

/// A trait that sets common required functionality
/// for all GenAI Clients that will be exposed to
/// external users.
///
/// The difference between these clients and native APIs
/// is that these clients contain all the parameters, meaning
/// that they only allow users to send different requests.
/// For instance, they allow users to say "Hi", "Bye" or
/// whatever, but the temperature, max tokens and every
/// other parameter is set within the client itself.
///
/// This has two main benefits: First, it prevents users from
/// overriding parameters; and second, it simplifies the input
/// messages and allow for an extraordinary polyphormism:
/// messages will never have tools (those are NOT sent by users).
/// You can just have a Role and some content, and that is that.
#[async_trait]
pub trait GenaiClientTrait {
    /// The main Chat Message as sent by users.
    type Message: GenaiMessageTrait;

    /// A chunk of a message, returned when streaming.
    type Chunk: Serialize + for<'a> Deserialize<'a> + Debug + 'static + Into<LLMChunk>;

    /// A full response returned when not-streaming
    /// (this one CAN have tools and other things, as
    /// populated by the LLM)
    type Response: Serialize + for<'a> Deserialize<'a> + Debug + Into<LLMResponse>;

    /// A tool that the LLM can call
    type Tool: From<LLMTool> + Serialize + for<'a> Deserialize<'a> + 'static;

    /// An optional value for the envrionment variable
    /// containing the API Key to attach in the
    /// `Authorization: Bearer` header
    const API_KEY_NAME: Option<&'static str> = None;

    /// The name of the provider, for OpenTelemetry purposes
    const PROVIDER: LLMProvider;

    /// Returns the name of the provider, for OpenTelemetry purposes
    fn otel_provider(&self) -> &str {
        Self::PROVIDER.otel()
    }

    /// Returns the name of the provider, for OpenInference purposes
    fn openinference_provider(&self) -> &str {
        Self::PROVIDER.openinference_provider()
    }

    /// Returns the name of the system, for OpenInference purposes
    fn openinference_system(&self) -> &str {
        Self::PROVIDER.openinference_system()
    }

    /// Returns the frequency penalty used to generate content
    fn frequency_penalty(&self) -> Option<f64>;

    /// Returns the maximum number of allowed tokens
    fn max_tokens(&self) -> Option<usize>;

    /// Returns the presence penalty used to generate content
    fn presence_penalty(&self) -> Option<f64>;

    /// Returns the temperature used to generate content
    fn temperature(&self) -> Option<f64>;

    /// Returns the `top_p` used to generate content
    fn top_p(&self) -> Option<f64>;

    /// Returns the model used to generate content
    fn model(&self) -> &str;

    /// Set the output schema
    fn set_output_schema(&mut self, schema: OpenAIResponseFormat);

    /// Checks whether it has an output schema
    fn has_output_schema(&self) -> bool;

    /// Set the tools to the model
    fn set_tools(&mut self, tools: Option<Vec<Self::Tool>>);

    /// Set the tool choice mode to the model
    fn set_tool_choice_mode(&mut self, tool_choice: Option<OpenAIToolChoiceMode>);

    /// Counts the tools for the model
    fn count_tools(&self) -> usize;

    /// The HTTP endpoint where to send requests
    fn chat_endpoint(&self) -> Result<String, String>;

    /// Streams results
    async fn stream<
        M: Into<Self::Message> + Send + 'static,
        O: From<Self::Chunk> + Send + 'static,
    >(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<ByteStream<O>, ReqwestError>;

    /// Generates results
    async fn generate<M: Into<Self::Message> + Send + 'static>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<Self::Response, String>;
}

pub(crate) async fn post_stream<T, O>(
    request: RequestBuilder,
) -> Result<ByteStream<O>, ReqwestError>
where
    T: Serialize + for<'a> Deserialize<'a> + Debug + 'static + Into<O>,
{
    let res = request.send().await?;
    // A refusal (a malformed conversation, a rate limit, an outage) is an
    // error for the caller to report, not a reason to bring the run down.
    if let Some(e) = res.error_for_status_ref().err() {
        let body = res.text().await.unwrap_or_default();
        tracing::error!(error = %e, body, "the provider refused the request");
        return Err(e);
    }

    // Turn the response stream into a line-oriented stream
    // let byte_stream = res.bytes_stream();
    let byte_stream = res
        .bytes_stream()
        .map_err(|err| std::io::Error::other(format!("reqwest error: {err}")));

    let reader = BufReader::new(StreamReader::new(byte_stream));
    let lines = LinesStream::new(reader.lines());

    // Parse lines
    let ret = lines.filter_map(|line_result| async move {
        let line = line_result.ok()?;
        let trimmed = line.trim();

        // Only care about lines starting with "data: "
        if trimmed.starts_with("data: ") {
            let data = trimmed.strip_prefix("data: ")?;

            if data == "[DONE]" {
                return None; // swallow sentinel
            }

            // Try parsing JSON
            match serde_json::from_str::<T>(data) {
                Ok(val) => {
                    let o: O = val.into();
                    Some(o)
                }
                Err(err) => {
                    eprintln!("JSON parse error: {err}, input was: {data}");
                    None
                }
            }
        } else {
            None
        }
    });

    Ok(Box::pin(ret))
}

pub(crate) async fn post<T>(request: RequestBuilder) -> Result<T, String>
where
    T: Serialize + for<'a> Deserialize<'a> + Debug,
{
    let res = request.send().await.map_err(|e| {
        if let Some(s) = e.source() {
            format!("'{e}' caused by '{s}'")
        } else {
            format!("'{e}' caused by 'unknonwn source'")
        }
    })?;
    if !res.status().is_success() {
        let req_e = res.error_for_status_ref().unwrap_err();
        let e = res.text().await.map_err(|e| format!("{e}"))?;
        return Err(format!("{req_e}: '{e}'"));
    }

    let bytes = res.bytes().await.map_err(|e| format!("{e}"))?;
    match serde_json::from_slice::<T>(&bytes) {
        Ok(val) => Ok(val),
        Err(err) => panic!(
            "JSON parse error: {err}, input was: {:?}",
            String::from_utf8_lossy(&bytes)
        ),
    }
}

#[cfg(test)]
mod test {
    use crate::genai_client_trait::LLMProvider;

    #[test]
    fn otel() {
        assert_eq!(LLMProvider::Generic.otel(), "generic");
        assert_eq!(LLMProvider::Ollama.otel(), "ollama");
        assert_eq!(LLMProvider::OpenAI.otel(), "openai");
        assert_eq!(LLMProvider::Groq.otel(), "groq");
        assert_eq!(LLMProvider::VertexAI.otel(), "gcp.vertex_ai");
        assert_eq!(LLMProvider::GoogleGenAI.otel(), "gcp.gen_ai");
        assert_eq!(LLMProvider::Anthropic.otel(), "anthropic");
    }

    #[test]
    fn openinf_system() {
        assert_eq!(LLMProvider::Generic.openinference_system(), "generic");
        assert_eq!(LLMProvider::Anthropic.openinference_system(), "anthropic");
        assert_eq!(LLMProvider::OpenAI.openinference_system(), "openai");
        assert_eq!(LLMProvider::VertexAI.openinference_system(), "vertexai");
        assert_eq!(LLMProvider::Ollama.openinference_system(), "ollama");
        assert_eq!(LLMProvider::Groq.openinference_system(), "groq");
        assert_eq!(LLMProvider::GoogleGenAI.openinference_system(), "vertexai");
    }
    #[test]
    fn openinf_provider() {
        assert_eq!(LLMProvider::Generic.openinference_provider(), "generic");
        assert_eq!(LLMProvider::Anthropic.openinference_provider(), "anthropic");
        assert_eq!(LLMProvider::OpenAI.openinference_provider(), "openai");
        assert_eq!(LLMProvider::VertexAI.openinference_provider(), "google");
        assert_eq!(LLMProvider::Ollama.openinference_provider(), "ollama");
        assert_eq!(LLMProvider::Groq.openinference_provider(), "groq");
        assert_eq!(LLMProvider::GoogleGenAI.openinference_provider(), "google");
    }
}
