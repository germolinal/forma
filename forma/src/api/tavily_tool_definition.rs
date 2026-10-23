use agent::tool::tavily_search_tool::{TavilySearchTool, TavilyTopic};
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// A tool for searching the web using Tavily.
///
/// Every field is optional. A field you set is locked: the LLM does not see
/// it and every search uses your value. A field you leave out becomes an
/// argument the LLM may fill in on each search.
///
/// ```yaml
/// type: tavily-search
/// topic: news
/// include_domains: [emol.com, latercera.com]
/// last_n_days: 2
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, FormaDocs)]
pub struct TavilyWebSearchDefinition {
    /// Either `general` (broad web search) or `news` (recent news:
    /// politics, sports, current events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<TavilyTopic>,

    /// Only return results from these domains (e.g., `emol.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_domains: Option<Vec<String>>,

    /// Never return results from these domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_domains: Option<Vec<String>>,

    /// Favour results from this country, written in lowercase English
    /// (e.g., `chile`). Only works when `topic` is `general`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,

    /// Favour results in this language, as an ISO 639-1 code (e.g., `es`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Only return results published in the last N days, counted from today
    /// in UTC. Must be at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_n_days: Option<u32>,
}

impl ObjectDefinition for TavilyWebSearchDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<TavilyWebSearchDefinition> for TavilySearchTool {
    type Error = FormaError;

    fn try_from(value: TavilyWebSearchDefinition) -> Result<Self, Self::Error> {
        Ok(TavilySearchTool {
            topic: value.topic,
            include_domains: value.include_domains,
            exclude_domains: value.exclude_domains,
            country: value.country,
            language: value.language,
            last_n_days: value.last_n_days,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_definition_still_parses() {
        let def: TavilyWebSearchDefinition = serde_yaml::from_str("{}").expect("parses");
        let tool: TavilySearchTool = def.clone().try_into().unwrap();
        assert!(tool.topic.is_none() && tool.last_n_days.is_none());
        assert_eq!(serde_yaml::to_string(&def).unwrap().trim(), "{}");
    }

    #[test]
    fn the_yaml_an_agent_author_writes_round_trips() {
        let def: TavilyWebSearchDefinition = serde_yaml::from_str(
            r#"
topic: news
include_domains: [emol.com, latercera.com]
last_n_days: 2
"#,
        )
        .expect("parses");
        let tool: TavilySearchTool = def.try_into().unwrap();
        assert_eq!(tool.topic, Some(TavilyTopic::News));
        assert_eq!(
            tool.include_domains,
            Some(vec!["emol.com".to_string(), "latercera.com".to_string()])
        );
        assert_eq!(tool.last_n_days, Some(2));
        assert!(tool.country.is_none());
    }
}
