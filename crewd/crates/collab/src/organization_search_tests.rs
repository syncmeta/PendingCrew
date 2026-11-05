use crate::{
    Sender,
    managed_objects::{self, Actor},
    organization_search as search,
    testkit::{NOW, count, ok, ok_at},
};
use crewd_proto::{
    WorkCreateParams, WorkUpdateParams, WorkUpdateStatus, managed_tables_v2 as v2,
    organization_search::*,
};
use crewd_store::{Store, organization_search as index};
use serde_json::{Value, json};
const HUMAN: Actor<'static> = Actor::Human("search-fixture");

fn clock(now: i64) -> search::ReadClock {
    search::ReadClock {
        wall_ms: now,
        elapsed_ms: now.saturating_sub(NOW).max(0) as u64,
        boot_id: "synthetic-os-boot-a".into(),
    }
}
fn run_search(c: &rusqlite::Connection, a: Actor<'_>, q: Search, now: i64) -> crate::Result<Page> {
    crate::organization_search::search(c, a, q, clock(now))
}
fn run_expand(
    c: &rusqlite::Connection,
    a: Actor<'_>,
    q: Expand,
    now: i64,
) -> crate::Result<ExpansionPage> {
    crate::organization_search::expand(c, a, q, clock(now))
}
fn ready(s: &mut Store) {
    for _ in 0..4096 {
        if !search::maintenance_needed(s.conn(), NOW).unwrap() {
            return;
        }
        let (_, changes) = crate::run(s, NOW, |tx| search::maintenance_step(tx, 64)).unwrap();
        assert!(
            changes.is_empty(),
            "index maintenance must not emit business changes"
        );
    }
    panic!("bounded index did not complete")
}
fn fixture() -> Store {
    let mut s = crate::testkit::store();
    ready(&mut s);
    s
}
fn query(q: &str) -> Search {
    serde_json::from_value(json!({"query":q})).unwrap()
}
fn qpage(s: &Store, actor: Actor<'_>, q: Search) -> Page {
    run_search(s.conn(), actor, q, NOW + 100_000).unwrap()
}
fn post(s: &mut Store, at: i64, channel: &str, text: &str) -> String {
    ok_at(s, at, |tx| {
        crate::messages::post(
            tx,
            Sender::Human,
            crate::messages::NewPost {
                channel_id: channel.into(),
                text: text.into(),
                ..Default::default()
            },
        )
    })
    .0
    .message
    .id
}
fn request(s: &Store, crew: &str, operation: Value) -> v2::Submit {
    v2::Submit {
        scope: managed_objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: serde_json::from_value(operation).unwrap(),
    }
}
fn table(s: &mut Store) -> String {
    let r = request(
        s,
        "crew-a",
        json!({"kind":"createTable","title":"Source table","purpose":"Synthetic searchable table","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false},{"name":"Private old label","kind":{"kind":"text"},"nullable":true}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let result = ok(s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    let v2::Effect::TableCreated { table_id, .. } = result.effect else {
        panic!("table")
    };
    table_id
}
fn row(s: &mut Store, t: &str, at: i64, title: &str, body: &str) -> String {
    let r = request(
        s,
        "crew-a",
        json!({"kind":"addRow","tableId":t,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":title}}},{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"text","value":body}}}]}),
    );
    let result = ok_at(s, at, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    let v2::Effect::RowAdded { row_id, .. } = result.0.effect else {
        panic!("row")
    };
    row_id
}
fn edit(s: &mut Store, row: &str, text: &str, at: i64) {
    let t = crewd_store::managed_tables_v2::row_base(s.conn(), row[1..].parse().unwrap())
        .unwrap()
        .unwrap();
    let r = request(
        s,
        "crew-a",
        json!({"kind":"patchRow","rowId":row,"expectedSchemaRevision":t.0.schema_revision,"expectedRowRevision":t.1.to_string(),"patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":text}}}]}),
    );
    ok_at(s, at, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
}
fn rows_query(t: &str) -> Search {
    let mut q = query("needle");
    q.table_id = Some(t.into());
    q.limit = 1;
    q
}

#[test]
fn actual_messages_short_chinese_quotes_percent_and_raw_unicode_are_literal() {
    let mut s = fixture();
    let a = post(
        &mut s,
        NOW,
        "crew-a/lobby",
        "\u{98ce}\u{9669}\u{767b}\u{8bb0} 100% \"quoted\" café",
    );
    let b = post(&mut s, NOW + 1, "crew-a/lobby", "100X cafe\u{301}");
    ready(&mut s);
    for term in [
        "\u{98ce}",
        "\u{98ce}\u{9669}",
        "\u{98ce}\u{9669}\u{767b}",
        "100%",
        "\"quoted\"",
        "café",
    ] {
        let p = qpage(&s, HUMAN, query(term));
        assert_eq!(p.index_state, IndexState::Ready);
        assert_eq!(p.items.len(), 1, "{term}");
        assert_eq!(p.items[0].target, v2::TargetRef::Message { id: a.clone() });
    }
    let p = qpage(&s, HUMAN, query("cafe\u{301}"));
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].target, v2::TargetRef::Message { id: b });
}

