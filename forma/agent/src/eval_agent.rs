use crate::{
    Agent, Eval, Node, Workflow,
    callable::{CallContext, Callable, Identifiable},
};

use async_trait::async_trait;
use genai_client::{GenaiClient, GenaiClientTrait, LLMMessage};
use protocols::FormaDB;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::RwLock;

/// Represents an entity that can be evaluated within an Agent.
pub trait Evaluatable: Clone + Identifiable {
    /// Retrieves a list of evaluations
    fn get_evals(&self) -> &[Eval];

    /// Retrieves the list of evaluations, so that it can be pruned
    /// (e.g., when the user narrows the run down to a single dataset)
    fn evals_mut(&mut self) -> &mut Vec<Eval>;

    /// Counts the number of evaluations
    fn count_evals(&self) -> usize {
        self.get_evals().len()
    }

    /// Gathers a list of all the callable elements within it
    fn collect_evals(&self, callables: &mut EvaluatableSet) -> Result<(), String>;

    /// Counts the total number of metrics within all evals.
    fn count_metrics(&self) -> usize {
        let mut c = 0;
        for e in self.get_evals().iter() {
            c += e.metrics.len();
        }
        c
    }
}

/// An auxiliary structure that holds a single `Callable`, its ID, and
/// the number of evals and metrics. This is useful for filtering
/// evals (e.g., to run a single eval instead of all of them), and
/// for displaying the available evals.
#[derive(Clone, Debug, Default)]
pub struct SingleEvaluatable<T> {
    pub obj: Arc<RwLock<T>>,
    pub n_evals: usize,
    pub n_metrics: usize,
    pub id: String,
}

impl<T> std::fmt::Display for SingleEvaluatable<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\x1b[1;32m{}\x1b[0m - {} evals | {} metrics",
            self.id, self.n_evals, self.n_metrics
        )
    }
}

impl<T> SingleEvaluatable<T>
where
    T: Evaluatable,
{
    pub fn new(obj: &T) -> Self {
        Self {
            obj: Arc::new(RwLock::new(obj.clone())),
            n_evals: obj.count_evals(),
            n_metrics: obj.count_metrics(),
            id: obj.get_id().to_string(),
        }
    }

    /// Drops the evals that do not read one of `datasets`, and refreshes the
    /// counts so that they keep describing what is left.
    async fn retain_datasets(&mut self, datasets: &[String]) {
        let mut obj = self.obj.write().await;
        obj.evals_mut()
            .retain(|e| datasets.iter().any(|d| d == &e.dataset));
        self.n_evals = obj.count_evals();
        self.n_metrics = obj.count_metrics();
    }
}

/// Represents a set of `Callable` entities within an Agent. These include the Agent itself, as well as its
/// Workflows, and Nodes. Provides methods for querying and filtering these entities.
#[derive(Clone, Debug, Default)]
pub struct EvaluatableSet {
    pub root: Option<(String, SingleEvaluatable<Agent>)>,
    pub workflows: HashMap<String, SingleEvaluatable<Workflow>>,
    pub nodes: HashMap<String, SingleEvaluatable<Node>>,
    pub llms: HashMap<String, SingleEvaluatable<EvaluatableLLM>>,
}

/// The error returned when the user selects an element that does exist,
/// but that carries no evaluations of its own.
fn no_evals_error<T: Identifiable>(id: &str) -> String {
    format!(
        "{} '{}' has no evals... add an 'evals:' block to it, or run with '--list' to see what can be evaluated",
        T::CLASS,
        id
    )
}

