//! Input authority must survive nested DTO parsing without silently dropping
//! spoofed fields before the original intent is digested.
use crewd_proto::managed_tables_v2::*;
use serde_json::json;

#[test]
fn nested_unit_variants_refuse_extra_identity_or_write_fields() {
    assert!(serde_json::from_value::<TargetRef>(json!({"kind":"human"})).is_ok());
    assert!(serde_json::from_value::<TargetRef>(json!({"kind":"human","id":"forged"})).is_err());
    assert!(
        serde_json::from_value::<TargetConstraint>(json!({"kind":"human","crewId":"forged"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<FieldKind>(json!({"kind":"text","target":{"kind":"human"}}))
            .is_err()
    );
    assert!(serde_json::from_value::<CellPatch>(json!({"kind":"keep","values":[]})).is_err());
    assert!(
        serde_json::from_value::<TargetRef>(json!({"kind":"folder","id":"unimplemented"})).is_err()
    );
}

#[test]
fn typed_real_cells_preserve_signed_zero_through_the_actual_wire() {
    assert_ne!(Cell::Real(0.0), Cell::Real(-0.0));
    assert_eq!(Cell::Real(-0.0), Cell::Real(-0.0));
    for input in [0.0_f64, -0.0_f64, 1.0_f64] {
        let encoded = serde_json::to_vec(&Cell::Real(input)).unwrap();
        let Cell::Real(decoded) = serde_json::from_slice::<Cell>(&encoded).unwrap() else {
            panic!("real cell lost its type");
        };
        assert_eq!(decoded.to_bits(), input.to_bits());
    }
    assert!(serde_json::from_value::<Cell>(json!({"type":"null","forged":"payload"})).is_err());
}

#[test]
fn graph_hint_contains_no_target_ids_counts_or_replay_receipt() {
    assert!(serde_json::from_value::<Changed>(json!({"crewId":"crew"})).is_ok());
    for extra in ["target", "objectIds", "hiddenCount", "receipt"] {
        let mut hint = json!({"crewId":"crew"});
        hint[extra] = json!("forged");
        assert!(
            serde_json::from_value::<Changed>(hint).is_err(),
            "accepted {extra}"
        );
    }
    let schema = crewd_proto::api_schema();
    assert!(schema["events"][CHANGED].is_object());
    for method in METHODS {
        assert!(schema["methods"][method].is_object(), "missing {method}");
    }
}
