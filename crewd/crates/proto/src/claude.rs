//! PCW-owned managed PTY task observations, not provider transcript/full history.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub const INSPECT: &str = "session.claude.inspect";
pub const HISTORY: &str = "session.claude.history";
pub const TASK_GET: &str = "session.claude.taskGet";
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeSessionParams {
    pub session_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeHistoryParams {
    pub session_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}
fn default_limit() -> u32 {
    32
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeTaskGetParams {
    pub session_id: String,
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeResumeEligibility {
    Eligible,
    Running,
    NotConfigured,
    MissingOriginalId,
    PreviousBirthLiveOrUnknown,
    RemovedCrew,
    MissingWorkdir,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeManualResume {
    pub eligibility: ClaudeResumeEligibility,
    pub original_agent_session_id: Option<String>,
    pub rechecks_program_and_source_on_launch: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeInspect {
    pub session_id: String,
    pub crew_id: String,
    pub executor_profile_id: String,
    pub backend: String,
    pub history_scope: String,
    pub full_history_load: bool,
    pub native_acp: bool,
    pub manual_resume: ClaudeManualResume,
    pub current_runtime_id: Option<String>,
    pub task_text_limit_bytes: u32,
    pub response_observation_limit_bytes: u32,
    pub retained_observations_per_task: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeTaskSummary {
    pub request_id: String,
    pub runtime_id: String,
    pub created_at: String,
    pub updated_at: String,
    /// Original receipt phase; completed means observed managed waiting only.
    pub phase: String,
    pub receipt_revision: String,
    pub prompt_id: Option<String>,
    pub observation_count: String,
    pub omitted_observations: String,
    pub latest_observed_at: Option<String>,
    /// Current runtime/birth + current receipt + latest matched waiting observation.
    pub waiting_observed: bool,
    pub current_runtime: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeHistoryPage {
    pub session_id: String,
    pub tasks: Vec<ClaudeTaskSummary>,
    pub next_cursor: Option<String>,
    pub membership_cut: String,
    pub observed_at: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeTaskText {
    pub text: Option<String>,
    pub managed_envelope: Option<String>,
    pub fidelity: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeResponseObservation {
    pub observation_no: String,
    pub prompt_id: String,
    pub observed_at: String,
    pub text: Option<String>,
    pub source_bytes: Option<String>,
    pub captured_bytes: String,
    pub source_sha256: Option<String>,
    pub availability: String,
}
// Bodies deliberately never enter Debug/error/log output.
impl std::fmt::Debug for ClaudeResponseObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeResponseObservation")
            .field("observation_no", &self.observation_no)
            .field("body", &"<private>")
            .finish()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeTaskDetail {
    pub session_id: String,
    pub task: ClaudeTaskSummary,
    pub original_task: ClaudeTaskText,
    pub observations: Vec<ClaudeResponseObservation>,
    pub latest_observation: Option<ClaudeResponseObservation>,
    pub waiting_observation_no: Option<String>,
    pub observed_at: String,
}

impl std::fmt::Debug for ClaudeTaskText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeTaskText")
            .field("fidelity", &self.fidelity)
            .field("body", &"<private>")
            .finish()
    }
}

pub const RECOVERY_INSPECT: &str = "session.claude.recoveryInspect";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct ClaudeRecoveryResolveParams {
    pub target: crate::commands::CommandRuntimeTarget,
    pub original_agent_session_id: String,
    pub evidence_token: String,
    pub original_request_ids: Vec<String>,
}

/// Read-only evidence for a separate, explicit disposition of original resumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeRecoveryEligibility {
    Eligible,
    Running,
    MissingOriginalId,
    NotConfigured,
    MissingWorkdir,
    RemovedCrew,
    PreviousBirthLiveOrUnknown,
    EvidenceUnavailable,
    NoUnresolvedRequests,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeRecoveryUnknown {
    pub request_id: String,
    pub runtime_binding_id: String,
    pub phase: ClaudeRecoveryUnknownPhase,
    pub historical: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeRecoveryDisposition {
    pub request_id: String,
    pub original_request_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeRecoveryInspect {
    pub session_id: String,
    #[schemars(required, extend("type" = ["string", "null"]))]
    pub original_agent_session_id: Option<String>,
    pub target: crate::commands::CommandRuntimeTarget,
    pub eligibility: ClaudeRecoveryEligibility,
    #[schemars(required, extend("type" = ["string", "null"]))]
    pub evidence_token: Option<String>,
    pub unresolved: Vec<ClaudeRecoveryUnknown>,
    pub dispositions: Vec<ClaudeRecoveryDisposition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase")]
pub enum ClaudeRecoveryUnknownPhase { Unknown }

#[cfg(test)]
mod recovery_contract_tests {
    #[test]
    fn recovery_inspection_has_a_typed_registered_contract() {
        let api=crate::api_schema();
        assert!(crate::methods::ALL.contains(&super::RECOVERY_INSPECT));
        assert_eq!(api["methods"][super::RECOVERY_INSPECT]["result"]["$ref"],"#/$defs/ClaudeRecoveryInspect");
        assert_eq!(api["$defs"]["ClaudeRecoveryInspect"]["additionalProperties"],false);
        assert_eq!(api["$defs"]["ClaudeRecoveryUnknownPhase"]["enum"],serde_json::json!(["unknown"]));
        assert_eq!(api["$defs"]["ClaudeRecoveryEligibility"]["enum"].as_array().unwrap().len(),9);
        for key in ["originalAgentSessionId", "evidenceToken"] {
            assert!(api["$defs"]["ClaudeRecoveryInspect"]["required"].as_array().unwrap().contains(&serde_json::json!(key)));
            assert_eq!(api["$defs"]["ClaudeRecoveryInspect"]["properties"][key]["type"],serde_json::json!(["string","null"]));
        }

    }
}
