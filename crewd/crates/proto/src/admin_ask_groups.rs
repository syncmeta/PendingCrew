//! Advisory organization of original pending questions. No answer or merge command.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "adminAskGroups.v1";
pub const LIST: &str = "adminAskGroups.list";
pub const TOPIC: &str = "adminAskGroups";
pub const CHANGED: &str = "adminAskGroups.changed";
pub const INVALIDATED: &str = "adminAskGroups.invalidated";
pub const METHODS: &[&str] = &[LIST];

pub(crate) fn schema_entries(
    g: &mut schemars::SchemaGenerator,
) -> Vec<(&'static str, serde_json::Value)> {
    vec![(LIST, crate::m(crate::sub::<List>(g), crate::sub::<Page>(g)))]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct List {
    pub crew_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Disabled,
    Unconfigured,
    Ready,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Prepared,
    Dispatched,
    Suggested,
    Different,
    Unavailable,
    Unknown,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub question_truncated: bool,
    pub resolution_revision: String,
    pub source_channel_id: String,
    pub source_message_id: String,
    pub asked_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub default_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_due_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pair {
    pub id: String,
    pub first: Question,
    pub second: Question,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    pub excerpt_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub scope: crate::managed_objects::Scope,
    pub crew_id: String,
    pub availability: Availability,
    pub observed_at_ms: i64,
    pub items: Vec<Pair>,
    /// Advances over retained observations, including currently stale rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
    /// Incomplete discovery, missing current sources, or an unresolved judgment.
    pub partial_coverage: bool,
    /// Old restored work is not automatically enrolled into a fresh model round.
    pub historical_baseline: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub crew_id: String,
}

/// Global advisory qualification changed, without identifying a private crew/H.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Invalidated {}

impl<'de> Deserialize<'de> for Invalidated {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Empty derived structs also accept sequences. This hint's wire
        // contract is exactly an empty object, including Value deserialization.
        let fields =
            std::collections::BTreeMap::<String, serde::de::IgnoredAny>::deserialize(deserializer)?;
        if !fields.is_empty() {
            return Err(serde::de::Error::custom(
                "expected an empty invalidation object",
            ));
        }
        Ok(Self {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_opt_in_keeps_old_config_encoding_and_decoding_exact() {
        let original = r#"{"enabled":true,"provider":"typesafe","baseUrl":"https://example.invalid","model":"synthetic","choiceMin":0.5,"wakeYes":0.5,"credentialSource":"file"}"#;
        let old: crate::judgments::JudgmentConfig = serde_json::from_str(original).unwrap();
        assert_eq!(old.similar_asks, None);
        assert_eq!(serde_json::to_string(&old).unwrap(), original);
        let mut explicit = old;
        explicit.similar_asks = Some(true);
        let value = serde_json::to_value(explicit).unwrap();
        assert_eq!(value["similarAsks"], true);
        let mut unchanged = value.as_object().unwrap().clone();
        unchanged.remove("similarAsks");
        assert_eq!(
            serde_json::Value::Object(unchanged),
            serde_json::from_str::<serde_json::Value>(original).unwrap()
        );
    }
    #[test]
    fn group_reads_are_closed_and_have_no_business_operation_or_forged_actor() {
        assert!(
            serde_json::from_str::<List>(r#"{"crewId":"crew-a","operation":"answer"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<List>(r#"{"crewId":"crew-a","principal":"human"}"#).is_err()
        );
        assert!(serde_json::from_str::<List>(r#"{"crewId":"crew-a","system":true}"#).is_err());
        assert!(serde_json::from_str::<List>(r#"{"crewId":"crew-a"}"#).is_ok());
    }
    #[test]
    fn shared_registry_exposes_exact_read_schema_topic_and_crew_only_hint() {
        assert_eq!(METHODS, &[LIST]);
        assert_eq!(
            crate::methods::ALL
                .iter()
                .filter(|method| **method == LIST)
                .count(),
            1
        );
        assert!(crate::events::TOPICS.contains(&TOPIC));
        let api = crate::api_schema();
        assert!(api["methods"].get(LIST).is_some());
        assert!(api["events"].get(CHANGED).is_some());
        assert_eq!(
            serde_json::to_value(Changed {
                crew_id: "actual-owner".into()
            })
            .unwrap(),
            serde_json::json!({"crewId":"actual-owner"})
        );
    }
    #[test]
    fn invalidated_is_a_strict_empty_object_not_a_private_identity_or_nullable_hint() {
        assert_eq!(
            serde_json::to_value(Invalidated {}).unwrap(),
            serde_json::json!({})
        );
        assert!(serde_json::from_str::<Invalidated>("{}").is_ok());
        for malformed in [
            "null",
            "[]",
            r#"{"crewId":"private"}"#,
            r#"{"ask":"H1"}"#,
            r#"{"reason":"disabled"}"#,
        ] {
            assert!(
                serde_json::from_str::<Invalidated>(malformed).is_err(),
                "{malformed}"
            );
        }
        let schema = crate::api_schema();
        assert!(schema["events"].get(INVALIDATED).is_some());
    }
}
