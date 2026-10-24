// Generates images with a Gemini image model on Vertex AI.
//
// The model returns pictures the same way it returns text: as parts of a
// `generateContent` response, with `inlineData` carrying the bytes. Those
// parts are streamed to the client as `SSEvent::FileAvailable` — the same
// route `GoogleAgentSearchTool` uses for its references — while the LLM gets
// back only a short JSON summary. The picture is what the reader wants; the
// model only needs to know that making it worked.

use std::collections::HashMap;

use async_trait::async_trait;
use genai_client::{
    JsonSchema, SSEvent,
    events::TokenUsage,
    openai::v1::{
        OpenAICompletionTokenDetails, OpenAIFile, OpenAIFilePart, OpenAIMessagePart,
        OpenAIPromptTokenDetails, OpenAIUsage,
    },
    schema::SchemaType,
    vertex_ai::{SafetySetting, safety_settings_off},
};
use protocols::{FormaDB, FormaError, FormaMessagePart};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CallContext, ToolTrait,
    tool::{mailgun_tool::string_schema, resolve_project_id, workspace::conversation},
};

use tracing::{info, warn};

/// `node_id` for every [`SSEvent::Usage`] this tool emits.
///
/// A tool only receives a `CallContext`, which has no reference to the node
/// that invoked it — unlike `Node::emit_usage`, which reads its own
/// `self.id`. Hardcoded here rather than threaded through `CallContext`
/// because `UsageTally`, the billing rollup, keys strictly on
/// `(provider, model)` and ignores `node_id` entirely, so a fixed label
/// costs nothing in billing accuracy.
const USAGE_NODE_ID: &str = "google-gen-image-tool";

pub fn default_gen_image_model() -> String {
    // "gemini-2.5-flash-image".to_string()
    "gemini-3.1-flash-lite-image".to_string()
}

pub fn default_gen_image_location() -> String {
    "global".to_string()
}

pub fn default_gen_image_timeout_seconds() -> u64 {
    120
}

/// Generates images with a Gemini image model on Vertex AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleGenImageTool {
    /// The Google Cloud project billed for the generation.
    ///
    /// When unset, it is read from the `GOOGLE_CLOUD_PROJECT` environment
    /// variable at call time — see [`resolve_project_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,

    /// The image-capable Gemini model to call.
    ///
    /// Examples are:
    /// - `gemini-3.1-flash-lite-image` (default, aka. Nano banana 2 lite)
    /// - `gemini-3.1-flash-image` (aka., Nano banana 2)
    /// - `gemini-3-pro-image` (aka., Nano banana pro)
    /// - `gemini-2.5-flash-image` (aka., Nano banana)
    pub model: String,

    /// The Vertex location. `global` uses the unprefixed host; a region uses
    /// its own, as `<region>-aiplatform.googleapis.com`.
    pub location: String,

    /// Passed through as `imageConfig.aspectRatio` when set.
    pub aspect_ratio: Option<String>,

    /// The safety filters sent with the request. Empty means the field is
    /// omitted entirely, leaving Google's own defaults in force.
    pub safety_settings: Vec<SafetySetting>,

    /// How long a single generation may take. Images are slower than text,
    /// so this is well above a chat timeout.
    pub request_timeout_seconds: u64,
}

impl Default for GoogleGenImageTool {
    fn default() -> Self {
        Self {
            project_id: None,
            model: default_gen_image_model(),
            location: default_gen_image_location(),
            aspect_ratio: None,
            safety_settings: safety_settings_off(),
            request_timeout_seconds: default_gen_image_timeout_seconds(),
        }
    }
}

/// What the LLM fills in.
#[derive(Debug, Deserialize)]
struct GenImageRequest {
    prompt: String,
}

// ── request wire format ─────────────────────────────────────────────────

