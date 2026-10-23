//! Renders evaluation results as Markdown, so that they can be read
//! (and reviewed, and diffed) without any service running.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::file::record::{ExperimentRecord, ExperimentSummary, MetricSummary, RunRecord, Target};
use crate::file::session::{REPORT_FILE, Session, SessionSummary};

/// Values longer than this are folded away, so that a report stays
/// readable when the agent is chatty.
const FOLD_ABOVE: usize = 800;

/// Escapes a value so that it can live inside a table cell
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

/// A score, always with the same number of decimals so that a column
/// of them can be compared at a glance
fn score(value: f64) -> String {
    format!("{value:.2}")
}

/// A number with no more precision than it needs (e.g., `1` rather than `1.00`)
fn number(value: f64) -> String {
    let ret = format!("{value:.2}");
    if ret.contains('.') {
        ret.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        ret
    }
}

/// A duration, in whatever unit makes it easiest to read
fn duration(ms: i64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let seconds = ms as f64 / 1000.0;
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    format!(
        "{}m {}s",
        (seconds / 60.0).floor(),
        (seconds % 60.0).round()
    )
}

/// Turns text into a blockquote, optionally indented to sit within a bullet
fn quote(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                format!("{indent}>")
            } else {
                format!("{indent}> {line}")
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Shows a value the way it was written: plain text is quoted, anything
/// else is shown as the JSON it is.
fn value_block(value: &Value) -> String {
    match value {
        Value::String(s) => quote(s, ""),
        Value::Null => "*empty*".to_string(),
        other => {
            let json =
                serde_json::to_string_pretty(other).unwrap_or_else(|_| "<unprintable>".to_string());
            format!("```json\n{json}\n```")
        }
    }
}

/// A titled value, folded away when it is too long to skim
fn labelled_block(label: &str, value: &Value) -> String {
    let block = value_block(value);
    if block.len() > FOLD_ABOVE {
        format!("<details><summary><b>{label}</b></summary>\n\n{block}\n\n</details>\n")
    } else {
        format!("**{label}**\n\n{block}\n")
    }
}

/// The scores a run was given, as they appear next to its title
fn run_scores(run: &RunRecord) -> String {
    let scores: Vec<String> = run
        .judgements
        .iter()
        .map(|j| match j.score {
            Some(s) => format!("{} {}", j.name, score(s)),
            None => format!("{} failed", j.name),
        })
        .collect();
    scores.join(" · ")
}

fn run_section(run: &RunRecord) -> String {
    let mut ret = String::new();
    let scores = run_scores(run);
    let title = match (&run.error, scores.is_empty()) {
        (Some(_), _) => format!("⚠️ {} — failed", run.example_id),
        (None, true) => run.example_id.clone(),
        (None, false) => format!("{} — {scores}", run.example_id),
    };
    ret.push_str(&format!("### {title}\n\n"));
    ret.push_str(&format!("*answered in {}*\n\n", duration(run.duration_ms)));
    ret.push_str(&labelled_block("Input", &run.input));
    ret.push('\n');

    if let Some(expected) = &run.expected_output {
        ret.push_str(&labelled_block("Expected output", expected));
        ret.push('\n');
    }

    match &run.error {
        Some(error) => {
            ret.push_str("**Error**\n\n");
            ret.push_str(&quote(error, ""));
            ret.push_str("\n\n");
        }
        None => {
            ret.push_str(&labelled_block("Answer", &run.output));
            ret.push('\n');
        }
    }

    if !run.judgements.is_empty() {
        ret.push_str("**Judgements**\n\n");
        for judgement in run.judgements.iter() {
            let verdict = match (&judgement.label, judgement.score) {
                (Some(label), Some(s)) => format!("`{label}` · {}", score(s)),
                (Some(label), None) => format!("`{label}`"),
                (None, Some(s)) => score(s),
                (None, None) => "⚠️ failed".to_string(),
            };
            ret.push_str(&format!("- **{}** — {verdict}\n", judgement.name));
            if let Some(explanation) = &judgement.explanation {
                ret.push_str(&format!("{}\n", quote(explanation, "  ")));
            }
            if let Some(error) = &judgement.error {
                ret.push_str(&format!("{}\n", quote(error, "  ")));
            }
        }
        ret.push('\n');
    }

    ret
}

fn experiment_section(experiment: &ExperimentRecord) -> String {
    let mut ret = format!("## {}\n\n", experiment.title());
    let mut notes = String::new();
    if experiment.summary.errors > 0 {
        notes.push_str(&format!(" · {} failed", experiment.summary.errors));
    }
    if experiment.summary.judge_errors > 0 {
        notes.push_str(&format!(
            " · judge failed on {}",
            experiment.summary.judge_errors
        ));
    }
    match experiment.summary.failures() {
        0 => {}
        1 => notes.push_str(" · ❌ 1 metric below requirement"),
        n => notes.push_str(&format!(" · ❌ {n} metrics below requirement")),
    }
    ret.push_str(&format!(
        "*{} of {} examples measured{notes}*\n\n",
        experiment.summary.succeeded(),
        experiment.dataset.example_count
    ));

    if experiment.runs.is_empty() {
        ret.push_str("*No answers were recorded.*\n\n");
        return ret;
    }

    for run in experiment.runs_worst_first() {
        ret.push_str(&run_section(run));
    }
    ret
}

/// One experiment, reduced to what a summary table needs.
///
/// The session report and the index keep experiments in different shapes
/// (`ExperimentRecord` and `ExperimentBrief`); this is what they have in
/// common, so that both are rendered by the same code.
struct Measured<'a> {
    /// The columns that say which run this came from, already escaped: empty
    /// in a session report—there is only one run—and `When`/`Experiment` in
    /// the index, which spans every run ever made.
    lead: Vec<String>,
    /// What was evaluated, e.g. ``node `thinking` ``
    callable: String,
    dataset: String,
    summary: &'a ExperimentSummary,
}

