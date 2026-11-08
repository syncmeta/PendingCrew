//! Retained H33 component guards under the current protected upgrade. Original
//! historical33 evidence remains attached to its original source, never relabeled.
use crate::{Store, admin_ask_groups as groups, schema};
use rusqlite::{Connection, params, types::ValueRef};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path};
const REQUEST: &str = "11111111-1111-4111-8111-111111111111";
const PRINCIPAL: &str = "synthetic-original-human";
fn version(c: &Connection) -> i64 {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
pub(super) fn rows(c: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut q = c
        .prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))
        .unwrap();
    let n = q.column_count();
    let mut out = q
        .query_map([], |r| {
            (0..n)
                .map(|i| {
                    Ok(match r.get_ref(i)? {
                        ValueRef::Null => "n".into(),
                        ValueRef::Integer(v) => format!("i:{v}"),
                        ValueRef::Real(v) => format!("r:{:016x}", v.to_bits()),
                        ValueRef::Text(v) => format!("t:{v:02x?}"),
                        ValueRef::Blob(v) => format!("b:{v:02x?}"),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    out.sort();
    out
}
pub(super) fn snapshot(c: &Connection) -> BTreeMap<String, Vec<Vec<String>>> {
    let names=c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') ORDER BY name").unwrap()
        .query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let data = rows(c, &name);
            (name, data)
        })
        .collect()
}
fn assert_old_rows(c: &Connection, old: &BTreeMap<String, Vec<Vec<String>>>) {
    for (table, data) in old {
        if table == "meta" {
            let mut actual = rows(c, table);
            for key in [
                groups::FORMAT_KEY,
                "adminAskGroups.schemaDatabase.v1",
                crate::admin_secretary::FORMAT_KEY,
                crate::admin_secretary::DATABASE_KEY,
                crate::admin_secretary::PUBLIC_FORMAT_KEY,
                crate::admin_secretary::PUBLIC_DATABASE_KEY,
            ] {
                let value: String = c
                    .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
                    .unwrap();
                let key = vec![
                    format!("t:{:02x?}", key.as_bytes()),
                    format!("t:{:02x?}", value.as_bytes()),
                ];
                let at = actual
                    .iter()
                    .position(|row| row == &key)
                    .expect("new format metadata row");
                actual.remove(at);
            }
            assert_eq!(
                &actual, data,
                "all old meta identity/scope/allocator bytes unchanged"
            );
        } else {
            assert_eq!(&rows(c, table), data, "changed original {table}");
        }
    }
}
pub(super) fn seed32(c: &Connection) {
    crate::testing::historical_fixture(c, 32).unwrap();
    assert_eq!(version(c), 32);
    assert!(!groups::available(c).unwrap());
    let context = crate::search_context::Context::register(c).unwrap();
    let _span = context.enter(1_000).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    tx.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('first33','First','old','old'),('neighbor33','Neighbor','old','old');
      INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('first33-worker','first33','worker','codex','old'),('neighbor33-worker','neighbor33','worker','codex','old');
      INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES('first33-human','first33','human','Human','human','old'),('neighbor33-human','neighbor33','human','Human','human','old');").unwrap();
    let mut book = crewd_work::WorkBook::restore(
        crate::work_organization::load(&tx).unwrap().unwrap(),
        crewd_work::Config::default(),
    )
    .unwrap();
    for crew in ["first33", "neighbor33"] {
        book.set_default_crew(crewd_work::CrewId::new(crew));
        let by = crewd_work::Actor::Session {
            id: crewd_work::SessionId::new(format!("{crew}-worker")),
            role: crewd_work::Role::Worker,
        };
        let opened = book
            .open_ask(
                &by,
                crewd_work::Ts(1_000),
                "Original independent question?",
                Some(crewd_work::DefaultOption {
                    text: "Keep original default".into(),
                    after_ms: 86_400_000,
                }),
                None,
            )
            .unwrap();
        let h = opened.value.to_string();
        crate::work_organization::bind_core(
            &tx,
            crate::object_refs::ObjectKind::HumanTask,
            crew,
            &h,
            "old",
        )
        .unwrap();
        let message = format!("{crew}-announcement");
        tx.execute("INSERT INTO messages(id,channel_id,sender_kind,sender_session_id,text,category,created_at) VALUES(?1,?2,'session',?3,'Original independent question?','ask','old')",params![message,format!("{crew}-human"),format!("{crew}-worker")]).unwrap();
        tx.execute("INSERT INTO ask_meta(crew_id,ask,resume_note,message_id) VALUES(?1,?2,'original resume',?3)",params![crew,format!("organization:{h}"),message]).unwrap();
    }
    let events = book.take_log();
    crate::work_organization::save(&tx, book.state(), &events, "old").unwrap();
    use crewd_proto::{
        managed_objects::{Cell, Field, FieldType},
        managed_tables_v2::Author,
    };
    let fields = [
        ("large", FieldType::Text),
        ("integer", FieldType::Integer),
        ("positive", FieldType::Real),
        ("negative", FieldType::Real),
    ]
    .into_iter()
    .map(|(name, field_type)| Field {
        name: name.into(),
        field_type,
        nullable: false,
    })
    .collect::<Vec<_>>();
    let table = crate::managed_objects::create_table_by(
        &tx,
        "first33",
        "Original table",
        &fields,
        &Author::Human {},
        "old",
    )
    .unwrap();
    let t = crate::managed_tables_v2::number(&table.id, 'T').unwrap();
    let row = crate::managed_objects::add_row_by(
        &tx,
        "first33",
        t,
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
    let r = crate::managed_tables_v2::number(&row.id, 'O').unwrap();
    tx.execute(
        "UPDATE managed_table_attributes SET schema_revision=17 WHERE number=?1",
        [t],
    )
    .unwrap();
    tx.execute("UPDATE managed_rows SET revision=23 WHERE number=?1", [r])
        .unwrap();
    let db = crate::meta_get(&tx, "dbId").unwrap().unwrap();
    use crewd_proto::commands::{CommandPhase, CommandSubmitParams, PublicCommand};
    let request = CommandSubmitParams {
        request_id: REQUEST.into(),
        db_id: db.clone(),
        runtime_binding_id: "22222222-2222-4222-8222-222222222222".into(),
        command: PublicCommand::Hibernate(Default::default()),
    };
    assert!(matches!(
        crate::command_receipts::reserve(&tx, PRINCIPAL, &request, "old").unwrap(),
        crate::command_receipts::Reserved::New(_)
    ));
    crate::command_receipts::transition(
        &tx,
        PRINCIPAL,
        &db,
        REQUEST,
        (CommandPhase::Reserved, CommandPhase::Unknown),
        None,
        "old",
    )
    .unwrap();
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('original-byte-witness',?1)",
        ["original\0bytes"],
    )
    .unwrap();
    tx.commit().unwrap();
}
#[test]
fn genuine32_to_current_preserves_h33_every_old_row_default_uuid_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.db");
    let c = Connection::open(&path).unwrap();
    seed32(&c);
    let old = snapshot(&c);
    let db = crate::meta_get(&c, "dbId").unwrap().unwrap();
    let receipt = crate::command_receipts::lookup(&c, PRINCIPAL, &db, REQUEST)
        .unwrap()
        .unwrap();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(c);
    let migrated = Store::open(&path).unwrap();
    assert_eq!(version(migrated.conn()), schema::SCHEMA_VERSION);
    groups::validate_schema(migrated.conn()).unwrap();
    assert!(groups::available(migrated.conn()).unwrap());
    assert_old_rows(migrated.conn(), &old);
    assert_eq!(
        crate::command_receipts::lookup(migrated.conn(), PRINCIPAL, &db, REQUEST)
            .unwrap()
            .unwrap(),
        receipt
    );
    for table in ["admin_ask_group_observations", "admin_ask_group_sources"] {
        assert!(rows(migrated.conn(), table).is_empty());
    }
    assert_eq!(rows(migrated.conn(), "admin_ask_group_scan").len(), 2);
    assert_eq!(rows(migrated.conn(), "admin_ask_group_scheduler").len(), 1);
    let backups = std::fs::read_dir(dir.path().join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert!(
        backups[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(&format!("crewd-v32-pre-v{}-", schema::SCHEMA_VERSION))
    );
    let before =
        Connection::open_with_flags(&backups[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&before), 32);
    assert_eq!(snapshot(&before), old);
    assert!(!groups::available(&before).unwrap());
    drop(before);
    let complete = snapshot(migrated.conn());
    migrated.finish().unwrap();
    drop(migrated);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(snapshot(reopened.conn()), complete);
    assert_eq!(
        std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .count(),
        1
    );
    reopened.finish().unwrap();
}
#[test]
fn late33_ddl_conflict_rolls_back_identity_tables_marker_and_original32_bytes() {
    let c = Connection::open_in_memory().unwrap();
    seed32(&c);
    c.execute_batch("CREATE TABLE admin_ask_group_sources(original TEXT);INSERT INTO admin_ask_group_sources VALUES('original conflict evidence')").unwrap();
    let old = snapshot(&c);
    let ddl = rows(&c, "sqlite_master");
    assert!(schema::migrate_guarded(&c, None).is_err());
    assert_eq!(version(&c), 32);
    assert_eq!(snapshot(&c), old);
    assert_eq!(rows(&c, "sqlite_master"), ddl);
    assert!(crate::meta_get(&c, groups::FORMAT_KEY).unwrap().is_none());
    assert!(
        crate::meta_get(&c, "adminAskGroups.schemaDatabase.v1")
            .unwrap()
            .is_none()
    );
    c.execute_batch("DROP TABLE admin_ask_group_sources")
        .unwrap();
    schema::migrate_guarded(&c, None).unwrap();
    assert_eq!(version(&c), schema::SCHEMA_VERSION);
    groups::validate_schema(&c).unwrap();
}
#[test]
fn fresh_current_has_real_h33_identity_and_validator_never_repairs_incompatible_formats() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(version(store.conn()), schema::SCHEMA_VERSION);
    let before = store.conn().total_changes();
    groups::validate_schema(store.conn()).unwrap();
    assert_eq!(store.conn().total_changes(), before);
    for mutation in [
        "DELETE FROM meta WHERE key='adminAskGroups.schemaFormat.v1'",
        "UPDATE meta SET value='another-db' WHERE key='adminAskGroups.schemaDatabase.v1'",
        "DROP INDEX admin_ask_group_pending",
        "DELETE FROM admin_ask_group_scheduler",
        "CREATE TRIGGER incompatible_h_effect AFTER INSERT ON admin_ask_group_sources BEGIN DELETE FROM admin_ask_group_observations;END",
        "CREATE TABLE work_view_unpublished_query33(unpublished TEXT)",
    ] {
        let store = Store::open_in_memory().unwrap();
        store.conn().execute_batch(mutation).unwrap();
        let data = snapshot(store.conn());
        let objects = rows(store.conn(), "sqlite_master");
        let changes = store.conn().total_changes();
        assert!(groups::validate_schema(store.conn()).is_err(), "{mutation}");
        assert!(
            schema::migrate_guarded(store.conn(), None).is_err(),
            "{mutation}"
        );
        assert_eq!(store.conn().total_changes(), changes);
        assert_eq!(snapshot(store.conn()), data);
        assert_eq!(rows(store.conn(), "sqlite_master"), objects);
    }
}
pub(super) fn image(dir: &Path) -> BTreeMap<String, (Vec<u8>, u32)> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, (Vec<u8>, u32)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let m = e.metadata().unwrap();
            if m.is_dir() {
                visit(root, &p, out);
            } else {
                assert!(m.is_file());
                out.insert(
                    p.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    (std::fs::read(&p).unwrap(), m.permissions().mode()),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(dir, dir, &mut out);
    out
}
#[test]
fn current_with_incompatible_h33_refuses_before_chmod_sidecars_or_restore_target_changes() {
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("incompatible.db");
    let s = Store::open(&path).unwrap();
    s.conn()
        .execute("DELETE FROM meta WHERE key=?1", [groups::FORMAT_KEY])
        .unwrap();
    s.finish().unwrap();
    drop(s);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    let before = image(source.path());
    assert!(Store::open(&path).is_err());
    assert_eq!(image(source.path()), before);
    let target = tempfile::tempdir().unwrap();
    let s = Store::open_data_dir(target.path()).unwrap();
    s.finish().unwrap();
    drop(s);
    let target_before = image(target.path());
    assert!(crate::backup::restore(target.path(), &path).is_err());
    assert_eq!(image(target.path()), target_before);
    assert_eq!(image(source.path()), before);
}
#[test]
fn current_committed_wal_with_bad_h33_refuses_without_shm_or_original_recovery() {
    let original = tempfile::tempdir().unwrap();
    let path = original.path().join("core.db");
    let s = Store::open(&path).unwrap();
    s.conn()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    s.conn()
        .execute("DELETE FROM meta WHERE key=?1", [groups::FORMAT_KEY])
        .unwrap();
    let source = tempfile::tempdir().unwrap();
    let copy = source.path().join("core.db");
    std::fs::copy(&path, &copy).unwrap();
    let wal = source.path().join("core.db-wal");
    std::fs::copy(original.path().join("core.db-wal"), &wal).unwrap();
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    assert!(!source.path().join("core.db-shm").exists());
    let before = image(source.path());
    assert!(Store::open(&copy).is_err());
    assert_eq!(image(source.path()), before);
    let target = tempfile::tempdir().unwrap();
    let good = Store::open_data_dir(target.path()).unwrap();
    good.finish().unwrap();
    drop(good);
    let old = image(target.path());
    assert!(crate::backup::restore(target.path(), &copy).is_err());
    assert_eq!(image(target.path()), old);
    assert_eq!(image(source.path()), before);
    drop(s);
}
#[test]
fn pure_schema_verifier_rejects_different_strict_columns_checks_fk_actions_and_orphan_sources() {
    let s = Store::open_in_memory().unwrap();
    let c = s.conn();
    // Schema snapshots independently prove each alteration remains unsupported;
    // even an otherwise usable table with copied names is not this exact format.
    for replacement in [
        groups::DDL.replace(") STRICT;", ");"),
        groups::DDL.replace(
            "REFERENCES crews(id) ON DELETE CASCADE",
            "REFERENCES crews(id) ON DELETE RESTRICT",
        ),
        groups::DDL.replace(
            "second_number > first_number",
            "second_number >= first_number",
        ),
        groups::DDL.replace("source_fingerprint TEXT\n", "source_fingerprint BLOB\n"),
    ] {
        let x = Connection::open_in_memory().unwrap();
        crate::testing::historical_fixture(&x, 32).unwrap();
        x.execute_batch(&replacement).unwrap();
        x.execute("INSERT INTO admin_ask_group_scheduler VALUES(1,NULL)", [])
            .unwrap();
        crate::meta_set(&x, groups::FORMAT_KEY, groups::FORMAT).unwrap();
        let db = crate::meta_get(&x, "dbId").unwrap().unwrap();
        crate::meta_set(&x, "adminAskGroups.schemaDatabase.v1", &db).unwrap();
        assert!(groups::validate_schema(&x).is_err());
    }
    c.execute_batch("PRAGMA foreign_keys=OFF;INSERT INTO admin_ask_group_sources VALUES(999999,'missing-crew','old-db','old-epoch','','excluded',NULL);PRAGMA foreign_keys=ON;").unwrap();
    assert!(groups::validate_schema(c).is_err());
}
