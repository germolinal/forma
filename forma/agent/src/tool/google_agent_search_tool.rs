// Tool based on
// https://docs.cloud.google.com/generative-ai-app-builder/docs/reference/rest/v1/projects.locations.collections.dataStores.servingConfigs/search
//
// Searches a Vertex AI Search (a.k.a. Discovery Engine / Agent Builder) data
// store. The only argument the LLM supplies is the search `query`; everything
// else (which project, which data store, page size, boost spec, page
// categories) is fixed configuration on the tool.

use std::{collections::HashMap, fmt::Display, sync::Arc};

use async_trait::async_trait;
use genai_client::{JsonSchema, SSEvent, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    CallContext, ToolTrait,
    tool::{mailgun_tool::string_schema, resolve_project_id, tavily_search_tool::SearchRequest},
};

use tracing::info;
#[cfg(not(feature = "airtight"))]
use tracing::warn;

/// Data stores created through the console live under this collection, and are
/// served by this serving config. We assume these defaults.
const DEFAULT_COLLECTION: &str = "default_collection";
const DEFAULT_SERVING_CONFIG: &str = "default_serving_config";

/// The value Google reports in a result's `snippet_status` when it managed to
/// build a snippet for it.
const SNIPPET_SUCCESS: &str = "SUCCESS";

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Copy, Clone, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentSearchLocation {
    #[default]
    Global,
    Us,
    Eu,
}

impl AgentSearchLocation {
    fn host(&self) -> &'static str {
        match self {
            Self::Global => "discoveryengine.googleapis.com",
            Self::Eu => "eu-discoveryengine.googleapis.com",
            Self::Us => "us-discoveryengine.googleapis.com",
        }
    }
}

impl Display for AgentSearchLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Global => write!(f, "global"),
            Self::Eu => write!(f, "eu"),
            Self::Us => write!(f, "us"),
        }
    }
}

pub fn default_show_snippet() -> bool {
    false
}

pub fn default_extractive_answer_count() -> i32 {
    0
}

pub fn default_extractive_segment_count() -> i32 {
    1
}

/// Searches a Google Vertex AI Search / Agent Builder data store.
///
/// Authentication uses Google Cloud application-default credentials, which are
/// injected at call time (locally via `gcloud auth application-default login`,
/// and automatically on deployed machines) — exactly like the `VertexAI`
/// client. No API key env var is required.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoogleAgentSearchTool {
    /// The Google Cloud project that owns the data store / search app.
    ///
    /// When unset, it is read from the `GOOGLE_CLOUD_PROJECT` environment
    /// variable at call time — see [`resolve_project_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,

    /// The ID of the data store to search.
    pub datastore_id: String,

    /// The location of the data store. Defaults to `global`; regional data
    /// stores use e.g. `us` or `eu`.
    #[serde(default)]
    pub location: AgentSearchLocation,

    /// Maximum number of results to return (the request's `pageSize`). When
    /// `None`, the API default is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i32>,

    /// Optional boost specification, passed through verbatim as the request's
    /// `boostSpec`. See the Discovery Engine `BoostSpec` schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_spec: Option<Value>,

    /// Optional list of page categories to restrict the search to (the
    /// request's `pageCategories`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_categories: Vec<String>,

    /// A value of true will generate a short piece of text from the search
    /// result document. This is meant to be rendered on the client side,
    /// as a preview of the search result's content.
    /// It is sent along with a Reference event.
    ///
    /// Defaults to `false`.
    ///
    /// This is not passed to the LLM as context to generate a response unless
    /// no extractive answers AND no extractive segments are requested.
    ///
    /// From [Google's documentation](https://docs.cloud.google.com/generative-ai-app-builder/docs/snippets):
    ///
    /// > A snippet is a brief extract of text from the search result
    /// > document that provides a preview of a search result's content.
    /// > It includes hit highlighting that you can render in your UI.
    /// > Snippets are typically displayed under each search result to
    /// > help end users assess the relevance and usefulness of that result.
    /// > Snippets are available for data stores with unstructured data and
    /// > website data (both basic website search and advanced website indexing).
    #[serde(default = "default_show_snippet")]
    pub return_snippet: bool,

    /// How many extractive answers to generate. These are brief pieces of text
    /// taken from the document, which will be used as context for the LLM
    /// that will generate a response. They will also be sent to the client
    /// for support. Defaults to `0` (we generally favour extractive segments
    /// over snippets for LLM generation).
    ///
    /// From [Google's documentation](https://docs.cloud.google.com/generative-ai-app-builder/docs/snippets):
    ///
    /// > An extractive answer is verbatim text that is returned with each search
    /// > result. It is extracted directly from the original document. Extractive
    /// > answers are typically displayed near the top of web pages to provide an
    /// > end user with a brief answer that is contextually relevant to their query.
    /// >
    /// > Extractive answers are available for data stores with unstructured data and
    /// > with advanced website indexing.
    #[serde(default = "default_extractive_answer_count")]
    pub extractive_answer_count: i32,

    /// How many extractive segments to generate. Extractive segments are
    /// verbatim text from the document, longer than extractive answers.
    /// They will be used as context by the summarization LLM, and will NOT be
    /// returned to the client.
    ///
    /// From [Google's documentation](https://docs.cloud.google.com/generative-ai-app-builder/docs/snippets):
    ///
    /// > An extractive segment is verbatim text that is returned with each search
    /// > result. An extractive segment is usually more verbose than an extractive
    /// > answer. Extractive segments can be displayed as an answer to a query,
    /// > and can be used to perform post-processing tasks and as input for large
    /// > language models to generate answers or new text. Extractive segments are
    /// > available for data stores with unstructured data and with advanced
    /// > website indexing.
    #[serde(default = "default_extractive_segment_count")]
    pub extractive_segment_count: i32,
}

