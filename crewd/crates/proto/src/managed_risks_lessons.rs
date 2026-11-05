//! Typed risk and lesson records from the organization directory.
use crate::ResourceActor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RiskStatus {
    Open,
    Mitigated,
    Occurred,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RiskOwner {
    Human {},
    Session { session_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RiskAbout {
    Crew { id: String },
    Work { id: String },
    Requirement { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RiskMitigation {
    Work { id: String },
    Decision { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskDraft {
    pub summary: String,
    pub impact: RiskLevel,
    pub likelihood: Option<RiskLevel>,
    pub mitigation: Option<String>,
    pub about: Vec<RiskAbout>,
    pub mitigated_by: Vec<RiskMitigation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Risk {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub draft: RiskDraft,
    pub owner: RiskOwner,
    pub author: ResourceActor,
    pub created_at: String,
    pub updated_at: String,
    pub next_review_at: String,
    pub status: RiskStatus,
    pub status_reason: Option<String>,
    pub became_lesson_id: Option<String>,
    pub frozen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LessonStatus {
    Active,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LessonSource {
    Work { id: String },
    Risk { id: String },
    Ask { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum LessonApplicability {
    Crew { id: String },
    Work { id: String },
    Table { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LessonDraft {
    pub happened: String,
    pub lesson: String,
    pub detection: Option<String>,
    pub learned_from: Vec<LessonSource>,
    pub evidence: Vec<String>,
    pub applies_to: Vec<LessonApplicability>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lesson {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub draft: LessonDraft,
    pub author: ResourceActor,
    pub created_at: String,
    pub last_checked_at: String,
    pub status: LessonStatus,
    pub stale_reason: Option<String>,
    pub risk_id: Option<String>,
    pub frozen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskOccurrence {
    pub risk_id: String,
    pub original_risk_revision: String,
    pub result_risk_revision: String,
    pub lesson_id: String,
    pub lesson_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskPage {
    pub items: Vec<Risk>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LessonPage {
    pub items: Vec<Lesson>,
    pub next_after: Option<String>,
}
