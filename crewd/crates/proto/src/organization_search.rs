//! Unified organization search. Index versions are disposable projections;
//! every returned result is checked against current authority and permissions.
use crate::managed_objects::Scope;
use crate::managed_tables_v2::{EdgeView, ResolvedTarget, TargetRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "organizationSearch.v1";
pub const SEARCH: &str = "organization.search";
pub const EXPAND: &str = "organization.search.expand";
pub const METHODS: &[&str] = &[SEARCH, EXPAND];
pub const DEFAULT_LIMIT: u16 = 20;
pub const MAX_LIMIT: u16 = 200;
pub const CURSOR_LIFETIME_MS: i64 = 15 * 60 * 1000;
pub const MAX_QUERY_CHARACTERS: usize = 256;
pub const MAX_QUERY_BYTES: usize = 1024;
pub const MAX_EXCERPT_CHARACTERS: usize = 280;

fn default_limit() -> u16 {
    DEFAULT_LIMIT
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Domain {
    Messages,
    Objects,
    #[default]
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Decision,
    Risk,
    Lesson,
    Work,
    Requirement,
    Ask,
    Table,
    Row,
    Artifact,
    Message,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Search {
    /// Optional data-version fence, never a caller identity or permission grant.
    pub scope: Option<Scope>,
    pub query: String,
    #[serde(default)]
    pub r#in: Domain,
    pub kinds: Option<Vec<Kind>>,
    /// Restricts row results to this real table; does not name an arbitrary type.
    pub table_id: Option<String>,
    pub crew_id: Option<String>,
    /// Messages only; incompatible object-only filters must be refused.
    pub channel_id: Option<String>,
    /// Inclusive/exclusive RFC3339 bounds on the source's actual change time.
    pub after_time: Option<String>,
    pub before_time: Option<String>,
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
    #[serde(default)]
    pub include_system_defaults: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum IndexState {
    Building,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Current,
    Historical,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MatchOn {
    Title,
    Text,
    HistoricalName,
    Reference,
}

/// Ordering and source population are fixed at the start of a search. Returned
/// labels, visible fields, existence and permissions are always current. No
/// globally ticking sequence, hidden count or per-reader activity is exposed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub sorted_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hit {
    pub target: TargetRef,
    pub kind: Kind,
    pub owner_crew_id: Option<String>,
    pub table_id: Option<String>,
    /// Current table label for row grouping, without extra per-hit RPC calls.
    pub table_title: Option<String>,
    pub title: String,
    pub excerpt: Option<String>,
    pub matched_on: MatchOn,
    pub status: Status,
    /// Current, real source change time, absent when it cannot be established.
    pub changed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub scope: Scope,
    pub index_state: IndexState,
    /// Building returns no snapshot, items or continuation. It is not an empty
    /// completed search. A fresh read can observe the bounded rebuild progress.
    pub snapshot: Option<Snapshot>,
    pub items: Vec<Hit>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expand {
    pub scope: Option<Scope>,
    pub target: TargetRef,
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u16,
    #[serde(default)]
    pub include_system_defaults: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// One real edge, not an invented inverse business action. Different relations
/// may lead to the same neighbor. Self-edges are emitted only once, outgoing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expansion {
    pub direction: Direction,
    pub edge: EdgeView,
    pub neighbor: ResolvedTarget,
    pub status: Status,
    pub changed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpansionPage {
    pub scope: Scope,
    pub index_state: IndexState,
    pub snapshot: Option<Snapshot>,
    pub items: Vec<Expansion>,
    pub next_cursor: Option<String>,
}