impl GoogleAgentSearchTool {
    /// The project this tool searches in, resolved as described on
    /// [`resolve_project_id`].
    fn project_id(&self) -> Result<String, FormaError> {
        resolve_project_id("GoogleAgentSearchTool", self.project_id.as_deref())
    }

    /// Builds the `:search` serving-config endpoint URL. Regional data stores
    /// are served from a `{location}-` host; the `global` location uses the
    /// bare host.
    fn search_url(&self, project_id: &str) -> String {
        let host = self.location.host();
        format!(
            "https://{host}/v1/projects/{}/locations/{}/collections/{}/dataStores/{}/servingConfigs/{}:search",
            project_id,
            self.location,
            DEFAULT_COLLECTION,
            self.datastore_id,
            DEFAULT_SERVING_CONFIG,
        )
    }

    fn content_search_spec(&self) -> Option<Value> {
        if !self.return_snippet
            && self.extractive_answer_count == 0
            && self.extractive_segment_count == 0
        {
            return None;
        }

        let mut spec = Map::new();
        if self.return_snippet {
            let mut aux = Map::new();
            aux.insert(
                "returnSnippet".to_string(),
                Value::Bool(self.return_snippet),
            );
            spec.insert("snippetSpec".to_string(), Value::Object(aux));
        }
        if self.extractive_answer_count > 0 || self.extractive_segment_count > 0 {
            let mut aux = Map::new();
            if self.extractive_answer_count > 0 {
                aux.insert(
                    "maxExtractiveAnswerCount".to_string(),
                    Value::Number(self.extractive_answer_count.into()),
                );
            }
            if self.extractive_segment_count > 0 {
                aux.insert(
                    "maxExtractiveSegmentCount".to_string(),
                    Value::Number(self.extractive_segment_count.into()),
                );
            }
            spec.insert("extractiveContentSpec".to_string(), Value::Object(aux));
        }

        Some(Value::Object(spec))
    }

