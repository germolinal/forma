use std::collections::HashMap;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use derive::FormaDocs;

use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_aiplatform_v1::model::{
    Blob, Candidate, Content, FileData, FunctionCall, FunctionCallingConfig, FunctionDeclaration,
    FunctionResponse, FunctionResponseBlob, FunctionResponseFileData, FunctionResponsePart,
    GenerateContentRequest, GenerateContentResponse, GenerationConfig, ImageConfig, Part, Schema,
    Tool, ToolConfig, Type, candidate::FinishReason, function_calling_config::Mode,
    function_response_part::Data as FunctionResponseData, generate_content_response::UsageMetadata,
    generation_config::ThinkingConfig, part::Data,
};
use google_cloud_gax::options::RequestOptionsBuilder;
use google_cloud_gax::retry_policy::{Aip194Strict, RetryPolicyExt};
use google_cloud_gax::streaming::ResponseStream;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{
    AssistantMessage, GenaiClientTrait, GenaiMessageTrait, GenaiRequestTrait, JsonSchema, LLMChunk,
    LLMFinishReason, LLMMessage, LLMResponse, MessageChoice, Role, UserMessage,
    genai_client_trait::LLMProvider,
    openai::v1::{
        OpenAIAudioPart, OpenAIChatDelta, OpenAICompletionTokenDetails, OpenAIFile, OpenAIFilePart,
        OpenAIFinishReason, OpenAIFunctionCallDelta, OpenAIFunctionToolChoice, OpenAIImagePart,
        OpenAIMessageChoiceDelta, OpenAIMessageContent, OpenAIMessagePart,
        OpenAIPromptTokenDetails, OpenAIResponseFormat, OpenAITextPart, OpenAITool,
        OpenAIToolCallDelta, OpenAIToolChoice, OpenAIToolChoiceMode, OpenAIToolMessage,
        OpenAIUsage,
    },
    schema::SchemaType,
};

/// Google's own safety-filter types, re-exported so that tools and agent
/// definitions can configure the filters without every crate taking a direct
/// dependency on the Vertex SDK.
pub use google_cloud_aiplatform_v1::model::safety_setting::HarmBlockThreshold;
pub use google_cloud_aiplatform_v1::model::{HarmCategory, SafetySetting};

const GOOGLE_CLOUD_PROJECT: &str = "GOOGLE_CLOUD_PROJECT";

/// The client every call to Vertex AI goes through. Building one reads the
/// credentials, sets up TLS and a connection of its own, and its first request
/// fetches an access token: built per call, all of that came with every LLM
/// call. A built client refreshes its token and reconnects on its own.
static PREDICTION_SERVICE: tokio::sync::OnceCell<PredictionService> =
    tokio::sync::OnceCell::const_new();

/// The process's Vertex AI client, built on first use. A failure to build it
/// is not kept: the next call tries again.
async fn prediction_service() -> Result<PredictionService, String> {
    PREDICTION_SERVICE
        .get_or_try_init(|| async {
            PredictionService::builder()
                .build()
                .await
                .map_err(|x| x.to_string())
        })
        .await
        .cloned()
}

/// The four categories Vertex lets a caller configure.
const CONFIGURABLE_HARM_CATEGORIES: [HarmCategory; 4] = [
    HarmCategory::HateSpeech,
    HarmCategory::DangerousContent,
    HarmCategory::SexuallyExplicit,
    HarmCategory::Harassment,
];

/// Every configurable filter turned off.
///
/// This is what a generation tool uses when its definition says nothing:
/// Forma does not second-guess what an agent author wants blocked, so opting
/// *into* Google's filters is an explicit act.
pub fn safety_settings_off() -> Vec<SafetySetting> {
    CONFIGURABLE_HARM_CATEGORIES
        .iter()
        .map(|category| {
            SafetySetting::new()
                .set_category(category.clone())
                .set_threshold(HarmBlockThreshold::Off)
        })
        .collect()
}

fn default_vertexai_model() -> String {
    "gemini-3.8-flash".to_string()
}

fn default_mime_type() -> String {
    "text/plain".to_string()
}
fn default_vertexai_region() -> String {
    "global".to_string()
}
fn default_request_timeout_seconds() -> u64 {
    300
}

// Defaults are left out when serializing, so a default client comes out as
// just `provider` + `model`, which every other provider also accepts.
fn is_default_mime_type(v: &String) -> bool {
    *v == default_mime_type()
}
fn is_default_vertexai_region(v: &String) -> bool {
    *v == default_vertexai_region()
}
fn is_default_request_timeout_seconds(v: &u64) -> bool {
    *v == default_request_timeout_seconds()
}

/// A client for calling Google Cloud's Vertex AI models.
///
/// > 🔑 **Authentication**: We use the [Google Library](https://github.com/googleapis/google-cloud-rust), so whe edhere to the
/// > [recommended Google Cloud Platform authentication method](https://docs.cloud.google.com/sdk/gcloud/reference/auth/application-default/login).
/// > To run locally, you should run `gcloud auth application-default login` to acquire credentials, and set the `GOOGLE_CLOUD_PROJECT`
/// > environment variable. These two requirements will be automatically set in your deployed machines.
///
///
/// > Note: the documentation below has been copied nearly verbatim
/// > from Google Cloud's documentation.
#[derive(Serialize, Deserialize, Debug, Clone, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct VertexAI {
    /// The model to use. Check Google Cloud's availability
    /// of models in different regions
    ///
    /// Common models are (source -> https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/migrate):
    /// - `gemini-3.8-flash`
    /// - `gemini-3.7-flash`
    /// - `gemini-3.6-flash`
    /// - `gemini-3.5-flash`
    /// - `gemini-3.5-flash-lite`
    /// - `gemini-3.1-flash-lite`
    /// - `gemini-2.5-flash`
    /// - `gemini-2.5-flash-lite`
    #[serde(default = "default_vertexai_model")]
    pub model: String,

    /// The location of the endpoint to use (e.g., 'global', 'us-central1')
    #[serde(
        default = "default_vertexai_region",
        skip_serializing_if = "is_default_vertexai_region"
    )]
    pub location: String,

    /// Overall deadline for a `generateContent` request, in seconds. This
    /// bounds the total time spent across retries. The underlying Google SDK
    /// defaults to 60s, which is often too short for slow models or long
    /// generations, so we raise it here.
    #[serde(
        default = "default_request_timeout_seconds",
        skip_serializing_if = "is_default_request_timeout_seconds"
    )]
    pub request_timeout_seconds: u64,

    /// The name of the cached content used as context to serve the
    /// prediction. Note: only used in explicit caching, where users can have
    /// control over caching (e.g. what content to cache) and enjoy guaranteed cost
    /// savings.
    ///
    /// It is assume to be cached in the same project and location as the Vertex
    /// AI client
    // `projects/{project}/locations/{location}/cachedContents/{cachedContent}`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_content: Option<String>,

    /// A `Tool` is a piece of code that enables the system to interact with
    /// external systems to perform an action, or set of actions, outside of
    /// knowledge and scope of the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub tools: Vec<Tool>,

    /* tool config */
    /// Which tools to call, if any. Options are:
    /// - `auto`: Default model behavior, model decides to predict either function calls or natural language response.
    /// - `required`: Model is constrained to always predicting function calls only. If "allowed_function_names" are set, the predicted function calls will be limited to any one of "allowed_function_names", else the predicted function calls will be any one of the provided "function_declarations".
    /// - `none`: Model will not predict any function calls. Model behavior is same as when not passing any function declarations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoiceMode>,

    /* GENERATION CONFIG  */
    /// Controls the randomness of predictions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// If specified, nucleus sampling will be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// If specified, top-k sampling will be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<f32>,

    /// Number of candidates to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<i32>,

    /// The maximum number of output tokens to generate per message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i32>,

    /// Stop sequences.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub stop_sequences: Vec<String>,

    /// If true, export the logprobs results in response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_logprobs: Option<bool>,

    /// Logit probabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<i32>,

    /// Positive penalties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,

    /// Frequency penalties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,

    /// Seed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,

    /// Output response mimetype of the generated candidate text.
    /// Supported mimetype:
    ///
    /// - `text/plain`: (default) Text output.
    /// - `application/json`: JSON response in the candidates.
    ///   The model needs to be prompted to output the appropriate response type,
    ///   otherwise the behavior is undefined.
    ///   This is a preview feature.
    #[serde(
        default = "default_mime_type",
        skip_serializing_if = "is_default_mime_type"
    )]
    pub response_mime_type: String,

    /// The `Schema` object allows the definition of input and output
    /// data types. These types can be objects, but also primitives and arrays.
    /// Represents a select subset of an [OpenAPI 3.0 schema
    /// object](https://spec.openapis.org/oas/v3.0.3#schema).
    /// If set, a compatible response_mime_type must also be set.
    /// Compatible mimetypes:
    /// `application/json`: Schema for JSON response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<JsonSchema>,
    /*
    /// Optional. Output schema of the generated response. This is an alternative
    /// to `response_schema` that accepts [JSON Schema](https://json-schema.org/).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_json_schema: Option<JsonSchema>,
    */

        /* GENERATION CONFIG / THINKING CONFIG */
    // Optional. Config for thinking features.
    // An error will be returned if this field is set for models that don't
    // support thinking.
    // pub thinking_config: Option<crate::model::generation_config::ThinkingConfig>,
    /// Indicates whether to include thoughts in the response.
    /// If true, thoughts are returned only when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,

    /// Optional. Indicates the thinking budget in tokens.
    /// This is only applied when enable_thinking is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,

    /* GENERATION CONFIG / IAMGE CONFIG */
    // Optional. Config for image generation features.
    // pub image_config: Option<crate::model::ImageConfig>,
    ///  The desired aspect ratio for the generated images. The following
    /// aspect ratios are supported:
    ///
    /// "1:1"
    /// "2:3", "3:2"
    /// "3:4", "4:3"
    /// "4:5", "5:4"
    /// "9:16", "16:9"
    /// "21:9"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_aspect_ratio: Option<String>,
}

impl Default for VertexAI {
    fn default() -> Self {
        Self {
            model: default_vertexai_model(),
            response_mime_type: default_mime_type(),
            location: default_vertexai_region(),
            request_timeout_seconds: default_request_timeout_seconds(),
            cached_content: None,
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_k: None,
            top_p: None,
            candidate_count: None,
            max_output_tokens: None,
            stop_sequences: vec![],
            response_logprobs: None,
            logprobs: None,
            presence_penalty: None,
            frequency_penalty: None,
            seed: None,
            // response_json_schema: None,
            response_format: None,
            include_thoughts: None,
            thinking_budget: None,
            image_aspect_ratio: None,
        }
    }
}

/// Gemini matches a tool result to its call by the **function name**
/// ([`FunctionResponse::name`] must equal the [`FunctionCall::name`]), whereas
/// the OpenAI shape references it by an opaque `tool_call_id`. This rewrites
/// each tool message's `tool_call_id` to the name of the function it answers —
/// looked up from the matching assistant tool call — so that the emitted
/// `functionResponse` carries a name Gemini can correlate. The correlation is
/// done by id (not position) because tool results may arrive out of call order
/// when tools run in parallel. Tool messages with no matching call are left
/// untouched.
pub(crate) fn resolve_tool_call_names(mut messages: Vec<LLMMessage>) -> Vec<LLMMessage> {
    let id_to_name: HashMap<String, String> = messages
        .iter()
        .filter_map(|m| match m {
            LLMMessage::Assistant(a) => Some(a),
            _ => None,
        })
        .flat_map(|a| a.tool_calls.iter().map(|tc| (tc.id(), tc.name())))
        .collect();
    for m in messages.iter_mut() {
        if let LLMMessage::Tool(t) = m
            && let Some(name) = id_to_name.get(&t.tool_call_id)
        {
            t.tool_call_id = name.clone();
        }
    }
    messages
}

/// Returns `true` when `content` is a user turn made up entirely of tool
/// results (i.e. every part is a `functionResponse`). These are the `Content`s
/// that must be merged together for Gemini.
fn is_tool_response(content: &Content) -> bool {
    content.role == "user"
        && !content.parts.is_empty()
        && content
            .parts
            .iter()
            .all(|p| matches!(&p.data, Some(Data::FunctionResponse(_))))
}

/// Collapses runs of consecutive tool-result `Content`s into a single user
/// `Content` so that the number of `functionResponse` parts matches the number
/// of `functionCall` parts emitted in the preceding model turn, as Gemini
/// requires.
fn merge_tool_responses(contents: Vec<Content>) -> Vec<Content> {
    let mut merged: Vec<Content> = Vec::with_capacity(contents.len());
    for content in contents {
        if is_tool_response(&content)
            && let Some(last) = merged.last_mut()
            && is_tool_response(last)
        {
            last.parts.extend(content.parts);
            continue;
        }
        merged.push(content);
    }
    merged
}

impl VertexAI {
    fn get_request_body<M: Into<Content>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<GenerateContentRequest, String> {
        let mut ret = GenerateContentRequest::default();
        let messages: Vec<Content> = messages.into_iter().map(|x| x.into()).collect();

        let role = match messages.first() {
            Some(v) => v.role.clone(),
            None => return Err("there are no messages to send to Vertex AI".to_string()),
        };
        if role == "system" || role == "developer" {
            ret.system_instruction = Some(messages[0].clone());
            ret.contents = messages[1..].to_vec();
        } else if role == "user" {
            ret.contents = messages;
        } else {
            return Err(format!(
                "a conversation for Vertex AI cannot start with a '{role}' message"
            ));
        }

        // Gemini requires that every `functionCall` in a model turn be answered
        // by exactly one `functionResponse` in the immediately following user
        // turn (same count). Our pipeline produces one `Content` per tool
        // result, so collapse consecutive tool-result `Content`s into a single
        // user `Content` before sending the request.
        ret.contents = merge_tool_responses(std::mem::take(&mut ret.contents));

        ret = ret.set_model(self.model_path()?);
        ret = ret.set_generation_config(self.generation_config());
        let function_declarations = if let Some(allowed) = allowed_tools {
            let mut ret = Vec::with_capacity(allowed.len());
            for t in self.tools.iter() {
                let mut t = t.clone();
                t.function_declarations = t
                    .function_declarations
                    .iter()
                    .filter(|x| allowed.contains(&x.name))
                    .cloned()
                    .collect();
                if !t.function_declarations.is_empty() {
                    ret.extend(t.function_declarations);
                    // ret.push(t)
                }
            }
            ret
        } else {
            self.tools
                .iter()
                .flat_map(|t| t.function_declarations.clone())
                .collect()
        };
        if !function_declarations.is_empty() {
            let mut tools = Tool::default();
            tools.function_declarations = function_declarations;
            ret = ret.set_tools(vec![tools]);
        }
        if let Some(cached) = &self.cached_content {
            let project_id = self.project_id()?;
            ret = ret.set_cached_content(format!(
                "projects/{}/locations/{}/cachedContents/{}",
                project_id, self.location, cached
            ))
        }

        if let Some(t) = self.tool_config() {
            ret = ret.set_tool_config(t);
        }

        Ok(ret)
    }

