use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::file::record::{ExperimentRecord, ExperimentSummary, Target};
use crate::file::store::slug;
use crate::file::{report, write_file};

/// The file that describes a session, briefly.
///
/// It exists so that the index of every session ever run can be built
/// without reading each experiment—which carries every answer given by
/// the agent, and is therefore much heavier.
pub(crate) const SESSION_FILE: &str = "session.json";

/// The name of the report of a single session
pub(crate) const REPORT_FILE: &str = "REPORT.md";

/// The name of the index of all the sessions
pub(crate) const INDEX_FILE: &str = "README.md";

/// The headline numbers of a single experiment, as kept in `session.json`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ExperimentBrief {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    pub dataset: String,
    /// Flattened, so that `session.json` keeps the same shape it always had
    /// and the index counts errors exactly like the report does.
    #[serde(flatten)]
    pub summary: ExperimentSummary,
}

impl From<&ExperimentRecord> for ExperimentBrief {
    fn from(exp: &ExperimentRecord) -> Self {
        Self {
            id: exp.id.clone(),
            target: exp.target.clone(),
            dataset: exp.dataset.name.clone(),
            summary: exp.summary.clone(),
        }
    }
}

/// What `session.json` contains
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SessionSummary {
    pub name: String,
    /// The directory of this session, relative to the results directory.
    /// The index uses it to link to the report.
    pub dir: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub experiments: Vec<ExperimentBrief>,
}

impl SessionSummary {
    /// Reads the summary of every session in a results directory,
    /// most recent first.
    ///
    /// Sessions that cannot be read are skipped: an old—or half
    /// written—session should not stop a new one from being reported.
    pub fn read_all(results_dir: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(results_dir) else {
            return Vec::new();
        };
        let mut ret: Vec<Self> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path().join(SESSION_FILE);
                let contents = std::fs::read_to_string(&path).ok()?;
                match serde_json::from_str::<Self>(&contents) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        tracing::warn!("ignoring session '{}': {e}", path.display());
                        None
                    }
                }
            })
            .collect();
        ret.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        ret
    }
}

/// One run of `forma eval`: everything evaluated under a single
/// experiment name, kept in a single directory.
pub(crate) struct Session {
    pub dir: PathBuf,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub experiments: Vec<ExperimentRecord>,
}

