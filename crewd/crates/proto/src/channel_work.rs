//! Channel-local references project the organization authority without another lifecycle book.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChannelWorkKind {
    Requirement,
    Work,
    HumanTask,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelWorkObject {
    pub kind: ChannelWorkKind,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelWorkListParams {
    pub channel_id: String,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelWorkLinkParams {
    pub channel_id: String,
    pub expected_link_revision: String,
    pub object: ChannelWorkObject,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkSummary {
    pub object: ChannelWorkObject,
    pub crew_id: String,
    pub label: String,
    pub label_truncated: bool,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkPage {
    pub channel_id: String,
    pub link_revision: String,
    /// Organization event high-water bound into the opaque cursor. A change explicitly expires it.
    pub source_revision: String,
    pub items: Vec<ChannelWorkSummary>,
    pub next: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkLinkReceipt {
    pub channel_id: String,
    pub link_revision: String,
    pub object: ChannelWorkObject,
    pub linked: bool,
}
/// Creation receipt only. Ordinary object reads never imply this association is still present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkAssociation {
    pub channel_id: String,
    pub link_revision: String,
    pub linked: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWorkChanged {
    pub channel_id: String,
    pub link_revision: String,
}
