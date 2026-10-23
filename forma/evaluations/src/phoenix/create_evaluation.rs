use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CreateEvalResultResponse {
    pub(crate) id: String,
}

// #[cfg(test)]
// mod integration_test {
//     use crate::{
//         judgement::{Annotator, EvalResult, Judgement},
//         phoenix::PhoenixClient,
//     };

//     use crate::EvalPlatform;
//     use chrono::{TimeDelta, Utc};
//     use tokio;

//     #[tokio::test]
//     async fn test_create_eval_result() -> Result<(), String> {
//         let run_id = "RXhwZXJpbWVudFJ1bjoy";
//         let req = Judgement {
//             experiment_run_id: run_id.to_string(),
//             name: "result-1".to_string(),
//             annotator_kind: Annotator::LLM,
//             start_time: Utc::now(),
//             end_time: Utc::now() + TimeDelta::seconds(2),
//             result: Some(EvalResult {
//                 label: "nice-label".to_string(),
//                 score: 2.0,
//                 explanation: "the result was pretty awesome".to_string(),
//             }),
//             ..Default::default()
//         };
//         let evaluator = PhoenixClient::new();
//         let id = evaluator.push_judgement(&req).await?;
//         println!("Created evaluation result with ID: {id}");
//         Ok(())
//     }
// }
