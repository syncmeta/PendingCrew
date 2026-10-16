//! One registry for implemented SQLite knowledge methods.
use crate::{
    agent::{ToolDef, ToolHints},
    team_data::*,
};
use serde_json::{Value, json};
pub const METHODS: &[&str] = &[
    "data.workspace.get",
    "data.submit",
    "data.request.get",
    "data.object.get",
    "data.object.list",
    "data.schema.get",
    "data.query",
    "data.table.get",
    "data.history",
    "data.preview",
    "data.confirm",
    "data.restore",
    "data.snapshots",
    "data.preview.get",
];
pub fn agent_method(name: &str) -> Option<&'static str> {
    METHODS.iter().copied().find(|m| {
        !matches!(
            *m,
            "data.confirm" | "data.restore" | "data.snapshots" | "data.preview.get"
        ) && m.replace('.', "_") == name
    })
}
pub fn agent_tools() -> Vec<ToolDef> {
    let mut g = schemars::SchemaGenerator::default();
    schema_entries(&mut g).into_iter()
        .filter(|(m, _)| !matches!(*m, "data.confirm" | "data.restore" | "data.snapshots" | "data.preview.get"))
        .map(|(m, s)| ToolDef {
            name: m.replace('.', "_"),
            description: format!("SQLite knowledge {m}. Current crew ACL applies. Use the original request UUID; never replay an Unknown outcome. Dangerous SQL needs real Human confirmation.{}", if m == "data.submit" { " For executable Mac views use definition kind='web', renderer='web', rendererVersion='1'. Supply your own HTML/CSS/JavaScript and declared bindings. The closed JavaScript bridge is window.knowledge.request(operation,input), resolving {callID,response}; read uses {binding:'bindingId'} and returns response.value. Writes use operation='write' with exactly {message,edits} and require requestedCapabilities:['knowledge-write'] plus separate native Human code-enable and write grants for this version. Readonly views need no write capability. Unknown writes must be looked up through the retained original writeCallID; never retry by inventing a new call ID. The bridge also supports lookup with {writeCallID} and navigate with an authorized Core reference; it grants no arbitrary RPC, network or file access." } else { "" }),
            input_schema: s["params"].clone(),
            annotations: Some(ToolHints { read_only_hint: !matches!(m, "data.submit" | "data.preview"), destructive_hint: m == "data.submit", idempotent_hint: true, open_world_hint: false }),
        }).collect()
}
pub(crate) fn schema_entries(g: &mut schemars::SchemaGenerator) -> Vec<(&'static str, Value)> {
    vec![
        (
            METHODS[0],
            json!({"params":crate::sub::<DataWorkspaceGetParams>(g),"result":crate::sub::<DataWorkspaceInfo>(g)}),
        ),
        (
            METHODS[1],
            json!({"params":crate::sub::<DataSubmitParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[2],
            json!({"params":crate::sub::<DataRequestGetParams>(g),"result":crate::sub::<DataRequestGetResult>(g)}),
        ),
        (
            METHODS[3],
            json!({"params":crate::sub::<DataObjectGetParams>(g),"result":crate::sub::<DataObjectDetail>(g)}),
        ),
        (
            METHODS[4],
            json!({"params":crate::sub::<DataObjectListParams>(g),"result":crate::sub::<DataObjectPage>(g)}),
        ),
        (
            METHODS[5],
            json!({"params":crate::sub::<DataSchemaParams>(g),"result":crate::sub::<DataSchemaResult>(g)}),
        ),
        (
            METHODS[6],
            json!({"params":crate::sub::<DataQueryParams>(g),"result":crate::sub::<DataRows>(g)}),
        ),
        (
            METHODS[7],
            json!({"params":crate::sub::<DataTableParams>(g),"result":crate::sub::<DataTablePage>(g)}),
        ),
        (
            METHODS[8],
            json!({"params":crate::sub::<DataHistoryParams>(g),"result":crate::sub::<DataHistory>(g)}),
        ),
        (
            METHODS[9],
            json!({"params":crate::sub::<DataPreviewParams>(g),"result":crate::sub::<DataPreview>(g)}),
        ),
        (
            METHODS[10],
            json!({"params":crate::sub::<DataConfirmParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[11],
            json!({"params":crate::sub::<DataRestoreParams>(g),"result":crate::sub::<DataChangeResult>(g)}),
        ),
        (
            METHODS[12],
            json!({"params":crate::sub::<DataSnapshotsParams>(g),"result":crate::sub::<DataSnapshots>(g)}),
        ),
        (
            METHODS[13],
            json!({"params":crate::sub::<DataPreviewGetParams>(g),"result":crate::sub::<DataPreview>(g)}),
        ),
    ]
}
