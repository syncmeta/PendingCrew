//! Independent attribution; never changes reply, mentions, Ask, work ownership or wake plans.
use crate::judgments::{JudgmentLink, JudgmentObjectKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const GET: &str = "message.attribution.get";
pub const SET: &str = "message.attribution.set";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttributionSource {
    Model,
    Human,
    None,
    Stale,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttributionAction {
    Set,
    Clear,
    Reset,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttributionTarget {
    pub kind: JudgmentObjectKind,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageAttributionGet {
    pub crew_id: String,
    pub message_id: String,
    #[serde(default)]
    pub target: Option<AttributionTarget>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageAttributionSet {
    pub crew_id: String,
    pub message_id: String,
    pub expected_revision: String,
    pub action: AttributionAction,
    #[serde(default)]
    pub link: Option<JudgmentLink>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttributionCandidate {
    pub link: JudgmentLink,
    pub summary: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttributionOverride {
    pub action: AttributionAction,
    pub link: Option<JudgmentLink>,
    pub actor: String,
    pub at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttributionView {
    pub crew_id: String,
    pub message_id: String,
    pub revision: String,
    pub source: AttributionSource,
    pub link: Option<JudgmentLink>,
    pub r#override: Option<MessageAttributionOverride>,
    pub candidates: Vec<AttributionCandidate>,
    pub candidates_truncated: bool,
    pub sample_only: bool,
    pub resolved_candidate: Option<AttributionCandidate>,
}
