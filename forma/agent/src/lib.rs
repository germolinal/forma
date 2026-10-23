#![warn(clippy::dbg_macro)]

pub mod agent;
pub mod callable;
pub mod eval_agent;
pub mod node;
pub mod template;
pub mod tool;
pub mod workflow;
use protocols::FormaError;

pub use agent::{Agent, Nodes, Tools, default_agent_id};
pub use callable::{CallContext, Callable, Identifiable};
pub use eval_agent::{Evaluatable, EvaluatableSet, SingleEvaluatable};
use genai_client::{GenaiClient, SSEvent};
pub use node::Node;
use serde::{Deserialize, Serialize};
pub use tool::{Tool, ToolRunner, ToolTrait};
pub use workflow::{Workflow, WorkflowOutput};

/// A metric utilized to judge the response given by an AI
/// Agent.
///
/// To scale it properly, the Judge will be an LLM itself
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    /// The `GenaiClient` that will judge the response according
    /// to the template above.
    #[serde(default)]
    pub judge: GenaiClient,

    /// Metric templates can contain three fields:
    ///
    /// 1. `actual_output` - Which will be replaced with the answer of the agent, node or other.
    /// 2. `input` - Which will be replaced by the input, extracted from the dataset.
    /// 3. `expected_output` - An optional value, that would also be extracted from the example.
    ///
    /// They will produce the following outputs:
    ///
    /// 1. `label` - A single-word verbal equivalent of the score (e.g., 'Good', 'Bad', 'Hallucination'). Base this value on the instructions provided
    /// 2. `score` - The numerical value reflecting the quality of the evaluation, assigned as per the instructions
    /// 3. `explanation` - A verbal explanation for the score and labels given
    ///
    /// For example:
    ///
    /// > Your job is to judge whether this sentence:
    /// >
    /// > {{actual_output}}
    /// >
    /// > (1) a good answer th the following question:
    /// >
    /// > {{input}}
    /// >
    /// > and (2), whether contradicts this reference answer:
    /// >
    /// > {{expected_output}}
    /// >
    /// > Give it a score between 0 and 2 (one point for each
    /// > criteria), explain your reasoning behind the score,
    /// > and indicate whether it is Horrible (0 points), Bad (1 point)
    /// > or Good (2 points)
    pub template: String,

    /// The mean score this metric must reach for the evaluation to
    /// be considered a success. See `MetricDefinition::required_score`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_score: Option<f64>,

    /// An ID for the metric.
    pub id: String,
}

/// An evaluation is not a complex thing:
///
/// 1. You have a sample question
/// 2. You ask that question to your agent
/// 3. You then you use some metric or rubric to decide whether the answer is good enough or not, and why.
///
/// Explanations of (1) and (3) are provided below
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Eval {
    /// The set of questions that will be asked to the agent,
    /// node or workflow in order to evaluate their performance
    pub dataset: String,

    /// The set of metrics to evaluate the answers with.
    /// If none is given, the Agent, Workflow or Node will
    /// still be evaluated, but tis answers will not be judged.
    ///
    /// Metric templates can contain three fields:
    ///
    /// 1. `actual_output` - Which will be replaced with the answer of the agent, node or other.
    /// 2. `input` - Which will be replaced by the input, extracted from the dataset.
    /// 3. `expected_output` - An optional value, that would also be extracted from the example.
    ///
    /// They will produce the following outputs:
    ///
    /// 1. `label` - A single-word verbal equivalent of the score (e.g., 'Good', 'Bad', 'Hallucination'). Base this value on the instructions provided
    /// 2. `score` - The numerical value reflecting the quality of the evaluation, assigned as per the instructions
    /// 3. `explanation` - A verbal explanation for the score and labels given
    ///
    /// For example:
    ///
    /// > Your job is to judge whether this sentence:
    /// >
    /// > {{actual_output}}
    /// >
    /// > (1) a good answer th the following question:
    /// >
    /// > {{input}}
    /// >
    /// > and (2), whether contradicts this reference answer:
    /// >
    /// > {{expected_output}}
    /// >
    /// > Give it a score between 0 and 2 (one point for each
    /// > criteria), explain your reasoning behind the score,
    /// > and indicate whether it is Horrible (0 points), Bad (1 point)
    /// > or Good (2 points)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub metrics: Vec<Metric>,
}
