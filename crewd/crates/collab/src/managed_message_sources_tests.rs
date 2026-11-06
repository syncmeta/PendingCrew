//! Source capture contracts through real collab transactions; all data is synthetic.
use crate::{
    Change as Fact, ProgressKind, Sender, managed_message_sources as sources,
    managed_objects::{self as objects, Actor},
    testkit::{NOW, count, database_snapshot, ok, ok_at},
};
use crewd_proto::{
    MemberRef, Message,
    managed_message_sources::{Lookup, RecordParams, RecordReceipt, Source},
    managed_objects::{Cell, Change, Field, FieldType, Object, Submit},
};
use crewd_store::Store;
use serde_json::json;

const HUMAN: Actor<'static> = Actor::Human("source-human");
const AUTHOR: Actor<'static> = Actor::Session("w-a1");
const FOREIGN: Actor<'static> = Actor::Session("w-b1");

fn fixture() -> Store {
    scoped_fixture(crate::testkit::store())
}
fn scoped_fixture(s: Store) -> Store {
    if crewd_store::meta_get(s.conn(), "dbId").unwrap().is_none() {
        crewd_store::meta_set(s.conn(), "dbId", "synthetic-message-source-db").unwrap();
    }
    s
}
fn post(s: &mut Store, channel: &str, text: &str) -> Message {
    ok(s, |tx| {
        crate::messages::post(
            tx,
            Sender::Human,
            crate::messages::NewPost {
                channel_id: channel.into(),
                text: text.into(),
                ..Default::default()
            },
        )
        .map(|posted| posted.message)
    })
}
fn decision(crew: &str) -> Change {
    serde_json::from_value(json!({"kind":"recordDecision","draft":{
        "question":"Which synthetic path?", "choice":"A", "rationale":"Verified fixture facts",
        "alternatives":null,"reversibility":"reversible","about":[{"kind":"crew","id":crew}]
    }}))
    .unwrap()
}
fn request(s: &Store, crew: &str, message: &Message, change: Change) -> RecordParams {
    RecordParams {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        source_message_id: message.id.clone(),
        change,
    }
}
fn copy(request: &RecordParams) -> RecordParams {
    serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap()
}
fn object_id(object: &Object) -> &str {
    match object {
        Object::Decision { decision } => &decision.id,
        Object::Risk { risk } => &risk.id,
        Object::Lesson { lesson } => &lesson.id,
        Object::Row { row } => &row.id,
        Object::Table { table } => &table.id,
    }
}
fn known(c: &rusqlite::Connection, actor: Actor<'_>, params: &RecordParams) -> RecordReceipt {
    match sources::lookup(c, actor, &params.scope, &params.crew_id, &params.request_id).unwrap() {
        Lookup::Known { receipt } => receipt,
        other => panic!("expected original known source receipt, got {other:?}"),
    }
}
fn table(s: &mut Store, crew: &str) -> String {
    let submit = Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: Change::CreateTable {
            title: "Synthetic capture rows".into(),
            fields: vec![Field {
                name: "Fact".into(),
                field_type: FieldType::Text,
                nullable: false,
            }],
        },
    };
    object_id(&ok(s, |tx| objects::submit(tx, HUMAN, submit)).object).to_string()
}

