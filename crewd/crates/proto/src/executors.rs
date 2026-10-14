//! Closed read-only executor discovery for the Native15 module.
//! Profile identifiers select configured Core adapters, never arbitrary paths.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const EXECUTOR_LIST: &str = "executor.list";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorListParams {
    /// Required crew scope. Local admitted Human or paired Human with current
    /// orchestra-read and this crew visibility may read the configured list.
    pub crew_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecutorBackend {
    Codex,
    ExistingClaudePty,
    NativeAcp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecutorUnavailableReason {
    NotConfigured,
    BinaryChanged,
    NativeVerificationPending,
    SourceHistoryIncomplete,
    UnsupportedPlatform,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorFeatures {
    pub initial_context: bool,
    pub manual_terminal: bool,
    pub structured_decisions: bool,
    pub full_history_load: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorProfile {
    /// Stable configured ID, not a filesystem path or an authentication token.
    pub id: String,
    /// The existing SessionStartParams.executor value, including acp:<id>.
    pub executor: String,
    pub display_name: String,
    pub backend: ExecutorBackend,
    /// Real factory/producer support. Normal launch authorization and limits
    /// still apply at submission. Unverified Native must remain unavailable.
    pub available: bool,
    pub features: ExecutorFeatures,
    pub unavailable_reason: Option<ExecutorUnavailableReason>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorListResult {
    /// Stable ID sort order. No home/auth/env/program path is disclosed.
    pub profiles: Vec<ExecutorProfile>,
}
