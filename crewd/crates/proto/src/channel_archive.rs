//! Explicit evidence-backed topic archive checks; no inactivity threshold or automatic action.
use crate::Channel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const CHECK: &str = "channel.archiveCheck";
pub const ARCHIVE: &str = "channel.archiveVerified";
pub const RESTORE: &str = "channel.restoreVerified";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelArchiveCheckParams {
    pub channel_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelArchiveVerifiedParams {
    pub channel_id: String,
    pub expected_revision: String,
    pub evidence: String,
    pub note: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelRestoreVerifiedParams {
    pub channel_id: String,
    pub expected_revision: String,
    pub note: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelArchiveBlocker {
    NoLinkedWork,
    NotTopic,
    CrewArchived,
    NotAuthorized,
    AlreadyArchived,
    LiveWork,
    OpenRequirement,
    OpenAsk,
    PendingWake,
    InflightWake,
    ActiveDelivery,
    PendingJudgment,
    PendingOwnerTransfer,
    Incomplete,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelArchiveFacts {
    pub linked_work: u64,
    pub live_work: u64,
    pub linked_requirements: u64,
    pub open_requirements: u64,
    pub open_asks: u64,
    pub stale_wakes: u64,
    pub pending_wakes: u64,
    pub inflight_wakes: u64,
    pub active_deliveries: u64,
    pub pending_judgments: u64,
    pub last_message_seq: Option<String>,
    pub last_message_at: Option<String>,
    pub incomplete: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelArchiveActionKind {
    Archive,
    Restore,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelArchiveAction {
    pub action: ChannelArchiveActionKind,
    pub actor: String,
    pub at: String,
    pub note: String,
    pub evidence: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelArchiveCheck {
    pub channel: Channel,
    pub revision: String,
    pub can_archive: bool,
    pub can_restore: bool,
    pub blockers: Vec<ChannelArchiveBlocker>,
    pub facts: ChannelArchiveFacts,
    pub last_action: Option<ChannelArchiveAction>,
}
