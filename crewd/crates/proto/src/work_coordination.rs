//! Progress observations and coordination requests; notification is not resolution.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationStatusParams {
    pub crew_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationPhase {
    Prepared,
    Dispatched,
    Settled,
    Stale,
    Unknown,
    Suppressed,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationReason {
    InsufficientReports,
    Unconfigured,
    Prepared,
    Answered,
    LowConfidence,
    ProviderUnavailable,
    RestartUnknown,
    StateChanged,
    Stopped,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationStatus {
    pub crew_id: String,
    pub id: String,
    pub record: Option<CoordinationView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assignment: Option<CoordinationAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_assignment: Option<CoordinationAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_split: Option<CoordinationSplit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_split: Option<CoordinationSplit>,
}
/// A verified owner change made in response to one specific coordination request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationAssignment {
    pub crew_id: String,
    pub id: String,
    pub decision_id: String,
    pub captain: String,
    pub previous_owner: String,
    pub owner: String,
    pub previous_revision: String,
    pub revision: String,
    pub event_seq: String,
    pub assigned_at_ms: i64,
}

/// An atomic set of children created in response to one coordination decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationSplit {
    pub crew_id: String,
    pub id: String,
    pub decision_id: String,
    pub captain: String,
    pub parent_revision: String,
    pub split_at_ms: i64,
    pub children: Vec<CoordinationSplitChild>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationSplitChild {
    pub id: String,
    pub owner: Option<String>,
    pub revision: String,
    pub event_seq: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationView {
    pub decision_id: String,
    pub phase: CoordinationPhase,
    pub reason: CoordinationReason,
    pub meaningful_progress: Option<f64>,
    pub needs_coordination: Option<f64>,
    pub adopted: bool,
    pub recipient: Option<String>,
    pub wake_id: Option<i64>,
    pub wake_state: Option<crate::wake_lifecycle::WakeState>,
}
