//! Durable observations, never permission to act on stale findings.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const PATROL_START: &str = "work.patrolStart";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatrolStartParams {
    pub crew_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatrolStartResult {
    pub crew_id: String,
    pub run_id: String,
    pub disposition: PatrolStartDisposition,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatrolStartDisposition {
    Started,
    Joined,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatrolStatusParams {
    pub crew_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatrolStatus {
    pub crew_id: String,
    pub run: Option<PatrolRun>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captain_brief: Option<PatrolCaptainBrief>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_run: Option<PatrolRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_run_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatrolCaptainBrief {
    pub run_id: String,
    pub captain_session_id: String,
    pub wake_id: i64,
    pub phase: PatrolCaptainBriefPhase,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatrolCaptainBriefPhase {
    Queued,
    Inflight,
    Delivered,
    Dropped,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatrolRun {
    pub run_id: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub scan: crate::WorkPatrolScan,
    pub after: Option<String>,
    pub captured_seq: i64,
    pub scanned_counts: BTreeMap<String, u64>,
    pub findings: BTreeMap<String, Vec<String>>,
    pub counts: BTreeMap<String, u64>,
    pub truncated: bool,
}
