use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Debug, Clone, Serialize, Deserialize, Copy, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PostDatasetAction {
    #[default]
    Create,
    Append,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_output: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Dataset {
    pub name: String,
    pub description: Option<String>,
    pub data: Vec<DataPoint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub metadata: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
    pub example_count: usize,
}

/// A dataset that the platform knows about, but cannot hand out (e.g., a
/// file that does not parse).
///
/// These are reported separately from `list_datasets` so that one broken
/// dataset does not hide every other one, while still being visible.
#[derive(Debug, Clone)]
pub struct BrokenDataset {
    /// The name one would use to ask for it—the file name, for platforms
    /// that keep their datasets in files
    pub id: String,
    /// Why it cannot be read. Names the dataset it refers to, so it can be
    /// printed on its own.
    pub error: String,
}