impl EvaluatableSet {
    /// Checks if the set contains no root agent, workflows, or nodes
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
            && self.workflows.is_empty()
            && self.nodes.is_empty()
            && self.llms.is_empty()
    }

    /// Returns a new `CallableSet` containing only those callables (root, workflows, nodes)
    /// that have at least one evaluation (`n_evals > 0`).
    pub fn with_evals(self) -> Self {
        let root = match self.root {
            Some(r) => {
                if r.1.n_evals == 0 {
                    None
                } else {
                    Some(r)
                }
            }
            None => None,
        };

        Self {
            root,
            workflows: self
                .workflows
                .clone()
                .into_iter()
                .filter(|(_, x)| x.n_evals > 0)
                .collect(),
            nodes: self
                .nodes
                .clone()
                .into_iter()
                .filter(|(_, x)| x.n_evals > 0)
                .collect(),
            llms: self
                .llms
                .clone()
                .into_iter()
                .filter(|(_, x)| x.n_evals > 0)
                .collect(),
        }
    }

    /// Checks that the IDs requested by the user can actually be evaluated.
    ///
    /// This must be called on the *unpruned* set (i.e., before `with_evals()`),
    /// which is what allows telling apart an ID that does not exist from one
    /// that does exist but carries no evaluations of its own.
    pub fn check_selection(
        &self,
        add_agent: bool,
        workflow_ids: &Option<Vec<String>>,
        node_ids: &Option<Vec<String>>,
        llm_ids: &Option<Vec<String>>,
    ) -> Result<(), String> {
        if add_agent {
            match &self.root {
                None => return Err("root agent is not available".to_string()),
                Some((id, root)) if root.n_evals == 0 => return Err(no_evals_error::<Agent>(id)),
                Some(_) => {}
            }
        }
        Self::check_selection_in(workflow_ids, &self.workflows)?;
        Self::check_selection_in(node_ids, &self.nodes)?;
        Self::check_selection_in(llm_ids, &self.llms)?;
        Ok(())
    }

    /// Collects the name of every dataset referenced by an eval anywhere
    /// in the set (root agent, workflows, nodes and LLMs).
    pub async fn dataset_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        if let Some((_, u)) = &self.root {
            let guard = u.obj.read().await;
            for e in guard.get_evals() {
                names.insert(e.dataset.clone());
            }
        }
        Self::collect_dataset_names_in(&self.workflows, &mut names).await;
        Self::collect_dataset_names_in(&self.nodes, &mut names).await;
        Self::collect_dataset_names_in(&self.llms, &mut names).await;
        names
    }

    /// Adds the dataset names referenced by the evals within `from` into `names`.
    async fn collect_dataset_names_in<T: Evaluatable>(
        from: &HashMap<String, SingleEvaluatable<T>>,
        names: &mut HashSet<String>,
    ) {
        for u in from.values() {
            let guard = u.obj.read().await;
            for e in guard.get_evals() {
                names.insert(e.dataset.clone());
            }
        }
    }

    /// Checks that every dataset requested by the user is actually referenced by
    /// at least one eval in the set. Naming a dataset that no target uses is an
    /// error, consistent with the `-n`/`-w`/`-l` selectors.
    ///
    /// Like `check_selection`, this must run on the *unpruned* set.
    pub async fn check_dataset_selection(
        &self,
        dataset_ids: &Option<Vec<String>>,
    ) -> Result<(), String> {
        let Some(wanted_ids) = dataset_ids else {
            return Ok(());
        };
        let available = self.dataset_names().await;
        for wanted_id in wanted_ids.iter() {
            if !available.contains(wanted_id) {
                return Err(format!("dataset '{wanted_id}' does not exist"));
            }
        }
        Ok(())
    }

    /// Drops every eval that does not read one of the requested datasets, and
    /// then every element that is left with nothing to evaluate.
    ///
    /// The counts are refreshed along the way, so that what `--dry-run` prints
    /// stays equal to what a real run would do.
    pub async fn filter_datasets(mut self, dataset_ids: &Option<Vec<String>>) -> Self {
        let Some(datasets) = dataset_ids else {
            // nothing was selected... nothing to filter
            return self;
        };
        if let Some((_, u)) = &mut self.root {
            u.retain_datasets(datasets).await;
        }
        Self::retain_datasets_in(&mut self.workflows, datasets).await;
        Self::retain_datasets_in(&mut self.nodes, datasets).await;
        Self::retain_datasets_in(&mut self.llms, datasets).await;

        self.with_evals()
    }

    /// Prunes the evals of every callable within `from`.
    async fn retain_datasets_in<T: Evaluatable>(
        from: &mut HashMap<String, SingleEvaluatable<T>>,
        datasets: &[String],
    ) {
        for u in from.values_mut() {
            u.retain_datasets(datasets).await;
        }
    }

    /// Checks a single selection of IDs against the map of callables it selects from.
    fn check_selection_in<T: Clone + Identifiable>(
        select: &Option<Vec<String>>,
        from: &HashMap<String, SingleEvaluatable<T>>,
    ) -> Result<(), String> {
        let Some(wanted_ids) = select else {
            // nothing was selected... nothing to check
            return Ok(());
        };
        for wanted_id in wanted_ids.iter() {
            match from.get(wanted_id) {
                None => return Err(format!("{} '{}' does not exist", T::CLASS, wanted_id)),
                Some(found) if found.n_evals == 0 => return Err(no_evals_error::<T>(wanted_id)),
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// Filters the provided map of callables by the given selection of IDs. Returns an error
    /// if any requested ID is not present in the map.
    fn filter_in<T: Clone + Identifiable>(
        &self,
        select: &Option<Vec<String>>,
        from: &HashMap<String, SingleEvaluatable<T>>,
    ) -> Result<HashMap<String, SingleEvaluatable<T>>, String> {
        if let Some(wanted_id) = &select {
            // Make sure all wanted ids are within the set
            for filter_id in wanted_id.iter() {
                if from.get(filter_id).is_none() {
                    return Err(format!("could not select {} '{}'", T::CLASS, filter_id));
                }
            }
            // Now, filter.
            let mut ret = HashMap::new();
            for filter_id in wanted_id.iter() {
                let new = from
                    .get(filter_id)
                    .expect("we already checked this, above")
                    .clone();
                ret.insert(filter_id.clone(), new);
            }

            Ok(ret)
        } else {
            // no filters... just return
            Ok(from.clone())
        }
    }

    /// Selects the callables from within the set as per the
    /// provided filters.
    ///
    /// If no filters are passed, no filters are applied. If one filter
    /// is passed (e.g., only a single workflow is selected), then all
    /// else is excluded (e.g., only THAT workflow will be returned)
    pub fn filter(
        self,
        add_agent: bool,
        workflow_ids: &Option<Vec<String>>,
        node_ids: &Option<Vec<String>>,
        llm_ids: &Option<Vec<String>>,
    ) -> Result<Self, String> {
        // user input something to filter
        let any_filter =
            add_agent || workflow_ids.is_some() || node_ids.is_some() || llm_ids.is_some();
        if !any_filter {
            return Ok(self);
        }
        let mut ret = Self::default();
        if add_agent {
            if self.root.is_none() {
                return Err("root agent is not available".to_string());
            }
            ret.root = self.root.clone();
        } else {
            ret.root = None;
        }
        if any_filter && workflow_ids.is_some() {
            ret.workflows = self.filter_in(workflow_ids, &self.workflows)?;
        }
        if any_filter && node_ids.is_some() {
            ret.nodes = self.filter_in(node_ids, &self.nodes)?;
        }
        if any_filter && llm_ids.is_some() {
            ret.llms = self.filter_in(llm_ids, &self.llms)?;
        }
        Ok(ret)
    }
}

impl std::fmt::Display for EvaluatableSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some((_, u)) = &self.root {
            writeln!(f, "\x1b[1;34mAgent\x1b[0m\n  - {u}")?;
        }
        if !self.workflows.is_empty() {
            writeln!(f, "\x1b[1;34mWorkflows\x1b[0m:")?;
            for u in self.workflows.values() {
                writeln!(f, "  - {u}")?;
            }
        }
        if !self.nodes.is_empty() {
            writeln!(f, "\x1b[1;34mNodes\x1b[0m:")?;
            for u in self.nodes.values() {
                writeln!(f, "  - {u}")?;
            }
        }
        if !self.llms.is_empty() {
            writeln!(f, "\x1b[1;34mLLMs\x1b[0m:")?;
            for u in self.llms.values() {
                writeln!(f, "  - {u}")?;
            }
        }

        Ok(())
    }
}

