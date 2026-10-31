//! Progress observations and coordination requests; notification is not resolution.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReassignmentStatusParams {
    pub crew_id: String,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReassignmentPhase {
    Prepared,
    Dispatched,
    Settled,
    Stale,
    Unknown,
    Suppressed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReassignmentReason {
    NoCandidates,
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
pub struct ReassignmentStatus {
    pub crew_id: String,
    pub id: String,
    pub record: Option<ReassignmentView>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReassignmentView {
    pub decision_id: String,
    pub phase: ReassignmentPhase,
    pub reason: ReassignmentReason,
    pub old_owner: String,
    pub new_owner: Option<String>,
    pub adopted: bool,
    pub confidence: Option<f64>,
    pub owner_wake_id: Option<i64>,
    pub owner_wake_state: Option<crate::wake_lifecycle::WakeState>,
    pub captain_wake_id: Option<i64>,
    pub captain_wake_state: Option<crate::wake_lifecycle::WakeState>,
}
