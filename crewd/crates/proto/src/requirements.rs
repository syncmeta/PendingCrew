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