#[test]
fn capture_preserves_unicode_snapshot_and_exact_replay_without_more_effects() {
    let mut s = fixture();
    let text = "💡\u{754c}e\u{301}".repeat(100);
    let m = post(&mut s, "crew-a/lobby", &text);
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let messages = count(&s, "SELECT count(*) FROM messages");
    let wakes = count(&s, "SELECT count(*) FROM wakes");
    let (saved, facts) = ok_at(&mut s, NOW, |tx| sources::record(tx, AUTHOR, copy(&req)));
    let id = object_id(&saved.object).to_string();
    assert!(saved.created);
    assert_eq!(
        facts,
        vec![
            Fact::Progress {
                session_id: "w-a1".into(),
                kind: ProgressKind::Ontology
            },
            Fact::OntologyChanged {
                crew_id: "crew-a".into(),
                object_ids: vec![id.clone()]
            },
        ]
    );
    let expected = Source::Available {
        crew_id: "crew-a".into(),
        message_id: m.id.clone(),
        channel_id: m.channel_id.clone(),
        excerpt: text.chars().take(280).collect(),
    };
    assert_eq!(sources::source(s.conn(), HUMAN, &id).unwrap(), expected);
    let before = database_snapshot(&s);
    let (replayed, facts) = ok_at(&mut s, NOW + 5000, |tx| {
        sources::record(tx, AUTHOR, copy(&req))
    });
    assert_eq!(replayed, saved);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let duplicate = request(&s, "crew-a", &m, decision("crew-a"));
    let (existing, facts) = ok_at(&mut s, NOW + 6000, |tx| {
        sources::record(tx, AUTHOR, duplicate)
    });
    assert!(!existing.created);
    assert_eq!(existing.object, saved.object);
    assert!(facts.is_empty());
    assert_eq!(count(&s, "SELECT count(*) FROM managed_message_sources"), 1);
    assert_eq!(count(&s, "SELECT count(*) FROM messages"), messages);
    assert_eq!(count(&s, "SELECT count(*) FROM wakes"), wakes);
    let mut changed = copy(&req);
    changed.change = decision("crew-x");
    let before = database_snapshot(&s);
    assert!(
        crate::run(&mut s, NOW + 7000, |tx| sources::record(
            tx, AUTHOR, changed
        ))
        .is_err()
    );
    assert_eq!(database_snapshot(&s), before);
    // The source is a captured original excerpt, not a later copy of the live message body.
    s.conn()
        .execute(
            "UPDATE messages SET text='later fixture text' WHERE id=?1",
            [&m.id],
        )
        .unwrap();
    assert_eq!(sources::source(s.conn(), HUMAN, &id).unwrap(), expected);
    assert_eq!(known(s.conn(), AUTHOR, &req), saved);
}

#[test]
fn source_creation_supports_decision_risk_lesson_and_one_row_per_exact_table() {
    // Preserve two real V1 tables created under their original namespace policy.
    let mut s = scoped_fixture(crate::testkit::v31_store());
    let m = post(&mut s, "crew-a/lobby", "Synthetic shared source");
    let t1 = table(&mut s, "crew-a");
    let t2 = table(&mut s, "crew-a");
    crewd_store::schema::migrate(s.conn()).unwrap();
    assert!(crewd_store::managed_tables_v3::enabled(s.conn()).unwrap());
    assert_eq!(sources::source(s.conn(), HUMAN, &t1).unwrap(), Source::None);
    let changes = vec![
        decision("crew-a"),
        serde_json::from_value(json!({"kind":"recordRisk","owner":null,"draft":{
            "summary":"Synthetic risk", "impact":"medium", "likelihood":null,"mitigation":null,
            "about":[{"kind":"crew","id":"crew-a"}],"mitigatedBy":[]
        }}))
        .unwrap(),
        serde_json::from_value(json!({"kind":"recordLesson","draft":{
            "happened":"Synthetic event", "lesson":"Retain the exact source", "detection":null,
            "learnedFrom":[],"evidence":[],"appliesTo":[]
        }}))
        .unwrap(),
        Change::AddRow {
            table_id: t1.clone(),
            values: vec![Cell::Text("one".into())],
        },
        Change::AddRow {
            table_id: t2,
            values: vec![Cell::Text("two".into())],
        },
    ];
    let mut ids = vec![];
    for change in changes {
        let req = request(&s, "crew-a", &m, change);
        let receipt = ok(&mut s, |tx| sources::record(tx, HUMAN, req));
        assert!(receipt.created);
        ids.push(object_id(&receipt.object).to_string());
    }
    let duplicate = request(
        &s,
        "crew-a",
        &m,
        Change::AddRow {
            table_id: t1,
            values: vec![Cell::Text("must not replace existing row".into())],
        },
    );
    let (saved, facts) = ok_at(&mut s, NOW, |tx| sources::record(tx, HUMAN, duplicate));
    assert!(!saved.created);
    assert_eq!(object_id(&saved.object), ids[3]);
    assert!(
        matches!(&saved.object, Object::Row { row } if row.values == vec![Cell::Text("one".into())])
    );
    assert!(facts.is_empty());
    let page = sources::objects_from_message(s.conn(), HUMAN, &m.id, None).unwrap();
    assert_eq!(
        page.items.iter().map(object_id).collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(page.next_after.is_none());
    let forbidden = request(
        &s,
        "crew-a",
        &m,
        Change::CreateTable {
            title: "Not a capture kind".into(),
            fields: vec![],
        },
    );
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| sources::record(tx, HUMAN, forbidden)).is_err());
    assert_eq!(database_snapshot(&s), before);
}

