use std::collections::{HashMap, HashSet, VecDeque};

use futures::future::BoxFuture;

use agent::{
    Node, Workflow, WorkflowOutput, template::is_valid_variable_name, workflow::default_workflow_id,
};
use derive::FormaDocs;
use protocols::FormaError;

use crate::api::mcp_tool_definition::McpAccess;
use serde::{Deserialize, Serialize};

use crate::api::{
    ObjectDefinition, eval_definition::EvalDefinition, node_definition::NodeDefinition,
    template_field::expand_string,
};

fn unique_nodes(workflow_id: &str, nodes: &[NodeDefinition]) -> Result<(), FormaError> {
    let mut ids = HashSet::with_capacity(nodes.len());
    for n in nodes.iter() {
        if !ids.insert(&n.id) {
            return Err(format!(
                "Duplicate node id '{}' found in workflow '{}'",
                n.id, workflow_id
            )
            .into());
        }
    }
    Ok(())
}

/// Checks every node of a workflow, and that no two of them share an ID.
fn check_nodes(
    workflow_id: &str,
    nodes: Vec<NodeDefinition>,
) -> Result<Vec<NodeDefinition>, FormaError> {
    unique_nodes(workflow_id, &nodes)?;
    nodes.into_iter().map(|n| n.check()).collect()
}

#[allow(dead_code)]
fn expand_nodes(
    workflow_id: &str,
    nodes: &[NodeDefinition],
    parent_dir: &str,
) -> Result<Vec<NodeDefinition>, FormaError> {
    unique_nodes(workflow_id, nodes)?;
    check_nodes_dependencies(workflow_id, nodes)?;

    let mut ret = Vec::with_capacity(nodes.len());
    for n in nodes.iter() {
        let n = n.clone().expand(parent_dir)?;
        ret.push(n);
    }
    Ok(ret)
}

fn get_output_nodes(
    id: &str,
    output: &Option<WorkflowOutput>,
    nodes: &[NodeDefinition],
) -> Result<HashSet<String>, FormaError> {
    match output {
        None => {
            if nodes.len() == 1 {
                // This is the only successful option
                let node = nodes.first().expect("we just checked that workflow had a single element... how can there be no first one?");
                Ok(HashSet::from([node.id.clone()]))
            } else {
                Err(format!("Workflow '{id}' has multiple nodes but no specific output").into())
            }
        }
        Some(map) => match map {
            WorkflowOutput::Text(node_id) => {
                contains_node(id, nodes, node_id)?;
                Ok(HashSet::from([node_id.clone()]))
            }
            WorkflowOutput::Map(map) => {
                let mut ret = HashSet::with_capacity(map.len());
                for node_id in map.values() {
                    contains_node(id, nodes, node_id)?;
                    ret.insert(node_id.clone());
                }
                Ok(ret)
            }
        },
    }
}

