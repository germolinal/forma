use crate::{
    dataset::{DataPoint, Dataset},
    judgement::{ActualAnswer, Annotator, EvalResult, Judgement},
};
use agent::{
    Callable, Eval, Evaluatable, EvaluatableSet, Metric, callable::CallContext,
    template::render_template,
};
use chrono::{DateTime, Utc};
use db::NoneDB;
use genai_client::{GenaiClientTrait, LLMMessage, SSEvent};
use protocols::{DeploymentCredentials, FormaError, SessionData, credentials::EncryptedTokenSet};
use rand::{SeedableRng, rngs::StdRng, seq::index};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{
    Mutex, RwLock,
    mpsc::{self, UnboundedSender},
};
use utils::ProgressBar;
use uuid::Uuid;

pub mod eval_platform;
pub mod judgement;
pub use eval_platform::EvalPlatform;

pub mod dataset;
pub mod experiment;
pub mod file;
pub mod phoenix;

#[derive(Clone, Debug, Default)]
pub struct Evaluator {
    pub experiment_name: String,
    pub dir: PathBuf,
    pub sampling: SampleSpec,
    pub callable_set: EvaluatableSet,
    pub batch_size: usize,
    /// Report metrics that fell below their `required_score`, but do not
    /// fail the run because of them. The report is unaffected: a breach
    /// is always written down, whatever this says.
    pub no_fail: bool,
}

/// A metric that did not reach the score its definition said it needed.
///
/// The requirement is checked on the mean of one metric, over one dataset,
/// for one element evaluated—the figure the *Breakdown* table of the
/// report shows. Pooling the datasets together would let a good one hide a
/// bad one, which is the thing a gate must not do.
#[derive(Clone, Debug, PartialEq)]
pub struct Breach {
    /// What was evaluated, e.g. ``Node 'reply'``
    pub callable: String,
    pub dataset: String,
    pub metric: String,
    pub required: f64,
    /// The mean that was measured, or `None` when the metric produced no
    /// score at all—because every judgement failed, or because the element
    /// itself never managed to answer.
    pub mean: Option<f64>,
}

impl std::fmt::Display for Breach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} · dataset '{}' · {}: ",
            self.callable, self.dataset, self.metric
        )?;
        match self.mean {
            Some(mean) => write!(f, "{mean:.2} is below the required {:.2}", self.required),
            // Worth spelling out: this one is not a bad score, it is the
            // absence of one, and it is the case people expect to pass.
            None => write!(
                f,
                "nothing could be scored, so the required {:.2} was not met",
                self.required
            ),
        }
    }
}

/// What one metric was measured at, while a dataset is being evaluated.
#[derive(Clone, Debug, Default)]
struct MetricTally {
    required: Option<f64>,
    total: f64,
    count: usize,
}

impl MetricTally {
    fn mean(&self) -> Option<f64> {
        match self.count {
            0 => None,
            n => Some(self.total / n as f64),
        }
    }

    /// The requirement, checked. `None` when the metric declared none, or
    /// when it met the one it declared.
    fn breach(&self, callable: &str, dataset: &str, metric: &str) -> Option<Breach> {
        let required = self.required?;
        let mean = self.mean();
        if mean.is_some_and(|mean| mean >= required) {
            return None;
        }
        Some(Breach {
            callable: callable.to_string(),
            dataset: dataset.to_string(),
            metric: metric.to_string(),
            required,
            mean,
        })
    }
}

/// How each dataset is trimmed before it is evaluated.
///
/// Sub-sampling is opt-in and, when enabled, seeded: two runs with the
/// same spec evaluate the same datapoints, so their scores stay
/// comparable.
#[derive(Clone, Copy, Debug, Default)]
pub struct SampleSpec {
    /// Cap on datapoints per dataset. `None` keeps every datapoint.
    pub max_examples: Option<usize>,
    /// `None` keeps the first `max_examples` datapoints in dataset
    /// order. `Some(seed)` picks them with a `seed`-seeded RNG.
    pub seed: Option<u64>,
}

impl SampleSpec {
    /// Seed used by `--random` when no explicit `--seed` is given.
    /// Fixed on purpose: `--random` still has to be reproducible.
    pub const DEFAULT_SEED: u64 = 0x5eed;

    /// Builds a spec from the CLI flags. `random` turns on seeded
    /// sub-sampling; an explicit `seed` implies `random`.
    pub fn new(max_examples: Option<usize>, random: bool, seed: Option<u64>) -> Self {
        let seed = match seed {
            Some(seed) => Some(seed),
            None if random => Some(Self::DEFAULT_SEED),
            None => None,
        };
        Self { max_examples, seed }
    }