/// How a callable is named in a report: what the runner said was evaluated,
/// or the experiment's own id when it could not tell.
fn callable_name(target: Option<&Target>, id: &str) -> String {
    match target {
        Some(t) => t.to_string(),
        None => format!("`{id}`"),
    }
}

/// The experiments of one session, as the summary tables see them. There is
/// only one run here, so nothing needs to say which one it was.
fn measured_experiments(experiments: &[ExperimentRecord]) -> Vec<Measured<'_>> {
    experiments
        .iter()
        .map(|e| Measured {
            lead: Vec::new(),
            callable: callable_name(e.target.as_ref(), &e.id),
            dataset: e.dataset.name.clone(),
            summary: &e.summary,
        })
        .collect()
}

/// The experiments of every session ever run, each one carrying when it ran
/// and a link to the report it came from
fn measured_sessions(sessions: &[SessionSummary]) -> Vec<Measured<'_>> {
    sessions
        .iter()
        .flat_map(|session| {
            let when = session.created_at.format("%Y-%m-%d %H:%M").to_string();
            let link = format!("[{}](./{}/{REPORT_FILE})", cell(&session.name), session.dir);
            session.experiments.iter().map(move |e| Measured {
                lead: vec![when.clone(), link.clone()],
                callable: callable_name(e.target.as_ref(), &e.id),
                dataset: e.dataset.clone(),
                summary: &e.summary,
            })
        })
        .collect()
}

