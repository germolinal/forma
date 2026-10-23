use crate::cli::platform::EvalPlatformKind;
use clap::Parser;
use colored::*;
use evaluations::EvalPlatform;
use evaluations::dataset::DatasetSummary;
use evaluations::file::FileClient;
use evaluations::phoenix::PhoenixClient;

/// A command for listing the datasets available
/// in the evaluation service
#[derive(Debug, Parser)]
pub struct DatasetListArgs {
    /// Where the datasets are kept
    #[arg(long, value_enum, default_value_t, env = "FORMA_EVAL_PLATFORM")]
    pub(crate) platform: EvalPlatformKind,
}

impl DatasetListArgs {
    async fn datasets<T: EvalPlatform>(&self) -> Result<Vec<DatasetSummary>, String> {
        let platform = T::new();
        // A dataset that cannot be read is not a dataset that is not
        // there... say so, instead of leaving it out of the list
        for broken in platform.broken_datasets().await {
            eprintln!("{}", format!("Warning: {}", broken.error).red());
        }
        platform.list_datasets().await
    }

    /// List the datasets available
    pub async fn list(&self) -> Result<(), String> {
        let list = match self.platform {
            EvalPlatformKind::File => self.datasets::<FileClient>().await?,
            EvalPlatformKind::Phoenix => self.datasets::<PhoenixClient>().await?,
        };
        if list.is_empty() {
            println!("No datasets available");
            return Ok(());
        }

        list.iter().for_each(|d| {
            let id = d.id.clone();
            let name = format!("# {}", d.name).clone().green().bold();
            let description = if d.description.is_empty() {
                "<not described>".to_string().dimmed().italic()
            } else {
                d.description.clone().italic()
            };
            let n = format!("{}", d.example_count).yellow();
            let extra = format!("len={n} | id={id}").yellow();
            println!("{name} ({extra})\n\n{description}\n\n");
        });
        Ok(())
    }
}
