use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::{
    dataset::DataPoint,
    judgement::{ActualAnswer, Annotator, Judgement},
};

/// The element being evaluated (e.g., the Node called 'thinking').
///
/// This is `None` when the experiment name does not follow the
/// `[<class> <id>] <name>` convention used by the evaluation runner.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct Target {
    pub class: String,
    pub id: String,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} `{}`", self.class, self.id)
    }
}

/// The dataset an experiment was run against
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct DatasetRef {
    pub name: String,
    pub example_count: usize,
}

/// One metric applied to one run (e.g., 'politeness' said this answer is 'OK')
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct JudgementRecord {
    pub name: String,
    pub annotator_kind: Annotator,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The mean score this metric is required to reach, if it declares one
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub required_score: Option<f64>,
    pub duration_ms: i64,
}

impl From<&Judgement> for JudgementRecord {
    fn from(j: &Judgement) -> Self {
        let (label, score, explanation) = match &j.result {
            Some(r) => (
                Some(r.label.clone()),
                Some(r.score),
                Some(r.explanation.clone()),
            ),
            None => (None, None, None),
        };
        Self {
            name: j.name.clone(),
            annotator_kind: j.annotator_kind,
            label,
            score,
            explanation,
            error: j.error.clone(),
            required_score: j.required_score,
            duration_ms: (j.end_time - j.start_time).num_milliseconds(),
        }
    }
}

/// What the agent answered when asked one example of the dataset,
/// and how that answer was judged.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RunRecord {
    pub id: String,
    pub example_id: String,
    pub input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_output: Option<Value>,
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub judgements: Vec<JudgementRecord>,
}

impl RunRecord {
    /// Builds a record out of the answer given by the agent, filling in the
    /// question that was asked—which the answer itself does not carry—from
    /// the dataset example it came from.
    pub fn new(id: String, answer: &ActualAnswer, example: Option<&DataPoint>) -> Self {
        let (input, expected_output) = match example {
            Some(e) => (e.input.clone(), e.expected_output.clone()),
            None => (Value::Null, None),
        };
        Self {
            id,
            example_id: answer.dataset_example_id.clone(),
            input,
            expected_output,
            output: answer.output.clone(),
            error: answer.error.clone(),
            started_at: answer.start_time,
            duration_ms: (answer.end_time - answer.start_time).num_milliseconds(),
            judgements: Vec::new(),
        }
    }

    /// Whether at least one metric could not judge this answer (e.g., the
    /// judge LLM failed). The agent may well have answered perfectly.
    pub fn judge_failed(&self) -> bool {
        self.judgements.iter().any(|j| j.error.is_some())
    }

    /// The lowest score awarded to this run, used for sorting the
    /// report so that the worst answers show up first.
    pub fn worst_score(&self) -> Option<f64> {
        self.judgements
            .iter()
            .filter_map(|j| j.score)
            .fold(None, |acc: Option<f64>, s| {
                Some(acc.map_or(s, |a| a.min(s)))
            })
    }
}

