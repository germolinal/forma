use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use agent::{Evaluatable, EvaluatableSet, SingleEvaluatable};
use clap::Parser;
use colored::Colorize;
use db::NoneDB;
use evaluations::{EvalPlatform, Evaluator, SampleSpec, file::FileClient, phoenix::PhoenixClient};
use protocols::FormaError;

use crate::cli::{path_file::assume_file_format, platform::EvalPlatformKind};
use crate::server::{FormaRuntime, load_agent};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedEvaluation {
    target: String,
    dataset: String,
    metrics: Vec<String>,
}

/// Expands a callable set into the individual evaluations it contains. Keeping
/// this here, rather than relying on `Display` for `Evaluator`, makes dry runs
/// describe the actual work (target, dataset, and judges), not just counts.
async fn planned_evaluations(callables: &EvaluatableSet) -> Vec<PlannedEvaluation> {
    async fn add<T: Evaluatable>(
        planned: &mut Vec<PlannedEvaluation>,
        kind: &str,
        callable: &SingleEvaluatable<T>,
    ) {
        let callable = callable.obj.read().await;
        for eval in callable.get_evals() {
            planned.push(PlannedEvaluation {
                target: format!("{kind} '{}'", callable.get_id()),
                dataset: eval.dataset.clone(),
                metrics: eval
                    .metrics
                    .iter()
                    .map(|metric| metric.template.clone())
                    .collect(),
            });
        }
    }

    let mut planned = Vec::new();
    if let Some((_, callable)) = &callables.root {
        add(&mut planned, "Agent", callable).await;
    }
    for callable in callables.workflows.values() {
        add(&mut planned, "Workflow", callable).await;
    }
    for callable in callables.nodes.values() {
        add(&mut planned, "Node", callable).await;
    }
    for callable in callables.llms.values() {
        add(&mut planned, "LLM", callable).await;
    }
    // Hash maps do not have a stable iteration order. A dry run should be
    // comparable with the run that follows it (and with another dry run).
    planned.sort_by(|left, right| {
        (&left.target, &left.dataset, &left.metrics).cmp(&(
            &right.target,
            &right.dataset,
            &right.metrics,
        ))
    });
    planned
}

