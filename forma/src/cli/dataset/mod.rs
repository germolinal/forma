pub(crate) mod create;
pub use create::DatasetUploadArgs;

pub(crate) mod list;
pub use list::DatasetListArgs;

pub(crate) mod download;
pub use download::DatasetDownloadArgs;

#[cfg(test)]
mod integration_test {

    use crate::cli::platform::EvalPlatformKind;
    use evaluations::dataset::{DataPoint, Dataset};
    use std::fs::File;
    use std::io::Write;
    use tokio::fs;

    use super::*;

    #[tokio::test]
    async fn test_dataset_actions() {
        let dataset = Dataset {
            name: uuid::Uuid::new_v4().to_string(),
            description: Some("some description".to_string()),
            data: vec![DataPoint {
                id: None,
                input: serde_json::Value::String("hey!".to_string()),
                expected_output: Some(serde_json::Value::String("Hello!".to_string())),
            }],
            metadata: vec![],
        };
        let data_str = serde_json::to_string(&dataset).expect("could not serialize test dataset");
        // write it down in tmp file
        let tmpdir = std::env::temp_dir();
        let mut tmpfile = tmpdir.clone();
        tmpfile.push("dataset.json");
        let mut file = File::create(&tmpfile).expect("could not create tmpfile for dataset");
        file.write_all(data_str.as_bytes())
            .expect("could not write dataset into tmp file");

        // upload.
        let file = tmpfile
            .to_str()
            .expect("could not turn buffer into string")
            .to_string();
        let args = DatasetUploadArgs {
            file,
            action: create::UploadDatasetMode::Create,
            platform: EvalPlatformKind::Phoenix,
        };
        args.upload().await.expect("Could not upload test dataset");

        // List
        let args = DatasetListArgs {
            platform: EvalPlatformKind::Phoenix,
        };
        args.list().await.expect("could not list datasets");

        // Download
        let mut outfile = tmpdir.clone();
        outfile.push("output.json");
        let output_file = outfile
            .to_str()
            .expect("could not turn buffer into string")
            .to_string();
        let args = DatasetDownloadArgs {
            name: dataset.name.clone(),
            output_file: Some(output_file),
            platform: EvalPlatformKind::Phoenix,
        };
        args.download().await.expect("could not download");

        // Check we can read
        let json_data = fs::read_to_string(outfile)
            .await
            .expect("could not read output file");
        let _: Dataset = serde_json::from_str(&json_data).expect("could not serialize file");
    }

    #[test]
    fn test_action_into_action() {}
}
