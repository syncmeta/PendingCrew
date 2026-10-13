//! Explicit raw evidence queries; no derived sum, cost, or task classification.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerUsageParams {
    pub session_id: String,
    /// Canonical positive decimal record ID, not a JSON number.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerUsageRecord {
    pub id: String,
    pub session_id: String,
    pub provider: String,
    pub thread_id: String,
    /// Absent/null provider turn means unknown; never inferred from the active turn.
    pub turn_id: Option<String>,
    /// Parsed canonical JSON text preserves fields, missing versus 0, and integer precision.
    /// Consumers must not sum cumulative snapshots or overlapping breakdown fields.
    pub token_usage_json: String,
    /// First accepted actor observation, not a provider timestamp.
    pub observed_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerUsagePage {
    pub items: Vec<RunnerUsageRecord>,
    pub next_after: Option<String>,
}