#[test]
fn source_authority_is_rechecked_for_reads_lookup_and_record_after_membership_loss() {
    let mut s = fixture();
    // Foreign-hosted DM is visible through actual explicit membership, not object organization-read.
    let (dm, _) = ok(&mut s, |tx| {
        crate::channels::ensure_dm(
            tx,
            "crew-x",
            &MemberRef::session("cap-x"),
            &MemberRef::session("w-a1"),
        )
    });
    let m = post(&mut s, &dm.id, "PRIVATE_SOURCE_which_must_not_leak");
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, AUTHOR, copy(&req)));
    let id = object_id(&saved.object);
    assert!(
        objects::get(s.conn(), FOREIGN, id).is_ok(),
        "organization object remains readable"
    );
    let restricted = sources::source(s.conn(), FOREIGN, id).unwrap();
    assert_eq!(
        serde_json::to_value(restricted).unwrap(),
        json!({"status":"restricted"})
    );
    assert!(sources::objects_from_message(s.conn(), FOREIGN, &m.id, None).is_err());
    let foreign_req = request(&s, "crew-b", &m, decision("crew-b"));
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| sources::record(tx, FOREIGN, foreign_req)).is_err());
    assert_eq!(database_snapshot(&s), before);
    s.conn()
        .execute(
            "UPDATE channel_members SET left_at=?1 WHERE channel_id=?2 AND member_id='w-a1'",
            rusqlite::params![crewd_store::ts_from_ms(NOW), dm.id],
        )
        .unwrap();
    assert!(!crate::channels::readable_by(s.conn(), &dm.id, "w-a1").unwrap());
    let before = database_snapshot(&s);
    assert_eq!(
        sources::source(s.conn(), AUTHOR, id).unwrap(),
        Source::Restricted
    );
    assert!(sources::lookup(s.conn(), AUTHOR, &req.scope, "crew-a", &req.request_id).is_err());
    assert!(sources::objects_from_message(s.conn(), AUTHOR, &m.id, None).is_err());
    assert!(crate::run(&mut s, NOW, |tx| sources::record(tx, AUTHOR, copy(&req))).is_err());
    assert_eq!(
        database_snapshot(&s),
        before,
        "even exact replay cannot bypass current source authority"
    );
    assert!(
        matches!(
            sources::lookup(s.conn(), HUMAN, &req.scope, "crew-a", &req.request_id).unwrap(),
            Lookup::NotFound
        ),
        "a distinct Human principal must not inherit the session's original UUID"
    );
    assert!(matches!(
        sources::source(s.conn(), HUMAN, id).unwrap(),
        Source::Available { .. }
    ));
}

