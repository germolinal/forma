use std::collections::HashMap;

use agent::{Eval, Metric};
use derive::FormaDocs;
use genai_client::GenaiClient;
use genai_client::{
    GenaiClientTrait, JsonSchema,
    openai::v1::{OpenAIResponseFormat, OpenAIResponseFormatContent, OpenAISchemaType},
    schema::SchemaType,
};
use protocols::FormaError;

use crate::api::{ObjectDefinition, template_field::expand_string};
use serde::{Deserialize, Serialize};

fn eval_output_schema() -> OpenAIResponseFormat {
    OpenAIResponseFormat{
        schema_type: OpenAISchemaType::JsonSchema,
        json_schema: OpenAIResponseFormatContent {
            name: "forma-eval-result-schema".to_string(),
            schema: JsonSchema {
                schema_type: Some(SchemaType::Object),
                properties: Some(HashMap::from([
                ("label".to_string(), JsonSchema {
                    schema_type: Some(SchemaType::String),
                    description: Some("A single-word verbal equivalent of the score (e.g., 'Good', 'Bad', 'Hallucination'). Base this value on the instructions provided".to_string()),
                    ..Default::default()
                }),
                ("score".to_string(), JsonSchema {
                    schema_type: Some(SchemaType::Number),
                    description: Some("The numerical value reflecting the quality of the evaluation, assigned as per the instructions".to_string()),
                    ..Default::default()
                }),
                ("explanation".to_string(), JsonSchema {
                    schema_type: Some(SchemaType::String),
                    description: Some("A verbal explanation for the score and labels given".to_string()),
                    ..Default::default()
                })
            ])),
            required: Some(vec![
                "label".to_string(),
                "score".to_string(),
                "explanation".to_string(),
            ]),
            ..Default::default()
            }
        }
    }
}

/// A metric utilized to judge the response given by an AI
/// Agent.
///
/// To scale it properly, the Judge will be an LLM itself
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct MetricDefinition {
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

    /// The mean score this metric must reach. If the mean of the scores
    /// this metric awarded—counted separately for each element evaluated
    /// and each dataset it was evaluated on—falls below this value,
    /// `forma eval` reports the metric as failed and exits with an error.
    ///
    /// Leave it out and the metric never fails the run, but it is still
    /// measured and reported.
    ///
    /// A metric that declares a requirement but that the judge could not
    /// score at all (e.g., the judge LLM failed on every answer) also
    /// fails: an unmeasured requirement has not been met.
    ///
    /// Note that 'not failing' is not necessarily 'ready to ship'.
    /// Judges are not deterministic: the same suite,
    /// re-run with nothing changed, has been seen to move by 0.4 points
    /// on a five-example dataset. A requirement set within the noise of
    /// the current score will fail at random.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_score: Option<f64>,

    /// An ID for the metric.
    pub id: String,
}

impl ObjectDefinition for MetricDefinition {
    fn expand(mut self, parent_dir: &str) -> Result<Self, FormaError> {
        self.template = expand_string(self.template, parent_dir)?;
        Ok(self)
    }
}

impl ObjectDefinition for EvalDefinition {
    fn expand(mut self, parent_dir: &str) -> Result<Self, FormaError> {
        self.metrics = self
            .metrics
            .into_iter()
            .map(|metric| metric.expand(parent_dir))
            .collect::<Result<_, _>>()?;
        Ok(self)
    }
}

impl TryFrom<MetricDefinition> for Metric {
    type Error = FormaError;
    fn try_from(value: MetricDefinition) -> Result<Self, Self::Error> {
        let mut judge: GenaiClient = value.judge;
        judge.set_output_schema(eval_output_schema());

        Ok(Self {
            template: value.template,
            judge,
            required_score: value.required_score,
            id: value.id,
        })
    }
}

impl TryFrom<EvalDefinition> for Eval {
    type Error = FormaError;
    fn try_from(value: EvalDefinition) -> Result<Self, Self::Error> {
        let mut metrics = Vec::with_capacity(value.metrics.len());
        for m in value.metrics.into_iter() {
            metrics.push(m.try_into()?);
        }

        Ok(Self {
            dataset: value.dataset,
            metrics,
        })
    }
}

/// An evaluation is not a complex thing:
///
/// 1. You have a sample question
/// 2. You ask that question to your agent
/// 3. You then you use some metric or rubric to decide whether the answer is good enough or not, and why.
///
/// Explanations of (1) and (3) are provided below
#[derive(Debug, Clone, Serialize, Deserialize, FormaDocs, Default)]
#[serde(deny_unknown_fields)]
pub struct EvalDefinition {
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
    pub metrics: Vec<MetricDefinition>,
}

#[cfg(test)]
mod test {

    use super::*;
    #[test]
    fn eval_schema_json() {
        let schema = eval_output_schema();
        let s = serde_json::to_string_pretty(&schema).unwrap();
        println!("{s}");
    }

    #[test]
    fn a_metric_can_declare_the_score_it_requires() {
        let yaml = "id: some-id\ntemplate: $../evals/metrics/politeness.md\nrequired_score: 2.5";
        let definition: MetricDefinition = serde_yaml::from_str(yaml).expect("valid metric");
        assert_eq!(definition.required_score, Some(2.5));
        let metric: Metric = definition.try_into().expect("could not build the metric");
        assert_eq!(metric.required_score, Some(2.5));
    }

    #[test]
    fn a_metric_that_declares_nothing_requires_nothing() {
        let yaml = "id: some-id\ntemplate: $../evals/metrics/politeness.md";
        let definition: MetricDefinition = serde_yaml::from_str(yaml).expect("valid metric");
        assert_eq!(definition.required_score, None);
        let metric: Metric = definition.try_into().expect("could not build the metric");
        assert_eq!(metric.required_score, None);
    }

    #[test]
    fn expand_resolves_metric_template_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("metric.md"), "judge this").unwrap();
        let metric = MetricDefinition {
            id: "politeness".to_string(),
            judge: GenaiClient::default(),
            template: "$metric.md".to_string(),
            required_score: None,
        };

        let metric = metric.expand(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(metric.template, "judge this");
    }

    #[test]
    fn expand_rejects_missing_metric_template_files() {
        let metric = MetricDefinition {
            id: "politeness".to_string(),
            judge: GenaiClient::default(),
            template: "$missing.md".to_string(),
            required_score: None,
        };
        assert!(metric.expand("/tmp").is_err());
    }
}
