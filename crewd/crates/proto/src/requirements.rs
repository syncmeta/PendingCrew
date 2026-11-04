//! Human-authored immutable requirements, identified by public organization # IDs.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoCreateParams {
    pub request_id: String,
    pub crew_id: String,
    pub text: String,
    #[serde(default)]
    pub channel_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoCreateLookupParams {
    pub request_id: String,
    pub crew_id: String,
    pub text_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "status",
    deny_unknown_fields
)]
pub enum TodoCreateLookupResult {
    NotFound {
        request_id: String,
    },
    Committed {
        request_id: String,
        receipt: TodoCreateLookupReceipt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoCreateLookupReceipt {
    pub crew_id: String,
    pub requirement_id: String,
    pub text_sha256: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_association: Option<crate::ChannelWorkAssociation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoGetParams {
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoCloseParams {
    pub id: String,
    pub note: String,
    pub expected_status: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingParams {
    pub crew_id: String,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingCheckParams {
    pub crew_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequirementRulingState {
    Ready,
    NotNeedsRuling,
    ReviewIncomplete,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingCheck {
    pub crew_id: String,
    pub id: String,
    pub state: RequirementRulingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_revision: Option<String>,
    pub linked_work_count: usize,
    pub work: Vec<RequirementRulingWork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequirementRulingDrop {
    Duplicate,
    Superseded,
    NotNeeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingWork {
    pub id: String,
    pub crew_id: String,
    pub title: String,
    pub revision: String,
    pub drop_kind: RequirementRulingDrop,
    pub drop_note: String,
    pub drop_note_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRuleCloseParams {
    pub crew_id: String,
    pub id: String,
    pub expected_revision: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequirementRulingBy {
    Human,
    Captain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingReceipt {
    pub crew_id: String,
    pub id: String,
    pub request_id: String,
    pub expected_revision: String,
    pub note: String,
    pub closed_at_ms: i64,
    pub event_seq: String,
    pub by: RequirementRulingBy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementRulingResult {
    pub crew_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RequirementRulingReceipt>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequirementClosure {
    pub note: String,
    pub at: String,
}
/// Existing Todo fields remain at the top level; closure is derived from organization authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequirementDetail {
    #[serde(rename = "todo")]
    pub todo_attributes: crate::TodoAttributes,
    pub derived_status: String,
    #[serde(flatten)]
    pub todo: crate::Todo,
    pub closure: Option<RequirementClosure>,
    /// Only creation receipts carry the current channel association.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_association: Option<crate::ChannelWorkAssociation>,
}