/// Groups experiments by whatever `key` returns, keeping the order in which
/// each group first shows up—which is newest first in the index, and the
/// order they were run in a session report.
fn group_by<'a, K, F>(measured: &'a [Measured<'a>], key: F) -> Vec<(K, Vec<&'a Measured<'a>>)>
where
    K: PartialEq,
    F: Fn(&Measured<'a>) -> K,
{
    let mut ret: Vec<(K, Vec<&Measured>)> = Vec::new();
    for item in measured.iter() {
        let k = key(item);
        match ret.iter_mut().find(|(seen, _)| *seen == k) {
            Some((_, group)) => group.push(item),
            None => ret.push((k, vec![item])),
        }
    }
    ret
}

/// The metrics of several experiments, merged into a single figure each.
///
/// This is the number the scores table shows, and it is an average over
/// every dataset the metric was used on—which is why the breakdown that
/// follows it splits them apart again.
fn merge_metrics(group: &[&Measured]) -> BTreeMap<String, MetricSummary> {
    let mut ret: BTreeMap<String, MetricSummary> = BTreeMap::new();
    for item in group.iter() {
        for (name, summary) in item.summary.metrics.iter() {
            ret.entry(name.clone()).or_default().merge(summary);
        }
    }
    ret
}

/// Every metric that was applied, in alphabetical order: one column each
fn metric_names<'a>(measured: &'a [Measured<'a>]) -> BTreeSet<&'a str> {
    measured
        .iter()
        .flat_map(|m| m.summary.metrics.keys().map(String::as_str))
        .collect()
}

/// A metric as it appears in a scores table.
///
/// A metric the judge could not always produce is flagged, because a mean
/// taken over fewer answers than were run looks exactly like a healthy one.
/// A metric that fell below the score it declared it needed is flagged
/// too, whether or not that stopped the run: `--no-fail` silences the
/// exit code, never the report.
fn score_cell(metric: Option<&MetricSummary>) -> String {
    let Some(metric) = metric else {
        return "—".to_string();
    };
    let mut marks = String::new();
    if metric.failures > 0 {
        marks.push_str(" ❌");
    }
    if metric.errors > 0 {
        marks.push_str(" ⚠️");
    }
    match (metric.count, marks.is_empty()) {
        // Nothing was measured, and nothing is wrong with that: the
        // metric was not applied here
        (0, true) => "—".to_string(),
        // Nothing was measured, and that is the whole point of the mark
        (0, false) => marks.trim_start().to_string(),
        _ => format!("{}{marks}", score(metric.mean)),
    }
}

/// The table that opens a report: one row per callable, one column per metric.
///
/// Scores are not comparable across metrics—each template defines its own
/// scale—so nothing is added up across a row. Down a column, though, the
/// same callable can be compared from one run to the next, which is what
/// this table is for.
fn scores_table(lead: &[&str], measured: &[Measured]) -> String {
    let metrics = metric_names(measured);

    let mut ret = String::from("|");
    for header in lead.iter() {
        ret.push_str(&format!(" {header} |"));
    }
    ret.push_str(" Callable | Runs | Failed |");
    for metric in metrics.iter() {
        ret.push_str(&format!(" {} |", cell(metric)));
    }
    ret.push_str("\n|");
    for _ in lead.iter() {
        ret.push_str(" --- |");
    }
    ret.push_str(" --- | ---: | ---: |");
    for _ in metrics.iter() {
        ret.push_str(" ---: |");
    }
    ret.push('\n');

    for (_, group) in group_by(measured, |m| (m.lead.clone(), m.callable.clone())) {
        let runs: usize = group.iter().map(|m| m.summary.succeeded()).sum();
        let errors: usize = group.iter().map(|m| m.summary.errors).sum();
        ret.push('|');
        for value in group[0].lead.iter() {
            ret.push_str(&format!(" {value} |"));
        }
        ret.push_str(&format!(
            " {} | {runs} | {errors} |",
            cell(&group[0].callable)
        ));
        let merged = merge_metrics(&group);
        for metric in metrics.iter() {
            ret.push_str(&format!(" {} |", score_cell(merged.get(*metric))));
        }
        ret.push('\n');
    }
    ret
}

/// The tables that follow the scores: for each callable, what every metric
/// made of it on each dataset separately.
///
/// The scores table averages a metric over the datasets it was used on, so
/// a regression on one of them can be cancelled by an improvement on
/// another. This is where that shows up.
fn breakdown_tables(lead: &[&str], measured: &[Measured]) -> String {
    let mut ret = String::new();
    for (callable, group) in group_by(measured, |m| m.callable.clone()) {
        ret.push_str(&format!("### {callable}\n\n"));

        ret.push('|');
        for header in lead.iter() {
            ret.push_str(&format!(" {header} |"));
        }
        ret.push_str(
            " Dataset | Runs | Failed | Metric | Mean | Required | Range | Labels | Warning |\n|",
        );
        for _ in lead.iter() {
            ret.push_str(" --- |");
        }
        ret.push_str(" --- | ---: | ---: | --- | ---: | ---: | --- | --- | --- |\n");

        for item in group.iter() {
            let mut head = String::from("|");
            for value in item.lead.iter() {
                head.push_str(&format!(" {value} |"));
            }
            head.push_str(&format!(
                " `{}` | {} | {} |",
                cell(&item.dataset),
                item.summary.succeeded(),
                item.summary.errors
            ));
            if item.summary.metrics.is_empty() {
                ret.push_str(&format!("{head} — | — | — | — | — | — |\n"));
                continue;
            }
            for (name, metric) in item.summary.metrics.iter() {
                let range = if metric.count == 0 {
                    "—".to_string()
                } else {
                    format!("{} – {}", number(metric.min), number(metric.max))
                };
                let mut mean = if metric.count == 0 {
                    "—".to_string()
                } else {
                    score(metric.mean)
                };
                // This is the number the requirement is actually checked
                // against—one element, one dataset, one metric—so this is
                // where a breach is marked.
                if metric.failures > 0 {
                    mean.push_str(" ❌");
                }
                let required = match metric.required {
                    Some(r) => score(r),
                    None => "—".to_string(),
                };
                let labels = match metric.label_tally().as_str() {
                    "" => "—".to_string(),
                    tally => cell(tally),
                };
                let warning = match metric.errors {
                    0 => "—".to_string(),
                    n => format!("⚠️ Judge failed x{n}"),
                };
                ret.push_str(&format!(
                    "{head} {} | {mean} | {required} | {range} | {labels} | {warning} |\n",
                    cell(name)
                ));
            }
        }
        ret.push('\n');
    }
    ret
}

/// The report of a single run of `forma eval`
pub(crate) fn session(session: &Session) -> String {
    let mut ret = format!("# {}\n\n", session.name);

    let runs: usize = session
        .experiments
        .iter()
        .map(|e| e.summary.succeeded())
        .sum();
    let errors: usize = session.experiments.iter().map(|e| e.summary.errors).sum();
    let unjudged: usize = session
        .experiments
        .iter()
        .map(|e| e.summary.judge_errors)
        .sum();
    let judged = match unjudged {
        0 => String::new(),
        n => format!(" · judge failed on {n}"),
    };
    // Always reported, whether or not it stopped the run: a requirement
    // that was not met is worth reviewing even when nobody asked to be
    // stopped by it.
    let failures: usize = session
        .experiments
        .iter()
        .map(|e| e.summary.failures())
        .sum();
    let below = match failures {
        0 => String::new(),
        1 => " · ❌ 1 metric below requirement".to_string(),
        n => format!(" · ❌ {n} metrics below requirement"),
    };
    let elapsed = (session.updated_at - session.created_at).num_milliseconds();
    ret.push_str(&format!(
        "*{} · {} evaluated · {runs} runs · {errors} failed{judged}{below} · {}*\n\n",
        session.created_at.format("%Y-%m-%d %H:%M UTC"),
        session.experiments.len(),
        duration(elapsed)
    ));

    if session.experiments.is_empty() {
        ret.push_str("*Nothing has been evaluated yet.*\n");
        return ret;
    }

    let measured = measured_experiments(&session.experiments);

    ret.push_str("## Scores\n\n");
    ret.push_str(
        "*One row per callable. Each score is the mean of that metric over \
         every dataset it was used on, so read it next to the breakdown below. \
         A ❌ marks a metric that fell below the score it declared it needed, \
         on at least one dataset; a ⚠️ marks one the judge could not always \
         produce.*\n\n",
    );
    ret.push_str(&scores_table(&[], &measured));
    ret.push('\n');

    ret.push_str("## Breakdown\n\n");
    ret.push_str(
        "*The same scores, one dataset at a time. This is where a \
         requirement is checked: one element, one dataset, one metric.*\n\n",
    );
    ret.push_str(&breakdown_tables(&[], &measured));

    for experiment in session.experiments.iter() {
        ret.push_str(&experiment_section(experiment));
    }

    ret.push_str(
        "---\n\n*Every answer and judgement summarised above is also kept, \
         in full, in the `experiment.json` file of each experiment.*\n",
    );
    ret
}

/// The index of every session ever run
pub(crate) fn index(sessions: &[SessionSummary]) -> String {
    let mut ret = String::from("# Evaluation results\n\n");
    if sessions.is_empty() {
        ret.push_str("*No evaluations have been run yet.*\n");
        return ret;
    }
    ret.push_str(
        "One row per callable per run of `forma eval`, most recent first. \
         Open a report to see what was asked, what the agent answered, \
         and what each metric made of it.\n\n",
    );

    let measured = measured_sessions(sessions);

    ret.push_str("## Scores\n\n");
    ret.push_str(
        "*Each score is the mean of that metric over every dataset it was \
         used on in that run, so a regression on one dataset can be hidden \
         here by an improvement on another—the breakdown below splits them \
         apart. A ❌ marks a metric that fell below the score it declared it \
         needed; a ⚠️ marks one the judge could not always produce.*\n\n",
    );
    ret.push_str(&scores_table(&["When", "Experiment"], &measured));
    ret.push('\n');

    ret.push_str("## Breakdown\n\n");
    ret.push_str("*The same scores, one dataset at a time.*\n\n");
    ret.push_str(&breakdown_tables(&["When", "Experiment"], &measured));
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::record::{
        DatasetRef, ExperimentSummary, JudgementRecord, MetricSummary, Target,
    };
    use crate::file::session::{ExperimentBrief, Session};
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn judgement(name: &str, label: &str, value: f64) -> JudgementRecord {
        JudgementRecord {
            name: name.to_string(),
            label: Some(label.to_string()),
            score: Some(value),
            explanation: Some("The answer was\nnot welcoming".to_string()),
            duration_ms: 900,
            ..Default::default()
        }
    }

    fn experiment() -> ExperimentRecord {
        let mut exp = ExperimentRecord {
            id: "node-thinking".to_string(),
            experiment: "my first experiment".to_string(),
            target: Some(Target {
                class: "node".to_string(),
                id: "thinking".to_string(),
            }),
            dataset: DatasetRef {
                name: "wanderer".to_string(),
                example_count: 3,
            },
            created_at: Utc::now(),
            runs: vec![
                RunRecord {
                    id: "node-thinking#0001".to_string(),
                    example_id: "ex-0001".to_string(),
                    input: "What can you do?".into(),
                    output: "Anything!".into(),
                    duration_ms: 1200,
                    judgements: vec![judgement("politeness", "PERFECT", 3.0)],
                    ..Default::default()
                },
                RunRecord {
                    id: "node-thinking#0002".to_string(),
                    example_id: "ex-0002".to_string(),
                    input: "Who are you?".into(),
                    expected_output: Some("A helpful assistant".into()),
                    output: "None of your business".into(),
                    duration_ms: 800,
                    judgements: vec![judgement("politeness", "HORRIBLE", 0.0)],
                    ..Default::default()
                },
                RunRecord {
                    id: "node-thinking#0003".to_string(),
                    example_id: "ex-0003".to_string(),
                    input: "Is anybody there?".into(),
                    error: Some("the agent exploded".to_string()),
                    duration_ms: 50,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        exp.summarize();
        exp
    }

    /// The same experiment, with a requirement `politeness` cannot meet:
    /// it averages 1.50 over the two answers that were judged.
    fn experiment_with_requirement(required: f64) -> ExperimentRecord {
        let mut exp = experiment();
        for run in exp.runs.iter_mut() {
            for judgement in run.judgements.iter_mut() {
                judgement.required_score = Some(required);
            }
        }
        exp.summarize();
        exp
    }

    #[test]
    fn a_metric_below_its_requirement_is_marked_in_every_table() {
        let exp = experiment_with_requirement(2.0);
        assert_eq!(exp.summary.failures(), 1);

        let experiments = [exp.clone()];
        let measured = measured_experiments(&experiments);

        // The scores table marks it next to the mean...
        let scores = scores_table(&[], &measured);
        assert!(
            scores.contains("| node `thinking` | 2 | 1 | 1.50 ❌ |"),
            "{scores}"
        );

        // ... and the breakdown says what was required, on the row where
        // the requirement is actually checked
        let detail = breakdown_tables(&[], &measured);
        assert!(
            detail.contains("| `wanderer` | 2 | 1 | politeness | 1.50 ❌ | 2.00 |"),
            "{detail}"
        );

        // ... and the experiment says so in one line, above the answers
        let section = experiment_section(&exp);
        assert!(
            section.contains("❌ 1 metric below requirement"),
            "{section}"
        );
    }

    #[test]
    fn a_metric_that_meets_its_requirement_is_not_marked() {
        let exp = experiment_with_requirement(1.5);
        assert_eq!(exp.summary.failures(), 0);

        let experiments = [exp.clone()];
        let measured = measured_experiments(&experiments);

        let scores = scores_table(&[], &measured);
        assert!(
            scores.contains("| node `thinking` | 2 | 1 | 1.50 |"),
            "{scores}"
        );
        // The requirement is still shown: it is worth knowing how close
        // to the line a passing metric ran
        let detail = breakdown_tables(&[], &measured);
        assert!(detail.contains("| politeness | 1.50 | 1.50 |"), "{detail}");
        let section = experiment_section(&exp);
        assert!(!section.contains("below requirement"), "{section}");
    }

    #[test]
    fn the_session_header_counts_the_requirements_that_were_not_met() {
        let now = Utc::now();
        let breached = Session {
            dir: std::path::PathBuf::new(),
            name: "my first experiment".to_string(),
            created_at: now,
            updated_at: now,
            experiments: vec![experiment_with_requirement(2.0)],
        };
        let report = session(&breached);
        assert!(report.contains("❌ 1 metric below requirement"), "{report}");

        let met = Session {
            experiments: vec![experiment_with_requirement(1.5)],
            ..breached
        };
        let report = session(&met);
        assert!(!report.contains("below requirement"), "{report}");
    }

    #[test]
    fn numbers_are_formatted_for_reading() {
        assert_eq!(&score(2.4), "2.40");
        assert_eq!(&number(2.0), "2");
        assert_eq!(&number(2.5), "2.5");
        assert_eq!(&number(2.25), "2.25");
        assert_eq!(&duration(120), "120ms");
        assert_eq!(&duration(1500), "1.5s");
        assert_eq!(&duration(90_000), "1m 30s");
    }

    #[test]
    fn table_cells_cannot_break_the_table() {
        assert_eq!(&cell("a | b"), "a \\| b");
        assert_eq!(&cell("two\nlines"), "two<br>lines");
    }

    #[test]
    fn quotes_keep_blank_lines_inside_the_quote() {
        assert_eq!(&quote("one\n\ntwo", ""), "> one\n>\n> two");
        assert_eq!(&quote("one\ntwo", "  "), "  > one\n  > two");
    }

    #[test]
    fn values_are_shown_as_what_they_are() {
        assert_eq!(&value_block(&"hello".into()), "> hello");
        assert_eq!(&value_block(&Value::Null), "*empty*");
        let json = value_block(&serde_json::json!({"a": 1}));
        assert!(json.starts_with("```json"), "{json}");
        assert!(json.contains("\"a\": 1"), "{json}");
    }

    #[test]
    fn long_values_are_folded_away() {
        let short = labelled_block("Answer", &"short".into());
        assert!(short.starts_with("**Answer**"), "{short}");

        let long = labelled_block("Answer", &"x".repeat(FOLD_ABOVE + 1).into());
        assert!(long.starts_with("<details>"), "{}", &long[..40]);
        assert!(long.contains("<b>Answer</b>"));
    }

    #[test]
    fn a_run_shows_question_answer_and_verdict() {
        let exp = experiment();
        let section = run_section(&exp.runs[1]);
        assert!(
            section.contains("### ex-0002 — politeness 0.00"),
            "{section}"
        );
        assert!(section.contains("*answered in 800ms*"), "{section}");
        assert!(section.contains("> Who are you?"), "{section}");
        assert!(section.contains("**Expected output**"), "{section}");
        assert!(section.contains("> A helpful assistant"), "{section}");
        assert!(section.contains("> None of your business"), "{section}");
        assert!(
            section.contains("- **politeness** — `HORRIBLE` · 0.00"),
            "{section}"
        );
        // ... including a multi-line explanation, quoted within its bullet
        assert!(
            section.contains("  > The answer was\n  > not welcoming"),
            "{section}"
        );
    }

    #[test]
    fn a_failed_run_shows_the_error_instead_of_an_answer() {
        let exp = experiment();
        let section = run_section(&exp.runs[2]);
        assert!(section.contains("### ⚠️ ex-0003 — failed"), "{section}");
        assert!(section.contains("**Error**"), "{section}");
        assert!(section.contains("> the agent exploded"), "{section}");
        assert!(!section.contains("**Answer**"), "{section}");
        assert!(!section.contains("**Judgements**"), "{section}");
    }

    #[test]
    fn the_scores_table_has_one_row_per_callable() {
        let experiments = [experiment()];
        let table = scores_table(&[], &measured_experiments(&experiments));
        let rows: Vec<&str> = table.lines().collect();
        assert_eq!(rows.len(), 3, "{table}"); // header, separator, one callable
        assert_eq!(
            rows[0], "| Callable | Runs | Failed | politeness |",
            "{table}"
        );
        // 'Runs' counts only what was measured: 3 answers, 1 of which failed
        assert_eq!(rows[2], "| node `thinking` | 2 | 1 | 1.50 |", "{table}");
    }

    #[test]
    fn the_breakdown_keeps_the_datasets_apart() {
        // The same callable, the same metric, two datasets: 1.00 on one and
        // 2.00 on the other. Pooled, that is 1.50 twice over—which is how a
        // regression on one dataset used to hide behind the other.
        let mut better = experiment();
        better.dataset.name = "facts".to_string();
        better.runs.truncate(1);
        better.runs[0].judgements = vec![judgement("politeness", "PERFECT", 2.0)];
        better.summarize();

        let mut worse = experiment();
        worse.runs.truncate(1);
        worse.runs[0].judgements = vec![judgement("politeness", "HORRIBLE", 1.0)];
        worse.summarize();

        let experiments = [worse, better];
        let measured = measured_experiments(&experiments);

        // One row, because it is one callable...
        let scores = scores_table(&[], &measured);
        assert_eq!(scores.lines().count(), 3, "{scores}");
        assert_eq!(
            scores.lines().nth(2),
            Some("| node `thinking` | 2 | 0 | 1.50 |"),
            "{scores}"
        );

        // ... and the breakdown says where that 1.50 came from
        let detail = breakdown_tables(&[], &measured);
        assert!(detail.contains("### node `thinking`"), "{detail}");
        assert!(
            detail.contains("| `wanderer` | 1 | 0 | politeness | 1.00 | — | 1 – 1 |"),
            "{detail}"
        );
        assert!(
            detail.contains("| `facts` | 1 | 0 | politeness | 2.00 | — | 2 – 2 |"),
            "{detail}"
        );
    }

    #[test]
    fn a_metric_the_judge_could_not_produce_is_flagged() {
        let mut exp = experiment();
        // The agent answered fine; the judge failed on both answers
        for run in exp.runs.iter_mut().filter(|r| r.error.is_none()) {
            run.judgements = vec![JudgementRecord {
                name: "politeness".to_string(),
                error: Some("the judge exploded".to_string()),
                ..Default::default()
            }];
        }
        exp.summarize();

        // Two answers went unjudged, so nothing at all was measured
        assert_eq!(exp.summary.succeeded(), 0);
        assert_eq!(exp.summary.judge_errors, 2);

        let experiments = [exp.clone()];
        let measured = measured_experiments(&experiments);

        // A mean of nothing would read exactly like a mean of zero answers
        // that were all fine, so the scores table says so
        let scores = scores_table(&[], &measured);
        assert!(
            scores.contains("| node `thinking` | 0 | 1 | ⚠️ |"),
            "{scores}"
        );

        let detail = breakdown_tables(&[], &measured);
        assert!(
            detail.contains("| politeness | — | — | — | — | ⚠️ Judge failed x2 |"),
            "{detail}"
        );

        let section = experiment_section(&exp);
        assert!(
            section.contains("*0 of 3 examples measured · 1 failed · judge failed on 2*"),
            "{section}"
        );
    }

    #[test]
    fn a_score_taken_from_fewer_answers_than_were_judged_is_flagged() {
        let mut exp = experiment();
        // One answer judged, one the judge could not judge
        exp.runs[1].judgements = vec![JudgementRecord {
            name: "politeness".to_string(),
            error: Some("the judge exploded".to_string()),
            ..Default::default()
        }];
        exp.summarize();

        let experiments = [exp];
        let scores = scores_table(&[], &measured_experiments(&experiments));
        assert!(scores.contains("| 3.00 ⚠️ |"), "{scores}");
    }

    #[test]
    fn experiments_without_metrics_still_show_up() {
        let mut exp = experiment();
        exp.runs.clear();
        exp.target = None;
        exp.summarize();

        let experiments = [exp.clone()];
        let measured = measured_experiments(&experiments);

        // With nothing measured there is no metric column at all
        let scores = scores_table(&[], &measured);
        assert!(
            scores.contains("| Callable | Runs | Failed |\n"),
            "{scores}"
        );
        assert!(scores.contains("| `node-thinking` | 0 | 0 |"), "{scores}");

        let detail = breakdown_tables(&[], &measured);
        assert!(detail.contains("### `node-thinking`"), "{detail}");
        assert!(
            detail.contains("| `wanderer` | 0 | 0 | — | — | — | — | — |"),
            "{detail}"
        );

        let section = experiment_section(&exp);
        assert!(section.contains("*No answers were recorded.*"), "{section}");
    }

    #[test]
    fn worst_runs_are_reported_first() {
        let section = experiment_section(&experiment());
        let failed = section.find("ex-0003").expect("no ex-0003");
        let worst = section.find("ex-0002").expect("no ex-0002");
        let best = section.find("ex-0001").expect("no ex-0001");
        assert!(failed < worst && worst < best, "{section}");
    }

    fn summary(name: &str, dir: &str, mean: f64, when: &str) -> SessionSummary {
        let mut metrics = BTreeMap::new();
        metrics.insert(
            "politeness".to_string(),
            MetricSummary {
                count: 2,
                mean,
                min: mean,
                max: mean,
                ..Default::default()
            },
        );
        SessionSummary {
            name: name.to_string(),
            dir: dir.to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339(when)
                .expect("bad date")
                .with_timezone(&Utc),
            updated_at: Utc::now(),
            experiments: vec![ExperimentBrief {
                id: "node-thinking".to_string(),
                target: Some(Target {
                    class: "node".to_string(),
                    id: "thinking".to_string(),
                }),
                dataset: "wanderer".to_string(),
                summary: ExperimentSummary {
                    runs: 2,
                    metrics,
                    ..Default::default()
                },
            }],
        }
    }

    #[test]
    fn the_index_links_to_every_report() {
        let sessions = vec![
            summary(
                "second try",
                "2026-08-16_140322_second-try",
                2.5,
                "2026-08-16T14:03:22Z",
            ),
            summary(
                "first try",
                "2026-08-15_090000_first-try",
                1.0,
                "2026-08-15T09:00:00Z",
            ),
        ];
        let index = index(&sessions);
        assert!(
            index.contains("| When | Experiment | Callable | Runs | Failed | politeness |"),
            "{index}"
        );
        assert!(
            index.contains("[second try](./2026-08-16_140322_second-try/REPORT.md)"),
            "{index}"
        );
        // Both runs measured the same callable, so its scores sit in the
        // same column, one row apart
        assert!(
            index.contains("| node `thinking` | 2 | 0 | 2.50 |"),
            "{index}"
        );
        assert!(
            index.contains("| node `thinking` | 2 | 0 | 1.00 |"),
            "{index}"
        );
        // ... and the breakdown gathers them under that callable
        assert!(index.contains("### node `thinking`"), "{index}");
        assert!(
            index.contains("| `wanderer` | 2 | 0 | politeness | 2.50 |"),
            "{index}"
        );
        // The newest one comes first
        let newest = index.find("second try").expect("no second try");
        let oldest = index.find("first try").expect("no first try");
        assert!(newest < oldest, "{index}");
    }

    #[test]
    fn the_index_names_the_metrics_the_judge_could_not_produce() {
        let mut session = summary("all judged", "a", 2.0, "2026-08-16T14:03:22Z");
        // 8 answers, none of which could be judged
        session.experiments[0].summary.runs = 8;
        session.experiments[0].summary.judge_errors = 8;
        session.experiments[0].summary.metrics.insert(
            "factual".to_string(),
            MetricSummary {
                errors: 8,
                ..Default::default()
            },
        );

        let index = index(&[session]);
        // A metric that scored nothing looks exactly like a metric that was
        // never configured, so the scores table marks it...
        assert!(
            index.contains("| node `thinking` | 0 | 0 | ⚠️ | 2.00 |"),
            "{index}"
        );
        // ... and the breakdown says how often the judge failed
        assert!(
            index.contains("| factual | — | — | — | — | ⚠️ Judge failed x8 |"),
            "{index}"
        );
    }

    #[test]
    fn an_empty_index_says_so() {
        let index = index(&[]);
        assert!(
            index.contains("*No evaluations have been run yet.*"),
            "{index}"
        );
    }

    #[test]
    fn a_metric_missing_from_a_session_is_left_blank() {
        let mut with_metric = summary("with", "a", 2.0, "2026-08-16T14:03:22Z");
        let mut without = summary("without", "b", 2.0, "2026-08-15T14:03:22Z");
        without.experiments[0].summary.metrics.clear();
        with_metric.experiments[0]
            .summary
            .metrics
            .insert("accuracy".to_string(), MetricSummary::default());

        let index = index(&[with_metric, without]);
        // Both metrics get a column, in alphabetical order...
        assert!(index.contains("| accuracy | politeness |"), "{index}");
        // ... 'accuracy' was never scored, and the second session has no metrics
        assert!(index.contains("| — | 2.00 |"), "{index}");
        assert!(index.contains("| — | — |"), "{index}");
    }
}