    /// Returns the datapoints to evaluate, applying the example cap and
    /// the selection strategy. Randomly selected indices are sorted, so
    /// the evaluation still runs in dataset order.
    fn select(&self, data: &[DataPoint]) -> Vec<DataPoint> {
        let Some(max) = self.max_examples else {
            return data.to_vec();
        };
        if data.len() <= max {
            return data.to_vec();
        }
        match self.seed {
            None => data[..max].to_vec(),
            Some(seed) => {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut picked = index::sample(&mut rng, data.len(), max).into_vec();
                picked.sort_unstable();
                picked.into_iter().map(|i| data[i].clone()).collect()
            }
        }
    }
}

impl Evaluator {
    pub fn is_empty(&self) -> bool {
        self.callable_set.is_empty()
    }
}

impl std::fmt::Display for Evaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.experiment_name.is_empty() {
            writeln!(
                f,
                "----------\nExperiment \"{}\"\n----------\n",
                self.experiment_name
            )?;
        }
        writeln!(f, "{}", self.callable_set)?;

        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct EvalResources {
    datasets: HashMap<String, Dataset>,
    templates: HashMap<String, Arc<RwLock<String>>>,
}

impl EvalResources {
    /// Retrieves a `Dataset` from the `datasets` field
    pub fn get_dataset(&self, name: &str) -> Result<&Dataset, FormaError> {
        match self.datasets.get(name) {
            Some(v) => Ok(v),
            None => Err(FormaError::EvalError(format!(
                "dataset '{name}' is not within the provided resources"
            ))),
        }
    }

    pub fn get_template(&self, name: &str) -> Result<Arc<RwLock<String>>, FormaError> {
        match self.templates.get(name) {
            Some(v) => Ok(v.clone()),
            None => Err(FormaError::EvalError(format!(
                "metric template '{name}' is not within the provided resources"
            ))),
        }
    }

    async fn check_datasets<T: EvalPlatform>(&self, eval_platform: &T) -> Result<(), FormaError> {
        let dataset_list = eval_platform.list_datasets().await?;
        for dataset_name in self.datasets.keys() {
            if dataset_list.iter().any(|ds| &ds.name == dataset_name) {
                continue;
            }
            // A dataset that is there but cannot be read is not missing.
            // `list_datasets` leaves it out, so say what is wrong with it
            // instead of claiming that it does not exist.
            let broken = eval_platform
                .broken_datasets()
                .await
                .into_iter()
                .find(|b| &b.id == dataset_name);
            return Err(FormaError::EvalError(match broken {
                Some(b) => b.error,
                None => format!("dataset '{dataset_name}' not found."),
            }));
        }

        Ok(())
    }

    async fn pull<T: EvalPlatform>(&mut self, eval_platform: &T) -> Result<(), String> {
        for (name, data) in self.datasets.iter_mut() {
            // Download dataset
            *data = eval_platform.get_dataset(name).await?;
        }

        Ok(())
    }
}

fn parse_session(
    datapoint_id: &str,
    dataset_name: &str,
    input: &Value,
) -> Result<SessionData, FormaError> {
    if serde_json::Value::as_object(input).is_some() {
        let mut s: SessionData = serde_json::from_value(input.clone())?;
        if s.id.is_none() {
            s.id = Some(Uuid::new_v4().to_string().into());
        }
        Ok(s)
    } else if serde_json::Value::as_str(input).is_some() {
        let message = serde_json::to_string(input).unwrap();
        // get rid of quotation marks
        let message = message.trim_matches('"').to_string();
        Ok(SessionData {
            id: Some(Uuid::new_v4().to_string().into()),
            messages: vec![message.into()],
            ..Default::default()
        })
    } else {
        Err(FormaError::EvalError(format!(
            "in datapoint '{datapoint_id}' of dataset '{dataset_name}' input should be a Session in JSON format, or a string."
        )))
    }
}

fn build_actual_answer(
    example_id: String,
    response: Result<Option<Value>, FormaError>,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
) -> ActualAnswer {
    match response {
        Ok(output) => {
            if let Some(output) = output {
                ActualAnswer {
                    dataset_example_id: example_id.clone(),
                    output: output.clone(),
                    repetition_number: 1,
                    start_time,
                    end_time,
                    ..Default::default()
                }
            } else {
                ActualAnswer {
                    dataset_example_id: example_id,
                    output: serde_json::Value::Null,
                    repetition_number: 1,
                    start_time,
                    end_time,
                    ..Default::default()
                }
            }
        }
        Err(e) => ActualAnswer {
            dataset_example_id: example_id,
            output: serde_json::Value::Null,
            repetition_number: 1,
            start_time,
            end_time,
            error: Some(e.to_string()),
            ..Default::default()
        },
    }
}

