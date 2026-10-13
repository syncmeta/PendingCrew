//! Explicit, read-only operational facts; not an administrative decision.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("type" = ["object", "null"]))]
pub struct WorkPatrolParams {
    #[serde(default)]
    pub crew_id: Option<String>,
    /// Global W cursor. The limit bounds scanned live items, not findings.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkPatrolPage {
    /// One observation time for the whole page. Pagination is not a frozen snapshot.
    pub observed_at: String,
    /// Only live work in non-removed crews is scanned. Excluded work is not cleared.
    pub live_only: bool,
    pub include_removed: bool,
    /// These pure-patrol categories are not evaluated by this projection.
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
    /// Use ask.get({crewId: row.crewId, askId}) for detail; not the global ID.
    pub ask_id: String,
}