/// One `contents[].parts[]` entry. Only text prompts are sent, so this is
/// the only part shape the request ever needs.
#[derive(Debug, Serialize)]
struct TextPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct RequestContent {
    role: &'static str,
    parts: Vec<TextPart>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageConfig {
    aspect_ratio: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    response_modalities: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_config: Option<ImageConfig>,
}

/// One proto-enum value (a safety category or threshold), rendered the way
/// the REST API documents it.
///
/// Google's SDK serialises these enums as their proto numbers. The wire
/// protocol accepts that, but the names are what the documentation shows and
/// what makes a logged request legible, so [`SafetySettingWire::from`] sends
/// the name whenever the SDK knows one; the number is the fallback for a
/// value this SDK version does not recognise.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SafetyEnumValue {
    Name(String),
    Number(i32),
}

#[derive(Debug, Serialize)]
struct SafetySettingWire {
    category: SafetyEnumValue,
    threshold: SafetyEnumValue,
}

impl From<&SafetySetting> for SafetySettingWire {
    fn from(setting: &SafetySetting) -> Self {
        fn name_or_number(name: Option<&str>, value: Option<i32>) -> SafetyEnumValue {
            match name {
                Some(n) => SafetyEnumValue::Name(n.to_string()),
                None => SafetyEnumValue::Number(value.unwrap_or_default()),
            }
        }
        Self {
            category: name_or_number(setting.category.name(), setting.category.value()),
            threshold: name_or_number(setting.threshold.name(), setting.threshold.value()),
        }
    }
}

/// The `generateContent` request body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenImageRequestBody {
    contents: Vec<RequestContent>,
    generation_config: GenerationConfig,
    // Omitted rather than sent empty, so that "no settings" means Google's
    // defaults rather than an explicitly empty list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    safety_settings: Vec<SafetySettingWire>,
}

// ── response wire format ────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponsePart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    inline_data: Option<InlineData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ResponseContent {
    #[serde(default)]
    parts: Vec<ResponsePart>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<ResponseContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Token accounting Vertex reports alongside a `generateContent` response.
///
/// A local mirror of the wire shape rather than the Vertex SDK's own
/// `UsageMetadata`: this tool talks to the REST endpoint directly (see the
/// module doc) instead of going through `GenaiClient`, so the typed SDK
/// response never exists here to convert from.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenImageUsage {
    #[serde(default)]
    prompt_token_count: usize,
    #[serde(default)]
    candidates_token_count: usize,
    #[serde(default)]
    total_token_count: usize,
    #[serde(default)]
    cached_content_token_count: usize,
    #[serde(default)]
    thoughts_token_count: usize,
}

impl From<&GenImageUsage> for OpenAIUsage {
    /// Same normalisation `vertex_ai::mod.rs` applies to the typed SDK
    /// response: Gemini reports thinking tokens outside
    /// `candidates_token_count`, so they're folded into `completion_tokens`
    /// here to match the OpenAI semantics that `TokenUsage`'s own
    /// conversion assumes.
    fn from(value: &GenImageUsage) -> Self {
        Self {
            prompt_tokens: value.prompt_token_count,
            completion_tokens: value.candidates_token_count + value.thoughts_token_count,
            total_tokens: value.total_token_count,
            completion_tokens_details: Some(OpenAICompletionTokenDetails {
                reasoning_tokens: value.thoughts_token_count,
                ..Default::default()
            }),
            prompt_tokens_details: Some(OpenAIPromptTokenDetails {
                cached_tokens: value.cached_content_token_count,
                ..Default::default()
            }),
        }
    }
}

/// The `generateContent` response body.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenImageResponseBody {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<GenImageUsage>,
    #[serde(default)]
    model_version: Option<String>,
}

/// The short JSON summary handed back to the LLM. The image bytes never
/// appear here — see the module doc.
#[derive(Debug, Serialize)]
struct GenImageSummary {
    ok: bool,
    images: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    /// How the conversation names each picture, so the agent can refer to
    /// one — to save it into a workspace, say.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    /// The pictures themselves, for the rest of this turn (see
    /// `SessionData::produced_files`). Never part of the result.
    #[serde(skip)]
    produced: Vec<FormaMessagePart>,
}

