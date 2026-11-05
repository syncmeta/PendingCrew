//! Typed organization objects. This contract never accepts a SQL expression or
//! uses a business key, table name, or array position as an object identity.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ResourceActor;
pub use crate::managed_risks_lessons::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    Text,
    Integer,
    Real,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub field_type: FieldType,
    pub nullable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Table {
    pub id: String,
    pub crew_id: String,
    pub title: String,
    pub fields: Vec<Field>,
    pub frozen: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub crew_id: String,
    pub table_id: String,
    pub revision: String,
    pub values: Vec<Cell>,
    pub frozen: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DecisionStatus {
    Active,
    Superseded,
    Withdrawn,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DecisionReversibility {
    Reversible,
    HardToReverse,
    Irreversible,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DecisionAbout {
    Crew { id: String },
    Work { id: String },
    Requirement { id: String },
    Row { id: String },
    Risk { id: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionDraft {
    pub question: String,
    pub choice: String,
    pub rationale: String,
    pub alternatives: Option<String>,
    pub reversibility: Option<DecisionReversibility>,
    pub about: Vec<DecisionAbout>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionWithdrawal {
    pub author: ResourceActor,
    pub reason: String,
    pub at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Decision {
    pub id: String,
    pub crew_id: String,
    pub revision: String,
    pub draft: DecisionDraft,
    pub author: ResourceActor,
    pub created_at: String,
    pub status: DecisionStatus,
    pub frozen: bool,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    pub withdrawal: Option<DecisionWithdrawal>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Object {
    Table { table: Table },
    Row { row: Row },
    Decision { decision: Decision },
    Risk { risk: Risk },
    Lesson { lesson: Lesson },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Change {
    CreateTable {
        title: String,
        fields: Vec<Field>,
    },
    AddRow {
        table_id: String,
        values: Vec<Cell>,
    },
    EditRow {
        row_id: String,
        expected_revision: String,
        values: Vec<Cell>,
    },
    RecordDecision {
        draft: DecisionDraft,
    },
    ReplaceDecision {
        decision_id: String,
        expected_revision: String,
        draft: DecisionDraft,
    },
    WithdrawDecision {
        decision_id: String,
        expected_revision: String,
        reason: String,
    },
    RecordRisk {
        draft: RiskDraft,
        owner: Option<RiskOwner>,
    },
    UpdateRisk {
        risk_id: String,
        expected_revision: String,
        draft: RiskDraft,
    },
    AssignRisk {
        risk_id: String,
        expected_revision: String,
        owner: RiskOwner,
    },
    ReviewRisk {
        risk_id: String,
        expected_revision: String,
    },
    SetRiskStatus {
        risk_id: String,
        expected_revision: String,
        status: RiskStatus,
        reason: String,
    },
    RecordRiskOccurrence {
        risk_id: String,
        expected_revision: String,
        lesson_draft: LessonDraft,
    },
    RecordLesson {
        draft: LessonDraft,
    },
    ConfirmLesson {
        lesson_id: String,
        expected_revision: String,
    },
    StaleLesson {
        lesson_id: String,
        expected_revision: String,
        reason: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submit {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub change: Change,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub db_id: String,
    pub restore_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Lookup {
    Known { receipt: Receipt },
    Historical { receipt: Option<Receipt> },
    NotFound,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub object: Object,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<RiskOccurrence>,
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
pub struct DecisionPage {
    pub items: Vec<Decision>,
    pub next_after: Option<String>,
}
