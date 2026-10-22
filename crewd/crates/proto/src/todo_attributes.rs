//! Source-global Human Todo facts and channel-local multivalue classification.
use crate::{Ask, ChannelWorkAssociation, ChannelWorkObject, MemberRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TodoOrigin {
    Requirement,
    AgentAsk,
    HumanTask,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ManualTodoState {
    Open,
    Done,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoAttributes {
    pub object: ChannelWorkObject,
    pub crew_id: String,
    pub revision: String,
    pub origin: TodoOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MemberRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_state: Option<ManualTodoState>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoAttributesGetParams {
    pub object: ChannelWorkObject,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoAssignParams {
    pub request_id: String,
    pub object: ChannelWorkObject,
    pub channel_id: String,
    pub expected_revision: String,
    #[serde(default)]
    pub owner: Option<MemberRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoSetStateParams {
    pub request_id: String,
    pub object: ChannelWorkObject,
    pub channel_id: String,
    pub expected_revision: String,
    pub state: ManualTodoState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanTaskCreateParams {
    pub request_id: String,
    pub crew_id: String,
    pub channel_id: String,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HumanTaskCreateResult {
    pub ask: Ask,
    pub todo: TodoAttributes,
    pub channel_association: ChannelWorkAssociation,
    pub request_id: String,
    pub reused: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelWorkTagsSetParams {
    pub request_id: String,
    pub channel_id: String,
    pub object: ChannelWorkObject,
    pub expected_link_revision: String,
    pub tags: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkTagsReceipt {
    pub channel_id: String,
    pub link_revision: String,
    pub object: ChannelWorkObject,
    pub tags: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub tag: String,
    pub count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceRequestLookupParams {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRequestLookupResult {
    pub request_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoAttributesChanged {
    pub crew_id: String,
    pub object: ChannelWorkObject,
    pub revision: String,
}
