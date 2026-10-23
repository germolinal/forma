//! An evaluation platform that needs nothing but a directory.
//!
//! Datasets are JSON files—the very same ones `forma tester` writes—and
//! the results of every experiment are written next to them, both as JSON
//! (so they can be processed) and as Markdown (so they can be read).
//!
//! ```text
//! evals/
//!   data/
//!     wanderer.json                        <- a dataset
//!   results/
//!     README.md                            <- every evaluation ever run
//!     2026-08-16_140322_my-experiment/
//!       REPORT.md                          <- what happened in this one
//!       session.json
//!       node-thinking/
//!         experiment.json                  <- every answer, in full
//! ```

pub(crate) mod record;
pub(crate) mod report;
pub(crate) mod session;
pub(crate) mod store;

use async_trait::async_trait;
use chrono::Utc;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

use crate::{
    EvalPlatform,
    dataset::{BrokenDataset, DataPoint, Dataset, DatasetSummary, PostDatasetAction},
    file::{
        record::{DatasetRef, ExperimentRecord, JudgementRecord, RunRecord, Target},
        session::{INDEX_FILE, Session, SessionSummary},
        store::{DatasetStore, assign_ids},
    },
    judgement::{ActualAnswer, Judgement},
};

/// The directory that holds the datasets, within the evaluations directory
const DATA_DIR: &str = "data";

/// The directory that holds the results, within the evaluations directory
const RESULTS_DIR: &str = "results";

/// Writes a file in a way that cannot leave a half written one behind:
/// the contents go to a temporary file first, which is then renamed.
pub(crate) fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("'{}' is not a file name", path.display()))?;
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, contents)
        .map_err(|e| format!("could not write '{}': {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not write '{}': {e}", path.display()))
}

/// Where a run is kept, so that a judgement can find the answer it judges
#[derive(Debug, Clone, Copy)]
struct RunLocation {
    experiment: usize,
    run: usize,
}

#[derive(Default)]
struct State {
    /// The session being written, created when the first experiment is
    session: Option<Session>,
    /// The datasets that have been served, kept so that answers and
    /// judgements can be reported next to the question that caused them
    datasets: HashMap<String, Dataset>,
    /// Where to find each run that has been recorded
    runs: HashMap<String, RunLocation>,
    /// The sessions that were already there when this one started. Nothing
    /// else is writing to the directory, so they are read only once.
    previous: Vec<SessionSummary>,
}

/// An evaluation platform backed by the filesystem.
///
/// The directory it works on is defined by the `FORMA_EVAL_DIR`
/// environment variable, and defaults to `./evals`.
pub struct FileClient {
    root: PathBuf,
    state: Mutex<State>,
}

impl Default for FileClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FileClient {
    /// Creates a client that works on a given directory
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            state: Mutex::new(State::default()),
        }
    }

    /// The directory this client is working on
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn store(&self) -> DatasetStore {
        DatasetStore::new(self.root.join(DATA_DIR))
    }

    fn results_dir(&self) -> PathBuf {
        self.root.join(RESULTS_DIR)
    }

    /// Reads a dataset into the cache—if it is not there already—and
    /// returns how many examples it has.
    fn cache_dataset(&self, state: &mut State, name: &str) -> Result<usize, String> {
        if !state.datasets.contains_key(name) {
            let mut dataset = self.store().find(name)?.dataset;
            // Answers and judgements refer to examples by ID, so every
            // example needs one before the dataset is handed out.
            assign_ids(&mut dataset);
            state.datasets.insert(name.to_string(), dataset);
        }
        Ok(state.datasets[name].data.len())
    }

    /// The example a given answer was answering
    fn example<'a>(state: &'a State, dataset: &str, example_id: &str) -> Option<&'a DataPoint> {
        state
            .datasets
            .get(dataset)?
            .data
            .iter()
            .find(|point| point.id.as_deref() == Some(example_id))
    }

    /// Rewrites the index of every session that has ever run
    fn write_index(&self, state: &State) -> Result<(), String> {
        let mut sessions = Vec::with_capacity(state.previous.len() + 1);
        if let Some(session) = &state.session {
            sessions.push(session.summary());
        }
        sessions.extend(state.previous.iter().cloned());
        write_file(
            &self.results_dir().join(INDEX_FILE),
            &report::index(&sessions),
        )
    }

    /// Saves everything that has changed: the experiment, the report of
    /// this session, and the index.
    fn persist(&self, state: &mut State, touched: usize) -> Result<(), String> {
        let session = state
            .session
            .as_mut()
            .ok_or_else(|| "no evaluation session has been started".to_string())?;
        session.updated_at = Utc::now();
        session.persist(Some(touched))?;
        self.write_index(state)
    }

    /// Finds the experiment a given ID refers to
    fn position_of(state: &State, experiment_id: &str) -> Result<usize, String> {
        state
            .session
            .as_ref()
            .and_then(|s| s.position_of(experiment_id))
            .ok_or_else(|| format!("experiment '{experiment_id}' was not created by this run"))
    }
}

