//! Group references into the existing SQLite knowledge authority; never copied content.
use crate::team_data::{DataContext, DataRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelKnowledgeReference {
    pub context: DataContext,
    pub reference: DataRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelKnowledgeListParams {
    pub channel_id: String,
    pub after: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelKnowledgeLinkParams {
    pub request_id: String,
    pub channel_id: String,
    pub expected_link_revision: String,
    pub source: ChannelKnowledgeReference,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelKnowledgeReceipt {
    pub channel_id: String,
    pub link_revision: String,
    pub source: ChannelKnowledgeReference,
    pub linked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelKnowledgeSummary {
    pub source: ChannelKnowledgeReference,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelKnowledgePage {
    pub channel_id: String,
    pub link_revision: String,
    pub source_revision: String,
    pub items: Vec<ChannelKnowledgeSummary>,
    pub next: Option<String>,
}
