//! Closed Human Codex session surface. Provider JSON is read-only evidence;
//! no public value can select an arbitrary provider RPC method or config key.
use crate::commands::CommandRuntimeTarget;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const INSPECT: &str = "session.codex.inspect";
pub const HISTORY: &str = "session.codex.history";
pub const MODELS: &str = "session.codex.models";
pub const TOOLS: &str = "session.codex.tools";
pub const CHANGED: &str = "session.codex.changed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", untagged, deny_unknown_fields)]
pub enum CodexApprovalPolicy {
    Simple(CodexSimpleApproval),
    Granular { granular: CodexGranularApproval },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexSimpleApproval {
    Untrusted,
    OnRequest,
    Never,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexGranularApproval {
    pub mcp_elicitations: bool,
    pub request_permissions: bool,
    pub rules: bool,
    pub sandbox_approval: bool,
    pub skill_approval: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexApprovalsReviewer {
    User,
    AutoReview,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CodexSandboxPolicy {
    #[serde(rename_all = "camelCase")]
    ReadOnly {
        network_access: bool,
    },
    #[serde(rename_all = "camelCase")]
    WorkspaceWrite {
        network_access: bool,
    },
    DangerFullAccess,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexCollaborationMode {
    Default,
    Plan,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexReasoningSummary {
    Auto,
    Concise,
    Detailed,
    None,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<CodexApprovalPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<CodexApprovalsReviewer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<CodexSandboxPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CodexCollaborationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<CodexReasoningSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
}
impl CodexSettingsPatch {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self == &Self::default() {
            return Err("empty Codex settings patch");
        }
        if self.permissions.is_some() && self.sandbox.is_some() {
            return Err("native permissions and sandbox are mutually exclusive");
        }
        for s in [&self.model, &self.effort, &self.permissions]
            .into_iter()
            .flatten()
        {
            if s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control) {
                return Err("invalid Codex model/effort");
            }
        }
        Ok(())
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexObservedSettings {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
    pub service_tier: Option<String>,
    pub approval_policy: Option<CodexApprovalPolicy>,
    pub approvals_reviewer: Option<CodexApprovalsReviewer>,
    pub sandbox: Option<CodexSandboxPolicy>,
    pub collaboration_mode: Option<CodexCollaborationMode>,
    pub summary: Option<CodexReasoningSummary>,
    pub permission_profile: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexTargetParams {
    pub target: CommandRuntimeTarget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexConfigureParams {
    pub target: CommandRuntimeTarget,
    pub patch: CodexSettingsPatch,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexTextParams {
    pub target: CommandRuntimeTarget,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<crate::codex_inputs::CodexInputSelection>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexDeleteParams {
    pub target: CommandRuntimeTarget,
    pub confirm_native_history_delete: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexRevertParams {
    pub target: CommandRuntimeTarget,
    pub before_turn_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexTitleParams {
    pub target: CommandRuntimeTarget,
    pub title: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexForkParams {
    pub target: CommandRuntimeTarget,
    #[serde(default)]
    pub last_turn_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CodexReviewTarget {
    UncommittedChanges,
    BaseBranch {
        branch: String,
    },
    Commit {
        sha: String,
        #[serde(default)]
        title: Option<String>,
    },
    Custom {
        instructions: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexReviewParams {
    pub target: CommandRuntimeTarget,
    pub review_target: CodexReviewTarget,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexHistoryParams {
    pub session_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
    #[serde(default)]
    pub items_view: Option<CodexHistoryItems>,
    #[serde(default)]
    pub sort_direction: Option<CodexSortDirection>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexHistoryItems {
    NotLoaded,
    Summary,
    Full,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexSortDirection {
    Asc,
    Desc,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexHistoryPage {
    pub session_id: String,
    pub thread_id: String,
    pub items: Vec<serde_json::Value>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexHistoryAction {
    Archive,
    Unarchive,
    Delete,
    Revert,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexNativeHistoryStatus {
    Active,
    Archived,
    Deleted,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexNativeHistoryState {
    pub status: CodexNativeHistoryStatus,
    pub request_id: Option<String>,
    pub operation: Option<CodexHistoryAction>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexApplication {
    FollowingTurns,
    CurrentTurn,
    Started,
    Metadata,
    Queued,
    Unknown,
    None,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexControlResult {
    pub session_id: String,
    pub thread_id: Option<String>,
    pub operation: String,
    pub provider_accepted: bool,
    pub application: CodexApplication,
    pub observed_settings: Option<CodexObservedSettings>,
    pub accepted_turn_id: Option<String>,
    pub detail: Option<String>,
    pub new_session_id: Option<String>,
    pub native_thread_id: Option<String>,
    pub native_name: Option<String>,
    pub delivery_id: Option<String>,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexTitleStatus {
    #[default]
    Pending,
    Synced,
    Unknown,
    Rejected,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexTitleSource {
    Human,
    #[default]
    Task,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexTitleState {
    pub desired: Option<String>,
    pub source: CodexTitleSource,
    pub native: Option<String>,
    pub status: CodexTitleStatus,
    pub detail: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexCapabilityAvailability {
    Available,
    RequiresRunning,
    ProviderRejected,
    PcwNotImplemented,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexProtocol {
    Stable,
    Experimental,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexCapability {
    pub id: String,
    pub availability: CodexCapabilityAvailability,
    pub protocol: CodexProtocol,
    pub detail: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionInspection {
    pub session_id: String,
    pub thread_id: Option<String>,
    pub target: CommandRuntimeTarget,
    pub running: bool,
    pub working: bool,
    pub queued_inputs: u32,
    pub settings: Option<CodexObservedSettings>,
    pub settings_observed_at: Option<String>,
    pub historical: bool,
    pub desired_settings: Option<CodexSettingsPatch>,
    pub settings_status: Option<String>,
    pub title: CodexTitleState,
    pub capabilities: Vec<CodexCapability>,
    pub pending_request: Option<crate::orchestra::RunnerRequestView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_history: Option<CodexNativeHistoryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_mode: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexModel {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub is_default: bool,
    pub supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    pub default_reasoning_effort: Option<String>,
    pub input_modalities: Option<Vec<String>>,
    pub supports_personality: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexReasoningEffort {
    pub reasoning_effort: String,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexModels {
    pub items: Vec<CodexModel>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodexTools {
    pub mcp_servers: Vec<serde_json::Value>,
    pub skills: Vec<serde_json::Value>,
}

#[cfg(test)]
mod completion_contract_tests {
    use super::*;
    use crate::commands::PublicCommand;
    use serde_json::json;

    #[test]
    fn completion_named_permission_selection_is_closed_and_excludes_sandbox() {
        let selected: CodexSettingsPatch =
            serde_json::from_value(json!({"permissions":"read-only"}))
                .expect("typed native permission selector must be wired");
        selected.validate().unwrap();
        let conflict: CodexSettingsPatch = serde_json::from_value(
            json!({"permissions":"read-only","sandbox":{"type":"dangerFullAccess"}}),
        )
        .unwrap();
        assert!(conflict.validate().is_err());
        assert!(
            serde_json::from_value::<CodexSettingsPatch>(
                json!({"config":{"permissions":"anything"}})
            )
            .is_err()
        );
    }

    #[test]
    fn completion_native_history_commands_are_original_typed_human_operations() {
        let target = json!({"sessionId":"original","generation":"birth","controlRevision":"7","turnId":null});
        for (operation, extra) in [
            ("session.codex.archive", json!({})),
            ("session.codex.unarchive", json!({})),
            (
                "session.codex.delete",
                json!({"confirmNativeHistoryDelete":true}),
            ),
            (
                "session.codex.revert",
                json!({"beforeTurnId":"original-turn"}),
            ),
        ] {
            let mut params = extra;
            params["target"] = target.clone();
            let cmd: PublicCommand =
                serde_json::from_value(json!({"operation":operation,"params":params}))
                    .expect("native history operation is a closed public command, never a raw RPC");
            assert_eq!(cmd.method(), operation);
        }
    }
}