/// A wrapper on top of a `GenaiClient` that is used to
/// make it evaluatable.
#[derive(Default, Clone, Debug)]
pub struct EvaluatableLLM {
    pub id: String,
    pub llm: GenaiClient,
    pub evals: Vec<Eval>,
}

impl EvaluatableLLM {
    /// Creates a new `EvaluatableLLM` for the triage LLM of a Node.
    pub fn new_triage(node: &Node) -> Self {
        let id = if node.triage_evals.is_empty() {
            format!("{}.llm", node.id)
        } else {
            format!("{}.triage-llm", node.id)
        };

        Self {
            id,
            llm: node.llm.clone(),
            evals: node.triage_evals.clone(),
        }
    }

    /// Creates a new `EvaluatableLLM` for the summarization LLM of a Node.
    pub fn new_summarization(node: &Node) -> Self {
        let id = format!("{}.summarize-llm", node.id);
        let llm = node.summarization_llm.clone();
        Self {
            id,
            llm,
            evals: node.summarize_evals.clone(),
        }
    }
}

impl Identifiable for EvaluatableLLM {
    const CLASS: &str = "LLM";
    fn get_id(&self) -> &str {
        &self.id
    }
}

#[async_trait]
impl<DB: FormaDB> Callable<DB> for EvaluatableLLM {
    fn check(&self) -> Result<(), crate::FormaError> {
        Ok(())
    }

    async fn run(
        &self,
        ctx: CallContext<DB>,
        _is_root: bool,
    ) -> Result<Option<serde_json::Value>, crate::FormaError> {
        let messages = {
            let c = ctx.session.read().await;
            c.messages.clone()
        };
        let messages: Vec<LLMMessage> = messages
            .into_iter()
            .map(LLMMessage::try_from)
            .collect::<Result<_, _>>()?;
        let ret = self.llm.generate(messages, None).await?;
        let msg = match ret.message() {
            Some(m) => m.clone(),
            None => {
                return Err(crate::FormaError::Execution(
                    "LLM returned no message".to_string(),
                ));
            }
        };
        if let LLMMessage::Assistant(msg) = &msg {
            let m: Vec<String> = msg.content.iter().map(|x| x.into()).collect();
            let m = m.join("\n");
            let ret = serde_json::to_value(&m)?;
            Ok(Some(ret))
        } else {
            unreachable!("non-assistant msg?")
        }
    }

