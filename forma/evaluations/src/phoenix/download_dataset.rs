use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    dataset::{DataPoint, Dataset},
    phoenix::upload_dataset::PhoenixValue,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PhoenixExample {
    id: String,
    input: PhoenixValue,
    output: PhoenixValue,
    metadata: Value,
    updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DownloadDatasetResponse {
    pub(crate) dataset_id: String,
    pub(crate) version_id: String,
    pub(crate) examples: Vec<PhoenixExample>,
}

impl From<DownloadDatasetResponse> for Dataset {
    fn from(value: DownloadDatasetResponse) -> Self {
        let data = value
            .examples
            .into_iter()
            .map(|e| DataPoint {
                id: Some(e.id),
                input: e.input.value,
                expected_output: Some(e.output.value),
            })
            .collect();
        Self {
            name: value.dataset_id,
            description: None,
            data,
            metadata: vec![json!({})],
        }
    }
}