#[test]
fn cut_excludes_new_sources_and_old_nonmatching_source_becoming_a_match() {
    let mut s = fixture();
    let t = table(&mut s);
    let tail = row(&mut s, &t, NOW, "needle tail", "");
    let nonmatch = row(&mut s, &t, NOW + 10, "unrelated", "");
    let head = row(&mut s, &t, NOW + 20, "needle head", "");
    ready(&mut s);
    let first = qpage(&s, HUMAN, rows_query(&t));
    assert_eq!(first.items[0].target, v2::TargetRef::Row { id: head });
    let original = first.snapshot.clone();
    let token = first.next_cursor.clone().expect("retained tail");
    edit(&mut s, &nonmatch, "needle newly matching", NOW + 30);
    row(&mut s, &t, NOW + 40, "needle new source", "");
    ready(&mut s);
    let mut q = rows_query(&t);
    q.cursor = Some(token);
    let page = qpage(&s, HUMAN, q);
    assert_eq!(page.snapshot, original);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].target, v2::TargetRef::Row { id: tail });
}

#[test]
fn ordinary_unrelated_message_writes_do_not_expire_a_valid_cursor() {
    let mut s = fixture();
    let t = table(&mut s);
    let tail = row(&mut s, &t, NOW, "needle tail", "");
    row(&mut s, &t, NOW + 1, "needle head", "");
    ready(&mut s);
    let first = qpage(&s, HUMAN, rows_query(&t));
    for i in 0..20 {
        post(
            &mut s,
            NOW + 2 + i,
            "crew-a/lobby",
            "unrelated live traffic",
        );
    }
    ready(&mut s);
    let mut q = rows_query(&t);
    q.cursor = first.next_cursor;
    let p = qpage(&s, HUMAN, q);
    assert_eq!(p.snapshot, first.snapshot);
    assert_eq!(p.items[0].target, v2::TargetRef::Row { id: tail });
}

#[test]
fn hidden_field_is_not_searchable_and_historical_schema_names_only_find_table() {
    let mut s = fixture();
    let t = table(&mut s);
    let o = row(&mut s, &t, NOW, "Visible row", "hiddenneedle");
    ready(&mut s);
    assert!(
        qpage(&s, HUMAN, query("hiddenneedle"))
            .items
            .iter()
            .any(|h| h.target == v2::TargetRef::Row { id: o.clone() })
    );
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":t,"expectedSchemaRevision":"1","change":{"kind":"renameField","fieldId":"p2","name":"New name"}}),
    );
    ok(&mut s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    ready(&mut s);
    let p = qpage(&s, HUMAN, query("Private old label"));
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].target, v2::TargetRef::Table { id: t.clone() });
    assert_eq!(p.items[0].matched_on, MatchOn::HistoricalName);
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":t,"expectedSchemaRevision":"2","change":{"kind":"hideField","fieldId":"p2"}}),
    );
    ok(&mut s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    ready(&mut s);
    assert!(qpage(&s, HUMAN, query("hiddenneedle")).items.is_empty());
}

#[test]
fn candidate_sql_filters_foreign_messages_and_asks_before_budget() {
    let mut s = fixture();
    for i in 0..140 {
        post(&mut s, NOW + i, "crew-x/lobby", "private needle");
    }
    let visible = post(&mut s, NOW - 1, "crew-a/lobby", "visible needle");
    ready(&mut s);
    let p = qpage(&s, Actor::Session("w-a1"), query("needle"));
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].target, v2::TargetRef::Message { id: visible });
    assert!(p.next_cursor.is_none());
    let ask = ok(&mut s, |tx| {
        crate::asks::open(
            tx,
            "cap-x",
            crate::asks::OpenAsk {
                question: "private ask needle".into(),
                default: None,
                resume_note: None,
            },
        )
    });
    ready(&mut s);
    let mut q = query("private ask needle");
    q.kinds = Some(vec![Kind::Ask]);
    assert!(
        qpage(&s, Actor::Session("w-a1"), q.clone())
            .items
            .is_empty()
    );
    assert_eq!(
        qpage(&s, HUMAN, q).items[0].target,
        v2::TargetRef::Ask { id: ask.ask }
    );
}