/// The evaluation runner names experiments after the element being
/// evaluated, as in `[node thinking] my first experiment`. This splits
/// that back into the element and the name the user actually typed.
fn split_experiment_name(decorated: &str) -> (Option<Target>, String) {
    let parsed = decorated
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .and_then(|(target, name)| {
            let (class, id) = target.split_once(' ')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((
                Target {
                    class: class.to_string(),
                    id: id.to_string(),
                },
                name.to_string(),
            ))
        });
    match parsed {
        Some((target, name)) => (Some(target), name),
        None => (None, decorated.to_string()),
    }
}

#[async_trait]
impl EvalPlatform for FileClient {
    fn new() -> Self {
        let root = std::env::var("FORMA_EVAL_DIR").unwrap_or_else(|_| "./evals".to_string());
        Self::with_root(root)
    }

    async fn set_requirements(
        &self,
        experiment_id: &str,
        requirements: &BTreeMap<String, f64>,
    ) -> Result<(), String> {
        if requirements.is_empty() {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let position = Self::position_of(&state, experiment_id)?;
        let session = state
            .session
            .as_mut()
            .ok_or_else(|| "no evaluation session has been started".to_string())?;
        let experiment = &mut session.experiments[position];
        experiment.requirements = requirements.clone();
        // Summarised again so that a requirement is visible in the report
        // from the start, even if this experiment never records anything
        experiment.summarize();
        self.persist(&mut state, position)?;
        Ok(())
    }

    async fn push_judgement(&self, req: &Judgement) -> Result<String, String> {
        let mut state = self.state.lock().await;
        let location = *state.runs.get(&req.experiment_run_id).ok_or_else(|| {
            format!(
                "cannot judge run '{}': it was not recorded by this run",
                req.experiment_run_id
            )
        })?;

        let id = {
            let session = state
                .session
                .as_mut()
                .ok_or_else(|| "no evaluation session has been started".to_string())?;
            let experiment = &mut session.experiments[location.experiment];
            let run = &mut experiment.runs[location.run];
            run.judgements.push(JudgementRecord::from(req));
            let id = format!("{}/{}", run.id, req.name);
            experiment.summarize();
            id
        };

        self.persist(&mut state, location.experiment)?;
        Ok(id)
    }

    async fn push_actual_answer(
        &self,
        experiment_id: &str,
        req: &ActualAnswer,
    ) -> Result<String, String> {
        let mut state = self.state.lock().await;
        let position = Self::position_of(&state, experiment_id)?;

        let dataset_name = state
            .session
            .as_ref()
            .map(|s| s.experiments[position].dataset.name.clone())
            .unwrap_or_default();
        let example = Self::example(&state, &dataset_name, &req.dataset_example_id);

        let run_number = state
            .session
            .as_ref()
            .map(|s| s.experiments[position].runs.len())
            .unwrap_or_default();
        let id = format!("{experiment_id}#{:04}", run_number + 1);
        let record = RunRecord::new(id.clone(), req, example);

        let session = state
            .session
            .as_mut()
            .ok_or_else(|| "no evaluation session has been started".to_string())?;
        let experiment = &mut session.experiments[position];
        experiment.runs.push(record);
        experiment.summarize();

        state.runs.insert(
            id.clone(),
            RunLocation {
                experiment: position,
                run: run_number,
            },
        );
        self.persist(&mut state, position)?;
        Ok(id)
    }

    async fn list_datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        Ok(self
            .store()
            .list()?
            .datasets
            .iter()
            .map(|found| found.summary())
            .collect())
    }

    async fn broken_datasets(&self) -> Vec<BrokenDataset> {
        // A directory that cannot even be read is not a broken dataset,
        // and whoever asks for one will hear about it soon enough
        self.store().list().map(|l| l.broken).unwrap_or_default()
    }

