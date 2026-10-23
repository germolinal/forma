#[cfg(all(test, feature = "cli"))]
mod test {

    use genai_client::GenaiClient;

    use crate::api::eval_definition::{EvalDefinition, MetricDefinition};
    use crate::api::node_definition::NodeDefinition;
    use crate::api::tool_definition::{ToolDefinition, ToolRunnerDefinition};
    use crate::api::workflow_definition::WorkflowDefinition;
    use crate::api::{AgentDefinition, InputVariable};

    use crate::server::runtime::{Client, FormaRuntime, Sandbox, Storage, Workspaces};

    const REFERENCE_SUMMARY_TITLE: &str = "# Reference Documentation";

    fn existing_summary(summary_file: &str) -> String {
        // read
        let current_summary =
            std::fs::read_to_string(summary_file).expect("Could not read summary file");
        // clear
        let f = std::fs::File::create(&summary_file)
            .map_err(|e| e.to_string())
            .unwrap();
        f.set_len(0).map_err(|e| e.to_string()).unwrap();
        let current_summary = current_summary
            .split(REFERENCE_SUMMARY_TITLE)
            .next()
            .unwrap();
        current_summary.trim().to_string()
    }

    #[test]
    fn write_docs() {
        // cargo test --package forma --lib -- test::write_docs --exact --nocapture
        let forma_dir = "../docs/src";
        let dir = format!("{forma_dir}/reference_docs");
        std::fs::create_dir_all(&dir).unwrap();
        let mut summary = String::new();
        let summary_file = format!("{forma_dir}/SUMMARY.md");
        let current_summary = existing_summary(&summary_file);

        AgentDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        EvalDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        // `runtime.yaml`, and what it is made of.
        FormaRuntime::print_doc(&dir, &mut summary, 0).unwrap();
        Client::print_doc(&dir, &mut summary, 1).unwrap();
        Storage::print_doc(&dir, &mut summary, 1).unwrap();
        Workspaces::print_doc(&dir, &mut summary, 1).unwrap();
        Sandbox::print_doc(&dir, &mut summary, 1).unwrap();
        GenaiClient::print_doc(&dir, &mut summary, 0).unwrap();
        InputVariable::print_doc(&dir, &mut summary, 0).unwrap();
        MetricDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        NodeDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        ToolDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        ToolRunnerDefinition::print_doc(&dir, &mut summary, 0).unwrap();
        WorkflowDefinition::print_doc(&dir, &mut summary, 0).unwrap();

        let whole_summary = format!(
            "{}\n\n{REFERENCE_SUMMARY_TITLE}\n\n{}",
            current_summary, summary
        );
        std::fs::write(summary_file, whole_summary.as_bytes()).unwrap();
    }
}