/// Generates an execution plan for this workflow.
/// This uses Kahn's algorithm to generate a topological sort grouped by layers.
/// Layers allow for parallel execution: nodes in Layer N depend only on nodes in Layers 0..N-1.
pub async fn plan_execution(
    nodes: &[NodeDefinition],
    mcp: &McpAccess,
    available_variables: Vec<String>,
) -> Result<Vec<Vec<Node>>, FormaError> {
    if nodes.is_empty() {
        return Ok(vec![]);
    }

    let mut id_to_node_def: HashMap<&String, &NodeDefinition> = HashMap::with_capacity(nodes.len());
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new(); // Parent -> Children
    let mut in_degree: HashMap<String, usize> = HashMap::new(); // Node -> Count of incoming deps

    // 1. Initialize maps
    for node in nodes {
        id_to_node_def.insert(&node.id, node);
        in_degree.insert(node.id.clone(), 0);
        dependents.entry(node.id.clone()).or_default();
    }

    // 2. Build the graph (populate dependents and in_degree)
    for node in nodes {
        let node_deps = node.get_dependencies()?;
        for dep_id in node_deps {
            // Skip global dependencies (those not provided in the nodes list)
            if available_variables.iter().any(|v| v == &dep_id) {
                continue;
            }

            // Check if dependency actually exists in the provided nodes
            // and variable
            if !id_to_node_def.contains_key(&dep_id)
                && !available_variables.iter().any(|v| v == &dep_id)
            {
                return Err(FormaError::Input(format!(
                    "Node '{}' depends on '{}', which is not within the variables available to it.",
                    node.id, dep_id
                )));
            }

            // The dependency (dep_id) is a 'parent' to the current node
            dependents
                .entry(dep_id.clone())
                .or_default()
                .push(node.id.clone());
            *in_degree.entry(node.id.clone()).or_default() += 1;
        }
    }

    // 3. Find initial roots (nodes with 0 dependencies)
    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    // Sort roots for deterministic behavior in tests/execution logs
    let mut sorted_queue_vec: Vec<String> = queue.iter().cloned().collect();
    sorted_queue_vec.sort();
    queue = sorted_queue_vec.into();

    let mut plan: Vec<Vec<Node>> = Vec::new();
    let mut nodes_processed = 0;

    // 4. Process layers
    while !queue.is_empty() {
        let mut current_layer_nodes = Vec::new();
        let mut next_layer_ids = Vec::new();

        // Drain the entire queue to form the current execution layer
        // Everything currently in `queue` is ready to run in parallel
        while let Some(node_id) = queue.pop_front() {
            nodes_processed += 1;
            let available_variables = available_variables.clone();

            // Convert Definition to Runtime Node
            let def = *id_to_node_def
                .get(&node_id)
                .expect("Node id missing from map");
            let node: Node = def.clone().into_node(mcp, available_variables).await?;
            current_layer_nodes.push(node);

            // Process children of this node
            if let Some(children) = dependents.get(&node_id) {
                for child_id in children {
                    let degree = in_degree
                        .get_mut(child_id)
                        .expect("Child missing in degree map");
                    *degree -= 1;
                    if *degree == 0 {
                        next_layer_ids.push(child_id.clone());
                    }
                }
            }
        }

        // Sort next layer for determinism
        next_layer_ids.sort();
        for id in next_layer_ids {
            queue.push_back(id);
        }

        plan.push(current_layer_nodes);
    }

    // 5. Cycle Detection
    if nodes_processed != nodes.len() {
        return Err(format!(
            "Cyclic dependency detected in workflow. Processed {} nodes out of {}. Check for loops in variable usage.",
            nodes_processed, nodes.len()
        ).into());
    }

    Ok(plan)
}

/// Makes sure that node_id is contained within the nodes
fn contains_node(
    workflow_id: &str,
    nodes: &[NodeDefinition],
    node_id: &str,
) -> Result<(), FormaError> {
    if nodes.iter().any(|n| n.id == node_id) {
        Ok(())
    } else {
        Err(format!("Node with id '{node_id}' not found in '{workflow_id}' nodes").into())
    }
}

/// Makes sure that all the dependencies of every node in the workflow are
/// contained within the workflow
#[allow(dead_code)]
fn check_nodes_dependencies(workflow_id: &str, nodes: &[NodeDefinition]) -> Result<(), FormaError> {
    for n in nodes.iter() {
        let deps = n.get_dependencies()?;
        for d in deps.iter() {
            contains_node(workflow_id, nodes, d)?
        }
    }
    Ok(())
}

/// Workflows are a mechanism to break down large tasks into smaller—more focused—tasks. This is beneficial because complicated tasks—which require very large system prompts—and thus the AI Models will struggle to follow those instructions faithfully. By breaking down a big task into smaller bits, you can provide more precise, prevent contadictions in your prompts. Like people, LLMs perform better with clear and focused instructions.
///
/// A Workflow is a set of nodes that depend on each other (for the Geeks, it is a Directed Acyclic Graph of nodes).
///
/// After its execution, a Workflow returns an object with the results of
/// its output nodes.
#[derive(Clone, Deserialize, Serialize, Debug, FormaDocs)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowDefinition {
    /// The ID. If absent, one will be provided.
    #[serde(default = "default_workflow_id")]
    pub(crate) id: String,

    /// The nodes within the Workflow. All nodes
    /// needed to generate the outputs are guaranteed
    /// to run.
    #[serde(default)]
    pub(crate) nodes: Vec<NodeDefinition>,

    /// The IDs of the nodes that will end up in the output
    /// provided by this workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<WorkflowOutput>,

    /// The names of the evaluations to be ran for this workflow
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) evals: Vec<EvalDefinition>,
}

