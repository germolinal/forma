use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    id: String,
    dataset_id: String,
    // dataset_version_id: String,
    // repetitions: usize ??
    metadata: Value,
    project_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    created_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    updated_at: String,
}