/// Makes sure that every dataset this run needs can actually be read,
/// before anything is evaluated.
///
/// A dataset that does not load is an error: skipping it would quietly
/// evaluate less than what was asked for, and report a success. A broken
/// dataset that this run does not need is only reported—it is not this
/// run's problem, but the user should still hear about it, because as far
/// as the rest of the CLI is concerned it does not exist.
///
/// This is generic on the platform on purpose: every `EvalPlatformKind`,
/// including the ones that do not exist yet, gets the same treatment.
async fn check_datasets<T: EvalPlatform>(
    platform: &T,
    wanted: &HashSet<String>,
) -> Result<(), FormaError> {
    for broken in platform.broken_datasets().await {
        if !wanted.contains(&broken.id) {
            eprintln!("{}", format!("Warning: {}", broken.error).red());
        }
    }

    // Sorted, so that a run that fails twice fails the same way
    let mut names: Vec<&String> = wanted.iter().collect();
    names.sort();
    let mut errors = Vec::new();
    for name in names {
        // Reading it is the only way of knowing that it can be read.
        // Platforms that keep the datasets they hand out—the file one
        // does—do not pay for this twice.
        if let Err(e) = platform.get_dataset(name).await {
            errors.push(format!(" - {e}"));
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(FormaError::EvalError(format!(
        "could not read the datasets to evaluate:\n{}",
        errors.join("\n")
    )))
}

/// Arguments for running evaluation
#[derive(Debug, Parser)]
pub struct EvalArgs {
    /// The path to the agent file
    #[arg(short, long, default_value = "./src/agent.yaml")]
    file: String,

    /// The file describing the runtime settings, for where the agent's tools
    /// keep workspaces and run code. Conversations are not stored, whatever
    /// it says.
    #[arg(short, long, default_value = "runtime.yaml")]
    runtime: String,

    /// The name of the experiment, so you can compare it with other iterations
    #[arg(short, long)]
    experiment_name: Option<String>,

    /// LLMs to evaluate.
    #[arg(short, long)]
    llm: Option<Vec<String>>,

    /// Nodes to evaluate.
    #[arg(short, long)]
    node: Option<Vec<String>>,

    /// Datasets to evaluate (repeatable). Evals whose dataset is not
    /// listed are skipped. Naming a dataset that the agent does not
    /// use is an error.
    #[arg(short, long)]
    dataset: Option<Vec<String>>,

    /// Workflows to evaluate.
    #[arg(short, long)]
    workflow: Option<Vec<String>>,

    /// Evaluate the agent
    #[arg(short, long, default_value_t = false)]
    agent: bool,

    /// Do not run evaluations, just print ALL the
    /// elements within the agent that can potentially be
    /// evaluated (even those that do not contain any evaluations)
    #[arg(long, default_value_t = false)]
    list: bool,

    /// Do not run evaluations, print the list of
    /// elements that would be evaluated based on the
    /// filters selected (based on '-n', '-w', '-a', '-l'
    /// and '-d' options)
    #[arg(long, default_value_t = false)]
    dry_run: bool,

    /// The maximum number of examples to use from each dataset. The
    /// first N are taken, in dataset order, so that repeated runs stay
    /// comparable. Pass --random to sub-sample instead.
    #[arg(short, long)]
    max_examples: Option<usize>,

    /// Sub-sample --max-examples examples pseudo-randomly instead of
    /// taking the first N. Seeded (see --seed), so repeated runs are
    /// still reproducible.
    #[arg(long, default_value_t = false)]
    random: bool,

    /// Seed for --random sub-sampling. Implies --random. Change it to
    /// evaluate a different subset.
    #[arg(long)]
    seed: Option<u64>,

    /// The number of dataset examples to run in parallel
    #[arg(short, long, default_value = "25")]
    batch_size: usize,

    /// The user ID to use in the REPL session
    #[arg(long, default_value = "test-user")]
    user: String,

    /// Where to read the datasets from, and write the results to
    #[arg(long, value_enum, default_value_t, env = "FORMA_EVAL_PLATFORM")]
    platform: EvalPlatformKind,

    /// Do not fail when a metric scores below its `required_score`.
    ///
    /// The breaches are still printed, and still marked with a ❌ in the
    /// report: this only stops them from ending the process with an
    /// error. It is what you want while working on the agent, and not
    /// what you want in CI.
    #[arg(long, default_value_t = false)]
    no_fail: bool,
}

impl EvalArgs {
    async fn get_evaluator(
        &self,
        dir: PathBuf,
        callables: EvaluatableSet,
    ) -> Result<Evaluator, FormaError> {
        let callable_set = callables
            .filter(self.agent, &self.workflow, &self.node, &self.llm)
            .map_err(FormaError::EvalError)?
            .filter_datasets(&self.dataset)
            .await;

        let ret = Evaluator {
            batch_size: self.batch_size,
            sampling: SampleSpec::new(self.max_examples, self.random, self.seed),
            dir,
            callable_set,
            experiment_name: self.experiment_name.clone().unwrap_or_default(),
            no_fail: self.no_fail,
        };
        Ok(ret)
    }

    /// Run the evaluations as determined by `self`
    pub async fn eval(&self) -> Result<(), FormaError> {
        if self.max_examples.is_none() && (self.random || self.seed.is_some()) {
            let flag = if self.seed.is_some() {
                "seed"
            } else {
                "random"
            };
            eprintln!("Warning: --{flag} has no effect without --max-examples");
        }

        let path = Path::new(&self.file);
        if !path.exists() {
            return Err(FormaError::EvalError(format!(
                "File '{}' does not exist.",
                self.file
            )));
        }
        if !path.is_file() {
            return Err(FormaError::EvalError(format!(
                "'{}' is not a file.",
                self.file
            )));
        }
        // Before the agent, whose tools are checked against what the server
        // gives them.
        FormaRuntime::load(&assume_file_format(&self.runtime))?.install()?;
        let db = Arc::new(NoneDB {});
        let agent = load_agent(&self.file, &self.user, db.clone()).await?;
        let path = Path::new(&self.file);
        let parent_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or("could not read file OOOO")?;

        let mut callables = EvaluatableSet::default();

        agent.collect_evals(&mut callables)?;
        // Are we just printing what can be evaluated?
        if self.list {
            println!("\n{}", callables);
            return Ok(());
        }

        // Check the selection against the whole set, before pruning it. This is
        // what allows us to tell the user whether the element they asked for
        // does not exist or simply has no evals of its own.
        callables
            .check_selection(self.agent, &self.workflow, &self.node, &self.llm)
            .map_err(FormaError::EvalError)?;

        callables
            .check_dataset_selection(&self.dataset)
            .await
            .map_err(FormaError::EvalError)?;

        // Get rid of callables that cannot be evaluated
        let callables = callables.with_evals();

        let evaluator = self.get_evaluator(parent_dir, callables).await?;

        // A dry run remains useful when filters leave no matching work: it
        // shows the zero-item plan.
        if self.dry_run {
            self.print_dry_run(&evaluator).await;
            return Ok(());
        }

        if evaluator.is_empty() {
            println!("... nothing to evaluate");
            return Ok(());
        }

        // If not just printing, we need an experiment name
        if self.experiment_name.is_none() {
            let err = FormaError::EvalError(
                "an experiment name is requred for running evaluations".to_string(),
            );
            return Err(err);
        }

        match self.platform {
            EvalPlatformKind::File => Self::run::<FileClient>(evaluator).await,
            EvalPlatformKind::Phoenix => Self::run::<PhoenixClient>(evaluator).await,
        }
    }

    /// Prints exactly what a non-dry run would evaluate. Only that: listing
    /// the excluded evaluations too puts datasets that will not run on
    /// screen, told apart by colour alone, which an agent reading the output
    /// cannot see. It deliberately does not read datasets or require an
    /// experiment name: neither happens during a dry run.
    async fn print_dry_run(&self, evaluator: &Evaluator) {
        let will_run = planned_evaluations(&evaluator.callable_set).await;

        println!("\n{}", "DRY RUN — no evaluations ran".cyan().bold());
        println!(
            "{}",
            "This is the work that would run without --dry-run.".dimmed()
        );
        println!();
        println!("{}", "Run settings".blue().bold());
        println!(
            "  {} {}",
            "Platform:".dimmed(),
            format!("{:?}", self.platform).yellow()
        );
        println!(
            "  {} {}",
            "Experiment:".dimmed(),
            self.experiment_name
                .as_deref()
                .unwrap_or("not set (required only when running)")
                .yellow()
        );
        println!(
            "  {} {}",
            "Batch size:".dimmed(),
            self.batch_size.to_string().yellow()
        );
        let examples = match (self.max_examples, evaluator.sampling.seed) {
            (None, _) => "all examples".to_string(),
            (Some(max), None) => format!("first {max} examples per dataset"),
            (Some(max), Some(seed)) => {
                format!("up to {max} randomly selected examples per dataset (seed {seed})")
            }
        };
        println!("  {} {}", "Examples:".dimmed(), examples.yellow());

        let targets = if !self.agent
            && self.workflow.is_none()
            && self.node.is_none()
            && self.llm.is_none()
        {
            "all targets with evaluations".to_string()
        } else {
            let mut targets = Vec::new();
            if self.agent {
                targets.push("agent".to_string());
            }
            if let Some(ids) = &self.workflow {
                targets.push(format!("workflows: {}", ids.join(", ")));
            }
            if let Some(ids) = &self.node {
                targets.push(format!("nodes: {}", ids.join(", ")));
            }
            if let Some(ids) = &self.llm {
                targets.push(format!("LLMs: {}", ids.join(", ")));
            }
            targets.join("; ")
        };
        println!("  {} {}", "Target filter:".dimmed(), targets.yellow());
        println!(
            "  {} {}",
            "Dataset filter:".dimmed(),
            self.dataset
                .as_ref()
                .map(|datasets| datasets.join(", "))
                .unwrap_or_else(|| "all datasets".to_string())
                .yellow()
        );

        Self::print_plan(&will_run);
    }

    fn print_plan(evaluations: &[PlannedEvaluation]) {
        let title = format!("Will run ({})", evaluations.len()).green().bold();
        println!("\n{title}");
        for evaluation in evaluations {
            println!("  {}", evaluation.target.bold());
            println!(
                "    {} {}",
                "dataset:".dimmed(),
                evaluation.dataset.yellow()
            );
            if evaluation.metrics.is_empty() {
                println!(
                    "    {} {}",
                    "metrics:".dimmed(),
                    "none (answers will not be judged)".italic()
                );
            } else {
                let truncated_metrics: Vec<String> = evaluation
                    .metrics
                    .iter()
                    .map(|x| {
                        const N: usize = 75;
                        let first_line = x.lines().next().unwrap_or("").trim();
                        if first_line.len() > N {
                            format!("{}...", &first_line[0..N])
                        } else {
                            format!("{}...", first_line)
                        }
                    })
                    .collect();
                println!(
                    "    {}{}",
                    "metrics:\n\t- ".dimmed(),
                    truncated_metrics.join("\n\t- ").cyan()
                );
            }
        }
    }

    /// Checks the datasets and, if they are all there, runs the
    /// evaluations—both on the same platform client.
    async fn run<T: EvalPlatform>(evaluator: Evaluator) -> Result<(), FormaError> {
        let platform = T::new();
        // The set has already been filtered, so these are exactly the
        // datasets that are about to be evaluated
        let wanted = evaluator.callable_set.dataset_names().await;
        check_datasets(&platform, &wanted).await?;
        evaluator.evaluate_with(&platform).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with a dataset that reads, and one that does not: a
    /// dataset written before `output` was renamed `expected_output`
    fn store() -> FileClient {
        let dir = std::env::temp_dir().join(format!("forma-eval-{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        std::fs::create_dir_all(&data).expect("could not create temp dir");
        std::fs::write(
            data.join("fine.json"),
            r#"{"name": "fine", "data": [{"input": "hi"}]}"#,
        )
        .expect("could not write dataset");
        std::fs::write(
            data.join("wanderer.json"),
            r#"{"name": "wanderer", "data": [{"input": "hi", "output": "hello"}]}"#,
        )
        .expect("could not write dataset");
        FileClient::with_root(dir)
    }

    #[tokio::test]
    async fn a_dataset_that_is_not_evaluated_does_not_stop_the_run() {
        let wanted = HashSet::from(["fine".to_string()]);
        check_datasets(&store(), &wanted)
            .await
            .expect("the broken dataset is not part of this run");
    }

    #[tokio::test]
    async fn a_dataset_that_is_evaluated_but_cannot_be_read_is_an_error() {
        let wanted = HashSet::from(["fine".to_string(), "wanderer".to_string()]);
        let Err(e) = check_datasets(&store(), &wanted).await else {
            panic!("a dataset that cannot be read should not be skipped");
        };
        let e = e.to_string();
        assert!(e.contains("wanderer.json"), "{e}");
        // ... and it says what is actually wrong with it
        assert!(e.contains("output"), "{e}");
    }
}