/// Takes a workflow and turns it into a Tool.
///
/// This workflow receives a fresh session. This means that it **is not**
/// aware of previous messages or the contributions made by other nodes
/// to the state.
///
/// After its execution, a Workflow returns an object with the results of
/// its output nodes.
#[derive(Clone, Deserialize, Serialize, Debug, Default, FormaDocs)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolDefinition {
    /// The ID. If absent, one will be provided.
    #[serde(default = "default_workflow_id")]
    pub(crate) id: String,

    /// The nodes within the Workflow. All nodes
    /// needed to generate the outputs are guaranteed
    /// to run.
    #[serde(default)]
    pub(crate) nodes: Vec<NodeDefinition>,

    /// The IDs of the nodes that will end up in the output
    /// provided by this workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<WorkflowOutput>,

    /// Determines the template for the message that will be used
    /// to trigger this workflow execution as a tool. The fields
    /// required will be generated by the LLM of the node that
    /// decides to call this tool.
    pub(crate) input_prompt: String,

    /// The names of the evaluations to be ran for this workflow
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub(crate) evals: Vec<EvalDefinition>,
}

impl WorkflowToolDefinition {
    /// Generates an execution plan for this workflow, starting
    /// from the output nodes and working backwards
    /// to determine the order of execution.
    pub async fn plan_execution(
        &self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> Result<Vec<Vec<Node>>, FormaError> {
        plan_execution(&self.nodes, mcp, available_variables).await
    }

    fn get_output_nodes(&self) -> Result<HashSet<String>, FormaError> {
        get_output_nodes(&self.id, &self.output, &self.nodes)
    }
}

impl ObjectDefinition for WorkflowToolDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        is_valid_variable_name(&self.id)?;

        // Checks
        get_output_nodes(&self.id, &self.output, &self.nodes)?;
        let nodes = expand_nodes(&self.id, &self.nodes, parent_dir)?;

        // respond
        let ret = Self {
            nodes,
            id: self.id,
            input_prompt: expand_string(self.input_prompt, parent_dir)?,
            output: self.output,
            evals: self
                .evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
        };
        Ok(ret)
    }

    fn check(self) -> Result<Self, FormaError> {
        let Self {
            id,
            nodes,
            output,
            input_prompt,
            evals,
        } = self;
        is_valid_variable_name(&id)?;
        get_output_nodes(&id, &output, &nodes)?;
        let nodes = check_nodes(&id, nodes)?;
        Ok(Self {
            id,
            nodes,
            output,
            input_prompt,
            evals,
        })
    }
}

impl WorkflowToolDefinition {
    pub fn into_workflow(
        self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> BoxFuture<'static, Result<Workflow, FormaError>> {
        let mcp = mcp.clone();

        Box::pin(async move {
            let plan = self.plan_execution(&mcp, available_variables).await?;
            let output = self.get_output_nodes()?;
            let mut evals = Vec::with_capacity(self.evals.len());
            for e in self.evals.into_iter() {
                let n = e.try_into()?;
                evals.push(n);
            }
            let ret = Workflow {
                plan,
                output,
                id: self.id,
                input_prompt: Some(self.input_prompt),
                evals,
            };
            Ok(ret)
        })
    }
}

impl Default for WorkflowDefinition {
    fn default() -> Self {
        Self {
            id: default_workflow_id(),
            nodes: vec![NodeDefinition::default()],
            output: None,
            evals: vec![],
        }
    }
}