#[test]
fn source_delete_retains_only_filtered_snapshot_and_original_human_receipt() {
    let mut s = fixture();
    let m = post(&mut s, "crew-a/lobby", "Captured before source deletion");
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, HUMAN, copy(&req)));
    let id = object_id(&saved.object);
    s.conn()
        .execute("DELETE FROM messages WHERE id=?1", [&m.id])
        .unwrap();
    let before = database_snapshot(&s);
    assert_eq!(
        sources::source(s.conn(), HUMAN, id).unwrap(),
        Source::Deleted {
            excerpt: m.text.clone()
        }
    );
    assert_eq!(
        sources::source(s.conn(), AUTHOR, id).unwrap(),
        Source::Deleted {
            excerpt: m.text.clone()
        }
    );
    assert_eq!(known(s.conn(), HUMAN, &req), saved);
    assert!(sources::objects_from_message(s.conn(), HUMAN, &m.id, None).is_err());
    assert!(crate::run(&mut s, NOW, |tx| sources::record(tx, HUMAN, copy(&req))).is_err());
    assert_eq!(
        sources::source(s.conn(), FOREIGN, id).unwrap(),
        Source::Restricted
    );
    assert_eq!(database_snapshot(&s), before);
    s.conn()
        .execute("DELETE FROM channels WHERE id=?1", [&m.channel_id])
        .unwrap();
    let before = database_snapshot(&s);
    assert_eq!(
        sources::source(s.conn(), AUTHOR, id).unwrap(),
        Source::Restricted
    );
    assert_eq!(
        sources::source(s.conn(), HUMAN, id).unwrap(),
        Source::Deleted { excerpt: m.text }
    );
    assert_eq!(known(s.conn(), HUMAN, &req), saved);
    assert_eq!(database_snapshot(&s), before);
}

#[test]
fn global_dedupe_keeps_original_owner_and_purge_clears_request_and_object_receipts_separately() {
    let mut s = fixture();
    let m = post(&mut s, "crew-x/lobby", "Cross-crew original source");
    let initial = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, HUMAN, copy(&initial)));
    let duplicate = request(&s, "crew-x", &m, decision("crew-x"));
    let (other_scope, facts) = ok_at(&mut s, NOW, |tx| {
        sources::record(tx, HUMAN, copy(&duplicate))
    });
    assert_eq!(other_scope.crew_id, "crew-x");
    assert_eq!(other_scope.object.crew_id(), "crew-a");
    assert!(!other_scope.created);
    assert_eq!(other_scope.object, saved.object);
    assert!(facts.is_empty());
    assert_eq!(known(s.conn(), HUMAN, &duplicate), other_scope);
    let surviving_request = request(&s, "crew-b", &m, decision("crew-b"));
    let (surviving_receipt, facts) = ok_at(&mut s, NOW, |tx| {
        sources::record(tx, HUMAN, copy(&surviving_request))
    });
    assert!(!surviving_receipt.created);
    assert_eq!(surviving_receipt.object, saved.object);
    assert!(facts.is_empty());
    assert_eq!(
        sources::source(s.conn(), HUMAN, object_id(&saved.object)).unwrap(),
        Source::Available {
            crew_id: "crew-x".into(),
            message_id: m.id.clone(),
            channel_id: m.channel_id.clone(),
            excerpt: m.text.clone(),
        },
        "navigation follows the actual source channel crew, not either receipt or object owner"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_message_receipts"),
        3
    );
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_message_receipts"),
        2
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM managed_message_receipts WHERE crew_id='crew-x'"
        ),
        0
    );
    assert_eq!(known(s.conn(), HUMAN, &initial), saved);
    assert_eq!(
        known(s.conn(), HUMAN, &surviving_request),
        surviving_receipt
    );
    assert_eq!(
        sources::source(s.conn(), HUMAN, object_id(&saved.object)).unwrap(),
        Source::Deleted { excerpt: m.text }
    );
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
    assert!(
        crate::crews::get(s.conn(), "crew-b").is_ok(),
        "requesting crew survives object owner's purge"
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM managed_message_receipts WHERE crew_id='crew-b'"
        ),
        0,
        "object FK must clear the still-existing other crew's receipt"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_message_receipts"),
        0
    );
    assert_eq!(count(&s, "SELECT count(*) FROM managed_message_sources"), 0);
}