impl Session {
    /// Creates the directory this session will be written to.
    ///
    /// It is named after the moment it started and the name the user gave
    /// it, so that sessions sort chronologically and can still be told
    /// apart at a glance.
    pub fn create(results_dir: &Path, name: &str, now: DateTime<Utc>) -> Result<Self, String> {
        let stamp = now.format("%Y-%m-%d_%H%M%S");
        let base = format!("{stamp}_{}", slug(name));
        let mut dir = results_dir.join(&base);
        let mut n = 2;
        while dir.exists() {
            dir = results_dir.join(format!("{base}-{n}"));
            n += 1;
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not create '{}': {e}", dir.display()))?;

        Ok(Self {
            dir,
            name: name.to_string(),
            created_at: now,
            updated_at: now,
            experiments: Vec::new(),
        })
    }

    /// The name of this session's directory, relative to the results directory
    pub fn dir_name(&self) -> String {
        self.dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    }

    /// Where the report of this session can be found
    pub fn report_path(&self) -> PathBuf {
        self.dir.join(REPORT_FILE)
    }

    /// An identifier that no other experiment of this session is using
    pub fn unique_id(&self, base: &str) -> String {
        let base = slug(base);
        let mut id = base.clone();
        let mut n = 2;
        while self.experiments.iter().any(|e| e.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    pub fn position_of(&self, experiment_id: &str) -> Option<usize> {
        self.experiments.iter().position(|e| e.id == experiment_id)
    }

    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            name: self.name.clone(),
            dir: self.dir_name(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            experiments: self.experiments.iter().map(ExperimentBrief::from).collect(),
        }
    }

    /// Writes this session to disk: the experiment that just changed, the
    /// report, and the summary the index is built from.
    ///
    /// This happens after every answer and every judgement, so that a run
    /// that is interrupted half way through still leaves behind everything
    /// it had learnt until then.
    pub fn persist(&self, touched: Option<usize>) -> Result<(), String> {
        let range = match touched {
            Some(i) => i..(i + 1).min(self.experiments.len()),
            None => 0..self.experiments.len(),
        };
        for experiment in &self.experiments[range] {
            let dir = self.dir.join(&experiment.id);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("could not create '{}': {e}", dir.display()))?;
            let contents = serde_json::to_string_pretty(experiment)
                .map_err(|e| format!("could not serialize experiment: {e}"))?;
            write_file(&dir.join("experiment.json"), &contents)?;
        }

        write_file(&self.report_path(), &report::session(self))?;

        let contents = serde_json::to_string_pretty(&self.summary())
            .map_err(|e| format!("could not serialize session: {e}"))?;
        write_file(&self.dir.join(SESSION_FILE), &contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::record::{DatasetRef, ExperimentSummary, MetricSummary};
    use std::collections::BTreeMap;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("forma-session-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("could not create temp dir");
        dir
    }

    fn experiment(id: &str, mean: f64, count: usize) -> ExperimentRecord {
        let mut metrics = BTreeMap::new();
        metrics.insert(
            "politeness".to_string(),
            MetricSummary {
                count,
                mean,
                min: mean,
                max: mean,
                ..Default::default()
            },
        );
        ExperimentRecord {
            id: id.to_string(),
            experiment: "my experiment".to_string(),
            target: Some(Target {
                class: "node".to_string(),
                id: id.to_string(),
            }),
            dataset: DatasetRef {
                name: "wanderer".to_string(),
                example_count: count,
            },
            created_at: Utc::now(),
            summary: ExperimentSummary {
                runs: count,
                metrics,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn session_directories_are_named_after_time_and_name() {
        let results = temp_dir("naming");
        let now = DateTime::parse_from_rfc3339("2026-08-16T14:03:22Z")
            .expect("bad date")
            .with_timezone(&Utc);
        let session = Session::create(&results, "My first experiment!", now).expect("no session");
        assert_eq!(&session.dir_name(), "2026-08-16_140322_my-first-experiment");
        assert!(session.dir.is_dir());

        // A second session started at the very same second gets its own directory
        let other = Session::create(&results, "My first experiment!", now).expect("no session");
        assert_eq!(&other.dir_name(), "2026-08-16_140322_my-first-experiment-2");
    }

    #[test]
    fn experiment_ids_do_not_collide() {
        let results = temp_dir("ids");
        let mut session = Session::create(&results, "e", Utc::now()).expect("no session");
        assert_eq!(&session.unique_id("node thinking"), "node-thinking");
        session
            .experiments
            .push(experiment("node-thinking", 1.0, 1));
        assert_eq!(&session.unique_id("node thinking"), "node-thinking-2");
        assert_eq!(session.position_of("node-thinking"), Some(0));
        assert!(session.position_of("nope").is_none());
    }

    #[test]
    fn persists_experiments_report_and_summary() {
        let results = temp_dir("persist");
        let mut session = Session::create(&results, "my experiment", Utc::now()).expect("session");
        session
            .experiments
            .push(experiment("node-thinking", 2.5, 4));
        session.persist(None).expect("could not persist");

        assert!(session.report_path().is_file());
        assert!(session.dir.join(SESSION_FILE).is_file());
        assert!(session.dir.join("node-thinking/experiment.json").is_file());

        // The summary is flattened into the brief, so `session.json` keeps
        // the flat shape older sessions were written with
        let contents =
            std::fs::read_to_string(session.dir.join(SESSION_FILE)).expect("no session.json");
        let json: serde_json::Value = serde_json::from_str(&contents).expect("not JSON");
        let brief = &json["experiments"][0];
        assert_eq!(brief["dataset"], "wanderer");
        assert_eq!(brief["runs"], 4);
        assert_eq!(brief["errors"], 0);
        assert!(brief["metrics"]["politeness"].is_object(), "{brief}");
        // ... and a session nothing went wrong in gains no new keys
        assert!(brief.get("judge_errors").is_none(), "{brief}");

        let found = SessionSummary::read_all(&results);
        assert_eq!(found.len(), 1);
        assert_eq!(&found[0].name, "my experiment");
        assert_eq!(found[0].experiments[0].summary.succeeded(), 4);
        assert_eq!(found[0].experiments[0].summary.errors, 0);
        assert_eq!(&found[0].dir, &session.dir_name());
    }

    #[test]
    fn persisting_one_experiment_leaves_the_others_alone() {
        let results = temp_dir("touched");
        let mut session = Session::create(&results, "my experiment", Utc::now()).expect("session");
        session.experiments.push(experiment("first", 1.0, 1));
        session.experiments.push(experiment("second", 2.0, 1));
        session.persist(Some(1)).expect("could not persist");

        assert!(!session.dir.join("first/experiment.json").exists());
        assert!(session.dir.join("second/experiment.json").is_file());
    }

    #[test]
    fn the_summary_keeps_every_experiment_apart() {
        let results = temp_dir("merge");
        let mut session = Session::create(&results, "my experiment", Utc::now()).expect("session");
        session.experiments.push(experiment("first", 1.0, 1));
        session.experiments.push(experiment("second", 3.0, 3));

        // Nothing is pooled here: the report decides what to merge, and how
        let summary = session.summary();
        assert_eq!(summary.experiments.len(), 2);
        let means: Vec<f64> = summary
            .experiments
            .iter()
            .map(|e| e.summary.metrics["politeness"].mean)
            .collect();
        assert_eq!(means, vec![1.0, 3.0]);
    }

    #[test]
    fn sessions_are_listed_most_recent_first() {
        let results = temp_dir("sorted");
        let old = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .expect("bad date")
            .with_timezone(&Utc);
        Session::create(&results, "older", old)
            .expect("session")
            .persist(None)
            .expect("could not persist");
        Session::create(&results, "newer", Utc::now())
            .expect("session")
            .persist(None)
            .expect("could not persist");

        let found = SessionSummary::read_all(&results);
        assert_eq!(found.len(), 2);
        assert_eq!(&found[0].name, "newer");
        assert_eq!(&found[1].name, "older");
    }

    #[test]
    fn unreadable_sessions_are_skipped() {
        let results = temp_dir("unreadable");
        Session::create(&results, "good", Utc::now())
            .expect("session")
            .persist(None)
            .expect("could not persist");
        let broken = results.join("broken-session");
        std::fs::create_dir_all(&broken).expect("could not create");
        std::fs::write(broken.join(SESSION_FILE), "{ not json").expect("could not write");

        let found = SessionSummary::read_all(&results);
        assert_eq!(found.len(), 1);
        assert_eq!(&found[0].name, "good");

        // ... and a results directory that does not exist yet is simply empty
        assert!(SessionSummary::read_all(&results.join("nope")).is_empty());
    }
}
