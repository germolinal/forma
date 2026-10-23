use clap::Parser;
use evaluations::EvalPlatform;
use evaluations::dataset::Dataset;
use evaluations::file::FileClient;
use evaluations::phoenix::PhoenixClient;
use std::fs::File;
use std::io::Write;

use crate::cli::platform::EvalPlatformKind;

/// Arguments to download a dataset
#[derive(Debug, Parser)]
pub struct DatasetDownloadArgs {
    #[arg(short, long)]
    pub(crate) name: String,
    #[arg(short, long)]
    pub(crate) output_file: Option<String>,

    /// Where the dataset is kept
    #[arg(long, value_enum, default_value_t, env = "FORMA_EVAL_PLATFORM")]
    pub(crate) platform: EvalPlatformKind,
}

impl DatasetDownloadArgs {
    async fn dataset<T: EvalPlatform>(&self) -> Result<Dataset, String> {
        T::new().get_dataset(&self.name).await
    }

    /// Download
    pub async fn download(&self) -> Result<(), String> {
        let dataset = match self.platform {
            EvalPlatformKind::File => self.dataset::<FileClient>().await?,
            EvalPlatformKind::Phoenix => self.dataset::<PhoenixClient>().await?,
        };
        let json =
            serde_json::to_string_pretty(&dataset).expect("could not serialize downloaded dataset");
        if let Some(filename) = &self.output_file {
            let mut file = File::create(filename)
                .map_err(|e| format!("Failed to create file {filename}: {e}"))?;

            file.write_all(json.as_bytes())
                .map_err(|e| format!("Failed to write to file {filename}: {e}"))?;
        } else {
            println!("{json}")
        }
        Ok(())
    }
}