impl WorkflowDefinition {
    /// Generates an execution plan for this workflow, starting
    /// from the output nodes and working backwards
    /// to determine the order of execution.
    pub async fn plan_execution(
        &self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> Result<Vec<Vec<Node>>, FormaError> {
        plan_execution(&self.nodes, mcp, available_variables).await
    }

    fn get_output_nodes(&self) -> Result<HashSet<String>, FormaError> {
        get_output_nodes(&self.id, &self.output, &self.nodes)
    }
}
impl ObjectDefinition for WorkflowDefinition {
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        get_output_nodes(&self.id, &self.output, &self.nodes)?;
        let nodes = expand_nodes(&self.id, &self.nodes, parent_dir)?;
        let ret = Self {
            nodes,
            id: self.id,
            output: self.output,
            evals: self
                .evals
                .into_iter()
                .map(|eval| eval.expand(parent_dir))
                .collect::<Result<_, _>>()?,
        };
        Ok(ret)
    }

    fn check(self) -> Result<Self, FormaError> {
        let Self {
            id,
            nodes,
            output,
            evals,
        } = self;
        is_valid_variable_name(&id)?;
        get_output_nodes(&id, &output, &nodes)?;
        let nodes = check_nodes(&id, nodes)?;
        Ok(Self {
            id,
            nodes,
            output,
            evals,
        })
    }
}

impl WorkflowDefinition {
    pub async fn into_workflow(
        self,
        mcp: &McpAccess,
        available_variables: Vec<String>,
    ) -> Result<Workflow, FormaError> {
        let plan = self.plan_execution(mcp, available_variables).await?;
        let output = self.get_output_nodes()?;
        let mut evals = Vec::with_capacity(self.evals.len());
        for e in self.evals.into_iter() {
            let n = e.try_into()?;
            evals.push(n);
        }
        let ret = Workflow {
            plan,
            output,
            id: self.id,
            input_prompt: None,
            evals,
        };
        Ok(ret)
    }
}

#[cfg(test)]
mod testing {

    use std::collections::{HashMap, HashSet};

    use agent::WorkflowOutput;
    use protocols::FormaError;

    use crate::api::mcp_tool_definition::McpAccess;

    use crate::api::{
        ObjectDefinition,
        node_definition::NodeDefinition,
        workflow_definition::{WorkflowDefinition, WorkflowToolDefinition, plan_execution},
    };

    #[test]
    fn missing_dependency() -> Result<(), FormaError> {
        let n = NodeDefinition {
            system_prompt: "Hello, {{world}}".to_string(),
            ..Default::default()
        };
        let w = WorkflowDefinition {
            nodes: vec![n],
            ..Default::default()
        };
        assert!(w.expand("some-dir").is_err());
        Ok(())
    }

    #[test]
    fn missing_dependency_tool() -> Result<(), FormaError> {
        let n = NodeDefinition {
            system_prompt: "Hello, {{world}}".to_string(), // this creates the dependency 'world'
            ..Default::default()
        };
        let w = WorkflowToolDefinition {
            nodes: vec![n],

            ..Default::default()
        };
        assert!(w.expand("some-dir").is_err());
        Ok(())
    }
    #[test]
    fn missing_dependency_tool2() -> Result<(), FormaError> {
        let n = NodeDefinition {
            system_prompt: "Hello, {{world:string}}".to_string(), // this creates the dependency 'world'
            ..Default::default()
        };
        let w = WorkflowToolDefinition {
            nodes: vec![n],

            ..Default::default()
        };
        assert!(w.expand("some-dir").is_err());
        Ok(())
    }

    #[test]
    fn missing_output_text() -> Result<(), FormaError> {
        let w = WorkflowToolDefinition {
            output: Some(WorkflowOutput::Text("missing-node".to_string())),
            ..Default::default()
        };
        assert!(w.expand("fake dir").is_err());
        Ok(())
    }
    #[test]
    fn missing_output_map() -> Result<(), FormaError> {
        let w = WorkflowToolDefinition {
            output: Some(WorkflowOutput::Map(HashMap::from([(
                "alias".to_string(),
                "missing-node".to_string(),
            )]))),
            ..Default::default()
        };
        assert!(w.expand("fake dir").is_err());
        Ok(())
    }

