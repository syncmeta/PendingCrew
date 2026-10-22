//! Personal drafts are inert text/mention/reply values, never execution inputs.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const STATUS: &str = "personalDraft.status";
pub const MEMBERS: &str = "personalDraft.members";
pub const CATALOG: &str = "personalDraft.catalog";
pub const BRANCHES: &str = "personalDraft.branches";
pub const REQUEST_FENCE: &str = "personalDraft.requestFence";
pub const MAX_PAYLOAD_BYTES: usize = 65_536;
pub const MAX_SPACE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_BRANCHES: usize = 4096;
pub const MAX_MEMBERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftScope {
    pub space_id: String,
    pub space_epoch: String,
    pub member_id: String,
    pub member_revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum PersonalDraftDestination {
    Channel(String),
    OriginalSession(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PersonalDraftKind {
    ChannelMessage,
    SessionMessage,
    InitialTask,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftKey {
    pub authority_id: String,
    pub db_id: String,
    pub crew_id: String,
    pub destination: PersonalDraftDestination,
    pub kind: PersonalDraftKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMention {
    pub kind: String,
    #[serde(rename = "target_id")]
    pub target_id: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftStagedMention {
    pub token: String,
    pub mention: PersonalDraftMention,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftReply {
    pub mention: Option<PersonalDraftMention>,
    pub reply_to_id: String,
    pub quoted_sender: String,
    pub quoted_snippet: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftPayload {
    pub text: String,
    pub staged_mentions: Vec<PersonalDraftStagedMention>,
    pub reply_target: Option<PersonalDraftReply>,
}
impl std::fmt::Debug for PersonalDraftPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PersonalDraftPayload([redacted])")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMember {
    pub member_id: String,
    pub writer_id: String,
    pub revision: String,
    pub grant_id: Option<String>,
    pub grant_revision: Option<String>,
    pub client_spki_sha256: Option<String>,
    pub opted_in: bool,
    pub revoked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftStatusParams {}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftStatus {
    pub db_id: String,
    pub owner: bool,
    pub owner_epoch: Option<String>,
    pub available: bool,
    pub reason: Option<String>,
    pub space_id: Option<String>,
    pub space_epoch: Option<String>,
    pub space_revision: String,
    pub member: Option<PersonalDraftMember>,
    pub payload_limit_bytes: u32,
    pub space_limit_bytes: u32,
    pub branch_limit: u32,
    pub member_limit: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftOwnerParams {
    pub owner_epoch: String,
    pub expected_space_id: Option<String>,
    pub expected_space_revision: String,
    pub local_enabled: bool,
    pub acknowledge_restore: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMemberSetParams {
    pub space_id: String,
    pub space_epoch: String,
    pub expected_space_revision: String,
    pub grant_id: String,
    pub grant_revision: String,
    pub client_spki_sha256: String,
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftConsentParams {
    pub scope: PersonalDraftScope,
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftCheckpointParams {
    pub scope: PersonalDraftScope,
    pub branch_id: String,
    pub key: PersonalDraftKey,
    pub expected_revision: String,
    pub revision: String,
    pub payload: PersonalDraftPayload,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftRetire {
    pub branch_id: String,
    pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftReplacement {
    pub branch_id: String,
    pub revision: String,
    pub payload: PersonalDraftPayload,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftResolveParams {
    pub scope: PersonalDraftScope,
    pub key: PersonalDraftKey,
    pub retiring: Vec<PersonalDraftRetire>,
    pub replacement: Option<PersonalDraftReplacement>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMembersParams {
    pub space_id: String,
    pub space_epoch: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMembers {
    pub space_id: String,
    pub space_epoch: String,
    pub revision: String,
    pub members: Vec<PersonalDraftMember>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftCatalogParams {
    pub scope: PersonalDraftScope,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftBranchesParams {
    pub scope: PersonalDraftScope,
    pub key: PersonalDraftKey,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftBranch {
    pub branch_id: String,
    pub key: PersonalDraftKey,
    pub writer_id: String,
    pub revision: String,
    pub retired: bool,
    pub payload: Option<PersonalDraftPayload>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftCatalogEntry {
    pub key: PersonalDraftKey,
    pub active_branches: u32,
    pub retired_branches: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftCatalogPage {
    pub scope: PersonalDraftScope,
    pub entries: Vec<PersonalDraftCatalogEntry>,
    pub membership_cut: String,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub observed_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftBranchPage {
    pub scope: PersonalDraftScope,
    pub branches: Vec<PersonalDraftBranch>,
    pub membership_cut: String,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub observed_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftMutationResult {
    pub operation: String,
    pub space_id: String,
    pub space_epoch: String,
    pub space_revision: String,
    pub member: Option<PersonalDraftMember>,
    pub branch: Option<PersonalDraftBranch>,
    pub retired: Vec<PersonalDraftRetire>,
}
/// This writes a negative receipt; it is not a read/lookup or a resubmission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftRequestFenceParams {
    pub original: crate::commands::CommandSubmitParams,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PersonalDraftFenceState {
    ExistingTerminal,
    ExistingPending,
    MissingFenced,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftRequestFence {
    pub state: PersonalDraftFenceState,
    pub receipt: crate::commands::PublicCommandReceipt,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostPersonalDraftOwnerParams {
    pub request_id: String,
    pub db_id: String,
    pub runtime_binding_id: String,
    pub params: PersonalDraftOwnerParams,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostPersonalDraftMemberParams {
    pub request_id: String,
    pub db_id: String,
    pub runtime_binding_id: String,
    pub params: PersonalDraftMemberSetParams,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalDraftChanged {
    pub space_id: String,
    pub space_epoch: String,
    pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostPersonalDraftStatus {
    pub runtime_binding_id: String,
    pub status: PersonalDraftStatus,
}
