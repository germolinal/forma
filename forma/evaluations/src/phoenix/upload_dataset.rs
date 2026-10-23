use crate::dataset::{DataPoint, Dataset, PostDatasetAction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PhoenixValue {
    pub(crate) value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PhoenixDataset {
    pub(crate) action: PostDatasetAction,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) inputs: Vec<PhoenixValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) outputs: Vec<PhoenixValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) metadata: Vec<Value>,
}

impl From<PhoenixDataset> for Dataset {
    fn from(dataset: PhoenixDataset) -> Self {
        let inputs = dataset.inputs.clone();
        let outputs = dataset.outputs.clone();

        let data = inputs
            .into_iter()
            .zip(outputs)
            .map(|(input, output)| {
                let input = input.value;
                // Check that this is an object, and that it is empty.
                let output = match output.value.as_object() {
                    Some(obj) => {
                        if obj.is_empty() {
                            None
                        } else {
                            Some(output.value)
                        }
                    }
                    None => None,
                };
                DataPoint {
                    id: None,
                    input,
                    expected_output: output,
                }
            })
            .collect();

        Self {
            name: dataset.name.clone(),
            description: dataset.description.clone(),
            metadata: dataset.metadata.clone(),
            data,
        }
    }
}

impl From<Dataset> for PhoenixDataset {
    fn from(dataset: Dataset) -> Self {
        let inputs = dataset
            .data
            .iter()
            .map(|x| PhoenixValue {
                value: x.input.clone(),
            })
            .collect();
        let outputs = dataset
            .data
            .into_iter()
            .map(|x| {
                let value = match x.expected_output {
                    Some(v) => v,
                    None => serde_json::json!({}),
                };
                PhoenixValue { value }
            })
            .collect();

        Self {
            action: Default::default(),
            name: dataset.name.clone(),
            description: dataset.description.clone(),
            metadata: dataset.metadata.clone(),
            inputs,
            outputs,
        }
    }
}
