//! Progress observations and coordination requests; notification is not resolution.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoordinationStatusParams {
    pub crew_id: String,
    pub id: String,
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