async fn run_unit_example<C: Callable<NoneDB>>(
    id: String,
    dataset_name: String,
    example: DataPoint,
    callable: Arc<RwLock<C>>,
    tx: UnboundedSender<(DataPoint, ActualAnswer)>,
) -> Result<(), FormaError> {
    let (mock_tx, mut mock_rx) = mpsc::unbounded_channel::<SSEvent>();
    let input = &example.input;
    let session: SessionData = parse_session(&id, &dataset_name, input)?;
    let session = Arc::new(RwLock::new(session));
    let (agent_id, agent_timezone): (Arc<str>, Option<Arc<str>>) = {
        let c = callable.read().await;
        (c.get_id().into(), c.timezone().map(Into::into))
    };

    // An evaluation has no device, so the agent's own zone stands in for the
    // user's.
    let ctx = CallContext::<NoneDB> {
        user_timezone: None,
        agent_timezone,
        session: session.clone(),
        tx: mock_tx,
        job_id: id.clone().into(),
        user_roles: vec![].into(),
        user_credentials: Arc::new(Mutex::new(EncryptedTokenSet::default())),
        db: Arc::new(NoneDB::default()),
        deployment_credentials: Arc::new(DeploymentCredentials::default()),
        agent_id,
        deployment_id: None,
        user_id: "test_user_id".into(),
    };

    let start_time: DateTime<Utc> = Utc::now();
    let agent = callable.read().await;
    let response = agent.run(ctx, false).await;
    let end_time: DateTime<Utc> = Utc::now();
    let response = build_actual_answer(id, response, start_time, end_time);

    tx.send((example, response)).unwrap();
    while mock_rx.recv().await.is_some() {} //. to keep the channel open
    Ok(())
}

async fn judge_unit_example(
    id: String,
    metric: Metric,
    template: Arc<RwLock<String>>,
    example: DataPoint,
    answer: ActualAnswer,
    tx: UnboundedSender<Judgement>,
) {
    let metric_name = metric.id.clone();
    let mut judgement = Judgement {
        experiment_run_id: id,
        name: metric_name.to_string().clone(),
        annotator_kind: Annotator::LLM,
        start_time: Utc::now(),
        // Set here, and on every exit path below, so that a judgement
        // that failed still says what was expected of it.
        required_score: metric.required_score,
        ..Default::default()
    };

    let template = template.read().await;

    let mut data: HashMap<String, Value> = HashMap::with_capacity(3);
    data.insert("input".to_string(), example.input.clone());
    data.insert("actual_output".to_string(), answer.output.clone());
    if let Some(o) = &example.expected_output {
        data.insert("expected_output".to_string(), o.clone());
    }

    let prompt = match render_template(&template, &data) {
        Ok(v) => v,
        Err(e) => {
            judgement.error = Some(e);
            judgement.end_time = Utc::now();
            tx.send(judgement)
                .expect("could not send failed judgement (template failed");
            return;
        }
    };
    // Straight to the provider's shape: a judge prompt is text, with nothing
    // to resolve.
    let message = LLMMessage::from(prompt);

    // We are always looking for the same output
    let response = match metric.judge.generate(vec![message], None).await {
        Ok(v) => v,
        Err(e) => {
            judgement.error = Some(e.to_string());
            judgement.end_time = Utc::now();
            tx.send(judgement)
                .expect("could not send failed judgement (template failed");
            return;
        }
    };
    let last_msg = response
        .message()
        .expect("Judge LLM did not return any message");
    let content: Vec<String> = if let LLMMessage::Assistant(msg) = last_msg {
        msg.content.iter().map(|part| part.into()).collect()
    } else {
        unreachable!("expecting an Assistant message");
    };
    let content: String = content.join("\n");
    let eval_result: EvalResult = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            judgement.error = Some(e.to_string());
            judgement.end_time = Utc::now();
            tx.send(judgement)
                .expect("could not send failed judgement (template failed");
            return;
        }
    };
    judgement.result = Some(eval_result);
    judgement.end_time = Utc::now();
    tx.send(judgement)
        .expect("failed to send message after judging example");
}