/// Aggregated results of one metric over a whole experiment.
///
/// Scores are *not* normalised: each metric template defines its own scale
/// (e.g., 'politeness' goes from 0 to 3), so this reports the range that was
/// actually observed rather than a pass rate against a threshold of its own
/// invention. The only threshold here is the one the metric declared for
/// itself, in `required_score`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct MetricSummary {
    pub count: usize,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    /// How many times each label was awarded (e.g., 'PERFECT' three times)
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub labels: BTreeMap<String, usize>,
    /// Judgements that could not be produced (e.g., the judge LLM failed)
    #[serde(skip_serializing_if = "is_zero", default)]
    pub errors: usize,
    /// The mean this metric declared it had to reach, if it declared one
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub required: Option<f64>,
    /// How many of the experiments merged into this figure did not meet
    /// that requirement.
    ///
    /// This is counted, rather than recomputed from `mean`, because the
    /// requirement is checked per experiment—one element, one dataset—and
    /// merging is what would hide it: a dataset that fell short can be
    /// pulled back over the line by one that did well, which is exactly
    /// the cancellation the breakdown tables exist to expose.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub failures: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl MetricSummary {
    fn add(&mut self, judgement: &JudgementRecord) {
        // Before anything else: a judgement that failed still knows what
        // was required of it, and a metric nothing could score is exactly
        // the case that must not be reported as met.
        if judgement.required_score.is_some() {
            self.required = judgement.required_score;
        }
        if judgement.error.is_some() {
            self.errors += 1;
        }
        if let Some(label) = &judgement.label {
            *self.labels.entry(label.clone()).or_default() += 1;
        }
        let Some(score) = judgement.score else {
            return;
        };
        if self.count == 0 {
            self.min = score;
            self.max = score;
            self.mean = score;
        } else {
            self.min = self.min.min(score);
            self.max = self.max.max(score);
            let total = self.mean * self.count as f64 + score;
            self.mean = total / (self.count + 1) as f64;
        }
        self.count += 1;
    }

    /// Merges another summary of the same metric into this one, so that
    /// several experiments can be reported as a single figure.
    pub fn merge(&mut self, other: &MetricSummary) {
        for (label, n) in other.labels.iter() {
            *self.labels.entry(label.clone()).or_default() += n;
        }
        self.errors += other.errors;
        self.failures += other.failures;
        // The same metric can be used on several datasets with different
        // requirements. The strictest is the one this figure is shown
        // against, so that a merged row never looks easier to pass than
        // the rows it merges.
        self.required = match (self.required, other.required) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            self.min = other.min;
            self.max = other.max;
            self.mean = other.mean;
        } else {
            self.min = self.min.min(other.min);
            self.max = self.max.max(other.max);
            let total = self.mean * self.count as f64 + other.mean * other.count as f64;
            self.mean = total / (self.count + other.count) as f64;
        }
        self.count += other.count;
    }

    /// Whether this metric met the requirement it set for itself.
    ///
    /// `None` when it declared none: there is nothing it could fail. A
    /// metric that declared one but that could not be scored at all has
    /// *not* met it—an unmeasured requirement is not a satisfied one, and
    /// reporting it as passed is how a run in which every judge failed
    /// would otherwise be read as a good one.
    pub fn met_requirement(&self) -> Option<bool> {
        let required = self.required?;
        Some(self.count > 0 && self.mean >= required)
    }

    /// Records whether the requirement was met. Called once the summary
    /// holds every judgement, because that verdict is a property of the
    /// finished figure, not of any one judgement in it.
    fn finish(&mut self) {
        self.failures = usize::from(!self.met_requirement().unwrap_or(true));
    }

    /// The labels awarded, most frequent first (e.g., `PERFECT x3, OK x1`)
    pub fn label_tally(&self) -> String {
        let mut labels: Vec<(&String, &usize)> = self.labels.iter().collect();
        labels.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        labels
            .iter()
            .map(|(label, n)| format!("{label} x{n}"))
            .collect::<Vec<String>>()
            .join(", ")
    }
}

/// The headline numbers of an experiment
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ExperimentSummary {
    /// Every answer recorded, whether or not it worked out
    pub runs: usize,
    /// Runs the agent could not answer
    pub errors: usize,
    /// Runs the agent answered but at least one metric could not judge
    #[serde(skip_serializing_if = "is_zero", default)]
    pub judge_errors: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub metrics: BTreeMap<String, MetricSummary>,
}

impl ExperimentSummary {
    /// The runs that actually measured something: the agent answered *and*
    /// every metric managed to judge that answer. `errors` and
    /// `judge_errors` count disjoint sets of runs, so this is exact.
    pub fn succeeded(&self) -> usize {
        self.runs.saturating_sub(self.errors + self.judge_errors)
    }

    /// How many metrics did not meet the requirement they declared.
    ///
    /// Metrics that declared none are not counted: they cannot fail.
    pub fn failures(&self) -> usize {
        self.metrics.values().map(|m| m.failures).sum()
    }
}

/// One element (e.g., a Node) evaluated against one dataset.
///
/// This is what gets written to `experiment.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ExperimentRecord {
    pub id: String,
    /// The name given by the user (e.g., through `forma eval -e`)
    pub experiment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    pub dataset: DatasetRef,
    /// What each metric of this experiment requires of it, by metric name.
    ///
    /// Kept apart from the judgements so that a metric can be reported
    /// against its requirement even when it produced no judgement at all.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub requirements: BTreeMap<String, f64>,
    pub created_at: DateTime<Utc>,
    pub summary: ExperimentSummary,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub runs: Vec<RunRecord>,
}

