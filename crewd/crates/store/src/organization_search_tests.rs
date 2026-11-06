use crate::{Store, organization_search as search};
use crewd_proto::{
    managed_tables_v2::TargetRef,
    organization_search::{MatchOn, Status},
};
use rusqlite::params;

fn fixture() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.conn()
        .execute(
            "UPDATE organization_search_state SET phase=8,build_cut=NULL",
            [],
        )
        .unwrap();
    s
}
fn document(title: &str) -> search::Document {
    search::Document {
        revision: None,
        target: TargetRef::Human {},
        kind: None,
        owner_crew: None,
        table_id: None,
        channel_id: None,
        title: title.into(),
        changed_at: Some("2026-11-06T00:00:00.000Z".into()),
        status: Status::Current,
        system_default: false,
        slots: vec![search::Slot {
            field: String::new(),
            matched_on: MatchOn::Title,
            text: title.into(),
        }],
        selection_labels: Vec::new(),
    }
}
fn matches(c: &rusqlite::Connection, query: &str) -> i64 {
    c.query_row(
        "SELECT count(*) FROM organization_search_fts WHERE organization_search_fts MATCH ?1",
        [query],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn retained_cut_keeps_original_order_version_while_current_fts_is_distinct() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    assert!(
        search::put(
            &tx,
            "human",
            search::Binding::default(),
            &document("first original")
        )
        .unwrap()
    );
    let cut = search::state(&tx).unwrap().clock;
    assert!(
        !search::put(
            &tx,
            "human",
            search::Binding::default(),
            &document("first original")
        )
        .unwrap()
    );
    assert_eq!(search::state(&tx).unwrap().clock, cut);
    search::put(
        &tx,
        "human",
        search::Binding::default(),
        &document("second current"),
    )
    .unwrap();
    let old = search::candidates(&tx, cut, None, 128, "1", vec![]).unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].document.title, "first original");
    assert_eq!(
        search::current(&tx, "human").unwrap().unwrap().title,
        "second current"
    );
    assert_eq!(matches(&tx, "original"), 1);
    assert_eq!(matches(&tx, "current"), 1);
    tx.commit().unwrap();
}

#[test]
fn projection_slots_clock_and_parent_rollback_together() {
    let s = fixture();
    let c = s.conn();
    let initial = search::state(c).unwrap().clock;
    {
        let tx = c.unchecked_transaction().unwrap();
        search::put(
            &tx,
            "human",
            search::Binding::default(),
            &document("rollback needle"),
        )
        .unwrap();
        assert_eq!(matches(&tx, "needle"), 1);
    }
    assert_eq!(search::state(c).unwrap().clock, initial);
    assert!(search::current(c, "human").unwrap().is_none());
    assert_eq!(matches(c, "needle"), 0);
}

