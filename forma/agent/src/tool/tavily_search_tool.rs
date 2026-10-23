// Tool based on https://docs.tavily.com/documentation/api-reference/endpoint/search

use std::collections::HashMap;
#[cfg(not(feature = "airtight"))]
use tracing::{info, warn};

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CallContext, ToolTrait,
    tool::mailgun_tool::{string_array_schema, string_schema},
};

const API_KEY_ENV: &str = "TAVILY_API_KEY";
const SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";

/// The single argument the LLM fills in for a search tool: the query string.
/// Shared by the Tavily and Google (Vertex AI Search) search tools.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SearchRequest {
    pub(crate) query: String,
}

/// The kind of search Tavily runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TavilyTopic {
    /// Broad web search.
    General,
    /// Recent news: politics, sports, current events.
    News,
}

/// Searches the web using Tavily.
///
/// Every field is optional. A field that is set is locked: it is left out
/// of the schema the LLM sees and is always sent as configured. A field that
/// is not set is offered to the LLM as an argument.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TavilySearchTool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<TavilyTopic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_domains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_domains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_n_days: Option<u32>,
}

/// What the LLM may send. Mirrors the schema produced by
/// `TryFrom<TavilySearchTool> for JsonSchema`; values for locked fields are
/// ignored.
#[derive(Clone, Debug, Deserialize)]
struct TavilySearchArgs {
    query: String,
    #[serde(default)]
    exact_match: bool,
    #[serde(default)]
    topic: Option<TavilyTopic>,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    language: Option<String>,
    /// A float, because models sometimes send whole numbers as `7.0`.
    #[serde(default)]
    last_n_days: Option<f64>,
}

/// The body posted to Tavily.
#[derive(Debug, Serialize, PartialEq)]
struct TavilyRequestBody {
    query: String,
    topic: TavilyTopic,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    exact_match: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    filter_by_published_date: bool,
    include_favicon: bool,
    include_usage: bool,
}

fn integer_schema(description: &str) -> JsonSchema {
    JsonSchema {
        schema_type: Some(SchemaType::Integer),
        description: Some(description.to_string()),
        ..Default::default()
    }
}

fn check_last_n_days(n: u32) -> Result<(), FormaError> {
    if n == 0 {
        return Err(FormaError::Input(
            "Tavily search: 'last_n_days' must be at least 1".to_string(),
        ));
    }
    Ok(())
}

impl TavilySearchTool {
    /// Merges the LLM's arguments with the locked configuration into the body
    /// sent to Tavily. `today` is passed in so tests do not depend on the clock.
    fn build_body(
        &self,
        args: TavilySearchArgs,
        today: NaiveDate,
    ) -> Result<TavilyRequestBody, FormaError> {
        if args.query.trim().is_empty() {
            return Err(FormaError::Input("Empty search query".to_string()));
        }

        let topic = self.topic.or(args.topic).unwrap_or(TavilyTopic::General);
        // Tavily only honours `country` on general searches.
        let country = match topic {
            TavilyTopic::General => self.country.clone().or(args.country),
            TavilyTopic::News => None,
        };

        // Dates are counted back from today in UTC. Tavily reads dates as
        // calendar days, so one day of slack either way is harmless and
        // sidesteps knowing the user's time zone.
        // A part of a day still counts as a day, so the model's value rounds
        // up. Negative values saturate to 0 and are refused below.
        let last_n_days = self
            .last_n_days
            .or(args.last_n_days.map(|n| n.ceil() as u32));
        let start_date = match last_n_days {
            Some(n) => {
                check_last_n_days(n)?;
                let start = today - chrono::Days::new(n as u64);
                Some(start.format("%Y-%m-%d").to_string())
            }
            None => None,
        };

        Ok(TavilyRequestBody {
            query: args.query,
            topic,
            exact_match: args.exact_match,
            include_domains: self
                .include_domains
                .clone()
                .or(args.include_domains)
                .unwrap_or_default(),
            exclude_domains: self
                .exclude_domains
                .clone()
                .or(args.exclude_domains)
                .unwrap_or_default(),
            country,
            language: self.language.clone().or(args.language),
            filter_by_published_date: start_date.is_some(),
            start_date,
            include_favicon: true,
            include_usage: true,
        })
    }
}