impl ExperimentRecord {
    /// Recalculates `summary` from the runs recorded so far.
    ///
    /// Judgements arrive one by one and out of order, so this runs after
    /// every push instead of trying to update the aggregates in place.
    pub fn summarize(&mut self) {
        let mut summary = ExperimentSummary {
            runs: self.runs.len(),
            ..Default::default()
        };
        for run in self.runs.iter() {
            if run.error.is_some() {
                summary.errors += 1;
            } else if run.judge_failed() {
                summary.judge_errors += 1;
            }
            for judgement in run.judgements.iter() {
                summary
                    .metrics
                    .entry(judgement.name.clone())
                    .or_default()
                    .add(judgement);
            }
        }
        // A metric that declared a requirement is reported whether or not
        // it was ever judged. Without this, the run in which nothing
        // worked—no answers, so no judgements—would be the one run whose
        // report mentions no failures at all.
        for (name, required) in self.requirements.iter() {
            summary.metrics.entry(name.clone()).or_default().required = Some(*required);
        }
        for metric in summary.metrics.values_mut() {
            metric.finish();
        }
        self.summary = summary;
    }

    /// How this experiment should be titled in a report
    pub fn title(&self) -> String {
        match &self.target {
            Some(t) => format!("{t} · dataset `{}`", self.dataset.name),
            None => format!("{} · dataset `{}`", self.id, self.dataset.name),
        }
    }