    fn project_id(&self) -> Result<String, String> {
        let ret = std::env::var(GOOGLE_CLOUD_PROJECT);
        let err = match ret {
            Ok(v) => {
                if v.is_empty() {
                    format!("empty '{GOOGLE_CLOUD_PROJECT}'")
                } else {
                    return Ok(v);
                }
            }
            Err(std::env::VarError::NotPresent) => {
                format!("could not find environment variable '{GOOGLE_CLOUD_PROJECT}'")
            }
            Err(e) => {
                format!("invalid value for environment variable '{GOOGLE_CLOUD_PROJECT}': {e}")
            }
        };
        Err(err)
    }

    fn thinking_config(&self) -> Option<ThinkingConfig> {
        let mut ret = ThinkingConfig::default();
        if let Some(thoughts) = self.include_thoughts {
            ret = ret.set_include_thoughts(thoughts);
        }
        if let Some(thinking_budget) = self.thinking_budget {
            ret = ret.set_thinking_budget(thinking_budget);
        }
        if ret.thinking_budget.is_none() && ret.include_thoughts.is_none() {
            None
        } else {
            Some(ret)
        }
    }
    fn tool_config(&self) -> Option<ToolConfig> {
        if let Some(c) = &self.tool_choice {
            let mut ret = ToolConfig::default();
            let f = FunctionCallingConfig::new().set_mode(c);
            ret.function_calling_config = Some(f);
            Some(ret)
        } else {
            None
        }
    }

    fn image_config(&self) -> Option<ImageConfig> {
        if let Some(aspect) = &self.image_aspect_ratio {
            let ret = ImageConfig::default().set_aspect_ratio(aspect);
            Some(ret)
        } else {
            None
        }
    }

    fn generation_config(&self) -> GenerationConfig {
        let mut ret = GenerationConfig::default();
        if let Some(t) = self.temperature {
            ret = ret.set_temperature(t);
        }
        if let Some(t) = self.top_p {
            ret = ret.set_top_p(t);
        }
        if let Some(t) = self.top_k {
            ret = ret.set_top_k(t)
        }
        if let Some(t) = self.candidate_count {
            ret = ret.set_candidate_count(t)
        }
        if let Some(t) = self.max_output_tokens {
            ret = ret.set_max_output_tokens(t);
        }
        ret = ret.set_stop_sequences(self.stop_sequences.clone());
        if let Some(t) = self.response_logprobs {
            ret = ret.set_response_logprobs(t)
        }
        if let Some(t) = self.presence_penalty {
            ret = ret.set_presence_penalty(t);
        }
        if let Some(t) = self.frequency_penalty {
            ret = ret.set_frequency_penalty(t);
        }
        if let Some(t) = self.seed {
            ret = ret.set_seed(t)
        }
        ret = ret.set_response_mime_type(self.response_mime_type.clone());
        if let Some(t) = &self.response_format {
            let s: Schema = t.clone().into();
            ret = ret.set_response_schema(s);
        }
        // if let Some(t) = &self.response_json_schema {
        //     let s: Schema = t.clone().into();
        //     ret = ret.set_response_json_schema(s);
        // }
        // ret = ret.set_routing_config(None) //. not supported
        let t = self.thinking_config();
        if let Some(t) = t {
            ret = ret.set_thinking_config(t.clone());
        }
        let t = self.image_config();
        if let Some(t) = t {
            ret = ret.set_image_config(t)
        }
        ret
    }

    async fn chat_stream<M: Into<Content>, O: Send + 'static + From<LLMChunk>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<crate::ByteStream<O>, reqwest::Error> {
        let responses = match self.open_stream(messages, allowed_tools).await {
            Ok(responses) => responses,
            Err(e) => {
                // We cannot construct a `reqwest::Error` here to return, so log
                // the failure and degrade to an empty stream rather than panic.
                tracing::error!("VertexAI streaming failed: {e}");
                return Ok(Box::pin(futures::stream::empty()));
            }
        };

        let chunks = futures::stream::unfold(
            (responses, VertexStream::default()),
            |(mut responses, mut state)| async move {
                let response = match responses.next().await? {
                    Ok(response) => response,
                    Err(e) => {
                        tracing::error!("VertexAI stream interrupted: {e}");
                        return None;
                    }
                };
                match state.chunk(response) {
                    Ok(chunk) => Some((O::from(chunk), (responses, state))),
                    Err(e) => {
                        tracing::error!("{e}");
                        None
                    }
                }
            },
        );
        Ok(Box::pin(chunks))
    }

    /// Starts a server-streaming generation. Retries only cover opening the
    /// stream; the timeout is a gRPC deadline, so it bounds the whole stream
    /// just as it bounds a unary [`Self::chat`].
    async fn open_stream<M: Into<Content>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<ResponseStream<GenerateContentResponse>, String> {
        let body = self.get_request_body(messages, allowed_tools)?;
        let client = prediction_service().await?;

        let timeout = std::time::Duration::from_secs(self.request_timeout_seconds);
        client
            .stream_generate_content()
            .with_request(body)
            .with_attempt_timeout(timeout)
            .with_retry_policy(Aip194Strict.with_attempt_limit(3).with_time_limit(timeout))
            .send()
            .await
            .map_err(|x| x.to_string())
    }

    fn model_path(&self) -> Result<String, String> {
        let model = &self.model;
        let project_id = self.project_id()?;
        let location = &self.location;
        Ok(format!(
            "projects/{project_id}/locations/{location}/publishers/google/models/{model}"
        ))
    }

    async fn chat<M: Into<Content>>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<GenerateContentResponse, String> {
        let body = self.get_request_body(messages, allowed_tools)?;
        let client = prediction_service().await?;

        // Override the SDK's default retry policy (60s overall time limit),
        // which is often too short for slow models / long generations and
        // surfaces as "the request exceeded the request deadline".
        let timeout = std::time::Duration::from_secs(self.request_timeout_seconds);
        let request = client
            .generate_content()
            .with_request(body)
            .with_attempt_timeout(timeout)
            .with_retry_policy(Aip194Strict.with_attempt_limit(3).with_time_limit(timeout));

        let response = request.send().await.map_err(|x| x.to_string())?;
        match response.candidates.first() {
            Some(candidate) => match candidate.finish_reason {
                // Reported as a finish reason, so the caller can ask again.
                FinishReason::MalformedFunctionCall => {
                    warn_malformed_call(&self.model, candidate.finish_message.as_deref());
                    Ok(response)
                }
                #[cfg(debug_assertions)]
                FinishReason::Unspecified | FinishReason::Other | FinishReason::UnknownValue(_) => {
                    tracing::error!("{}", serde_json::to_string_pretty(&response).unwrap());
                    Ok(response)
                }
                _ => Ok(response),
            },
            None => Err("No candidates found".to_string()),
        }
    }
}

/// The host Vertex AI serves `location` from: `global` has the unprefixed
/// one, a region its own (`us-central1-aiplatform.googleapis.com`).
pub fn vertex_ai_host(location: &str) -> String {
    if location == "global" {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{location}-aiplatform.googleapis.com")
    }
}

#[async_trait]
impl GenaiClientTrait for VertexAI {
    type Chunk = LLMChunk;
    type Message = Content;
    type Response = GenerateContentResponse;
    type Tool = FunctionDeclaration;

    const PROVIDER: LLMProvider = LLMProvider::VertexAI;

    async fn generate<M: Into<Self::Message> + Send + 'static>(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<Self::Response, String> {
        self.chat(messages, allowed_tools).await
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn frequency_penalty(&self) -> Option<f64> {
        self.frequency_penalty.map(|v| v as f64)
    }
    fn max_tokens(&self) -> Option<usize> {
        self.max_output_tokens.map(|v| v as usize)
    }
    fn presence_penalty(&self) -> Option<f64> {
        self.presence_penalty.map(|v| v as f64)
    }
    fn temperature(&self) -> Option<f64> {
        self.temperature.map(|v| v as f64)
    }
    fn top_p(&self) -> Option<f64> {
        self.top_p.map(|v| v as f64)
    }

    fn set_tools(&mut self, tools: Option<Vec<Self::Tool>>) {
        match tools {
            None => self.tools.clear(),
            Some(t) => {
                if t.is_empty() {
                    self.tools.clear();
                    return;
                }
                let function_declarations = t.into_iter().collect();
                let mut tools = Tool::default();
                tools.function_declarations = function_declarations;
                self.tools = vec![tools];
            }
        }
    }

    fn set_tool_choice_mode(&mut self, tool_choice: Option<OpenAIToolChoiceMode>) {
        match tool_choice {
            None => self.tool_choice = None,
            Some(mode) => self.tool_choice = Some(mode),
        }
    }

    fn set_output_schema(&mut self, schema: OpenAIResponseFormat) {
        self.response_format = Some(schema.json_schema.schema);
        self.response_mime_type = "application/json".to_string();
    }

    fn has_output_schema(&self) -> bool {
        self.response_format.is_some()
    }

    fn count_tools(&self) -> usize {
        self.tools.len()
    }

    /// From the location alone: the endpoint also needs the project, which
    /// the host does not depend on.
    fn server_address(&self) -> String {
        vertex_ai_host(&self.location)
    }

    fn chat_endpoint(&self) -> Result<String, String> {
        let api_endpoint = if &self.location == "global" {
            "https://aiplatform.googleapis.com".to_string()
        } else {
            format!("https://{}-aiplatform.googleapis.com", self.location)
        };
        let endpoint = format!("{}:generateContent", self.model_path()?);

        Ok(format!("{api_endpoint}/v1/{endpoint}"))
    }
    async fn stream<
        M: Into<Self::Message> + Send + 'static,
        O: 'static + Send + From<Self::Chunk>,
    >(
        &self,
        messages: Vec<M>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<crate::ByteStream<O>, String> {
        self.chat_stream::<M, O>(messages, allowed_tools)
            .await
            .map_err(|e| e.to_string())
    }
}

impl From<SchemaType> for Type {
    fn from(value: SchemaType) -> Self {
        match value {
            SchemaType::Object => Type::Object,
            SchemaType::Array => Type::Array,
            SchemaType::String => Type::String,
            SchemaType::Number => Type::Number,
            SchemaType::Integer => Type::Integer,
            SchemaType::Boolean => Type::Boolean,
            SchemaType::Null => Type::Unspecified,
            SchemaType::Schema => Type::Unspecified,
        }
    }
}

impl From<JsonSchema> for Schema {
    fn from(value: JsonSchema) -> Self {
        let mut schema = Schema::default();
        if let Some(t) = value.schema_type {
            schema.r#type = t.into();
        }
        if let Some(n) = &value.name {
            schema.title = n.to_string();
        }
        if let Some(n) = &value.properties {
            let props: HashMap<String, Schema> = n
                .iter()
                .map(|(key, value)| (key.clone(), value.clone().into()))
                .collect();
            schema.properties = props;
        }
        if let Some(i) = &value.items {
            let i: Schema = (*i.clone()).into();
            schema.items = Some(Box::new(i));
        }
        if let Some(r) = &value.required {
            schema.required = r.clone();
        }
        if let Some(v) = value.r#enum {
            schema.r#enum = v.clone();
        }
        if let Some(d) = &value.description {
            schema.description = d.clone();
        }
        schema
    }
}

impl From<OpenAIToolChoiceMode> for Mode {
    fn from(value: OpenAIToolChoiceMode) -> Self {
        (&value).into()
    }
}

impl From<&OpenAIToolChoiceMode> for Mode {
    fn from(value: &OpenAIToolChoiceMode) -> Self {
        match value {
            OpenAIToolChoiceMode::None => Mode::None,
            OpenAIToolChoiceMode::Auto => Mode::Auto,
            OpenAIToolChoiceMode::Required => Mode::Any,
        }
    }
}

impl From<OpenAITool> for FunctionDeclaration {
    fn from(value: OpenAITool) -> Self {
        let mut func = FunctionDeclaration::default();
        func.name = value.name().clone();
        func.description = value.description();
        if let Some(p) = value.parameters() {
            let s: Schema = p.clone().into();
            func.parameters = Some(s);
        }
        func
    }
}

impl From<FinishReason> for LLMFinishReason {
    fn from(value: FinishReason) -> Self {
        match value {
            // Handled normally
            FinishReason::Stop => LLMFinishReason::Stop,
            FinishReason::MaxTokens => LLMFinishReason::Length,
            // Big bucket of safety
            FinishReason::Safety
            | FinishReason::Recitation
            | FinishReason::Blocklist
            | FinishReason::ProhibitedContent
            | FinishReason::Spii
            | FinishReason::ModelArmor => LLMFinishReason::ContentFilter,
            // Unsure what to do
            FinishReason::Unspecified => {
                warn!("{:?}", value);
                LLMFinishReason::Other("VertexAI - Unspecified".to_string())
            }
            FinishReason::MalformedFunctionCall => LLMFinishReason::MalformedToolCall,
            FinishReason::Other => LLMFinishReason::Other("VertexAI - Other".to_string()),
            FinishReason::UnknownValue(_) => {
                warn!("VertexAI - UnknownValue: {:?}", value);
                LLMFinishReason::Other("VertexAI - UnknownValue".to_string())
            }
            // The enum is non-exhaustive: reasons Google adds later land here.
            _ => {
                warn!("VertexAI - unsupported finish reason: {:?}", value);
                LLMFinishReason::Other(format!("VertexAI - {value:?}"))
            }
        }
    }
}

fn function_call_to_tool_choice(data: Data) -> OpenAIToolChoice {
    if let Data::FunctionCall(b) = data {
        let arguments = match &b.args {
            Some(v) => serde_json::to_string(v)
                .expect("could not serialize arguments from VertexAI Function Call"),
            None => "{}".to_string(),
        };

        OpenAIToolChoice::Function {
            id: uuid::Uuid::new_v4().to_string(),
            function: OpenAIFunctionToolChoice {
                name: b.name,
                arguments,
            },
        }
    } else {
        panic!("function `translate_function_to_tool` only allows for Data::FunctionCall inpus")
    }
}

impl From<Box<Blob>> for OpenAIFilePart {
    fn from(value: Box<Blob>) -> Self {
        // The blob holds raw bytes, so it is re-encoded into the data URL
        // `file_data` is defined to carry. Reading it as UTF-8 instead used to
        // panic on the first PNG a model returned.
        let ext = value.mime_type.split('/').next_back().unwrap_or("bin");
        let data_url = format!(
            "data:{};base64,{}",
            value.mime_type,
            B64.encode(&value.data)
        );
        OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(data_url),
                file_id: None,
                filename: Some(format!("file.{ext}")),
            },
        }
    }
}

