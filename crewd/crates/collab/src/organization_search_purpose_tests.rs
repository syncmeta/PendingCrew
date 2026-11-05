//! Historical purposes are table-only aliases of exact retained schema events.
use super::*;
const OLD: &str = "purpose-before-needle";
const NEW: &str = "purpose-current-needle";
fn legacy_thirty() -> Store {
    let s = crewd_store::testing::open_legacy_store(30).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "crew-a".into(),
            title: "Synthetic purpose crew".into(),
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
fn create(s: &mut Store) -> (String, String) {
    let r = request(
        s,
        "crew-a",
        json!({"kind":"createTable","title":"Purpose contract table","purpose":OLD,"fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false},{"name":"Body","kind":{"kind":"text"},"nullable":true}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let receipt = ok(s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
    let v2::Effect::TableCreated { table_id, .. } = receipt.effect else {
        panic!("table")
    };
    let row = row(s, &table_id, NOW, "Neutral row", "Neutral cell");
    (table_id, row)
}
fn change(s: &mut Store, id: &str) {
    let r = request(
        s,
        "crew-a",
        json!({"kind":"alterTable","tableId":id,"expectedSchemaRevision":"1","change":{"kind":"setPurpose","purpose":NEW}}),
    );
    ok_at(s, NOW + 200, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
}
fn assert_table_only(s: &Store, t: &str, o: &str) {
    for (text, on) in [(OLD, MatchOn::HistoricalName), (NEW, MatchOn::Text)] {
        let page = qpage(s, HUMAN, query(text));
        assert_eq!(page.index_state, IndexState::Ready);
        assert_eq!(page.items.len(), 1, "purpose query {text}");
        assert_eq!(page.items[0].target, v2::TargetRef::Table { id: t.into() });
        assert_eq!(page.items[0].matched_on, on);
        let mut row_query = query(text);
        row_query.table_id = Some(t.into());
        assert!(qpage(s, HUMAN, row_query).items.is_empty());
    }
    let raw:i64=s.conn().query_row("SELECT count(*) FROM organization_search_slots slot JOIN organization_search_versions v ON v.id=slot.version_id WHERE v.source=?1 AND (instr(slot.body,?2)>0 OR instr(slot.body,?3)>0)",rusqlite::params![index::source_object(o[1..].parse().unwrap()),OLD,NEW],|r|r.get(0)).unwrap();
    assert_eq!(raw, 0, "table purpose must not be copied into row slots");
}
fn facts(s: &Store) -> (i64, i64, i64, i64) {
    (
        count(s, "SELECT count(*) FROM object_refs"),
        count(s, "SELECT count(*) FROM messages"),
        count(s, "SELECT count(*) FROM wakes"),
        count(s, "SELECT count(*) FROM managed_table_schema_events"),
    )
}
#[test]
fn protected_thirty_bootstrap_searches_original_and_current_purpose_only_on_table() {
    let mut old = legacy_thirty();
    let (t, o) = create(&mut old);
    change(&mut old, &t);
    let before = facts(&old);
    let scope = managed_objects::scope(old.conn()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("purpose.sqlite");
    old.conn()
        .execute("VACUUM INTO ?1", [file.to_str().unwrap()])
        .unwrap();
    drop(old);
    let mut migrated = Store::open(&file).unwrap();
    assert_eq!(
        migrated
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        31
    );
    assert_eq!(managed_objects::scope(migrated.conn()).unwrap(), scope);
    let snapshots = std::fs::read_dir(root.path().join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect::<Vec<_>>();
    assert_eq!(
        snapshots.len(),
        1,
        "guarded upgrade retains the actual schema30 input"
    );
    let backup = crewd_store::backup::open_read_only(&snapshots[0]).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        30
    );
    let evidence:(String,String)=backup.query_row("SELECT json_extract(before_json,'$.purpose'),json_extract(after_json,'$.purpose') FROM managed_table_schema_events",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(evidence, (OLD.into(), NEW.into()));
    ready(&mut migrated);
    assert_table_only(&migrated, &t, &o);
    assert_eq!(facts(&migrated), before);
}
#[test]
fn live_thirty_one_purpose_change_keeps_exact_old_alias_without_row_fanout() {
    let mut s = fixture();
    let (t, o) = create(&mut s);
    ready(&mut s);
    let row_revision = crewd_store::managed_tables_v2::row_base(s.conn(), o[1..].parse().unwrap())
        .unwrap()
        .unwrap()
        .1;
    let before = facts(&s);
    change(&mut s, &t);
    ready(&mut s);
    assert_table_only(&s, &t, &o);
    let after = facts(&s);
    assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
    assert_eq!(after.3, before.3 + 1);
    assert_eq!(
        crewd_store::managed_tables_v2::row_base(s.conn(), o[1..].parse().unwrap())
            .unwrap()
            .unwrap()
            .1,
        row_revision
    );
    let number: i64 = t[1..].parse().unwrap();
    let aliases:i64=s.conn().query_row("SELECT count(*) FROM organization_search_aliases WHERE table_number=?1 AND field_id='@purpose' AND label=?2",rusqlite::params![number,OLD],|r|r.get(0)).unwrap();
    assert_eq!(aliases, 1);
}