#[test]
fn snapshot_or_source_receipt_insert_abort_rolls_back_number_object_receipts_and_progress() {
    for table in ["managed_message_sources", "managed_message_receipts"] {
        let mut s = fixture();
        let m = post(&mut s, "crew-a/lobby", "Rollback at exact source write");
        let req = request(&s, "crew-a", &m, decision("crew-a"));
        s.conn().execute_batch(&format!("CREATE TRIGGER fail_source BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic source write fault'); END")).unwrap();
        let before = database_snapshot(&s);
        let failure =
            crate::run(&mut s, NOW, |tx| sources::record(tx, AUTHOR, copy(&req))).unwrap_err();
        assert!(format!("{failure:?}").contains("synthetic source write fault"));
        assert_eq!(
            database_snapshot(&s),
            before,
            "{table}: no partial object, alias, source, normal/source receipt or session progress"
        );
        s.conn().execute_batch("DROP TRIGGER fail_source").unwrap();
        let (saved, facts) = ok_at(&mut s, NOW, |tx| sources::record(tx, AUTHOR, copy(&req)));
        assert!(saved.created);
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(fact, Fact::OntologyChanged { .. }))
                .count(),
            1
        );
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(
                    fact,
                    Fact::Progress {
                        kind: ProgressKind::Ontology,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(known(s.conn(), AUTHOR, &req), saved);
    }
}

#[test]
fn source_and_normal_request_namespaces_do_not_retroactively_capture_or_duplicate() {
    let mut s = fixture();
    let m = post(&mut s, "crew-a/lobby", "Source namespace fixture");
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let normal = Submit {
        scope: req.scope.clone(),
        crew_id: req.crew_id.clone(),
        request_id: req.request_id.clone(),
        change: req.change.clone(),
    };
    let normal_saved = ok(&mut s, |tx| objects::submit(tx, HUMAN, normal.clone()));
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| sources::record(tx, HUMAN, copy(&req))).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(
        sources::source(s.conn(), HUMAN, object_id(&normal_saved.object)).unwrap(),
        Source::None
    );
    assert!(matches!(
        sources::lookup(s.conn(), HUMAN, &req.scope, "crew-a", &req.request_id).unwrap(),
        Lookup::NotFound
    ));
    let captured = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, HUMAN, copy(&captured)));
    let compatible_normal = Submit {
        scope: captured.scope.clone(),
        crew_id: captured.crew_id.clone(),
        request_id: captured.request_id.clone(),
        change: captured.change.clone(),
    };
    let before = database_snapshot(&s);
    let (old_shape, facts) = ok_at(&mut s, NOW, |tx| {
        objects::submit(tx, HUMAN, compatible_normal)
    });
    assert_eq!(old_shape.object, saved.object);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
}

