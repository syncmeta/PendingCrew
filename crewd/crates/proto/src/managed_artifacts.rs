//! Unverified natural-key pointers and their organization object associations.
use crate::{ResourceActor, managed_objects::Scope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "managedArtifacts.v1";
pub const PREVIEW: &str = "artifact.preview";
pub const LIST: &str = "artifact.list";
pub const GET: &str = "artifact.get";
pub const BACKLINKS: &str = "artifact.backlinks";
pub const SUBJECT: &str = "artifact.subject";
pub const SUBMIT: &str = "artifact.submit";
pub const REQUEST_GET: &str = "artifact.request.get";
pub const CHANGED: &str = "artifact.changed";
pub const METHODS: &[&str] = &[PREVIEW, LIST, GET, BACKLINKS, SUBJECT, SUBMIT, REQUEST_GET];

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Namespace {
    Global,
    Repository { id: String },
    Unassigned { crew_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub namespace: Namespace,
    pub key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Commit,
    PullRequest,
    File,
    Test,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub kind: Kind,
    pub value: String,
    pub repository: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub reference: Reference,
    pub created_at: String,
    pub verification: Verification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verification {
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Subject {
    Work { id: String },
    Requirement { id: String },
    Decision { id: String },
    Lesson { id: String },
    Risk { id: String },
    Row { id: String },
    Table { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Relation {
    Evidence,
    AppliesTo,
    Relates,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Origin {
    Manual {
        actor: ResourceActor,
    },
    WorkDone {
        event_seq: String,
    },
    LegacyWorkDone {
        crew_id: String,
        local_seq: String,
        legacy_work_id: String,
    },
    LegacyWorkSnapshot {
        crew_id: String,
        legacy_work_id: String,
        snapshot_seq: String,
    },
    LessonEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edge {
    pub id: String,
    pub reference: Reference,
    pub crew_id: String,
    pub subject: Subject,
    pub relation: Relation,
    pub origin: Origin,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Change {
    Register {
        input: Input,
    },
    Link {
        input: Input,
        subject: Subject,
        relation: Relation,
    },
    LinkExisting {
        reference: Reference,
        subject: Subject,
        relation: Relation,
    },
    Unlink {
        reference: Reference,
        subject: Subject,
        relation: Relation,
        expected_edge_id: String,
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
    Registered {
        created: bool,
        claim_added: bool,
    },
    Linked {
        created: bool,
        added: bool,
        edge: Box<Edge>,
    },
    Unlinked {
        edge_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submit {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub scope: Scope,
    pub crew_id: String,
    pub request_id: String,
    pub recorded_at: String,
    pub change: Change,
    pub artifact: Artifact,
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
pub struct PreviewParams {
    pub crew_id: String,
    pub input: Input,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListParams {
    pub after: Option<Reference>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub reference: Reference,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacklinksParams {
    pub reference: Reference,
    pub after: Option<String>,
    #[serde(default)]
    pub include_legacy_sources: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubjectParams {
    pub subject: Subject,
    pub after: Option<String>,
    #[serde(default)]
    pub include_legacy_sources: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub items: Vec<Artifact>,
    pub next_after: Option<Reference>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePage {
    pub items: Vec<Edge>,
    pub next_after: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub crew_id: String,
}
