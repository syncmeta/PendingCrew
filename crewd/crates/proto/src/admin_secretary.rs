//! Explicit secretary-case wire types. These do not confer role or tool authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const HUMAN_GET: &str = "adminCase.secretaryBrief";
pub const HUMAN_REQUEST: &str = "adminCase.secretaryRequest";
pub const CAPABILITY: &str = "adminCase.secretaryBrief.v1";
pub const SNAPSHOT_TOOL: &str = "admin_secretary_snapshot";
pub const PUBLISH_TOOL: &str = "admin_secretary_publish";
pub const RECEIPT_TOOL: &str = "admin_secretary_receipt";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminSecretaryReference {
    HumanWait { id: String },
    Case { id: String },
    Patrol { id: String },
}
impl AdminSecretaryReference {
    pub fn id(&self) -> &str {
        match self {
            Self::HumanWait { id } | Self::Case { id } | Self::Patrol { id } => id,
        }
    }
    pub fn kind(&self) -> &str {
        match self {
            Self::HumanWait { .. } => "human_wait",
            Self::Case { .. } => "case",
            Self::Patrol { .. } => "patrol",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretarySnapshot {
    pub source_id: String,
    pub case_id: String,
    pub crew_id: String,
    pub scope: crate::managed_objects::Scope,
    pub judgment_epoch: String,
    pub configuration_revision: Option<String>,
    /// Stable completed finding identities and coverage, never an in-flight run.
    pub patrol_revision: Option<String>,
    pub revision: String,
    pub observed_at_ms: i64,
    pub facts: crate::AdminHumanBrief,
    pub references: Vec<AdminSecretaryReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<AdminSecretaryAuthor>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryAuthor {
    pub session_id: String,
    pub attempt_id: String,
    pub generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryPublish {
    pub request_id: String,
    pub source_revision: String,
    pub generation: String,
    pub body: String,
    pub references: Vec<AdminSecretaryReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryPublication {
    pub source_id: String,
    pub case_id: String,
    pub crew_id: String,
    pub source_revision: String,
    pub author_session_id: String,
    pub attempt_id: String,
    pub generation: String,
    pub request_id: String,
    pub revision: String,
    pub body: String,
    pub references: Vec<AdminSecretaryReference>,
    pub published_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminSecretaryState {
    NotRecorded,
    Waiting,
    Writing,
    Current,
    Stale,
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminSecretaryView {
    pub crew_id: String,
    pub observed_at_ms: i64,
    pub state: AdminSecretaryState,
    pub current_revision: Option<String>,
    pub source: Option<AdminSecretarySnapshot>,
    pub case: Option<crate::AdminCaseView>,
    pub publication: Option<AdminSecretaryPublication>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryGet {
    pub crew_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryRequest {
    pub crew_id: String,
    pub expected_revision: String,
}

/// The public command receipt confirms identity only; it must never retain a
/// second copy of Human questions or generated prose outside purgeable sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminSecretaryRequestResult {
    pub crew_id: String,
    pub source_id: String,
    pub case_id: String,
    pub source_revision: String,
}
