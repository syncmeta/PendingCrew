//! Public work views. Issued IDs and revisions are text, safe for all clients.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkListParams {
    #[serde(default)]
    pub crew_id: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub live_only: bool,
    #[serde(default)]
    pub include_removed: Option<bool>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkGetParams {
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkEvidence {
    pub kind: String,
    pub value: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPlan {
    /// Receipt-only association; work.get does not retain a channel projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_association: Option<crate::ChannelWorkAssociation>,
    pub id: String,
    pub crew_id: String,
    pub frozen: bool,
    pub title: String,
    pub goal: String,
    pub level: String,
    pub acceptance: String,
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_transfer: Option<WorkPendingTransfer>,
    pub status: String,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
    pub sources: Vec<String>,
    pub blocked_on: Vec<String>,
    pub next_check_at: Option<String>,
    pub revision: String,
    pub evidence: Option<WorkEvidence>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPage {
    pub items: Vec<WorkSummary>,
    pub next_after: Option<String>,
}

/// Lists carry working context; long goals and evidence are fetched with work.get.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_association: Option<crate::ChannelWorkAssociation>,
    pub id: String,
    pub crew_id: String,
    pub frozen: bool,
    pub title: String,
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_transfer: Option<WorkTransferHint>,
    pub status: String,
    pub parent: Option<String>,
    /// Stable requirement references permit lightweight linkage without fetching full plans.
    #[serde(default)]
    pub sources: Vec<String>,
    pub blocked_on: Vec<String>,
    pub next_check_at: Option<String>,
    pub revision: String,
    pub note: Option<String>,
}
impl From<WorkPlan> for WorkSummary {
    fn from(p: WorkPlan) -> Self {
        Self {
            channel_association: p.channel_association,
            id: p.id,
            crew_id: p.crew_id,
            frozen: p.frozen,
            title: p.title,
            owner: p.owner,
            pending_transfer: p.pending_transfer.map(|t| WorkTransferHint {
                to: t.to,
                request_seq: t.request_seq,
            }),
            status: p.status,
            parent: p.parent,
            sources: p.sources,
            blocked_on: p.blocked_on,
            next_check_at: p.next_check_at,
            revision: p.revision,
            note: p.note,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkLevel {
    #[default]
    Local,
    Public,
    Production,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkUpdateStatus {
    Open,
    Active,
    Done,
    Dropped,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkEvidenceKind {
    Commit,
    Url,
    Text,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkEvidenceParam {
    pub kind: WorkEvidenceKind,
    pub value: String,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkCreateParams {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    pub crew_id: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub level: WorkLevel,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub next_check_at: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkUpdateParams {
    pub id: String,
    pub expected_revision: String,
    #[serde(default)]
    pub status: Option<WorkUpdateStatus>,
    #[serde(default)]
    pub next_check_at: Option<String>,
    #[serde(default)]
    pub evidence: Option<WorkEvidenceParam>,
    #[serde(default)]
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkChanged {
    pub crew_id: String,
    pub id: String,
}

/// Bounded current facts accompanying a rejected revision; never a write result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkConflictFacts {
    pub id: String,
    pub revision: String,
    pub owner: Option<String>,
    pub status: String,
    pub next_check_at: Option<String>,
    pub blocker_count: usize,
    pub note: Option<String>,
    pub note_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_transfer: Option<WorkPendingTransfer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkAssignParams {
    pub id: String,
    pub expected_revision: String,
    pub owner: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkHistoryParams {
    pub id: String,
    pub before: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkHistoryActor {
    Human,
    System,
    Session { id: String, role: crate::Role },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkOwnerChange {
    pub seq: String,
    pub from: Option<String>,
    pub to: String,
    pub by: WorkHistoryActor,
    pub how: String,
    pub note: String,
    pub at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkHistoryPage {
    pub id: String,
    pub revision: String,
    pub total: usize,
    pub items: Vec<WorkOwnerChange>,
    pub next_before: Option<String>,
}

/// Current offer, not evidence that the target accepted ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPendingTransfer {
    /// Actual organization request event identity; unknown for migrated legacy offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_seq: Option<String>,
    pub to: String,
    pub requested_by: String,
    pub note: String,
    pub note_truncated: bool,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkBlockOn {
    Work {
        id: String,
    },
    Human {
        id: String,
    },
    NewHuman {
        question: String,
        #[serde(default)]
        default: Option<crate::AskDefault>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkContinuation {
    Nothing,
    Split { items: Vec<WorkCreateParams> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkBlockParams {
    pub id: String,
    pub expected_revision: String,
    pub on: Vec<WorkBlockOn>,
    pub can_continue: WorkContinuation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkBlockReply {
    pub item: WorkPlan,
    pub human_tasks: Vec<String>,
    pub split: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkTransferHint {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_seq: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTransferRequestParams {
    pub id: String,
    pub expected_revision: String,
    pub to: String,
    pub note: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTransferResolveParams {
    pub id: String,
    pub expected_revision: String,
}
