//! Explicit message provenance, read under the message's current authority.
use crate::managed_objects::{Change, Object, Scope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedMessageSources.v1";
pub const RECORD: &str = "managedObject.recordFromMessage";
pub const GET: &str = "managedObject.source";
pub const MESSAGE_OBJECTS: &str = "managedObject.messageObjects";
pub const REQUEST_GET: &str = "managedObject.messageRequest.get";

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordParams {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub source_message_id: String,
    pub change: Change,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageObjectsParams {
    pub message_id: String,
    pub after: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Source {
    None,
    Restricted,
    Available {
        crew_id: String,
        message_id: String,
        channel_id: String,
        excerpt: String,
    },
    Deleted {
        excerpt: String,
    },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageObjectsPage {
    pub items: Vec<Object>,
    pub next_after: Option<String>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordReceipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub source_message_id: String,
    pub created: bool,
    pub object: Object,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Lookup {
    Known { receipt: RecordReceipt },
    Historical { receipt: Option<RecordReceipt> },
    NotFound,
}
