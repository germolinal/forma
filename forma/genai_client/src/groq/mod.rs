use crate::genai_client_trait::{GenaiClientTrait, LLMProvider, post, post_stream, rest_request};
use crate::openai::v1::{OpenAIResponseFormat, OpenAIStreamOption, OpenAIToolChoiceMode};
use crate::{ByteStream, OpenAIChatChunk, OpenAIChatMessage, OpenAIChatResponse};
use crate::{JsonSchema, OpenAIChatRequest, openai::v1::OpenAITool};
use async_trait::async_trait;
use derive::FormaDocs;
use reqwest::Error as ReqwestError;
use serde::{Deserialize, Serialize};

fn default_groq_endpoint() -> String {
    "https://api.groq.com/openai".to_string()
}

// Left out when serializing, so a default client comes out as just
// `provider` + `model`, which every other provider also accepts.
fn is_default_groq_endpoint(v: &String) -> bool {
    *v == default_groq_endpoint()
}

fn default_groq_model() -> String {
    "llama-3.1-8b-instant".to_string()
}

/// A client for Groq running in OpenaAI-compatible mode
/// (which happens by default)
///
/// It attempts to adhere as strictly as possible (even if
/// some features are not supported by Forma).
///
/// > 🔑 **Authentication**: uses the `GROQ_API_KEY` API Key
///
/// > Note: the documentation below has been copied nearly verbatim
/// > from OpenAI's one, as reference
#[derive(Serialize, Deserialize, Debug, Clone, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct Groq {
    /// The endpooint to utilize. Defaults to `http://127.0.0.1:11434`
    #[serde(
        default = "default_groq_endpoint",
        skip_serializing_if = "is_default_groq_endpoint"
    )]
    pub endpoint: String,

    /// The model to utilize. Defaults to `llama-3.1-8b-instant`
    ///
    /// Some models (https://console.groq.com/docs/models)
    ///   - `llama-3.1-8b-instant`
    ///   - `llama-3.3-70b-versatile`
    ///   - `groq/compound`
    ///   - `groq/compound-mini`
    ///   - `meta-llama/llama-prompt-guard-2-22m`
    ///   - `meta-llama/llama-prompt-guard-2-86m`
    ///   - `openai/gpt-oss-120b`
    ///   - `openai/gpt-oss-20b`
    ///   - `openai/gpt-oss-safeguard-20b`
    ///   - `qwen/qwen3.6-27b`
    ///   - `qwen/qwen3.8-27b`
    #[serde(default = "default_groq_model")]
    pub model: String,

    /// If set to true, the model response data will be streamed
    /// to the client as it is generated using server-sent events.
    /// See the Streaming section below for more information,
    /// along with the streaming responses guide for more information
    /// on how to handle the streaming events.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,

    /// A list of tools the model may select as appropriate to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,

    /// An object specifying the format that the model must output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<JsonSchema>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_keep: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,

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

    /// An alternative to sampling with temperature, called nucleus sampling,
    /// where the model considers the results of the tokens with top_p probability
    /// mass. So 0.1 means only the tokens comprising the top 10% probability
    /// mass are considered.
    ///
    /// We generally recommend altering this or temperature but not both.s
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

impl Groq {
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
            frequency_penalty: self.frequency_penalty,
            presence_penalty: self.presence_penalty,
            response_format: self.response_format.clone(),
            stream: self.stream,
            temperature: self.temperature,
            // tool_choice: self.tool_choice,
            tools,
            top_p: self.top_p,
            ..Default::default()
        }
    }
}

impl Default for Groq {
    fn default() -> Self {
        Self {
            endpoint: default_groq_endpoint(),
            model: default_groq_model(),
            stream: None,
            tools: None,
            format: None,
            num_keep: None,
            seed: None,
            num_predict: None,
            top_k: None,
            top_p: None,
            min_p: None,
            typical_p: None,
            repeat_last_n: None,
            temperature: None,
            repeat_penalty: None,
            presence_penalty: None,
            frequency_penalty: None,
            stop: None,
            response_format: None,
        }
    }
}

#[async_trait]
impl GenaiClientTrait for Groq {
    type Chunk = OpenAIChatChunk;
    type Response = OpenAIChatResponse;
    type Message = OpenAIChatMessage;
    type Tool = OpenAITool;

    const PROVIDER: LLMProvider = LLMProvider::Groq;
    const API_KEY_NAME: Option<&'static str> = Some("GROQ_API_KEY");

    fn model(&self) -> &str {
        &self.model
    }

