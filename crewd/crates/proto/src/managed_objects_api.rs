//! Transport for organization-readable typed objects.
use crate::managed_objects::Scope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedObjects.v1";
pub const DECISIONS_CAPABILITY: &str = "managedDecisions.v1";
pub const RISK_LESSONS_CAPABILITY: &str = "managedRiskLessons.v1";
pub const EVENTS_CAPABILITY: &str = "managedObjects.events.v1";
pub const SCOPE: &str = "managedObject.scope";
pub const GET: &str = "managedObject.get";
pub const TABLES: &str = "managedObject.tables";
pub const ROWS: &str = "managedObject.rows";
pub const DECISIONS: &str = "managedObject.decisions";
pub const RISKS: &str = "managedObject.risks";
pub const LESSONS: &str = "managedObject.lessons";
pub const SUBMIT: &str = "managedObject.submit";
pub const REQUEST_GET: &str = "managedObject.request.get";
pub const METHODS: &[&str] = &[
    SCOPE,
    GET,
    TABLES,
    ROWS,
    DECISIONS,
    RISKS,
    LESSONS,
    SUBMIT,
    REQUEST_GET,
    crate::decision_asks::GET,
    crate::decision_asks::DECISION_ORIGIN,
];

/// An invalidation, not a write receipt or an object snapshot.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OntologyChanged {
    pub crew_id: String,
    #[schemars(length(min = 1, max = 2))]
    pub object_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub id: String,
    #[serde(default)]
    pub include_system_defaults: bool,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TablesParams {
    pub after: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionsParams {
    pub after: Option<String>,
    #[serde(default)]
    pub include_system_defaults: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowsParams {
    pub table_id: String,
    pub after: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestParams {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
}
