//! Administrative escalation evidence. Observation does not imply intervention.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const LIST: &str = "adminCase.list";
pub const GET: &str = "adminCase.get";
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseGetParams {
    pub crew_id: String,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseListParams {
    pub crew_id: String,
    pub after: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseDecision {
    Observed,
    Prepared,
    Dispatched,
    Answered,
    Unknown,
    Unconfigured,
    WaitingAdmission,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseExecution {
    NotStarted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseRetirement {
    NotStarted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseScope {
    InScope,
    OutOfScope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseTargetKind {
    Unanswered,
    Overdue,
    ReadyUnscheduled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseTarget {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub kind: AdminCaseTargetKind,
    pub scope: AdminCaseScope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseSource {
    pub session_id: String,
    pub budget_epoch: i64,
    pub restore_epoch: String,
    pub announcement_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseView {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub created_at: i64,
    pub source: AdminCaseSource,
    pub targets: Vec<AdminCaseTarget>,
    pub decision: AdminCaseDecision,
    #[serde(default)]
    pub decision_record: Option<AdminCaseDecisionView>,
    pub execution: AdminCaseExecution,
    pub retirement: AdminCaseRetirement,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseList {
    pub crew_id: String,
    pub items: Vec<AdminCaseView>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseDecisionReason {
    Prepared,
    NativeAnswered,
    Generated,
    ProviderUnavailable,
    RestartUnknown,
    Unconfigured,
    NoAdmission,
    StateChanged,
    Stopped,
    NoInScopeTargets,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseDecisionView {
    pub decision_id: String,
    pub phase: AdminCaseDecision,
    pub probability: Option<f64>,
    pub adopted: bool,
    pub reason: AdminCaseDecisionReason,
    pub snapshot_witness: String,
}