impl GoogleGenImageTool {
    /// The project this tool's generations are billed to, resolved as
    /// described on [`resolve_project_id`].
    fn project_id(&self) -> Result<String, FormaError> {
        resolve_project_id("GoogleGenImageTool", self.project_id.as_deref())
    }

    /// The `generateContent` endpoint for the configured model.
    fn endpoint(&self, project_id: &str) -> String {
        let host = if self.location == "global" {
            "https://aiplatform.googleapis.com".to_string()
        } else {
            format!("https://{}-aiplatform.googleapis.com", self.location)
        };
        format!(
            "{host}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
            project_id, self.location, self.model
        )
    }

    /// Builds the request body.
    ///
    /// `responseModalities` is the whole point: without `IMAGE` in that list
    /// an image model answers in prose about the picture it would have drawn.
    fn request_body(&self, prompt: &str) -> GenImageRequestBody {
        let image_config = self
            .aspect_ratio
            .clone()
            .map(|aspect_ratio| ImageConfig { aspect_ratio });

        GenImageRequestBody {
            contents: vec![RequestContent {
                role: "user",
                parts: vec![TextPart {
                    text: prompt.to_string(),
                }],
            }],
            generation_config: GenerationConfig {
                response_modalities: vec!["TEXT", "IMAGE"],
                image_config,
            },
            safety_settings: self
                .safety_settings
                .iter()
                .map(SafetySettingWire::from)
                .collect(),
        }
    }

    /// Reports what the call consumed: an [`SSEvent::Usage`] for billing —
    /// mirroring `Node::emit_usage` — plus a log line for observability.
    fn emit_usage(
        &self,
        tx: &tokio::sync::mpsc::UnboundedSender<SSEvent>,
        model_version: Option<&str>,
        usage: &OpenAIUsage,
    ) -> Result<(), FormaError> {
        // Prefer the model version the provider echoed back; fall back to
        // the configured one when it's absent, so the record is never
        // unattributable.
        let model = model_version
            .filter(|m| !m.is_empty())
            .unwrap_or(self.model.as_str());

        info!(
            model = %model,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            total_tokens = usage.total_tokens,
            "Vertex AI image generation token usage"
        );

        tx.send(SSEvent::Usage {
            node_id: USAGE_NODE_ID.to_string(),
            model: model.to_string(),
            // Named as every other call to Vertex AI is, so that its tokens are
            // priced under the same key (OpenTelemetry's provider name).
            provider: genai_client::genai_client_trait::LLMProvider::VertexAI
                .otel()
                .to_string(),
            server_address: genai_client::vertex_ai::vertex_ai_host(&self.location),
            usage: TokenUsage::from(usage),
        })
        .map_err(|e| {
            FormaError::Execution(format!("GoogleGenImageTool: could not emit usage: {e}"))
        })
    }