    /// The runs, worst first: failures, then the lowest scores. The point of a
    /// report is to show what needs fixing without scrolling.
    pub fn runs_worst_first(&self) -> Vec<&RunRecord> {
        let mut runs: Vec<&RunRecord> = self.runs.iter().collect();
        runs.sort_by(|a, b| {
            let failed = b.error.is_some().cmp(&a.error.is_some());
            failed
                .then_with(|| match (a.worst_score(), b.worst_score()) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    // Unjudged runs are not interesting; keep them at the bottom
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| a.example_id.cmp(&b.example_id))
        });
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgement::EvalResult;
    use chrono::TimeDelta;

    fn judgement(name: &str, score: f64, label: &str) -> JudgementRecord {
        let start = Utc::now();
        JudgementRecord::from(&Judgement {
            name: name.to_string(),
            start_time: start,
            end_time: start + TimeDelta::seconds(2),
            result: Some(EvalResult {
                label: label.to_string(),
                score,
                explanation: "because".to_string(),
            }),
            ..Default::default()
        })
    }

    #[test]
    fn judgement_record_from_judgement() {
        let j = judgement("politeness", 2.0, "OK");
        assert_eq!(j.name, "politeness");
        assert_eq!(j.score, Some(2.0));
        assert_eq!(j.label.as_deref(), Some("OK"));
        assert_eq!(j.explanation.as_deref(), Some("because"));
        assert!(j.error.is_none());
        assert_eq!(j.duration_ms, 2000);
    }

    #[test]
    fn judgement_record_keeps_errors() {
        let j = JudgementRecord::from(&Judgement {
            name: "politeness".to_string(),
            error: Some("the judge exploded".to_string()),
            ..Default::default()
        });
        assert!(j.score.is_none());
        assert!(j.label.is_none());
        assert_eq!(j.error.as_deref(), Some("the judge exploded"));
    }

    /// The same judgement, from a metric that declared what it needs
    fn required_judgement(name: &str, score: f64, required: f64) -> JudgementRecord {
        JudgementRecord {
            required_score: Some(required),
            ..judgement(name, score, "OK")
        }
    }

    #[test]
    fn a_metric_with_no_requirement_never_fails() {
        let mut s = MetricSummary::default();
        s.add(&judgement("m", 0.0, "HORRIBLE"));
        s.finish();
        // Nothing was asked of it, so there is nothing it could fall below
        assert_eq!(s.met_requirement(), None);
        assert_eq!(s.failures, 0);
    }

    #[test]
    fn a_metric_below_its_requirement_fails() {
        let mut s = MetricSummary::default();
        s.add(&required_judgement("m", 1.0, 2.5));
        s.add(&required_judgement("m", 2.0, 2.5));
        s.finish();
        assert_eq!(s.required, Some(2.5));
        assert_eq!(s.met_requirement(), Some(false));
        assert_eq!(s.failures, 1);
    }

    #[test]
    fn a_metric_that_meets_its_requirement_exactly_passes() {
        let mut s = MetricSummary::default();
        s.add(&required_judgement("m", 2.0, 2.5));
        s.add(&required_judgement("m", 3.0, 2.5));
        s.finish();
        // 2.50 is 'required', not 'must be beaten'
        assert!((s.mean - 2.5).abs() < 1e-9);
        assert_eq!(s.met_requirement(), Some(true));
        assert_eq!(s.failures, 0);
    }

    #[test]
    fn a_requirement_that_could_not_be_measured_is_not_met() {
        let mut s = MetricSummary::default();
        // The judge failed, so it carries the requirement and no score
        s.add(&JudgementRecord {
            name: "m".to_string(),
            error: Some("the judge exploded".to_string()),
            required_score: Some(2.5),
            ..Default::default()
        });
        s.finish();
        assert_eq!(s.count, 0);
        // Unmeasured is not the same as met: this is the case that would
        // otherwise let a run in which nothing worked pass a gate
        assert_eq!(s.met_requirement(), Some(false));
        assert_eq!(s.failures, 1);
    }

    #[test]
    fn merging_keeps_the_failures_and_the_strictest_requirement() {
        let mut good = MetricSummary::default();
        good.add(&required_judgement("m", 3.0, 2.0));
        good.finish();

        let mut bad = MetricSummary::default();
        bad.add(&required_judgement("m", 1.0, 2.5));
        bad.finish();

        good.merge(&bad);
        // The pooled mean is 2.00, which meets neither requirement's
        // reading of it—but what matters is that the dataset that fell
        // short is still counted, rather than averaged away
        assert_eq!(good.failures, 1);
        assert_eq!(good.required, Some(2.5));
    }

    #[test]
    fn a_requirement_is_reported_even_when_nothing_was_judged() {
        // The element failed on every example, so it was never judged:
        // the requirement is all the report has to go on
        let mut exp = ExperimentRecord {
            runs: vec![RunRecord {
                example_id: "ex-0001".to_string(),
                error: Some("the agent exploded".to_string()),
                ..Default::default()
            }],
            requirements: BTreeMap::from([("politeness".to_string(), 2.0)]),
            ..Default::default()
        };
        exp.summarize();

        let metric = exp
            .summary
            .metrics
            .get("politeness")
            .expect("a metric that requires something is always reported");
        assert_eq!(metric.count, 0);
        assert_eq!(metric.required, Some(2.0));
        assert_eq!(exp.summary.failures(), 1);
    }

    #[test]
    fn summarize_flags_a_metric_below_its_requirement() {
        let mut exp = ExperimentRecord {
            runs: vec![RunRecord {
                example_id: "ex-0001".to_string(),
                judgements: vec![required_judgement("politeness", 1.0, 2.0)],
                ..Default::default()
            }],
            ..Default::default()
        };
        exp.summarize();
        assert_eq!(exp.summary.failures(), 1);
        // ... and stays flagged when the summary is rebuilt
        exp.summarize();
        assert_eq!(exp.summary.failures(), 1);
    }

    #[test]
    fn metric_summary_aggregates() {
        let mut s = MetricSummary::default();
        s.add(&judgement("m", 3.0, "PERFECT"));
        s.add(&judgement("m", 1.0, "BAD"));
        s.add(&judgement("m", 2.0, "OK"));
        assert_eq!(s.count, 3);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 3.0);
        assert!((s.mean - 2.0).abs() < 1e-9);
        assert_eq!(s.labels.len(), 3);
    }

    #[test]
    fn metric_summary_ignores_missing_scores() {
        let mut s = MetricSummary::default();
        s.add(&judgement("m", 2.0, "OK"));
        s.add(&JudgementRecord {
            name: "m".to_string(),
            error: Some("failed".to_string()),
            ..Default::default()
        });
        assert_eq!(s.count, 1);
        assert_eq!(s.errors, 1);
        assert!((s.mean - 2.0).abs() < 1e-9);
    }