impl From<Data> for OpenAIMessagePart {
    fn from(data: Data) -> Self {
        match data {
            Data::Text(d) => {
                let p = OpenAITextPart { text: d };
                OpenAIMessagePart::Text(p)
            }
            Data::InlineData(f) => OpenAIMessagePart::File(f.into()),
            // What the model answers with is the provider's to decide, so a
            // kind of part Forma does not use is kept as words, not a crash.
            Data::FileData(f) if f.mime_type.starts_with("image/") => {
                OpenAIMessagePart::ImageUrl(OpenAIImagePart {
                    image_url: crate::openai::v1::OpenAIImageUrl {
                        url: f.file_uri,
                        detail: None,
                    },
                })
            }
            Data::FileData(f) => OpenAIMessagePart::Text(OpenAITextPart {
                text: format!("[a file ({}): {}]", f.mime_type, f.file_uri),
            }),
            Data::FunctionCall(_b) => {
                panic!(
                    "VertexAI Function Call cannot be turned into OpenAI Part. use `translate_function_to_tool()`"
                )
            }
            Data::FunctionResponse(r) => {
                let text = r
                    .response
                    .as_ref()
                    .and_then(|m| m.get("output"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                OpenAIMessagePart::Text(OpenAITextPart { text })
            }
            Data::ExecutableCode(c) => OpenAIMessagePart::Text(OpenAITextPart {
                text: format!("```\n{}\n```", c.code),
            }),
            Data::CodeExecutionResult(r) => OpenAIMessagePart::Text(OpenAITextPart {
                text: format!("```\n{}\n```", r.output),
            }),
            other => {
                warn!("ignoring a kind of part Vertex AI answered with: {other:?}");
                OpenAIMessagePart::Text(OpenAITextPart::default())
            }
        }
    }
}

fn get_parts_tool_calls(parts: &[Part]) -> (Vec<OpenAIMessagePart>, Vec<OpenAIToolChoice>) {
    let mut ret = Vec::with_capacity(parts.len());
    let mut tool_calls = Vec::with_capacity(parts.len());
    for p in parts.iter().cloned() {
        if let Some(data) = p.data {
            if let Data::FunctionCall(_) = data {
                tool_calls.push(function_call_to_tool_choice(data));
            } else {
                let new_p = data.into();
                ret.push(new_p)
            }
        }
    }
    (ret, tool_calls)
}

impl From<Content> for AssistantMessage {
    fn from(content: Content) -> Self {
        let (parts, tool_calls) = get_parts_tool_calls(&content.parts);
        // Find the first part with a non-empty thought signature
        let thought_signature = match content
            .parts
            .iter()
            .find(|x| !x.thought_signature.is_empty())
        {
            Some(part) => {
                let thought = part.thought_signature.clone();
                Some(thought)
            }
            None => None,
        };

        AssistantMessage {
            content: parts,
            name: None,
            refusal: None,
            tool_calls,
            audio: None,
            thought_signature,
        }
    }
}

impl From<Content> for UserMessage {
    fn from(content: Content) -> Self {
        let (parts, _tool_calls) = get_parts_tool_calls(&content.parts);

        UserMessage {
            content: parts,
            name: None,
        }
    }
}

impl From<Candidate> for MessageChoice {
    fn from(value: Candidate) -> Self {
        // A candidate without content is how Gemini answers when it blocks
        // one (`finish_reason` says why): an empty answer, not a crash.
        let message = match value.content {
            Some(content) => LLMMessage::Assistant(content.into()),
            None => {
                warn!(finish_reason = ?value.finish_reason, "Vertex AI answered with no content");
                LLMMessage::Assistant(AssistantMessage::default())
            }
        };

        Self {
            finish_reason: value.finish_reason.into(),
            index: value.index as usize,
            logprobs: None,
            message,
        }
    }
}

impl From<&UsageMetadata> for OpenAIUsage {
    fn from(value: &UsageMetadata) -> Self {
        let completion_tokens_details = OpenAICompletionTokenDetails {
            reasoning_tokens: value.thoughts_token_count as usize,
            ..Default::default()
        };

        let prompt_tokens_details = OpenAIPromptTokenDetails {
            cached_tokens: value.cached_content_token_count as usize,
            ..Default::default()
        };
        // Gemini reports thinking tokens *outside* `candidates_token_count`,
        // where OpenAI counts them inside `completion_tokens`. Normalising to
        // OpenAI's meaning here keeps `OpenAIUsage` honest about its own shape:
        // otherwise every consumer — spans, bills — silently undercounts the
        // output of a thinking model by however much it thought.
        let completion_tokens =
            (value.candidates_token_count + value.thoughts_token_count) as usize;

        Self {
            prompt_tokens: value.prompt_token_count as usize,
            completion_tokens,
            total_tokens: value.total_token_count as usize,
            completion_tokens_details: Some(completion_tokens_details),
            prompt_tokens_details: Some(prompt_tokens_details),
        }
    }
}

impl From<GenerateContentResponse> for LLMResponse {
    fn from(value: GenerateContentResponse) -> Self {
        let choices = value.candidates.into_iter().map(|x| x.into()).collect();

        let usage = if let Some(usage_info) = &value.usage_metadata {
            usage_info.into()
        } else {
            warn!("Vertex AI usage metadata not available");
            OpenAIUsage::default()
        };

        Self {
            choices,
            // created
            // id
            model: value.model_version,
            // object
            // service_tier
            usage,
            ..Default::default()
        }
    }
}

/// Maps our common [`LLMFinishReason`] to the OpenAI streaming finish reason
/// used in chunks. `Other` is treated as a normal stop.
/// Logs a `MALFORMED_FUNCTION_CALL`, with Gemini's own account of what was
/// wrong when it gives one, so we can tell which models do this and why.
fn warn_malformed_call(model: &str, finish_message: Option<&str>) {
    warn!(
        model,
        finish_message = finish_message.unwrap_or_default(),
        "Vertex AI returned a malformed function call"
    );
}

fn openai_finish_reason(reason: LLMFinishReason) -> OpenAIFinishReason {
    match reason {
        LLMFinishReason::Stop => OpenAIFinishReason::Stop,
        LLMFinishReason::Length => OpenAIFinishReason::Length,
        LLMFinishReason::ContentFilter => OpenAIFinishReason::ContentFilter,
        LLMFinishReason::ToolCalls => OpenAIFinishReason::ToolCalls,
        LLMFinishReason::FunctionCalls => OpenAIFinishReason::FunctionCalls,
        LLMFinishReason::MalformedToolCall => OpenAIFinishReason::MalformedToolCall,
        LLMFinishReason::Other(_) => OpenAIFinishReason::Stop,
    }
}

/// Turns the partial responses of a Vertex AI stream into OpenAI-style
/// chunks, carrying across responses what a single one cannot know.
///
/// It assumes one candidate per request, which is all we ask for.
#[derive(Debug, Default)]
struct VertexStream {
    /// Tool calls emitted so far. Each partial response holds whole function
    /// calls, so every call gets the next index; restarting from zero per
    /// response would merge distinct calls in the consumer's accumulator.
    tool_calls: usize,
    /// Whether the thought signature has been emitted. Gemini puts it on the
    /// first function-call part (or the last part, without calls), and the
    /// consumer keeps the last one it sees, so only the first is forwarded.
    thought_signature_sent: bool,
}

impl VertexStream {
    fn chunk(&mut self, value: GenerateContentResponse) -> Result<LLMChunk, String> {
        let usage = value.usage_metadata.as_ref().map(OpenAIUsage::from);

        let mut choices = Vec::with_capacity(value.candidates.len());
        for candidate in value.candidates {
            let finish_reason = match candidate.finish_reason {
                // Every partial response but the last one.
                FinishReason::Unspecified => None,
                reason => {
                    if reason == FinishReason::MalformedFunctionCall {
                        warn_malformed_call(
                            &value.model_version,
                            candidate.finish_message.as_deref(),
                        );
                    }
                    Some(openai_finish_reason(reason.into()))
                }
            };

            let parts = candidate.content.map(|c| c.parts).unwrap_or_default();
            let thought_signature = if self.thought_signature_sent {
                None
            } else {
                parts
                    .iter()
                    .find(|x| !x.thought_signature.is_empty())
                    .map(|x| x.thought_signature.clone())
            };
            self.thought_signature_sent |= thought_signature.is_some();

            let (content, tool_calls) = get_parts_tool_calls(&parts);
            let tool_calls: Vec<OpenAIToolCallDelta> = tool_calls
                .into_iter()
                .map(|tc| {
                    let index = self.tool_calls;
                    self.tool_calls += 1;
                    OpenAIToolCallDelta {
                        index,
                        id: Some(tc.id()),
                        call_type: Some("function".to_string()),
                        function: Some(OpenAIFunctionCallDelta {
                            name: Some(tc.name()),
                            arguments: Some(tc.arguments()),
                        }),
                    }
                })
                .collect();

            choices.push(OpenAIMessageChoiceDelta {
                finish_reason,
                index: candidate.index as usize,
                logprobs: None,
                delta: OpenAIChatDelta {
                    content: (!content.is_empty()).then_some(OpenAIMessageContent::Parts(content)),
                    tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                    thought_signature,
                    role: Some(Role::Assistant),
                    ..Default::default()
                },
            });
        }

        Ok(LLMChunk {
            choices,
            model: value.model_version,
            object: "chat.completion.chunk".to_string(),
            usage,
            ..Default::default()
        })
    }
}

impl GenaiMessageTrait for Content {
    /// Gets the message role in our common
    fn role(&self) -> Role {
        match self.role.as_str() {
            "user" => Role::User,
            "model" => Role::Assistant,
            "system" => Role::System,
            // Vertex AI names only `user` and `model`.
            other => {
                warn!("unexpected role '{other}' in a Vertex AI message; read as the user's");
                Role::User
            }
        }
    }

    /// Checks if the message has any content in it
    fn has_content(&self) -> bool {
        !self.parts.is_empty()
    }

    /// Retrieves the String content from a message, if any
    fn content_string(&self) -> Option<String> {
        if self.has_content() {
            let mut ret = Vec::with_capacity(self.parts.len());
            for p in self.parts.iter() {
                if p.thought {
                    continue;
                }
                if let Some(data) = &p.data {
                    match data {
                        Data::Text(t) => ret.push(t.clone()),
                        Data::FileData(_) | Data::InlineData(_) => ret.push("[file]".to_string()),
                        _ => {}
                    }
                }
            }

            Some(ret.join("\n"))
        } else {
            None
        }
    }
}

/// From https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/MIME_types/Common_types
fn mimetype(s: &str) -> &'static str {
    const DEFAULT_TYPE: &str = "text/plain";
    let extension = match s.split('.').next_back() {
        Some(v) => v,
        None => return DEFAULT_TYPE,
    };

    match extension {
        "aac" => "audio/aac",
        "abw" => "applicaton/x-abiword",
        "apng" => "image/apng",
        "arc" => "application/x-freearc",
        "avif" => "image/avif",
        "avi" => "video/x-msvideo",
        "azw" => "application/vnd.amazon.ebook",
        "bin" => "application/octet-stream",
        "bmp" => "image/bmp",
        "bz" => "application/x-bzip",
        "bz2" => "application/x-bzip2",
        "cda" => "application/x-cdf",
        "csh" => "application/x-csh",
        "css" => "text/css",
        "csv" => "text/csv",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "eot" => "application/vnd.ms-fontobject",
        "epub" => "application/epub+zip",
        "gz" => "application/gzip",
        "gif" => "image/gif",
        "htm" | "html" => "text/html",
        "ico" => "image/vnd.microsoft.icon",
        "ics" => "text/calendar",
        "jar" => "application/java-archive",
        "jpeg" | "jpg" => "image/jpeg",
        "js" => "text/javascript",
        "json" => "application/json",
        "jsonld" => "application/ld+json",
        "md" => "text/markdown",
        "mid" | "midi" => "audio/midi",
        "mjs" => "text/javascript",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mpeg" => "video/mpeg",
        "mpkg" => "application/vnd.apple.installer+xml",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odt" => "application/vnd.oasis.opendocument.text",
        "oga" => "audio/ogg",
        "ogv" => "video/ogg",
        "ogx" => "application/ogg",
        "opus" => "audio/ogg",
        "otf" => "font/otf",
        "png" => "image/png",
        "pdf" => "application/pdf",
        "php" => "application/x-httpd-php",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rar" => "application/vnd.rar",
        "rtf" => "application/rtf",
        "sh" => "application/x-sh",
        "svg" => "image/svg+xml",
        "tar" => "application/x-tar",
        "tif" | "tiff" => "image/tiff",
        "ts" => "video/mp2t",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        "vsd" => "application/vnd.visio",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "webm" => "video/webm",
        "webmanifest" => "application/manifest+json",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "xhtml" => "application/xhtml+xml",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xml" => "application/xml",
        "xul" => "application/vnd.mozilla.xul+xml",
        "zip" => "application/zip",
        "3gp" => "video/3gpp",
        "3g2" => "video/3gpp2",
        "7z" => "application/x-7z-compressed",
        _ => DEFAULT_TYPE,
    }
}

/// Splits an [RFC 2397](https://datatracker.ietf.org/doc/html/rfc2397) base64
/// data URL into its media type and its still-encoded payload.
///
/// Returns `None` for anything that is not a base64 data URL — a `gs://` or
/// `https://` URI, or a bare base64 blob that carries no media type of its
/// own — leaving the caller to work the type out from a filename instead.
fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let (meta, payload) = url.strip_prefix("data:")?.split_once(',')?;
    // Percent-encoded data URLs would need a different decoder, so they are
    // reported as "not a base64 data URL" rather than silently mishandled.
    if !meta.ends_with(";base64") {
        return None;
    }
    // Drop any parameters (`;charset=utf-8`); Vertex wants a bare IANA type.
    let media_type = meta.split(';').next().filter(|m| !m.is_empty())?;
    Some((media_type, payload))
}

/// Decodes a base64 document payload into the raw bytes Vertex expects.
///
/// [`Blob::data`] is base64-encoded by the Vertex SDK on the way out, so
/// handing it an already-encoded string encodes it twice and gives Gemini
/// garbage rather than a document.
fn decode_base64(payload: &str) -> Result<bytes::Bytes, String> {
    B64.decode(payload.trim())
        .map(bytes::Bytes::from)
        .map_err(|e| format!("could not decode base64 document payload: {e}"))
}

/// Builds the inline blob for an OpenAI `file` part.
///
/// `file_data` is a data URL in OpenAI's own API
/// (`data:application/pdf;base64,…`), but a bare base64 string is accepted
/// too, in which case the media type is guessed from `filename`.
fn file_part_to_blob(part: &OpenAIFilePart) -> Result<Blob, String> {
    let Some(data) = &part.file.file_data else {
        return Err(match &part.file.file_id {
            Some(id) => format!(
                "file part refers to uploaded file '{id}', which Vertex AI cannot resolve; send the file inline instead"
            ),
            None => "file part carries neither `file_data` nor `file_id`".to_string(),
        });
    };

    let (mime_type, payload) = match split_data_url(data) {
        Some((mime_type, payload)) => (mime_type, payload),
        None => (
            mimetype(part.file.filename.as_deref().unwrap_or_default()),
            data.as_str(),
        ),
    };

    Ok(Blob::new()
        .set_mime_type(mime_type)
        .set_data(decode_base64(payload)?))
}

