//! Stable typed table schemas and source-authorized manual links.
use crate::{
    ChannelKnowledgeReference,
    managed_artifacts::{Input as ArtifactInput, Reference as ArtifactRef},
    managed_objects::Scope,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TABLES_CAPABILITY: &str = "managedTables.v2";
pub const LINKS_CAPABILITY: &str = "managedLinks.v1";

pub const TABLE_GET: &str = "managedTable.v2.get";
pub const TABLE_LIST: &str = "managedTable.v2.list";
pub const ROW_GET: &str = "managedTable.v2.row.get";
pub const ROW_LIST: &str = "managedTable.v2.rows";
pub const TABLE_SUBMIT: &str = "managedTable.v2.submit";
pub const TABLE_REQUEST_GET: &str = "managedTable.v2.request.get";
pub const LINK_LIST: &str = "managedLink.list";
pub const LINK_SUBMIT: &str = "managedLink.submit";
pub const LINK_REQUEST_GET: &str = "managedLink.request.get";
pub const MANAGED_LINK_RESOLVE_TARGET: &str = "managedLink.resolveTarget";
pub const METHODS: &[&str] = &[
    TABLE_GET,
    TABLE_LIST,
    ROW_GET,
    ROW_LIST,
    TABLE_SUBMIT,
    TABLE_REQUEST_GET,
    LINK_LIST,
    LINK_SUBMIT,
    LINK_REQUEST_GET,
    MANAGED_LINK_RESOLVE_TARGET,
];
pub const CHANGED: &str = "links.changed";
pub const TOPIC: &str = "links";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub crew_id: String,
}
/// Distinct from V1: preserve IEEE signed zero and never normalize real bits.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Cell {
    Null,
    Text(String),
    Integer(String),
    Real(f64),
}
impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Text(a), Self::Text(b)) | (Self::Integer(a), Self::Integer(b)) => a == b,
            (Self::Real(a), Self::Real(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveTarget {
    pub target: TargetRef,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedTarget {
    pub target: TargetRef,
    pub summary: String,
    pub owner_crew_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_channel_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TargetRef {
    Work { id: String },
    Requirement { id: String },
    Ask { id: String },
    Table { id: String },
    Row { id: String },
    Decision { id: String },
    Risk { id: String },
    Lesson { id: String },
    Artifact { reference: ArtifactRef },
    Crew { id: String },
    Session { id: String },
    Channel { id: String },
    Message { id: String },
    Human {},
    Document { source: ChannelKnowledgeReference },
    Knowledge { source: ChannelKnowledgeReference },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TargetConstraint {
    Work {},
    Requirement {},
    Ask {},
    Table {},
    Row { table_id: Option<String> },
    Decision {},
    Risk {},
    Lesson {},
    Artifact {},
    Crew {},
    Session {},
    Channel {},
    Message {},
    Human {},
    Document {},
    KnowledgeView {},
    AnyReadable {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FieldKind {
    Text {},
    Integer {},
    Real {},
    Link {
        target: TargetConstraint,
        many: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub name: String,
    pub kind: FieldKind,
    pub nullable: bool,
    pub hidden: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDraft {
    pub name: String,
    pub kind: FieldKind,
    pub nullable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Author {
    Human {},
    Session { session_id: String },
    LegacyUnknown {},
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RowWritePolicy {
    AuthorCaptainHuman,
    OwnerCrewWriters,
    LegacyCrewWriters,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NewRowWritePolicy {
    AuthorCaptainHuman,
    OwnerCrewWriters,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Table {
    pub id: String,
    pub crew_id: String,
    pub schema_revision: String,
    pub title: String,
    pub purpose: Option<String>,
    pub title_field_id: String,
    pub row_write_policy: RowWritePolicy,
    pub author: Author,
    pub fields: Vec<Field>,
    pub next_field_number: String,
    pub frozen: bool,
    pub v1_readable: bool,
    pub v1_writable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisibleLink {
    pub edge_id: String,
    pub target: TargetRef,
    pub summary: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HiddenSet {
    pub keep_token: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScalarValue {
    pub field_id: String,
    pub value: Cell,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkCell {
    pub field_id: String,
    pub visible: Vec<VisibleLink>,
    pub hidden_set: Option<HiddenSet>,
    pub incomplete_required_link: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub crew_id: String,
    pub table_id: String,
    pub schema_revision: String,
    pub revision: String,
    pub author: Author,
    pub scalar_cells: Vec<ScalarValue>,
    pub link_cells: Vec<LinkCell>,
    pub frozen: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LinkWrite {
    Reference { target: TargetRef },
    DeclareArtifact { input: ArtifactInput },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CellPatch {
    Keep {},
    SetScalar {
        value: Cell,
    },
    ReplaceLinks {
        values: Vec<LinkWrite>,
        keep_hidden_set_token: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    pub field_id: String,
    pub change: CellPatch,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SchemaChange {
    AddField { field: FieldDraft },
    RenameField { field_id: String, name: String },
    RenameTable { title: String },
    SetPurpose { purpose: String },
    HideField { field_id: String },
    RestoreField { field_id: String },
    SetTitleField { field_id: String },
    WidenSingleLink { field_id: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    // titleFieldIndex applies only to this just-submitted, not-yet-issued creation draft.
    // Core assigns p IDs and echoes the actual titleFieldId in the immutable result.
    CreateTable {
        title: String,
        purpose: String,
        fields: Vec<FieldDraft>,
        title_field_index: u16,
        row_write_policy: NewRowWritePolicy,
    },
    AlterTable {
        table_id: String,
        expected_schema_revision: String,
        change: SchemaChange,
    },
    AddRow {
        table_id: String,
        expected_schema_revision: String,
        patches: Vec<Patch>,
    },
    PatchRow {
        row_id: String,
        expected_schema_revision: String,
        expected_row_revision: String,
        patches: Vec<Patch>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submit {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub operation: Operation,
}

/// Immutable mutation result. No target payload is echoed here after ACL changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Effect {
    TableCreated {
        table_id: String,
        schema_revision: String,
        field_ids: Vec<String>,
        title_field_id: String,
    },
    SchemaChanged {
        table_id: String,
        original_schema_revision: String,
        result_schema_revision: String,
        changed_field_ids: Vec<String>,
        changed: bool,
    },
    RowAdded {
        table_id: String,
        row_id: String,
        schema_revision: String,
        row_revision: String,
        edge_ids: Vec<String>,
    },
    RowPatched {
        table_id: String,
        row_id: String,
        schema_revision: String,
        original_row_revision: String,
        result_row_revision: String,
        changed_field_ids: Vec<String>,
        added_edge_ids: Vec<String>,
        removed_edge_ids: Vec<String>,
        changed: bool,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    CreateTable,
    AlterTable,
    AddRow,
    PatchRow,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub operation_kind: OperationKind,
    pub intent_digest: String,
    pub event_id: Option<String>,
    pub effect: Effect,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SourceRef {
    Work {
        id: String,
    },
    Requirement {
        id: String,
    },
    Ask {
        id: String,
    },
    Table {
        id: String,
    },
    Row {
        id: String,
    },
    Decision {
        id: String,
    },
    Risk {
        id: String,
    },
    Lesson {
        id: String,
    },
    Artifact {
        reference: ArtifactRef,
        metadata_crew_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualOperation {
    Link {
        source: SourceRef,
        target: ManualTargetWrite,
        expected_manual_link_revision: String,
    },
    Unlink {
        source: SourceRef,
        expected_manual_link_revision: String,
        expected_edge_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualSubmit {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub operation: ManualOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualTargetWrite {
    Reference { target: TargetRef },
    DeclareArtifact { input: ArtifactInput },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableGet {
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableList {
    pub after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowList {
    pub table_id: String,
    pub after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TablePage {
    pub items: Vec<Table>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowPage {
    pub table: Table,
    pub items: Vec<Row>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestGet {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum RequestLookup {
    Known { receipt: Receipt },
    Historical { receipt: Option<Receipt> },
    NotFound {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "origin",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EdgeOrigin {
    ManualRelates {},
    Column {
        field_id: String,
    },
    Adapter {
        adapter: String,
        readonly_edge_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "direction",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LinkQuery {
    Outgoing {
        source: SourceRef,
        include_adapters: bool,
    },
    Incoming {
        target: TargetRef,
        include_adapters: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListLinks {
    pub query: LinkQuery,
    pub after: Option<String>,
    pub limit: u16,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutgoingHiddenSet {
    pub marker: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeView {
    pub id: String,
    pub owner_crew_id: String,
    pub source: SourceRef,
    pub target: TargetRef,
    pub summary: String,
    pub origin: EdgeOrigin,
    pub editable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePage {
    pub source: Option<SourceRef>,
    pub manual_link_revision: Option<String>,
    pub outgoing_hidden_set: Option<OutgoingHiddenSet>,
    pub items: Vec<EdgeView>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManualEffect {
    Linked { edge_id: String, added: bool },
    Unlinked { edge_id: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualReceipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub intent_digest: String,
    pub source: SourceRef,
    pub original_manual_link_revision: String,
    pub result_manual_link_revision: String,
    pub event_id: Option<String>,
    pub effect: ManualEffect,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum ManualLookup {
    Known { receipt: ManualReceipt },
    Historical { receipt: Option<ManualReceipt> },
    NotFound {},
}