    #[test]
    fn available_output_text() -> Result<(), FormaError> {
        let nodeid = "the-node".to_string();
        let w = WorkflowToolDefinition {
            id: "the-id".to_string(),
            nodes: vec![NodeDefinition {
                id: nodeid.clone(),
                ..Default::default()
            }],
            output: Some(WorkflowOutput::Text(nodeid)),
            ..Default::default()
        };
        w.expand("fake dir")?;
        Ok(())
    }
    #[test]
    fn available_output_map() -> Result<(), FormaError> {
        let nodeid = "the-node".to_string();
        let w = WorkflowToolDefinition {
            id: "the-id".to_string(),
            nodes: vec![NodeDefinition {
                id: nodeid.clone(),
                ..Default::default()
            }],
            output: Some(WorkflowOutput::Map(HashMap::from([(
                "alias".to_string(),
                nodeid,
            )]))),
            ..Default::default()
        };
        w.expand("fake dir")?;
        Ok(())
    }

    #[test]
    fn missing_output_definitions() -> Result<(), FormaError> {
        let w = WorkflowToolDefinition {
            nodes: vec![
                NodeDefinition {
                    id: "node-1".to_string(),
                    ..Default::default()
                },
                NodeDefinition {
                    id: "node-2".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(w.expand("fake dir").is_err());
        Ok(())
    }
    #[test]
    fn duplicated_nodes() -> Result<(), FormaError> {
        let nodeid = "node-1".to_string();
        let w = WorkflowToolDefinition {
            nodes: vec![
                NodeDefinition {
                    id: nodeid.clone(),
                    ..Default::default()
                },
                NodeDefinition {
                    id: nodeid.clone(),
                    ..Default::default()
                },
            ],
            output: Some(WorkflowOutput::Map(HashMap::from([(
                "alias".to_string(),
                nodeid,
            )]))),
            ..Default::default()
        };
        assert!(w.expand("fake dir").is_err());
        Ok(())
    }

    // Helper to create a node definition with specific ID and dependencies
    // Assumes `get_dependencies` parses {{var}} from system_prompt
    fn mock_node(id: &str, dependencies: Vec<&str>) -> NodeDefinition {
        let mut prompt = String::from("Do something");
        for dep in dependencies {
            prompt.push_str(&format!(" with {{{{ {} }}}}", dep));
        }

        NodeDefinition {
            id: id.to_string(),
            system_prompt: prompt,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn plan_single_node() {
        let nodes = vec![mock_node("A", vec![])];
        let oauth = McpAccess::Connect(HashMap::new().into());
        let plan = plan_execution(&nodes, &oauth, vec![])
            .await
            .expect("Plan should succeed");

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].len(), 1);
        assert_eq!(plan[0][0].id, "A");
    }

    #[tokio::test]
    async fn plan_linear_chain() {
        // A -> B -> C
        let nodes = vec![
            mock_node("A", vec![]),
            mock_node("B", vec!["A"]),
            mock_node("C", vec!["B"]),
        ];

        let oauth = McpAccess::Connect(HashMap::new().into());
        let plan = plan_execution(&nodes, &oauth, vec![])
            .await
            .expect("Plan should succeed");

        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0][0].id, "A");
        assert_eq!(plan[1][0].id, "B");
        assert_eq!(plan[2][0].id, "C");
    }

    #[tokio::test]
    async fn plan_parallel_independent() {
        // A, B (no relation)
        let nodes = vec![mock_node("A", vec![]), mock_node("B", vec![])];
        let oauth = McpAccess::Connect(HashMap::new().into());
        let plan = plan_execution(&nodes, &oauth, vec![])
            .await
            .expect("Plan should succeed");

        // Should be one layer with both nodes
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].len(), 2);