    /// Pulls the generated images out of a response, emitting one
    /// [`SSEvent::FileAvailable`] each, and returns the summary the LLM sees.
    ///
    /// Any prose the model produced alongside the picture is kept in that
    /// summary: it is often a caption, and it is the only part of the answer
    /// the model can read back.
    fn process_response(
        &self,
        body: GenImageResponseBody,
        tx: &tokio::sync::mpsc::UnboundedSender<SSEvent>,
    ) -> Result<GenImageSummary, FormaError> {
        if body.candidates.is_empty() {
            return Err(FormaError::Serde(format!(
                "GoogleGenImageTool: response carried no candidates: {body:?}"
            )));
        }

        // Reported regardless of whether a picture came back: a safety block
        // still spends prompt tokens, so it still has to be billed.
        let usage = match &body.usage_metadata {
            Some(usage) => usage.into(),
            None => {
                warn!("GoogleGenImageTool: Vertex AI response carried no usage metadata");
                OpenAIUsage::default()
            }
        };
        self.emit_usage(tx, body.model_version.as_deref(), &usage)?;

        let mut images = 0usize;
        let mut notes: Vec<String> = Vec::new();
        let mut produced: Vec<FormaMessagePart> = Vec::new();

        for candidate in &body.candidates {
            let Some(content) = &candidate.content else {
                continue;
            };

            for part in &content.parts {
                if let Some(text) = &part.text {
                    if !text.trim().is_empty() {
                        notes.push(text.trim().to_string());
                    }
                    continue;
                }

                let Some(inline) = &part.inline_data else {
                    continue;
                };

                images += 1;
                let extension = inline.mime_type.split('/').next_back().unwrap_or("png");
                let file = OpenAIMessagePart::File(OpenAIFilePart {
                    file: OpenAIFile {
                        file_data: Some(format!(
                            "data:{};base64,{}",
                            inline.mime_type, inline.data
                        )),
                        file_id: None,
                        filename: Some(format!("generated-{images}.{extension}")),
                    },
                });
                produced.push(file.clone().into());
                tx.send(SSEvent::FileAvailable {
                    id: format!("gen-image-{images}"),
                    part: file,
                })
                .map_err(|e| {
                    FormaError::Execution(format!(
                        "GoogleGenImageTool: could not emit the generated image: {e}"
                    ))
                })?;
            }
        }

        if images == 0 {
            // Worth reporting rather than claiming success: a safety block
            // returns a well-formed response with no picture in it.
            let reason = body
                .candidates
                .first()
                .and_then(|c| c.finish_reason.clone())
                .unwrap_or_else(|| "unknown".to_string());
            return Ok(GenImageSummary {
                ok: false,
                images: 0,
                note: notes.join("\n"),
                finish_reason: Some(reason),
                error: Some("the model returned no image".to_string()),
                display: None,
                files: Vec::new(),
                produced: Vec::new(),
            });
        }

        Ok(GenImageSummary {
            ok: true,
            images,
            note: notes.join("\n"),
            finish_reason: None,
            error: None,
            // Said plainly because the bytes are not in this result: the LLM
            // would otherwise describe an image it cannot see, or try again.
            display: Some(
                "The image has already been shown to the user. Do not describe it in detail; refer to it briefly."
                    .to_string(),
            ),
            files: produced
                .iter()
                .filter_map(conversation::data_url_of)
                .map(conversation::handle_of)
                .collect(),
            produced,
        })
    }
}

