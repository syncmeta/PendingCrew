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
    pub id: String,
    pub crew_id: String,
    pub frozen: bool,
    pub title: String,
    pub goal: String,
    pub level: String,
    pub acceptance: String,
    pub owner: Option<String>,
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
    pub id: String,
    pub crew_id: String,
    pub frozen: bool,
    pub title: String,
    pub owner: Option<String>,
    pub status: String,
    pub parent: Option<String>,
    pub blocked_on: Vec<String>,
    pub next_check_at: Option<String>,
    pub revision: String,
    pub note: Option<String>,
}
impl From<WorkPlan> for WorkSummary {
    fn from(p: WorkPlan) -> Self {
        Self {
            id: p.id,
            crew_id: p.crew_id,
            frozen: p.frozen,
            title: p.title,
            owner: p.owner,
            status: p.status,
            parent: p.parent,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkAssignParams {
    pub id: String,
    pub expected_revision: String,
    pub owner: String,
    pub note: String,
}
