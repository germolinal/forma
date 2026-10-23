use std::collections::{HashMap, HashSet};

use crate::{
    Eval, Node,
    callable::{CallContext, Callable, Identifiable},
    eval_agent::{Evaluatable, EvaluatableSet, SingleEvaluatable},
    template::is_valid_variable_name,
};
use async_trait::async_trait;
use futures::future::join_all;
use protocols::{FormaDB, FormaError, Session, SessionData};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLockReadGuard;
use tracing::{Instrument, info, span};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum WorkflowOutput {
    Text(String),
    Map(HashMap<String, String>),
}

#[derive(Clone, Serialize, Debug, Deserialize)]
pub struct Workflow {
    #[serde(default = "default_workflow_id")]
    pub id: String,
    #[serde(default)]
    pub plan: Vec<Vec<Node>>,
    #[serde(default)]
    pub output: HashSet<String>,
    /// determines the schema and the messsate that will be used
    /// to trigger this workflow execution as a tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_prompt: Option<String>,

    /// The evaluations to run
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub evals: Vec<Eval>,
}

impl Workflow {
    fn gather_single_output(
        &self,
        session: &RwLockReadGuard<'_, SessionData>,
        node_id: &String,
    ) -> Result<serde_json::Value, FormaError> {
        if let Some(value) = session.variables.get(node_id) {
            let value = serde_json::to_value(value).expect("could not serialize AssistantMessage");
            Ok(value)
        } else {
            Err(format!(
                "State has no output registered for node '{node_id}' and it is required as an output"
            )
            .into())
        }
    }

    /// Gathers the output of this workflow
    async fn collect_output(&self, session: Session) -> Result<serde_json::Value, FormaError> {
        let mut ret = serde_json::Map::new();
        let session = session.read().await;

        for node_id in &self.output {
            let value = self.gather_single_output(&session, node_id)?;
            ret.insert(node_id.clone(), value);
        }
        Ok(ret.into())
    }
}

pub fn default_workflow_id() -> String {
    // Make sure it does not start with a number
    let ret = uuid::Uuid::new_v4().to_string();
    let tail = &ret[1..];
    let ret = format!("w{tail}");
    ret
}

impl Default for Workflow {
    fn default() -> Self {
        let node = Node::default();
        let node_id = node.id.clone();
        Self {
            id: default_workflow_id(),
            plan: vec![vec![node]],
            output: HashSet::from([node_id]),
            input_prompt: None,
            evals: vec![],
        }
    }
}

impl Identifiable for Workflow {
    const CLASS: &str = "workflow";

    fn get_id(&self) -> &str {
        &self.id
    }
}

impl Evaluatable for Workflow {
    fn collect_evals(&self, callables: &mut EvaluatableSet) -> Result<(), String> {
        callables
            .workflows
            .insert(self.id.clone(), SingleEvaluatable::new(self));
        for layer in self.plan.iter() {
            for node in layer.iter() {
                // Insert node
                let id = node.id.clone();
                if callables.nodes.contains_key(&id) {
                    return Err(format!("node with id '{id}' already exists in agent"));
                }
                callables.nodes.insert(id, SingleEvaluatable::new(node));
                // Insert contents
                node.collect_evals(callables)?;
            }
        }

        Ok(())
    }

    fn get_evals(&self) -> &[Eval] {
        &self.evals
    }

    fn evals_mut(&mut self) -> &mut Vec<Eval> {
        &mut self.evals
    }
}

#[async_trait]
impl<DB: FormaDB> Callable<DB> for Workflow {
    fn check(&self) -> Result<(), FormaError> {
        is_valid_variable_name(&self.id)?;

        if self.plan.is_empty() {
            return Err(FormaError::Input(format!(
                "workflow '{}' has no nodes in its plan",
                self.id
            )));
        }

        if self.output.is_empty() {
            return Err(FormaError::Input(format!(
                "workflow '{}' has no output nodes",
                self.id
            )));
        }

        // Collect node IDs while checking for duplicates
        let mut seen_ids: HashSet<String> = HashSet::new();
        for node in self.plan.iter().flatten() {
            if !seen_ids.insert(node.id.clone()) {
                return Err(FormaError::Input(format!(
                    "workflow '{}' has duplicate node id '{}'",
                    self.id, node.id
                )));
            }
            <Node as Callable<DB>>::check(node)?;
        }

        // Verify every output ID refers to a node that exists in the plan
        for out_id in &self.output {
            if !seen_ids.contains(out_id) {
                return Err(FormaError::Input(format!(
                    "workflow '{}' lists '{}' as an output but no node with that id exists in the plan",
                    self.id, out_id
                )));
            }
        }

        Ok(())
    }

