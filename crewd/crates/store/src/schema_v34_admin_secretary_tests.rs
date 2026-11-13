//! Public34 is Secretary only. These are genuine historical inputs and original
//! open/backup/restore operations; no private query tree or hand-downgraded DB.
use super::schema_v33_admin_ask_groups_tests::{image, rows, seed32, snapshot};
use crate::{Store, admin_secretary as secretary, schema};
use rusqlite::{
    Connection,
    hooks::{AuthAction, AuthContext, Authorization},
};
use std::os::unix::fs::PermissionsExt;
const PRINCIPAL: &str = "synthetic-original-human";
const REQUEST: &str = "11111111-1111-4111-8111-111111111111";
fn version(c: &Connection) -> i64 {
    c.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
fn seed33(c: &Connection) {
    seed32(c);
    let tx = c.unchecked_transaction().unwrap();
    crate::admin_ask_groups::migrate(&tx).unwrap();
    tx.execute_batch("PRAGMA user_version=33;").unwrap();
    crate::admin_ask_groups::validate_schema(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(version(c), 33);
    assert!(!secretary::available(c).unwrap());
}
fn assert_preserved(c: &Connection, before: &std::collections::BTreeMap<String, Vec<Vec<String>>>) {
    for (table, data) in before {
        let mut actual = rows(c, table);
        if table == "meta" {
            for key in [
                secretary::FORMAT_KEY,
                secretary::DATABASE_KEY,
                secretary::PUBLIC_FORMAT_KEY,
                secretary::PUBLIC_DATABASE_KEY,
            ] {
                let value = crate::meta_get(c, key).unwrap().unwrap();
                let row = vec![
                    format!("t:{:02x?}", key.as_bytes()),
                    format!("t:{:02x?}", value.as_bytes()),
                ];
                let index = actual.iter().position(|v| v == &row).unwrap();
                actual.remove(index);
            }
        }
        assert_eq!(&actual, data, "original {table} changed during33->34");
    }
}
#[test]
fn real33_protected_open_installs_public34_once_without_touching_original_authority_or_receipts() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join(crate::DB_FILE);
    let c = Connection::open(&path).unwrap();
    seed33(&c);
    let old = snapshot(&c);
    let db = crate::meta_get(&c, "dbId").unwrap().unwrap();
    let receipt = crate::command_receipts::lookup(&c, PRINCIPAL, &db, REQUEST)
        .unwrap()
        .unwrap();
    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    drop(c);
    let s = Store::open(&path).unwrap();
    assert_eq!(version(s.conn()), 34);
    secretary::validate_public(s.conn()).unwrap();
    assert!(secretary::available(s.conn()).unwrap());
    assert_preserved(s.conn(), &old);
    assert_eq!(
        crate::command_receipts::lookup(s.conn(), PRINCIPAL, &db, REQUEST)
            .unwrap()
            .unwrap(),
        receipt
    );
    for table in [
        "admin_secretary_sources",
        "admin_secretary_references",
        "admin_secretary_publications",
        "admin_secretary_observers",
    ] {
        assert!(rows(s.conn(), table).is_empty())
    }
    let backups = std::fs::read_dir(d.path().join("backups"))
        .unwrap()
        .map(|v| v.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    assert!(
        backups[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("crewd-v33-pre-v34-")
    );
    let protected =
        Connection::open_with_flags(&backups[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&protected), 33);
    assert_eq!(snapshot(&protected), old);
    drop(protected);
    let post = snapshot(s.conn());
    s.finish().unwrap();
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(snapshot(s.conn()), post);
    assert_eq!(
        std::fs::read_dir(d.path().join("backups")).unwrap().count(),
        1
    );
    let changes = s.conn().total_changes();
    secretary::validate_public(s.conn()).unwrap();
    assert_eq!(s.conn().total_changes(), changes);
}
#[test]
fn supported_real_older_generations_protect_their_exact_input_before_public34() {
    for old in [4, 8, 21, 32, 33] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(crate::DB_FILE);
        let c = Connection::open(&path).unwrap();
        crate::testing::historical_fixture(&c, old).unwrap();
        c.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('retained','original','old','old')").unwrap();
        crate::meta_set(&c, "migration34.originalWitness", "original\0bytes").unwrap();
        let draft = crate::meta_get(&c, "personalDraft.restoreStamp.v1").unwrap();
        let before = snapshot(&c);
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        drop(c);
        let s = Store::open(&path).unwrap();
        assert_eq!(version(s.conn()), 34);
        secretary::validate_public(s.conn()).unwrap();
        assert_eq!(
            crate::meta_get(s.conn(), "migration34.originalWitness")
                .unwrap()
                .as_deref(),
            Some("original\0bytes")
        );
        if let Some(draft) = draft {
            assert_eq!(crate::personal_drafts::epoch(s.conn()).unwrap(), draft)
        }
        let cuts = std::fs::read_dir(d.path().join("backups"))
            .unwrap()
            .map(|v| v.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(cuts.len(), 1);
        let protected =
            Connection::open_with_flags(&cuts[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(version(&protected), old);
        assert_eq!(snapshot(&protected), before);
    }
}
#[test]
fn late_ddl_and_final_marker_faults_roll_back_the_entire_administrative_transaction() {
    for old in [32, 33] {
        for fault in ["ddl", "marker"] {
            let c = Connection::open_in_memory().unwrap();
            if old == 33 {
                seed33(&c)
            } else {
                seed32(&c)
            }
            if fault == "marker" {
                c.execute_batch("CREATE TRIGGER reject_public34_marker BEFORE INSERT ON meta WHEN NEW.key='adminSecretary.publicFormat.v34' BEGIN SELECT RAISE(ABORT,'injected late public34 marker failure'); END;").unwrap();
            }
            let before = snapshot(&c);
            let catalog = rows(&c, "sqlite_master");
            if fault == "ddl" {
                c.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
                    AuthAction::CreateTable { table_name }
                        if table_name == "admin_secretary_observers" =>
                    {
                        Authorization::Deny
                    }
                    _ => Authorization::Allow,
                }))
                .unwrap();
            }
            assert!(schema::migrate_guarded(&c, None).is_err(), "{old}/{fault}");
            c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();
            assert_eq!(version(&c), old);
            assert_eq!(snapshot(&c), before);
            assert_eq!(rows(&c, "sqlite_master"), catalog);
            assert!(
                crate::meta_get(&c, secretary::PUBLIC_FORMAT_KEY)
                    .unwrap()
                    .is_none()
            );
            assert!(
                crate::meta_get(&c, secretary::FORMAT_KEY)
                    .unwrap()
                    .is_none()
            );
            if fault == "marker" {
                c.execute_batch("DROP TRIGGER reject_public34_marker")
                    .unwrap()
            }
            schema::migrate_guarded(&c, None).unwrap();
            assert_eq!(version(&c), 34);
            secretary::validate_public(&c).unwrap();
        }
    }
}
const BAD: &[&str] = &[
    "PRAGMA application_id=1234",
    "DELETE FROM meta WHERE key='adminSecretary.publicFormat.v34'",
    "UPDATE meta SET value='wrong-format' WHERE key='adminSecretary.schemaFormat.v1'",
    "UPDATE meta SET value='other-db' WHERE key='adminSecretary.publicDatabase.v34'",
    "INSERT INTO meta VALUES('adminSecretary.unpublishedFlag','not-public34')",
    "CREATE TABLE admin_secretary_unpublished(extra TEXT)",
    "CREATE INDEX unrelated_extra_secretary_index ON admin_secretary_sources(crew_id)",
    "CREATE TRIGGER incompatible_secretary_effect AFTER INSERT ON admin_secretary_sources BEGIN DELETE FROM admin_secretary_observers; END",
    "CREATE TABLE work_view_unpublished_query34(unpublished TEXT)",
    "INSERT INTO meta VALUES('managedViews.syntheticForeignFormat','unpublished')",
];
#[test]
fn current34_verifiers_are_pure_and_reject_bad_extra_hybrid_or_same_number_unsupported_formats() {
    for mutation in BAD {
        let s = Store::open_in_memory().unwrap();
        s.conn().execute_batch(mutation).unwrap();
        let before = snapshot(s.conn());
        let catalog = rows(s.conn(), "sqlite_master");
        let changes = s.conn().total_changes();
        assert!(secretary::validate_public(s.conn()).is_err(), "{mutation}");
        assert!(
            schema::migrate_guarded(s.conn(), None).is_err(),
            "{mutation}"
        );
        assert_eq!(s.conn().total_changes(), changes);
        assert_eq!(snapshot(s.conn()), before);
        assert_eq!(rows(s.conn(), "sqlite_master"), catalog);
    }
    let old = Connection::open_in_memory().unwrap();
    seed33(&old);
    old.execute_batch("PRAGMA user_version=34").unwrap();
    assert!(
        schema::migrate_guarded(&old, None).is_err(),
        "a reused number without public Secretary format is never adopted"
    );
    let old = Connection::open_in_memory().unwrap();
    seed33(&old);
    old.execute_batch("BEGIN").unwrap();
    secretary::migrate(&old).unwrap();
    old.execute_batch("COMMIT").unwrap();
    let before = snapshot(&old);
    assert!(
        schema::migrate_guarded(&old, None).is_err(),
        "unactivated33 module candidate is not genuine33 input"
    );
    assert_eq!(snapshot(&old), before);
}
#[test]
fn invalid_cold34_refuses_before_original_chmod_sidecars_recovery_backup_or_restore_target_changes()
{
    for mutation in BAD {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join(crate::DB_FILE);
        let s = Store::open(&path).unwrap();
        s.conn().execute_batch(mutation).unwrap();
        s.finish().unwrap();
        drop(s);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let before = image(source.path());
        assert!(Store::open(&path).is_err(), "{mutation}");
        assert!(Store::recovery_input_guard(source.path()).is_err());
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("refused.db");
        assert!(crate::backup::backup_db_quiescent(&path, &dest).is_err());
        assert!(!dest.exists());
        let target = tempfile::tempdir().unwrap();
        let current = Store::open_data_dir(target.path()).unwrap();
        current.finish().unwrap();
        drop(current);
        let target_before = image(target.path());
        assert!(crate::backup::restore(target.path(), &path).is_err());
        assert_eq!(image(target.path()), target_before);
        assert_eq!(image(source.path()), before);
    }
}
#[test]
fn committed_wal34_qualifies_latest_cut_on_private_copy_and_refuses_bad_format_without_shm() {
    for mutation in [
        None,
        Some("DELETE FROM meta WHERE key='adminSecretary.publicFormat.v34'"),
    ] {
        let live = tempfile::tempdir().unwrap();
        let db = live.path().join(crate::DB_FILE);
        let s = Store::open(&db).unwrap();
        s.conn()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        if let Some(sql) = mutation {
            s.conn().execute_batch(sql).unwrap()
        } else {
            crate::meta_set(s.conn(), "wal34-original", "latest committed value").unwrap()
        }
        let cut = tempfile::tempdir().unwrap();
        let copied = cut.path().join(crate::DB_FILE);
        std::fs::copy(&db, &copied).unwrap();
        std::fs::copy(
            live.path().join("crewd.db-wal"),
            cut.path().join("crewd.db-wal"),
        )
        .unwrap();
        assert!(
            std::fs::metadata(cut.path().join("crewd.db-wal"))
                .unwrap()
                .len()
                > 0
        );
        assert!(!cut.path().join("crewd.db-shm").exists());
        let before = image(cut.path());
        let guard = Store::recovery_input_guard(cut.path());
        if mutation.is_some() {
            assert!(guard.is_err());
            assert!(Store::open(&copied).is_err())
        } else {
            guard.unwrap().recheck().unwrap();
        }
        assert_eq!(image(cut.path()), before);
        if mutation.is_none() {
            let opened = Store::open(&copied).unwrap();
            assert_eq!(
                crate::meta_get(opened.conn(), "wal34-original")
                    .unwrap()
                    .as_deref(),
                Some("latest committed value")
            );
            secretary::validate_public(opened.conn()).unwrap();
        }
    }
}
#[test]
fn core_restore_old33_and_current34_preserves_business_and_original_unknown_receipt_but_fences_scope()
 {
    for old in [33, 34] {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("source.db");
        let c = Connection::open(&path).unwrap();
        seed33(&c);
        if old == 34 {
            schema::migrate_guarded(&c, None).unwrap()
        }
        let old_book = rows(&c, "organization_work_book");
        let messages = rows(&c, "messages");
        let db = crate::meta_get(&c, "dbId").unwrap().unwrap();
        let stamp = crate::personal_drafts::epoch(&c).unwrap();
        let judge = crate::judgment_state::epoch(&c).unwrap();
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);PRAGMA journal_mode=DELETE;")
            .unwrap();
        drop(c);
        let source_before = image(source.path());
        let target = tempfile::tempdir().unwrap();
        let current = Store::open_data_dir(target.path()).unwrap();
        current.finish().unwrap();
        drop(current);
        crate::backup::restore(target.path(), &path).unwrap();
        let restored = Store::open_data_dir(target.path()).unwrap();
        assert_eq!(version(restored.conn()), 34);
        secretary::validate_public(restored.conn()).unwrap();
        assert_eq!(rows(restored.conn(), "organization_work_book"), old_book);
        assert_eq!(rows(restored.conn(), "messages"), messages);
        assert_ne!(
            crate::personal_drafts::epoch(restored.conn()).unwrap(),
            stamp
        );
        assert_ne!(
            crate::judgment_state::epoch(restored.conn()).unwrap(),
            judge
        );
        let receipt = crate::command_receipts::lookup(restored.conn(), PRINCIPAL, &db, REQUEST)
            .unwrap()
            .unwrap();
        assert!(receipt.historical);
        assert_eq!(receipt.phase, crewd_proto::commands::CommandPhase::Unknown);
        assert!(rows(restored.conn(), "admin_secretary_observers").is_empty());
        assert!(rows(restored.conn(), "admin_secretary_sources").is_empty());
        assert_eq!(image(source.path()), source_before);
    }
}

#[test]
fn knowledge_only_restore_preserves_public34_core_scope_work_messages_and_receipts_and_full_split_stays_closed()
 {
    use crate::team_data::{Knowledge, Mutation};
    use crewd_proto::team_data::{DataContext, DataRestoreParams};
    use std::collections::BTreeMap;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().canonicalize().unwrap();
    let path = data.join(crate::DB_FILE);
    let c = Connection::open(&path).unwrap();
    seed33(&c);
    drop(c);
    let core = Store::open(&path).unwrap();
    let before = snapshot(core.conn());
    let db = crate::meta_get(core.conn(), "dbId").unwrap().unwrap();
    let base = DataContext {
        db_id: db.clone(),
        authority_id: db.clone(),
        restore_epoch: "knowledge-original".into(),
        crew_id: "first33".into(),
        namespace: crate::team_data::namespace(&db, "first33"),
        generation: "0".into(),
    };
    let mut knowledge =
        Knowledge::open(&data.join("knowledge/first33.sqlite"), base, true).unwrap();
    let context = knowledge.context().unwrap();
    knowledge
        .apply(
            &uuid::Uuid::new_v4().to_string(),
            &context,
            "human:current",
            "original",
            BTreeMap::new(),
            vec![
                Mutation::Sql {
                    sql: "CREATE TABLE facts(id INTEGER PRIMARY KEY)".into(),
                    params: vec![],
                },
                Mutation::Sql {
                    sql: "INSERT INTO facts VALUES(1)".into(),
                    params: vec![],
                },
            ],
            false,
            None,
        )
        .unwrap();
    let saved = knowledge.snapshot().unwrap();
    let context = knowledge.context().unwrap();
    knowledge
        .apply(
            &uuid::Uuid::new_v4().to_string(),
            &context,
            "human:current",
            "new data",
            BTreeMap::new(),
            vec![Mutation::Sql {
                sql: "INSERT INTO facts VALUES(2)".into(),
                params: vec![],
            }],
            false,
            None,
        )
        .unwrap();
    let context = knowledge.context().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    knowledge
        .restore(
            DataRestoreParams {
                request_id: request.clone(),
                context: context.clone(),
                snapshot_id: saved,
            },
            "human:current",
        )
        .unwrap();
    assert_ne!(
        knowledge.context().unwrap().restore_epoch,
        context.restore_epoch
    );
    assert!(matches!(
        knowledge
            .lookup(&request, &context, "human:current", true)
            .unwrap(),
        crewd_proto::team_data::DataRequestLookup::Historical(_)
    ));
    assert_eq!(
        snapshot(core.conn()),
        before,
        "knowledge rewind cannot rewind Core ACL, Source scope, original H/W/messages or original public receipts"
    );
    secretary::validate_public(core.conn()).unwrap();
    let source = root.path().join("full-source.sqlite");
    crate::backup::backup_db_quiescent(&path, &source).unwrap();
    core.finish().unwrap();
    drop(core);
    // Existing full/partition policy stays closed: never split a knowledge
    // workspace or native-data authority using a standalone Core restore.
    let before = image(&data);
    assert!(crate::backup::restore(&data, &source).is_err());
    assert_eq!(image(&data), before);
    let target = tempfile::tempdir().unwrap();
    let current = Store::open_data_dir(target.path()).unwrap();
    current.finish().unwrap();
    drop(current);
    std::fs::create_dir(target.path().join("native-data")).unwrap();
    let native_before = image(target.path());
    assert!(crate::backup::restore(target.path(), &source).is_err());
    assert_eq!(image(target.path()), native_before);
}

#[test]
fn genuine33_with_unpublished_completion_or_case_traces_is_not_adopted_as_clean_input() {
    for (key, value) in [
        (
            "patrol.secretary.completed.v1:first33",
            "old unqualified completion cache",
        ),
        ("patrol.run.v1:first33", r#"{"secretary_scope":null}"#),
        (
            "admin.case.v1:synthetic:AC00000000000000000001",
            r#"{"view":{"source":{"provenance":{"kind":"secretary_brief"}}}}"#,
        ),
    ] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(crate::DB_FILE);
        let c = Connection::open(&path).unwrap();
        seed33(&c);
        crate::meta_set(&c, key, value).unwrap();
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        drop(c);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let before = image(d.path());
        assert!(Store::open(&path).is_err(), "{key}");
        assert!(Store::recovery_input_guard(d.path()).is_err());
        assert_eq!(
            image(d.path()),
            before,
            "old candidate evidence is never repaired, relabeled or upgraded"
        );
    }
}