/// Builds the part payload for an OpenAI `image_url` part.
///
/// Browsers upload images as `data:` URLs, which Vertex takes as an inline
/// blob. A `gs://` or `https://` URI is passed through as a file reference
/// instead, since that is the only form `FileData.file_uri` accepts.
fn image_part_to_data(part: &OpenAIImagePart) -> Result<Data, String> {
    let url = &part.image_url.url;

    if let Some((mime_type, payload)) = split_data_url(url) {
        return Ok(Data::InlineData(Box::new(
            Blob::new()
                .set_mime_type(mime_type)
                .set_data(decode_base64(payload)?),
        )));
    }

    if url.starts_with("data:") {
        // Deliberately not quoting the URL: it is the whole document.
        return Err(
            "image data URL is neither base64-encoded nor typed; only `data:<type>;base64,…` is supported"
                .to_string(),
        );
    }

    Ok(Data::FileData(Box::new(
        FileData::new()
            .set_mime_type(mimetype(url))
            .set_file_uri(url),
    )))
}

/// Builds the inline blob for an OpenAI `input_audio` part, whose payload is
/// always bare base64 with the media type given by its `format`.
fn audio_part_to_blob(part: &OpenAIAudioPart) -> Result<Blob, String> {
    Ok(Blob::new()
        .set_mime_type(part.input_audio.format.mime_type())
        .set_data(decode_base64(&part.input_audio.data)?))
}

impl From<&OpenAIMessagePart> for Part {
    fn from(value: &OpenAIMessagePart) -> Self {
        let data = match value {
            OpenAIMessagePart::Text(t) => Ok(Data::Text(t.text.to_string())),
            OpenAIMessagePart::ImageUrl(p) => image_part_to_data(p),
            OpenAIMessagePart::InputAudio(p) => {
                audio_part_to_blob(p).map(|b| Data::InlineData(Box::new(b)))
            }
            OpenAIMessagePart::File(p) => {
                file_part_to_blob(p).map(|b| Data::InlineData(Box::new(b)))
            }
        };

        // One malformed attachment must not take the conversation down, and
        // `From` has no error channel. Say so in the prompt instead, so the
        // model reports a missing document rather than answering as though it
        // had read one.
        let data = data.unwrap_or_else(|e| {
            warn!("dropping unusable document part: {e}");
            Data::Text(format!("[unusable attachment: {e}]"))
        });

        let mut ret = Part::default();
        ret.data = Some(data);
        ret
    }
}

impl From<&OpenAIMessageContent> for Vec<Part> {
    fn from(value: &OpenAIMessageContent) -> Self {
        match value {
            OpenAIMessageContent::String(s) => {
                // Single part.
                let mut p = Part::default();
                p.data = Some(Data::Text(s.to_string()));
                vec![p]
            }
            OpenAIMessageContent::Parts(p) => p.iter().clone().map(|x| x.into()).collect(),
        }
    }
}

impl From<&AssistantMessage> for Vec<Part> {
    fn from(m: &AssistantMessage) -> Self {
        let mut parts: Vec<Part> = m.content.iter().map(|x| x.into()).collect();

        for tc in &m.tool_calls {
            let mut fc = FunctionCall::default();
            fc.name = tc.name();
            let args: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&tc.arguments()).unwrap_or_default();
            fc.args = Some(args);
            let mut part = Part::default();
            part.data = Some(Data::FunctionCall(Box::new(fc)));
            parts.push(part);
        }
        if let Some(thought_signature) = &m.thought_signature {
            parts.first_mut().unwrap().thought_signature = thought_signature.clone();
        }
        parts
    }
}

impl From<&OpenAIToolMessage> for Vec<Part> {
    fn from(m: &OpenAIToolMessage) -> Self {
        // A tool that fetched a document returns it as a non-text part, which
        // has no place in the JSON `response` struct. Vertex carries those in
        // `FunctionResponse.parts`, so the two kinds are split here rather
        // than stringifying the attachment into the text output.
        let mut text = Vec::new();
        let mut parts = Vec::new();
        for part in m.content.as_parts() {
            match &part {
                OpenAIMessagePart::Text(t) => text.push(t.text.clone()),
                _ => match function_response_part(&part) {
                    Ok(p) => parts.push(p),
                    Err(e) => {
                        warn!("dropping unusable document in tool result: {e}");
                        text.push(format!("[unusable attachment: {e}]"));
                    }
                },
            }
        }

        let mut struct_map = serde_json::Map::new();
        struct_map.insert(
            "output".to_string(),
            serde_json::Value::String(text.join("\n")),
        );

        let mut response = FunctionResponse::default();
        response.name = m.tool_call_id.clone();
        response.response = Some(struct_map);
        response.parts = parts;

        let mut part = Part::default();
        part.data = Some(Data::FunctionResponse(Box::new(response)));
        vec![part]
    }
}

/// Converts a non-text content part into the shape a function response uses,
/// which is a parallel set of types to the ordinary [`Part`] ones.
fn function_response_part(part: &OpenAIMessagePart) -> Result<FunctionResponsePart, String> {
    let data = match part {
        OpenAIMessagePart::Text(_) => {
            unreachable!("text parts belong in the function response `output`")
        }
        OpenAIMessagePart::File(p) => {
            let blob = file_part_to_blob(p)?;
            FunctionResponseData::InlineData(Box::new(
                FunctionResponseBlob::new()
                    .set_mime_type(blob.mime_type)
                    .set_data(blob.data),
            ))
        }
        OpenAIMessagePart::InputAudio(p) => {
            let blob = audio_part_to_blob(p)?;
            FunctionResponseData::InlineData(Box::new(
                FunctionResponseBlob::new()
                    .set_mime_type(blob.mime_type)
                    .set_data(blob.data),
            ))
        }
        OpenAIMessagePart::ImageUrl(p) => match image_part_to_data(p)? {
            Data::InlineData(b) => FunctionResponseData::InlineData(Box::new(
                FunctionResponseBlob::new()
                    .set_mime_type(b.mime_type)
                    .set_data(b.data),
            )),
            Data::FileData(f) => FunctionResponseData::FileData(Box::new(
                FunctionResponseFileData::new()
                    .set_mime_type(f.mime_type)
                    .set_file_uri(f.file_uri),
            )),
            _ => unreachable!("`image_part_to_data` only returns inline or file data"),
        },
    };

    let mut ret = FunctionResponsePart::default();
    ret.data = Some(data);
    Ok(ret)
}

impl From<LLMMessage> for Content {
    fn from(value: LLMMessage) -> Self {
        let mut ret = Content::default();

        let role = match value.role() {
            Role::System | Role::Developer => {
                // This is not really supported by Vertex AI.
                // We need to handle this properly later.
                "system"
            }
            Role::User => "user",
            Role::Assistant => "model",
            Role::Tool => "user",
        };
        ret = ret.set_role(role);

        let parts = match &value {
            LLMMessage::Assistant(m) => m.into(),
            LLMMessage::User(m) => m.content.iter().map(|x| x.into()).collect(),
            LLMMessage::System(m) => m.content.as_parts().iter().map(|x| x.into()).collect(),
            LLMMessage::Developer(m) => m.content.as_parts().iter().map(|x| x.into()).collect(),
            LLMMessage::Tool(m) => m.into(),
        };
        ret.parts = parts;
        ret
    }
}

impl GenaiRequestTrait for GenerateContentRequest {
    fn set_streaming(&mut self, _streaming: bool) {
        unreachable!("setting streaming is not done at a request level in VertexAI")
    }
}

#[cfg(test)]
mod test {

    use google_cloud_aiplatform_v1::model::{
        CodeExecutionResult, ExecutableCode, FunctionCall, FunctionResponse,
    };