    async fn describe_dataset(&self, dataset_name: &str) -> Result<DatasetSummary, String> {
        Ok(self.store().find(dataset_name)?.summary())
    }

    async fn get_dataset(&self, dataset_name: &str) -> Result<Dataset, String> {
        let mut state = self.state.lock().await;
        self.cache_dataset(&mut state, dataset_name)?;
        Ok(state.datasets[dataset_name].clone())
    }

    async fn create_experiment(
        &self,
        dataset_name: &str,
        experiment_name: String,
        description: Option<String>,
    ) -> Result<String, String> {
        let mut state = self.state.lock().await;
        let example_count = self.cache_dataset(&mut state, dataset_name)?;
        let (target, name) = split_experiment_name(&experiment_name);

        if state.session.is_none() {
            let results = self.results_dir();
            std::fs::create_dir_all(&results)
                .map_err(|e| format!("could not create '{}': {e}", results.display()))?;
            // Read what is already there before adding to it
            state.previous = SessionSummary::read_all(&results);
            let session = Session::create(&results, &name, Utc::now())?;
            println!(
                "writing evaluation report to {}",
                session.report_path().display()
            );
            state.session = Some(session);
        }

        let session = state
            .session
            .as_mut()
            .ok_or_else(|| "no evaluation session has been started".to_string())?;
        let id = session.unique_id(&match &target {
            Some(t) => format!("{} {}", t.class, t.id),
            None => name.clone(),
        });
        session.experiments.push(ExperimentRecord {
            id: id.clone(),
            experiment: name,
            description,
            target,
            dataset: DatasetRef {
                name: dataset_name.to_string(),
                example_count,
            },
            created_at: Utc::now(),
            ..Default::default()
        });

        let position = session.experiments.len() - 1;
        self.persist(&mut state, position)?;
        Ok(id)
    }