    fn frequency_penalty(&self) -> Option<f64> {
        self.frequency_penalty
    }
    fn max_tokens(&self) -> Option<usize> {
        None
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

    fn set_tools(&mut self, tools: Option<Vec<Self::Tool>>) {
        self.tools = tools;
    }

    fn set_tool_choice_mode(&mut self, _tool_choice: Option<OpenAIToolChoiceMode>) {
        // self.tool_choice = tool_choice;
        tracing::warn!("Tool choice mode is not supported by Groq")
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
        let request = rest_request(self, &body).expect("LLM client could not build request");
        post::<Self::Response>(request).await
    }
    async fn stream<M: Into<Self::Message> + Send + 'static, O: From<Self::Chunk>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<ByteStream<O>, ReqwestError> {
        let mut body = self.get_request_body(messages, allowed_tools);
        body.stream = Some(true);
        // Ask for a final usage chunk so token counts are available while streaming.
        body.stream_options = Some(OpenAIStreamOption {
            include_usage: Some(true),
            ..Default::default()
        });
        let request = rest_request(self, &body).expect("LLM client could not build request");
        post_stream::<Self::Chunk, O>(request).await
    }
}

#[cfg(test)]
mod testing {
    use crate::{
        GenaiClientTrait, Groq, JsonSchema, OpenAIChatMessage,
        groq::{default_groq_endpoint, default_groq_model},
        openai::v1::{FunctionToolFields, OpenAITool},
    };

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
        let mut client = Groq::default();
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
        let mut client = Groq::default();
        client.set_tools(Some(vec![tool]));

        let body = client.get_request_body(messages, None);

        assert!(body.tools.is_some());
        assert_eq!(body.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_get_request_body_no_tools() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let client = Groq::default();

        let body = client.get_request_body(messages, None);

        assert!(body.tools.is_none());
    }

    #[test]
    fn test_get_request_body_preserves_parameters() {
        let messages: Vec<OpenAIChatMessage> = vec!["test".to_string().into()];
        let temperature = 0.5;
        let top_p = 0.9;
        let mut client = Groq::default();
        client.temperature = Some(temperature);
        client.top_p = Some(top_p);

        let body = client.get_request_body(messages, None);

        assert_eq!(body.temperature, Some(temperature));
        assert_eq!(body.top_p, Some(top_p));
        assert_eq!(body.model, client.model);
    }

    #[test]
    fn default() {
        let client = Groq::default();
        let model = default_groq_model();
        assert_eq!(&client.model, &model);
        assert_eq!(client.model(), &model);
        let endpoint = default_groq_endpoint();
        assert_eq!(&client.endpoint, &endpoint);
    }

    #[test]
    fn test_default_groq_model() {
        let end = default_groq_model();
        assert_eq!(&end, "llama-3.1-8b-instant");
    }

    #[test]
    fn test_endpoints() {
        let end = default_groq_endpoint();
        assert_eq!(&end, "https://api.groq.com/openai");
        let client = Groq::default();
        assert_eq!(
            client.chat_endpoint().expect("no endpoint"),
            format!("{end}/v1/chat/completions")
        );
    }

    #[test]
    fn test_params() {
        let messages: Vec<OpenAIChatMessage> = vec![];
        let temperature = 0.2;
        let frequency_penalty = 1.2;
        let presence_penalty = 32.1;
        let top_p = 2.0;
        let client = Groq {
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
    fn set_tools() {
        let mut client = Groq::default();
        let tool = OpenAITool::Function {
            function: FunctionToolFields {
                name: "some-tool".to_string(),
                description: Some("the tool description".to_string()),
                parameters: Some(JsonSchema::default()),
                strict: None,
            },
        };
        let tool_str = serde_json::to_string(&tool).unwrap();
        client.set_tools(Some(vec![tool]));
        assert!(&client.tools.is_some());
        assert_eq!(client.count_tools(), 1);
        let added_tools = client.tools.expect("tools").clone();
        assert_eq!(added_tools.len(), 1);
        let added_tool_str = serde_json::to_string(&added_tools[0]).unwrap();
        assert_eq!(tool_str, added_tool_str);
    }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {

    use super::GenaiClientTrait;
    use crate::{OpenAIChatChunk, OpenAIChatMessage, openai::v1::OpenAIFinishReason};
    use futures::StreamExt;

    use super::Groq;

    #[tokio::test]
    async fn test_groq_generate() {
        let llm = Groq::default();
        let messages: Vec<OpenAIChatMessage> = vec!["hello!".to_string().into()];
        let _r = llm
            .generate(messages, None)
            .await
            .expect("Expected a response from generate");
    }

    #[tokio::test]
    async fn test_groq_stream() {
        let llm = Groq::default();

        let messages: Vec<OpenAIChatMessage> = vec!["hello!".to_string().into()];
        let mut r = llm
            .stream::<OpenAIChatMessage, OpenAIChatChunk>(messages, None)
            .await
            .expect("Expected a response from stream");
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
}
