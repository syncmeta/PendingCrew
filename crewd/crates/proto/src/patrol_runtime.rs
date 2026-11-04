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

/// Latest authoritative responsibility episodes for one crew.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelResponsibilityParams {
    pub crew_id: String,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResponsibilityPage {
    pub crew_id: String,
    pub items: Vec<ChannelResponsibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResponsibility {
    pub channel_id: String,
    pub episode_id: String,
    pub phase: ChannelResponsibilityPhase,
    pub notice_phase: ChannelResponsibilityNoticePhase,
    pub observed_at_ms: i64,
    pub updated_at_ms: i64,
    pub work_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captain_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handled: Option<ChannelResponsibilityHandled>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelResponsibilityPhase {
    Open,
    Handled,
    Ended,
    Stale,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelResponsibilityNoticePhase {
    NotQueued,
    Queued,
    Inflight,
    Delivered,
    Dropped,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResponsibilityHandled {
    pub work_id: String,
    pub owner_session_id: String,
    pub event_seq: String,
    pub at_ms: i64,
}