    async fn run(
        &self,
        ctx: CallContext<DB>,
        is_root: bool,
    ) -> Result<Option<serde_json::Value>, FormaError> {
        let session_id = {
            let s = ctx.session.read().await;
            s.id_string()
                .expect("workflow does not have an id")
                .to_string()
        };

        // An `invoke_workflow` span, in the OpenTelemetry GenAI conventions.
        let span = span!(tracing::Level::INFO,
            Workflow::CLASS,
            otel.name = format!("invoke_workflow {}", self.id),
            gen_ai.operation.name = "invoke_workflow",
            gen_ai.workflow.name = %self.id,
            gen_ai.conversation.id = session_id,
            job_id = %ctx.job_id,
        );

        info!(workflow = %self.id, "running workflow");
        for layer in self.plan.iter() {
            // Only output nodes talk to the user. The others still report
            // their steps and tool calls, but their text is for the nodes that
            // read it, not for the chat.
            let futures = layer.iter().map(|node| {
                let speaks = is_root && self.output.contains(&node.id);
                node.run(ctx.clone(), speaks).instrument(span.clone())
            });
            let results = join_all(futures).await;
            let mut layer_has_none = false;
            for result in results {
                if result?.is_none() {
                    layer_has_none = true;
                }
            }
            if layer_has_none {
                return Ok(None);
            }
        }
        let out = self.collect_output(ctx.session).await?;
        info!(workflow = %self.id, "workflow complete");
        Ok(Some(out))
    }

    fn inner_ids(&self) -> Vec<String> {
        let mut ret = Vec::new();
        ret.push(self.id.clone());
        for layer in self.plan.iter() {
            for node in layer.iter() {
                let newids = <Node as Callable<DB>>::inner_ids(node);
                ret.extend(newids);
            }
        }
        ret
    }
}

#[cfg(test)]
mod testing {
    use super::*;

    use std::{collections::HashMap, sync::Arc};

