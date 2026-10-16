//! Public03 method/schema registry. The immutable DTO source remains unchanged.
use crate::{team_data::*,agent::{ToolDef,ToolHints}};
use serde_json::{Value,json};
pub const METHODS:&[&str]=&["data.workspace.get","data.submit","data.request.get","data.object.get","data.object.list","data.schema.get","data.query","data.table.get","data.history","data.diff","data.branches","data.export"];
pub fn agent_method(name:&str)->Option<&'static str>{match name{
 "data_workspace_get"=>Some(METHODS[0]),"data_submit"=>Some(METHODS[1]),"data_request_get"=>Some(METHODS[2]),"data_object_get"=>Some(METHODS[3]),"data_object_list"=>Some(METHODS[4]),"data_schema_get"=>Some(METHODS[5]),"data_query"=>Some(METHODS[6]),"data_table_get"=>Some(METHODS[7]),"data_history"=>Some(METHODS[8]),"data_diff"=>Some(METHODS[9]),"data_branches"=>Some(METHODS[10]),"data_export"=>Some(METHODS[11]),_=>None}}
fn schema<T:schemars::JsonSchema>()->Value{serde_json::to_value(schemars::schema_for!(T)).expect("static data tool schema")}
#[derive(schemars::JsonSchema,serde::Serialize,serde::Deserialize)]#[serde(rename_all="camelCase",deny_unknown_fields)]struct AgentSubmit{request_id:String,context:DataContext,change:AgentChange}
#[derive(schemars::JsonSchema,serde::Serialize,serde::Deserialize)]#[serde(tag="kind",rename_all="camelCase",deny_unknown_fields)]enum AgentChange{Apply{message:String,edits:Vec<DataEdit>}}
pub fn agent_tools()->Vec<ToolDef>{
 let entries=[
  ("data_workspace_get","Read the currently authorized knowledge workspace without initializing it.",schema::<DataWorkspaceGetParams>()),
  ("data_submit","Apply one bounded knowledge content/schema/view batch using the original request UUID and exact CAS context. Never replay an Unknown request. Only Apply is allowed for agents; version/recovery administration requires a real Human.",schema::<AgentSubmit>()),
  ("data_request_get","Pure lookup of an original request UUID and its original context. NotFound is not proof that no effect happened; do not resubmit automatically.",schema::<DataRequestGetParams>()),
  ("data_object_get","Read one typed block/view/relation at an exact snapshot. Invisible references are Unavailable; historical references never resolve against today's HEAD.",schema::<DataObjectGetParams>()),
  ("data_object_list","Read a bounded typed-object page at a fixed snapshot.",schema::<DataObjectListParams>()),
  ("data_schema_get","Inspect authorized business schema, excluding protected control envelopes.",schema::<DataSchemaParams>()),
  ("data_query","Execute one readonly business SQL statement with typed values and an exact-snapshot cursor. No implicit ordering or initialization.",schema::<DataQueryParams>()),
  ("data_table_get","Read bounded typed table rows. Continuation requires an actual usable nonnull primary key; do not treat truncated unordered rows as complete.",schema::<DataTableParams>()),
  ("data_history","Read the actual native version graph with bounded snapshot-pinned continuation.",schema::<DataHistoryParams>()),
  ("data_diff","Read an authorized business-data version diff; never changes HEAD.",schema::<DataDiffParams>()),
  ("data_branches","Read existing native branches; this tool cannot create or switch branches.",schema::<DataBranchesParams>()),
  ("data_export","Read bounded authorized content/schema/data snapshot pages. This is a logical export, not a paired Core/native recovery backup.",schema::<DataExportParams>()),
 ];
 entries.into_iter().map(|(name,description,input_schema)|{
  ToolDef{name:name.into(),description:description.into(),input_schema,annotations:Some(ToolHints{read_only_hint:name!="data_submit",destructive_hint:name=="data_submit",idempotent_hint:true,open_world_hint:false})}
 }).collect()
}
pub(crate) fn schema_entries(g:&mut schemars::SchemaGenerator)->Vec<(&'static str,Value)>{
 let method=|p:Value,r:Value|json!({"params":p,"result":r});
 vec![
 (METHODS[0],method(crate::sub::<DataWorkspaceGetParams>(g),crate::sub::<DataWorkspaceInfo>(g))),
 (METHODS[1],method(crate::sub::<DataSubmitParams>(g),crate::sub::<DataChangeResult>(g))),
 (METHODS[2],method(crate::sub::<DataRequestGetParams>(g),crate::sub::<DataRequestGetResult>(g))),
 (METHODS[3],method(crate::sub::<DataObjectGetParams>(g),crate::sub::<DataObjectDetail>(g))),
 (METHODS[4],method(crate::sub::<DataObjectListParams>(g),crate::sub::<DataObjectPage>(g))),
 (METHODS[5],method(crate::sub::<DataSchemaParams>(g),crate::sub::<DataSchemaResult>(g))),
 (METHODS[6],method(crate::sub::<DataQueryParams>(g),crate::sub::<DataRows>(g))),
 (METHODS[7],method(crate::sub::<DataTableParams>(g),crate::sub::<DataTablePage>(g))),
 (METHODS[8],method(crate::sub::<DataHistoryParams>(g),crate::sub::<DataHistory>(g))),
 (METHODS[9],method(crate::sub::<DataDiffParams>(g),crate::sub::<DataDiff>(g))),
 (METHODS[10],method(crate::sub::<DataBranchesParams>(g),crate::sub::<DataBranches>(g))),
 (METHODS[11],method(crate::sub::<DataExportParams>(g),crate::sub::<DataExportPage>(g))),
 ]
}