    /// Builds the JSON request body. Only `query` is always present; the rest of
    /// the fields are included only when configured, so the API applies its
    /// defaults otherwise.
    fn request_body(&self, user_id: Arc<str>, query: &str) -> Value {
        let mut body = Map::new();
        body.insert("query".to_string(), Value::String(query.to_string()));
        if let Some(page_size) = self.page_size {
            body.insert("pageSize".to_string(), Value::from(page_size));
        }
        if let Some(boost) = &self.boost_spec {
            body.insert("boostSpec".to_string(), boost.clone());
        }
        if !self.page_categories.is_empty() {
            body.insert(
                "pageCategories".to_string(),
                Value::Array(
                    self.page_categories
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        // Google strongly recommends using this
        // field because it improves model performance
        // and personalization quality.
        body.insert("userPseudoId".to_string(), user_id.to_string().into());

        if let Some(spec) = self.content_search_spec() {
            body.insert("contentSearchSpec".to_string(), spec);
        }
        Value::Object(body)
    }

    /// Turns a `:search` response into the payload the summarizing LLM sees,
    /// emitting one `Reference` event per result on the way so the client can
    /// show what the answer was built from.
    ///
    /// The two audiences get different things, following the field docs above:
    /// extractive segments are too long for the client and never reach it,
    /// while snippets are previews rather than context and only reach the LLM
    /// when the result carries no extractive content at all.
    fn process_response(
        &self,
        body: Value,
        tx: &UnboundedSender<SSEvent>,
    ) -> Result<Value, FormaError> {
        let resp: SearchResponse = serde_json::from_value(body).map_err(|e| {
            FormaError::Serde(format!(
                "GoogleAgentSearchTool: could not parse the search response: {e}"
            ))
        })?;

        let mut results = Vec::with_capacity(resp.results.len());
        for result in resp.results {
            let DerivedStructData {
                title,
                link,
                snippets,
                extractive_answers,
                extractive_segments,
            } = result.document.derived;

            // Titles come from the document metadata and often carry the
            // line ending they were extracted with.
            let title = title.trim().to_string();

            // Failures are reported in-band: the `snippet` field then holds a
            // placeholder message instead of text from the document.
            let snippet = snippets
                .into_iter()
                .find(|s| s.snippet_status == SNIPPET_SUCCESS && !s.snippet.is_empty())
                .map(|s| s.snippet);

            // A result with neither a title nor a link is still worth
            // summarizing, but there is nothing for the client to cite.
            if !title.is_empty() || !link.is_empty() {
                tx.send(SSEvent::Reference {
                    title: title.clone(),
                    url: link.clone(),
                    snippet: snippet.clone(),
                    extractive_answer: if extractive_answers.is_empty() {
                        None
                    } else {
                        Some(
                            extractive_answers
                                .iter()
                                .map(|a| a.content.clone())
                                .collect(),
                        )
                    },
                })?;
            }

            let fallback_snippet = if extractive_answers.is_empty()
                && extractive_segments.is_empty()
            {
                info!(
                    title = %title,
                    link = %link,
                    has_snippet = snippet.is_some(),
                    "Vertex AI Search result has no extractive content; falling back to its snippet"
                );
                snippet
            } else {
                None
            };

            results.push(SearchResultEntry {
                title,
                link,
                extractive_answers,
                extractive_segments,
                snippet: fallback_snippet,
            });
        }

        serde_json::to_value(SearchOutput { results }).map_err(|e| {
            FormaError::Serde(format!(
                "GoogleAgentSearchTool: could not serialize the search results: {e}"
            ))
        })
    }
}

// ── the response we parse ─────────────────────────────────────────────────────
//
// Only the fields we use are modelled, and all of them are optional: which of
// them Google fills in depends on the `contentSearchSpec` we asked for, and on
// the data store's own contents.

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default)]
    document: Document,
}

#[derive(Debug, Default, Deserialize)]
struct Document {
    /// Everything Google derives at query time — the document's title and
    /// link, plus the snippets and extractive content we requested. Note that
    /// the envelope around it is camelCase while its own fields are not.
    #[serde(default, rename = "derivedStructData")]
    derived: DerivedStructData,
}

#[derive(Debug, Default, Deserialize)]
struct DerivedStructData {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    snippets: Vec<Snippet>,
    #[serde(default)]
    extractive_answers: Vec<ExtractiveChunk>,
    #[serde(default)]
    extractive_segments: Vec<ExtractiveChunk>,
}

#[derive(Debug, Deserialize)]
struct Snippet {
    #[serde(default)]
    snippet: String,
    #[serde(default, alias = "snippetStatus")]
    snippet_status: String,
}

/// A piece of verbatim document text — an extractive answer or an extractive
/// segment. Parsed from the response and handed to the LLM unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ExtractiveChunk {
    #[serde(default)]
    content: String,
    /// Only present for paginated documents (PDFs and the like). Google sends
    /// it as a string, but it is worth quoting back in an answer either way,
    /// so we pass through whatever we are given.
    #[serde(default, alias = "pageNumber", skip_serializing_if = "Option::is_none")]
    page_number: Option<Value>,
}