    async fn create_dataset(
        &self,
        dataset: Dataset,
        action: PostDatasetAction,
    ) -> Result<String, String> {
        let store = self.store();
        let mut dataset = dataset;
        if action == PostDatasetAction::Append
            && let Ok(found) = store.find(&dataset.name)
        {
            let mut merged = found.dataset;
            merged.data.extend(dataset.data);
            merged.metadata.extend(dataset.metadata);
            if merged.description.is_none() {
                merged.description = dataset.description;
            }
            dataset = merged;
        }
        let path = store.write(&dataset)?;

        // Whatever was served before is now out of date
        let mut state = self.state.lock().await;
        state.datasets.remove(&dataset.name);

        Ok(path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgement::{Annotator, EvalResult};
    use chrono::TimeDelta;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("forma-file-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn dataset(name: &str) -> Dataset {
        Dataset {
            name: name.to_string(),
            description: Some("A test dataset".into()),
            data: vec![
                DataPoint {
                    id: None,
                    input: "Hi".into(),
                    expected_output: None,
                },
                DataPoint {
                    id: None,
                    input: "Who are you?".into(),
                    expected_output: Some("A helpful assistant".into()),
                },
            ],
            metadata: vec![],
        }
    }

    fn answer(example_id: &str, output: &str) -> ActualAnswer {
        let start = Utc::now();
        ActualAnswer {
            dataset_example_id: example_id.to_string(),
            output: output.into(),
            repetition_number: 1,
            start_time: start,
            end_time: start + TimeDelta::seconds(1),
            ..Default::default()
        }
    }

    fn judgement(run_id: &str, score: f64, label: &str) -> Judgement {
        let start = Utc::now();
        Judgement {
            experiment_run_id: run_id.to_string(),
            name: "politeness".to_string(),
            annotator_kind: Annotator::LLM,
            start_time: start,
            end_time: start + TimeDelta::seconds(1),
            result: Some(EvalResult {
                label: label.to_string(),
                score,
                explanation: "it was ok".to_string(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn experiment_names_are_split_into_element_and_name() {
        let (target, name) = split_experiment_name("[node thinking] my first experiment");
        let target = target.expect("no target");
        assert_eq!(&target.class, "node");
        assert_eq!(&target.id, "thinking");
        assert_eq!(&name, "my first experiment");

        // Names that do not follow the convention are kept whole
        for odd in [
            "just a name",
            "[node thinking]",
            "[nothing] ",
            "[node] no id",
        ] {
            let (target, name) = split_experiment_name(odd);
            assert!(target.is_none(), "'{odd}' should have no target");
            assert_eq!(&name, odd);
        }
    }

    #[test]
    fn files_are_written_whole() {
        let dir = temp_root("write");
        std::fs::create_dir_all(&dir).expect("could not create dir");
        let path = dir.join("REPORT.md");
        write_file(&path, "hello").expect("could not write");
        write_file(&path, "goodbye").expect("could not overwrite");
        assert_eq!(
            std::fs::read_to_string(&path).expect("could not read"),
            "goodbye"
        );
        // ... and no leftovers
        let files: Vec<String> = std::fs::read_dir(&dir)
            .expect("could not read dir")
            .filter_map(|e| Some(e.ok()?.file_name().to_str()?.to_string()))
            .collect();
        assert_eq!(files, vec!["REPORT.md".to_string()]);
    }

    #[tokio::test]
    async fn datasets_can_be_created_listed_and_read() {
        let client = FileClient::with_root(temp_root("datasets"));
        assert!(
            client
                .list_datasets()
                .await
                .expect("could not list")
                .is_empty()
        );

        let id = client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create");
        assert_eq!(&id, "wanderer");

        let list = client.list_datasets().await.expect("could not list");
        assert_eq!(list.len(), 1);
        assert_eq!(&list[0].name, "wanderer");
        assert_eq!(list[0].example_count, 2);
        assert_eq!(&list[0].description, "A test dataset");

        let summary = client
            .describe_dataset("wanderer")
            .await
            .expect("could not describe");
        assert_eq!(&summary.name, "wanderer");

        let found = client.get_dataset("wanderer").await.expect("could not get");
        assert_eq!(&found.name, "wanderer");
        assert_eq!(found.description.as_deref(), Some("A test dataset"));
        // Examples are given an ID, which is what answers refer to
        assert_eq!(found.data[0].id.as_deref(), Some("ex-0001"));
        assert_eq!(found.data[1].id.as_deref(), Some("ex-0002"));

        // The dataset is a plain JSON file, where one would expect it
        let path = client.root().join("data/wanderer.json");
        assert!(path.is_file());
        let contents = std::fs::read_to_string(&path).expect("could not read");
        assert!(contents.contains("\"Who are you?\""), "{contents}");
    }

    #[tokio::test]
    async fn missing_datasets_are_reported() {
        let client = FileClient::with_root(temp_root("missing"));
        let err = client.get_dataset("nope").await.expect_err("should fail");
        assert_eq!(&err, "dataset 'nope' not found");
        assert!(client.describe_dataset("nope").await.is_err());
        assert!(
            client
                .create_experiment("nope", "[node thinking] x".to_string(), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn appending_adds_to_what_is_already_there() {
        let client = FileClient::with_root(temp_root("append"));
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create");
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Append)
            .await
            .expect("could not append");

        let found = client.get_dataset("wanderer").await.expect("could not get");
        assert_eq!(found.data.len(), 4);

        // ... whereas creating it again replaces it
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create");
        let found = client.get_dataset("wanderer").await.expect("could not get");
        assert_eq!(found.data.len(), 2);
    }

    /// Runs a whole evaluation, the way the evaluation runner does
    async fn evaluate(client: &FileClient, experiment: &str) -> String {
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create dataset");
        client
            .get_dataset("wanderer")
            .await
            .expect("could not get dataset");

        let id = client
            .create_experiment(
                "wanderer",
                format!("[node thinking] {experiment}"),
                Some("a description".to_string()),
            )
            .await
            .expect("could not create experiment");

        let run = client
            .push_actual_answer(&id, &answer("ex-0001", "Hello there!"))
            .await
            .expect("could not push answer");
        client
            .push_judgement(&judgement(&run, 3.0, "PERFECT"))
            .await
            .expect("could not push judgement");

        let run = client
            .push_actual_answer(&id, &answer("ex-0002", "Go away"))
            .await
            .expect("could not push answer");
        client
            .push_judgement(&judgement(&run, 0.0, "HORRIBLE"))
            .await
            .expect("could not push judgement");

        let mut failed = answer("ex-0002", "");
        failed.error = Some("the agent exploded".to_string());
        client
            .push_actual_answer(&id, &failed)
            .await
            .expect("could not push failure");

        id
    }

    #[tokio::test]
    async fn a_requirement_shows_up_in_the_report_of_a_run_that_judged_nothing() {
        let client = FileClient::with_root(temp_root("requirements"));
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create dataset");
        let id = client
            .create_experiment(
                "wanderer",
                "[node thinking] nothing works".to_string(),
                None,
            )
            .await
            .expect("could not create experiment");
        client
            .set_requirements(&id, &BTreeMap::from([("politeness".to_string(), 2.0)]))
            .await
            .expect("could not set the requirements");

        // Every answer failed, so no judgement is ever pushed
        let mut failed = answer("ex-0001", "");
        failed.error = Some("the agent exploded".to_string());
        client
            .push_actual_answer(&id, &failed)
            .await
            .expect("could not push failure");

        let report =
            std::fs::read_to_string(session_dir(&client).join("REPORT.md")).expect("no REPORT.md");
        // The run in which nothing worked is the one that most needs to
        // say that a requirement went unmet
        assert!(report.contains("❌ 1 metric below requirement"), "{report}");
        assert!(report.contains("| politeness | — ❌ | 2.00 |"), "{report}");
    }

    /// The single directory a session was written to
    fn session_dir(client: &FileClient) -> PathBuf {
        let results = client.results_dir();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&results)
            .expect("no results directory")
            .filter_map(|e| {
                let path = e.ok()?.path();
                path.is_dir().then_some(path)
            })
            .collect();
        dirs.sort();
        assert_eq!(dirs.len(), 1, "expected a single session: {dirs:?}");
        dirs.pop().expect("no session")
    }

    #[tokio::test]
    async fn an_experiment_is_written_as_readable_json() {
        let client = FileClient::with_root(temp_root("json"));
        let id = evaluate(&client, "my first experiment").await;
        assert_eq!(&id, "node-thinking");

        let path = session_dir(&client).join("node-thinking/experiment.json");
        let contents = std::fs::read_to_string(&path).expect("no experiment.json");
        let json: serde_json::Value =
            serde_json::from_str(&contents).expect("experiment.json is not JSON");

        assert_eq!(json["id"], "node-thinking");
        assert_eq!(json["experiment"], "my first experiment");
        assert_eq!(json["description"], "a description");
        assert_eq!(json["target"]["class"], "node");
        assert_eq!(json["target"]["id"], "thinking");
        assert_eq!(json["dataset"]["name"], "wanderer");
        assert_eq!(json["dataset"]["example_count"], 2);

        assert_eq!(json["summary"]["runs"], 3);
        assert_eq!(json["summary"]["errors"], 1);
        assert_eq!(json["summary"]["metrics"]["politeness"]["count"], 2);
        assert_eq!(json["summary"]["metrics"]["politeness"]["mean"], 1.5);
        assert_eq!(
            json["summary"]["metrics"]["politeness"]["labels"]["PERFECT"],
            1
        );

        let runs = json["runs"].as_array().expect("no runs");
        assert_eq!(runs.len(), 3);
        // Answers are kept next to the question that caused them
        assert_eq!(runs[0]["id"], "node-thinking#0001");
        assert_eq!(runs[0]["example_id"], "ex-0001");
        assert_eq!(runs[0]["input"], "Hi");
        assert_eq!(runs[0]["output"], "Hello there!");
        assert_eq!(runs[0]["judgements"][0]["name"], "politeness");
        assert_eq!(runs[0]["judgements"][0]["label"], "PERFECT");
        assert_eq!(runs[0]["judgements"][0]["score"], 3.0);
        // ... including the expected answer, when the dataset has one
        assert_eq!(runs[1]["expected_output"], "A helpful assistant");
        assert_eq!(runs[2]["error"], "the agent exploded");
    }

    #[tokio::test]
    async fn a_report_shows_what_happened() {
        let client = FileClient::with_root(temp_root("report"));
        evaluate(&client, "my first experiment").await;

        let report =
            std::fs::read_to_string(session_dir(&client).join("REPORT.md")).expect("no REPORT.md");

        assert!(report.starts_with("# my first experiment\n"), "{report}");
        assert!(report.contains("## Scores"), "{report}");
        assert!(
            report.contains("| node `thinking` | 2 | 1 | 1.50 |"),
            "{report}"
        );
        assert!(report.contains("## Breakdown"), "{report}");
        assert!(report.contains("### node `thinking`\n"), "{report}");
        assert!(
            report.contains("| `wanderer` | 2 | 1 | politeness | 1.50 | — | 0 – 3 |"),
            "{report}"
        );
        assert!(report.contains("HORRIBLE x1, PERFECT x1"), "{report}");

        assert!(
            report.contains("## node `thinking` · dataset `wanderer`"),
            "{report}"
        );
        assert!(report.contains("### ⚠️ ex-0002 — failed"), "{report}");
        assert!(report.contains("### ex-0002 — politeness 0.00"), "{report}");
        assert!(report.contains("> Go away"), "{report}");
        assert!(
            report.contains("- **politeness** — `HORRIBLE` · 0.00"),
            "{report}"
        );
        assert!(report.contains("  > it was ok"), "{report}");
    }

    #[tokio::test]
    async fn every_session_shows_up_in_the_index() {
        let root = temp_root("index");
        let first = FileClient::with_root(&root);
        evaluate(&first, "first try").await;

        let index =
            std::fs::read_to_string(first.results_dir().join("README.md")).expect("no README.md");
        assert!(index.contains("| politeness |"), "{index}");
        assert!(index.contains("[first try](./"), "{index}");
        assert!(
            index.contains("| node `thinking` | 2 | 1 | 1.50 |"),
            "{index}"
        );

        // A second run adds a row without touching the first
        let second = FileClient::with_root(&root);
        evaluate(&second, "second try").await;
        let index =
            std::fs::read_to_string(second.results_dir().join("README.md")).expect("no README.md");
        assert!(index.contains("[first try](./"), "{index}");
        assert!(index.contains("[second try](./"), "{index}");
        let newest = index.find("second try").expect("no second try");
        let oldest = index.find("first try").expect("no first try");
        assert!(newest < oldest, "{index}");
    }

    #[tokio::test]
    async fn several_elements_share_one_session() {
        let client = FileClient::with_root(temp_root("several"));
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create dataset");

        let node = client
            .create_experiment("wanderer", "[node thinking] same run".to_string(), None)
            .await
            .expect("could not create experiment");
        let agent = client
            .create_experiment("wanderer", "[agent dory] same run".to_string(), None)
            .await
            .expect("could not create experiment");
        assert_eq!(&node, "node-thinking");
        assert_eq!(&agent, "agent-dory");

        let dir = session_dir(&client);
        assert!(dir.join("node-thinking/experiment.json").is_file());
        assert!(dir.join("agent-dory/experiment.json").is_file());

        let report = std::fs::read_to_string(dir.join("REPORT.md")).expect("no REPORT.md");
        assert!(report.contains("## node `thinking`"), "{report}");
        assert!(report.contains("## agent `dory`"), "{report}");
        assert!(report.contains("2 evaluated"), "{report}");
    }

    #[tokio::test]
    async fn answers_and_judgements_need_something_to_belong_to() {
        let client = FileClient::with_root(temp_root("orphans"));
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create dataset");

        let err = client
            .push_actual_answer("nope", &answer("ex-0001", "hi"))
            .await
            .expect_err("should fail");
        assert!(err.contains("experiment 'nope'"), "{err}");

        let err = client
            .push_judgement(&judgement("nope#0001", 1.0, "OK"))
            .await
            .expect_err("should fail");
        assert!(err.contains("nope#0001"), "{err}");
    }

    #[tokio::test]
    async fn answers_to_unknown_examples_are_still_recorded() {
        let client = FileClient::with_root(temp_root("unknown-example"));
        client
            .create_dataset(dataset("wanderer"), PostDatasetAction::Create)
            .await
            .expect("could not create dataset");
        let id = client
            .create_experiment("wanderer", "[node thinking] x".to_string(), None)
            .await
            .expect("could not create experiment");

        let run = client
            .push_actual_answer(&id, &answer("who-knows", "hello"))
            .await
            .expect("could not push answer");
        assert_eq!(&run, "node-thinking#0001");

        let path = session_dir(&client).join("node-thinking/experiment.json");
        let contents = std::fs::read_to_string(&path).expect("no experiment.json");
        let json: serde_json::Value = serde_json::from_str(&contents).expect("not JSON");
        assert_eq!(json["runs"][0]["input"], serde_json::Value::Null);
        assert_eq!(json["runs"][0]["output"], "hello");
    }
}
