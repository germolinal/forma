use crate::callable::{CallContext, Callable};
use std::collections::HashMap;

use async_trait::async_trait;
use genai_client::JsonSchema;

use protocols::{FormaDB, FormaError, FormaMessage};

use crate::{
    Workflow,
    template::{extract_template_schema, render_template},
    tool::ToolTrait,
};

impl TryFrom<Workflow> for JsonSchema {
    type Error = FormaError;

    fn try_from(item: Workflow) -> Result<Self, FormaError> {
        let schema: Self = match item.input_prompt {
            // Return empty schema
            None => serde_json::from_str(
                r#"{
                "type": "object",
                "properties": {},
                "required": []
            }"#,
            )?,
            Some(template) => extract_template_schema(&template)?,
        };
        Ok(schema)
    }
}

fn as_session(input: &serde_json::Value) -> Option<HashMap<String, serde_json::Value>> {
    input
        .as_object()
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

async fn restore_messages<DB: FormaDB>(
    ctx: &CallContext<DB>,
    old_messages: Vec<FormaMessage>,
) -> Result<(), FormaError> {
    let mut s = ctx.session.write().await;
    s.messages = old_messages;
    Ok(())
}

async fn get_fresh_context<DB: FormaDB>(
    ctx: &CallContext<DB>,
    input: serde_json::Value,
    input_prompt: &Option<String>,
) -> Result<Vec<FormaMessage>, FormaError> {
    let mut s = ctx.session.write().await;

    let old_messages = s.messages.clone();
    s.messages = Vec::new();
    // Add the first message, if any
    if let Some(q) = input_prompt {
        // Add message, if any
        let data = as_session(&input)
            .ok_or_else(|| "could not transform inputs into map... is it an object?".to_string())?;
        let query = render_template(q, &data)?;
        s.messages.push(query.into());
    }

    Ok(old_messages)
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for Workflow {
    fn check(&self) -> Result<(), FormaError> {
        // Delegate to Callable::check to validate ID + all nodes recursively
        <Workflow as Callable<DB>>::check(self)?;
        // Additionally validate the input_prompt template syntax
        if let Some(prompt) = &self.input_prompt {
            extract_template_schema(prompt)?;
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: CallContext<DB>,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        let old_messages = get_fresh_context(&ctx, input, &self.input_prompt).await?;
        let is_root = false;
        let r = self.run(ctx.clone(), is_root).await;

        restore_messages(&ctx, old_messages).await?;
        r
    }
}

#[cfg(test)]
mod testing {

    use std::sync::Arc;

    use db::NoneDB;
    use genai_client::{SSEvent, openai::v1::OpenAIRole};
    use protocols::DeploymentCredentials;
    use protocols::EncryptedTokens;
    use protocols::PendingToolCall;
    use protocols::SessionData;
    use tokio::sync::{Mutex, RwLock};

    use super::*;
    use crate::callable::CallContext;

    #[tokio::test]
    async fn test_fresh_session() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<SSEvent>();

        let tool_call = PendingToolCall {
            tool_call_id: "the-id".to_string(),
            node_id: "the-node-id".to_string(),
            tool_name: "the-tool-name".to_string(),
            arguments: serde_json::json!({}),
            approved: false,
            from_root: true,
            tool_type: String::new(),
            bundle: None,
        };
        let user_id = "user-123".to_string();
        let agent_id = "agent-1".to_string();
        let channel_id = "channel-1".to_string();

        let original_session = SessionData {
            user_id: user_id.clone(),
            agent_id: agent_id.clone(),
            channel_id: channel_id.clone(),
            pending_tool_calls: vec![tool_call.clone()],
            messages: vec!["Hello!".to_string().into()],
            ..Default::default()
        };
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(original_session)),
            tx,
            job_id: "job-1".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let input = serde_json::json!({});
        let result = get_fresh_context(&ctx, input, &None).await;

        assert!(result.is_ok());
        let _old_messages = result.unwrap();
        let session = ctx.session.read().await;
        assert_eq!(&session.user_id, &user_id);
        assert_eq!(&session.agent_id, &agent_id);
        assert_eq!(&session.channel_id, &channel_id);
        assert_eq!(session.pending_tool_calls.len(), 1);
        assert_eq!(
            &session.pending_tool_calls[0].tool_call_id,
            &tool_call.tool_call_id
        );
        assert_eq!(&session.pending_tool_calls[0].node_id, &tool_call.node_id);
        assert_eq!(
            &session.pending_tool_calls[0].tool_name,
            &tool_call.tool_name
        );
        assert_eq!(
            &session.pending_tool_calls[0].arguments,
            &tool_call.arguments
        );
        assert!(session.messages.is_empty());
    }

    #[tokio::test]
    async fn test_fresh_session_with_template() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<SSEvent>();
        let original_session = SessionData::default();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(original_session)),
            tx,
            job_id: "job-1".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let input = serde_json::json!({"name": "Alice"});
        let template = Some("Hello {{name}}".to_string());
        let result = get_fresh_context(&ctx, input, &template).await;

        assert!(result.is_ok());
        let _old_msgs = result.unwrap();
        let session = ctx.session.read().await;
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content.role(), OpenAIRole::User);
    }

    #[tokio::test]
    async fn test_fresh_session_invalid_input() {
        let original_session = SessionData::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(original_session)),
            tx,
            job_id: "job-1".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };

        let input = serde_json::json!("not an object");
        let template = Some("Hello {{name}}".to_string());
        let result = get_fresh_context(&ctx, input, &template).await;

        assert!(result.is_err());
    }

    // ── <Workflow as ToolTrait>::check ────────────────────────────────────────

    use std::collections::HashSet;

    use crate::{Node, Tool, ToolRunner, tool::TemplateTool};

    fn simple_workflow(wf_id: &str, node_id: &str) -> Workflow {
        Workflow {
            id: wf_id.to_string(),
            plan: vec![vec![Node {
                id: node_id.to_string(),
                ..Default::default()
            }]],
            output: HashSet::from([node_id.to_string()]),
            input_prompt: None,
            evals: vec![],
        }
    }

    #[test]
    fn test_tool_check_valid_workflow_no_prompt() {
        let wf = simple_workflow("wf_one", "node_one");
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_ok());
    }

    #[test]
    fn test_tool_check_valid_workflow_with_prompt() {
        let mut wf = simple_workflow("wf_one", "node_one");
        wf.input_prompt = Some("Do {{task: string \"the task\"}}".to_string());
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_ok());
    }

    #[test]
    fn test_tool_check_invalid_prompt_type() {
        let mut wf = simple_workflow("wf_one", "node_one");
        wf.input_prompt = Some("Do {{task: object \"desc\"}}".to_string());
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_err());
    }

    #[test]
    fn test_tool_check_invalid_workflow_id() {
        let mut wf = simple_workflow("wf_one", "node_one");
        wf.id = "1bad".to_string();
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_err());
    }

    #[test]
    fn test_tool_check_invalid_node_inside_workflow() {
        // A node with an invalid ID — must now be caught via the recursive Callable::check
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![Node {
                id: "1bad_node".to_string(),
                ..Default::default()
            }]],
            output: HashSet::from(["1bad_node".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_err());
    }

    #[test]
    fn test_tool_check_output_references_missing_node() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![Node {
                id: "real_node".to_string(),
                ..Default::default()
            }]],
            output: HashSet::from(["ghost_node".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let err = <Workflow as ToolTrait<NoneDB>>::check(&wf)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost_node"));
    }

    #[test]
    fn test_tool_check_duplicate_node_ids_in_inner_workflow() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![
                vec![Node {
                    id: "dup_node".to_string(),
                    ..Default::default()
                }],
                vec![Node {
                    id: "dup_node".to_string(),
                    ..Default::default()
                }],
            ],
            output: HashSet::from(["dup_node".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_err());
    }

    #[test]
    fn test_tool_check_node_with_invalid_tool_inside_workflow() {
        let bad_tool = Tool {
            name: "1bad_tool".to_string(),
            description: None,
            tool: ToolRunner::Template(TemplateTool {
                templates: vec!["hello".to_string()],
            }),
            confirm_before_calling: false,
            just_for: vec![],
            bundle: None,
        };
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![Node {
                id: "node_one".to_string(),
                tools: vec![bad_tool],
                ..Default::default()
            }]],
            output: HashSet::from(["node_one".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        assert!(<Workflow as ToolTrait<NoneDB>>::check(&wf).is_err());
    }
}