#[test]
fn bounded_message_backlinks_preserve_every_exact_identity_and_read_without_effects() {
    let mut s = scoped_fixture(crate::testkit::v31_store());
    let tables = (0..18).map(|_| table(&mut s, "crew-a")).collect::<Vec<_>>();
    crewd_store::schema::migrate(s.conn()).unwrap();
    assert!(crewd_store::managed_tables_v3::enabled(s.conn()).unwrap());
    let m = post(&mut s, "crew-a/lobby", "Bounded backlinks");
    let mut expected = vec![];
    for (n, table_id) in tables.into_iter().enumerate() {
        let req = request(
            &s,
            "crew-a",
            &m,
            Change::AddRow {
                table_id,
                values: vec![Cell::Text(format!("row-{n}"))],
            },
        );
        expected
            .push(object_id(&ok(&mut s, |tx| sources::record(tx, HUMAN, req)).object).to_string());
    }
    let before = database_snapshot(&s);
    let first = sources::objects_from_message(s.conn(), AUTHOR, &m.id, None).unwrap();
    assert_eq!(first.items.len(), 16);
    let cursor = first.next_after.as_deref().unwrap();
    assert_eq!(cursor, &expected[15][1..]);
    let second = sources::objects_from_message(s.conn(), AUTHOR, &m.id, Some(cursor)).unwrap();
    assert_eq!(second.items.len(), 2);
    assert!(second.next_after.is_none());
    let actual = first
        .items
        .iter()
        .chain(&second.items)
        .map(object_id)
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for bad in ["0", "01", "-1", "O1", "9223372036854775808"] {
        assert!(sources::objects_from_message(s.conn(), HUMAN, &m.id, Some(bad)).is_err());
    }
    assert!(sources::objects_from_message(s.conn(), FOREIGN, &m.id, None).is_err());
    assert_eq!(
        database_snapshot(&s),
        before,
        "backlinks, pagination and refusals never enqueue or modify anything"
    );
}

fn disk_fixture(data: &std::path::Path) -> Store {
    let s = Store::open_data_dir(data).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "crew-a".into(),
            title: "Synthetic persisted crew".into(),
            number: Some(1),
            created_at: crewd_store::ts_from_ms(NOW),
            updated_at: crewd_store::ts_from_ms(NOW),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_store::schema::backfill_v4(s.conn()).unwrap();
    s
}

#[test]
fn physical_reopen_and_official_restore_preserve_original_receipts_without_reexecution() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap();
    let snapshot = data.join("retained-source.db");
    let mut s = disk_fixture(&data);
    let m = post(&mut s, "crew-a/lobby", "Persisted captured source");
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, HUMAN, copy(&req)));
    crewd_store::meta_set(s.conn(), "knowledge.sqlite.v1", "1").unwrap();
    crewd_store::team_data::private_dir(&data.join("knowledge")).unwrap();
    drop(s);
    crewd_store::backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &snapshot).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let before = database_snapshot(&s);
    assert_eq!(known(s.conn(), HUMAN, &req), saved);
    let (replayed, facts) = ok_at(&mut s, NOW + 1, |tx| sources::record(tx, HUMAN, copy(&req)));
    assert_eq!(replayed, saved);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let later_message = post(&mut s, "crew-a/lobby", "Not retained by old snapshot");
    let later = request(&s, "crew-a", &later_message, decision("crew-a"));
    ok(&mut s, |tx| sources::record(tx, HUMAN, copy(&later)));
    drop(s);
    crewd_store::backup::restore(&data, &snapshot).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    assert_ne!(
        objects::scope(s.conn()).unwrap().restore_epoch,
        req.scope.restore_epoch
    );
    let before = database_snapshot(&s);
    match sources::lookup(s.conn(), HUMAN, &req.scope, "crew-a", &req.request_id).unwrap() {
        Lookup::Historical {
            receipt: Some(receipt),
        } => assert_eq!(receipt, saved),
        other => panic!("retained receipt must be historical: {other:?}"),
    }
    assert!(matches!(
        sources::lookup(s.conn(), HUMAN, &later.scope, "crew-a", &later.request_id).unwrap(),
        Lookup::Historical { receipt: None }
    ));
    assert!(crate::run(&mut s, NOW + 2, |tx| sources::record(tx, HUMAN, copy(&req))).is_err());
    assert_eq!(database_snapshot(&s), before);
}