impl TryFrom<GoogleGenImageTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_item: GoogleGenImageTool) -> Result<JsonSchema, FormaError> {
        let mut props = HashMap::new();
        props.insert(
            "prompt".to_string(),
            string_schema(
                "a detailed description of the image to generate, including subject, style and composition",
            ),
        );
        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props),
            required: Some(vec!["prompt".to_string()]),
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for GoogleGenImageTool {
    fn check(&self) -> Result<(), FormaError> {
        // A stated-but-blank project is a mistake worth catching now; an
        // *absent* one is not, because the environment supplies it at call
        // time and this check runs when the agent is loaded.
        if self
            .project_id
            .as_ref()
            .is_some_and(|p| p.trim().is_empty())
        {
            return Err(FormaError::Input(
                "GoogleGenImageTool: 'project_id' must not be empty".to_string(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(FormaError::Input(
                "GoogleGenImageTool: 'model' must not be empty".to_string(),
            ));
        }
        if self.location.trim().is_empty() {
            return Err(FormaError::Input(
                "GoogleGenImageTool: 'location' must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let req: GenImageRequest = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("GoogleGenImageTool: invalid input: {e}")))?;

        if req.prompt.trim().is_empty() {
            return Err(FormaError::Input("Empty image prompt".to_string()));
        }

        let url = self.endpoint(&self.project_id()?);
        let body = self.request_body(&req.prompt);

        #[cfg(not(feature = "airtight"))]
        {
            // Application-default credentials, as everywhere else Forma talks
            // to Google: injected from the environment when deployed, from
            // `gcloud auth application-default login` locally. Kept between
            // calls, with their token.
            use crate::tool::google_auth;

            let auth_headers = google_auth::auth_headers(&[google_auth::CLOUD_PLATFORM_SCOPE])
                .await
                .map_err(|e| FormaError::ApiCall {
                    url: Some(url.clone()),
                    msg: e.to_string(),
                })?;

            info!(url = %url, model = self.model, "Vertex AI image generation");
            let client = crate::tool::http::client();
            let resp = client
                .post(&url)
                .headers(auth_headers)
                .timeout(std::time::Duration::from_secs(self.request_timeout_seconds))
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                warn!(url = %url, %status, error = %error_body, "Vertex AI image generation failed");
                return Err(FormaError::ApiCall {
                    url: Some(url),
                    msg: format!("HTTP {status}: {error_body}"),
                });
            }

            let body =
                resp.json::<GenImageResponseBody>()
                    .await
                    .map_err(|e| FormaError::ApiCall {
                        url: Some(url),
                        msg: format!("Vertex AI returned a body that is not JSON: {e}"),
                    })?;

            let mut summary = self.process_response(body, &ctx.tx)?;
            ctx.session
                .write()
                .await
                .produced_files
                .append(&mut summary.produced);
            return Ok(Some(
                serde_json::to_value(summary).expect("GenImageSummary always serialises"),
            ));
        }
        #[cfg(feature = "airtight")]
        {
            // Airtight builds make no network calls.
            let _ = (url, body, &ctx);
            Ok(Some(Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use db::NoneDB;
    use genai_client::vertex_ai::{HarmBlockThreshold, HarmCategory};
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use serde_json::json;
    use tokio::sync::{Mutex, RwLock, mpsc};

    fn tool() -> GoogleGenImageTool {
        GoogleGenImageTool {
            project_id: Some("the-project".to_string()),
            ..Default::default()
        }
    }

    /// A context whose receiver is kept, so a test can read what the tool
    /// streamed to the client.
    fn make_ctx() -> (CallContext<NoneDB>, mpsc::UnboundedReceiver<SSEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "test".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        (ctx, rx)
    }

    // ── configuration ─────────────────────────────────────────────────────

    #[test]
    fn global_location_uses_the_bare_host() {
        // The model is read from the default rather than spelled out, so
        // changing which model Forma ships does not break this.
        assert_eq!(
            tool().endpoint("the-project"),
            format!(
                "https://aiplatform.googleapis.com/v1/projects/the-project/locations/global/publishers/google/models/{}:generateContent",
                default_gen_image_model()
            )
        );
    }

    #[test]
    fn regional_location_prefixes_the_host() {
        let t = GoogleGenImageTool {
            location: "europe-west4".to_string(),
            ..tool()
        };
        assert!(
            t.endpoint("the-project")
                .starts_with("https://europe-west4-aiplatform.googleapis.com/")
        );
        assert!(
            t.endpoint("the-project")
                .contains("/locations/europe-west4/")
        );
    }

    #[test]
    fn check_rejects_empty_configuration() {
        let empty = GoogleGenImageTool {
            project_id: Some(String::new()),
            ..tool()
        };
        assert!(<GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&empty).is_err());

        let no_model = GoogleGenImageTool {
            model: String::new(),
            ..tool()
        };
        assert!(<GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&no_model).is_err());

        assert!(<GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&tool()).is_ok());
    }

    /// `check` runs when the agent is loaded, which may be long before — and
    /// on a different machine than — the call. An absent project is therefore
    /// not yet a problem: the environment gets its turn at call time.
    #[test]
    fn check_without_a_project_is_ok_even_with_no_environment() {
        let _env = crate::tool::ProjectEnvGuard::set(None);
        let t = GoogleGenImageTool {
            project_id: None,
            ..tool()
        };
        assert!(<GoogleGenImageTool as ToolTrait<db::NoneDB>>::check(&t).is_ok());
    }

    #[test]
    fn schema_only_takes_a_prompt() {
        let schema: JsonSchema = tool().try_into().expect("schema");
        let props = schema.properties.expect("properties");
        assert_eq!(props.len(), 1);
        assert!(props.contains_key("prompt"));
        assert_eq!(schema.required, Some(vec!["prompt".to_string()]));
    }

    // ── project resolution ────────────────────────────────────────────────

    #[test]
    fn a_stated_project_is_billed_and_the_environment_ignored() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("env-project"));
        let t = tool();
        assert_eq!(t.project_id().unwrap(), "the-project");
        assert!(
            t.endpoint(&t.project_id().unwrap())
                .contains("/projects/the-project/")
        );
    }

    #[test]
    fn an_unstated_project_comes_from_the_environment_at_call_time() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("env-project"));
        let t = GoogleGenImageTool {
            project_id: None,
            ..tool()
        };
        assert_eq!(t.project_id().unwrap(), "env-project");
        assert!(
            t.endpoint(&t.project_id().unwrap())
                .contains("/projects/env-project/")
        );
    }

    /// The tool value is fixed once the agent is loaded, so the same one has
    /// to follow the environment it is called in.
    #[test]
    fn the_same_tool_follows_the_environment_between_calls() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("first-project"));
        let t = GoogleGenImageTool {
            project_id: None,
            ..tool()
        };
        assert_eq!(t.project_id().unwrap(), "first-project");

        crate::tool::set_or_unset(Some("second-project"));
        assert_eq!(t.project_id().unwrap(), "second-project");
    }

    /// Reported as a plain input error when the call happens, rather than by
    /// panicking somewhere in the load path.
    #[tokio::test]
    async fn execute_without_any_project_fails_with_a_helpful_error() {
        let _env = crate::tool::ProjectEnvGuard::set(None);
        let t = GoogleGenImageTool {
            project_id: None,
            ..tool()
        };

        let (ctx, _rx) = make_ctx();
        let err = t
            .execute(json!({ "prompt": "a cat" }), ctx)
            .await
            .expect_err("no project is configured anywhere");
        let FormaError::Input(msg) = err else {
            panic!("expected an Input error, got {err:?}");
        };
        assert!(msg.contains("project_id"), "{msg}");
        assert!(msg.contains("GOOGLE_CLOUD_PROJECT"), "{msg}");
    }

    // ── serialisation ─────────────────────────────────────────────────────

    /// A tool with no project of its own must round-trip as one, rather than
    /// picking one up from whichever machine happened to serialise it.
    #[test]
    fn an_unstated_project_is_neither_written_nor_invented_when_read_back() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("env-project"));
        let t = GoogleGenImageTool {
            project_id: None,
            ..tool()
        };

        let value = serde_json::to_value(&t).expect("serialises");
        assert!(
            value.get("project_id").is_none(),
            "an absent project must not be written out: {value}"
        );

        let back: GoogleGenImageTool = serde_json::from_value(value).expect("deserialises");
        assert_eq!(back.project_id, None);
    }

    // ── request body ──────────────────────────────────────────────────────

    #[test]
    fn body_asks_for_an_image_modality() {
        let body = serde_json::to_value(tool().request_body("a cat")).unwrap();
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            json!(["TEXT", "IMAGE"])
        );
        assert_eq!(body["contents"][0]["parts"][0]["text"], "a cat");
        // No aspect ratio configured, so no imageConfig at all.
        assert!(body["generationConfig"].get("imageConfig").is_none());
    }

    #[test]
    fn body_carries_the_aspect_ratio_when_set() {
        let t = GoogleGenImageTool {
            aspect_ratio: Some("16:9".to_string()),
            ..tool()
        };
        let body = serde_json::to_value(t.request_body("a cat")).unwrap();
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            "16:9"
        );
    }

    #[test]
    fn safety_settings_default_to_off_and_serialise_as_google_expects() {
        let body = serde_json::to_value(tool().request_body("a cat")).unwrap();
        let settings = body["safetySettings"].as_array().expect("safety settings");
        assert_eq!(settings.len(), 4);
        for s in settings {
            assert_eq!(s["threshold"], "OFF");
        }
        let categories: Vec<&str> = settings
            .iter()
            .map(|s| s["category"].as_str().expect("category"))
            .collect();
        assert!(categories.contains(&"HARM_CATEGORY_HATE_SPEECH"));
        assert!(categories.contains(&"HARM_CATEGORY_DANGEROUS_CONTENT"));
        assert!(categories.contains(&"HARM_CATEGORY_SEXUALLY_EXPLICIT"));
        assert!(categories.contains(&"HARM_CATEGORY_HARASSMENT"));
    }

    #[test]
    fn configured_safety_settings_are_sent_verbatim() {
        let t = GoogleGenImageTool {
            safety_settings: vec![
                SafetySetting::new()
                    .set_category(HarmCategory::HateSpeech)
                    .set_threshold(HarmBlockThreshold::BlockOnlyHigh),
            ],
            ..tool()
        };
        let body = serde_json::to_value(t.request_body("a cat")).unwrap();
        assert_eq!(
            body["safetySettings"],
            json!([{ "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_ONLY_HIGH" }])
        );
    }

    #[test]
    fn empty_safety_settings_omit_the_field() {
        let t = GoogleGenImageTool {
            safety_settings: vec![],
            ..tool()
        };
        let body = serde_json::to_value(t.request_body("a cat")).unwrap();
        assert!(body.get("safetySettings").is_none());
    }

    // ── response handling ─────────────────────────────────────────────────

    fn response_with_image() -> GenImageResponseBody {
        serde_json::from_value(json!({
            "candidates": [{
                "content": { "role": "model", "parts": [
                    { "text": "Here is your cat." },
                    { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } },
                ]},
                "finishReason": "STOP",
            }]
        }))
        .expect("fixture should deserialise")
    }

    /// The summary names each picture the way the conversation will, so the
    /// agent can save one; the picture itself is kept for the rest of the turn.
    #[test]
    fn each_image_is_named_for_the_agent_and_kept_for_the_turn() {
        let (ctx, _rx) = make_ctx();
        let out = tool()
            .process_response(response_with_image(), &ctx.tx)
            .expect("should process");
        assert_eq!(
            out.files,
            vec![conversation::handle_of(
                "data:image/png;base64,iVBORw0KGgo="
            )]
        );
        assert_eq!(out.produced.len(), 1);
        assert_eq!(
            conversation::data_url_of(&out.produced[0]),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        let summary = serde_json::to_value(&out).unwrap();
        assert!(summary.get("produced").is_none());
    }

    #[test]
    fn an_image_is_emitted_as_an_event_not_returned_to_the_llm() {
        let (ctx, mut rx) = make_ctx();
        let out = tool()
            .process_response(response_with_image(), &ctx.tx)
            .expect("should process");

        // The LLM gets a summary, and crucially not the bytes.
        assert!(out.ok);
        assert_eq!(out.images, 1);
        assert_eq!(out.note, "Here is your cat.");
        assert!(
            !serde_json::to_string(&out).unwrap().contains("iVBORw0KGgo"),
            "the payload must not be echoed into the LLM's context"
        );

        // No usage metadata in the fixture, so usage is still reported —
        // just defaulted to zero — before the file itself.
        assert!(matches!(rx.try_recv(), Ok(SSEvent::Usage { .. })));

        let event = rx.try_recv().expect("a file event");
        let SSEvent::FileAvailable { part, .. } = event else {
            panic!("expected a FileAvailable event");
        };
        let OpenAIMessagePart::File(f) = part else {
            panic!("expected a file part");
        };
        assert_eq!(
            f.file.file_data.as_deref(),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        assert_eq!(f.file.filename.as_deref(), Some("generated-1.png"));
    }

    #[test]
    fn several_images_each_get_their_own_event() {
        let (ctx, mut rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({
            "candidates": [{ "content": { "parts": [
                { "inlineData": { "mimeType": "image/png", "data": "AAA=" } },
                { "inlineData": { "mimeType": "image/jpeg", "data": "BBB=" } },
            ]}}]
        }))
        .unwrap();

        let out = tool().process_response(body, &ctx.tx).expect("processes");
        assert_eq!(out.images, 2);
        assert!(matches!(rx.try_recv(), Ok(SSEvent::Usage { .. })));
        assert!(matches!(rx.try_recv(), Ok(SSEvent::FileAvailable { .. })));
        assert!(matches!(rx.try_recv(), Ok(SSEvent::FileAvailable { .. })));
        assert!(rx.try_recv().is_err(), "only three events expected");
    }

    #[test]
    fn a_blocked_generation_reports_failure_rather_than_success() {
        // A safety block is a well-formed response with no picture in it, so
        // the tool has to look at what came back rather than at the status.
        let (ctx, mut rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": "SAFETY" }]
        }))
        .unwrap();

        let out = tool().process_response(body, &ctx.tx).expect("processes");
        assert!(!out.ok);
        assert_eq!(out.images, 0);
        assert_eq!(out.finish_reason.as_deref(), Some("SAFETY"));
        assert!(
            matches!(rx.try_recv(), Ok(SSEvent::Usage { .. })),
            "a blocked generation still spent tokens, so it is still billed"
        );
        assert!(rx.try_recv().is_err(), "nothing to show the reader");
    }

    #[test]
    fn a_response_without_candidates_is_an_error() {
        let (ctx, _rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({})).unwrap();
        assert!(tool().process_response(body, &ctx.tx).is_err());
    }

    // ── usage ────────────────────────────────────────────────────────────

    #[test]
    fn usage_is_reported_before_the_image_and_reflects_the_response() {
        let (ctx, mut rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({
            "candidates": [{ "content": { "parts": [
                { "inlineData": { "mimeType": "image/png", "data": "AAA=" } },
            ]}}],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15,
            },
            "modelVersion": "gemini-3.1-flash-image-001",
        }))
        .unwrap();

        tool().process_response(body, &ctx.tx).expect("processes");

        let Ok(SSEvent::Usage {
            node_id,
            model,
            provider,
            server_address,
            usage,
        }) = rx.try_recv()
        else {
            panic!("expected a Usage event first");
        };
        assert_eq!(node_id, USAGE_NODE_ID);
        assert_eq!(model, "gemini-3.1-flash-image-001");
        assert_eq!(provider, "gcp.vertex_ai");
        assert_eq!(
            server_address,
            genai_client::vertex_ai::vertex_ai_host(&tool().location)
        );
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);

        assert!(matches!(rx.try_recv(), Ok(SSEvent::FileAvailable { .. })));
    }

    #[test]
    fn usage_folds_thinking_tokens_into_output_tokens() {
        let (ctx, mut rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": "SAFETY" }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 7,
                "cachedContentTokenCount": 2,
                "totalTokenCount": 22,
            },
        }))
        .unwrap();

        tool().process_response(body, &ctx.tx).expect("processes");

        let Ok(SSEvent::Usage { usage, .. }) = rx.try_recv() else {
            panic!("expected a Usage event");
        };
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.cached_input_tokens, 2);
        // Gemini reports thinking tokens outside candidatesTokenCount; they
        // must land in output_tokens rather than vanish from the bill.
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.reasoning_tokens, 7);
    }

    #[test]
    fn usage_falls_back_to_the_configured_model_when_the_response_omits_a_version() {
        let (ctx, mut rx) = make_ctx();
        let body: GenImageResponseBody = serde_json::from_value(json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": "SAFETY" }],
            "usageMetadata": { "promptTokenCount": 3, "totalTokenCount": 3 },
        }))
        .unwrap();

        tool().process_response(body, &ctx.tx).expect("processes");

        let Ok(SSEvent::Usage { model, .. }) = rx.try_recv() else {
            panic!("expected a Usage event");
        };
        assert_eq!(model, default_gen_image_model());
    }

    #[tokio::test]
    async fn execute_rejects_an_empty_prompt() {
        let (ctx, _rx) = make_ctx();
        let out = tool().execute(json!({ "prompt": "  " }), ctx).await;
        assert!(out.is_err());
    }

    #[tokio::test]
    async fn execute_rejects_input_without_a_prompt() {
        let (ctx, _rx) = make_ctx();
        let out = tool().execute(json!({ "subject": "a cat" }), ctx).await;
        assert!(out.is_err());
    }
}
