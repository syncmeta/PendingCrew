//! Process retirement preserves the resumable session and all work history.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRetirementStatusParams {
    pub crew_id: String,
    pub session_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetirementPhase {
    Prepared,
    Dispatched,
    DecisionReady,
    ExitRequested,
    Parked,
    Stale,
    Unknown,
    Suppressed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetirementReason {
    Prepared,
    Answered,
    LowConfidence,
    ProviderUnavailable,
    Unconfigured,
    RestartUnknown,
    StateChanged,
    Stopped,
    ExitUnknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionRetirementStatus {
    pub crew_id: String,
    pub session_id: String,
    pub record: Option<RetirementView>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetirementView {
    pub decision_id: String,
    pub phase: RetirementPhase,
    pub reason: RetirementReason,
    pub probability: Option<f64>,
    pub adopted: bool,
    pub generation: String,
}