async fn run_eval<C: Callable<NoneDB>, T: EvalPlatform>(
    unit: Arc<RwLock<C>>,
    eval: Arc<RwLock<Eval>>,
    experiment_name: &str,
    resources: &EvalResources,
    batch_size: usize,
    sampling: SampleSpec,
    eval_platform: &T,
) -> Result<Vec<Breach>, FormaError> {
    // Communication channel between Callable elements (e.g., Nodes, Workflows) and this thread
    let (run_tx, mut run_rx) = mpsc::unbounded_channel::<(DataPoint, ActualAnswer)>();

    let callable_id = {
        let c = unit.read().await;
        c.get_id().to_string()
    };

    let (dataset_name, experiment_name) = {
        let eval = eval.read().await;
        let experiment_name = format!("[{} {}] {}", C::CLASS, callable_id, experiment_name);
        (eval.dataset.clone(), experiment_name)
    };

    println!(
        "\x1b[1;32mEvaluating {} '{}' with dataset '{}'\x1b[0m",
        C::CLASS,
        callable_id,
        dataset_name
    );
    // Register this run as part of an experiment within the dataset.
    let experiment_id = eval_platform
        .create_experiment(&dataset_name, experiment_name.to_string().clone(), None)
        .await?;

    // Sent before anything runs, so that the platform can report a metric
    // against its requirement even if it never gets to judge anything.
    let requirements: BTreeMap<String, f64> = {
        let eval = eval.read().await;
        eval.metrics
            .iter()
            .filter_map(|m| Some((m.id.clone(), m.required_score?)))
            .collect()
    };
    eval_platform
        .set_requirements(&experiment_id, &requirements)
        .await?;

    // Go through each example within this dataset, and evaluate.
    // `sampling` may trim the dataset down to `--max-examples`.
    let dataset = resources.get_dataset(&dataset_name)?;
    let data = sampling.select(&dataset.data);

    let progress = Arc::new(RwLock::new(ProgressBar::new(
        format!("Asking {}", C::CLASS),
        data.len(),
    )));

    for chunk in data.chunks(batch_size) {
        let mut handles = Vec::with_capacity(batch_size);
        for example in chunk.iter() {
            // prepare inputs
            let id = example.id.clone().unwrap_or_default();
            let example_clone = example.clone();
            let dataset_name_clone = dataset_name.clone();
            let unit_clone = unit.clone();
            let tx_clone = run_tx.clone();
            let pclone = progress.clone();
            let h = tokio::spawn(async move {
                run_unit_example(id, dataset_name_clone, example_clone, unit_clone, tx_clone)
                    .await
                    .unwrap();
                pclone.read().await.tic();
            });
            handles.push(h);
        }
        futures::future::join_all(handles).await;
    }
    progress.read().await.done();
    drop(run_tx);

    // Communication channel between the Actual Response and the judgement
    let (judge_tx, mut judge_rx) = mpsc::unbounded_channel::<Judgement>();
    // Process the ActualResponses as they come back.
    while let Some((example, actual_answer)) = run_rx.recv().await {
        // Push actual response
        let id = eval_platform
            .push_actual_answer(&experiment_id, &actual_answer)
            .await?;
        if actual_answer.error.is_none() {
            let eval = eval.read().await;
            for m in eval.metrics.iter() {
                let template = resources.get_template(&m.id)?;
                let id_clone = id.clone();
                let answer_clone = actual_answer.clone();
                let tx_clone = judge_tx.clone();
                let metric_clone = m.clone();
                let example_clone = example.clone();
                tokio::spawn(async move {
                    judge_unit_example(
                        id_clone,
                        metric_clone,
                        template,
                        example_clone,
                        answer_clone,
                        tx_clone,
                    )
                    .await
                });
            }
        }
    }
    drop(judge_tx);
    // Seeded from the metrics, not from the judgements, so that a metric
    // that was never judged at all—because every answer failed, and
    // judging is skipped for those—is still checked against its
    // requirement instead of quietly disappearing.
    let mut measured: HashMap<String, MetricTally> = {
        let eval = eval.read().await;
        eval.metrics
            .iter()
            .map(|m| {
                let tally = MetricTally {
                    required: m.required_score,
                    ..Default::default()
                };
                (m.id.clone(), tally)
            })
            .collect()
    };
    while let Some(r) = judge_rx.recv().await {
        // Push actual response
        if let Some(e) = &r.error {
            eprint!("error when judging: '{}'", e)
        }
        let tally = measured.entry(r.name.clone()).or_default();
        if let Some(result) = &r.result {
            tally.total += result.score;
            tally.count += 1;
        }
        eval_platform.push_judgement(&r).await?;
    }

    let callable = format!("{} '{}'", C::CLASS, callable_id);
    // Sorted, so that a run that fails twice fails the same way
    let mut names: Vec<&String> = measured.keys().collect();
    names.sort();
    let breaches = names
        .iter()
        .filter_map(|name| measured[*name].breach(&callable, &dataset_name, name))
        .collect();

    Ok(breaches)
}

/// Runs the evaluations
async fn evaluate_callable<C: Evaluatable + Callable<NoneDB>, T: EvalPlatform>(
    unit: Arc<RwLock<C>>,
    experiment_name: &str,
    resources: &EvalResources,
    batch_size: usize,
    sampling: SampleSpec,
    eval_platform: &T,
) -> Result<Vec<Breach>, FormaError> {
    let evals: Vec<Arc<RwLock<Eval>>> = {
        let w = unit.read().await;
        w.get_evals()
            .iter()
            .map(|e| Arc::new(RwLock::new(e.clone())))
            .collect()
    };

    let mut breaches = Vec::new();
    for e in evals.iter() {
        breaches.extend(
            run_eval(
                unit.clone(),
                e.clone(),
                experiment_name,
                resources,
                batch_size,
                sampling,
                eval_platform,
            )
            .await?,
        );
    }

    Ok(breaches)
}

impl Evaluator {
    /// Populates an `EvalResources` object with default values and the right names.
    async fn extract_templates_datasets<T: Evaluatable>(
        resources: &mut EvalResources,
        w: &Arc<RwLock<T>>,
    ) -> Result<(), FormaError> {
        let w = w.read().await;

        let from = w.get_evals();
        for u in from.iter() {
            // Do dataset
            let dataset = u.dataset.clone();
            resources.datasets.entry(dataset).or_default();

            // Do templates
            for t in u.metrics.iter() {
                let aux = resources.templates.entry(t.id.clone()).or_default();
                let mut aux = aux.write().await;
                *aux = t.template.clone();
            }
        }
        Ok(())
    }

