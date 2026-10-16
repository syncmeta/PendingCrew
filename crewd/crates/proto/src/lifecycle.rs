//! Local Human daemon management, independent of crew data ACL.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const METHODS: &[&str] = &[
    "lifecycle.status",
    "lifecycle.claim",
    "lifecycle.release",
    "lifecycle.exit",
];
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleEmpty {}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleStatus {
    pub role: String,
    pub owner_present: bool,
    pub previous_exit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleExitParams {
    pub mode: String,
    pub timeout_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleExitResult {
    pub status: String,
    pub mode: String,
    pub clean: Vec<String>,
    pub interrupted: Vec<String>,
    pub failed: Vec<String>,
}
pub(crate) fn schema_entries(g: &mut schemars::SchemaGenerator) -> Vec<(&'static str, Value)> {
    let mut entries=METHODS[..3].iter().map(|m|(*m,json!({"params":crate::sub::<LifecycleEmpty>(g),"result":crate::sub::<LifecycleStatus>(g)}))).collect::<Vec<_>>();
    entries.push((METHODS[3],json!({"params":crate::sub::<LifecycleExitParams>(g),"result":crate::sub::<LifecycleExitResult>(g)})));
    entries
}
