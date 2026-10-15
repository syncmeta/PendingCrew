use crewd_proto::judgments::{JudgmentRequestLookup, JudgmentSettingsResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "JudgmentRequestLookup")]
// Preserve the original unboxed wire02 fixture solely for equivalence proof.
#[allow(clippy::large_enum_variant)]
enum OriginalLookup {
    Known { result: JudgmentSettingsResult },
    NotFound,
    Historical,
}
#[test]
fn boxed_known_result_preserves_wire02_json_and_schema() {
    let values = [
        serde_json::json!({"status":"not_found"}),
        serde_json::json!({"status":"historical"}),
        serde_json::json!({"status":"known","result":{"requestId":"00000000-0000-0000-0000-000000000001","outcome":"unknown","historical":false,"view":{"settings":{"revision":null,"config":null},"credentialState":"update_unknown","pendingRequestId":"00000000-0000-0000-0000-000000000001"}}}),
    ];
    for value in values {
        let original: OriginalLookup = serde_json::from_value(value.clone()).unwrap();
        let actual: JudgmentRequestLookup = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_value(original).unwrap(),
            serde_json::to_value(actual).unwrap()
        );
    }
    assert_eq!(
        serde_json::to_value(schemars::schema_for!(OriginalLookup)).unwrap(),
        serde_json::to_value(schemars::schema_for!(JudgmentRequestLookup)).unwrap()
    );
}
