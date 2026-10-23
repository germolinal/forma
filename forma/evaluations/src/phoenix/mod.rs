pub(crate) mod create_evaluation;
pub(crate) mod download_dataset;
use async_trait::async_trait;
use create_evaluation::CreateEvalResultResponse;
pub(crate) mod create_experiment;
use create_experiment::{CreateExperimentRequest, CreateExperimentResponse};
pub(crate) mod create_run;

pub(crate) mod upload_dataset;
use tokio::time::sleep;
use upload_dataset::PhoenixDataset;

use reqwest::{self, Client, Response};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Debug, time::Duration};

use crate::{
    EvalPlatform,
    dataset::{BrokenDataset, Dataset, DatasetSummary, PostDatasetAction},
    judgement::{ActualAnswer, Judgement},
    phoenix::{create_run::CreateRunResponse, download_dataset::DownloadDatasetResponse},
};

/// Creates a new Phoenix client, for evaluation purposes.
/// The endpoint of the Phoenix server is defined by the
/// `PHOENIX_EVAL_ENDPOINT` environment variable. Defaults
/// to `http://localhost:6006`
pub struct PhoenixClient {
    endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ResponseWrapper<T> {
    data: T,
}

impl Default for PhoenixClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PhoenixClient {
    async fn send_post_request<REQ: Serialize>(
        &self,
        path: &str,
        req: &REQ,
    ) -> Result<Response, String> {
        let client = Client::new();
        let url = format!("{}/{}", self.endpoint, path);
        client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("Failed to send POST request to Phoenix: {e}"))
    }

    async fn send_get_request(&self, path: &str) -> Result<Response, String> {
        let client = Client::new();
        let url = format!("{}/{}", self.endpoint, path);
        client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to send GET request to Phoenix: {e}"))
    }

    async fn phoenix_error<RES>(&self, res: Response) -> Result<RES, String> {
        let status = res.status();
        let err = res.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(format!("Error from Phoenix [{status}]: {err}"))
    }

    async fn parse_response<RES: for<'a> Deserialize<'a>>(
        &self,
        res: Response,
    ) -> Result<RES, String> {
        if res.status().is_success() {
            let bytes = res
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Phoenix response bytes: {e}"))?;
            let wrapper: ResponseWrapper<RES> = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Failed to parse Phoenix response: {e}"))?;
            Ok(wrapper.data)
        } else {
            self.phoenix_error(res).await
        }
    }

    async fn post<REQ: Serialize, RES: for<'a> Deserialize<'a>>(
        &self,
        path: &str,
        req: &REQ,
    ) -> Result<RES, String> {
        let res = self.send_post_request(path, req).await?;
        self.parse_response(res).await
    }

    async fn get<RES: for<'a> Deserialize<'a>>(&self, path: &str) -> Result<RES, String> {
        let res = self.send_get_request(path).await?;
        self.parse_response(res).await
    }

    async fn post_no_response<REQ: Serialize>(&self, path: &str, req: &REQ) -> Result<(), String> {
        let res = self.send_post_request(path, req).await?;
        if res.status().is_success() {
            Ok(())
        } else {
            self.phoenix_error(res).await
        }
    }
}

#[async_trait]
impl EvalPlatform for PhoenixClient {
    fn new() -> Self {
        let endpoint = std::env::var("PHOENIX_EVAL_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:6006".to_string());
        Self { endpoint }
    }

    /// Phoenix has its own way of showing an experiment's results, and no
    /// notion of a metric that must reach a number. The requirements are
    /// checked by the runner either way.
    async fn set_requirements(
        &self,
        _experiment_id: &str,
        _requirements: &BTreeMap<String, f64>,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn push_judgement(&self, req: &Judgement) -> Result<String, String> {
        let path = "v1/experiment_evaluations";
        let res: CreateEvalResultResponse = self.post(path, req).await?;
        Ok(res.id)
    }

    async fn push_actual_answer(
        &self,
        experiment_id: &str,
        req: &ActualAnswer,
    ) -> Result<String, String> {
        let path = format!("v1/experiments/{experiment_id}/runs");
        let ret: CreateRunResponse = self.post(&path, req).await?;
        Ok(ret.id)
    }

    async fn list_datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        self.get("v1/datasets").await
    }
    /// Phoenix hands out its datasets over an API: whatever it returns is,
    /// by definition, readable, so there are no broken ones to report.
    async fn broken_datasets(&self) -> Vec<BrokenDataset> {
        Vec::new()
    }

    async fn describe_dataset(&self, dataset_name: &str) -> Result<DatasetSummary, String> {
        let list = self.list_datasets().await?;
        let dataset = list.into_iter().find(|d| d.name == dataset_name);
        match dataset {
            Some(d) => Ok(d),
            None => Err(format!("dataset '{dataset_name}' not found")),
        }
    }

    async fn get_dataset(&self, dataset_name: &str) -> Result<Dataset, String> {
        let summary = self.describe_dataset(dataset_name).await?;

        let path = format!("v1/datasets/{}/examples", summary.id);
        let res: DownloadDatasetResponse = self.get(&path).await?;
        let mut ret: Dataset = res.into();
        ret.name = dataset_name.to_string();
        ret.description = Some(summary.description);
        Ok(ret)
    }

    /// Creates an experiments, returns the ID
    async fn create_experiment(
        &self,
        dataset_name: &str,
        experiment_name: String,
        description: Option<String>,
    ) -> Result<String, String> {
        let summary = self.describe_dataset(dataset_name).await?;
        let path = format!("v1/datasets/{}/experiments", summary.id);
        let req = CreateExperimentRequest {
            name: experiment_name,
            description,
            repetitions: 1,
            metadata: serde_json::json!({}),
        };
        let ret: CreateExperimentResponse = self.post(&path, &req).await?;
        Ok(ret.id)
    }

    async fn create_dataset(
        &self,
        dataset: Dataset,
        action: PostDatasetAction,
    ) -> Result<String, String> {
        let mut dataset: PhoenixDataset = dataset.into();
        dataset.action = action;
        let path = "v1/datasets/upload";
        let dataset_name = dataset.name.clone();
        self.post_no_response(path, &dataset).await?;
        // This server is not very fast.
        sleep(Duration::from_secs_f32(0.1)).await;
        let summary = self.describe_dataset(&dataset_name).await?;
        Ok(summary.id)
    }
}

#[cfg(test)]
mod integration_test {
    use crate::EvalPlatform;
    use crate::dataset::DataPoint;
    use crate::phoenix::PhoenixClient;

    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_dataset_actions() {
        let dataset = Dataset {
            name: uuid::Uuid::new_v4().to_string(),
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

        let phoenix = PhoenixClient::new();
        phoenix
            .create_dataset(dataset.clone(), PostDatasetAction::Create)
            .await
            .expect("could not create phoenix dataset");

        let l = phoenix
            .list_datasets()
            .await
            .expect("could not download dataset");

        assert!(l.len() > 1);

        let l = phoenix
            .get_dataset(&dataset.name)
            .await
            .expect("could not download dataste");

        assert_eq!(&l.name, &dataset.name);
        assert!(dataset.description.is_some());
        assert_eq!(
            &l.description.expect("??"),
            &dataset.description.expect("?")
        );
    }
}