#[test]
fn protected_v26_migration_retains_existing_objects_and_original_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap();
    let mut s = crewd_store::testing::open_legacy_store(26).unwrap();
    s.conn()
        .execute_batch(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew-a','A','t','t');",
        )
        .unwrap();
    let scope = objects::scope(s.conn()).unwrap();
    let prior = ok(&mut s, |tx| {
        objects::submit(
            tx,
            HUMAN,
            Submit {
                scope,
                crew_id: "crew-a".into(),
                request_id: uuid::Uuid::new_v4().to_string(),
                change: decision("crew-a"),
            },
        )
    });
    let prior_number: i64 = object_id(&prior.object)[1..].parse().unwrap();
    s.conn()
        .execute(
            "VACUUM INTO ?1",
            [data.join(crewd_store::DB_FILE).to_str().unwrap()],
        )
        .unwrap();
    drop(s);
    let s = Store::open_data_dir(&data).unwrap();
    assert_eq!(
        s.conn()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        crewd_store::schema::SCHEMA_VERSION
    );
    assert_eq!(
        objects::get(s.conn(), HUMAN, object_id(&prior.object)).unwrap(),
        prior.object
    );
    assert_eq!(
        sources::source(s.conn(), HUMAN, object_id(&prior.object)).unwrap(),
        Source::None
    );
    let backups = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!(
                    "crewd-v26-pre-v{}-",
                    crewd_store::schema::SCHEMA_VERSION
                ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        backups.len(),
        1,
        "protected migration requires one original-version backup"
    );
    let old = rusqlite::Connection::open_with_flags(
        &backups[0],
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        26
    );
    assert_eq!(old.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('managed_message_sources','managed_message_receipts')", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    let Object::Decision { decision } = &prior.object else {
        panic!("real pre27 decision")
    };
    assert_eq!(
        crewd_store::managed_decisions::get(&old, prior_number)
            .unwrap()
            .as_ref(),
        Some(decision)
    );
    drop(old);
    drop(s);
    let reopened = Store::open_data_dir(&data).unwrap();
    assert_eq!(
        objects::get(reopened.conn(), HUMAN, object_id(&prior.object)).unwrap(),
        prior.object
    );
    assert_eq!(
        std::fs::read_dir(data.join("backups"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(
                |entry| entry.file_name().to_string_lossy().starts_with(&format!(
                    "crewd-v26-pre-v{}-",
                    crewd_store::schema::SCHEMA_VERSION
                ))
            )
            .count(),
        1,
        "ordinary reopen must not migrate again"
    );
}

#[test]
fn missing_source_snapshot_cannot_be_presented_as_a_known_capture_on_replay_or_lookup() {
    let mut s = fixture();
    let m = post(&mut s, "crew-a/lobby", "Original witnessed capture");
    let req = request(&s, "crew-a", &m, decision("crew-a"));
    let saved = ok(&mut s, |tx| sources::record(tx, AUTHOR, copy(&req)));
    assert!(saved.created);
    assert_eq!(known(s.conn(), AUTHOR, &req), saved);
    let number: i64 = object_id(&saved.object)[1..].parse().unwrap();
    // Damage only optional source evidence: retain the actual object and both original receipts.
    assert_eq!(
        s.conn()
            .execute(
                "DELETE FROM managed_message_sources WHERE number=?1",
                [number]
            )
            .unwrap(),
        1
    );
    assert_eq!(
        objects::get(s.conn(), AUTHOR, object_id(&saved.object)).unwrap(),
        saved.object
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_message_receipts"),
        1
    );
    let before = database_snapshot(&s);
    assert!(
        sources::lookup(s.conn(), AUTHOR, &req.scope, "crew-a", &req.request_id).is_err(),
        "no receipt can assert a captured source after the matching source evidence was lost"
    );
    assert!(
        crate::run(&mut s, NOW + 1, |tx| sources::record(
            tx,
            AUTHOR,
            copy(&req)
        ))
        .is_err(),
        "exact replay must obey the same evidence check as read-only lookup"
    );
    assert_eq!(
        database_snapshot(&s),
        before,
        "refusal must never recreate evidence, objects, progress, wakes or receipts"
    );
}
