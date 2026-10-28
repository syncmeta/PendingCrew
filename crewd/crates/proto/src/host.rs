//! Read-only owner host metadata.
use serde::{Serialize, Deserialize};
use schemars::JsonSchema;
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DirectoryParams { pub path: Option<String>, pub after: Option<String>, pub limit: Option<usize> }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase")]
pub struct DirectoryEntry { pub name: String, pub path: String }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase")]
pub struct DirectoryResult { pub path: String, pub parent_path: Option<String>, pub entries: Vec<DirectoryEntry>, pub next_after: Option<String> }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase")]
pub struct QuotaResult { pub codex: serde_json::Value, pub fetched_at: String }
