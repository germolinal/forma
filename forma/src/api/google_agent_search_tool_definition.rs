use agent::tool::google_agent_search_tool::{
    AgentSearchLocation, GoogleAgentSearchTool, default_extractive_answer_count,
    default_extractive_segment_count, default_show_snippet,
};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::ObjectDefinition;

/// Searches a Google [Agent Search](https://docs.cloud.google.com/generative-ai-app-builder/docs) (aka, Vertex AI Search, Agent Builder, Discovery Engine) data
/// store. It would seem that this datastore needs to
///
/// 1. Be attached to a Custom Search App
/// 2. You need to enable enterprise and genai features.
///
/// Authentication uses Google Cloud application-default credentials, which are
/// injected at call time — no API key env var is required. The only argument
/// the LLM supplies is the search `query`.
///
/// > **NOTES**: The caller needs permission to `discoveryengine.servingConfigs.search`
/// > (`roles/discoveryengine.viewer` should do).
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
pub struct GoogleAgentSearchDefinition {
    /// The Google Cloud project that owns the data store / search app.
    ///
    /// Optional: when it is not given, the project named by the
    /// `GOOGLE_CLOUD_PROJECT` environment variable of whichever machine runs
    /// the agent is used instead, resolved when the tool is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,

    /// The ID of the data store to search. Note that this
    /// does not always match the name you gave it
    pub datastore_id: String,

    /// The location of the data store. Defaults to `global`; regional data
    /// stores use e.g. `us` or `eu`.
    #[serde(default)]
    pub location: AgentSearchLocation,

    /// Maximum number of results to return. When omitted, the API default is
    /// used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i32>,

    /// Optional boost specification, passed through verbatim as the request's
    /// `boostSpec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_spec: Option<Value>,

    /// Optional list of page categories to restrict the search to.
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

impl ObjectDefinition for GoogleAgentSearchDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<GoogleAgentSearchDefinition> for GoogleAgentSearchTool {
    type Error = FormaError;

    fn try_from(value: GoogleAgentSearchDefinition) -> Result<Self, Self::Error> {
        // The project is passed through as-is, unresolved: reading the
        // environment here would tie the compiled agent to the machine that
        // compiled it. The tool asks for it when it is actually called.
        Ok(GoogleAgentSearchTool {
            project_id: value.project_id,
            datastore_id: value.datastore_id,
            location: value.location,
            page_size: value.page_size,
            boost_spec: value.boost_spec,
            page_categories: value.page_categories,
            return_snippet: value.return_snippet,
            extractive_answer_count: value.extractive_answer_count,
            extractive_segment_count: value.extractive_segment_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ProjectEnvGuard;
    use agent::ToolTrait;

    /// The whole point: an agent file that names no project is still a valid
    /// agent file on a machine that has no project set. Nothing in the load
    /// path may reach for the environment — the tool does that when it runs.
    #[test]
    fn a_definition_without_a_project_needs_no_environment() {
        let _env = ProjectEnvGuard::set(None);

        let def: GoogleAgentSearchDefinition = serde_yaml::from_str("datastore_id: my-datastore")
            .expect("a definition with no project must still parse");
        assert_eq!(def.project_id, None);

        let tool: GoogleAgentSearchTool = def.try_into().expect("converts without an environment");
        assert_eq!(
            tool.project_id, None,
            "the project must stay unresolved until the tool is called"
        );
        assert!(
            <GoogleAgentSearchTool as ToolTrait<db::NoneDB>>::check(&tool).is_ok(),
            "a project supplied at run time is not a load-time error"
        );
    }

    /// Present in the environment but absent from the file: the definition
    /// must still not bake it in, or the compiled agent would carry whichever
    /// project happened to be set on the machine that compiled it.
    #[test]
    fn the_environment_does_not_leak_into_the_definition() {
        let _env = ProjectEnvGuard::set(Some("env-project"));

        let def: GoogleAgentSearchDefinition =
            serde_yaml::from_str("datastore_id: my-datastore").expect("parses");
        assert_eq!(def.project_id, None);

        let tool: GoogleAgentSearchTool = def.try_into().expect("converts");
        assert_eq!(tool.project_id, None);
    }

    /// A project the author did write is carried through untouched, whatever
    /// the environment says.
    #[test]
    fn a_stated_project_survives_the_conversion() {
        let _env = ProjectEnvGuard::set(Some("env-project"));

        let def: GoogleAgentSearchDefinition = serde_yaml::from_str(
            r#"
project_id: some-project
datastore_id: my-datastore
"#,
        )
        .expect("parses");
        assert_eq!(def.project_id.as_deref(), Some("some-project"));

        let tool: GoogleAgentSearchTool = def.try_into().expect("converts");
        assert_eq!(tool.project_id.as_deref(), Some("some-project"));
    }

    #[test]
    fn an_unstated_project_is_not_serialised_back_out() {
        let def: GoogleAgentSearchDefinition =
            serde_yaml::from_str("datastore_id: my-datastore").expect("parses");
        let yaml = serde_yaml::to_string(&def).expect("serialises");
        assert!(
            !yaml.contains("project_id"),
            "an absent project must not reappear on the way out: {yaml}"
        );
    }

    /// The rest of the definition still fills in from its own defaults when
    /// only the required field is given.
    #[test]
    fn the_yaml_an_agent_author_writes_round_trips() {
        let def: GoogleAgentSearchDefinition = serde_yaml::from_str(
            r#"
datastore_id: my-datastore
location: eu
page_size: 5
"#,
        )
        .expect("parses");

        assert_eq!(def.datastore_id, "my-datastore");
        assert_eq!(def.location, AgentSearchLocation::Eu);
        assert_eq!(def.page_size, Some(5));
        assert!(!def.return_snippet);
        assert_eq!(
            def.extractive_answer_count,
            default_extractive_answer_count()
        );
        assert_eq!(
            def.extractive_segment_count,
            default_extractive_segment_count()
        );
    }
}
