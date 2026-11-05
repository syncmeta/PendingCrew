//! An explicitly organization-readable decision request on the existing H ledger.
use crate::managed_objects::{DecisionAbout, DecisionReversibility, Scope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "structuredAskDecisions.v1";
pub const GET: &str = "ask.decisionGet";
pub const DECISION_ORIGIN: &str = "managedObject.decisionOrigin";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionDraft {
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultChoice {
    pub option_id: String,
    pub after_minutes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub question: String,
    pub context: String,
    /// Stable identifiers A through H follow the immutable option order.
    #[schemars(length(min = 2, max = 8))]
    pub options: Vec<OptionDraft>,
    #[schemars(length(min = 1, max = 8))]
    pub about: Vec<DecisionAbout>,
    pub default: Option<DefaultChoice>,
    pub reversibility: Option<DecisionReversibility>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Human,
    SystemDefault,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capture {
    pub decision_id: String,
    pub event_seq: String,
    pub option_id: String,
    pub source: Source,
    pub human_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    pub crew_id: String,
    pub ask: String,
    pub draft: Draft,
    pub capture: Option<Capture>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open {
    pub scope: Scope,
    pub request_id: String,
    pub draft: Draft,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenReceipt {
    pub scope: Scope,
    pub request_id: String,
    pub opened: View,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum OpenLookup {
    Known { receipt: OpenReceipt },
    Historical { receipt: Option<OpenReceipt> },
    NotFound,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookupParams {
    pub scope: Scope,
    pub request_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetParams {
    pub crew_id: String,
    pub ask: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginParams {
    pub id: String,
}