    async fn get_eval_resources<T: EvalPlatform>(
        &self,
        eval_platform: &T,
    ) -> Result<EvalResources, FormaError> {
        let mut resources = EvalResources::default();
        // Expand
        if let Some((_, w)) = &self.callable_set.root {
            Self::extract_templates_datasets(&mut resources, &w.obj).await?;
        }
        for w in self.callable_set.workflows.values() {
            Self::extract_templates_datasets(&mut resources, &w.obj).await?;
        }
        for w in self.callable_set.nodes.values() {
            Self::extract_templates_datasets(&mut resources, &w.obj).await?;
        }
        for w in self.callable_set.llms.values() {
            Self::extract_templates_datasets(&mut resources, &w.obj).await?;
        }
        // Make sure stuff is there
        resources.check_datasets(eval_platform).await?;
        // Download
        resources.pull(eval_platform).await?;

        Ok(resources)
    }

    /// Evaluats all the Agent's subcomponents (e.g., Node, Agent, Workflows)
    /// one by one.
    ///
    /// Paralelization happens within each component
    pub async fn evaluate<T: EvalPlatform>(&self) -> Result<(), FormaError> {
        self.evaluate_with(&T::new()).await
    }

    /// Just like `evaluate`, but on a platform client that has already been
    /// created—so that the caller can check the datasets, or anything else,
    /// before the evaluations start.
    pub async fn evaluate_with<T: EvalPlatform>(
        &self,
        eval_platform: &T,
    ) -> Result<(), FormaError> {
        let resources = self.get_eval_resources(eval_platform).await?;
        let mut breaches: Vec<Breach> = Vec::new();

        // Do root
        if let Some((_, u)) = &self.callable_set.root {
            breaches.extend(
                evaluate_callable(
                    u.obj.clone(),
                    &self.experiment_name,
                    &resources,
                    self.batch_size,
                    self.sampling,
                    eval_platform,
                )
                .await?,
            );
        }
        // Do workflows
        for u in self.callable_set.workflows.values() {
            breaches.extend(
                evaluate_callable(
                    u.obj.clone(),
                    &self.experiment_name,
                    &resources,
                    self.batch_size,
                    self.sampling,
                    eval_platform,
                )
                .await?,
            );
        }
        // Do nodes
        for u in self.callable_set.nodes.values() {
            breaches.extend(
                evaluate_callable(
                    u.obj.clone(),
                    &self.experiment_name,
                    &resources,
                    self.batch_size,
                    self.sampling,
                    eval_platform,
                )
                .await?,
            );
        }
        // Do LLMs
        for u in self.callable_set.llms.values() {
            breaches.extend(
                evaluate_callable(
                    u.obj.clone(),
                    &self.experiment_name,
                    &resources,
                    self.batch_size,
                    self.sampling,
                    eval_platform,
                )
                .await?,
            );
        }

        self.check_requirements(breaches)
    }

    /// Turns the requirements that were not met into the outcome of the
    /// run: an error, unless the caller asked not to be stopped.
    ///
    /// Every breach is printed, not just the first—fixing one metric only
    /// to be told about the next one is a slow way to learn what a run
    /// found. The report has already been written by this point, and says
    /// the same thing whether or not `no_fail` is set: not failing the run
    /// is not the same as pretending it passed.
    fn check_requirements(&self, breaches: Vec<Breach>) -> Result<(), FormaError> {
        if breaches.is_empty() {
            return Ok(());
        }
        for breach in breaches.iter() {
            eprintln!("\x1b[1;31m{breach}\x1b[0m");
        }
        let summary = match breaches.len() {
            1 => "1 metric is below the score it requires".to_string(),
            n => format!("{n} metrics are below the score they require"),
        };
        if self.no_fail {
            eprintln!("\x1b[1;33m{summary} (--no-fail: not failing the run)\x1b[0m");
            return Ok(());
        }
        Err(FormaError::EvalError(summary))
    }
}

#[cfg(test)]
mod test {
    use agent::{Node, eval_agent::EvaluatableLLM};
    use chrono::Utc;
    use protocols::FormaMessage;
    use serde_json::json;

    use super::*;

    #[test]
    fn display() {
        let mut ev = Evaluator::default();
        println!("{ev}");
        ev.experiment_name = "the experiment".to_string();
        println!("{ev}");
    }

    #[test]
    fn is_empty() {
        let ev = Evaluator::default();
        assert!(ev.is_empty());
    }

    fn tally(required: Option<f64>, scores: &[f64]) -> MetricTally {
        MetricTally {
            required,
            total: scores.iter().sum(),
            count: scores.len(),
        }
    }

