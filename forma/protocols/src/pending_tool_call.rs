use serde::{Deserialize, Serialize};

/// Represents a tool that is waiting for external feedback.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    /// ID of the pending tool call
    pub tool_call_id: String,
    /// ID of the node that requested the tool call
    pub node_id: String,
    /// The name of the tool to be called
    pub tool_name: String,
    /// The arguments proposed by the AI
    pub arguments: serde_json::Value,
    /// Has this been approved?
    pub approved: bool,
    /// Indicates whether the tool call is from a root workflow, or not.
    pub from_root: bool,
    /// What the tool is (see `SSEvent::ToolInputStart`), so a reopened
    /// session renders the request as the live stream did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_type: String,
    /// The bundle of the tool, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
}
