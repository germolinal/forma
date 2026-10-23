use std::path::Path;

use clap::{Parser, ValueEnum};
use evaluations::EvalPlatform;
use evaluations::{
    dataset::{Dataset, PostDatasetAction},
    file::FileClient,
    phoenix::PhoenixClient,
};
use serde::Serialize;

use crate::cli::platform::EvalPlatformKind;

/// Indicates whether to append or create a dataset
#[derive(Default, Copy, Clone, PartialEq, Eq, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum UploadDatasetMode {
    #[default]
    Create,
    Append,
}

impl From<UploadDatasetMode> for PostDatasetAction {
    fn from(item: UploadDatasetMode) -> Self {
        match item {
            UploadDatasetMode::Append => PostDatasetAction::Append,
            UploadDatasetMode::Create => PostDatasetAction::Create,
        }
    }
}

/// Arguments for uploading a new (or appending to) dataset
#[derive(Debug, Parser)]
pub struct DatasetUploadArgs {
    /// The name of the JSON file containing the dataset
    #[arg(short, long)]
    pub(crate) file: String,

    /// Should we 'append' or 'create'?
    #[arg(short, long, default_value_t, value_enum)]
    pub(crate) action: UploadDatasetMode,

    /// Where the dataset should be kept
    #[arg(long, value_enum, default_value_t, env = "FORMA_EVAL_PLATFORM")]
    pub(crate) platform: EvalPlatformKind,
}

impl DatasetUploadArgs {
    async fn create<T: EvalPlatform>(&self, dataset: Dataset) -> Result<String, String> {
        T::new().create_dataset(dataset, self.action.into()).await
    }

    /// Upload a dataset
    pub async fn upload(&self) -> Result<(), String> {
        use std::fs::File;
        use std::io::BufReader;

        let filepath = Path::new(&self.file);
        if !filepath.exists() {
            let err = format!("File '{}' does not exist", self.file);
            return Err(err);
        }
        let file = File::open(&self.file).map_err(|e| format!("Failed to open file: {e}"))?;
        let reader = BufReader::new(file);

        // Replace `Dataset` with your actual struct type
        let dataset: Dataset =
            serde_json::from_reader(reader).map_err(|e| format!("Failed to parse JSON: {e}"))?;

        let id = match self.platform {
            EvalPlatformKind::File => self.create::<FileClient>(dataset).await?,
            EvalPlatformKind::Phoenix => self.create::<PhoenixClient>(dataset).await?,
        };
        match self.action {
            UploadDatasetMode::Create => println!("dataset '{id}' created"),
            UploadDatasetMode::Append => println!("dataset '{id}' appended to"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod integration_test {

    use super::*;

    #[test]
    fn test_convert() {
        let d: PostDatasetAction = UploadDatasetMode::Create.into();
        assert_eq!(d, PostDatasetAction::Create);

        let d: PostDatasetAction = UploadDatasetMode::Append.into();
        assert_eq!(d, PostDatasetAction::Append);
    }
}