    #[test]
    fn a_metric_with_no_requirement_cannot_breach() {
        let tally = tally(None, &[0.0, 0.0]);
        assert_eq!(tally.breach("Node 'reply'", "wanderer", "politeness"), None);
    }

    #[test]
    fn a_metric_is_checked_on_its_mean() {
        // 1.00 and 2.00 average to 1.50, which clears 1.50 and not 2.00
        assert!(
            tally(Some(1.5), &[1.0, 2.0])
                .breach("Node 'reply'", "wanderer", "politeness")
                .is_none()
        );
        let breach = tally(Some(2.0), &[1.0, 2.0])
            .breach("Node 'reply'", "wanderer", "politeness")
            .expect("1.50 does not meet 2.00");
        assert_eq!(breach.mean, Some(1.5));
        assert_eq!(breach.required, 2.0);
        assert_eq!(
            breach.to_string(),
            "Node 'reply' · dataset 'wanderer' · politeness: 1.50 is below the required 2.00"
        );
    }

    #[test]
    fn a_requirement_nothing_could_measure_is_a_breach() {
        // Every judgement failed, or the element never answered at all
        let breach = tally(Some(2.0), &[])
            .breach("Node 'reply'", "wanderer", "politeness")
            .expect("an unmeasured requirement has not been met");
        assert_eq!(breach.mean, None);
        assert!(
            breach.to_string().contains("nothing could be scored"),
            "{breach}"
        );
    }

    fn breach(metric: &str) -> Breach {
        Breach {
            callable: "Node 'reply'".to_string(),
            dataset: "wanderer".to_string(),
            metric: metric.to_string(),
            required: 2.0,
            mean: Some(1.5),
        }
    }

    #[test]
    fn breaches_fail_the_run() {
        let evaluator = Evaluator::default();
        assert!(evaluator.check_requirements(vec![]).is_ok());

        let Err(e) = evaluator.check_requirements(vec![breach("politeness")]) else {
            panic!("a metric below its requirement should fail the run");
        };
        assert!(e.to_string().contains("1 metric is below"), "{e}");

        let Err(e) = evaluator.check_requirements(vec![breach("politeness"), breach("grounding")])
        else {
            panic!("two metrics below their requirement should fail the run");
        };
        assert!(e.to_string().contains("2 metrics are below"), "{e}");
    }

    #[test]
    fn no_fail_keeps_the_run_green() {
        let evaluator = Evaluator {
            no_fail: true,
            ..Default::default()
        };
        // Still reported—both on stderr here, and with a ❌ in the
        // report—but not the outcome of the run
        assert!(
            evaluator
                .check_requirements(vec![breach("politeness")])
                .is_ok()
        );
    }

    fn sample_data(n: usize) -> Vec<DataPoint> {
        (0..n)
            .map(|i| DataPoint {
                id: Some(i.to_string()),
                input: json!(i),
                expected_output: None,
            })
            .collect()
    }

    fn selected_ids(data: &[DataPoint]) -> Vec<usize> {
        data.iter()
            .map(|d| d.id.clone().unwrap().parse().unwrap())
            .collect()
    }

    #[test]
    fn sample_spec_new_resolves_seed() {
        // no --random, no --seed -> deterministic head selection
        assert_eq!(SampleSpec::new(Some(3), false, None).seed, None);
        // --random with no --seed -> the fixed default seed
        assert_eq!(
            SampleSpec::new(Some(3), true, None).seed,
            Some(SampleSpec::DEFAULT_SEED)
        );
        // an explicit --seed implies --random
        assert_eq!(SampleSpec::new(Some(3), false, Some(42)).seed, Some(42));
        assert_eq!(SampleSpec::new(Some(3), true, Some(42)).seed, Some(42));
    }

    #[test]
    fn sample_spec_select_keeps_all_when_not_capped() {
        let data = sample_data(5);
        // no cap: seed is irrelevant, every datapoint is kept
        assert_eq!(SampleSpec::new(None, true, Some(1)).select(&data).len(), 5);
        // cap larger than the dataset: kept as-is
        assert_eq!(
            SampleSpec::new(Some(10), false, None).select(&data).len(),
            5
        );
    }

    #[test]
    fn sample_spec_select_head_is_deterministic_and_ordered() {
        let data = sample_data(10);
        let got = SampleSpec::new(Some(3), false, None).select(&data);
        assert_eq!(selected_ids(&got), vec![0, 1, 2]);
    }

    #[test]
    fn sample_spec_select_seeded_is_reproducible_ordered_subset() {
        let data = sample_data(20);
        let spec = SampleSpec::new(Some(5), true, Some(7));

        let first = selected_ids(&spec.select(&data));
        let second = selected_ids(&spec.select(&data));

        // same spec -> same datapoints, so two runs stay comparable
        assert_eq!(first, second);
        assert_eq!(first.len(), 5);

        // without replacement, and still in dataset order
        let mut sorted = first.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(first, sorted);
        assert!(first.iter().all(|&i| i < 20));
    }

