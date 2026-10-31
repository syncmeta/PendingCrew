//! Explicit, read-only operational facts; not an administrative decision.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("type" = ["object", "null"]))]
pub struct WorkPatrolParams {
    #[serde(default)]
    pub scan: WorkPatrolScan,
    #[serde(default)]
    pub crew_id: Option<String>,
    /// Cursor for the selected source: W identity, # identity, or channel ID.
    /// The limit bounds scanned sources, not findings.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolPage {
    #[serde(default)]
    pub scan: WorkPatrolScan,
    #[serde(default)]
    pub entries: Vec<WorkPatrolEntry>,
    #[serde(default)]
    pub channels: Vec<WorkPatrolChannel>,
    /// One observation time for the whole page. Pagination is not a frozen snapshot.
    pub observed_at: String,
    /// Only live work in non-removed crews is scanned. Excluded work is not cleared.
    pub live_only: bool,
    pub include_removed: bool,
    /// Categories not evaluated by this page's selected scan.
    pub unavailable_categories: Vec<String>,
    /// Includes scanned items with no findings; use work.get for authoritative detail.
    pub items: Vec<WorkPatrolItem>,
    pub next_after: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolItem {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub unowned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lost: Option<WorkPatrolLost>,
    /// Global budget escalated AND this item has a current stop violation.
    pub stop_escalated: bool,
    /// Open human blockers meeting the existing pinned-wait threshold. Maximum 8.
    pub long_waits: Vec<WorkPatrolWait>,
    pub long_waits_truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum WorkPatrolLost {
    Unrecoverable {
        #[serde(rename = "resumeFailedAt")]
        resume_failed_at: String,
    },
    Silent {
        #[serde(rename = "missedChecks")]
        missed_checks: u32,
    },
}

/// Issued global identity plus the existing ask.get readback handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolWait {
    pub id: String,
    /// The human task's crew, which may differ from the blocked work item's crew.
    pub crew_id: String,
    /// Public H identity; ask.get uses this wait's crewId and askId.
    pub ask_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkPatrolScan {
    #[default]
    Work,
    Entries,
    Channels,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolEntry {
    pub id: String,
    pub crew_id: String,
    pub unclaimed: bool,
    pub needs_ruling: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolChannel {
    pub id: String,
    pub crew_id: String,
    /// Historical work exists and all observed work is terminal. NOT permission
    /// to archive: message activity, authority and links must be rechecked.
    pub archive_candidate: bool,
    pub blocked_by: Vec<WorkPatrolChannelBlocker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkPatrolChannelBlocker {
    NoLinkedWork,
    LiveWork,
    NonWorkLinks,
    SourceRemoved,
    LinksTruncated,
}