#[test]
fn real_work_create_report_and_done_reach_search_via_shared_save_without_extra_effects() {
    let mut s = fixture();
    let w = ok(&mut s, |tx| {
        crate::work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-a".into(),
                title: "workneedle".into(),
                owner: Some("w-a1".into()),
                ..Default::default()
            },
        )
    });
    ready(&mut s);
    let p = qpage(&s, HUMAN, query("workneedle"));
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].status, Status::Current);
    let reported = ok_at(&mut s, NOW + 10, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a1".into()),
            WorkUpdateParams {
                id: w.id.clone(),
                expected_revision: w.revision.clone(),
                note: Some("reportneedle".into()),
                ..Default::default()
            },
        )
    });
    ready(&mut s);
    assert_eq!(
        qpage(&s, HUMAN, query("reportneedle")).items[0].target,
        v2::TargetRef::Work { id: w.id.clone() }
    );
    ok_at(&mut s, NOW + 20, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a1".into()),
            WorkUpdateParams {
                id: w.id.clone(),
                expected_revision: reported.0.revision,
                status: Some(WorkUpdateStatus::Done),
                ..Default::default()
            },
        )
    });
    ready(&mut s);
    let p = qpage(&s, HUMAN, query("workneedle"));
    assert_eq!(p.items[0].status, Status::Historical);
    let before = (
        count(&s, "SELECT count(*) FROM messages"),
        count(&s, "SELECT count(*) FROM wakes"),
        count(&s, "SELECT count(*) FROM organization_work_events"),
    );
    let _ = qpage(&s, HUMAN, query("workneedle"));
    assert_eq!(
        before,
        (
            count(&s, "SELECT count(*) FROM messages"),
            count(&s, "SELECT count(*) FROM wakes"),
            count(&s, "SELECT count(*) FROM organization_work_events")
        )
    );
}

#[test]
fn index_failure_rolls_back_real_object_alias_receipt_and_notification_changes() {
    let mut s = fixture();
    let before = (
        count(&s, "SELECT count(*) FROM object_refs"),
        count(&s, "SELECT count(*) FROM managed_tables"),
        count(&s, "SELECT count(*) FROM managed_v2_receipts"),
    );
    s.conn().execute_batch("CREATE TRIGGER fail_search_version BEFORE INSERT ON organization_search_versions BEGIN SELECT RAISE(ABORT,'synthetic index failure'); END;").unwrap();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Rollback table","purpose":"Atomic rollback fixture","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let result = crate::run(&mut s, NOW, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    assert!(result.is_err());
    assert_eq!(
        before,
        (
            count(&s, "SELECT count(*) FROM object_refs"),
            count(&s, "SELECT count(*) FROM managed_tables"),
            count(&s, "SELECT count(*) FROM managed_v2_receipts")
        )
    );
}

#[test]
fn cursor_scope_principal_query_expiry_and_retained_floor_are_exact() {
    let mut s = fixture();
    let t = table(&mut s);
    let o = row(&mut s, &t, NOW, "needle tail", "");
    row(&mut s, &t, NOW + 1, "needle head", "");
    ready(&mut s);
    let first = qpage(&s, HUMAN, rows_query(&t));
    let token = first.next_cursor.unwrap();
    let mut q = rows_query(&t);
    q.cursor = Some(token.clone());
    assert!(
        matches!(run_search(s.conn(),Actor::Human("other-principal"),q.clone(),NOW+100_001),Err(crate::CollabError::Conflict(r))if r.code==crate::refusal::Code::SearchCursorExpired)
    );
    q.query = "different".into();
    assert!(run_search(s.conn(), HUMAN, q, NOW + 100_001).is_err());
    let mut q = rows_query(&t);
    q.cursor = Some(token.clone());
    assert!(
        run_search(
            s.conn(),
            HUMAN,
            q.clone(),
            NOW + 100_000 + CURSOR_LIFETIME_MS
        )
        .is_err()
    );
    for i in 0..70 {
        edit(&mut s, &o, &format!("needle changed {i}"), NOW + 200 + i);
    }
    ready(&mut s);
    assert!(
        matches!(run_search(s.conn(),HUMAN,q,NOW+100_001),Err(crate::CollabError::Conflict(r))if r.code==crate::refusal::Code::SearchCursorExpired)
    );
}

#[test]
fn building_and_incompatible_filters_are_not_empty_complete_searches() {
    let mut s = fixture();
    ok(&mut s, |tx| {
        index::mark(tx.conn(), "human")?;
        tx.organization_search_budget = 0;
        Ok(())
    });
    let p = qpage(&s, HUMAN, query("absent"));
    assert_eq!(p.index_state, IndexState::Building);
    assert!(p.snapshot.is_none() && p.items.is_empty() && p.next_cursor.is_none());
    let mut q = query("needle");
    q.r#in = Domain::Objects;
    q.kinds = Some(vec![Kind::Message]);
    assert!(run_search(s.conn(), HUMAN, q, NOW).is_err());
    let expand = Expand {
        scope: None,
        target: v2::TargetRef::Human {},
        cursor: None,
        limit: 0,
        include_system_defaults: false,
    };
    assert!(run_expand(s.conn(), HUMAN, expand, NOW).is_err());
}

#[test]
fn noop_original_replay_does_not_advance_unrelated_raw_backlog_or_change_any_table() {
    let mut s = fixture();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Original table","purpose":"Original synthetic receipt","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let original = ok(&mut s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r.clone(), &crate::managed_links::DenyExternal)
    });
    ready(&mut s);
    let id = post(&mut s, NOW, "crew-a/lobby", "source body");
    ready(&mut s);
    let mut raw = crewd_store::messages::get(s.conn(), &id).unwrap().unwrap();
    raw.id = uuid::Uuid::new_v4().to_string();
    raw.text = "raw pending needle".into();
    crewd_store::messages::insert(s.conn(), &raw, None).unwrap();
    assert!(search::maintenance_needed(s.conn(), NOW).unwrap());
    let before = crate::testkit::database_snapshot(&s);
    let (replayed, changes) = crate::run(&mut s, NOW + 1, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    })
    .unwrap();
    assert_eq!(original, replayed);
    assert!(changes.is_empty());
    assert_eq!(crate::testkit::database_snapshot(&s), before);
    ready(&mut s);
    assert_eq!(qpage(&s, HUMAN, query("raw pending needle")).items.len(), 1);
}

