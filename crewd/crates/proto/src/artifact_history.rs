//! Explicit, bounded indexing of retained artifact evidence. No source facts are rewritten.
use crate::managed_objects::Scope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedArtifacts.history.v1";
pub const STATUS: &str = "artifact.history.status";
pub const BEGIN: &str = "artifact.history.begin";
pub const STEP: &str = "artifact.history.step";
pub const REQUEST_GET: &str = "artifact.history.request.get";
pub const CHANGED: &str = "artifact.historyChanged";
pub const METHODS: &[&str] = &[STATUS, BEGIN, STEP, REQUEST_GET];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    LegacyEvents,
    LegacySnapshots,
    Events,
    Lessons,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cursor {
    pub phase: Phase,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cut {
    pub legacy_event_seq: String,
    pub legacy_item: String,
    pub legacy_snapshot_seq: String,
    pub legacy_snapshot_digest: Option<String>,
    pub legacy_retained_events: String,
    pub event_seq: String,
    pub lesson_number: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Counts {
    pub scanned: String,
    pub done_events: String,
    pub legacy_snapshots: String,
    pub lessons: String,
    pub missing_targets: String,
    pub artifacts_added: String,
    pub edges_added: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Job {
    pub id: String,
    pub scope: Scope,
    pub crew_id: String,
    pub cut: Cut,
    pub cursor: Cursor,
    pub revision: String,
    pub counts: Counts,
    pub created_at: String,
    pub updated_at: String,
    pub last_request_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusParams {
    pub crew_id: String,
    pub job_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub scope: Scope,
    pub crew_id: String,
    pub job: Option<Job>,
    pub can_continue: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginParams {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepParams {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub job_id: String,
    pub expected_revision: String,
    pub expected_cursor: Cursor,
    #[schemars(range(min = 1, max = 64))]
    pub limit: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Begin,
    Step {
        job_id: String,
        expected_revision: String,
        expected_cursor: Cursor,
        limit: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Effect {
    Begun {
        created: bool,
    },
    Advanced {
        before_revision: String,
        before_cursor: Cursor,
        delta: Box<Counts>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub operation: Operation,
    pub job: Job,
    pub effect: Effect,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Lookup {
    Known { receipt: Receipt },
    Historical { receipt: Option<Receipt> },
    NotFound,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub crew_id: String,
    pub job_id: String,
}
