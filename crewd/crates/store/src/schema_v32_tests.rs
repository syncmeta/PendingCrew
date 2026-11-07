//! Real historical31 -> protected32 migration. No current-schema relabeling.
use crate::{Store, managed_objects as old, managed_tables_v3 as new, schema};
use crewd_proto::managed_objects::{Cell, Field, FieldType};
use crewd_proto::managed_tables_v2::Author;
use rusqlite::Connection;

fn seed(c: &Connection) -> (i64, i64) {
    crate::testing::historical_fixture(c, 31).unwrap();
    let context = crate::search_context::Context::register(c).unwrap();
    let _write = context.enter(42).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO crews(id,title,created_at,updated_at) VALUES('legacy32','old table owner','old','old')", []).unwrap();
    let fields = [
        ("说明", FieldType::Text),
        ("整数", FieldType::Integer),
        ("正零", FieldType::Real),
        ("负零", FieldType::Real),
    ]
    .into_iter()
    .map(|(name, field_type)| Field {
        name: name.into(),
        field_type,
        nullable: false,
    })
    .collect::<Vec<_>>();
    let table =
        old::create_table_by(&tx, "legacy32", "旧表", &fields, &Author::Human {}, "old").unwrap();
    let table_number = crate::managed_tables_v2::number(&table.id, 'T').unwrap();
    let row = old::add_row_by(
        &tx,
        "legacy32",
        table_number,
        &[
            Cell::Text("旧".repeat(20_000)),
            Cell::Integer(i64::MAX.to_string()),
            Cell::Real(0.0),
            Cell::Real(-0.0),
        ],
        &Author::Human {},
        "old",
    )
    .unwrap();
    let row_number = crate::managed_tables_v2::number(&row.id, 'O').unwrap();
    tx.execute(
        "UPDATE managed_table_attributes SET schema_revision=17 WHERE number=?1",
        [table_number],
    )
    .unwrap();
    tx.execute(
        "UPDATE managed_rows SET revision=23 WHERE number=?1",
        [row_number],
    )
    .unwrap();
    tx.commit().unwrap();
    (table_number, row_number)
}
fn original(c: &Connection, table: i64, row: i64) -> String {
    c.query_row("SELECT json_array(t.number,t.title,t.fields_json,a.schema_revision,a.next_field_number,a.purpose,a.title_field_id,a.row_policy,a.author_json,a.v1_layout,r.number,r.revision,r.values_json,ra.author_json) FROM managed_tables t JOIN managed_table_attributes a ON a.number=t.number JOIN managed_rows r ON r.table_number=t.number JOIN managed_row_authors ra ON ra.number=r.number WHERE t.number=?1 AND r.number=?2",[table,row],|r|r.get(0)).unwrap()
}
fn version(c: &Connection) -> i64 {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
#[test]
fn core32_real31_protected_migration_keeps_legacy_bytes_authors_revisions_and_zero_bits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.db");
    let c = Connection::open(&path).unwrap();
    let (table, row) = seed(&c);
    let before = original(&c, table, row);
    assert_eq!(version(&c), 31);
    assert!(!new::enabled(&c).unwrap());
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(c);
    let migrated = Store::open(&path).unwrap();
    assert_eq!(version(migrated.conn()), schema::SCHEMA_VERSION);
    assert_eq!(original(migrated.conn(), table, row), before);
    let t = new::table(migrated.conn(), table).unwrap().unwrap();
    assert_eq!(t.schema_revision, "17");
    assert!(t.purpose.is_none());
    assert!(t.lifecycle.archived_at.is_none());
    let r = new::row_base(migrated.conn(), row).unwrap().unwrap();
    assert_eq!(r.revision, "23");
    assert!(r.lifecycle.archived_at.is_none());
    use crewd_proto::managed_tables_v3::Cell as V3Cell;
    assert!(matches!(&r.logical[0].value,V3Cell::Text(s) if s == &"旧".repeat(20_000)));
    assert!(matches!(&r.logical[1].value,V3Cell::Integer(s) if s == &i64::MAX.to_string()));
    assert!(matches!(r.logical[2].value,V3Cell::Real(n) if n.to_bits()==0.0_f64.to_bits()));
    assert!(matches!(r.logical[3].value,V3Cell::Real(n) if n.to_bits()==(-0.0_f64).to_bits()));
    let snapshots = std::fs::read_dir(dir.path().join("backups"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    let snapshot =
        Connection::open_with_flags(&snapshots[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&snapshot), 31);
    assert!(!new::enabled(&snapshot).unwrap());
    assert_eq!(original(&snapshot, table, row), before);
    migrated.finish().unwrap();
}
#[test]
fn core32_late_ddl_failure_rolls_back_all_new_metadata_and_preserves_actual31() {
    let c = Connection::open_in_memory().unwrap();
    let (table, row) = seed(&c);
    let before = original(&c, table, row);
    c.execute_batch("CREATE TABLE managed_table_audit_values(retained TEXT); INSERT INTO managed_table_audit_values VALUES('original conflict evidence')").unwrap();
    assert!(schema::migrate_guarded(&c, None).is_err());
    assert_eq!(version(&c), 31);
    assert!(!new::enabled(&c).unwrap());
    assert_eq!(original(&c, table, row), before);
    for name in [
        "managed_row_v3_meta",
        "managed_table_scalar_codecs",
        "managed_table_definition_clock",
        "managed_table_audit",
    ] {
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!exists, "partial schema32 object survived rollback: {name}");
    }
    let evidence: String = c
        .query_row("SELECT retained FROM managed_table_audit_values", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(evidence, "original conflict evidence");
}

#[test]
fn core32_v3_production_intent_bytes_match_all_eight_shared_swift_goldens() {
    use crate::managed_links;
    use crewd_proto::managed_tables_v3::{INTENT_DOMAIN, SUBMIT, Submit};
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../scripts/dev/fixtures/managed-table-v3-intent-goldens.json"
    ))
    .unwrap();
    let vectors = vectors.as_array().unwrap();
    assert_eq!(vectors.len(), 8);
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        let mut arguments = vector["normalized"].clone();
        assert_eq!(
            arguments.as_object_mut().unwrap().remove("method").unwrap(),
            SUBMIT
        );
        let request: Submit = serde_json::from_value(arguments).unwrap();
        let normalized = serde_json::json!({"method":SUBMIT,"scope":request.scope,
            "crewId":request.crew_id,"requestId":request.request_id,"operation":request.operation});
        assert_eq!(
            normalized, vector["normalized"],
            "typed normalization {name}"
        );
        let mut bytes = INTENT_DOMAIN.to_vec();
        managed_links::encode(&normalized, &mut bytes).unwrap();
        assert_eq!(
            managed_links::hex(&bytes),
            vector["canonicalHex"].as_str().unwrap(),
            "actual production encoder {name}"
        );
        let digest = managed_links::intent(
            SUBMIT,
            &request.scope,
            &request.crew_id,
            &request.request_id,
            &request.operation,
        )
        .unwrap();
        assert_eq!(
            digest,
            vector["sha256"].as_str().unwrap(),
            "actual submit intent {name}"
        );
    }
}
