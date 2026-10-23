use async_trait::async_trait;
use std::collections::BTreeMap;

use crate::{
    dataset::{BrokenDataset, Dataset, DatasetSummary, PostDatasetAction},
    judgement::{ActualAnswer, Judgement},
};

/// An evaluation platform client, for saving, plotting
/// and organising AI Evaluations.
#[async_trait]
pub trait EvalPlatform {
    /// Creates a new client. Should read the Env variables
    /// necessary for configuration
    fn new() -> Self;

    /// Pushes the judgement of an AI result (e.g.,
    /// is it good? bad?). This can be calculated by
    /// an LLM or any other method.
    ///
    /// Returns the ID of the record created
    async fn push_judgement(&self, req: &Judgement) -> Result<String, String>;

    /// Pushes the actual result produced by an AI Agent, as a response to
    /// the input provided on one example.
    ///
    /// Returns the ID of the record created
    async fn push_actual_answer(
        &self,
        experiment_id: &str,
        req: &ActualAnswer,
    ) -> Result<String, String>;

    /// Retrieves a list of all the datasets available in the plafrorm
    async fn list_datasets(&self) -> Result<Vec<DatasetSummary>, String>;

    /// The datasets that are in the platform but cannot be used, and why.
    ///
    /// `list_datasets` leaves them out—one broken dataset should not make
    /// the whole store unreadable—so this is what makes them visible.
    /// Platforms that serve their datasets over an API cannot have any,
    /// and return an empty vector.
    async fn broken_datasets(&self) -> Vec<BrokenDataset>;

    /// Retrieves a dataset ID and Description from its name.
    async fn describe_dataset(&self, dataset_name: &str) -> Result<DatasetSummary, String>;

    /// Retrieves a dataset by name
    async fn get_dataset(&self, dataset_name: &str) -> Result<Dataset, String>;

    /// Tells the platform what the metrics of an experiment require of
    /// it, so that it can report them against their own requirements.
    ///
    /// This is sent when the experiment is created, rather than being
    /// inferred from the judgements as they arrive, because the case that
    /// matters most is the one in which no judgement arrives at all: an
    /// element that answered nothing is never judged, and a requirement
    /// that was never measured has not been met.
    ///
    /// Platforms that do not report requirements ignore them—the run is
    /// still failed by the runner, which does not need the platform's
    /// help to compare a mean with a number.
    async fn set_requirements(
        &self,
        experiment_id: &str,
        requirements: &BTreeMap<String, f64>,
    ) -> Result<(), String>;

    /// Creates an experiments, returns the ID
    async fn create_experiment(
        &self,
        dataset_name: &str,
        experiment_name: String,
        description: Option<String>,
    ) -> Result<String, String>;

    /// Creates a dataset, returns its ID
    async fn create_dataset(
        &self,
        dataset: Dataset,
        action: PostDatasetAction,
    ) -> Result<String, String>;
}
