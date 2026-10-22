//! Source-bound native input staging; opaque IDs never authorize execution.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const INPUT_PREPARE: &str = "session.codex.inputPrepare";
pub const INPUT_LOOKUP: &str = "session.codex.inputLookup";
pub const INPUT_DISCARD: &str = "session.codex.inputDiscard";
pub const PERMISSION_PROFILES: &str = "session.codex.permissionProfiles";
pub const MAX_MEDIA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TEXT_FILE_BYTES: usize = 512 * 1024;
pub const MAX_SELECTION_REFS: usize = 16;
pub const MAX_SESSION_INPUT_BYTES: i64 = 64 * 1024 * 1024;
pub const MAX_CORE_INPUT_BYTES: i64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CodexInputKind {
    Image,
    Audio,
    Text,
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexInputPrepareParams {
    pub request_id: String,
    pub crew_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: CodexInputKind,
    pub filename: String,
    pub mime_type: String,
    pub sha256: String,
    pub bytes_base64: String,
}
impl std::fmt::Debug for CodexInputPrepareParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodexInputPrepareParams([private input redacted])")
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexInputLookupParams {
    pub crew_id: String,
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexInputDiscardParams {
    pub request_id: String,
    pub crew_id: String,
    pub input: CodexInputRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexInputRef {
    pub input_id: String,
    pub sha256: String,
    pub kind: CodexInputKind,
    pub bytes: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CodexInputResult {
    Prepared { input: CodexInputRef },
    Discarded { input_id: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CodexInputLookup {
    NotFound,
    Known { result: CodexInputResult },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexSkillSelection {
    pub name: String,
    pub path: String,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexInputSelection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<CodexInputRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<CodexSkillSelection>,
}
impl CodexInputSelection {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.inputs.len() + self.skills.len() == 0
            || self.inputs.len() + self.skills.len() > MAX_SELECTION_REFS
        {
            return Err("native input selection must contain 1..16 elements");
        }
        let mut ids = std::collections::BTreeSet::new();
        for input in &self.inputs {
            if !crate::valid_remote_uuid(&input.input_id)
                || input.sha256.len() != 64
                || !input
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || input.bytes == 0
                || input.bytes as usize > MAX_MEDIA_BYTES
                || !ids.insert(input.input_id.clone())
            {
                return Err("invalid or duplicate original input reference");
            }
        }
        let mut paths = std::collections::BTreeSet::new();
        for skill in &self.skills {
            if skill.name.trim().is_empty()
                || skill.name.len() > 256
                || skill.name.chars().any(char::is_control)
                || skill.path.len() > 4096
                || !std::path::Path::new(&skill.path).is_absolute()
                || !paths.insert(&skill.path)
            {
                return Err("invalid skill catalog selection");
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexPermissionProfilesParams {
    pub session_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexPermissionProfile {
    pub id: String,
    pub allowed: bool,
    pub description: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexPermissionProfiles {
    pub items: Vec<CodexPermissionProfile>,
    pub next_cursor: Option<String>,
}