    use crate::callable::CallContext;
    use crate::tool::TemplateTool;
    use crate::{Callable, Node, Tool, ToolRunner};
    use db::NoneDB;
    use genai_client::openai::v1::{OpenAIMessagePart, OpenAITextPart};
    use genai_client::{AssistantMessage, SSEvent};
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData, SessionId};
    use tokio::sync::{Mutex, RwLock, mpsc};

    #[test]
    fn test_default_workflow_id_format() {
        let id = default_workflow_id();
        assert!(id.starts_with('w'));
        assert_eq!(id.len(), 36);
    }

    #[test]
    fn test_workflow_default_initialization() {
        let workflow = Workflow::default();
        assert!(workflow.id.starts_with('w'));
        assert!(workflow.evals.is_empty());
    }

    #[test]
    fn test_workflow_identifiable() {
        let workflow = Workflow::default();
        assert_eq!(workflow.get_id(), &workflow.id);
    }

    #[test]
    fn test_workflow_evaluatable_evals() {
        let workflow = Workflow::default();
        assert!(workflow.get_evals().is_empty());
    }

    fn make_ctx(
        state: HashMap<String, AssistantMessage>,
    ) -> (CallContext<NoneDB>, mpsc::UnboundedReceiver<SSEvent>) {
        let session = Arc::new(RwLock::new(SessionData {
            id: Some(SessionId("some-session".to_string())),
            cached_messages: state,
            ..Default::default()
        }));
        let (tx, rx) = mpsc::unbounded_channel::<SSEvent>();
        let ctx = CallContext::<NoneDB> {
            user_timezone: None,
            agent_timezone: None,
            session,
            tx,
            job_id: "test-job".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
            user_id: "user_id".into(),
        };
        (ctx, rx)
    }

    /// All nodes in a single layer are pre-resolved; workflow should return Some
    /// with the collected output of both nodes.
    #[tokio::test]
    async fn test_single_layer_all_nodes_run() {
        let node_a = Node {
            id: "node_a".to_string(),
            ..Default::default()
        };
        let node_b = Node {
            id: "node_b".to_string(),
            ..Default::default()
        };

        let workflow = Workflow {
            plan: vec![vec![node_a, node_b]],
            output: HashSet::from(["node_a".to_string(), "node_b".to_string()]),
            ..Default::default()
        };

        let state = HashMap::from([
            (
                "node_a-triage-0".to_string(),
                AssistantMessage {
                    name: Some("node_a".to_string()),
                    content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                        text: "node_a".to_string(),
                        ..Default::default()
                    })],
                    ..Default::default()
                },
            ),
            (
                "node_b-triage-0".to_string(),
                AssistantMessage {
                    name: Some("node_b".to_string()),
                    content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                        text: "node_b".to_string(),
                        ..Default::default()
                    })],
                    ..Default::default()
                },
            ),
        ]);
        let (ctx, _rx) = make_ctx(state);
        let result = workflow.run(ctx, false).await;
        result.clone().unwrap();
        assert!(result.is_ok());
        let out = result.unwrap().expect("expected Some output");
        assert_eq!(out["node_a"], serde_json::json!("node_a"));
        assert_eq!(out["node_b"], serde_json::json!("node_b"));
    }

    /// Nodes across multiple layers are all run; workflow collects output from
    /// the last layer's node.
    #[tokio::test]
    async fn test_multiple_layers_all_nodes_run() {
        let node_a = Node {
            id: "node_a".to_string(),
            ..Default::default()
        };
        let node_b = Node {
            id: "node_b".to_string(),
            ..Default::default()
        };

        let workflow = Workflow {
            plan: vec![vec![node_a], vec![node_b]],
            output: HashSet::from(["node_b".to_string()]),
            ..Default::default()
        };

        let state = HashMap::from([
            (
                "node_a-triage-0".to_string(),
                AssistantMessage {
                    name: Some("node_a".to_string()),
                    content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                        text: "Hello!".to_string(),
                        ..Default::default()
                    })],
                    ..Default::default()
                },
            ),
            (
                "node_b-triage-0".to_string(),
                AssistantMessage {
                    name: Some("node_b".to_string()),
                    content: vec![OpenAIMessagePart::Text(OpenAITextPart {
                        text: "hey hey!".to_string(),
                        ..Default::default()
                    })],
                    ..Default::default()
                },
            ),
        ]);
        let (ctx, _rx) = make_ctx(state);
        let result = workflow.run(ctx, true).await;
        result.clone().unwrap();
        assert!(result.is_ok());
        let out = result.unwrap().expect("expected Some output");
        assert_eq!(out["node_b"], serde_json::json!("hey hey!"));
    }

    /// Serves the same chat completion, saying `text`, to every request, and
    /// returns the endpoint to point an Ollama client at.
    async fn fake_llm(text: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    // Read the headers and the body, so the client is not cut off
                    let mut req = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        req.extend_from_slice(&buf[..n]);
                        let head = String::from_utf8_lossy(&req);
                        let Some(end) = head.find("\r\n\r\n") else {
                            continue;
                        };
                        let length = head[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if req.len() >= end + 4 + length {
                            break;
                        }
                    }
                    let body = serde_json::json!({
                        "id": "chatcmpl-1",
                        "object": "chat.completion",
                        "created": 0,
                        "model": "fake",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": text},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        endpoint
    }

    /// In a chat, only the output nodes' text reaches the user. Every node
    /// still reports its step.
    #[tokio::test]
    async fn test_only_output_nodes_talk_to_the_user() {
        let endpoint = fake_llm("some text").await;
        let llm: genai_client::GenaiClient =
            serde_json::from_value(serde_json::json!({"provider": "ollama", "endpoint": endpoint}))
                .unwrap();
        let node = |id: &str| Node {
            id: id.to_string(),
            llm: llm.clone(),
            ..Default::default()
        };

        let workflow = Workflow {
            plan: vec![vec![node("study")], vec![node("post")]],
            output: HashSet::from(["post".to_string()]),
            ..Default::default()
        };

        let (ctx, mut rx) = make_ctx(HashMap::new());
        let out = workflow
            .run(ctx, true)
            .await
            .unwrap()
            .expect("expected Some output");
        assert_eq!(out["post"], serde_json::json!("some text"));

        let mut steps = Vec::new();
        let mut texts = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                SSEvent::StartStep { id } => steps.push(id),
                SSEvent::TextStart { .. } => texts += 1,
                _ => {}
            }
        }
        assert_eq!(steps, vec!["study".to_string(), "post".to_string()]);
        assert_eq!(texts, 1, "only 'post' should have sent its text");
    }

    #[test]
    fn test_inner_ids_empty_plan() {
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![],
            output: HashSet::new(),
            input_prompt: None,
            evals: vec![],
        };
        let ids = <Workflow as Callable<NoneDB>>::inner_ids(&workflow);
        assert_eq!(ids, vec!["workflow_1"]);
    }

    #[test]
    fn test_inner_ids_single_node() {
        let node = Node {
            id: "node_1".to_string(),
            ..Default::default()
        };
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![vec![node]],
            output: HashSet::from(["node_1".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let ids = <Workflow as Callable<NoneDB>>::inner_ids(&workflow);
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_1".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_inner_ids_multiple_layers() {
        let node_a = Node {
            id: "node_a".to_string(),
            ..Default::default()
        };
        let node_b = Node {
            id: "node_b".to_string(),
            ..Default::default()
        };
        let node_c = Node {
            id: "node_c".to_string(),
            ..Default::default()
        };
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![vec![node_a, node_b], vec![node_c]],
            output: HashSet::from(["node_c".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let ids = <Workflow as Callable<NoneDB>>::inner_ids(&workflow);
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_a".to_string()));
        assert!(ids.contains(&"node_b".to_string()));
        assert!(ids.contains(&"node_c".to_string()));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn test_inner_ids_node_with_template_tool_excluded() {
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![Tool {
                name: "t1".to_string(),
                description: None,
                tool: ToolRunner::Template(TemplateTool {
                    templates: vec!["result".to_string()],
                }),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![vec![node]],
            output: HashSet::from(["node_1".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        // Template tools are not included in inner_ids
        let ids = <Workflow as Callable<NoneDB>>::inner_ids(&workflow);
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_1".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_inner_ids_node_with_nested_workflow_tool() {
        let inner_node = Node {
            id: "inner_node".to_string(),
            ..Default::default()
        };
        let inner_workflow = Workflow {
            id: "inner_workflow".to_string(),
            plan: vec![vec![inner_node]],
            output: HashSet::from(["inner_node".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let node = Node {
            id: "node_1".to_string(),
            tools: vec![Tool {
                name: "wf_tool".to_string(),
                description: None,
                tool: ToolRunner::Workflow(inner_workflow),
                just_for: vec![],
                confirm_before_calling: false,
                bundle: None,
            }],
            ..Default::default()
        };
        let workflow = Workflow {
            id: "workflow_1".to_string(),
            plan: vec![vec![node]],
            output: HashSet::from(["node_1".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let ids = <Workflow as Callable<NoneDB>>::inner_ids(&workflow);
        assert!(ids.contains(&"workflow_1".to_string()));
        assert!(ids.contains(&"node_1".to_string()));
        assert!(ids.contains(&"inner_workflow".to_string()));
        assert!(ids.contains(&"inner_node".to_string()));
        assert_eq!(ids.len(), 4);
    }

    // ── Workflow::check (Callable) ────────────────────────────────────────────

    fn make_simple_workflow(wf_id: &str, node_id: &str) -> Workflow {
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
    fn test_check_valid_workflow() {
        let wf = make_simple_workflow("wf_one", "node_one");
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_workflow_invalid_id() {
        let mut wf = make_simple_workflow("wf_one", "node_one");
        wf.id = "1invalid".to_string();
        let err = <Workflow as Callable<NoneDB>>::check(&wf)
            .unwrap_err()
            .to_string();
        assert!(err.contains("1invalid"));
    }

    #[test]
    fn test_check_workflow_empty_id() {
        let mut wf = make_simple_workflow("wf_one", "node_one");
        wf.id = String::new();
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
    }

    #[test]
    fn test_check_workflow_output_references_missing_node() {
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
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("ghost_node"));
    }

    #[test]
    fn test_check_workflow_output_references_one_of_several_nodes() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![
                vec![Node {
                    id: "node_a".to_string(),
                    ..Default::default()
                }],
                vec![Node {
                    id: "node_b".to_string(),
                    ..Default::default()
                }],
            ],
            // only the last-layer node is output — fine
            output: HashSet::from(["node_b".to_string()]),
            input_prompt: None,
            evals: vec![],
        };

        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_workflow_duplicate_node_ids() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![
                vec![Node {
                    id: "node_dup".to_string(),
                    ..Default::default()
                }],
                vec![Node {
                    id: "node_dup".to_string(),
                    ..Default::default()
                }],
            ],
            output: HashSet::from(["node_dup".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("node_dup"));
    }

    #[test]
    fn test_check_workflow_duplicate_node_ids_same_layer() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![
                Node {
                    id: "node_dup".to_string(),
                    ..Default::default()
                },
                Node {
                    id: "node_dup".to_string(),
                    ..Default::default()
                },
            ]],
            output: HashSet::from(["node_dup".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("node_dup"));
    }

    #[test]
    fn test_check_workflow_propagates_node_error() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![Node {
                id: "1bad_id".to_string(),
                ..Default::default()
            }]],
            output: HashSet::from(["1bad_id".to_string()]),
            input_prompt: None,
            evals: vec![],
        };
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("1bad_id"));
    }

    #[test]
    fn test_check_workflow_empty_plan_is_err() {
        let wf = Workflow {
            id: "wf_empty".to_string(),
            plan: vec![],
            output: HashSet::new(),
            input_prompt: None,
            evals: vec![],
        };
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("no nodes"));
    }

    #[test]
    fn test_check_workflow_empty_output_is_err() {
        let wf = Workflow {
            id: "wf_one".to_string(),
            plan: vec![vec![Node {
                id: "node_one".to_string(),
                ..Default::default()
            }]],
            output: HashSet::new(),
            input_prompt: None,
            evals: vec![],
        };
        let check = <Workflow as Callable<NoneDB>>::check(&wf);
        assert!(check.is_err());
        let err = check.unwrap_err().to_string();
        assert!(err.contains("no output"));
    }
}
