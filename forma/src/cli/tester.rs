use crate::cli::dataset::create::UploadDatasetMode;
use crate::cli::path_file::PathAndFile;
use clap::Parser;
use evaluations::dataset::{DataPoint, Dataset};
use genai_client::{GenaiClient, GenaiClientTrait, GenaiMessageTrait, LLMMessage};
use protocols::FormaError;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;
use utils::ProgressBar;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Tester {
    llm: GenaiClient,
    prompt: String,
}

/// Arguments to call Personas to generate a dataset
#[derive(Debug, Parser)]
pub struct TesterArgs {
    /// The path where the 'personas' descriptions are kept
    #[arg(short, long, default_value = "./evals/personas")]
    path: String,

    /// The name of the 'persona' file within the given path
    #[arg(short, long)]
    file: String,

    /// The relative path of the output dataset file
    #[arg(short, long, default_value = "./evals/data")]
    output_path: String,

    /// The number of questions to try
    #[arg(short, long, default_value = "5")]
    n: usize,

    /// Whether to 'append' to an existing dataset file or 'create' (overwrite) it
    #[arg(short, long, default_value_t, value_enum)]
    action: UploadDatasetMode,
}

static LEADING_LIST_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\.\s*").expect("valid list-number regex"));

/// Removes list numbering that can leak from the generated-input feedback prompt.
fn strip_leading_list_number(content: String) -> String {
    LEADING_LIST_NUMBER.replace(&content, "").into_owned()
}

impl TesterArgs {
    fn resolve_outdir(&self) -> Result<PathBuf, String> {
        let current_dir =
            std::env::current_dir().map_err(|e| format!("could not get current directory: {e}"))?;
        if self.path.is_empty() {
            Err("please provide a path using the '-p' argument".to_string())
        } else if self.path == "." {
            Ok(current_dir)
        } else {
            let mut path = self.output_path.clone();
            if path.ends_with('/') {
                path = path.trim_end_matches('/').to_string();
            }
            if path.starts_with('/') {
                path = path.trim_start_matches('/').to_string();
            }
            if path.starts_with("./") {
                path = path.trim_start_matches("./").to_string();
            }
            let path = current_dir.join(path);
            Ok(path)
        }
    }

    /// Ask personas to ask questions
    pub async fn test(&self) -> Result<(), FormaError> {
        let file = PathAndFile {
            filename: self.file.clone(),
            path: self.path.clone(),
        };
        let mut outfile = file.filename.clone();

        if outfile.ends_with(".yaml") {
            outfile = outfile.trim_end_matches(".yaml").to_string();
        }
        let tester_name = outfile.clone();
        outfile = format!("{outfile}.json");
        let filename = file.validate();
        let yaml = std::fs::read_to_string(&filename.filename).expect("Failed to read YAML file");
        let outdir = self.resolve_outdir()?;
        let outfile = outdir.join(outfile);

        let tester: Tester = serde_yaml::from_str(&yaml).map_err(|x| x.to_string())?;

        let mut dataset = if self.action == UploadDatasetMode::Append && outfile.exists() {
            let existing = std::fs::read_to_string(&outfile).map_err(|e| e.to_string())?;
            serde_json::from_str::<Dataset>(&existing).map_err(|e| e.to_string())?
        } else {
            Dataset {
                description: Some(format!("A dataset by Persona '{tester_name}'")),
                name: tester_name,
                data: Vec::with_capacity(self.n),
                ..Default::default()
            }
        };

        // Collect already-generated inputs to feed back for diversity
        let mut generated: Vec<String> = dataset
            .data
            .iter()
            .filter_map(|dp| dp.input.as_str().map(|s| s.to_string()))
            .collect();

        let progress = ProgressBar::new("Generating questions".to_string(), self.n);
        for _ in 0..self.n {
            let prompt = if generated.is_empty() {
                tester.prompt.clone()
            } else {
                format!(
                    "{}\n\nYou have already generated the following inputs; generate something different:\n{}",
                    tester.prompt,
                    generated
                        .iter()
                        .enumerate()
                        .map(|(i, q)| format!("{}. {q}", i + 1))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            // A small model may ignore the feedback list. Do not add an exact
            // duplicate to the dataset; give it one more chance to respond.
            let mut content = None;
            for _ in 0..2 {
                let msg: LLMMessage = prompt.clone().into();
                let r = tester.llm.generate(vec![msg], None).await?;
                let r = r.message().ok_or("llm did not respond with a message")?;
                let candidate = strip_leading_list_number(r.content_string().unwrap());

                if !generated.contains(&candidate) {
                    content = Some(candidate);
                    break;
                }
            }
            let content = content.ok_or("llm generated a duplicate input twice")?;
            generated.push(content.clone());
            dataset.data.push(DataPoint {
                id: None,
                input: serde_json::to_value(content).unwrap(),
                expected_output: None,
            });
            progress.tic()
        }
        progress.done();
        std::fs::create_dir_all(&outdir)
            .map_err(|e| format!("could not create directory '{}': {e}", outdir.display()))?;
        let mut output = std::fs::File::create(&outfile).map_err(|x| x.to_string())?;
        let s = serde_json::to_string_pretty(&dataset).map_err(|x| x.to_string())?;
        write!(output, "{}", s).map_err(|x| x.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::strip_leading_list_number;

    #[test]
    fn removes_feedback_list_numbering_from_generated_input() {
        assert_eq!(
            strip_leading_list_number("8. I'm planning a trip to Italy".to_string()),
            "I'm planning a trip to Italy"
        );
    }

    #[test]
    fn preserves_an_unnumbered_generated_input() {
        assert_eq!(
            strip_leading_list_number("Can you help me plan a trip?".to_string()),
            "Can you help me plan a trip?"
        );
    }
}
