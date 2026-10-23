use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Annotator {
    #[default]
    LLM,
    CODE,
    HUMAN,
}

/// The result of a judgement, whether it is made by
/// an LLM or not.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvalResult {
    /// The verbal equivalent of a score (e.g., Good, Bad, OK)
    pub label: String,
    /// The score of
    pub score: f64,
    /// An explanation for the given score
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Judgement {
    pub experiment_run_id: String,
    /// Name of the evaluation
    pub name: String,
    pub annotator_kind: Annotator,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<EvalResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// The mean score the metric that produced this judgement is required
    /// to reach, if it declares one.
    ///
    /// This travels with the judgement so that the platform storing it can
    /// report a metric against its own requirement. It is deliberately not
    /// serialised: it is not part of any evaluation platform's API, and
    /// `Judgement` is posted verbatim to Phoenix.
    #[serde(skip)]
    pub required_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActualAnswer {
    pub dataset_example_id: String,
    pub output: Value,
    pub repetition_number: i32,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
