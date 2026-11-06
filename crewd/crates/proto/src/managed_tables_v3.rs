//! Protocol-only candidate v0.4, not registered or compiled.
//! Exact source baseline: 23e65d009b9f00ec00e8ff2200b9bb2f44d649cc.
use crate::{ResourceActor, managed_objects::Scope, managed_tables_v2 as v2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedTables.v3";
pub const TABLE_GET: &str = "managedTable.v3.get";
pub const TABLE_LIST: &str = "managedTable.v3.list";
pub const ROW_GET: &str = "managedTable.v3.row.get";
pub const ROW_LIST: &str = "managedTable.v3.rows";
pub const ROW_LINKS: &str = "managedTable.v3.row.links";
pub const SUBMIT: &str = "managedTable.v3.submit";
pub const REQUEST_GET: &str = "managedTable.v3.request.get";
pub const DEFINE_CHECK: &str = "managedTable.v3.defineCheck";
pub const HISTORY: &str = "managedTable.v3.history";
pub const HISTORY_VALUE: &str = "managedTable.v3.history.value";
pub const METHODS: &[&str] = &[
    TABLE_GET,
    TABLE_LIST,
    ROW_GET,
    ROW_LIST,
    ROW_LINKS,
    SUBMIT,
    REQUEST_GET,
    DEFINE_CHECK,
    HISTORY,
    HISTORY_VALUE,
];
// Reuse links.changed / topic links, with the existing Human-only event gate.
pub const MAX_FIELDS: usize = 64;
pub const TEXT_SCALARS: u32 = 500;
pub const LONG_TEXT_SCALARS: u32 = 1500;
pub const TEXT_UTF8_BYTES: u32 = 65536;
pub const ROW_SCALARS_ENCODED_BYTES: usize = 65536;
pub const DEFINITION_ENCODED_BYTES: usize = 65536;
pub const TABLE_PAGE_ENCODED_BYTES: usize = 262144;
pub const ROW_PAGE_ENCODED_BYTES: usize = 262144;
pub const ROW_LINK_CELL_ENCODED_BYTES: usize = 1048576;
pub const DECIMAL_PRECISION: u8 = 38;
pub const DECIMAL_MAX_SCALE: u8 = 18;
pub const OPTION_ACTIVE_MAX: usize = 12;
pub const OPTION_HISTORY_PER_FIELD_MAX: usize = 256;
pub const TITLE_SCALARS: usize = 256; // Preserve the existing V2 definition limit.
pub const PURPOSE_SCALARS: usize = 1500;
pub const FIELD_NAME_SCALARS: usize = 120;
pub const REASON_SCALARS: usize = 1500;
pub const CHECK_SOURCE_BUDGET: usize = 64;
pub const NEAR_CANDIDATES_MAX: usize = 8;
pub const HISTORY_RECORD_BUDGET: usize = 64;
pub const HISTORY_ENCODED_BYTES: usize = 32768;
pub const HISTORY_CHUNK_UTF8_BYTES: usize = 4096;
pub const INTENT_DOMAIN: &[u8] = b"managed-v3-intent.v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextBounds {
    pub max_utf8_bytes: u32,
    pub max_unicode_scalars: Option<u32>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    #[default]
    Neutral,
    Good,
    Attention,
    Bad,
    Info,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionDraft {
    pub label: String,
    #[serde(default)]
    pub tone: Tone,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionDefinition {
    pub id: String,
    pub label: String,
    pub tone: Tone,
    pub active: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionRef {
    pub table_id: String,
    pub field_id: String,
    pub option_id: String,
}

/// New callers cannot select legacy text bounds or issue their own option IDs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum KindDraft {
    Text {},
    LongText {},
    Integer {},
    Real {},
    Decimal {
        unit: Option<String>,
        display_scale: Option<u8>,
    },
    SingleSelect {
        options: Vec<OptionDraft>,
    },
    Date {},
    Boolean {},
    Url {},
    Person {
        allow_human: bool,
    },
    Link {
        target: v2::TargetConstraint,
        many: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FieldKind {
    Text {
        bounds: TextBounds,
    },
    LongText {
        bounds: TextBounds,
    },
    Integer {},
    Real {},
    Decimal {
        precision: u8,
        max_scale: u8,
        unit: Option<String>,
        display_scale: Option<u8>,
    },
    SingleSelect {
        options: Vec<OptionDefinition>,
    },
    Date {},
    Boolean {},
    Url {},
    Person {
        allow_human: bool,
    },
    Link {
        target: v2::TargetConstraint,
        many: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDraft {
    pub name: String,
    pub kind: KindDraft,
    pub nullable: bool,
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
/// Real equality is by IEEE bits, exactly as V2; no +0/-0 normalization.
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
    Decimal(String),
    SingleSelect(OptionRef),
    Date(String),
    Boolean(bool),
    Url(String),
}
impl PartialEq for Cell {
    fn eq(&self, b: &Self) -> bool {
        match (self, b) {
            (Self::Null, Self::Null) => true,
            (Self::Text(a), Self::Text(b))
            | (Self::Integer(a), Self::Integer(b))
            | (Self::Decimal(a), Self::Decimal(b))
            | (Self::Date(a), Self::Date(b))
            | (Self::Url(a), Self::Url(b)) => a == b,
            (Self::Real(a), Self::Real(b)) => a.to_bits() == b.to_bits(),
            (Self::Boolean(a), Self::Boolean(b)) => a == b,
            (Self::SingleSelect(a), Self::SingleSelect(b)) => a == b,
            _ => false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScalarValue {
    pub field_id: String,
    pub value: Cell,
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
        values: Vec<v2::LinkWrite>,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lifecycle {
    pub archived_at: Option<String>,
    pub archived_by: Option<ResourceActor>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum FrozenReason {
    CrewArchived {},
    TableArchived {},
    RowArchived {},
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
    pub row_write_policy: v2::RowWritePolicy,
    pub author: v2::Author,
    pub fields: Vec<Field>,
    pub next_field_number: String,
    pub next_option_number: String,
    pub lifecycle: Lifecycle,
    pub frozen_reasons: Vec<FrozenReason>,
    pub v1_readable: bool,
    pub v1_writable: bool,
    pub v2_readable: bool,
    pub v2_writable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub crew_id: String,
    pub table_id: String,
    pub schema_revision: String,
    pub revision: String,
    pub author: v2::Author,
    pub scalar_cells: Vec<ScalarValue>,
    /// Field IDs only: unloaded links are never represented as empty values.
    pub link_field_ids: Vec<String>,
    pub lifecycle: Lifecycle,
    pub frozen_reasons: Vec<FrozenReason>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SchemaChange {
    AddField {
        field: FieldDraft,
    },
    RenameField {
        field_id: String,
        name: String,
    },
    RenameTable {
        title: String,
        definition_check_token: String,
        confirm_new: bool,
    },
    SetPurpose {
        purpose: String,
        definition_check_token: String,
        confirm_new: bool,
    },
    HideField {
        field_id: String,
    },
    RestoreField {
        field_id: String,
    },
    SetTitleField {
        field_id: String,
    },
    WidenSingleLink {
        field_id: String,
    },
    WidenText {
        field_id: String,
    },
    WidenNumberToText {
        field_id: String,
    },
    AddOption {
        field_id: String,
        option: OptionDraft,
    },
    RenameOption {
        field_id: String,
        option: OptionRef,
        label: String,
    },
    DeactivateOption {
        field_id: String,
        option: OptionRef,
    },
    RestoreOption {
        field_id: String,
        option: OptionRef,
    },
    SetOptionTone {
        field_id: String,
        option: OptionRef,
        tone: Tone,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    CreateTable {
        title: String,
        purpose: String,
        fields: Vec<FieldDraft>,
        title_field_index: u16,
        row_write_policy: v2::NewRowWritePolicy,
        definition_check_token: String,
        confirm_new: bool,
    },
    AlterTable {
        table_id: String,
        expected_schema_revision: String,
        reason: String,
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
        reason: String,
        patches: Vec<Patch>,
    },
    ArchiveTable {
        table_id: String,
        expected_schema_revision: String,
        reason: String,
    },
    RestoreTable {
        table_id: String,
        expected_schema_revision: String,
        reason: String,
    },
    ArchiveRow {
        row_id: String,
        expected_schema_revision: String,
        expected_row_revision: String,
        reason: String,
    },
    RestoreRow {
        row_id: String,
        expected_schema_revision: String,
        expected_row_revision: String,
        reason: String,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    CreateTable,
    AlterTable,
    AddRow,
    PatchRow,
    ArchiveTable,
    RestoreTable,
    ArchiveRow,
    RestoreRow,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssuedOption {
    pub field_id: String,
    pub option_id: String,
}
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
        issued_options: Vec<IssuedOption>,
    },
    SchemaChanged {
        table_id: String,
        original_schema_revision: String,
        result_schema_revision: String,
        changed_field_ids: Vec<String>,
        issued_options: Vec<IssuedOption>,
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
    TableLifecycle {
        table_id: String,
        original_schema_revision: String,
        result_schema_revision: String,
        archived: bool,
        changed: bool,
    },
    RowLifecycle {
        table_id: String,
        row_id: String,
        schema_revision: String,
        original_row_revision: String,
        result_row_revision: String,
        archived: bool,
        changed: bool,
    },
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
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Lookup {
    Known { receipt: Receipt },
    Historical { receipt: Option<Receipt> },
    NotFound {},
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Get {
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableList {
    pub after: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowList {
    pub table_id: String,
    pub after: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowLinks {
    pub row_id: String,
    pub field_id: String,
    pub expected_schema_revision: String,
    pub expected_row_revision: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowLinkCell {
    pub scope: Scope,
    pub row_id: String,
    pub schema_revision: String,
    pub row_revision: String,
    pub cell: v2::LinkCell,
}
pub type RequestGet = v2::RequestGet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefineCheck {
    pub scope: Scope,
    pub crew_id: String,
    pub excluding_table_id: Option<String>,
    pub title: String,
    pub purpose: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CheckState {
    Checking,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NearCandidate {
    pub table_id: String,
    pub schema_revision: String,
    pub title: String,
    pub purpose: Option<String>,
    pub archived: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionCheck {
    pub scope: Scope,
    pub crew_id: String,
    pub state: CheckState,
    pub exact_name_taken: bool,
    pub near_candidates: Vec<NearCandidate>,
    pub more_similar: bool,
    pub definition_check_token: Option<String>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HistorySource {
    Table { id: String },
    Row { id: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HistoryKey {
    Event {
        event_id: String,
    },
    LegacyReceipt {
        codec: LegacyCodec,
        request_id: String,
        entry_locator: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LegacyCodec {
    ManagedObjectsV1,
    ManagedTablesV2,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct History {
    pub source: HistorySource,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HistoryAction {
    CreateTable,
    AlterTable,
    AddRow,
    PatchRow,
    ArchiveTable,
    RestoreTable,
    ArchiveRow,
    RestoreRow,
    LegacyRowWrite,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistorySummary {
    pub key: HistoryKey,
    pub action: HistoryAction,
    pub actor: ResourceActor,
    pub recorded_at: String,
    pub reason_preview: Option<String>,
    pub reason_available: bool,
    pub changed_field_ids: Vec<String>,
    pub original_revision: Option<String>,
    pub result_revision: Option<String>,
    pub legacy_details_unavailable: bool,
    pub restricted_details: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPage {
    pub scope: Scope,
    pub source: HistorySource,
    pub items: Vec<HistorySummary>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HistorySide {
    Before,
    After,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HistoryPath {
    Scalar { field_id: String, side: HistorySide },
    Links { field_id: String, side: HistorySide },
    Reason {},
    Schema { side: HistorySide },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryValue {
    pub source: HistorySource,
    pub key: HistoryKey,
    pub path: HistoryPath,
    pub cursor: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HistoryValueType {
    Null,
    Text,
    Integer,
    Real,
    Decimal,
    SingleSelect,
    Date,
    Boolean,
    Url,
    Reason,
    Schema,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ValueChunk {
    Scalar {
        scope: Scope,
        source: HistorySource,
        key: HistoryKey,
        path: HistoryPath,
        value_type: HistoryValueType,
        utf8: String,
        next_cursor: Option<String>,
    },
    Links {
        scope: Scope,
        source: HistorySource,
        key: HistoryKey,
        path: HistoryPath,
        visible: Vec<v2::VisibleLink>,
        restricted_details: bool,
        next_cursor: Option<String>,
    },
    Restricted {},
    Unavailable {},
}

/// Only the explicit version-3 branch of the EXISTING managed_table_v2_get tool.
/// Dispatch contractVersion/action before decoding; malformed presence never falls back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolGetV3 {
    Table {
        id: String,
    },
    DefineCheck {
        scope: Scope,
        crew_id: String,
        excluding_table_id: Option<String>,
        title: String,
        purpose: Option<String>,
        cursor: Option<String>,
    },
    History {
        source: HistorySource,
        cursor: Option<String>,
    },
    HistoryValue {
        source: HistorySource,
        key: HistoryKey,
        path: HistoryPath,
        cursor: Option<String>,
    },
}

/// Explicit version-3 branch of the existing managed_row_v2_get tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolRowGetV3 {
    Row {
        id: String,
    },
    Links {
        row_id: String,
        field_id: String,
        expected_schema_revision: String,
        expected_row_revision: String,
    },
}