impl TryFrom<TavilySearchTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: TavilySearchTool) -> Result<JsonSchema, FormaError> {
        let mut props = HashMap::new();
        props.insert(
            "query".to_string(),
            string_schema(
                "What to search for, in plain words. Search operators such as quotes or 'site:' are not supported; use the other arguments instead.",
            ),
        );
        props.insert(
            "exact_match".to_string(),
            JsonSchema {
                schema_type: Some(SchemaType::Boolean),
                description: Some(
                    "If true, only return results containing the exact phrase(s) written in quotes inside the query.".to_string(),
                ),
                ..Default::default()
            },
        );

        // Locked fields are not offered to the LLM.
        if item.topic.is_none() {
            props.insert(
                "topic".to_string(),
                JsonSchema {
                    schema_type: Some(SchemaType::String),
                    description: Some(
                        "'news' for recent news (politics, sports, current events); 'general' otherwise. Defaults to 'general'.".to_string(),
                    ),
                    r#enum: Some(vec!["general".to_string(), "news".to_string()]),
                    ..Default::default()
                },
            );
        }
        if item.include_domains.is_none() {
            props.insert(
                "include_domains".to_string(),
                string_array_schema(
                    "Only return results from these domains (e.g., 'example.com').",
                ),
            );
        }
        if item.exclude_domains.is_none() {
            props.insert(
                "exclude_domains".to_string(),
                string_array_schema("Never return results from these domains."),
            );
        }
        if item.country.is_none() {
            props.insert(
                "country".to_string(),
                string_schema(
                    "Favour results from this country, written in lowercase English (e.g., 'chile', 'united states'). Ignored when topic is 'news'.",
                ),
            );
        }
        if item.language.is_none() {
            props.insert(
                "language".to_string(),
                string_schema(
                    "Favour results in this language, as an ISO 639-1 code (e.g., 'es', 'en').",
                ),
            );
        }
        if item.last_n_days.is_none() {
            props.insert(
                "last_n_days".to_string(),
                integer_schema("Only return results published in the last N days (at least 1)."),
            );
        }

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
impl<DB: FormaDB> ToolTrait<DB> for TavilySearchTool {
    fn check(&self) -> Result<(), FormaError> {
        if let Some(n) = self.last_n_days {
            check_last_n_days(n)?;
        }
        if self.topic == Some(TavilyTopic::News) && self.country.is_some() {
            return Err(FormaError::Input(
                "Tavily search: 'country' only works with topic 'general'".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let args: TavilySearchArgs = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("Search tool: invalid input: {e}")))?;
        let body = self.build_body(args, Utc::now().date_naive())?;

        let _api_key = std::env::var(API_KEY_ENV).map_err(|e| FormaError::MissingEnvVar {
            key: API_KEY_ENV.to_string(),
            msg: e.to_string(),
        })?;

        #[cfg(not(feature = "airtight"))]
        {
            info!(url = %SEARCH_ENDPOINT, query = body.query, "Tavily search");
            let resp = crate::tool::http::client()
                .post(SEARCH_ENDPOINT.to_string())
                .header("Authorization", format!("Bearer {_api_key}"))
                .json(&body)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                warn!(url = %SEARCH_ENDPOINT, %status, error = %error_body, "Tavily search returned error");
                return Err(FormaError::ApiCall {
                    url: Some(SEARCH_ENDPOINT.to_string()),
                    msg: format!("HTTP {status}: {error_body}"),
                });
            }
            let body = resp.json::<Value>().await.unwrap_or(Value::Null);
            return Ok(Some(body));
        }
        #[cfg(feature = "airtight")]
        {
            // Airtight builds make no network calls.
            let _ = body;
            Ok(Some(Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 29).unwrap()
    }

    fn args(v: Value) -> TavilySearchArgs {
        serde_json::from_value(v).expect("valid args")
    }

    fn schema_props(tool: TavilySearchTool) -> Vec<String> {
        let schema: JsonSchema = tool.try_into().unwrap();
        let mut keys: Vec<String> = schema.properties.unwrap().into_keys().collect();
        keys.sort();
        keys
    }

    #[test]
    fn an_unconfigured_tool_offers_every_argument() {
        assert_eq!(
            schema_props(TavilySearchTool::default()),
            vec![
                "country",
                "exact_match",
                "exclude_domains",
                "include_domains",
                "language",
                "last_n_days",
                "query",
                "topic"
            ]
        );
    }

    #[test]
    fn locked_fields_are_hidden_from_the_llm() {
        let tool = TavilySearchTool {
            include_domains: Some(vec!["emol.com".into()]),
            last_n_days: Some(2),
            ..Default::default()
        };
        let props = schema_props(tool);
        assert!(!props.contains(&"include_domains".to_string()));
        assert!(!props.contains(&"last_n_days".to_string()));
        assert!(props.contains(&"exclude_domains".to_string()));
    }

    #[test]
    fn locked_values_win_over_the_llm() {
        let tool = TavilySearchTool {
            include_domains: Some(vec!["emol.com".into()]),
            last_n_days: Some(2),
            ..Default::default()
        };
        let body = tool
            .build_body(
                args(json!({
                    "query": "litio",
                    "include_domains": ["evil.com"],
                    "last_n_days": 365
                })),
                today(),
            )
            .unwrap();
        assert_eq!(body.include_domains, vec!["emol.com".to_string()]);
        assert_eq!(body.start_date.as_deref(), Some("2026-09-27"));
        assert!(body.filter_by_published_date);
    }

    #[test]
    fn a_bare_query_sends_only_the_defaults() {
        let body = TavilySearchTool::default()
            .build_body(args(json!({"query": "litio"})), today())
            .unwrap();
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({
                "query": "litio",
                "topic": "general",
                "include_favicon": true,
                "include_usage": true
            })
        );
    }

    #[test]
    fn country_is_dropped_on_news_searches() {
        let body = TavilySearchTool::default()
            .build_body(
                args(json!({"query": "litio", "topic": "news", "country": "chile"})),
                today(),
            )
            .unwrap();
        assert_eq!(body.country, None);
    }

    #[test]
    fn days_sent_as_floats_round_up() {
        let tool = TavilySearchTool::default();
        let whole = tool
            .build_body(args(json!({"query": "x", "last_n_days": 2.0})), today())
            .unwrap();
        let partial = tool
            .build_body(args(json!({"query": "x", "last_n_days": 1.2})), today())
            .unwrap();
        assert_eq!(whole.start_date.as_deref(), Some("2026-09-27"));
        assert_eq!(partial.start_date, whole.start_date);
        assert!(
            tool.build_body(args(json!({"query": "x", "last_n_days": -3.0})), today())
                .is_err()
        );
    }

    #[test]
    fn zero_days_and_empty_queries_are_rejected() {
        let tool = TavilySearchTool::default();
        assert!(
            tool.build_body(args(json!({"query": "x", "last_n_days": 0})), today())
                .is_err()
        );
        assert!(
            tool.build_body(args(json!({"query": "  "})), today())
                .is_err()
        );
    }

    #[test]
    fn check_rejects_contradictory_configuration() {
        let tool = TavilySearchTool {
            topic: Some(TavilyTopic::News),
            country: Some("chile".into()),
            ..Default::default()
        };
        assert!(<TavilySearchTool as ToolTrait<db::NoneDB>>::check(&tool).is_err());
        let tool = TavilySearchTool {
            last_n_days: Some(0),
            ..Default::default()
        };
        assert!(<TavilySearchTool as ToolTrait<db::NoneDB>>::check(&tool).is_err());
    }
}
