//! Optional decision service. These are judgments, not runner permission decisions.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentConfig {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub choice_min: f64,
    pub wake_yes: f64,
    /// Storage implementation, scoped to this database and an explicitly written secret slot.
    #[serde(default = "platform")]
    pub credential_source: String,
}
fn platform() -> String {
    "platform".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentSettings {
    pub revision: Option<String>,
    pub config: Option<JudgmentConfig>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentSettingsSet {
    pub request_id: String,
    #[serde(default)]
    pub credential_update: CredentialUpdate,
    pub expected_revision: Option<String>,
    pub config: Option<JudgmentConfig>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentSourceKind {
    Requirement,
    Message,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentInspect {
    pub crew_id: String,
    pub kind: JudgmentSourceKind,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentLink {
    pub kind: JudgmentObjectKind,
    pub id: String,
    pub revision: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentPhase {
    Pending,
    Dispatched,
    Settled,
    Suppressed,
    Historical,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentRecord {
    pub version: u32,
    pub decision_id: String,
    pub db_id: String,
    pub restore_epoch: String,
    pub config_revision: String,
    pub source_version: String,
    pub phase: JudgmentPhase,
    pub prepared_at_ms: i64,
    pub captured_floors: BTreeMap<String, i64>,
    pub captured_stop_epochs: BTreeMap<String, String>,
    pub recipients: Vec<String>,
    pub attribution: Option<JudgmentLink>,
    pub diagnostic: serde_json::Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentInspection {
    pub source: JudgmentInspect,
    pub record: Option<JudgmentView>,
}
pub const SETTINGS_GET: &str = "judgment.settings.get";
pub const SETTINGS_SET: &str = "judgment.settings.set";
pub const INSPECT: &str = "judgment.inspect";
pub const METHODS: &[&str] = &[SETTINGS_GET, SETTINGS_SET, INSPECT];

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialUpdate {
    #[default]
    Keep,
    Set {
        value: String,
    },
    Clear,
}
impl std::fmt::Debug for CredentialUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Set { .. } => "Set(***)",
            Self::Clear => "Clear",
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Missing,
    RecordedUnverified,
    UpdateUnknown,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentSettingsView {
    pub settings: JudgmentSettings,
    pub credential_state: CredentialState,
    pub pending_request_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentConfigurationOutcome {
    Committed,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentSettingsResult {
    pub historical: bool,
    pub request_id: String,
    pub outcome: JudgmentConfigurationOutcome,
    pub view: JudgmentSettingsView,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentUsageView {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentReason {
    Answered,
    Prepared,
    Unconfigured,
    Deadline,
    ProviderError,
    InvalidResponse,
    RestartUnknown,
    Stale,
    Busy,
    ConfigurationChanged,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentView {
    pub decision_id: String,
    pub phase: JudgmentPhase,
    pub reason: JudgmentReason,
    pub backend: JudgmentBackend,
    pub model: Option<String>,
    pub probability_source: Option<JudgmentProbabilitySource>,
    pub calibrated: bool,
    pub attempts: Option<u32>,
    pub latency_ms: Option<u64>,
    pub usage: Option<JudgmentUsageView>,
    pub recipients: Vec<String>,
    pub attribution: Option<JudgmentLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentObjectKind {
    Requirement,
    Work,
    Message,
}
impl JudgmentObjectKind {
    pub fn from_key(value: &str) -> Option<Self> {
        match value {
            "requirement" => Some(Self::Requirement),
            "work" => Some(Self::Work),
            "message" => Some(Self::Message),
            _ => None,
        }
    }
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::Work => "work",
            Self::Message => "message",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentProbabilitySource {
    Native,
    GeneratedEstimate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentBackend {
    Typesafe,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentSettingsGet {
    pub request_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum JudgmentRequestLookup {
    Known { result: Box<JudgmentSettingsResult> },
    NotFound,
    Historical,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgmentSettingsResponse {
    pub view: JudgmentSettingsView,
    pub request_lookup: Option<JudgmentRequestLookup>,
}