#[test]
fn source_history_cap_moves_private_cut_floor_and_keeps_current() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    for i in 0..70 {
        search::put(
            &tx,
            "human",
            search::Binding::default(),
            &document(&format!("version {i}")),
        )
        .unwrap();
    }
    for _ in 0..256 {
        if !search::gc_pending(&tx).unwrap() {
            break;
        }
        search::gc_step(&tx, 64).unwrap();
    }
    let closed: i64 = tx
        .query_row(
            "SELECT count(*) FROM organization_search_versions WHERE valid_to IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(closed <= 64);
    let state = search::state(&tx).unwrap();
    assert!(state.min_cut > 1);
    assert_eq!(
        search::current(&tx, "human").unwrap().unwrap().title,
        "version 69"
    );
    let actual: i64 = tx
        .query_row(
            "SELECT closed_count FROM organization_search_state",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(actual, closed);
    search::remove(&tx, "human").unwrap();
    let remaining: i64 = tx
        .query_row(
            "SELECT count(*) FROM organization_search_versions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(matches(&tx, "version"), 0);
    tx.commit().unwrap();
}

#[test]
fn integer_clock_exhaustion_refuses_without_promoting_real_or_partial_document() {
    let s = fixture();
    let c = s.conn();
    c.execute("UPDATE organization_search_state SET clock=?1", [i64::MAX])
        .unwrap();
    {
        let tx = c.unchecked_transaction().unwrap();
        assert!(
            search::put(
                &tx,
                "human",
                search::Binding::default(),
                &document("cannot issue")
            )
            .is_err()
        );
    }
    assert_eq!(search::state(c).unwrap().clock, i64::MAX);
    assert!(search::current(c, "human").unwrap().is_none());
    let ty: String = c
        .query_row(
            "SELECT typeof(clock) FROM organization_search_state",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ty, "integer");
}

#[test]
fn dirty_counter_exhaustion_is_checked_by_sqlite_integer_constraint() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    search::mark(&tx, "human").unwrap();
    tx.execute(
        "UPDATE organization_search_dirty SET revision=?1 WHERE source='human'",
        [i64::MAX],
    )
    .unwrap();
    assert!(search::mark(&tx, "human").is_err());
    let ty: String = tx
        .query_row(
            "SELECT typeof(revision) FROM organization_search_dirty WHERE source='human'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ty, "integer");
}

#[test]
fn restoration_changes_only_private_generation_not_original_projection() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    search::put(
        &tx,
        "human",
        search::Binding::default(),
        &document("preserved"),
    )
    .unwrap();
    let before = search::state(&tx).unwrap();
    let original = search::current(&tx, "human").unwrap();
    search::restore(&tx).unwrap();
    let after = search::state(&tx).unwrap();
    assert_ne!(before.generation, after.generation);
    assert_eq!(before.clock, after.clock);
    assert_eq!(original, search::current(&tx, "human").unwrap());
    tx.commit().unwrap();
}

#[test]
fn genuine_thirty_migration_starts_building_without_fake_versions_or_business_effects() {
    let old = crate::testing::open_legacy_store(30).unwrap();
    let c = old.conn();
    assert!(!search::enabled(c).unwrap());
    let before: i64 = c
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    crate::schema::migrate(c).unwrap();
    let actual_version: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(actual_version, crate::schema::SCHEMA_VERSION);
    assert!(search::state(c).unwrap().building);
    let versions: i64 = c
        .query_row(
            "SELECT count(*) FROM organization_search_versions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(versions, 0);
    let after: i64 = c
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn sqlite_trigram_is_literal_superset_and_short_substrings_remain_available() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    let mut doc =
        document("\u{98ce}\u{9669}\u{767b}\u{8bb0} rate 100% \"quoted\" café cafe\u{301}");
    search::put(&tx, "human", search::Binding::default(), &doc).unwrap();
    assert_eq!(matches(&tx, "\"\u{98ce}\u{9669}\""), 0);
    assert_eq!(matches(&tx, "\"\u{98ce}\u{9669}\u{767b}\""), 1);
    assert_eq!(matches(&tx, "\"100%\""), 1);
    let short: i64 = tx
        .query_row(
            "SELECT count(*) FROM organization_search_slots WHERE instr(body,?1)>0",
            ["\u{98ce}"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(short, 1);
    doc.slots[0].text = "new exact body".into();
    doc.title = "new exact body".into();
    search::put(&tx, "human", search::Binding::default(), &doc).unwrap();
    let n:i64=tx.query_row("SELECT count(*) FROM organization_search_slots s JOIN organization_search_versions v ON v.id=s.version_id WHERE v.valid_to IS NULL AND instr(s.body,?1)>0",params!["\u{98ce}"],|r|r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn actual_utf8_history_bytes_and_credit_limited_reclamation_are_separate_from_current() {
    let s = fixture();
    let c = s.conn();
    let tx = c.unchecked_transaction().unwrap();
    let old = document(&"\u{754c}".repeat(100));
    search::put(&tx, "human", search::Binding::default(), &old).unwrap();
    let exact:i64=tx.query_row("SELECT length(CAST(v.document AS BLOB))+(SELECT sum(length(CAST(s.body AS BLOB))) FROM organization_search_slots s WHERE s.version_id=v.id) FROM organization_search_versions v",[],|r|r.get(0)).unwrap();
    search::put(
        &tx,
        "human",
        search::Binding::default(),
        &document("current complete content"),
    )
    .unwrap();
    assert_eq!(search::payload_usage(&tx).unwrap().history_bytes, exact);
    assert!(exact > 600);
    assert_eq!(search::HISTORY_BYTES, 128 * 1024 * 1024);
    super::schedule_gc_with(&tx, None, exact - 1).unwrap();
    assert!(search::state(&tx).unwrap().min_cut > 0);
    let before: i64 = tx
        .query_row("SELECT count(*) FROM organization_search_slots", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(search::gc_step(&tx, 1).unwrap(), 1);
    let after: i64 = tx
        .query_row("SELECT count(*) FROM organization_search_slots", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(before - after, 1);
    assert!(search::gc_pending(&tx).unwrap());
    assert_eq!(search::gc_step(&tx, 1).unwrap(), 1);
    assert!(!search::gc_pending(&tx).unwrap());
    assert_eq!(search::payload_usage(&tx).unwrap().history_bytes, 0);
    assert_eq!(
        search::current(&tx, "human").unwrap().unwrap().title,
        "current complete content"
    );
}

#[test]
fn trusted_context_clears_and_raw_mutation_cannot_reuse_previous_source_time() {
    let s = fixture();
    let c = s.conn();
    let before = c.total_changes();
    {
        let guard = s.search_write_context(1_800_000_000_000).unwrap();
        let tx = c.unchecked_transaction().unwrap();
        assert!(search::span(&tx).unwrap().is_some());
        tx.commit().unwrap();
        drop(guard);
    }
    assert!(search::span(c).unwrap().is_none());
    assert_eq!(c.total_changes(), before);
    {
        let _guard = s.search_write_context(1_800_000_000_001).unwrap();
        let _tx = c.unchecked_transaction().unwrap();
    }
    assert!(search::span(c).unwrap().is_none());
    assert_eq!(c.total_changes(), before);
    // Real source triggers observe only the current trusted owner, then raw SQL
    // invalidates the time rather than reusing the preceding owner's wall clock.
    c.execute("INSERT INTO crews(id,title,created_at,updated_at) VALUES('context-crew','first','2026-11-06T00:00:00.000Z','2026-11-06T00:00:00.000Z')",[]).unwrap();
    let key = "c:context-crew";
    {
        let _guard = s.search_write_context(1_800_000_000_002).unwrap();
        let tx = c.unchecked_transaction().unwrap();
        tx.execute(
            "UPDATE crews SET title='trusted' WHERE id='context-crew'",
            [],
        )
        .unwrap();
        assert_eq!(
            search::mutation_time(&tx, key).unwrap().unwrap().0,
            Some(crate::ts_from_ms(1_800_000_000_002))
        );
        tx.commit().unwrap();
    }
    c.execute("UPDATE crews SET title='raw' WHERE id='context-crew'", [])
        .unwrap();
    assert_eq!(search::mutation_time(c, key).unwrap().unwrap().0, None);
    assert!(search::span(c).unwrap().is_none());
}