    #[test]
    fn sample_spec_select_different_seeds_differ() {
        let data = sample_data(50);
        let a = selected_ids(&SampleSpec::new(Some(10), true, Some(1)).select(&data));
        let b = selected_ids(&SampleSpec::new(Some(10), true, Some(2)).select(&data));
        assert_ne!(a, b);
    }

    #[test]
    fn test_get_dataset() {
        let mut resources = EvalResources::default();

        // Test dataset not found
        let result = resources.get_dataset("nonexistent");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FormaError::EvalError(_)));

        // Test dataset found
        let dataset = Dataset::default();
        resources.datasets.insert("test".to_string(), dataset);
        let result = resources.get_dataset("test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_dataset_multiple() {
        let mut resources = EvalResources::default();

        // Test with empty resources
        let result = resources.get_dataset("test1");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Eval Error: dataset 'test1' is not within the provided resources"
        );

        // Add multiple datasets
        resources
            .datasets
            .insert("test1".to_string(), Dataset::default());
        resources
            .datasets
            .insert("test2".to_string(), Dataset::default());

        // Test finding existing datasets
        assert!(resources.get_dataset("test1").is_ok());
        assert!(resources.get_dataset("test2").is_ok());

        // Test case sensitivity
        let result = resources.get_dataset("TEST1");
        assert!(result.is_err());

        // Test with empty string
        let result = resources.get_dataset("");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_template_not_found() {
        let resources = EvalResources::default();
        let result = resources.get_template("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FormaError::EvalError(_)));
        assert_eq!(
            err.to_string(),
            "Eval Error: metric template 'nonexistent' is not within the provided resources"
        );
    }

    #[test]
    fn test_get_template_found_and_modify() {
        let mut resources = EvalResources::default();
        let key = "$tpl".to_string();
        let initial = "initial content".to_string();
        resources
            .templates
            .insert(key.clone(), Arc::new(RwLock::new(initial.clone())));

        let got = resources
            .get_template(&key)
            .expect("template should be found");
        // Read the content
        let read_guard = got.clone();
        let content = futures::executor::block_on(async { read_guard.read().await.clone() });
        assert_eq!(content, initial);

        // Modify the content via the returned Arc and ensure the stored value updates
        let write_guard = got.clone();
        futures::executor::block_on(async {
            let mut w = write_guard.write().await;
            *w = "modified".to_string();
        });

        let stored = resources.templates.get(&key).expect("present");
        let final_content = futures::executor::block_on(async { stored.read().await.clone() });
        assert_eq!(final_content, "modified".to_string());
    }

    #[test]
    fn test_get_template_multiple_and_case_sensitivity() {
        let mut resources = EvalResources::default();
        resources
            .templates
            .insert("$a".to_string(), Arc::new(RwLock::new("A".to_string())));
        resources
            .templates
            .insert("$A".to_string(), Arc::new(RwLock::new("a".to_string())));

        let t1 = resources.get_template("$a").expect("should find $a");
        let t2 = resources.get_template("$A").expect("should find $A");

        let v1 = futures::executor::block_on(async { t1.read().await.clone() });
        let v2 = futures::executor::block_on(async { t2.read().await.clone() });

        assert_eq!(v1, "A");
        assert_eq!(v2, "a");

        // missing different case
        let res = resources.get_template("$b");
        assert!(res.is_err());
    }

    #[test]
    fn test_get_template_empty_name() {
        let mut resources = EvalResources::default();
        // Insert an entry with empty key to ensure retrieving empty key can succeed if present
        resources
            .templates
            .insert("".to_string(), Arc::new(RwLock::new("empty".to_string())));

        // Found case
        let found = resources.get_template("").expect("empty key present");
        let val = futures::executor::block_on(async { found.read().await.clone() });
        assert_eq!(val, "empty");

        // Not found case on a fresh resources
        let fresh = EvalResources::default();
        let res = fresh.get_template("");
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err().to_string(),
            "Eval Error: metric template '' is not within the provided resources"
        );
    }

    #[test]
    fn test_parse_session() {
        // Test with string input
        let txt = "hello world";
        let input = json!("hello world");
        let result = parse_session("id1", "dataset1", &input).unwrap();
        let msg: FormaMessage = txt.to_string().into();
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].to_string(), msg.to_string());

        // Test with valid SessionData object
        let mut session = SessionData::default();
        session.messages.push(msg.clone());

        let session_obj = serde_json::to_value(&session).unwrap();
        let result = parse_session("id2", "dataset2", &session_obj).unwrap();
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].to_string(), msg.to_string());

        // Test invalid input type (number)
        let invalid = json!(42);
        let err = parse_session("id3", "dataset3", &invalid).unwrap_err();
        assert!(matches!(err, FormaError::EvalError(_)));
        assert_eq!(
            err.to_string(),
            "Eval Error: in datapoint 'id3' of dataset 'dataset3' input should be a Session in JSON format, or a string."
        );

        // Test invalid input type (array)
        let invalid = json!(["test"]);
        let err = parse_session("id4", "dataset4", &invalid).unwrap_err();
        assert!(matches!(err, FormaError::EvalError(_)));
    }

    #[test]
    fn test_build_actual_answer_ok() {
        let example_id = "ex1".to_string();
        let value = serde_json::json!({"foo": "bar"});
        let start_time = Utc::now();
        let end_time = Utc::now();

        let actual = build_actual_answer(
            example_id.clone(),
            Ok(Some(value.clone())),
            start_time,
            end_time,
        );

        assert_eq!(actual.dataset_example_id, example_id);
        assert_eq!(actual.output, value);
        assert_eq!(actual.repetition_number, 1);
        assert_eq!(actual.start_time, start_time);
        assert_eq!(actual.end_time, end_time);
        assert!(actual.error.is_none());
    }

    #[test]
    fn test_build_actual_answer_err() {
        let example_id = "ex2".to_string();
        let err = FormaError::EvalError("something went wrong".to_string());
        let start_time = Utc::now();
        let end_time = Utc::now();

        let actual =
            build_actual_answer(example_id.clone(), Err(err.clone()), start_time, end_time);

        assert_eq!(actual.dataset_example_id, example_id);
        assert_eq!(actual.output, serde_json::Value::Null);
        assert_eq!(actual.repetition_number, 1);
        assert_eq!(actual.start_time, start_time);
        assert_eq!(actual.end_time, end_time);
        assert!(actual.error.is_some());
        assert_eq!(actual.error.unwrap(), err.to_string());
    }

    #[test]
    fn test_build_actual_answer_ok_with_null_value() {
        let example_id = "ex3".to_string();
        let value = serde_json::Value::Null;
        let start_time = Utc::now();
        let end_time = Utc::now();

        let actual = build_actual_answer(
            example_id.clone(),
            Ok(Some(value.clone())),
            start_time,
            end_time,
        );

        assert_eq!(actual.dataset_example_id, example_id);
        assert_eq!(actual.output, serde_json::Value::Null);
        assert_eq!(actual.repetition_number, 1);
        assert_eq!(actual.start_time, start_time);
        assert_eq!(actual.end_time, end_time);
        assert!(actual.error.is_none());
    }

    #[test]
    fn test_build_actual_answer_err_with_different_error_types() {
        let example_id = "ex4".to_string();
        let err = FormaError::EvalError("other error".to_string());
        let start_time = Utc::now();
        let end_time = Utc::now();

        let actual =
            build_actual_answer(example_id.clone(), Err(err.clone()), start_time, end_time);

        assert_eq!(actual.dataset_example_id, example_id);
        assert_eq!(actual.output, serde_json::Value::Null);
        assert_eq!(actual.repetition_number, 1);
        assert_eq!(actual.start_time, start_time);
        assert_eq!(actual.end_time, end_time);
        assert!(actual.error.is_some());
        assert_eq!(actual.error.unwrap(), err.to_string());
    }

    #[test]
    fn test_new_summarization_id_no_evals() {
        let mut node = Node::default();
        node.id = "summ-empty".to_string();
        node.summarize_evals = vec![];

        let ellm = EvaluatableLLM::new_summarization(&node);
        assert_eq!(ellm.id, format!("{}.summarize-llm", node.id));
        assert!(ellm.evals.is_empty());
    }

    #[test]
    fn test_new_triage_evals_clone_independence() {
        let mut node = Node::default();
        node.id = "node-clone".to_string();
        node.triage_evals.push(Eval::default());

        let ellm = EvaluatableLLM::new_triage(&node);
        // Mutate original after creation
        node.triage_evals.clear();

        // Ensure EvaluatableLLM kept its own clone
        assert_eq!(ellm.evals.len(), 1);
        assert_eq!(ellm.id, format!("{}.triage-llm", node.id));
    }

    #[test]
    fn test_new_summarization_evals_clone_independence() {
        let mut node = Node::default();
        node.id = "summ-clone".to_string();
        node.summarize_evals.push(Eval::default());

        let ellm = EvaluatableLLM::new_summarization(&node);
        // Mutate original after creation
        node.summarize_evals.clear();

        // Ensure EvaluatableLLM kept its own clone
        assert_eq!(ellm.evals.len(), 1);
        assert_eq!(ellm.id, format!("{}.summarize-llm", node.id));
    }

    #[test]
    fn test_triage_and_summarization_ids_are_distinct() {
        let mut node = Node::default();
        node.id = "both-llms".to_string();
        node.triage_evals.push(Eval::default());
        node.summarize_evals.push(Eval::default());

        let triage = EvaluatableLLM::new_triage(&node);
        let summarization = EvaluatableLLM::new_summarization(&node);

        assert!(triage.id.ends_with(".triage-llm"));
        assert!(summarization.id.ends_with(".summarize-llm"));
        assert_ne!(triage.id, summarization.id);
    }
}