    fn inner_ids(&self) -> Vec<String> {
        vec![self.id.clone()]
    }
}

impl Evaluatable for EvaluatableLLM {
    fn get_evals(&self) -> &[Eval] {
        &self.evals
    }

    fn evals_mut(&mut self) -> &mut Vec<Eval> {
        &mut self.evals
    }

    fn collect_evals(&self, _callable: &mut EvaluatableSet) -> Result<(), String> {
        unreachable!("LLMs do not have nested callables");
    }
}

#[cfg(test)]
mod testing {

    use db::NoneDB;

    use super::*;
    use crate::Metric;

    /// `Metric` has no `Default`... this keeps the tests below readable.
    fn a_metric() -> Metric {
        Metric {
            id: "".to_string(),
            judge: GenaiClient::default(),
            template: "$some-metric.md".to_string(),
            required_score: None,
        }
    }

    #[test]
    fn test_new() {
        let id = "p987123q";
        let agent = Agent {
            id: id.to_string(),
            evals: vec![Eval::default()],
            ..Default::default()
        };
        let _c = SingleEvaluatable::new(&agent);
    }

    #[test]
    fn test_callable_set_is_empty() {
        // Empty set
        let empty_set = EvaluatableSet::default();
        assert!(empty_set.is_empty());

        // Set with only root
        let mut root_set = EvaluatableSet::default();
        root_set.root = Some((
            "test".to_string(),
            SingleEvaluatable::new(&Agent::default()),
        ));
        assert!(!root_set.is_empty());

        // Set with only workflow
        let mut workflow_set = EvaluatableSet::default();
        workflow_set.workflows.insert(
            "test".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        assert!(!workflow_set.is_empty());

        // Set with only node
        let mut node_set = EvaluatableSet::default();
        node_set
            .nodes
            .insert("test".to_string(), SingleEvaluatable::new(&Node::default()));
        assert!(!node_set.is_empty());

        // Set with all elements
        let mut full_set = EvaluatableSet::default();
        full_set.root = Some((
            "test".to_string(),
            SingleEvaluatable::new(&Agent::default()),
        ));
        full_set.workflows.insert(
            "test".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        full_set
            .nodes
            .insert("test".to_string(), SingleEvaluatable::new(&Node::default()));
        assert!(!full_set.is_empty());
    }

    #[test]
    fn test_callable_set_with_evals() {
        let mut set = EvaluatableSet::default();
        // Root with no evals
        let agent = Agent::default();
        set.root = Some(("some-id".to_string(), SingleEvaluatable::new(&agent)));
        let filtered = set.clone().with_evals();

        assert!(filtered.root.is_none());
        assert!(filtered.workflows.is_empty());
        assert!(filtered.nodes.is_empty());

        // Add root with evals
        let mut agent = Agent::default();
        agent.evals.push(Eval::default());
        set.root = Some(("root".to_string(), SingleEvaluatable::new(&agent)));

        // Add workflow with and without evals
        let mut workflow_with_evals = Workflow::default();
        workflow_with_evals.evals.push(Eval::default());
        let workflow_without_evals = Workflow::default();

        set.workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&workflow_with_evals),
        );
        set.workflows.insert(
            "workflow2".to_string(),
            SingleEvaluatable::new(&workflow_without_evals),
        );

        // Add node with and without evals
        let mut node_with_evals = Node::default();
        node_with_evals.evals.push(Eval::default());
        let node_without_evals = Node::default();

        set.nodes.insert(
            "node1".to_string(),
            SingleEvaluatable::new(&node_with_evals),
        );
        set.nodes.insert(
            "node2".to_string(),
            SingleEvaluatable::new(&node_without_evals),
        );

        // Test filtering
        let filtered = set.with_evals();

        assert!(filtered.root.is_some());
        assert_eq!(filtered.workflows.len(), 1);
        assert!(filtered.workflows.contains_key("workflow1"));
        assert_eq!(filtered.nodes.len(), 1);
        assert!(filtered.nodes.contains_key("node1"));

        // No root agent
        let mut set = EvaluatableSet::default();
        set.root = None; // just to be sure
        let filtered = set.with_evals();
        assert!(filtered.root.is_none());
    }

