//! Durable public command correlation. UUIDs never confer actor authority.
use crate::{CrewCreateParams, MessagePostParams, orchestra};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Correlation issued by this exact daemon instance. Clients bind it only after
/// kernel peer/child-birth validation (local) or verified pairing (remote).
/// JSON alone never proves process ownership or grants authority to shut down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIdentity {
    pub db_id: String,
    pub runtime_binding_id: String,
}

pub const LOCAL_CLIENT_TRUST_STATUS: &str = "local.clientTrust.status";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalClientTrustStatusParams {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalClientTrustStatus {
    pub valid_exact_code: bool,
    pub bootstrap_allowed: bool,
    pub trust_id: Option<String>,
}

pub const COMMAND_SUBMIT: &str = "command.submit";
pub const COMMAND_LOOKUP: &str = "command.lookup";
pub const COMMAND_LIST_UNKNOWN: &str = "command.listUnknown";
pub const SESSION_TERMINAL_INSPECT: &str = "session.terminal.inspect";
pub const SESSION_TERMINAL_WATCH: &str = "session.terminal.watch";
pub const TERMINAL_OUTPUT_EVENT: &str = "session.terminal.output";
pub const SESSION_CONTROL_TARGET: &str = "session.controlTarget";
pub const MAX_COMMAND_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 64 * 1024;

/// A target is issued by Core and pins an actual runtime. Client-selected session
/// IDs or an old receipt cannot target a new generation or a replacement turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRuntimeTarget {
    pub session_id: String,
    pub generation: String,
    /// Core mutation epoch invalidates delayed controls after a new turn/input.
    pub control_revision: String,
    #[serde(default)]
    pub turn_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirstMessageCrewParams {
    pub title: String,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirstMessageParams {
    pub crew: FirstMessageCrewParams,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeStopParams {
    pub target: CommandRuntimeTarget,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDecisionParams {
    pub target: CommandRuntimeTarget,
    /// Exact structured runner request, never inferred from rendered TUI text.
    pub fingerprint: String,
    pub runner_request_id: String,
    pub response: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWriteParams {
    pub target: CommandRuntimeTarget,
    pub lease_id: String,
    pub bytes_base64: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResizeParams {
    pub target: CommandRuntimeTarget,
    pub lease_id: String,
    pub cols: u16,
    pub rows: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LocalClientTrustMode {
    ExactCode,
    Publisher,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalClientTrustParams {
    /// Selection only; OS kernel PID/birth/UID/code are independently verified.
    #[serde(default)]
    pub client_pid: Option<u32>,
    pub mode: LocalClientTrustMode,
    pub expected_cd_hash: String,
    #[serde(default)]
    pub expected_team: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalClientTrustRevokeParams {
    pub trust_id: String,
}
/// Closed typed whitelist. Resource-specific requestId/CAS methods keep their
/// existing contracts; this is not an arbitrary RPC proxy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "params", deny_unknown_fields)]
pub enum PublicCommand {
    #[serde(rename="session.claude.recoveryResolve")]
    ClaudeRecoveryResolve(crate::claude::ClaudeRecoveryResolveParams),
    #[serde(rename="personalDraft.ownerConfigure")]
    PersonalDraftOwner(crate::personal_drafts::PersonalDraftOwnerParams),
    #[serde(rename="personalDraft.memberSet")]
    PersonalDraftMember(crate::personal_drafts::PersonalDraftMemberSetParams),
    #[serde(rename="personalDraft.consentSet")]
    PersonalDraftConsent(crate::personal_drafts::PersonalDraftConsentParams),
    #[serde(rename="personalDraft.checkpoint")]
    PersonalDraftCheckpoint(crate::personal_drafts::PersonalDraftCheckpointParams),
    #[serde(rename="personalDraft.resolve")]
    PersonalDraftResolve(crate::personal_drafts::PersonalDraftResolveParams),
    #[serde(rename = "local.clientTrust")]
    LocalClientTrust(LocalClientTrustParams),
    #[serde(rename = "local.clientTrustRevoke")]
    LocalClientTrustRevoke(LocalClientTrustRevokeParams),
    /// Explicit local owned-child Quit only. Remote clients cancel their own
    /// connection/watch/lease instead of stopping the remote team.
    #[serde(rename = "crewd.hibernate")]
    Hibernate(orchestra::HibernateParams),
    #[serde(rename = "crew.create")]
    CrewCreate(CrewCreateParams),
    #[serde(rename = "crew.createFirstMessage")]
    CrewCreateFirstMessage(FirstMessageParams),
    #[serde(rename = "message.post")]
    MessagePost(MessagePostParams),
    #[serde(rename = "session.start")]
    SessionStart(orchestra::SessionStartParams),
    #[serde(rename = "session.codex.configure")]
    CodexConfigure(crate::codex::CodexConfigureParams),
    #[serde(rename = "session.codex.compact")]
    CodexCompact(crate::codex::CodexTargetParams),
    #[serde(rename = "session.codex.steer")]
    CodexSteer(crate::codex::CodexTextParams),
    #[serde(rename = "session.codex.queue")]
    CodexQueue(crate::codex::CodexTextParams),
    #[serde(rename = "session.codex.review")]
    CodexReview(crate::codex::CodexReviewParams),
    #[serde(rename = "session.codex.titleSet")]
    CodexTitleSet(crate::codex::CodexTitleParams),
    #[serde(rename = "session.codex.fork")]
    CodexFork(crate::codex::CodexForkParams),
    #[serde(rename = "session.codex.archive")]
    CodexArchive(crate::codex::CodexTargetParams),
    #[serde(rename = "session.codex.unarchive")]
    CodexUnarchive(crate::codex::CodexTargetParams),
    #[serde(rename = "session.codex.delete")]
    CodexDelete(crate::codex::CodexDeleteParams),
    #[serde(rename = "session.codex.revert")]
    CodexRevert(crate::codex::CodexRevertParams),
    #[serde(rename = "session.stop")]
    SessionStop(RuntimeStopParams),
    #[serde(rename = "session.resume")]
    SessionResume(CommandRuntimeTarget),
    #[serde(rename = "session.interrupt")]
    SessionInterrupt(CommandRuntimeTarget),
    #[serde(rename = "session.answer")]
    SessionAnswer(RuntimeDecisionParams),
    #[serde(rename = "session.respondRunnerRequest")]
    SessionRespondRunnerRequest(RuntimeDecisionParams),
    #[serde(rename = "session.terminal.claim")]
    TerminalClaim(TerminalClaimParams),
    #[serde(rename = "session.terminal.release")]
    TerminalRelease(TerminalReleaseParams),
    #[serde(rename = "session.terminal.write")]
    TerminalWrite(TerminalWriteParams),
    #[serde(rename = "session.terminal.resize")]
    TerminalResize(TerminalResizeParams),
}
impl PublicCommand {
    pub fn codex_target(&self) -> Option<&CommandRuntimeTarget> {
        match self {
            Self::CodexConfigure(p)=>Some(&p.target), Self::CodexCompact(p)=>Some(&p.target),
            Self::CodexSteer(p)|Self::CodexQueue(p)=>Some(&p.target),Self::CodexReview(p)=>Some(&p.target),
            Self::CodexTitleSet(p)=>Some(&p.target),Self::CodexFork(p)=>Some(&p.target),
            Self::CodexArchive(p)|Self::CodexUnarchive(p)=>Some(&p.target),Self::CodexDelete(p)=>Some(&p.target),Self::CodexRevert(p)=>Some(&p.target),
            Self::SessionInterrupt(p)=>Some(p),_=>None,
        }
    }
    pub fn native_history_action(&self) -> Option<crate::codex::CodexHistoryAction> {
        use crate::codex::CodexHistoryAction as A;
        match self { Self::CodexArchive(_)=>Some(A::Archive),Self::CodexUnarchive(_)=>Some(A::Unarchive),Self::CodexDelete(_)=>Some(A::Delete),Self::CodexRevert(_)=>Some(A::Revert),_=>None }
    }
    pub fn authorized_methods(&self) -> &'static [&'static str] {
        match self {
            Self::PersonalDraftOwner(_) => &["personalDraft.ownerConfigure"],
            Self::PersonalDraftMember(_) => &["personalDraft.memberSet"],
            Self::PersonalDraftConsent(_) => &["personalDraft.consentSet"],
            Self::PersonalDraftCheckpoint(_) => &["personalDraft.checkpoint"],
            Self::PersonalDraftResolve(_) => &["personalDraft.resolve"],
            Self::LocalClientTrust(_) => &["local.clientTrust"],
            Self::LocalClientTrustRevoke(_) => &["local.clientTrustRevoke"],
            Self::Hibernate(_) => &["crewd.hibernate"],
            Self::CrewCreateFirstMessage(_) => &["crew.create", "message.post"],
            Self::CrewCreate(_) => &["crew.create"],
            Self::MessagePost(_) => &["message.post"],
            Self::SessionStart(_) => &["session.start"],
            Self::CodexConfigure(_) => &["session.codex.configure"],
            Self::CodexCompact(_) => &["session.codex.compact"],
            Self::CodexSteer(_) => &["session.codex.steer"],
            Self::CodexQueue(_) => &["session.codex.queue"],
            Self::CodexReview(_) => &["session.codex.review"],
            Self::CodexTitleSet(_) => &["session.codex.titleSet"],
            Self::CodexFork(_) => &["session.codex.fork"],
            Self::CodexArchive(_) => &["session.codex.archive"],
            Self::CodexUnarchive(_) => &["session.codex.unarchive"],
            Self::CodexDelete(_) => &["session.codex.delete"],
            Self::CodexRevert(_) => &["session.codex.revert"],
            Self::SessionStop(_) => &["session.stop"],
            Self::SessionResume(_) | Self::ClaudeRecoveryResolve(_) => &["session.resume"],
            Self::SessionInterrupt(_) => &["session.interrupt"],
            Self::SessionAnswer(_) => &["session.answer"],
            Self::SessionRespondRunnerRequest(_) => &["session.respondRunnerRequest"],
            Self::TerminalClaim(_) => &["session.terminal.claim"],
            Self::TerminalRelease(_) => &["session.terminal.release"],
            Self::TerminalWrite(_) => &["session.terminal.write"],
            Self::TerminalResize(_) => &["session.terminal.resize"],
        }
    }
    pub fn method(&self) -> &'static str {
        match self {
            Self::PersonalDraftOwner(_) => "personalDraft.ownerConfigure",
            Self::PersonalDraftMember(_) => "personalDraft.memberSet",
            Self::PersonalDraftConsent(_) => "personalDraft.consentSet",
            Self::PersonalDraftCheckpoint(_) => "personalDraft.checkpoint",
            Self::PersonalDraftResolve(_) => "personalDraft.resolve",
            Self::LocalClientTrust(_) => "local.clientTrust",
            Self::LocalClientTrustRevoke(_) => "local.clientTrustRevoke",
            Self::Hibernate(_) => "crewd.hibernate",
            Self::CrewCreate(_) => "crew.create",
            Self::CrewCreateFirstMessage(_) => "crew.createFirstMessage",
            Self::MessagePost(_) => "message.post",
            Self::SessionStart(_) => "session.start",
            Self::CodexConfigure(_) => "session.codex.configure",
            Self::CodexCompact(_) => "session.codex.compact",
            Self::CodexSteer(_) => "session.codex.steer",
            Self::CodexQueue(_) => "session.codex.queue",
            Self::CodexReview(_) => "session.codex.review",
            Self::CodexTitleSet(_) => "session.codex.titleSet",
            Self::CodexFork(_) => "session.codex.fork",
            Self::CodexArchive(_) => "session.codex.archive",
            Self::CodexUnarchive(_) => "session.codex.unarchive",
            Self::CodexDelete(_) => "session.codex.delete",
            Self::CodexRevert(_) => "session.codex.revert",
            Self::SessionStop(_) => "session.stop",
            Self::ClaudeRecoveryResolve(_) => "session.claude.recoveryResolve",
            Self::SessionResume(_) => "session.resume",
            Self::SessionInterrupt(_) => "session.interrupt",
            Self::SessionAnswer(_) => "session.answer",
            Self::SessionRespondRunnerRequest(_) => "session.respondRunnerRequest",
            Self::TerminalClaim(_) => "session.terminal.claim",
            Self::TerminalRelease(_) => "session.terminal.release",
            Self::TerminalWrite(_) => "session.terminal.write",
            Self::TerminalResize(_) => "session.terminal.resize",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandSubmitParams {
    pub request_id: String,
    pub db_id: String,
    pub runtime_binding_id: String,
    pub command: PublicCommand,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandLookupParams {
    pub request_id: String,
    pub db_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CommandPhase {
    Reserved,
    Committed,
    Admitted,
    Dispatched,
    Launched,
    Written,
    Resized,
    Confirmed,
    Unknown,
    Suppressed,
    Rejected,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicCommandReceipt {
    pub runtime_binding_id: String,
    pub request_id: String,
    pub db_id: String,
    pub command: PublicCommand,
    pub payload_sha256: String,
    pub phase: CommandPhase,
    pub result: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    /// A copied historical receipt never proves current external completion.
    pub historical: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandListUnknownParams {
    pub db_id: String,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
/// A bounded summary cannot expose or concatenate large original body/result
/// frames. Full read-only lookup remains separate and re-authorized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommandSummary {
    pub runtime_binding_id: String,
    pub request_id: String,
    pub db_id: String,
    pub operation: String,
    pub payload_sha256: String,
    pub phase: CommandPhase,
    pub created_at: String,
    pub updated_at: String,
    pub historical: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommandUnknownPage {
    pub items: Vec<CommandSummary>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalClaimParams {
    pub target: CommandRuntimeTarget,
    /// Explicit controlled handoff; never guessed or silently stolen.
    #[serde(default)]
    pub previous_lease_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalReleaseParams {
    pub target: CommandRuntimeTarget,
    pub lease_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalInspectParams {
    pub session_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalWatchParams {
    pub target: CommandRuntimeTarget,
    #[serde(default)]
    pub after: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TerminalOutputKind {
    Pty,
    Runner,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInspection {
    pub target: CommandRuntimeTarget,
    pub output_kind: TerminalOutputKind,
    pub screen: Option<String>,
    pub oldest_cursor: String,
    pub next_cursor: String,
    pub truncated: bool,
    pub exited: bool,
    pub manual_control_available: bool,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub screen_bytes_base64: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutput {
    pub target: CommandRuntimeTarget,
    pub cursor: String,
    pub next_cursor: String,
    /// Lossless output, bounded to64KiB decoded per complete encoded frame.
    pub bytes_base64: String,
    pub truncated: bool,
    pub exited: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWriteResult {
    pub input_bytes: u32,
    /// Exact bytes accepted by the kernel PTY, when observed. No native reader
    /// consumption or successful application command is inferred.
    pub written_bytes: Option<u32>,
    pub unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWatchStarted {
    pub watch_id: String,
    pub target: CommandRuntimeTarget,
    pub after: String,
    pub initial_snapshot: Option<TerminalInspection>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWatchEvent {
    pub watch_id: String,
    pub output: TerminalOutput,
    pub closed: bool,
}

#[cfg(test)]
mod first_message_default_directory_tests {
    use super::*;

    #[test]
    fn first_message_default_directory_wire_is_optional_without_relaxing_regular_create() {
        for crew in [
            serde_json::json!({"title":"Reykjavik"}),
            serde_json::json!({"title":"Reykjavik","workingDirectory":null}),
        ] {
            let parsed = serde_json::from_value::<FirstMessageParams>(
                serde_json::json!({"crew":crew,"text":"first human message"}),
            );
            assert!(
                parsed.is_ok(),
                "firstMessage must accept an authority-local default: {parsed:?}"
            );
            assert_eq!(
                serde_json::to_value(parsed.unwrap()).unwrap()["crew"]["workingDirectory"],
                serde_json::Value::Null
            );
        }
        assert!(
            serde_json::from_value::<CrewCreateParams>(
                serde_json::json!({"title":"ordinary create"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<CrewCreateParams>(
                serde_json::json!({"title":"ordinary create","workingDirectory":null})
            )
            .is_err()
        );
    }
}