    #[test]
    fn metric_summary_merge_weighs_by_count() {
        let mut a = MetricSummary::default();
        a.add(&judgement("m", 1.0, "BAD"));
        a.add(&judgement("m", 1.0, "BAD"));
        let mut b = MetricSummary::default();
        b.add(&judgement("m", 4.0, "GOOD"));

        a.merge(&b);
        assert_eq!(a.count, 3);
        assert_eq!(a.min, 1.0);
        assert_eq!(a.max, 4.0);
        assert!((a.mean - 2.0).abs() < 1e-9);
        assert_eq!(a.labels.get("BAD"), Some(&2));
        assert_eq!(a.labels.get("GOOD"), Some(&1));
    }

    #[test]
    fn metric_summary_merge_into_empty() {
        let mut a = MetricSummary::default();
        let mut b = MetricSummary::default();
        b.add(&judgement("m", 3.0, "PERFECT"));
        a.merge(&b);
        assert_eq!(a.count, 1);
        assert_eq!(a.mean, 3.0);
        assert_eq!(a.min, 3.0);
        assert_eq!(a.max, 3.0);

        // ... and merging an empty one changes nothing
        let before = a.mean;
        a.merge(&MetricSummary::default());
        assert_eq!(a.count, 1);
        assert_eq!(a.mean, before);
    }

    #[test]
    fn label_tally_is_ordered_by_frequency() {
        let mut s = MetricSummary::default();
        s.add(&judgement("m", 1.0, "BAD"));
        s.add(&judgement("m", 3.0, "PERFECT"));
        s.add(&judgement("m", 3.0, "PERFECT"));
        assert_eq!(&s.label_tally(), "PERFECT x2, BAD x1");
    }

    fn run_with(example_id: &str, scores: &[f64], error: Option<&str>) -> RunRecord {
        RunRecord {
            example_id: example_id.to_string(),
            error: error.map(|e| e.to_string()),
            judgements: scores
                .iter()
                .map(|s| judgement("politeness", *s, "OK"))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn worst_score_picks_the_minimum() {
        let run = run_with("ex-0001", &[3.0, 1.0, 2.0], None);
        assert_eq!(run.worst_score(), Some(1.0));
        assert!(run_with("ex-0002", &[], None).worst_score().is_none());
    }

    #[test]
    fn summarize_counts_runs_and_errors() {
        let mut exp = ExperimentRecord {
            runs: vec![
                run_with("ex-0001", &[3.0], None),
                run_with("ex-0002", &[1.0], None),
                run_with("ex-0003", &[], Some("boom")),
            ],
            ..Default::default()
        };
        exp.summarize();
        assert_eq!(exp.summary.runs, 3);
        assert_eq!(exp.summary.errors, 1);
        let metric = exp.summary.metrics.get("politeness").expect("no metric");
        assert_eq!(metric.count, 2);
        assert!((metric.mean - 2.0).abs() < 1e-9);
    }

    #[test]
    fn summarize_is_idempotent() {
        let mut exp = ExperimentRecord {
            runs: vec![run_with("ex-0001", &[3.0], None)],
            ..Default::default()
        };
        exp.summarize();
        exp.summarize();
        let metric = exp.summary.metrics.get("politeness").expect("no metric");
        assert_eq!(metric.count, 1);
        assert_eq!(metric.labels.get("OK"), Some(&1));
    }

    #[test]
    fn runs_are_sorted_worst_first() {
        let exp = ExperimentRecord {
            runs: vec![
                run_with("ex-0001", &[3.0], None),
                run_with("ex-0002", &[], None),
                run_with("ex-0003", &[1.0], None),
                run_with("ex-0004", &[], Some("boom")),
            ],
            ..Default::default()
        };
        let sorted: Vec<&str> = exp
            .runs_worst_first()
            .iter()
            .map(|r| r.example_id.as_str())
            .collect();
        assert_eq!(sorted, vec!["ex-0004", "ex-0003", "ex-0001", "ex-0002"]);
    }

    #[test]
    fn title_falls_back_to_the_id() {
        let mut exp = ExperimentRecord {
            id: "some-experiment".to_string(),
            dataset: DatasetRef {
                name: "wanderer".to_string(),
                example_count: 2,
            },
            ..Default::default()
        };
        assert_eq!(&exp.title(), "some-experiment · dataset `wanderer`");
        exp.target = Some(Target {
            class: "node".to_string(),
            id: "thinking".to_string(),
        });
        assert_eq!(&exp.title(), "node `thinking` · dataset `wanderer`");
    }
}