        // Check contents (order might vary unless sorted, our impl sorts)
        let ids: HashSet<String> = plan[0].iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains("A"));
        assert!(ids.contains("B"));
    }

    #[tokio::test]
    async fn plan_diamond_dependency() {
        // A -> B
        // A -> C
        // B -> D
        // C -> D
        // Expected layers: [A], [B, C], [D]
        let nodes = vec![
            mock_node("A", vec![]),
            mock_node("B", vec!["A"]),
            mock_node("C", vec!["A"]),
            mock_node("D", vec!["B", "C"]),
        ];

        let oauth = McpAccess::Connect(HashMap::new().into());
        let plan = plan_execution(&nodes, &oauth, vec![])
            .await
            .expect("Plan should succeed");

        assert_eq!(plan.len(), 3);

        // Layer 1: A
        assert_eq!(plan[0].len(), 1);
        assert_eq!(plan[0][0].id, "A");

        // Layer 2: B and C
        assert_eq!(plan[1].len(), 2);
        let layer2_ids: HashSet<String> = plan[1].iter().map(|n| n.id.clone()).collect();
        assert!(layer2_ids.contains("B"));
        assert!(layer2_ids.contains("C"));

        // Layer 3: D
        assert_eq!(plan[2].len(), 1);
        assert_eq!(plan[2][0].id, "D");
    }

    #[tokio::test]
    async fn detect_direct_cycle() {
        // A -> B -> A
        let nodes = vec![mock_node("A", vec!["B"]), mock_node("B", vec!["A"])];

        let oauth = McpAccess::Connect(HashMap::new().into());
        let err = plan_execution(&nodes, &oauth, vec![]).await.unwrap_err();
        assert!(err.to_string().contains("Cyclic dependency detected"));
    }

    #[tokio::test]
    async fn detect_self_cycle() {
        // A -> A
        let nodes = vec![mock_node("A", vec!["A"])];

        let oauth = McpAccess::Connect(HashMap::new().into());
        let err = plan_execution(&nodes, &oauth, vec![]).await.unwrap_err();
        assert!(err.to_string().contains("Cyclic dependency detected"));
    }

    #[tokio::test]
    async fn detect_missing_dependency() {
        // A -> Z (Z does not exist)
        let nodes = vec![mock_node("A", vec!["Z"])];

        // Note: typically `check_nodes_dependencies` runs before this,
        // but plan_execution should be robust enough to handle or re-check.
        let oauth = McpAccess::Connect(HashMap::new().into());
        let err = plan_execution(&nodes, &oauth, vec![]).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("depends on 'Z', which is not within the variables available to it")
        );

        // A -> Z (Z does not exist)
        let nodes = vec![mock_node("A", vec!["Z"])];

        // Note: typically `check_nodes_dependencies` runs before this,
        // but plan_execution should be robust enough to handle or re-check.
        let oauth = McpAccess::Connect(HashMap::new().into());
        let ret = plan_execution(&nodes, &oauth, vec!["Z".to_string()]).await;
        assert!(ret.is_ok());
    }

    // ── check ────────────────────────────────────────────────────────────────

    #[test]
    fn check_rejects_duplicate_node_ids() {
        let node = NodeDefinition {
            id: "node_1".to_string(),
            ..Default::default()
        };
        let w = WorkflowDefinition {
            id: "wf_1".to_string(),
            nodes: vec![node.clone(), node],
            output: Some(WorkflowOutput::Text("node_1".to_string())),
            evals: vec![],
        };
        let err = w
            .check()
            .expect_err("two nodes with one id should be rejected")
            .to_string();
        assert!(err.contains("Duplicate node id 'node_1'"), "{err}");
    }

    #[test]
    fn check_rejects_output_naming_a_missing_node() {
        let w = WorkflowDefinition {
            id: "wf_1".to_string(),
            nodes: vec![NodeDefinition {
                id: "node_1".to_string(),
                ..Default::default()
            }],
            output: Some(WorkflowOutput::Text("ghost_node".to_string())),
            evals: vec![],
        };
        let err = w
            .check()
            .expect_err("an output naming no node should be rejected")
            .to_string();
        assert!(err.contains("ghost_node"), "{err}");
    }
}
