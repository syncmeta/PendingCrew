//! Transport for organization-readable typed objects.
use crate::managed_objects::Scope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedObjects.v1";
pub const SCOPE: &str = "managedObject.scope";
pub const GET: &str = "managedObject.get";
pub const TABLES: &str = "managedObject.tables";
pub const ROWS: &str = "managedObject.rows";
pub const SUBMIT: &str = "managedObject.submit";
pub const REQUEST_GET: &str = "managedObject.request.get";
pub const METHODS: &[&str] = &[SCOPE, GET, TABLES, ROWS, SUBMIT, REQUEST_GET];

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TablesParams {
    pub after: Option<String>,
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
