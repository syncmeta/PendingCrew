//! Administrative escalation evidence. Observation does not imply intervention.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SIMILAR_REQUIREMENTS: &str = "adminCase.similarRequirements";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SimilarRequirementsParams {
    pub crew_id: String,
    pub after: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SimilarRequirementsPage {
    pub crew_id: String,
    pub enabled: bool,
    pub items: Vec<SimilarRequirement>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SimilarRequirement {
    pub first_id: String,
    pub second_id: String,
    pub probability: f64,
    pub observed_at_ms: i64,
}
pub const LIST: &str = "adminCase.list";
pub const GET: &str = "adminCase.get";
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseGetParams {
    pub crew_id: String,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseListParams {
    pub crew_id: String,
    pub after: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseDecision {
    Observed,
    Prepared,
    Dispatched,
    Answered,
    Unknown,
    Unconfigured,
    WaitingAdmission,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseExecution {
    NotStarted,
    Registered,
    Requested,
    Running,
    Unknown,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseRetirement {
    NotStarted,
    Prepared,
    Dispatched,
    DecisionReady,
    ExitRequested,
    Parked,
    Unknown,
    Stale,
    Suppressed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseScope {
    InScope,
    OutOfScope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseTargetKind {
    Unanswered,
    Overdue,
    ReadyUnscheduled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseTarget {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub kind: AdminCaseTargetKind,
    pub scope: AdminCaseScope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<AdminCaseProvenance>,
    pub session_id: String,
    pub budget_epoch: i64,
    pub restore_epoch: String,
    pub announcement_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseView {
    #[serde(default)]
    pub launch_record: Option<AdminCaseLaunchView>,
    #[serde(default)]
    pub custody_record: Option<AdminCaseCustodyView>,
    #[serde(default)]
    pub handoff_record: Option<AdminCaseHandoffView>,
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub created_at: i64,
    pub source: AdminCaseSource,
    pub targets: Vec<AdminCaseTarget>,
    pub decision: AdminCaseDecision,
    #[serde(default)]
    pub decision_record: Option<AdminCaseDecisionView>,
    pub execution: AdminCaseExecution,
    #[serde(default)]
    pub execution_record: Option<AdminCaseExecutionView>,
    pub retirement: AdminCaseRetirement,
    #[serde(default)]
    pub retirement_record: Option<crate::RetirementView>,
    #[serde(default)]
    pub outcome_record: Option<AdminCaseOutcomeView>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseList {
    pub crew_id: String,
    pub items: Vec<AdminCaseView>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseDecisionReason {
    Prepared,
    NativeAnswered,
    Generated,
    ProviderUnavailable,
    RestartUnknown,
    Unconfigured,
    NoAdmission,
    StateChanged,
    Stopped,
    NoInScopeTargets,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseDecisionView {
    pub decision_id: String,
    pub phase: AdminCaseDecision,
    pub probability: Option<f64>,
    pub adopted: bool,
    pub reason: AdminCaseDecisionReason,
    pub snapshot_witness: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseExecutionReason {
    Registered,
    LaunchRequested,
    Started,
    LaunchUnknown,
    RestartUnknown,
    StateChanged,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseExecutionView {
    pub session_id: String,
    pub attempt_id: String,
    pub phase: AdminCaseExecution,
    pub reason: AdminCaseExecutionReason,
    pub generation: Option<String>,
    pub effects_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseOutcomePhase {
    Unresolved,
    Resolved,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseOutcomeReason {
    NoExecution,
    NoVerifiedAdminEffect,
    TargetsUnresolved,
    OutOfScope,
    Verified,
    AuthorityChanged,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseTargetOutcomeState {
    Resolved,
    Pending,
    Blocked,
    Missing,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseResolutionKind {
    Closed,
    Reassigned,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseResolutionActor {
    CaseAdmin,
    Other,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseTargetOutcomeReason {
    ClosedWithEvidence,
    ReassignedAndAcknowledged,
    NoClosureEvidence,
    NoMatchingEvent,
    OriginalProblemRemains,
    NotificationPending,
    NotificationDropped,
    OwnerUnavailable,
    TargetMissing,
    NoVerifiedAdminReceipt,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseTargetOutcome {
    pub id: String,
    pub source_kinds: Vec<AdminCaseTargetKind>,
    pub state: AdminCaseTargetOutcomeState,
    pub resolution: Option<AdminCaseResolutionKind>,
    pub resolved_by: Option<AdminCaseResolutionActor>,
    pub actor_session_id: Option<String>,
    pub event_seq: Option<String>,
    pub wake_id: Option<i64>,
    pub reason: AdminCaseTargetOutcomeReason,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseOutcomeView {
    pub case_id: String,
    pub crew_id: String,
    pub revision: String,
    pub verified_at: i64,
    pub phase: AdminCaseOutcomePhase,
    pub reason: AdminCaseOutcomeReason,
    pub targets: Vec<AdminCaseTargetOutcome>,
    pub out_of_scope: Vec<String>,
    pub missing: Vec<String>,
    pub pending: Vec<String>,
    pub blocked: Vec<String>,
    pub resolved_by_admin: Vec<String>,
    pub resolved_by_others: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseHandoffPhase {
    Waiting,
    Delivered,
    Accepted,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseHandoffReason {
    Requested,
    NotificationDelivered,
    CaptainAccepted,
    StateChanged,
    Stopped,
    RecipientChanged,
    NotificationDropped,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseHandoffView {
    pub request_id: String,
    pub case_id: String,
    pub crew_id: String,
    pub revision: String,
    pub phase: AdminCaseHandoffPhase,
    pub reason: AdminCaseHandoffReason,
    pub admin_session_id: String,
    pub source_session_id: String,
    pub captain_session_id: String,
    pub request_reason: String,
    pub targets: Vec<AdminCaseTarget>,
    pub wake_id: i64,
    pub created_at: i64,
    pub delivered_at: Option<String>,
    pub accepted_at: Option<i64>,
    pub acceptance_note: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseHandoffList {
    pub items: Vec<AdminCaseHandoffView>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseCustodyPhase {
    Active,
    Stale,
    Discharged,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseCustodyTransferPhase {
    Waiting,
    Delivered,
    Accepted,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseCustodyReason {
    OriginalCustody,
    Proposed,
    NotificationDelivered,
    CaptainAccepted,
    StateChanged,
    NotificationDropped,
    Superseded,
    TargetsUnresolved,
    OutOfScope,
    TargetMissing,
    EvidenceIncomplete,
    Verified,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyPropose {
    pub crew_id: String,
    pub case_id: String,
    pub request_id: String,
    pub expected_revision: String,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyGetParams {
    pub crew_id: String,
    pub case_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyTransferGetParams {
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyTransfer {
    pub request_id: String,
    pub case_id: String,
    pub crew_id: String,
    pub predecessor_receipt_id: String,
    pub from_session_id: String,
    pub to_session_id: String,
    pub phase: AdminCaseCustodyTransferPhase,
    pub revision: String,
    pub reason: AdminCaseCustodyReason,
    pub request_reason: String,
    pub targets: Vec<AdminCaseTarget>,
    pub wake_id: i64,
    pub created_at: i64,
    pub delivered_at: Option<String>,
    pub accepted_at: Option<i64>,
    pub acceptance_note: Option<String>,
    pub superseded_by: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyDischarge {
    pub revision: String,
    pub verified_at: i64,
    pub targets: Vec<AdminCaseTargetOutcome>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyView {
    pub case_id: String,
    pub crew_id: String,
    pub revision: String,
    pub original_request_id: String,
    pub current_custodian_session_id: String,
    pub current_receipt_id: String,
    pub phase: AdminCaseCustodyPhase,
    pub reason: AdminCaseCustodyReason,
    pub targets: Vec<AdminCaseTarget>,
    pub target_results: Vec<AdminCaseTargetOutcome>,
    pub out_of_scope: Vec<String>,
    pub pending: Option<AdminCaseCustodyTransfer>,
    pub current_record: Option<AdminCaseCustodyTransfer>,
    pub discharge: Option<AdminCaseCustodyDischarge>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseCustodyList {
    pub items: Vec<AdminCaseCustodyTransfer>,
    pub next_after: Option<String>,
}

/// Scheduling intent only. Bound execution is described by executionRecord.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLaunchPhase {
    Waiting,
    Dispatching,
    Bound,
    Unknown,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLaunchReason {
    Ready,
    SourceNotRunning,
    SourceAdmissionMissing,
    CapacityUnavailable,
    ExecutorUnsupported,
    NativeAuthUnavailable,
    RuntimeUnavailable,
    SettingsDisabled,
    SourceStateChanged,
    WorkspaceUnavailable,
    ShuttingDown,
    Claimed,
    Bound,
    RestartUnknown,
    StateChanged,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLaunchView {
    pub phase: AdminCaseLaunchPhase,
    pub reason: AdminCaseLaunchReason,
    pub revision: String,
    pub decision_id: String,
    pub updated_at: i64,
    pub attempt_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminCaseProvenance {
    HumanRevalidation {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "legacyMessageId")]
        legacy_message_id: String,
        #[serde(rename = "announcementDigest")]
        announcement_digest: String,
        #[serde(rename = "previewRevision")]
        preview_revision: String,
        note: String,
        #[serde(rename = "revalidatedAt")]
        revalidated_at: i64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLegacyState {
    Observed,
    Linked,
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyView {
    pub message_id: String,
    pub message_seq: i64,
    pub historical_text: String,
    pub created_at: String,
    pub revision: String,
    pub state: AdminCaseLegacyState,
    pub history_complete: bool,
    pub linked_case_id: Option<String>,
    pub linked_crew_id: Option<String>,
    pub reason: AdminCaseLegacyReason,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyList {
    pub items: Vec<AdminCaseLegacyView>,
    pub next_after: Option<String>,
    pub scan: AdminCaseLegacyScan,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyPreviewParams {
    pub message_id: String,
    pub crew_id: String,
    pub session_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLegacyBlocker {
    InvalidHistory,
    SourceUnavailable,
    CrewUnavailable,
    Stopped,
    MissingBudget,
    NotEscalated,
    BudgetMismatch,
    NoTargets,
    NoInScopeTargets,
    IndexIncomplete,
    AlreadyLinked,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyPreview {
    pub message_id: String,
    pub crew_id: String,
    pub session_id: String,
    pub revision: String,
    pub can_confirm: bool,
    pub blockers: Vec<AdminCaseLegacyBlocker>,
    pub targets: Vec<AdminCaseTarget>,
    pub budget_epoch: Option<i64>,
    pub restore_epoch: String,
    pub existing_case_id: Option<String>,
    pub observed_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyConfirmParams {
    pub message_id: String,
    pub crew_id: String,
    pub session_id: String,
    pub request_id: String,
    pub expected_preview_revision: String,
    pub note: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyReceipt {
    pub request_id: String,
    pub message_id: String,
    pub crew_id: String,
    pub session_id: String,
    pub case_id: String,
    pub created: bool,
    pub confirmed_at: i64,
    pub note: String,
    pub preview_revision: String,
}
pub const LEGACY_LIST: &str = "adminCase.legacyList";
pub const LEGACY_GET: &str = "adminCase.legacyGet";
pub const LEGACY_PREVIEW: &str = "adminCase.legacyPreview";
pub const LEGACY_CONFIRM: &str = "adminCase.legacyConfirm";
pub const LEGACY_RECEIPT: &str = "adminCase.legacyReceipt";
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyListParams {
    pub after: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyGetParams {
    pub message_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyReceiptParams {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLegacyReason {
    HistoricalHintOnly,
    InvalidHistory,
    InvalidEvidence,
    HumanRevalidated,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminCaseLegacyScanPhase {
    Indexing,
    Scanning,
    Current,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminCaseLegacyScan {
    pub phase: AdminCaseLegacyScanPhase,
    pub through_seq: i64,
    pub captured_upper: i64,
    pub unverified_sources: u32,
}
