//! One independently frozen byte contract is shared with the native client.
//! Exercise the actual typed request normalization and production encoder.
use crewd_proto::managed_tables_v2 as wire;
use crewd_store::managed_links;
use serde_json::Value;

#[test]
fn native_and_core_share_exact_typed_intent_bytes_and_digests() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/managed_v2_intent_v1.json")).unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let expected = &case["normalized"];
        let mut packet = expected.clone();
        let normalized = match packet.as_object_mut().unwrap().remove("method") {
            Some(Value::String(method)) if method == wire::TABLE_SUBMIT => {
                let request: wire::Submit = serde_json::from_value(packet).unwrap();
                let digest = managed_links::intent(
                    &method,
                    &request.scope,
                    &request.crew_id,
                    &request.request_id,
                    &request.operation,
                )
                .unwrap();
                assert_eq!(digest, case["sha256"], "{name}: typed table digest");
                let mut value = serde_json::to_value(request).unwrap();
                value["method"] = method.into();
                value
            }
            Some(Value::String(method)) if method == wire::LINK_SUBMIT => {
                let request: wire::ManualSubmit = serde_json::from_value(packet).unwrap();
                let digest = managed_links::intent(
                    &method,
                    &request.scope,
                    &request.crew_id,
                    &request.request_id,
                    &request.operation,
                )
                .unwrap();
                assert_eq!(digest, case["sha256"], "{name}: typed manual digest");
                let mut value = serde_json::to_value(request).unwrap();
                value["method"] = method.into();
                value
            }
            None => expected.clone(),
            _ => panic!("unexpected frozen method in {name}"),
        };
        let mut bytes = b"managed-v2-intent.v1\0".to_vec();
        managed_links::encode(&normalized, &mut bytes).unwrap();
        let hex = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hex, case["canonicalHex"], "{name}: exact canonical bytes");
        assert_eq!(
            managed_links::hash(&bytes),
            case["sha256"],
            "{name}: byte digest"
        );
    }
}
