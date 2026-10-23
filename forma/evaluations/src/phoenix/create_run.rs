use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CreateRunResponse {
    pub(crate) id: String,
}

// #[cfg(test)]
// mod integration_test {

//     use crate::EvalPlatform;
//     use crate::judgement::ActualAnswer;
//     use crate::phoenix::PhoenixClient;
//     use chrono::{TimeDelta, Utc};
//     use tokio;

//     #[tokio::test]
//     async fn test_create_run() -> Result<(), String> {
//         let evaluator = PhoenixClient::new();
//         let req = ActualAnswer {
//             dataset_example_id: "RGF0YXNldTV4YW1wbGU6Nw==".into(),
//             output: "some beautiful output".into(),
//             repetition_number: 1,
//             start_time: Utc::now(),
//             end_time: Utc::now() + TimeDelta::seconds(2),
//             ..Default::default()
//         };
//         let id = evaluator
//             .push_actual_answer("RXhwZXJpbWVudDoy", &req)
//             .await?;
//         println!("Created run with ID: {id}");
//         Ok(())
//     }
// }