#[test]
fn elapsed_boot_clock_not_wall_time_controls_cursor_across_a_real_file_reopen() {
    let mut s = fixture();
    let t = table(&mut s);
    row(&mut s, &t, NOW, "needle tail", "");
    row(&mut s, &t, NOW + 1, "needle head", "");
    ready(&mut s);
    let initial = search::ReadClock {
        wall_ms: NOW + 100_000,
        elapsed_ms: 1_000,
        boot_id: "synthetic-os-boot-a".into(),
    };
    let first =
        crate::organization_search::search(s.conn(), HUMAN, rows_query(&t), initial.clone())
            .unwrap();
    let token = first.next_cursor.unwrap();
    let mut q = rows_query(&t);
    q.cursor = Some(token);
    let wall_jump = search::ReadClock {
        wall_ms: NOW + 100_000 + CURSOR_LIFETIME_MS * 4,
        elapsed_ms: 1_001,
        boot_id: initial.boot_id.clone(),
    };
    let allowed =
        crate::organization_search::search(s.conn(), HUMAN, q.clone(), wall_jump).unwrap();
    assert_eq!(allowed.snapshot, first.snapshot);
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("retained.sqlite");
    s.conn()
        .execute("VACUUM INTO ?1", [file.to_str().unwrap()])
        .unwrap();
    drop(s);
    let reopened = Store::open(&file).unwrap();
    let same = search::ReadClock {
        wall_ms: NOW - 1_000_000,
        elapsed_ms: 1_002,
        boot_id: initial.boot_id.clone(),
    };
    assert!(crate::organization_search::search(reopened.conn(), HUMAN, q.clone(), same).is_ok());
    let expired_clock = search::ReadClock {
        wall_ms: NOW + 100_001,
        elapsed_ms: 1_000 + CURSOR_LIFETIME_MS as u64,
        boot_id: initial.boot_id.clone(),
    };
    assert!(
        crate::organization_search::search(
            reopened.conn(),
            HUMAN,
            q.clone(),
            expired_clock.clone()
        )
        .is_err()
    );
    let rollback = search::ReadClock {
        wall_ms: NOW,
        elapsed_ms: expired_clock.elapsed_ms + 1,
        boot_id: initial.boot_id,
    };
    assert!(
        crate::organization_search::search(reopened.conn(), HUMAN, q.clone(), rollback).is_err()
    );
    let reboot = search::ReadClock {
        wall_ms: NOW + 100_002,
        elapsed_ms: 1_003,
        boot_id: "synthetic-os-boot-b".into(),
    };
    assert!(crate::organization_search::search(reopened.conn(), HUMAN, q, reboot).is_err());
}

#[path = "organization_search_quality_tests.rs"]
mod independent_quality;

#[path = "organization_search_recency_tests.rs"]
mod recency;

#[path = "organization_search_purpose_tests.rs"]
mod purpose;

#[path = "organization_search_scope_tests.rs"]
mod canonical_scope;