// ── the payload we return ─────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Serialize)]
struct SearchOutput {
    results: Vec<SearchResultEntry>,
}

/// One search result, shaped for an LLM writing an answer from it: where the
/// text came from, and the text itself.
#[derive(Debug, PartialEq, Serialize)]
struct SearchResultEntry {
    title: String,
    link: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extractive_answers: Vec<ExtractiveChunk>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extractive_segments: Vec<ExtractiveChunk>,
    /// Set only when the result carried neither answers nor segments, as a
    /// last resort so the result is not entirely opaque to the LLM.
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

impl TryFrom<GoogleAgentSearchTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_item: GoogleAgentSearchTool) -> Result<JsonSchema, FormaError> {
        let mut props = HashMap::new();
        props.insert(
            "query".to_string(),
            string_schema("the search query to use"),
        );
        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props),
            required: Some(vec!["query".to_string()]),
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for GoogleAgentSearchTool {
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
                "GoogleAgentSearchTool: 'project_id' must not be empty".to_string(),
            ));
        }
        if self.datastore_id.trim().is_empty() {
            return Err(FormaError::Input(
                "GoogleAgentSearchTool: 'datastore_id' must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let req: SearchRequest = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("GoogleAgentSearchTool: invalid input: {e}")))?;

        if req.query.is_empty() {
            return Err(FormaError::Input("Empty search query".to_string()));
        }

        let url = self.search_url(&self.project_id()?);
        let user_id = ctx.user_id.clone();
        let body = self.request_body(user_id, &req.query);

        #[cfg(not(feature = "airtight"))]
        {
            // The application-default credentials: in deployed machines they
            // come from the environment automatically; locally from
            // `gcloud auth application-default login`. Kept between calls,
            // with their token, like the client the request goes out on.
            use crate::tool::{google_auth, http};

            let auth_headers = google_auth::auth_headers(&[google_auth::CLOUD_PLATFORM_SCOPE])
                .await
                .map_err(|e| FormaError::ApiCall {
                    url: Some(url.clone()),
                    msg: e.to_string(),
                })?;

            info!(url = %url, query = req.query, "Vertex AI Search");
            let resp = http::client()
                .post(&url)
                .headers(auth_headers)
                .json(&body)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                warn!(url = %url, %status, error = %error_body, "Vertex AI Search returned error");
                return Err(FormaError::ApiCall {
                    url: Some(url),
                    msg: format!("HTTP {status}: {error_body}"),
                });
            }
            let body = resp
                .json::<Value>()
                .await
                .map_err(|e| FormaError::ApiCall {
                    url: Some(url),
                    msg: format!("Vertex AI Search returned a body that is not JSON: {e}"),
                })?;

            return Ok(Some(self.process_response(body, &ctx.tx)?));
        }
        #[cfg(feature = "airtight")]
        {
            // Airtight builds make no network calls.
            let _ = (url, body);
            Ok(Some(Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callable::CallContext;
    use db::NoneDB;
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock, mpsc};

    fn make_ctx() -> CallContext<NoneDB> {
        let (tx, _rx) = mpsc::unbounded_channel::<genai_client::SSEvent>();
        CallContext {
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
        }
    }

    fn tool(location: AgentSearchLocation) -> GoogleAgentSearchTool {
        GoogleAgentSearchTool {
            project_id: Some("my-project".to_string()),
            datastore_id: "my-datastore".to_string(),
            location,
            page_size: None,
            boost_spec: None,
            page_categories: vec![],
            return_snippet: false,
            extractive_answer_count: 0,
            extractive_segment_count: 1,
        }
    }

    // ── url ───────────────────────────────────────────────────────────────────

    #[test]
    fn url_global_uses_bare_host() {
        assert_eq!(
            tool(AgentSearchLocation::Global).search_url("my-project"),
            format!(
                "https://discoveryengine.googleapis.com/v1/projects/my-project/locations/global/collections/{DEFAULT_COLLECTION}/dataStores/my-datastore/servingConfigs/{DEFAULT_SERVING_CONFIG}:search"
            )
        );
    }

    #[test]
    fn url_regional_prefixes_host() {
        let t = tool(AgentSearchLocation::Eu);
        assert_eq!(
            t.search_url("my-project"),
            format!(
                "https://eu-discoveryengine.googleapis.com/v1/projects/my-project/locations/eu/collections/{DEFAULT_COLLECTION}/dataStores/my-datastore/servingConfigs/{DEFAULT_SERVING_CONFIG}:search"
            )
        );
    }

    // ── request body ──────────────────────────────────────────────────────────

    #[test]
    fn body_only_query_by_default() {
        let body = tool(AgentSearchLocation::default()).request_body("user".into(), "hello");
        assert_eq!(
            body,
            serde_json::json!({
                "query": "hello",
                "userPseudoId": "user",
                "contentSearchSpec": {
                    "extractiveContentSpec": { "maxExtractiveSegmentCount": 1 }
                },
            })
        );
    }

    #[test]
    fn body_includes_configured_fields() {
        let mut t = tool(AgentSearchLocation::default());
        t.page_size = Some(5);
        t.boost_spec = Some(serde_json::json!({ "conditionBoostSpecs": [] }));
        t.page_categories = vec!["Shoes".to_string(), "Boots".to_string()];
        let body = t.request_body("user".into(), "hello");
        assert_eq!(
            body,
            serde_json::json!({
                "query": "hello",
                "pageSize": 5,
                "boostSpec": { "conditionBoostSpecs": [] },
                "pageCategories": ["Shoes", "Boots"],
                "userPseudoId": "user",
                "contentSearchSpec": {
                    "extractiveContentSpec": { "maxExtractiveSegmentCount": 1 }
                },
            })
        );
    }

    // ── schema ────────────────────────────────────────────────────────────────

    #[test]
    fn schema_only_takes_query() {
        let schema = JsonSchema::try_from(tool(AgentSearchLocation::default())).unwrap();
        assert_eq!(schema.schema_type, Some(SchemaType::Object));
        let props = schema.properties.unwrap();
        assert_eq!(props["query"].schema_type, Some(SchemaType::String));
        assert_eq!(schema.required.unwrap(), vec!["query".to_string()]);
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn check_valid() {
        assert!(
            <GoogleAgentSearchTool as ToolTrait<NoneDB>>::check(&tool(
                AgentSearchLocation::default()
            ))
            .is_ok()
        );
    }

    #[test]
    fn check_empty_project_is_err() {
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = Some("  ".to_string());
        assert!(<GoogleAgentSearchTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    /// `check` runs when the agent is loaded, which may be long before — and
    /// on a different machine than — the call. An absent project is therefore
    /// not yet a problem: the environment gets its turn at call time.
    #[test]
    fn check_without_a_project_is_ok_even_with_no_environment() {
        let _env = crate::tool::ProjectEnvGuard::set(None);
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = None;
        assert!(<GoogleAgentSearchTool as ToolTrait<NoneDB>>::check(&t).is_ok());
    }

    #[test]
    fn check_empty_datastore_is_err() {
        let mut t = tool(AgentSearchLocation::default());
        t.datastore_id = String::new();
        assert!(<GoogleAgentSearchTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    // ── project resolution ────────────────────────────────────────────────

    #[test]
    fn a_stated_project_is_searched_and_the_environment_ignored() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("env-project"));
        let t = tool(AgentSearchLocation::default());
        assert_eq!(t.project_id().unwrap(), "my-project");
        assert!(
            t.search_url(&t.project_id().unwrap())
                .contains("/projects/my-project/")
        );
    }

    #[test]
    fn an_unstated_project_comes_from_the_environment_at_call_time() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("env-project"));
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = None;
        assert_eq!(t.project_id().unwrap(), "env-project");
        assert!(
            t.search_url(&t.project_id().unwrap())
                .contains("/projects/env-project/")
        );
    }

    /// The tool value is fixed once the agent is loaded, so the same one has
    /// to follow the environment it is called in.
    #[test]
    fn the_same_tool_follows_the_environment_between_calls() {
        let _env = crate::tool::ProjectEnvGuard::set(Some("first-project"));
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = None;
        assert_eq!(t.project_id().unwrap(), "first-project");

        crate::tool::set_or_unset(Some("second-project"));
        assert_eq!(t.project_id().unwrap(), "second-project");
    }

    /// Reported as a plain input error when the call happens, rather than by
    /// panicking somewhere in the load path.
    #[tokio::test]
    async fn execute_without_any_project_fails_with_a_helpful_error() {
        let _env = crate::tool::ProjectEnvGuard::set(None);
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = None;

        let err = t
            .execute(serde_json::json!({ "query": "hello" }), make_ctx())
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
        let mut t = tool(AgentSearchLocation::default());
        t.project_id = None;

        let json = serde_json::to_value(&t).expect("serialises");
        assert!(
            json.get("project_id").is_none(),
            "an absent project must not be written out: {json}"
        );

        let back: GoogleAgentSearchTool = serde_json::from_value(json).expect("deserialises");
        assert_eq!(back.project_id, None);
    }

    // ── execute (input validation) ─────────────────────────────────────────────

    #[tokio::test]
    async fn execute_non_object_input_is_err() {
        let result = tool(AgentSearchLocation::default())
            .execute(serde_json::json!("not an object"), make_ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_empty_query_is_err() {
        let result = tool(AgentSearchLocation::default())
            .execute(serde_json::json!({ "query": "" }), make_ctx())
            .await;
        assert!(result.is_err());
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_valid_returns_null_in_airtight() {
        let result = tool(AgentSearchLocation::default())
            .execute(serde_json::json!({ "query": "hello" }), make_ctx())
            .await;
        assert_eq!(result.unwrap(), Some(Value::Null));
    }

    // ── response processing ───────────────────────────────────────────────────

    /// Runs `process_response` and returns both what the LLM would see and the
    /// events the client would receive.
    fn process(body: Value) -> (Value, Vec<SSEvent>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<SSEvent>();
        let out = tool(AgentSearchLocation::default())
            .process_response(body, &tx)
            .expect("failed to process response");
        drop(tx);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        (out, events)
    }

    /// Pulls the fields out of a `Reference` event, panicking on anything else.
    fn reference(event: &SSEvent) -> (&str, &str, Option<&str>, Option<&Vec<String>>) {
        match event {
            SSEvent::Reference {
                title,
                url,
                snippet,
                extractive_answer,
            } => (title, url, snippet.as_deref(), extractive_answer.as_ref()),
            other => panic!("expected a Reference event, got {other:?}"),
        }
    }

    /// A response shaped like the real thing: unknown fields we ignore, a
    /// camelCase envelope around snake_case derived data, and a title carrying
    /// the line ending it was extracted with.
    fn sample_response() -> Value {
        serde_json::json!({
            "results": [{
                "id": "afad74c629b3748a44fe0bd63c50e8b6",
                "document": {
                    "name": "projects/1070808685962/locations/eu/collections/default_collection/dataStores/test/branches/0/documents/afad74c629b3748a44fe0bd63c50e8b6",
                    "id": "afad74c629b3748a44fe0bd63c50e8b6",
                    "derivedStructData": {
                        "extractive_segments": [
                            { "content": "7.2 Capability and capacity", "pageNumber": "23" },
                            { "content": "6.3 Digital communication", "pageNumber": "19" }
                        ],
                        "snippets": [
                            {
                                "snippet": "Teams cannot <b>communicate</b> effectively&nbsp;...",
                                "snippet_status": "SUCCESS"
                            }
                        ],
                        "extractive_answers": [
                            { "content": "Communication resources should be developed or acquired.", "pageNumber": "23" }
                        ],
                        "title": "GovS 011: Communication\r",
                        "link": "gs://sample-rag-data/GovS_011-_Communication_Version_2.1.pdf",
                        "can_fetch_raw_content": "true"
                    }
                },
                "rankSignals": { "defaultRank": 1 }
            }],
            "totalSize": 1,
            "attributionToken": "_QL0fAEKDAjeju3TBhDF",
            "summary": {},
            "semanticState": "ENABLED"
        })
    }

    #[test]
    fn response_keeps_answers_and_segments_for_the_llm() {
        let (out, _) = process(sample_response());
        assert_eq!(
            out,
            serde_json::json!({
                "results": [{
                    "title": "GovS 011: Communication",
                    "link": "gs://sample-rag-data/GovS_011-_Communication_Version_2.1.pdf",
                    "extractive_answers": [
                        { "content": "Communication resources should be developed or acquired.", "page_number": "23" }
                    ],
                    "extractive_segments": [
                        { "content": "7.2 Capability and capacity", "page_number": "23" },
                        { "content": "6.3 Digital communication", "page_number": "19" }
                    ],
                }]
            })
        );
    }

    #[test]
    fn response_emits_a_reference_per_result() {
        let (_, events) = process(sample_response());
        assert_eq!(events.len(), 1);
        let (title, url, snippet, answers) = reference(&events[0]);
        assert_eq!(title, "GovS 011: Communication");
        assert_eq!(
            url,
            "gs://sample-rag-data/GovS_011-_Communication_Version_2.1.pdf"
        );
        assert_eq!(
            snippet,
            Some("Teams cannot <b>communicate</b> effectively&nbsp;...")
        );
        assert_eq!(
            answers,
            Some(&vec![
                "Communication resources should be developed or acquired.".to_string()
            ])
        );
    }

    /// Extractive segments are long, and only exist to be summarized — the
    /// client is not meant to render them.
    #[test]
    fn reference_carries_no_segments() {
        let mut body = sample_response();
        body["results"][0]["document"]["derivedStructData"]["extractive_answers"] =
            serde_json::json!([]);
        let (_, events) = process(body);
        let (.., answers) = reference(&events[0]);
        assert_eq!(answers, None);
    }

    #[test]
    fn result_without_extractive_content_falls_back_to_its_snippet() {
        let mut body = sample_response();
        let derived = &mut body["results"][0]["document"]["derivedStructData"];
        derived["extractive_answers"] = serde_json::json!([]);
        derived["extractive_segments"] = serde_json::json!([]);

        let (out, _) = process(body);
        assert_eq!(
            out["results"][0]["snippet"],
            serde_json::json!("Teams cannot <b>communicate</b> effectively&nbsp;...")
        );
    }

    /// A snippet Google could not build is reported in-band, with a
    /// placeholder in the `snippet` field. It is not text from the document.
    #[test]
    fn unsuccessful_snippets_are_dropped() {
        let mut body = sample_response();
        let derived = &mut body["results"][0]["document"]["derivedStructData"];
        derived["extractive_answers"] = serde_json::json!([]);
        derived["extractive_segments"] = serde_json::json!([]);
        derived["snippets"] = serde_json::json!([{
            "snippet": "No snippet is available for this page.",
            "snippet_status": "NO_SNIPPET_AVAILABLE"
        }]);

        let (out, events) = process(body);
        assert_eq!(out["results"][0].get("snippet"), None);
        let (.., snippet, _) = reference(&events[0]);
        assert_eq!(snippet, None);
    }

    /// The snippet is a preview for the client, not context: a result that has
    /// extractive content keeps it out of what the LLM reads.
    #[test]
    fn snippet_is_not_sent_to_the_llm_alongside_extractive_content() {
        let (out, _) = process(sample_response());
        assert_eq!(out["results"][0].get("snippet"), None);
    }

    /// There is nothing to cite without a title or a link, but the text is
    /// still worth summarizing.
    #[test]
    fn result_without_title_or_link_is_kept_but_not_referenced() {
        let mut body = sample_response();
        let derived = &mut body["results"][0]["document"]["derivedStructData"];
        derived["title"] = serde_json::json!("");
        derived["link"] = serde_json::json!("");

        let (out, events) = process(body);
        assert!(events.is_empty());
        assert_eq!(out["results"].as_array().map(|r| r.len()), Some(1));
    }

    #[test]
    fn response_without_results_is_empty() {
        let (out, events) = process(serde_json::json!({ "totalSize": 0, "summary": {} }));
        assert_eq!(out, serde_json::json!({ "results": [] }));
        assert!(events.is_empty());
    }

    #[test]
    fn unparseable_response_is_err() {
        let (tx, _rx) = mpsc::unbounded_channel::<SSEvent>();
        let result = tool(AgentSearchLocation::default()).process_response(Value::Null, &tx);
        assert!(matches!(result, Err(FormaError::Serde(_))));
    }
}
