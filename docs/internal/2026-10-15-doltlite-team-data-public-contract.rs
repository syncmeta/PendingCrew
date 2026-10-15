//! DoltLite workspace public contract; activation and advertisement require Core16.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataContext {
    pub db_id:String, pub authority_id:String, pub restore_epoch:String,
    pub crew_id:String, pub namespace:String, pub branch:String, pub head:Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataRef { pub namespace:String, pub kind:String, pub id:String }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSource { pub reference:Option<DataRefState>, pub at_commit:Option<String>, pub attributes:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataProvenance {
    pub created_by:super::ResourceActor, pub updated_by:super::ResourceActor,
    pub created_at:String, pub updated_at:String, pub sources:Vec<DataSource>, pub attributes:Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataPlacement { pub id:String, pub block:DataRefState, pub at_commit:Option<String>, pub layout:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="type", content="value", rename_all="camelCase")]
pub enum DataCell { Null, Integer(String), Real(f64), Text(String), BlobHex(String) }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSql { pub sql:String, pub params:Vec<DataCell> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataBindingSource {
    Query { statement:DataSql },
    Object { reference:DataRefState, at_commit:Option<String> },
    Objects { object_type:String, limit:u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataBinding { pub id:String, pub source:DataBindingSource, pub attributes:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataViewDefinition {
    Document { placements:Vec<DataPlacement>, layout:Value },
    Web { renderer:String, renderer_version:String, html:String, css:String, javascript:String, bindings:Vec<DataBinding>, requested_capabilities:Vec<String>, attributes:Value },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataObject {
    Block { id:String, revision:String, content_type:String, content:Value, archived:bool, provenance:DataProvenance },
    View { id:String, revision:String, title:String, definition:DataViewDefinition, archived:bool, provenance:DataProvenance },
    Relation { id:String, revision:String, subject:DataRefState, predicate:String, object:DataRefState, attributes:Value, archived:bool, provenance:DataProvenance },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataEdit {
    CreateBlock { alias:String, content_type:String, content:Value, sources:Vec<DataSourceInput>, attributes:Value },
    UpdateBlock { id:String, expected_revision:String, content_type:String, content:Value, sources:Option<Vec<DataSourceInput>>, attributes:Value },
    ForkBlock { alias:String, source:DataRef, at_commit:Option<String>, attributes:Value },
    ArchiveBlock { id:String, expected_revision:String, archived:bool },
    CreateView { alias:String, title:String, definition:DataViewDefinitionInput, sources:Vec<DataSourceInput>, attributes:Value },
    UpdateView { id:String, expected_revision:String, title:String, definition:DataViewDefinitionInput, sources:Option<Vec<DataSourceInput>>, attributes:Value },
    ArchiveView { id:String, expected_revision:String, archived:bool },
    CreateRelation { alias:String, subject:DataInputRef, predicate:String, object:DataInputRef, attributes:Value },
    ArchiveRelation { id:String, expected_revision:String, archived:bool },
    Sql { statements:Vec<DataSql> },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataChange {
    Initialize,
    Apply { message:String, edits:Vec<DataEdit> },
    CreateBranch { name:String },
    SwitchBranch { name:String },
    MergeBranch { name:String, expected_source_head:String, message:String, resolutions:Vec<DataConflictResolution> },
    RestoreVersion { target_commit:String },
    ResolveUnknown { original_request_id:String, observed_head:String, acknowledge_observed_state:bool },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataConflictResolution { pub table:String, pub choice:String }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSubmitParams { pub request_id:String, pub context:DataContext, pub change:DataChange }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataRequestGetParams { pub request_id:String, pub context:DataContext }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataObjectVersion { pub reference:DataRef, pub revision:String }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase")]
pub enum DataPhase { Committed, Rejected, Conflicted, Unknown, Historical }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataChangeResult {
    pub request_id:String, pub phase:DataPhase, pub context:DataContext,
    pub before_head:Option<String>, pub after_head:Option<String>, pub commit:Option<String>,
    pub created:std::collections::BTreeMap<String,DataCreated>, pub changed:Vec<DataObjectVersion>,
    pub conflicts:Vec<DataConflict>, pub reason:Option<String>, pub resolved_by_request:Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataConflict { pub table:String, pub row_count:String, pub schema_conflict:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="status", content="result", rename_all="camelCase")]
pub enum DataRequestLookup { Known(DataChangeResult), NotFound, Historical(DataChangeResult) }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataReadParams { pub context:DataContext, pub at_commit:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataObjectGetParams { pub context:DataContext, pub reference:DataRef, pub at_commit:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataObjectListParams { pub context:DataContext, pub object_type:String, pub after:Option<String>, pub limit:u32, pub at_commit:Option<String>, pub include_archived:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataQueryParams { pub context:DataContext, pub statement:DataSql, pub at_commit:Option<String>, pub cursor:Option<String>, pub limit:u32 }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataRows { pub columns:Vec<String>, pub rows:Vec<Vec<DataCell>>, pub truncated:bool, pub next:Option<String>, pub commit:String, pub stable_order:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataHistoryParams { pub context:DataContext, pub after:Option<String>, pub limit:u32 }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataDiffParams { pub context:DataContext, pub from:String, pub to:String, pub table:Option<String>, pub after:Option<String>, pub limit:u32 }

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataWorkspaceGetParams { pub crew_id:String }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataWorkspaceInfo { pub context:DataContext, pub state:String, pub engine_version:String, pub reason:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataObjectPage { pub context:DataContext, pub commit:String, pub items:Vec<DataObject>, pub next:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataObjectDetail { pub context:DataContext, pub commit:String, pub object:DataObject, pub references:Vec<DataResolvedRef>, pub affected_live_views:Vec<DataRef>, pub affected_truncated:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="state", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataRefState { Visible { reference:DataRef }, Unavailable }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataColumn { pub name:String, pub declared_type:String, pub not_null:bool, pub primary_key_position:u32, pub default_sql:Option<String>, pub attributes:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSchemaObject { pub kind:String, pub name:String, pub create_sql:String, pub columns:Vec<DataColumn>, pub attributes:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSchemaResult { pub context:DataContext, pub commit:String, pub objects:Vec<DataSchemaObject>, pub next:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSchemaParams { pub context:DataContext, pub at_commit:Option<String>, pub after:Option<String>, pub limit:u32 }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataTableParams { pub context:DataContext, pub table:String, pub after_key:Option<Vec<DataCell>>, pub at_commit:Option<String>, pub limit:u32 }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataTablePage { pub context:DataContext, pub commit:String, pub columns:Vec<String>, pub rows:Vec<Vec<DataCell>>, pub next_key:Option<Vec<DataCell>>, pub truncated:bool, pub stable_order:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataCommit { pub hash:String, pub parents:Vec<String>, pub author:String, pub timestamp:String, pub message:String }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataHistory { pub context:DataContext, pub items:Vec<DataCommit>, pub next:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataBranch { pub name:String, pub head:String, pub active:bool }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataBranches { pub context:DataContext, pub items:Vec<DataBranch>, pub next:Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataDiff { pub context:DataContext, pub from:String, pub to:String, pub table:Option<String>, pub changes:DataRows }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataExportParams { pub context:DataContext, pub at_commit:Option<String>, pub after:Option<String>, pub limit:u32 }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataExportPage { pub context:DataContext, pub commit:String, pub objects:Vec<DataObject>, pub schema:Vec<DataSchemaObject>, pub tables:Vec<DataExportTable>, pub next:Option<String>, pub complete:bool }

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataExportTable { pub table:String, pub data:DataRows }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataBranchesParams { pub context:DataContext, pub after:Option<String>, pub limit:u32 }

/// Request-local names never become durable IDs. Existing references are authority-checked.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataInputRef { Existing { reference:DataRef }, Created { alias:String } }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataSourceInput { pub reference:Option<DataInputRef>, pub at_commit:Option<String>, pub attributes:Value }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataPlacementEdit {
    Keep { id:String, layout:Value },
    Set { id:Option<String>, alias:Option<String>, block:DataInputRef, at_commit:Option<String>, layout:Value },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataBindingInputSource {
    Query { statement:DataSql }, Object { reference:DataInputRef, at_commit:Option<String> }, Objects { object_type:String, limit:u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataBindingEdit {
    Keep { id:String }, Set { id:String, source:DataBindingInputSource, attributes:Value },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataViewDefinitionInput {
    Document { placements:Vec<DataPlacementEdit>, layout:Value },
    Web { renderer:String, renderer_version:String, html:String, css:String, javascript:String, bindings:Vec<DataBindingEdit>, requested_capabilities:Vec<String>, attributes:Value },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag="kind", rename_all="camelCase", rename_all_fields="camelCase", deny_unknown_fields)]
pub enum DataCreated { Object { object:DataObjectVersion }, Placement { id:String, parent_view:DataObjectVersion } }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataResolvedRef {
    pub anchor:String, pub target:DataRefState, pub commit:Option<String>, pub revision:Option<String>,
    pub live:bool, pub complete:bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct DataRequestGetResult { pub workspace:DataWorkspaceInfo, pub lookup:DataRequestLookup }
