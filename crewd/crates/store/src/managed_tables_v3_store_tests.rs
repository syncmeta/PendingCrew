use super::*;
#[test]
fn option_floor_is_transactional_survives_purge_and_carries_before_schema32() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE managed_table_v3_meta(next_option_number INTEGER NOT NULL);").unwrap();
    assert!(remember_option_floor(&c, 17).is_err());
    {
        let tx = c.unchecked_transaction().unwrap();
        remember_option_floor(&tx, 17).unwrap();
    }
    assert_eq!(option_allocator_floor(&c).unwrap(), 0);
    {
        let tx = c.unchecked_transaction().unwrap();
        remember_option_floor(&tx, 17).unwrap();
        tx.commit().unwrap();
    }
    c.execute("INSERT INTO managed_table_v3_meta VALUES(3)", [])
        .unwrap();
    c.execute("DELETE FROM managed_table_v3_meta", []).unwrap();
    assert_eq!(option_allocator_floor(&c).unwrap(), 17);
    c.execute("DROP TABLE managed_table_v3_meta", []).unwrap();
    {
        let tx = c.unchecked_transaction().unwrap();
        preserve_option_restore_floor(&tx, 73).unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(option_allocator_floor(&c).unwrap(), 73);
    c.execute(
        "CREATE TABLE managed_table_v3_meta(next_option_number INTEGER NOT NULL)",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO managed_table_v3_meta VALUES(2)", [])
        .unwrap();
    {
        let tx = c.unchecked_transaction().unwrap();
        preserve_option_restore_floor(&tx, i64::MAX).unwrap();
        tx.commit().unwrap();
    }
    let pair: (i64, String) = c
        .query_row(
            "SELECT next_option_number,typeof(next_option_number) FROM managed_table_v3_meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(pair, (i64::MAX, "integer".into()));
    assert!(pair.0.checked_add(1).is_none());
}
#[test]
fn option_witness_is_private_monotone_and_refuses_corrupt_or_linked_files() {
    let d = tempfile::tempdir().unwrap();
    persist_option_witness(d.path(), 73).unwrap();
    persist_option_witness(d.path(), 2).unwrap();
    assert_eq!(option_witness_floor(d.path()).unwrap(), 73);
    crate::fsutil::atomic_write(&d.path().join(OPTION_WITNESS), b"073\n", 0o600).unwrap();
    assert!(option_witness_floor(d.path()).is_err());
    assert!(persist_option_witness(d.path(), 90).is_err());
    std::fs::remove_file(d.path().join(OPTION_WITNESS)).unwrap();
    std::os::unix::fs::symlink("missing", d.path().join(OPTION_WITNESS)).unwrap();
    assert!(option_witness_floor(d.path()).is_err());
}
#[test]
fn legacy_bounds_and_lazy_numeric_codec_do_not_convert_storage_or_float_decimals() {
    let old = vec![old::Field {
        id: "p1".into(),
        name: "Legacy".into(),
        kind: old::FieldKind::Text {},
        nullable: false,
        hidden: false,
    }];
    let fields = values::decode_fields(&serde_json::to_string(&old).unwrap()).unwrap();
    assert!(
        matches!(fields[0].kind,FieldKind::Text{ref bounds}if bounds.max_unicode_scalars.is_none())
    );
    assert!(values::scalar_valid(
        "T1",
        &fields[0],
        &Cell::Text("x".repeat(65536)),
        true
    ));
    let cell = values::logical_cell(
        Cell::Decimal("12345678901234567890123456789012345678".into()),
        Some("decimal"),
    )
    .unwrap();
    assert_eq!(
        cell,
        Cell::Text("12345678901234567890123456789012345678".into())
    );
    assert!(values::logical_cell(Cell::Decimal("1e9".into()), Some("decimal")).is_err());
}