    /// What building a client costs, which every Vertex AI call used to pay
    /// before [`super::prediction_service`] kept one. It reads the machine's
    /// application default credentials and sends nothing: the connection, and
    /// the access token, only come with the first request, so a fresh client
    /// paid for those too, on top of this. Run it with
    /// `cargo test --release -p genai_client building_a_vertex_client -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn building_a_vertex_client() {
        let n = 20;
        let start = std::time::Instant::now();
        for _ in 0..n {
            super::PredictionService::builder()
                .build()
                .await
                .expect("could not build a client: are there application default credentials?");
        }
        let per_client = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("building a Vertex AI client: {per_client:.2} ms");
    }

    use crate::{
        OpenAISystemMessage,
        openai::v1::{
            FunctionToolFields, OpenAIImageUrl, OpenAIResponseFormatContent, OpenAISchemaType,
        },
    };

    use super::*;

    const FAKE_GCP_PROJECT: &str = "some-gcp-project";

    fn set_fake_gcp_project() {
        unsafe {
            std::env::set_var(GOOGLE_CLOUD_PROJECT, FAKE_GCP_PROJECT);
        }
    }

    #[test]
    fn endpoint() {
        set_fake_gcp_project();

        let model = "tha-model";
        let region = "aaee";
        let client = VertexAI {
            model: model.to_string(),
            location: region.to_string(),
            ..Default::default()
        };
        let endpoint = client
            .chat_endpoint()
            .expect("chat endpoint should be valid");
        let path = client.model_path().expect("model path should be valid");
        let exp = format!("https://{region}-aiplatform.googleapis.com/v1/{path}:generateContent");
        assert_eq!(exp, endpoint);

        let model = "tha-model";
        let region = "global";
        let client = VertexAI {
            model: model.to_string(),
            location: region.to_string(),
            ..Default::default()
        };
        let endpoint = client
            .chat_endpoint()
            .expect("chat endpoint should be valid");
        let path = client.model_path().expect("model path should be valid");
        let exp = format!("https://aiplatform.googleapis.com/v1/{path}:generateContent");
        assert_eq!(exp, endpoint)
    }

    #[test]
    fn test_default_vertexai_model() {
        assert_eq!(default_vertexai_model(), "gemini-3.8-flash")
    }

    #[test]
    fn test_default_mime_type() {
        assert_eq!("text/plain", default_mime_type());
    }

    #[test]
    fn test_default_vertexai_endpoint() {
        assert_eq!("global", default_vertexai_region())
    }

    #[test]
    fn test_default() {
        let client = VertexAI::default();
        assert_eq!(client.model(), default_vertexai_model());
        assert_eq!(client.location, default_vertexai_region());
        assert_eq!(client.response_mime_type, default_mime_type());
        assert!(client.tools.is_empty());
        assert!(client.top_p.is_none());
        assert!(client.top_k.is_none());
        assert!(client.candidate_count.is_none());
        assert!(client.max_output_tokens.is_none());
        assert!(client.response_logprobs.is_none());
        assert!(client.logprobs.is_none());
        assert!(client.presence_penalty.is_none());
        assert!(client.frequency_penalty.is_none());
        assert!(client.seed.is_none());
        assert!(client.response_format.is_none());
        assert!(client.include_thoughts.is_none());
        assert!(client.thinking_budget.is_none());
        assert!(client.image_aspect_ratio.is_none());
        assert!(client.cached_content.is_none());
        assert!(client.tool_choice.is_none());
    }

    #[test]
    #[should_panic]
    fn body_no_messages() {
        let client = VertexAI::default();
        let content: Vec<Content> = vec![];
        client
            .get_request_body(content, None)
            .expect("no request body");
    }

    #[test]
    #[should_panic]
    fn body_no_weird_role() {
        let client = VertexAI::default();
        let mut msg = Content::default();
        msg.role = "some-unsupported-role".to_string();
        client
            .get_request_body(vec![msg], None)
            .expect("no request body");
    }

    fn check_body(role: &str) {
        let client = VertexAI::default();

        let instruction = "you are an assistant!".to_string();
        let mut system_msg = Content::default();
        system_msg.role = role.to_string();
        let mut system_part = Part::default();
        system_part.data = Some(Data::Text(instruction.clone()));
        system_msg.parts = vec![system_part];

        let txt = "Hey".to_string();
        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text(txt.clone()));
        user_msg.parts = vec![user_part];

        let b = client
            .get_request_body(vec![system_msg, user_msg], None)
            .expect("no request body");
        assert!(b.system_instruction.is_some());
        let sys = b.system_instruction.clone().unwrap();
        assert_eq!(sys.parts.len(), 1);
        assert_eq!(sys.role, role);
        let part = sys.parts.first().unwrap();
        assert!(part.data.is_some());
        if let Data::Text(i) = part.data.clone().unwrap() {
            assert_eq!(i, instruction)
        } else {
            panic!("Expecting Text part")
        }

        // Check user message
        assert_eq!(b.contents.len(), 1);
        assert_eq!(b.contents[0].parts.len(), 1);
        let part = b.contents[0].parts[0].clone();
        assert!(part.data.is_some());
        if let Data::Text(i) = part.data.clone().unwrap() {
            assert_eq!(i, txt)
        } else {
            panic!("Expecting Text part")
        }
    }

    #[test]
    fn body_with_system_prompt() {
        set_fake_gcp_project();
        check_body("system");
        check_body("developer");
    }
    #[test]
    fn body_no_system_prompt() {
        set_fake_gcp_project();
        let client = VertexAI::default();

        let txt = "Hey".to_string();
        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text(txt.clone()));
        user_msg.parts = vec![user_part];

        let b = client
            .get_request_body(vec![user_msg], None)
            .expect("no request body");
        assert!(b.system_instruction.is_none());

        // Check user message
        assert_eq!(b.contents.len(), 1);
        assert_eq!(b.contents[0].parts.len(), 1);
        let part = b.contents[0].parts[0].clone();
        assert!(part.data.is_some());
        if let Data::Text(i) = part.data.clone().unwrap() {
            assert_eq!(i, txt)
        } else {
            panic!("Expecting Text part")
        }
    }

    #[test]
    fn test_project_id() {
        set_fake_gcp_project();
        let client = VertexAI::default();
        assert_eq!(
            client.project_id().expect("no project id"),
            FAKE_GCP_PROJECT
        );
    }

    #[test]
    fn thinking_config_none() {
        let client = VertexAI::default();
        assert!(client.thinking_config().is_none());
    }

    #[test]
    fn thinking_config_include_only() {
        let mut client = VertexAI::default();
        client.include_thoughts = Some(true);
        let cfg = client
            .thinking_config()
            .expect("expected Some when include_thoughts is set");
        assert_eq!(cfg.include_thoughts, Some(true));
        assert!(cfg.thinking_budget.is_none());
    }

    #[test]
    fn thinking_config_budget_only() {
        let mut client = VertexAI::default();
        client.thinking_budget = Some(123);
        let cfg = client
            .thinking_config()
            .expect("expected Some when thinking_budget is set");
        assert_eq!(cfg.thinking_budget, Some(123));
        assert!(cfg.include_thoughts.is_none());
    }

    #[test]
    fn thinking_config_both_set() {
        let mut client = VertexAI::default();
        client.include_thoughts = Some(false);
        client.thinking_budget = Some(7);
        let cfg = client
            .thinking_config()
            .expect("expected Some when either field is set");
        assert_eq!(cfg.include_thoughts, Some(false));
        assert_eq!(cfg.thinking_budget, Some(7));
    }
    #[test]
    fn tool_config_none() {
        let client = VertexAI::default();
        assert!(client.tool_config().is_none());
    }

    #[test]
    fn tool_config_some() {
        let mut client = VertexAI::default();
        client.tool_choice = Some(OpenAIToolChoiceMode::Auto);
        let cfg = client
            .tool_config()
            .expect("expected Some when tool_choice is set");
        assert!(cfg.function_calling_config.is_some());
        assert_eq!(cfg.function_calling_config.unwrap().mode, Mode::Auto);
    }

    #[test]
    fn image_config_none() {
        let client = VertexAI::default();
        assert!(client.image_config().is_none());
    }

    #[test]
    fn image_config_some() {
        let mut client = VertexAI::default();
        client.image_aspect_ratio = Some("16:9".to_string());
        let cfg = client
            .image_config()
            .expect("expected Some when aspect ratio is set");
        assert!(cfg.aspect_ratio.is_some());
        assert_eq!(cfg.aspect_ratio.unwrap(), "16:9");
    }

    #[test]
    fn test_params() {
        set_fake_gcp_project();

        let temperature = 0.2;
        let frequency_penalty = 1.2;
        let presence_penalty = 32.1;
        let top_p = 2.0;
        let top_k = 2.12312;
        let candidate_count = 2;
        let max_output_tokens = 23;
        let response_logprobs = true;
        let cache_name = "cache-name";
        let tools_choice = OpenAIToolChoiceMode::Auto;
        let seed = 123123;
        let response_format = JsonSchema::default();
        let include_thoughts = true;
        let thinking_budget = 129387;
        let image_ratio = "3:4";
        let mut client = VertexAI {
            temperature: Some(temperature),
            top_p: Some(top_p),
            max_output_tokens: Some(max_output_tokens),
            frequency_penalty: Some(frequency_penalty),
            presence_penalty: Some(presence_penalty),
            response_logprobs: Some(response_logprobs),
            response_format: None,
            candidate_count: Some(candidate_count),
            cached_content: Some(cache_name.to_string()),
            tool_choice: Some(tools_choice.clone()),
            top_k: Some(top_k),
            seed: Some(seed),
            include_thoughts: Some(include_thoughts),
            thinking_budget: Some(thinking_budget),
            image_aspect_ratio: Some(image_ratio.to_string()),
            ..Default::default()
        };
        client.set_tools(Some(vec![]));
        assert!(client.tools.is_empty());
        client.set_tools(None);
        assert!(client.tools.is_empty());
        client.set_output_schema(OpenAIResponseFormat {
            schema_type: OpenAISchemaType::JsonSchema,
            json_schema: OpenAIResponseFormatContent {
                name: "some".to_string(),
                schema: response_format,
            },
        });
        assert_eq!(client.response_mime_type, "application/json");
        assert!(client.response_format.is_some());

        let txt = "Hey".to_string();
        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text(txt.clone()));
        user_msg.parts = vec![user_part];
        let messages = vec![user_msg];

        let b = client
            .get_request_body(messages, None)
            .expect("no request body");
        // Model
        let exp_model = format!(
            "projects/{}/locations/global/publishers/google/models/{}",
            FAKE_GCP_PROJECT, &client.model
        );
        assert_eq!(&exp_model, &b.model);

        // Cache
        let exp_cache = format!(
            "projects/{}/locations/global/cachedContents/{}",
            FAKE_GCP_PROJECT, &cache_name
        );
        assert_eq!(&exp_cache, &b.cached_content);

        // Generation config
        let config = b.generation_config.clone().expect("generation_config");
        assert_eq!(
            config.frequency_penalty.expect("frequency_penalty"),
            frequency_penalty
        );
        assert_eq!(
            config.presence_penalty.expect("presence_penalty"),
            presence_penalty
        );
        assert!(config.response_schema.is_some());
        assert_eq!(config.temperature.expect("temperature"), temperature);
        assert_eq!(config.top_p.expect("top_p"), top_p);
        assert_eq!(config.top_k.expect("top_k"), top_k);
        assert_eq!(config.seed.expect("seed"), seed);
        assert_eq!(
            config.max_output_tokens.expect("max_output_tokens"),
            max_output_tokens
        );
        assert_eq!(
            config.response_logprobs.expect("response_logprobs"),
            response_logprobs
        );
        assert_eq!(
            config.candidate_count.expect("candidate_count"),
            candidate_count
        );
        assert!(config.response_schema.is_some());
        assert_eq!(config.response_mime_type, "application/json");
        // Thinking config
        let thinking = config.thinking_config.expect("thinking config");
        assert_eq!(
            thinking.thinking_budget.expect("thinking-budget"),
            thinking_budget
        );
        assert_eq!(
            thinking.include_thoughts.expect("include thoughts"),
            include_thoughts
        );

        assert_eq!(
            config
                .image_config
                .expect("image-config")
                .aspect_ratio
                .expect("aspect ratio"),
            image_ratio
        );
        // Tools
        assert!(b.tools.is_empty());
        assert_eq!(client.count_tools(), 0);

        assert_eq!(b.contents.len(), 1);
        let tool_config = b.tool_config.expect("tool_config");
        assert_eq!(
            tool_config.function_calling_config.unwrap().mode,
            tools_choice.into()
        );

        assert_eq!(client.model(), &default_vertexai_model());
        assert_eq!(
            client.frequency_penalty().expect("frequency_penalty") as f32,
            frequency_penalty as f32
        );
        assert_eq!(
            client.presence_penalty().expect("presence_penalty") as f32,
            presence_penalty as f32
        );
        assert_eq!(
            client.temperature().expect("temperature") as f32,
            temperature as f32
        );
        assert_eq!(client.top_p().expect("top_p") as f32, top_p as f32);
        assert_eq!(
            client.max_tokens().expect("max_tokens") as f32,
            max_output_tokens as f32
        );
        assert_eq!(b.contents.len(), 1);
        assert_eq!(client.count_tools(), 0);
    }

    #[test]
    fn schema_type_conv() {
        assert_eq!(Type::Object, SchemaType::Object.into());
        assert_eq!(Type::Array, SchemaType::Array.into());
        assert_eq!(Type::String, SchemaType::String.into());
        assert_eq!(Type::Number, SchemaType::Number.into());
        assert_eq!(Type::Integer, SchemaType::Integer.into());
        assert_eq!(Type::Boolean, SchemaType::Boolean.into());
        assert_eq!(Type::Unspecified, SchemaType::Null.into());
        assert_eq!(Type::Unspecified, SchemaType::Schema.into());
    }

    fn validate_schema(input: JsonSchema) {
        let output: Schema = input.clone().into();
        if let Some(t) = input.schema_type {
            assert_eq!(output.r#type, t.into());
        } else {
            assert_eq!(output.r#type, Type::Unspecified);
        }
        assert_eq!(output.title, input.name.unwrap_or_default());
        // props
        if let Some(p) = &input.properties {
            for v in p.values() {
                validate_schema(v.clone());
            }
        } else {
            output.properties.is_empty();
        }
        if let Some(items) = &input.items {
            validate_schema(*items.clone());
        } else {
            assert!(output.items.is_none());
        }
        if let Some(req) = &input.required {
            assert_eq!(req, &output.required);
        } else {
            assert!(output.required.is_empty());
        }
        if let Some(e) = &input.r#enum {
            assert_eq!(e, &output.r#enum);
        } else {
            assert!(output.r#enum.is_empty());
        }
        if let Some(d) = &input.description {
            assert_eq!(d, &output.description);
        } else {
            assert!(output.description.is_empty());
        }
    }

    #[test]
    fn schema_conv() {
        let schema = JsonSchema {
            ..Default::default()
        };
        validate_schema(schema);
        let schema = JsonSchema {
            name: Some("the name".to_string()),
            ..Default::default()
        };
        validate_schema(schema);

        let schema = JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(HashMap::from([(
                "some-key".to_string(),
                JsonSchema::default(),
            )])),
            ..Default::default()
        };
        validate_schema(schema);

        let schema = JsonSchema {
            schema_type: Some(SchemaType::Object),
            required: Some(vec!["1".to_string(), "2".to_string()]),
            description: Some("a great schema".to_string()),
            ..Default::default()
        };
        validate_schema(schema);

        let schema = JsonSchema {
            schema_type: Some(SchemaType::Object),
            required: Some(vec!["1".to_string(), "2".to_string()]),
            description: Some("a great schema".to_string()),
            r#enum: Some(vec!["one".to_string(), "another".to_string()]),
            ..Default::default()
        };
        validate_schema(schema);

        let schema = JsonSchema {
            schema_type: Some(SchemaType::Object),
            required: Some(vec!["1".to_string(), "2".to_string()]),
            description: Some("a great schema".to_string()),
            r#enum: Some(vec!["one".to_string(), "another".to_string()]),
            items: Some(Box::new(JsonSchema::default())),
            ..Default::default()
        };
        validate_schema(schema);
    }

    #[test]
    fn tool_conv() {
        let tool_name = "the tool";
        let tool_description = "the most amazing tool";
        let tool = OpenAITool::Function {
            function: FunctionToolFields {
                name: tool_name.to_string(),
                description: Some(tool_description.to_string()),
                parameters: Some(JsonSchema::default()),
                strict: Some(true),
            },
        };

        let tool: FunctionDeclaration = tool.into();
        assert_eq!(&tool.name, tool_name);
        assert_eq!(&tool.description, tool_description);
    }

    #[test]
    fn finish_reason_malformed_function_call() {
        let s: LLMFinishReason = FinishReason::MalformedFunctionCall.into();
        assert_eq!(s, LLMFinishReason::MalformedToolCall);
    }

    #[test]
    fn finish_reason_conv() {
        let s: LLMFinishReason = FinishReason::Stop.into();
        assert_eq!(s, LLMFinishReason::Stop);

        let s: LLMFinishReason = FinishReason::MaxTokens.into();
        assert_eq!(s, LLMFinishReason::Length);

        let s: LLMFinishReason = FinishReason::Safety.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);

        let s: LLMFinishReason = FinishReason::Recitation.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);

        let s: LLMFinishReason = FinishReason::Blocklist.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);

        let s: LLMFinishReason = FinishReason::ProhibitedContent.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);

        let s: LLMFinishReason = FinishReason::Spii.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);

        let s: LLMFinishReason = FinishReason::ModelArmor.into();
        assert_eq!(s, LLMFinishReason::ContentFilter);
        let _s: LLMFinishReason = FinishReason::Unspecified.into();
        assert!(matches!(_s, LLMFinishReason::Other(_)));
        let _s: LLMFinishReason = FinishReason::Other.into();
        assert!(matches!(_s, LLMFinishReason::Other(_)));
    }

    #[test]
    #[should_panic]
    fn bad_tool_choice() {
        let data = Data::Text("some text".to_string());
        function_call_to_tool_choice(data);
    }

    #[test]
    fn tool_choice() {
        let tool_name = "tool_name";

        let mut call = FunctionCall::default();
        call = call.set_name(tool_name.to_string());
        call.args = Some(serde_json::Map::default());

        let data = Data::FunctionCall(Box::new(call));
        let ret = function_call_to_tool_choice(data);
        assert_eq!(ret.name(), tool_name);

        let tool_name = "tool_name";

        let mut call = FunctionCall::default();
        call = call.set_name(tool_name.to_string());
        call.args = None;

        let data = Data::FunctionCall(Box::new(call));
        let ret = function_call_to_tool_choice(data);
        assert_eq!(ret.name(), tool_name);
    }

    #[test]
    fn box_to_file() {
        // Raw bytes in, a data URL out: `file_data` is defined to carry one,
        // and only base64 survives a payload that is not valid UTF-8.
        let mime = "image/png";
        let data = [0x89u8, 0x50, 0x4e, 0x47, 0xff, 0x00];
        let b = Box::new(Blob::default().set_mime_type(mime).set_data(data.to_vec()));

        let file: OpenAIFilePart = b.into();
        assert_eq!(
            file.file.file_data.expect("data"),
            format!("data:image/png;base64,{}", B64.encode(data))
        );
        assert_eq!(&file.file.filename.expect("name"), "file.png");
    }

    #[test]
    fn binary_blob_survives_the_trip_back() {
        // A PNG is not valid UTF-8, which is exactly what used to panic here.
        let png = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe];
        let b = Box::new(
            Blob::default()
                .set_mime_type("image/png")
                .set_data(png.to_vec()),
        );

        let part: OpenAIMessagePart = Data::InlineData(b).into();
        let OpenAIMessagePart::File(f) = part else {
            panic!("expected a file part");
        };

        // Round-trip the data URL and confirm the original bytes come back.
        let url = f.file.file_data.expect("data");
        let payload = url.split_once(',').expect("a data URL").1;
        assert_eq!(B64.decode(payload).expect("valid base64"), png);
    }

    #[test]
    fn part_text_conv() {
        let txt = "some text";
        let part = Data::Text(txt.to_string());
        let other_part: OpenAIMessagePart = part.into();
        if let OpenAIMessagePart::Text(OpenAITextPart { text }) = &other_part {
            assert_eq!(text, &txt)
        } else {
            panic!("No text....{:?}", other_part);
        }
    }

    #[test]
    fn part_inline_data_conv() {
        let mime = "image/png";
        let data = "98712lkjalskjayso9a87-0";
        let b = Box::new(Blob::default().set_mime_type(mime).set_data(data));

        let part = Data::InlineData(b);
        let other_part: OpenAIMessagePart = part.into();
        if let OpenAIMessagePart::File(f) = &other_part {
            let f = f.clone();
            assert_eq!(
                f.file.file_data.expect("data"),
                format!("data:image/png;base64,{}", B64.encode(data))
            );
            assert_eq!(f.file.filename.expect("name"), "file.png");
        } else {
            panic!("No file....{:?}", other_part);
        }
    }

    #[test]
    fn part_file_data_conv() {
        // A picture by URL is an image; any other file, words saying where.
        let mut f = FileData::default();
        f.mime_type = "image/png".to_string();
        f.file_uri = "gs://bucket/a.png".to_string();
        let part: OpenAIMessagePart = Data::FileData(Box::new(f)).into();
        assert!(
            matches!(part, OpenAIMessagePart::ImageUrl(p) if p.image_url.url == "gs://bucket/a.png")
        );
        let mut f = FileData::default();
        f.mime_type = "application/pdf".to_string();
        f.file_uri = "gs://bucket/a.pdf".to_string();
        let part: OpenAIMessagePart = Data::FileData(Box::new(f)).into();
        assert!(matches!(part, OpenAIMessagePart::Text(t) if t.text.contains("gs://bucket/a.pdf")));
    }
    #[test]
    #[should_panic]
    fn part_function_call_conv() {
        let f = FunctionCall::default();
        let part = Data::FunctionCall(Box::new(f));
        let _other_part: OpenAIMessagePart = part.into();
    }
    #[test]
    fn part_function_response_conv() {
        let mut struct_map = serde_json::Map::new();
        struct_map.insert(
            "output".to_string(),
            serde_json::Value::String("the result".to_string()),
        );
        let mut f = FunctionResponse::default();
        f.response = Some(struct_map);
        let part = Data::FunctionResponse(Box::new(f));
        let other_part: OpenAIMessagePart = part.into();
        if let OpenAIMessagePart::Text(t) = other_part {
            assert_eq!(t.text, "the result");
        } else {
            panic!("expected Text part");
        }
    }

    #[test]
    fn tool_message_to_content() {
        use crate::openai::v1::OpenAIToolMessage;
        let tool_msg = LLMMessage::Tool(OpenAIToolMessage {
            tool_call_id: "call_abc123".to_string(),
            content: crate::openai::v1::OpenAIMessageContent::String("tool result".to_string()),
        });
        let content: Content = tool_msg.into();
        assert_eq!(content.role, "user");
        assert_eq!(content.parts.len(), 1);
        if let Some(Data::FunctionResponse(r)) = &content.parts[0].data {
            assert_eq!(r.name, "call_abc123");
            let output = r
                .response
                .as_ref()
                .and_then(|m| m.get("output"))
                .and_then(|v| v.as_str())
                .unwrap();
            assert_eq!(output, "tool result");
        } else {
            panic!("expected FunctionResponse part");
        }
    }

    /// Builds a user `Content` containing a single `functionResponse` part
    /// named `name` — mirroring what a converted `LLMMessage::Tool` looks like.
    fn tool_response_content(name: &str) -> Content {
        let mut response = FunctionResponse::default();
        response.name = name.to_string();
        let mut part = Part::default();
        part.data = Some(Data::FunctionResponse(Box::new(response)));
        let mut content = Content::default();
        content.role = "user".to_string();
        content.parts = vec![part];
        content
    }

    /// Builds a user `Content` with a single text part.
    fn text_user_content(text: &str) -> Content {
        let mut part = Part::default();
        part.data = Some(Data::Text(text.to_string()));
        let mut content = Content::default();
        content.role = "user".to_string();
        content.parts = vec![part];
        content
    }

    /// Builds a model `Content` with `count` function-call parts.
    fn model_calls_content(count: usize) -> Content {
        let parts = (0..count)
            .map(|i| {
                let mut fc = FunctionCall::default();
                fc.name = format!("call_{i}");
                let mut part = Part::default();
                part.data = Some(Data::FunctionCall(Box::new(fc)));
                part
            })
            .collect();
        let mut content = Content::default();
        content.role = "model".to_string();
        content.parts = parts;
        content
    }

    #[test]
    fn resolve_tool_call_names_maps_id_to_function_name() {
        use crate::openai::v1::{
            OpenAIFunctionToolChoice, OpenAIMessageContent, OpenAIToolChoice, OpenAIToolMessage,
        };

        let assistant = LLMMessage::Assistant(AssistantMessage {
            content: vec![],
            name: None,
            refusal: None,
            tool_calls: vec![
                OpenAIToolChoice::Function {
                    id: "call_1".to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "web-search".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
                OpenAIToolChoice::Function {
                    id: "call_2".to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "curl".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
            ],
            audio: None,
            thought_signature: None,
        });
        // Note: results arrive out of call order — id correlation must still hold.
        let messages = vec![
            assistant,
            LLMMessage::Tool(OpenAIToolMessage {
                tool_call_id: "call_2".to_string(),
                content: OpenAIMessageContent::String("curl result".to_string()),
            }),
            LLMMessage::Tool(OpenAIToolMessage {
                tool_call_id: "call_1".to_string(),
                content: OpenAIMessageContent::String("search result".to_string()),
            }),
            // Unknown id must be left untouched.
            LLMMessage::Tool(OpenAIToolMessage {
                tool_call_id: "call_unknown".to_string(),
                content: OpenAIMessageContent::String("orphan".to_string()),
            }),
        ];

        let resolved = resolve_tool_call_names(messages);
        let ids: Vec<&str> = resolved
            .iter()
            .filter_map(|m| match m {
                LLMMessage::Tool(t) => Some(t.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["curl", "web-search", "call_unknown"]);
    }

    #[test]
    fn is_tool_response_detects_function_responses() {
        assert!(is_tool_response(&tool_response_content("a")));
        assert!(!is_tool_response(&text_user_content("hi")));
        assert!(!is_tool_response(&model_calls_content(2)));
        // Empty parts should not be treated as a tool response.
        let mut empty = Content::default();
        empty.role = "user".to_string();
        assert!(!is_tool_response(&empty));
    }

    #[test]
    fn merge_tool_responses_collapses_consecutive_results() {
        let contents = vec![
            text_user_content("do stuff"),
            model_calls_content(3),
            tool_response_content("a"),
            tool_response_content("b"),
            tool_response_content("c"),
        ];
        let merged = merge_tool_responses(contents);
        // text + model + single merged tool-response turn.
        assert_eq!(merged.len(), 3);
        let last = merged.last().unwrap();
        assert_eq!(last.role, "user");
        // The three responses now match the three calls in the model turn.
        assert_eq!(last.parts.len(), 3);
        assert_eq!(merged[1].parts.len(), 3);
    }

    #[test]
    fn merge_tool_responses_keeps_separate_batches() {
        // Two independent tool-call/response rounds must stay separate.
        let contents = vec![
            model_calls_content(1),
            tool_response_content("a"),
            model_calls_content(2),
            tool_response_content("b"),
            tool_response_content("c"),
        ];
        let merged = merge_tool_responses(contents);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[1].parts.len(), 1);
        assert_eq!(merged[3].parts.len(), 2);
    }

    #[test]
    fn merge_tool_responses_does_not_merge_text_users() {
        let contents = vec![text_user_content("one"), text_user_content("two")];
        let merged = merge_tool_responses(contents);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn get_request_body_merges_tool_responses() {
        set_fake_gcp_project();
        let client = VertexAI::default();
        let contents = vec![
            text_user_content("do stuff"),
            model_calls_content(2),
            tool_response_content("a"),
            tool_response_content("b"),
        ];
        let b = client
            .get_request_body(contents, None)
            .expect("no request body");
        // user + model + merged tool responses.
        assert_eq!(b.contents.len(), 3);
        assert_eq!(b.contents[2].parts.len(), 2);
    }

    #[test]
    fn part_executable_code_conv() {
        // Code the model ran is kept as text.
        let mut f = ExecutableCode::default();
        f.code = "print(1)".to_string();
        let part = Data::ExecutableCode(Box::new(f));
        let other_part: OpenAIMessagePart = part.into();
        assert!(matches!(other_part, OpenAIMessagePart::Text(t) if t.text.contains("print(1)")));
    }

    #[test]
    fn part_code_execution_resilt_conv() {
        // So is what it printed.
        let mut f = CodeExecutionResult::default();
        f.output = "1".to_string();
        let part = Data::CodeExecutionResult(Box::new(f));
        let other_part: OpenAIMessagePart = part.into();
        assert!(matches!(other_part, OpenAIMessagePart::Text(t) if t.text.contains('1')));
    }

    #[test]
    fn from_assistant_message_text_only() {
        let m = AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "hello".to_string(),
            })],
            ..Default::default()
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0].data, Some(Data::Text(t)) if t == "hello"));
    }

    #[test]
    fn from_assistant_message_tool_calls_only() {
        use crate::openai::v1::{OpenAIFunctionToolChoice, OpenAIToolChoice};
        let m = AssistantMessage {
            tool_calls: vec![OpenAIToolChoice::Function {
                id: "call_1".to_string(),
                function: OpenAIFunctionToolChoice {
                    name: "fn_a".to_string(),
                    arguments: r#"{"k":"v"}"#.to_string(),
                },
            }],
            ..Default::default()
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 1);
        if let Some(Data::FunctionCall(fc)) = &parts[0].data {
            assert_eq!(fc.name, "fn_a");
            assert_eq!(
                fc.args
                    .as_ref()
                    .and_then(|a| a.get("k"))
                    .and_then(|v| v.as_str()),
                Some("v")
            );
        } else {
            panic!("expected FunctionCall part");
        }
    }

    #[test]
    fn from_assistant_message_multiple_tool_calls() {
        use crate::openai::v1::{OpenAIFunctionToolChoice, OpenAIToolChoice};
        let m = AssistantMessage {
            tool_calls: vec![
                OpenAIToolChoice::Function {
                    id: "call_1".to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "fn_a".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
                OpenAIToolChoice::Function {
                    id: "call_2".to_string(),
                    function: OpenAIFunctionToolChoice {
                        name: "fn_b".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
            ],
            ..Default::default()
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 2);
        let names: Vec<_> = parts
            .iter()
            .filter_map(|p| {
                if let Some(Data::FunctionCall(fc)) = &p.data {
                    Some(fc.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(names, ["fn_a", "fn_b"]);
    }

    #[test]
    fn from_assistant_message_invalid_args_defaults_to_empty() {
        use crate::openai::v1::{OpenAIFunctionToolChoice, OpenAIToolChoice};
        let m = AssistantMessage {
            tool_calls: vec![OpenAIToolChoice::Function {
                id: "call_1".to_string(),
                function: OpenAIFunctionToolChoice {
                    name: "fn_a".to_string(),
                    arguments: "not valid json".to_string(),
                },
            }],
            ..Default::default()
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 1);
        if let Some(Data::FunctionCall(fc)) = &parts[0].data {
            assert!(fc.args.as_ref().map(|a| a.is_empty()).unwrap_or(true));
        } else {
            panic!("expected FunctionCall part");
        }
    }

    #[test]
    fn from_tool_message_string_content() {
        use crate::openai::v1::OpenAIToolMessage;
        let m = OpenAIToolMessage {
            tool_call_id: "call_xyz".to_string(),
            content: OpenAIMessageContent::String("the result".to_string()),
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 1);
        if let Some(Data::FunctionResponse(r)) = &parts[0].data {
            assert_eq!(r.name, "call_xyz");
            assert_eq!(
                r.response
                    .as_ref()
                    .and_then(|m| m.get("output"))
                    .and_then(|v| v.as_str()),
                Some("the result")
            );
        } else {
            panic!("expected FunctionResponse part");
        }
    }

    #[test]
    fn from_tool_message_parts_content() {
        use crate::openai::v1::OpenAIToolMessage;
        let m = OpenAIToolMessage {
            tool_call_id: "call_abc".to_string(),
            content: OpenAIMessageContent::Parts(vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "part result".to_string(),
            })]),
        };
        let parts: Vec<Part> = (&m).into();
        assert_eq!(parts.len(), 1);
        if let Some(Data::FunctionResponse(r)) = &parts[0].data {
            assert_eq!(r.name, "call_abc");
            assert_eq!(
                r.response
                    .as_ref()
                    .and_then(|m| m.get("output"))
                    .and_then(|v| v.as_str()),
                Some("part result")
            );
        } else {
            panic!("expected FunctionResponse part");
        }
    }

    #[test]
    fn assistant_tool_calls_to_content() {
        use crate::openai::v1::{OpenAIFunctionToolChoice, OpenAIToolChoice};
        let msg = LLMMessage::Assistant(AssistantMessage {
            content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                text: "calling a tool".to_string(),
            })],
            tool_calls: vec![OpenAIToolChoice::Function {
                id: "call_1".to_string(),
                function: OpenAIFunctionToolChoice {
                    name: "my_tool".to_string(),
                    arguments: r#"{"x":1}"#.to_string(),
                },
            }],
            ..Default::default()
        });
        let content: Content = msg.into();
        assert_eq!(content.role, "model");
        // one text part + one function call part
        assert_eq!(content.parts.len(), 2);
        if let Some(Data::FunctionCall(fc)) = &content.parts[1].data {
            assert_eq!(fc.name, "my_tool");
            let x = fc
                .args
                .as_ref()
                .and_then(|m| m.get("x"))
                .and_then(|v| v.as_i64())
                .unwrap();
            assert_eq!(x, 1);
        } else {
            panic!("expected FunctionCall part");
        }
    }

    #[test]
    fn content_to_chat_message_user() {
        let mut c = Content::default();
        c.role = "user".to_string();
        let mut p = Part::default();
        p.data = Some(Data::Text("Hello".to_string()));
        c.parts = vec![p];

        let msg: UserMessage = c.into();

        assert_eq!(msg.content.len(), 1);
        if let OpenAIMessagePart::Text(OpenAITextPart { text }) = &msg.content[0] {
            assert_eq!(text, "Hello");
        } else {
            panic!("expected text part");
        }
    }

    #[test]
    fn content_to_chat_message_model_with_toolcall() {
        let mut c = Content::default();
        c.role = "model".to_string();

        // Function call part
        let mut call = FunctionCall::default();
        call = call.set_name("tool_1".to_string());
        call.args = Some(serde_json::Map::new());
        let mut p1 = Part::default();
        p1.data = Some(Data::FunctionCall(Box::new(call)));

        // Text part after the function call
        let mut p2 = Part::default();
        p2.data = Some(Data::Text("after".to_string()));

        c.parts = vec![p1, p2];

        let msg: AssistantMessage = c.into();

        // tool call should be present
        let tools = &msg.tool_calls;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "tool_1");

        assert_eq!(msg.content.len(), 1);
        if let OpenAIMessagePart::Text(OpenAITextPart { text }) = &msg.content[0] {
            assert_eq!(text, "after");
        } else {
            panic!("expected text part");
        }
    }

    #[test]
    fn generate_content_response_to_llm_response() {
        // Build a Content with a single text part
        let mut content = Content::default();
        content.role = "model".to_string();
        let mut part = Part::default();
        part.data = Some(Data::Text("hello".to_string()));
        content.parts = vec![part];

        // Build a Candidate wrapping the content
        let mut cand = Candidate::default();
        cand.content = Some(content);
        cand.finish_reason = FinishReason::Stop;
        cand.index = 5;

        // Build the GenerateContentResponse
        let mut resp = GenerateContentResponse::default();
        resp.candidates = vec![cand];
        resp.model_version = "g-model-123".to_string();

        // Convert and assert
        let llm: LLMResponse = resp.into();
        assert_eq!(llm.model, "g-model-123");
        assert_eq!(llm.choices.len(), 1);

        let choice = &llm.choices[0];
        assert_eq!(choice.index, 5_usize);
        assert_eq!(choice.finish_reason, LLMFinishReason::Stop);

        // Verify the message content was converted
        if let LLMMessage::Assistant(m) = &choice.message {
            assert_eq!(m.content.len(), 1);
            if let OpenAIMessagePart::Text(OpenAITextPart { text }) = &m.content[0] {
                assert_eq!(text, "hello");
            } else {
                panic!("expected text part");
            }
        } else {
            panic!("expecting assistant message")
        }
    }

    #[test]
    fn generate_content_response_to_chat_chunk() {
        // A model turn with a tool call followed by some text.
        let mut content = Content::default();
        content.role = "model".to_string();

        let mut call = FunctionCall::default();
        call = call.set_name("get_weather".to_string());
        let mut args = serde_json::Map::new();
        args.insert("city".to_string(), serde_json::json!("Paris"));
        call.args = Some(args);
        let mut call_part = Part::default();
        call_part.data = Some(Data::FunctionCall(Box::new(call)));

        let mut text_part = Part::default();
        text_part.data = Some(Data::Text("here you go".to_string()));

        content.parts = vec![call_part, text_part];

        let mut cand = Candidate::default();
        cand.content = Some(content);
        cand.finish_reason = FinishReason::Stop;
        cand.index = 0;

        let mut usage = UsageMetadata::default();
        usage.prompt_token_count = 3;
        usage.candidates_token_count = 4;
        usage.total_token_count = 7;

        let mut resp = GenerateContentResponse::default();
        resp.candidates = vec![cand];
        resp.model_version = "gemini-test".to_string();
        resp.usage_metadata = Some(usage);

        let chunk = VertexStream::default().chunk(resp).unwrap();

        assert_eq!(chunk.model, "gemini-test");
        // Usage carried over from the full response.
        let usage = chunk.usage.expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 3);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 7);

        assert_eq!(chunk.choices.len(), 1);
        let choice = &chunk.choices[0];
        assert_eq!(choice.finish_reason, Some(OpenAIFinishReason::Stop));

        // The text is in the delta content.
        let content = choice.delta.content.as_ref().expect("content");
        let text: String = content
            .as_parts()
            .iter()
            .map(|p| {
                let s: String = p.into();
                s
            })
            .collect();
        assert_eq!(text, "here you go");

        // The tool call is a fully-formed delta fragment.
        let tool_calls = choice.delta.tool_calls.as_ref().expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].index, 0);
        assert_eq!(
            tool_calls[0].function.as_ref().unwrap().name.as_deref(),
            Some("get_weather")
        );
        assert_eq!(
            tool_calls[0]
                .function
                .as_ref()
                .unwrap()
                .arguments
                .as_deref(),
            Some(r#"{"city":"Paris"}"#)
        );
    }

    #[test]
    fn chat_chunk_preserves_thought_signature() {
        // A model turn whose function-call part carries a thought signature.
        let mut content = Content::default();
        content.role = "model".to_string();

        let mut call = FunctionCall::default();
        call = call.set_name("get_weather".to_string());
        let mut call_part = Part::default();
        call_part.data = Some(Data::FunctionCall(Box::new(call)));
        call_part.thought_signature = bytes::Bytes::from_static(b"sig-123");
        content.parts = vec![call_part];

        let mut cand = Candidate::default();
        cand.content = Some(content);
        cand.finish_reason = FinishReason::Stop;

        let mut resp = GenerateContentResponse::default();
        resp.candidates = vec![cand];

        let chunk = VertexStream::default().chunk(resp).unwrap();
        let delta = &chunk.choices[0].delta;
        assert_eq!(
            delta.thought_signature.as_deref(),
            Some(b"sig-123".as_slice()),
            "thought signature must survive the streaming chunk conversion"
        );
    }

    /// A partial streaming response: one candidate with the given parts.
    fn partial_response(parts: Vec<Part>, finish_reason: FinishReason) -> GenerateContentResponse {
        let mut content = Content::default();
        content.role = "model".to_string();
        content.parts = parts;

        let mut cand = Candidate::default();
        cand.content = Some(content);
        cand.finish_reason = finish_reason;

        let mut resp = GenerateContentResponse::default();
        resp.candidates = vec![cand];
        resp
    }

    fn text_part(text: &str) -> Part {
        let mut part = Part::default();
        part.data = Some(Data::Text(text.to_string()));
        part
    }

    fn call_part(name: &str) -> Part {
        let call = FunctionCall::default().set_name(name.to_string());
        let mut part = Part::default();
        part.data = Some(Data::FunctionCall(Box::new(call)));
        part
    }

    #[test]
    fn stream_only_final_chunk_finishes() {
        let mut stream = VertexStream::default();

        let first = stream
            .chunk(partial_response(
                vec![text_part("Hel")],
                FinishReason::Unspecified,
            ))
            .unwrap();
        assert_eq!(first.choices[0].finish_reason, None);

        let last = stream
            .chunk(partial_response(vec![text_part("lo")], FinishReason::Stop))
            .unwrap();
        assert_eq!(
            last.choices[0].finish_reason,
            Some(OpenAIFinishReason::Stop)
        );
    }

    #[test]
    fn stream_tool_call_indices_continue_across_chunks() {
        let mut stream = VertexStream::default();

        let first = stream
            .chunk(partial_response(
                vec![call_part("a"), call_part("b")],
                FinishReason::Unspecified,
            ))
            .unwrap();
        let second = stream
            .chunk(partial_response(vec![call_part("c")], FinishReason::Stop))
            .unwrap();

        let indices: Vec<usize> = [first, second]
            .iter()
            .flat_map(|c| c.choices[0].delta.tool_calls.clone().unwrap())
            .map(|tc| tc.index)
            .collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn stream_forwards_first_thought_signature_only() {
        let mut stream = VertexStream::default();

        let mut signed_call = call_part("a");
        signed_call.thought_signature = bytes::Bytes::from_static(b"sig-call");
        let mut signed_text = text_part("");
        signed_text.thought_signature = bytes::Bytes::from_static(b"sig-later");

        let first = stream
            .chunk(partial_response(
                vec![signed_call],
                FinishReason::Unspecified,
            ))
            .unwrap();
        let second = stream
            .chunk(partial_response(vec![signed_text], FinishReason::Stop))
            .unwrap();

        assert_eq!(
            first.choices[0].delta.thought_signature.as_deref(),
            Some(b"sig-call".as_slice())
        );
        assert_eq!(second.choices[0].delta.thought_signature, None);
    }

    /// A streamed malformed call ends the stream with that finish reason,
    /// instead of cutting it short with nothing but a log line.
    #[test]
    fn stream_malformed_function_call_is_reported() {
        let resp = partial_response(vec![], FinishReason::MalformedFunctionCall);
        let chunk = VertexStream::default().chunk(resp).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some(OpenAIFinishReason::MalformedToolCall)
        );
    }

    #[test]
    fn stream_candidate_without_content() {
        // The final response may carry only a finish reason and usage.
        let mut cand = Candidate::default();
        cand.finish_reason = FinishReason::Stop;
        let mut resp = GenerateContentResponse::default();
        resp.candidates = vec![cand];

        let chunk = VertexStream::default().chunk(resp).unwrap();
        let choice = &chunk.choices[0];
        assert_eq!(choice.finish_reason, Some(OpenAIFinishReason::Stop));
        assert!(choice.delta.content.is_none());
        assert!(choice.delta.tool_calls.is_none());
    }

    #[test]
    fn bad_role() {
        // Read as the user's, rather than bringing the run down.
        let mut content = Content::default();
        content.role = "no real role".to_string();
        assert_eq!(content.role(), Role::User);
    }

    #[test]
    fn user_role() {
        let mut content = Content::default();
        content.role = "user".to_string();
        assert_eq!(content.role(), Role::User);
        assert!(!content.has_content());
        assert!(content.content_string().is_none());
    }

    #[test]
    fn model_role() {
        let mut content = Content::default();
        content.role = "model".to_string();
        assert_eq!(content.role(), Role::Assistant);
        assert!(!content.has_content());
        assert!(content.content_string().is_none());
    }
    #[test]
    fn system_role() {
        let mut content = Content::default();
        content.role = "system".to_string();
        assert_eq!(content.role(), Role::System);
        assert!(!content.has_content());
        assert!(content.content_string().is_none());
    }

    #[test]
    fn content_to_string() {
        let mut content = Content::default();

        let txt1 = "hey!";
        let part_1 = Part::default();
        let part_1 = part_1.set_data(Data::Text(txt1.to_string()));
        content.parts.push(part_1);

        let thought = "...hmmm";
        let part_2 = Part::default();
        let part_2 = part_2.set_data(Data::Text(thought.to_string()));
        let part_2 = part_2.set_thought(true);
        content.parts.push(part_2);

        let txt2 = "I got it!";
        let part_3 = Part::default();
        let part_3 = part_3.set_data(Data::Text(txt2.to_string()));
        content.parts.push(part_3);

        let s = content.content_string().expect("str");
        assert_eq!(s, format!("{txt1}\n{txt2}"))
    }

    #[test]
    fn content_to_string_img() {
        let mut content = Content::default();

        let txt1 = "hey!";
        let part_1 = Part::default();
        let part_1 = part_1.set_data(Data::Text(txt1.to_string()));
        content.parts.push(part_1);

        let thought = "...hmmm";
        let part_2 = Part::default();
        let part_2 = part_2.set_data(Data::Text(thought.to_string()));
        let part_2 = part_2.set_thought(true);
        content.parts.push(part_2);

        let part_3 = Part::default();
        let part_3 = part_3.set_data(Data::FileData(Box::new(FileData::default())));
        content.parts.push(part_3);

        let s = content.content_string().expect("str");
        assert_eq!(s, format!("{txt1}\n[file]"))
    }

    #[test]
    fn image_to_filedata() {
        let url = "the_image.png";
        let part = OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: url.to_string(),
                ..Default::default()
            },
        };
        let Data::FileData(newp) = image_part_to_data(&part).expect("should convert") else {
            panic!("a non-data URL should stay a file reference");
        };

        assert_eq!(newp.file_uri, url);
        assert_eq!(newp.mime_type, "image/png");
    }

    #[test]
    fn filepart_to_blob() {
        // Bare base64 with no data URL header: the type comes from the filename.
        let raw = "some file bytes";
        let data = B64.encode(raw);
        let part = OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(data.clone()),
                file_id: None,
                filename: Some("the_image.png".to_string()),
            },
        };
        let newp = file_part_to_blob(&part).expect("should convert");

        assert_eq!(newp.data, raw.as_bytes());
        assert_eq!(newp.mime_type, "image/png");

        // Some data... no extension
        let part = OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(data),
                file_id: None,
                filename: Some("the_image".to_string()),
            },
        };
        let newp = file_part_to_blob(&part).expect("should convert");

        assert_eq!(newp.mime_type, "text/plain");

        // Now with no data... there is nothing to send, so this is an error
        // rather than a silently empty blob.
        let part = OpenAIFilePart {
            file: OpenAIFile {
                file_data: None,
                file_id: None,
                filename: Some("the_image".to_string()),
            },
        };
        assert!(file_part_to_blob(&part).is_err());
    }

    #[test]
    fn test_mimetype() {
        // Test default cases
        assert_eq!(mimetype(""), "text/plain");
        assert_eq!(mimetype("noextension"), "text/plain");
        assert_eq!(mimetype("unknown.xyz"), "text/plain");

        // Test audio formats
        assert_eq!(mimetype("audio.aac"), "audio/aac");
        assert_eq!(mimetype("music.mp3"), "audio/mpeg");
        assert_eq!(mimetype("sound.wav"), "audio/wav");
        assert_eq!(mimetype("song.midi"), "audio/midi");
        assert_eq!(mimetype("song.mid"), "audio/midi");
        assert_eq!(mimetype("audio.oga"), "audio/ogg");
        assert_eq!(mimetype("audio.opus"), "audio/ogg");
        assert_eq!(mimetype("audio.weba"), "audio/webm");

        // Test image formats
        assert_eq!(mimetype("image.apng"), "image/apng");
        assert_eq!(mimetype("image.avif"), "image/avif");
        assert_eq!(mimetype("image.bmp"), "image/bmp");
        assert_eq!(mimetype("image.gif"), "image/gif");
        assert_eq!(mimetype("image.ico"), "image/vnd.microsoft.icon");
        assert_eq!(mimetype("image.jpeg"), "image/jpeg");
        assert_eq!(mimetype("image.jpg"), "image/jpeg");
        assert_eq!(mimetype("image.png"), "image/png");
        assert_eq!(mimetype("image.svg"), "image/svg+xml");
        assert_eq!(mimetype("image.tif"), "image/tiff");
        assert_eq!(mimetype("image.tiff"), "image/tiff");
        assert_eq!(mimetype("image.webp"), "image/webp");

        // Test video formats
        assert_eq!(mimetype("video.avi"), "video/x-msvideo");
        assert_eq!(mimetype("video.mp4"), "video/mp4");
        assert_eq!(mimetype("video.mpeg"), "video/mpeg");
        assert_eq!(mimetype("video.ogv"), "video/ogg");
        assert_eq!(mimetype("video.webm"), "video/webm");
        assert_eq!(mimetype("video.3gp"), "video/3gpp");
        assert_eq!(mimetype("video.3g2"), "video/3gpp2");
        assert_eq!(mimetype("video.ts"), "video/mp2t");

        // Test document formats
        assert_eq!(mimetype("doc.abw"), "applicaton/x-abiword");
        assert_eq!(mimetype("doc.doc"), "application/msword");
        assert_eq!(
            mimetype("doc.docx"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(
            mimetype("doc.odt"),
            "application/vnd.oasis.opendocument.text"
        );
        assert_eq!(mimetype("doc.pdf"), "application/pdf");
        assert_eq!(mimetype("doc.rtf"), "application/rtf");
        assert_eq!(mimetype("doc.txt"), "text/plain");

        // Test spreadsheet formats
        assert_eq!(
            mimetype("sheet.ods"),
            "application/vnd.oasis.opendocument.spreadsheet"
        );
        assert_eq!(mimetype("sheet.xls"), "application/vnd.ms-excel");
        assert_eq!(
            mimetype("sheet.xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );

        // Test presentation formats
        assert_eq!(
            mimetype("pres.odp"),
            "application/vnd.oasis.opendocument.presentation"
        );
        assert_eq!(mimetype("pres.ppt"), "application/vnd.ms-powerpoint");
        assert_eq!(
            mimetype("pres.pptx"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        );

        // Test web formats
        assert_eq!(mimetype("web.css"), "text/css");
        assert_eq!(mimetype("web.htm"), "text/html");
        assert_eq!(mimetype("web.html"), "text/html");
        assert_eq!(mimetype("web.js"), "text/javascript");
        assert_eq!(mimetype("web.mjs"), "text/javascript");
        assert_eq!(mimetype("web.json"), "application/json");
        assert_eq!(mimetype("web.jsonld"), "application/ld+json");
        assert_eq!(mimetype("web.php"), "application/x-httpd-php");
        assert_eq!(mimetype("web.xml"), "application/xml");
        assert_eq!(mimetype("web.xhtml"), "application/xhtml+xml");
        assert_eq!(mimetype("web.xul"), "application/vnd.mozilla.xul+xml");
        assert_eq!(mimetype("web.webmanifest"), "application/manifest+json");

        // Test font formats
        assert_eq!(mimetype("font.eot"), "application/vnd.ms-fontobject");
        assert_eq!(mimetype("font.otf"), "font/otf");
        assert_eq!(mimetype("font.ttf"), "font/ttf");
        assert_eq!(mimetype("font.woff"), "font/woff");
        assert_eq!(mimetype("font.woff2"), "font/woff2");

        // Test archive formats
        assert_eq!(mimetype("archive.7z"), "application/x-7z-compressed");
        assert_eq!(mimetype("archive.arc"), "application/x-freearc");
        assert_eq!(mimetype("archive.bz"), "application/x-bzip");
        assert_eq!(mimetype("archive.bz2"), "application/x-bzip2");
        assert_eq!(mimetype("archive.gz"), "application/gzip");
        assert_eq!(mimetype("archive.rar"), "application/vnd.rar");
        assert_eq!(mimetype("archive.tar"), "application/x-tar");
        assert_eq!(mimetype("archive.zip"), "application/zip");

        // Test other formats
        assert_eq!(mimetype("other.azw"), "application/vnd.amazon.ebook");
        assert_eq!(mimetype("other.bin"), "application/octet-stream");
        assert_eq!(mimetype("other.cda"), "application/x-cdf");
        assert_eq!(mimetype("other.csh"), "application/x-csh");
        assert_eq!(mimetype("other.ics"), "text/calendar");
        assert_eq!(mimetype("other.jar"), "application/java-archive");
        assert_eq!(mimetype("other.md"), "text/markdown");
        assert_eq!(
            mimetype("other.mpkg"),
            "application/vnd.apple.installer+xml"
        );
        assert_eq!(mimetype("other.ogx"), "application/ogg");
        assert_eq!(mimetype("other.sh"), "application/x-sh");
        assert_eq!(mimetype("other.vsd"), "application/vnd.visio");
    }

    #[test]
    fn openai_messagepart_to_part() {
        // Test text part conversion
        let text = "hello world";
        let text_part = OpenAIMessagePart::Text(OpenAITextPart {
            text: text.to_string(),
        });
        let part: Part = (&text_part).into();
        assert!(part.data.is_some());
        if let Data::Text(t) = part.data.unwrap() {
            assert_eq!(t, text);
        } else {
            panic!("Expected Text data");
        }

        // Test image URL part conversion
        let img_url = "http://example.com/image.png";
        let img_part = OpenAIMessagePart::ImageUrl(OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: img_url.to_string(),
                ..Default::default()
            },
        });
        let part: Part = (&img_part).into();
        assert!(part.data.is_some());
        if let Data::FileData(f) = part.data.unwrap() {
            assert_eq!(f.file_uri, img_url);
        } else {
            panic!("Expected FileData");
        }

        // Test file part conversion. `file_data` is base64 on the way in and
        // raw bytes on the way out, since the SDK encodes `Blob::data` itself.
        let file_part = OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(B64.encode("file content")),
                filename: Some("test.txt".to_string()),
                file_id: None,
            },
        });
        let part: Part = (&file_part).into();
        assert!(part.data.is_some());
        if let Data::InlineData(b) = part.data.unwrap() {
            assert_eq!(b.data, "file content".as_bytes());
            assert_eq!(b.mime_type, "text/plain");
        } else {
            panic!("Expected InlineData");
        }
    }

    #[test]
    fn data_url_split() {
        assert_eq!(
            split_data_url("data:application/pdf;base64,QUJD"),
            Some(("application/pdf", "QUJD"))
        );
        // Parameters are dropped; Vertex wants a bare IANA type.
        assert_eq!(
            split_data_url("data:text/plain;charset=utf-8;base64,QUJD"),
            Some(("text/plain", "QUJD"))
        );
        // Not data URLs at all.
        assert_eq!(split_data_url("https://example.com/a.png"), None);
        assert_eq!(split_data_url("gs://bucket/a.png"), None);
        assert_eq!(split_data_url("QUJD"), None);
        // Data URLs we cannot decode or type.
        assert_eq!(split_data_url("data:text/plain,hello"), None);
        assert_eq!(split_data_url("data:;base64,QUJD"), None);
    }

    #[test]
    fn image_data_url_becomes_inline_blob() {
        // The shape a browser upload arrives in: it must become inline bytes,
        // not a `file_uri` that Vertex cannot resolve.
        let png = [0x89u8, 0x50, 0x4e, 0x47];
        let part = OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: format!("data:image/png;base64,{}", B64.encode(png)),
                ..Default::default()
            },
        };
        match image_part_to_data(&part).expect("should convert") {
            Data::InlineData(b) => {
                assert_eq!(b.mime_type, "image/png");
                assert_eq!(b.data, png.as_slice());
            }
            other => panic!("expected InlineData, got {other:?}"),
        }
    }

    #[test]
    fn image_remote_uri_stays_a_reference() {
        let url = "gs://some-bucket/photo.jpg";
        let part = OpenAIImagePart {
            image_url: OpenAIImageUrl {
                url: url.to_string(),
                ..Default::default()
            },
        };
        match image_part_to_data(&part).expect("should convert") {
            Data::FileData(f) => {
                assert_eq!(f.file_uri, url);
                assert_eq!(f.mime_type, "image/jpeg");
            }
            other => panic!("expected FileData, got {other:?}"),
        }
    }

    #[test]
    fn file_part_prefers_data_url_mime_over_filename() {
        // The data URL states the type; the filename disagrees and must lose.
        let part = OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some(format!(
                    "data:application/pdf;base64,{}",
                    B64.encode("%PDF")
                )),
                filename: Some("report.txt".to_string()),
                file_id: None,
            },
        };
        let blob = file_part_to_blob(&part).expect("should convert");
        assert_eq!(blob.mime_type, "application/pdf");
        assert_eq!(blob.data, "%PDF".as_bytes());
    }

    #[test]
    fn file_part_without_data_is_an_error() {
        // An uploaded-file reference is meaningless to Vertex, and saying so
        // beats sending an empty blob.
        let part = OpenAIFilePart {
            file: OpenAIFile {
                file_data: None,
                file_id: Some("file-abc123".to_string()),
                filename: None,
            },
        };
        assert!(file_part_to_blob(&part).is_err());
    }

    #[test]
    fn malformed_attachment_degrades_to_text() {
        // Bad base64 must not panic the request; it becomes a note to the model.
        let part = OpenAIMessagePart::File(OpenAIFilePart {
            file: OpenAIFile {
                file_data: Some("not valid base64!!".to_string()),
                filename: Some("test.pdf".to_string()),
                file_id: None,
            },
        });
        let part: Part = (&part).into();
        match part.data.expect("should have data") {
            Data::Text(t) => assert!(t.contains("unusable attachment")),
            other => panic!("expected a Text fallback, got {other:?}"),
        }
    }

    #[test]
    fn audio_part_is_converted_inline() {
        // Audio used to hit a `todo!()` and abort the process.
        let pcm = [0x00u8, 0x01, 0x02, 0x03];
        let part = OpenAIMessagePart::InputAudio(OpenAIAudioPart {
            input_audio: crate::openai::v1::OpenAIInputAudio {
                data: B64.encode(pcm),
                format: crate::AudioFormat::Wav,
            },
        });
        let part: Part = (&part).into();
        match part.data.expect("should have data") {
            Data::InlineData(b) => {
                assert_eq!(b.mime_type, "audio/wav");
                assert_eq!(b.data, pcm.as_slice());
            }
            other => panic!("expected InlineData, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_carries_documents_beside_its_text() {
        let tool_msg = OpenAIToolMessage {
            tool_call_id: "call_abc123".to_string(),
            content: OpenAIMessageContent::Parts(vec![
                OpenAIMessagePart::Text(OpenAITextPart {
                    text: "here is the invoice".to_string(),
                }),
                OpenAIMessagePart::File(OpenAIFilePart {
                    file: OpenAIFile {
                        file_data: Some(format!(
                            "data:application/pdf;base64,{}",
                            B64.encode("%PDF")
                        )),
                        filename: None,
                        file_id: None,
                    },
                }),
            ]),
        };

        let parts: Vec<Part> = (&tool_msg).into();
        assert_eq!(parts.len(), 1);
        let Some(Data::FunctionResponse(r)) = parts[0].data.clone() else {
            panic!("expected a FunctionResponse");
        };

        // Text stays in `output`, the document travels as a separate part.
        let output = r
            .response
            .as_ref()
            .and_then(|m| m.get("output"))
            .and_then(|v| v.as_str())
            .expect("output");
        assert_eq!(output, "here is the invoice");

        assert_eq!(r.parts.len(), 1);
        match r.parts[0].data.clone().expect("part data") {
            FunctionResponseData::InlineData(b) => {
                assert_eq!(b.mime_type, "application/pdf");
                assert_eq!(b.data, "%PDF".as_bytes());
            }
            other => panic!("expected InlineData, got {other:?}"),
        }
    }

    #[test]
    fn openai_messagecontent_to_parts() {
        // Test string content conversion
        let text = "simple message";
        let content = OpenAIMessageContent::String(text.to_string());
        let parts: Vec<Part> = (&content).into();
        assert_eq!(parts.len(), 1);
        if let Data::Text(t) = parts[0].data.clone().unwrap() {
            assert_eq!(t, text);
        } else {
            panic!("Expected Text data");
        }

        // Test parts content conversion
        let parts_content = OpenAIMessageContent::Parts(vec![
            OpenAIMessagePart::Text(OpenAITextPart {
                text: "first".to_string(),
            }),
            OpenAIMessagePart::Text(OpenAITextPart {
                text: "second".to_string(),
            }),
        ]);
        let parts: Vec<Part> = (&parts_content).into();
        assert_eq!(parts.len(), 2);
        if let Data::Text(t) = parts[0].data.clone().unwrap() {
            assert_eq!(t, "first");
        } else {
            panic!("Expected Text data");
        }
        if let Data::Text(t) = parts[1].data.clone().unwrap() {
            assert_eq!(t, "second");
        } else {
            panic!("Expected Text data");
        }
    }

    #[test]
    fn chatmessage_to_content() {
        // Test user message conversion
        let user_msg: UserMessage = "user input".to_string().into();
        let user_msg = LLMMessage::User(user_msg);
        let content: Content = user_msg.into();
        assert_eq!(content.role, "user");
        assert_eq!(content.parts.len(), 1);
        if let Data::Text(t) = content.parts[0].data.clone().unwrap() {
            assert_eq!(t, "user input");
        } else {
            panic!("Expected Text data");
        }

        // Test system message conversion
        let system_msg: OpenAISystemMessage = "system prompt".to_string().into();
        let system_msg = LLMMessage::System(system_msg);
        let content: Content = system_msg.into();
        assert_eq!(content.role, "system");
        assert_eq!(content.parts.len(), 1);
        if let Data::Text(t) = content.parts[0].data.clone().unwrap() {
            assert_eq!(t, "system prompt");
        } else {
            panic!("Expected Text data");
        }

        // Test assistant message conversion
        let assistant_msg: AssistantMessage = "assistant reply".to_string().into();
        let assistant_msg = LLMMessage::Assistant(assistant_msg);

        let content: Content = assistant_msg.into();
        assert_eq!(content.role, "model");
        assert_eq!(content.parts.len(), 1);
        if let Data::Text(t) = content.parts[0].data.clone().unwrap() {
            assert_eq!(t, "assistant reply");
        } else {
            panic!("Expected Text data");
        }
    }

    #[test]
    fn get_request_body_with_allowed_tools() {
        set_fake_gcp_project();

        let mut client = VertexAI::default();

        // Create some tools
        let mut tool1 = Tool::default();
        let mut func1 = FunctionDeclaration::default();
        func1.name = "tool_1".to_string();
        tool1.function_declarations = vec![func1];

        let mut tool2 = Tool::default();
        let mut func2a = FunctionDeclaration::default();
        func2a.name = "tool_2a".to_string();
        let mut func2b = FunctionDeclaration::default();
        func2b.name = "tool_2b".to_string();
        tool2.function_declarations = vec![func2a, func2b];

        client.tools = vec![tool1, tool2];

        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text("test".to_string()));
        user_msg.parts = vec![user_part];

        // Allow only specific tools
        let allowed = Some(vec!["tool_1".to_string(), "tool_2b".to_string()]);
        let body = client
            .get_request_body(vec![user_msg], allowed)
            .expect("no request body");

        // Should have 1 tool (with tool_1 and tool_2b)
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].function_declarations.len(), 2);
        assert_eq!(body.tools[0].function_declarations[0].name, "tool_1");
        assert_eq!(body.tools[0].function_declarations[1].name, "tool_2b");
    }

    #[test]
    fn get_request_body_with_no_allowed_tools() {
        set_fake_gcp_project();

        let mut client = VertexAI::default();

        let mut tool = Tool::default();
        let mut func = FunctionDeclaration::default();
        func.name = "my_tool".to_string();
        tool.function_declarations = vec![func];

        client.tools = vec![tool];

        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text("test".to_string()));
        user_msg.parts = vec![user_part];

        // No allowed_tools filter
        let body = client
            .get_request_body(vec![user_msg], None)
            .expect("no request body");

        // Should have all tools
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].function_declarations.len(), 1);
        assert_eq!(body.tools[0].function_declarations[0].name, "my_tool");
    }

    #[test]
    fn get_request_body_filters_tools_correctly() {
        set_fake_gcp_project();

        let mut client = VertexAI::default();

        let mut tool = Tool::default();
        let mut func1 = FunctionDeclaration::default();
        func1.name = "allowed_func".to_string();
        let mut func2 = FunctionDeclaration::default();
        func2.name = "filtered_func".to_string();
        tool.function_declarations = vec![func1, func2];

        client.tools = vec![tool];

        let mut user_msg = Content::default();
        user_msg.role = "user".to_string();
        let mut user_part = Part::default();
        user_part.data = Some(Data::Text("test".to_string()));
        user_msg.parts = vec![user_part];

        let allowed = Some(vec!["allowed_func".to_string()]);
        let body = client
            .get_request_body(vec![user_msg], allowed)
            .expect("no request body");

        // Should only have the allowed function
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].function_declarations.len(), 1);
        assert_eq!(body.tools[0].function_declarations[0].name, "allowed_func");
    }

    #[test]
    fn usage_metadata_to_openai_usage() {
        let mut meta = UsageMetadata::default();
        meta.prompt_token_count = 10;
        meta.candidates_token_count = 20;
        meta.total_token_count = 30;
        meta.thoughts_token_count = 5;
        meta.cached_content_token_count = 3;

        let usage = OpenAIUsage::from(&meta);

        assert_eq!(usage.prompt_tokens, 10);
        // Thinking tokens fold into the completion count, matching OpenAI's
        // meaning of the field: 20 visible + 5 thought.
        assert_eq!(usage.completion_tokens, 25);
        assert_eq!(usage.total_tokens, 30);

        let completion_details = usage.completion_tokens_details.unwrap();
        assert_eq!(completion_details.reasoning_tokens, 5);

        let prompt_details = usage.prompt_tokens_details.unwrap();
        assert_eq!(prompt_details.cached_tokens, 3);
    }

    #[test]
    fn usage_metadata_zeros_to_openai_usage() {
        let meta = UsageMetadata::default();
        let usage = OpenAIUsage::from(&meta);

        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage.completion_tokens_details.is_some());
        assert!(usage.prompt_tokens_details.is_some());
    }
}

#[cfg(all(test, feature = "integration_test"))]
mod integration_test {
    use futures::StreamExt;

    use super::{GenaiClientTrait, VertexAI};
    use crate::{LLMChunk, LLMMessage, openai::v1::OpenAIFinishReason};

    #[tokio::test]
    async fn test_vertex_ai_stream() {
        let llm = VertexAI::default();

        let messages: Vec<LLMMessage> = vec![
            "Count from 1 to 50, one number per line."
                .to_string()
                .into(),
        ];
        let mut r = llm
            .stream::<LLMMessage, LLMChunk>(messages, None)
            .await
            .expect("Expected a response from stream");
        let mut count = 0;
        let mut done: Option<OpenAIFinishReason> = None;
        let mut usage_seen = false;
        while let Some(e) = r.next().await {
            count += 1;
            usage_seen |= e.usage.is_some();
            assert!(done.is_none(), "no chunk may follow the finishing one");
            done = e.choices[0].finish_reason;
        }
        assert_eq!(done, Some(OpenAIFinishReason::Stop));
        assert!(count > 1, "Expected the response in several chunks");
        assert!(usage_seen, "Expected usage on the final chunk");
    }
}
