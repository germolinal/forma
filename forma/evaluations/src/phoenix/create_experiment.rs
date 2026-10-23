use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CreateExperimentRequest {
    pub name: String,
    pub description: Option<String>,
    pub metadata: Value,
    pub repetitions: i32,
}

// #[derive(Debug, Clone, Serialize, Deserialize, Default)]
// pub(crate) struct ExperimentSummary {
//     pub(crate) id: String,
//     pub(crate) dataset_id: String,
//     pub(crate) dataset_version_id: String,
//     pub(crate) repetitions: usize,
//     pub(crate) metadata: Value,
//     pub(crate) project_name: String,
//     pub(crate) created_at: String,
//     pub(crate) updated_at: String,
// }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CreateExperimentResponse {
    pub id: String,
    pub dataset_id: String,
    pub dataset_version_id: String,
    pub repetitions: i32,
    pub metadata: Value,
    pub project_name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod integration_test {
    use crate::{
        EvalPlatform,
        dataset::{DataPoint, Dataset},
        phoenix::PhoenixClient,
    };

    use tokio;

    #[tokio::test]
    async fn test_create_experiment() -> Result<(), String> {
        let phoenix = PhoenixClient::new();
        // Experiments need a dataset
        let dataset_name = uuid::Uuid::new_v4().to_string();
        let dataset = Dataset {
            name: dataset_name.clone(),
            description: Some("A test dataset".into()),
            data: vec![
                DataPoint {
                    id: None,
                    input: "Hi".into(),
                    expected_output: None,
                },
                DataPoint {
                    id: None,
                    input: "Hi".into(),
                    expected_output: Some("How are you?".into()),
                },
            ],
            metadata: vec![],
        };
        phoenix
            .create_dataset(dataset, crate::dataset::PostDatasetAction::Create)
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Now we can test
        let id = phoenix
            .create_experiment(
                &dataset_name,
                "new experiment".to_string(),
                Some("based on commit #098as".to_string()),
            )
            .await?;
        println!("Created experiment with ID: {}", id);
        Ok(())
    }
}