    #[test]
    fn test_callable_set_filter() {
        let mut set = EvaluatableSet::default();

        // Setup test data
        set.root = Some((
            "root".to_string(),
            SingleEvaluatable::new(&Agent::default()),
        ));
        set.workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        set.workflows.insert(
            "workflow2".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        set.nodes.insert(
            "node1".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );
        set.nodes.insert(
            "node2".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );
        set.llms.insert(
            "llm1".to_string(),
            SingleEvaluatable::new(&EvaluatableLLM::default()),
        );

        // Test no filters... returns `self`, untouched.
        let result = set.clone().filter(false, &None, &None, &None);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert!(filtered.root.is_some());
        assert_eq!(filtered.nodes.len(), 2);
        assert!(filtered.nodes.contains_key("node1"));
        assert!(filtered.nodes.contains_key("node2"));
        assert_eq!(filtered.workflows.len(), 2);
        assert!(filtered.workflows.contains_key("workflow1"));
        assert!(filtered.workflows.contains_key("workflow2"));

        // Test agent filter... returns only the agent
        let result = set.clone().filter(true, &None, &None, &None);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert!(filtered.root.is_some());
        assert!(filtered.nodes.is_empty());
        assert!(filtered.workflows.is_empty());

        // Test workflow filter, no agent.
        let workflow_ids = Some(vec!["workflow1".to_string()]);
        let result = set.clone().filter(false, &workflow_ids, &None, &None);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert!(filtered.root.is_none());
        assert_eq!(filtered.workflows.len(), 1);
        assert!(filtered.workflows.contains_key("workflow1"));
        assert!(filtered.nodes.is_empty());

        // Test node filter
        let node_ids = Some(vec!["node1".to_string()]);
        let result = set.clone().filter(false, &None, &node_ids, &None);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert!(filtered.root.is_none());
        assert!(filtered.workflows.is_empty());
        assert_eq!(filtered.nodes.len(), 1);
        assert!(filtered.nodes.contains_key("node1"));

        // Test LLM filter
        let llm_ids = Some(vec!["llm1".to_string()]);
        let result = set.clone().filter(false, &None, &None, &llm_ids);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert!(filtered.root.is_none());
        assert!(filtered.workflows.is_empty());
        assert!(filtered.nodes.is_empty());
        assert_eq!(filtered.llms.len(), 1);
        assert!(filtered.llms.contains_key("llm1"));

        // Test invalid filter
        let invalid_workflow_ids = Some(vec!["nonexistent".to_string()]);
        let result = set
            .clone()
            .filter(false, &invalid_workflow_ids, &None, &None);
        assert!(result.is_err());
    }

    #[test]
    fn test_check_selection_distinguishes_missing_from_eval_less() {
        let mut set = EvaluatableSet::default();

        let mut node_with_evals = Node::default();
        node_with_evals.evals.push(Eval::default());
        set.nodes.insert(
            "with-evals".to_string(),
            SingleEvaluatable::new(&node_with_evals),
        );
        set.nodes.insert(
            "no-evals".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );

        // A node that has evals... all good
        let selection = Some(vec!["with-evals".to_string()]);
        assert!(set.check_selection(false, &None, &selection, &None).is_ok());

        // A node that does not exist
        let selection = Some(vec!["typo".to_string()]);
        let err = set
            .check_selection(false, &None, &selection, &None)
            .unwrap_err();
        assert_eq!(err, "node 'typo' does not exist");

        // A node that exists, but that has no evals of its own
        let selection = Some(vec!["no-evals".to_string()]);
        let err = set
            .check_selection(false, &None, &selection, &None)
            .unwrap_err();
        assert!(err.starts_with("node 'no-evals' has no evals"), "{err}");

        // Nothing selected... nothing to check
        assert!(set.check_selection(false, &None, &None, &None).is_ok());
    }

    #[test]
    fn test_check_selection_workflows_and_llms() {
        let mut set = EvaluatableSet::default();

        let mut workflow = Workflow::default();
        workflow.evals.push(Eval::default());
        set.workflows
            .insert("flow".to_string(), SingleEvaluatable::new(&workflow));
        set.llms.insert(
            "some.llm".to_string(),
            SingleEvaluatable::new(&EvaluatableLLM::default()),
        );

        let flow = Some(vec!["flow".to_string()]);
        assert!(set.check_selection(false, &flow, &None, &None).is_ok());

        let missing = Some(vec!["nope".to_string()]);
        let err = set
            .check_selection(false, &missing, &None, &None)
            .unwrap_err();
        assert_eq!(err, "workflow 'nope' does not exist");

        let llm = Some(vec!["some.llm".to_string()]);
        let err = set.check_selection(false, &None, &None, &llm).unwrap_err();
        assert!(err.starts_with("LLM 'some.llm' has no evals"), "{err}");
    }

    #[tokio::test]
    async fn test_check_dataset_selection() {
        let mut set = EvaluatableSet::default();

        let mut node = Node::default();
        node.evals.push(Eval {
            dataset: "greetings".to_string(),
            ..Default::default()
        });
        node.evals.push(Eval {
            dataset: "refunds".to_string(),
            ..Default::default()
        });
        set.nodes
            .insert("node1".to_string(), SingleEvaluatable::new(&node));

        let mut workflow = Workflow::default();
        workflow.evals.push(Eval {
            dataset: "escalations".to_string(),
            ..Default::default()
        });
        set.workflows
            .insert("flow".to_string(), SingleEvaluatable::new(&workflow));

        // Nothing requested... nothing to check
        assert!(set.check_dataset_selection(&None).await.is_ok());

        // A dataset that exists, from a node
        assert!(
            set.check_dataset_selection(&Some(vec!["greetings".to_string()]))
                .await
                .is_ok()
        );

        // A dataset that exists, from a workflow
        assert!(
            set.check_dataset_selection(&Some(vec!["escalations".to_string()]))
                .await
                .is_ok()
        );

        // Several existing datasets at once
        assert!(
            set.check_dataset_selection(&Some(vec![
                "greetings".to_string(),
                "refunds".to_string(),
            ]))
            .await
            .is_ok()
        );

        // A dataset that no target uses
        let err = set
            .check_dataset_selection(&Some(vec!["typo".to_string()]))
            .await
            .unwrap_err();
        assert_eq!(err, "dataset 'typo' does not exist");

        // One good, one bad
        let err = set
            .check_dataset_selection(&Some(vec!["greetings".to_string(), "nope".to_string()]))
            .await
            .unwrap_err();
        assert_eq!(err, "dataset 'nope' does not exist");
    }

    #[tokio::test]
    async fn test_filter_datasets_prunes_evals_and_counts() {
        let mut set = EvaluatableSet::default();

        // A node with one eval per dataset, and one metric on the kept one.
        let mut node = Node::default();
        node.evals.push(Eval {
            dataset: "keep".to_string(),
            metrics: vec![a_metric()],
            ..Default::default()
        });
        node.evals.push(Eval {
            dataset: "drop".to_string(),
            metrics: vec![a_metric(), a_metric()],
            ..Default::default()
        });
        set.nodes
            .insert("node1".to_string(), SingleEvaluatable::new(&node));

        // A workflow that only ever uses the dataset we are dropping
        let mut workflow = Workflow::default();
        workflow.evals.push(Eval {
            dataset: "drop".to_string(),
            ..Default::default()
        });
        set.workflows
            .insert("flow".to_string(), SingleEvaluatable::new(&workflow));

        // Before filtering, the counts describe everything
        assert_eq!(set.nodes.get("node1").unwrap().n_evals, 2);
        assert_eq!(set.nodes.get("node1").unwrap().n_metrics, 3);

        let filtered = set.filter_datasets(&Some(vec!["keep".to_string()])).await;

        // The workflow had nothing left to evaluate... it is gone
        assert!(filtered.workflows.is_empty());

        // The node kept the one eval that reads 'keep', and the counts say so
        let node = filtered.nodes.get("node1").expect("node1 should remain");
        assert_eq!(node.n_evals, 1);
        assert_eq!(node.n_metrics, 1);

        // ... and so does the object that will actually be run
        let obj = node.obj.read().await;
        assert_eq!(obj.get_evals().len(), 1);
        assert_eq!(obj.get_evals()[0].dataset, "keep");
    }

    #[tokio::test]
    async fn test_filter_datasets_without_selection_changes_nothing() {
        let mut set = EvaluatableSet::default();
        let mut node = Node::default();
        node.evals.push(Eval {
            dataset: "some-ds".to_string(),
            ..Default::default()
        });
        // A node with no evals at all, which `filter_datasets` must not prune
        // when nothing was requested.
        set.nodes.insert(
            "empty".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );
        set.nodes
            .insert("node1".to_string(), SingleEvaluatable::new(&node));

        let filtered = set.filter_datasets(&None).await;
        assert_eq!(filtered.nodes.len(), 2);
        assert_eq!(filtered.nodes.get("node1").unwrap().n_evals, 1);
    }

    #[tokio::test]
    async fn test_filter_datasets_drops_the_root() {
        let mut set = EvaluatableSet::default();
        let mut agent = Agent::default();
        agent.evals.push(Eval {
            dataset: "only-this".to_string(),
            ..Default::default()
        });
        set.root = Some(("root".to_string(), SingleEvaluatable::new(&agent)));

        // Kept
        let filtered = set
            .clone()
            .filter_datasets(&Some(vec!["only-this".to_string()]))
            .await;
        assert_eq!(filtered.root.as_ref().unwrap().1.n_evals, 1);

        // Dropped... nothing left to evaluate
        let filtered = set
            .filter_datasets(&Some(vec!["something-else".to_string()]))
            .await;
        assert!(filtered.root.is_none());
        assert!(filtered.is_empty());
    }

    #[tokio::test]
    async fn test_dataset_names() {
        let mut set = EvaluatableSet::default();
        assert!(set.dataset_names().await.is_empty());

        let mut agent = Agent::default();
        agent.evals.push(Eval {
            dataset: "root-ds".to_string(),
            ..Default::default()
        });
        set.root = Some(("root".to_string(), SingleEvaluatable::new(&agent)));

        let mut node = Node::default();
        node.evals.push(Eval {
            dataset: "node-ds".to_string(),
            ..Default::default()
        });
        // A duplicate, to make sure the set dedupes.
        node.evals.push(Eval {
            dataset: "root-ds".to_string(),
            ..Default::default()
        });
        set.nodes
            .insert("node1".to_string(), SingleEvaluatable::new(&node));

        let names = set.dataset_names().await;
        assert_eq!(names.len(), 2);
        assert!(names.contains("root-ds"));
        assert!(names.contains("node-ds"));
    }

    #[test]
    fn test_check_selection_agent() {
        // No root at all
        let set = EvaluatableSet::default();
        let err = set.check_selection(true, &None, &None, &None).unwrap_err();
        assert_eq!(err, "root agent is not available");

        // A root without evals
        let mut set = EvaluatableSet::default();
        set.root = Some((
            "my-agent".to_string(),
            SingleEvaluatable::new(&Agent::default()),
        ));
        let err = set.check_selection(true, &None, &None, &None).unwrap_err();
        assert!(err.starts_with("agent 'my-agent' has no evals"), "{err}");

        // A root with evals
        let mut agent = Agent::default();
        agent.evals.push(Eval::default());
        set.root = Some(("my-agent".to_string(), SingleEvaluatable::new(&agent)));
        assert!(set.check_selection(true, &None, &None, &None).is_ok());
    }

    #[test]
    fn test_filter_in_with_none_selection() {
        let set = EvaluatableSet::default();
        let mut workflows = HashMap::new();
        workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        workflows.insert(
            "workflow2".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );

        let result = set.filter_in(&None, &workflows);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key("workflow1"));
        assert!(filtered.contains_key("workflow2"));
    }

    #[test]
    fn test_filter_with_no_root() {
        // trying to include the Root, when root is None
        let mut set = EvaluatableSet::default();
        set.root = None;
        assert!(set.filter(true, &None, &None, &None).is_err());
    }

    #[test]
    fn test_filter_in_with_single_valid_id() {
        let set = EvaluatableSet::default();
        let mut workflows = HashMap::new();
        workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );
        workflows.insert(
            "workflow2".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );

        let result = set.filter_in(&Some(vec!["workflow1".to_string()]), &workflows);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 1);
        assert!(filtered.contains_key("workflow1"));
        assert!(!filtered.contains_key("workflow2"));
    }

    #[test]
    fn test_filter_in_with_multiple_valid_ids() {
        let set = EvaluatableSet::default();
        let mut nodes = HashMap::new();
        nodes.insert(
            "node1".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );
        nodes.insert(
            "node2".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );
        nodes.insert(
            "node3".to_string(),
            SingleEvaluatable::new(&Node::default()),
        );

        let result = set.filter_in(
            &Some(vec!["node1".to_string(), "node3".to_string()]),
            &nodes,
        );
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key("node1"));
        assert!(filtered.contains_key("node3"));
        assert!(!filtered.contains_key("node2"));
    }

    #[test]
    fn test_filter_in_with_invalid_id() {
        let set = EvaluatableSet::default();
        let workflows: HashMap<String, SingleEvaluatable<Workflow>> = HashMap::new();

        let result = set.filter_in(&Some(vec!["nonexistent".to_string()]), &workflows);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("could not select"));
    }

    #[test]
    fn test_filter_in_with_partial_invalid_ids() {
        let set = EvaluatableSet::default();
        let mut workflows = HashMap::new();
        workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );

        let result = set.filter_in(
            &Some(vec!["workflow1".to_string(), "invalid".to_string()]),
            &workflows,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_filter_in_empty_selection_vec() {
        let set = EvaluatableSet::default();
        let mut workflows = HashMap::new();
        workflows.insert(
            "workflow1".to_string(),
            SingleEvaluatable::new(&Workflow::default()),
        );

        let result = set.filter_in(&Some(vec![]), &workflows);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn display() {
        let mut set = EvaluatableSet::default();
        let agent = Agent::default();
        // Setup test data
        set.root = Some(("root".to_string(), SingleEvaluatable::new(&agent)));
        let workflow_1 = Workflow::default();
        set.workflows
            .insert("workflow1".to_string(), SingleEvaluatable::new(&workflow_1));
        let workflow_2 = Workflow::default();
        set.workflows
            .insert("workflow2".to_string(), SingleEvaluatable::new(&workflow_2));
        let node_2 = Node::default();
        set.nodes
            .insert("node2".to_string(), SingleEvaluatable::new(&node_2));
        let node_1 = Node::default();
        set.nodes
            .insert("node1".to_string(), SingleEvaluatable::new(&node_1));

        let llm = GenaiClient::default();
        let llm = EvaluatableLLM {
            llm,
            ..Default::default()
        };
        let llm = SingleEvaluatable {
            obj: Arc::new(RwLock::new(llm)),
            ..Default::default()
        };
        set.llms.insert("some-llm".to_string(), llm);

        // no panic!
        println!("{}", set.to_string());
    }

    #[test]
    fn test_new_triage_id_no_evals() {
        let mut node = Node::default();
        node.id = "node1".to_string();
        node.triage_evals = vec![];

        let ellm = EvaluatableLLM::new_triage(&node);
        assert_eq!(ellm.id, format!("{}.llm", node.id));
        assert_eq!(ellm.evals.len(), 0);
    }

    #[test]
    fn test_new_triage_id_with_evals() {
        let mut node = Node::default();
        node.id = "node2".to_string();
        node.triage_evals.push(Eval::default());

        let ellm = EvaluatableLLM::new_triage(&node);
        assert_eq!(ellm.id, format!("{}.triage-llm", node.id));
        assert_eq!(ellm.evals.len(), 1);
    }

    #[test]
    fn test_new_summarization_id() {
        let mut node = Node::default();
        node.id = "summ-node".to_string();
        node.summarize_evals.push(Eval::default());

        let ellm = EvaluatableLLM::new_summarization(&node);
        assert_eq!(ellm.id, format!("{}.summarize-llm", node.id));
        assert_eq!(ellm.evals.len(), 1);
    }

    #[test]
    fn test_inner_ids_triage_llm_no_evals() {
        let mut node = Node::default();
        node.id = "node_1".to_string();
        // No triage evals → id is "{node_id}.llm"
        let ellm = EvaluatableLLM::new_triage(&node);
        let ids = <EvaluatableLLM as Callable<NoneDB>>::inner_ids(&ellm);
        assert_eq!(ids, vec!["node_1.llm"]);
    }

    #[test]
    fn test_inner_ids_triage_llm_with_evals() {
        let mut node = Node::default();
        node.id = "node_1".to_string();
        node.triage_evals.push(Eval::default());
        // With triage evals → id is "{node_id}.triage-llm"
        let ellm = EvaluatableLLM::new_triage(&node);
        let ids = <EvaluatableLLM as Callable<NoneDB>>::inner_ids(&ellm);
        assert_eq!(ids, vec!["node_1.triage-llm"]);
    }

    #[test]
    fn test_inner_ids_summarization_llm() {
        let mut node = Node::default();
        node.id = "node_1".to_string();
        let ellm = EvaluatableLLM::new_summarization(&node);
        let ids = <EvaluatableLLM as Callable<NoneDB>>::inner_ids(&ellm);
        assert_eq!(ids, vec!["node_1.summarize-llm"]);
    }

    #[test]
    fn test_inner_ids_contains_only_own_id() {
        let mut node = Node::default();
        node.id = "my_node".to_string();
        let ellm = EvaluatableLLM::new_triage(&node);
        let ids = <EvaluatableLLM as Callable<NoneDB>>::inner_ids(&ellm);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], ellm.id);
    }

    // ── EvaluatableLLM::check ─────────────────────────────────────────────────

    #[test]
    fn test_check_triage_llm_ok() {
        let node = Node::default();
        let ellm = EvaluatableLLM::new_triage(&node);
        let check = <EvaluatableLLM as Callable<NoneDB>>::check(&ellm);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_summarization_llm_ok() {
        let node = Node::default();
        let ellm = EvaluatableLLM::new_summarization(&node);
        let check = <EvaluatableLLM as Callable<NoneDB>>::check(&ellm);
        assert!(check.is_ok());
    }

    #[test]
    fn test_check_triage_llm_with_evals_ok() {
        let mut node = Node::default();
        node.triage_evals = vec![Eval::default()];
        let ellm = EvaluatableLLM::new_triage(&node);
        let check = <EvaluatableLLM as Callable<NoneDB>>::check(&ellm);
        assert!(check.is_ok());
    }
}
