use super::*;
use crate::testkit::{NOW, count, database_snapshot, err, ok, ok_at};
use crewd_store::Store;
use serde_json::{Value, json};
const HUMAN: Actor<'static> = Actor::Human("synthetic-v3-human");
const AUTHOR: Actor<'static> = Actor::Session("w-a1");
const OTHER: Actor<'static> = Actor::Session("w-a2");
const X: &links::DenyExternal = &links::DenyExternal;
fn fixture() -> Store {
    let mut s = crate::testkit::store();
    if crewd_store::meta_get(s.conn(), "dbId").unwrap().is_none() {
        crewd_store::meta_set(s.conn(), "dbId", "synthetic-v3-db").unwrap()
    }
    if !old_db::enabled(s.conn()).unwrap() {
        ok(&mut s, |tx| {
            tx.conn().execute_batch(old_db::DDL)?;
            old_db::migrate_v1(tx.conn())?;
            tx.conn().execute_batch(graph::DDL)?;
            Ok(())
        });
    }
    if !db::enabled(s.conn()).unwrap() {
        ok(&mut s, |tx| {
            tx.conn().execute_batch(db::DDL)?;
            db::migrate(tx.conn())?;
            Ok(())
        })
    }
    s
}
fn request(s: &Store, operation: Value) -> Submit {
    Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: serde_json::from_value(operation).unwrap(),
    }
}
fn check(
    s: &Store,
    actor: Actor<'_>,
    title: &str,
    purpose: Option<&str>,
    excluding: Option<&str>,
) -> DefinitionCheck {
    let mut q = DefineCheck {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        excluding_table_id: excluding.map(Into::into),
        title: title.into(),
        purpose: purpose.map(Into::into),
        cursor: None,
    };
    loop {
        let result = define_check(s.conn(), actor, q.clone()).unwrap();
        if result.state == CheckState::Complete {
            return result;
        }
        q.cursor = result.next_cursor;
    }
}
fn create(s: &mut Store, actor: Actor<'_>, title: &str, kinds: Vec<Value>, policy: &str) -> String {
    let token = check(s, actor, title, Some("Synthetic V3 authority"), None)
        .definition_check_token
        .unwrap();
    let fields = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| json!({"name":format!("Field{}",i+1),"kind":kind,"nullable":i>0}))
        .collect::<Vec<_>>();
    let req = request(
        s,
        json!({"kind":"createTable","title":title,"purpose":"Synthetic V3 authority","fields":fields,"titleFieldIndex":0,"rowWritePolicy":policy,"definitionCheckToken":token,"confirmNew":true}),
    );
    let Effect::TableCreated { table_id, .. } = ok(s, |tx| submit(tx, actor, req, X)).effect else {
        panic!("create")
    };
    table_id
}
fn set(field: &str, value: Value) -> Value {
    json!({"fieldId":field,"change":{"kind":"setScalar","value":value}})
}
fn add(s: &mut Store, actor: Actor<'_>, table: &str, patches: Vec<Value>) -> String {
    let t = get(s.conn(), HUMAN, table, X).unwrap();
    let r = request(
        s,
        json!({"kind":"addRow","tableId":table,"expectedSchemaRevision":t.schema_revision,"patches":patches}),
    );
    let Effect::RowAdded { row_id, .. } = ok(s, |tx| submit(tx, actor, r, X)).effect else {
        panic!("add")
    };
    row_id
}
fn patch(s: &Store, row_id: &str, patches: Vec<Value>) -> Submit {
    let row = row(s.conn(), HUMAN, row_id, X).unwrap();
    request(
        s,
        json!({"kind":"patchRow","rowId":row_id,"expectedSchemaRevision":row.schema_revision,"expectedRowRevision":row.revision,"reason":"Synthetic exact edit","patches":patches}),
    )
}
fn alter(s: &mut Store, actor: Actor<'_>, id: &str, change: Value) -> Receipt {
    let t = get(s.conn(), HUMAN, id, X).unwrap();
    let r = request(
        s,
        json!({"kind":"alterTable","tableId":id,"expectedSchemaRevision":t.schema_revision,"reason":"Synthetic schema evolution","change":change}),
    );
    ok(s, |tx| submit(tx, actor, r, X))
}
#[test]
fn typed_values_preserve_false_null_decimal_and_signed_zero_and_noop() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Exact scalar ledger",
        vec![
            json!({"kind":"text"}),
            json!({"kind":"decimal","unit":"USD","displayScale":2}),
            json!({"kind":"boolean"}),
            json!({"kind":"date"}),
            json!({"kind":"real"}),
        ],
        "authorCaptainHuman",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![
            set("p1", json!({"type":"text","value":"Raw e\u{301}"})),
            set(
                "p2",
                json!({"type":"decimal","value":"0.000000000000000001"}),
            ),
            set("p3", json!({"type":"boolean","value":false})),
            set("p4", json!({"type":"date","value":"2000-02-29"})),
            set("p5", json!({"type":"real","value":0.0})),
        ],
    );
    let result = row(s.conn(), HUMAN, &id, X).unwrap();
    assert_eq!(result.scalar_cells[2].value, Cell::Boolean(false));
    let req = patch(
        &s,
        &id,
        vec![set("p5", json!({"type":"real","value":-0.0}))],
    );
    let receipt = ok(&mut s, |tx| submit(tx, AUTHOR, req.clone(), X));
    assert!(receipt.event_id.is_some());
    let value = row(s.conn(), HUMAN, &id, X)
        .unwrap()
        .scalar_cells
        .pop()
        .unwrap()
        .value;
    assert!(matches!(value,Cell::Real(v)if v.to_bits()==(-0.0f64).to_bits()));
    let again = patch(
        &s,
        &id,
        vec![set("p5", json!({"type":"real","value":-0.0}))],
    );
    let (noop, changes) = ok_at(&mut s, NOW + 10, |tx| submit(tx, AUTHOR, again, X));
    assert!(noop.event_id.is_none());
    assert!(changes.is_empty());
    assert!(matches!(
        noop.effect,
        Effect::RowPatched { changed: false, .. }
    ));
    let before = database_snapshot(&s);
    let replay = ok(&mut s, |tx| submit(tx, AUTHOR, req, X));
    assert_eq!(replay, receipt);
    assert_eq!(database_snapshot(&s), before);
    for invalid_value in [
        json!({"type":"decimal","value":"1.0"}),
        json!({"type":"decimal","value":"1e2"}),
        json!({"type":"decimal","value":"-0"}),
    ] {
        let request = patch(&s, &id, vec![set("p2", invalid_value)]);
        let before = database_snapshot(&s);
        err(&mut s, |tx| submit(tx, AUTHOR, request, X));
        assert_eq!(database_snapshot(&s), before);
    }
}
#[test]
fn option_binding_inactive_keep_restore_and_table_wide_issued_ids() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Stable choice identifiers",
        vec![
            json!({"kind":"text"}),
            json!({"kind":"singleSelect","options":[{"label":"First","tone":"neutral"}]}),
            json!({"kind":"singleSelect","options":[{"label":"Second","tone":"good"}]}),
        ],
        "ownerCrewWriters",
    );
    let table = get(s.conn(), HUMAN, &t, X).unwrap();
    assert_eq!(table.next_option_number, "3");
    let choice = json!({"tableId":t,"fieldId":"p2","optionId":"e1"});
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![
            set("p1", json!({"type":"text","value":"Choice row"})),
            set("p2", json!({"type":"singleSelect","value":choice})),
        ],
    );
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"deactivateOption","fieldId":"p2","option":choice}),
    );
    assert!(matches!(
        row(s.conn(), HUMAN, &id, X).unwrap().scalar_cells[1].value,
        Cell::SingleSelect(_)
    ));
    let same = patch(
        &s,
        &id,
        vec![set("p2", json!({"type":"singleSelect","value":choice}))],
    );
    let before = database_snapshot(&s);
    err(&mut s, |tx| submit(tx, AUTHOR, same, X));
    assert_eq!(database_snapshot(&s), before);
    let keep = patch(
        &s,
        &id,
        vec![set("p1", json!({"type":"text","value":"Changed title"}))],
    );
    ok(&mut s, |tx| submit(tx, OTHER, keep, X));
    let wrong = patch(
        &s,
        &id,
        vec![set("p3", json!({"type":"singleSelect","value":choice}))],
    );
    err(&mut s, |tx| submit(tx, AUTHOR, wrong, X));
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"restoreOption","fieldId":"p2","option":choice}),
    );
    let next = alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"addOption","fieldId":"p3","option":{"label":"Third","tone":"info"}}),
    );
    assert!(
        matches!(next.effect,Effect::SchemaChanged{ref issued_options,..}if issued_options[0].option_id=="e3")
    );
}
#[test]
fn archived_row_policy_is_independent_of_table_schema_and_does_not_resolve_business() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Lifecycle records",
        vec![json!({"kind":"text"})],
        "ownerCrewWriters",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![set("p1", json!({"type":"text","value":"Still factual"}))],
    );
    let original = row(s.conn(), HUMAN, &id, X).unwrap();
    let req = request(
        &s,
        json!({"kind":"archiveRow","rowId":id,"expectedSchemaRevision":original.schema_revision,"expectedRowRevision":original.revision,"reason":"Synthetic retention"}),
    );
    let receipt = ok(&mut s, |tx| submit(tx, OTHER, req.clone(), X));
    assert!(receipt.event_id.is_some());
    let current = row(s.conn(), HUMAN, &id, X).unwrap();
    assert_eq!(current.scalar_cells, original.scalar_cells);
    assert!(current.lifecycle.archived_at.is_some());
    assert!(
        rows(
            s.conn(),
            HUMAN,
            RowList {
                table_id: t.clone(),
                after: None,
                include_archived: false
            },
            X
        )
        .unwrap()
        .items
        .is_empty()
    );
    let edit = patch(
        &s,
        &id,
        vec![set("p1", json!({"type":"text","value":"Forbidden"}))],
    );
    err(&mut s, |tx| submit(tx, AUTHOR, edit, X));
    let before = database_snapshot(&s);
    assert_eq!(ok(&mut s, |tx| submit(tx, OTHER, req, X)), receipt);
    assert_eq!(database_snapshot(&s), before);
    let table = get(s.conn(), HUMAN, &t, X).unwrap();
    let archive = request(
        &s,
        json!({"kind":"archiveTable","tableId":t,"expectedSchemaRevision":table.schema_revision,"reason":"Synthetic table retention"}),
    );
    err(&mut s, |tx| submit(tx, OTHER, archive.clone(), X));
    ok(&mut s, |tx| submit(tx, AUTHOR, archive, X));
    let r = row(s.conn(), HUMAN, &id, X).unwrap();
    let restore = request(
        &s,
        json!({"kind":"restoreRow","rowId":id,"expectedSchemaRevision":r.schema_revision,"expectedRowRevision":r.revision,"reason":"Synthetic return"}),
    );
    err(&mut s, |tx| submit(tx, AUTHOR, restore, X));
    assert_eq!(count(&s, "SELECT count(*) FROM wakes"), 0);
}
#[test]
fn legacy_none_purpose_rename_does_not_mint_a_create_or_setpurpose_certificate() {
    let mut s = fixture();
    let r:crewd_proto::managed_objects::Submit=serde_json::from_value(json!({"scope":objects::scope(s.conn()).unwrap(),"crewId":"crew-a","requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"createTable","title":"Legacy absent purpose","fields":[{"name":"Name","fieldType":"text","nullable":false}]}})).unwrap();
    let crewd_proto::managed_objects::Object::Table { table } =
        ok(&mut s, |tx| objects::submit(tx, AUTHOR, r)).object
    else {
        panic!("legacy")
    };
    let table_id = table.id;
    let proof = check(&s, AUTHOR, "Legacy renamed", None, Some(&table_id))
        .definition_check_token
        .unwrap();
    let rename_request = request(
        &s,
        json!({"kind":"alterTable","tableId":table_id,"expectedSchemaRevision":"1","reason":"Preserve absent purpose","change":{"kind":"renameTable","title":"Legacy renamed","definitionCheckToken":proof,"confirmNew":true}}),
    );
    ok(&mut s, |tx| submit(tx, AUTHOR, rename_request, X));
    assert!(
        get(s.conn(), HUMAN, &table_id, X)
            .unwrap()
            .purpose
            .is_none()
    );
    assert!(
        define_check(
            s.conn(),
            AUTHOR,
            DefineCheck {
                scope: objects::scope(s.conn()).unwrap(),
                crew_id: "crew-a".into(),
                excluding_table_id: None,
                title: "Missing purpose create".into(),
                purpose: None,
                cursor: None
            }
        )
        .is_err()
    );
    let wrong = request(
        &s,
        json!({"kind":"createTable","title":"Legacy renamed","purpose":"Now provided","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","definitionCheckToken":proof,"confirmNew":true}),
    );
    let before = database_snapshot(&s);
    err(&mut s, |tx| submit(tx, AUTHOR, wrong, X));
    assert_eq!(database_snapshot(&s), before);
}
#[test]
fn required_new_link_and_late_audit_failure_roll_back_number_receipt_event_and_source() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Transactional table",
        vec![json!({"kind":"text"})],
        "authorCaptainHuman",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![set("p1", json!({"type":"text","value":"Before"}))],
    );
    let table = get(s.conn(), HUMAN, &t, X).unwrap();
    let required = request(
        &s,
        json!({"kind":"alterTable","tableId":t,"expectedSchemaRevision":table.schema_revision,"reason":"No implicit default","change":{"kind":"addField","field":{"name":"Required","kind":{"kind":"link","target":{"kind":"work"},"many":false},"nullable":false}}}),
    );
    let before = database_snapshot(&s);
    err(&mut s, |tx| submit(tx, AUTHOR, required, X));
    assert_eq!(database_snapshot(&s), before);
    s.conn().execute_batch("CREATE TRIGGER synthetic_audit_failure BEFORE INSERT ON managed_table_audit_values BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;").unwrap();
    let edit = patch(
        &s,
        &id,
        vec![set("p1", json!({"type":"text","value":"After"}))],
    );
    let before = database_snapshot(&s);
    let error = err(&mut s, |tx| submit(tx, AUTHOR, edit, X));
    assert!(error.to_string().contains("synthetic audit failure"));
    assert_eq!(database_snapshot(&s), before);
}
#[test]
fn numeric_to_text_is_lazy_and_original_receipts_remain_old_codecs() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Numeric widening evidence",
        vec![json!({"kind":"text"}), json!({"kind":"real"})],
        "authorCaptainHuman",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![
            set("p1", json!({"type":"text","value":"Original"})),
            set("p2", json!({"type":"real","value":-0.0})),
        ],
    );
    let raw: String = s
        .conn()
        .query_row(
            "SELECT values_json FROM managed_rows WHERE number=?1",
            [num(&id, 'O').unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"widenNumberToText","fieldId":"p2"}),
    );
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT values_json FROM managed_rows WHERE number=?1",
                [num(&id, 'O').unwrap()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        raw
    );
    assert_eq!(
        row(s.conn(), HUMAN, &id, X).unwrap().scalar_cells[1].value,
        Cell::Text("-0.0".into())
    );
    let edit = patch(
        &s,
        &id,
        vec![set("p1", json!({"type":"text","value":"New"}))],
    );
    ok(&mut s, |tx| submit(tx, AUTHOR, edit, X));
    assert!(
        matches!(db::stored_scalars(s.conn(),num(&id,'O').unwrap()).unwrap()[1].value,Cell::Real(v)if v.to_bits()==(-0.0f64).to_bits())
    );
}
#[test]
fn legacy_long_value_history_chunks_are_lossless_and_do_not_inline_into_headers() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Chunked reasons and values",
        vec![json!({"kind":"text"}), json!({"kind":"longText"})],
        "authorCaptainHuman",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![
            set("p1", json!({"type":"text","value":"Original"})),
            set("p2", json!({"type":"text","value":"x".repeat(1500)})),
        ],
    );
    let edit = patch(
        &s,
        &id,
        vec![set(
            "p2",
            json!({"type":"text","value":"\u{754c}".repeat(1500)}),
        )],
    );
    let receipt = ok(&mut s, |tx| submit(tx, AUTHOR, edit, X));
    let history = history(
        s.conn(),
        HUMAN,
        History {
            source: HistorySource::Row { id: id.clone() },
            cursor: None,
        },
        X,
    )
    .unwrap();
    assert!(serde_json::to_vec(&history).unwrap().len() <= HISTORY_ENCODED_BYTES);
    let mut q = HistoryValue {
        source: HistorySource::Row { id },
        key: HistoryKey::Event {
            event_id: receipt.event_id.unwrap(),
        },
        path: HistoryPath::Scalar {
            field_id: "p2".into(),
            side: HistorySide::After,
        },
        cursor: None,
    };
    let before = database_snapshot(&s);
    let changes = s.conn().total_changes();
    let mut full = String::new();
    loop {
        let ValueChunk::Scalar {
            utf8, next_cursor, ..
        } = history_value(s.conn(), HUMAN, q.clone(), X).unwrap()
        else {
            panic!("scalar")
        };
        assert!(utf8.len() <= HISTORY_CHUNK_UTF8_BYTES);
        full.push_str(&utf8);
        if next_cursor.is_none() {
            break;
        }
        q.cursor = next_cursor;
    }
    assert_eq!(full, "\u{754c}".repeat(1500));
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), changes);
}
#[test]
fn versioned_lookup_never_translates_an_original_receipt_or_reexecutes_its_uuid() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Versioned recovery",
        vec![json!({"kind":"text"}), json!({"kind":"boolean"})],
        "authorCaptainHuman",
    );
    assert!(old_domain::get(s.conn(), AUTHOR, &t, X).is_err());
    assert!(objects::get(s.conn(), AUTHOR, &t).is_err());
    let uuid: String = s
        .conn()
        .query_row(
            "SELECT request_id FROM managed_v2_receipts WHERE method=?1 AND owner_crew='crew-a'",
            [SUBMIT],
            |r| r.get(0),
        )
        .unwrap();
    let scope = objects::scope(s.conn()).unwrap();
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    assert!(matches!(
        lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).unwrap(),
        Lookup::Known { .. }
    ));
    assert!(old_domain::lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).is_err());
    assert!(objects::lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
    // Shared organization object references stay usable even when a full old
    // Table/Row codec cannot express the current schema.
    assert_eq!(
        objects::read_authority(s.conn(), Actor::Session("w-b1"), &t).unwrap(),
        "crew-a"
    );
    assert_eq!(
        links::resolve(
            s.conn(),
            Actor::Session("w-b1"),
            &v2::TargetRef::Table { id: t.clone() },
            X
        )
        .unwrap()
        .target,
        v2::TargetRef::Table { id: t }
    );
}
#[test]
fn actual_crew_purge_cascades_v3_lifecycle_audit_and_keeps_issued_option_floor() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Purge issued choices",
        vec![
            json!({"kind":"text"}),
            json!({"kind":"singleSelect","options":[{"label":"Retained allocation","tone":"neutral"}]}),
        ],
        "ownerCrewWriters",
    );
    let id = add(
        &mut s,
        AUTHOR,
        &t,
        vec![set("p1", json!({"type":"text","value":"Disposable row"}))],
    );
    let floor = db::option_allocator_floor(s.conn()).unwrap();
    assert!(floor >= 2);
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
    assert!(
        db::table(s.conn(), num(&t, 'T').unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        db::row_base(s.conn(), num(&id, 'O').unwrap())
            .unwrap()
            .is_none()
    );
    for table in [
        "managed_table_v3_meta",
        "managed_row_v3_meta",
        "managed_table_audit",
        "managed_table_audit_values",
    ] {
        assert_eq!(count(&s, &format!("SELECT count(*) FROM {table}")), 0)
    }
    assert_eq!(db::option_allocator_floor(s.conn()).unwrap(), floor);
    assert!(crate::crews::get(s.conn(), "crew-b").is_ok());
}
#[test]
fn maximum_definition_text_and_eight_candidates_mint_consumable_compact_tokens() {
    let mut s = fixture();
    let purpose = "\u{1f9ea}".repeat(1500);
    for i in 0..8 {
        let title = format!("Large definition {i}");
        let proof = check(&s, AUTHOR, &title, Some(&purpose), None)
            .definition_check_token
            .unwrap();
        assert!(proof.len() <= 16384);
        let r = request(
            &s,
            json!({"kind":"createTable","title":title,"purpose":purpose,"fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","definitionCheckToken":proof,"confirmNew":true}),
        );
        ok(&mut s, |tx| submit(tx, AUTHOR, r, X));
    }
    let title = "\u{1f9ea}".repeat(256);
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    let check = check(&s, AUTHOR, &title, Some(&purpose), None);
    assert_eq!(check.near_candidates.len(), 8);
    let token = check.definition_check_token.unwrap();
    assert!(token.len() <= 16384);
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
    let r = request(
        &s,
        json!({"kind":"createTable","title":title,"purpose":purpose,"fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","definitionCheckToken":token,"confirmNew":true}),
    );
    ok(&mut s, |tx| submit(tx, AUTHOR, r, X));
}
fn search_ready(s: &mut Store) {
    for _ in 0..4096 {
        if !crate::organization_search::maintenance_needed(s.conn(), NOW).unwrap() {
            return;
        }
        let (_, changes) = crate::run(s, NOW, |tx| {
            crate::organization_search::maintenance_step(tx, 64)
        })
        .unwrap();
        assert!(changes.is_empty());
    }
    panic!("search maintenance did not settle")
}
fn query(
    s: &Store,
    term: &str,
    cursor: Option<String>,
    limit: u16,
) -> crewd_proto::organization_search::Page {
    let q = serde_json::from_value(
        json!({"query":term,"in":"objects","kinds":["row"],"cursor":cursor,"limit":limit}),
    )
    .unwrap();
    crate::organization_search::search(
        s.conn(),
        HUMAN,
        q,
        crate::organization_search::ReadClock {
            wall_ms: NOW + 100_000,
            elapsed_ms: 100_000,
            boot_id: "synthetic-v3-boot".into(),
        },
    )
    .unwrap()
}
fn collect_search(
    s: &Store,
    term: &str,
    mut cursor: Option<String>,
) -> Vec<crewd_proto::organization_search::Hit> {
    let mut hits = Vec::new();
    for _ in 0..256 {
        let page = query(s, term, cursor, 1);
        hits.extend(page.items);
        if page.next_cursor.is_none() {
            return hits;
        }
        cursor = page.next_cursor;
    }
    panic!("search cursor did not finish")
}
#[test]
fn selection_label_rename_uses_retained_cut_and_current_match_without_row_fanout() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Selection cut fixtures",
        vec![
            json!({"kind":"text"}),
            json!({"kind":"singleSelect","options":[{"label":"Option alpha","tone":"neutral"},{"label":"Other choice","tone":"good"}]}),
        ],
        "ownerCrewWriters",
    );
    let choice = |id: &str| json!({"type":"singleSelect","value":{"tableId":t,"fieldId":"p2","optionId":id}});
    let mut alpha = Vec::new();
    for i in 0..2 {
        alpha.push(add(
            &mut s,
            AUTHOR,
            &t,
            vec![
                set(
                    "p1",
                    json!({"type":"text","value":format!("Unseen candidate {i}")}),
                ),
                set("p2", choice("e1")),
            ],
        ));
    }
    for i in 0..2 {
        add(
            &mut s,
            AUTHOR,
            &t,
            vec![
                set(
                    "p1",
                    json!({"type":"text","value":format!("Option beta seed {i}")}),
                ),
                set("p2", choice("e2")),
            ],
        );
    }
    search_ready(&mut s);
    let old_beta = query(&s, "Option beta", None, 1);
    assert!(old_beta.next_cursor.is_some());
    let old_alpha = query(&s, "Option alpha", None, 1);
    assert!(old_alpha.next_cursor.is_some());
    let before_rows=s.conn().query_row("SELECT json_group_array(json_array(number,revision,values_json)) FROM (SELECT * FROM managed_rows ORDER BY number)",[],|r|r.get::<_,String>(0)).unwrap();
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"renameOption","fieldId":"p2","option":{"tableId":t,"fieldId":"p2","optionId":"e1"},"label":"Option beta"}),
    );
    search_ready(&mut s);
    assert_eq!(s.conn().query_row("SELECT json_group_array(json_array(number,revision,values_json)) FROM (SELECT * FROM managed_rows ORDER BY number)",[],|r|r.get::<_,String>(0)).unwrap(),before_rows);
    let old_tail = collect_search(&s, "Option beta", old_beta.next_cursor);
    assert!(old_tail.iter().all(|h| {
        !alpha
            .iter()
            .any(|id| h.target == v2::TargetRef::Row { id: id.clone() })
    }));
    assert!(collect_search(&s, "Option alpha", old_alpha.next_cursor).is_empty());
    let fresh = collect_search(&s, "Option beta", None);
    for id in &alpha {
        assert!(
            fresh
                .iter()
                .any(|h| h.target == v2::TargetRef::Row { id: id.clone() })
        );
    }
    // Moving the title role to this same real scalar field changes no row
    // revision or mutation time, and returns the current chosen label.
    let changed_at = fresh
        .iter()
        .find(|h| {
            h.target
                == v2::TargetRef::Row {
                    id: alpha[0].clone(),
                }
        })
        .unwrap()
        .changed_at
        .clone();
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"setTitleField","fieldId":"p2"}),
    );
    search_ready(&mut s);
    let fresh = collect_search(&s, "Option beta", None);
    let hit = fresh
        .iter()
        .find(|h| {
            h.target
                == v2::TargetRef::Row {
                    id: alpha[0].clone(),
                }
        })
        .unwrap();
    assert_eq!(hit.title, "Option beta");
    assert_eq!(hit.changed_at, changed_at);
}
#[test]
fn table_archive_search_fanout_is_bounded_and_never_bumps_business_rows() {
    let mut s = fixture();
    let t = create(
        &mut s,
        AUTHOR,
        "Bounded lifecycle search",
        vec![json!({"kind":"text"})],
        "ownerCrewWriters",
    );
    for i in 0..70 {
        add(
            &mut s,
            AUTHOR,
            &t,
            vec![set(
                "p1",
                json!({"type":"text","value":format!("Lifecycle result {i}")}),
            )],
        );
    }
    search_ready(&mut s);
    let original = s
        .conn()
        .query_row("SELECT sum(revision) FROM managed_rows", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    let table = get(s.conn(), HUMAN, &t, X).unwrap();
    let r = request(
        &s,
        json!({"kind":"archiveTable","tableId":t,"expectedSchemaRevision":table.schema_revision,"reason":"Synthetic bounded maintenance"}),
    );
    ok(&mut s, |tx| submit(tx, AUTHOR, r, X));
    let page = query(&s, "Lifecycle result", None, 20);
    assert_eq!(
        page.index_state,
        crewd_proto::organization_search::IndexState::Building
    );
    assert!(page.items.is_empty());
    let before = count(&s, "SELECT count(*) FROM organization_search_dirty");
    ok(&mut s, |tx| {
        assert_eq!(db::search_fanout_step(tx.conn(), 7)?, 7);
        Ok(())
    });
    let after = count(&s, "SELECT count(*) FROM organization_search_dirty");
    assert!(after - before <= 7);
    search_ready(&mut s);
    let hits = collect_search(&s, "Lifecycle result", None);
    assert_eq!(hits.len(), 70);
    assert!(
        hits.iter()
            .all(|h| h.status == crewd_proto::organization_search::Status::Archived)
    );
    assert_eq!(
        s.conn()
            .query_row("SELECT sum(revision) FROM managed_rows", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        original
    );
}
#[test]
fn fresh_legacy_mutations_audit_once_and_old_receipts_survive_new_codec_and_archive() {
    let mut s = fixture();
    let scope = objects::scope(s.conn()).unwrap();
    let create:crewd_proto::managed_objects::Submit=serde_json::from_value(json!({"scope":scope,"crewId":"crew-a","requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"createTable","title":"Legacy recovery authority","fields":[{"name":"Name","fieldType":"text","nullable":false}]}})).unwrap();
    let created = ok(&mut s, |tx| objects::submit(tx, AUTHOR, create.clone()));
    let crewd_proto::managed_objects::Object::Table { table } = &created.object else {
        panic!("table")
    };
    let t = table.id.clone();
    let add:crewd_proto::managed_objects::Submit=serde_json::from_value(json!({"scope":scope,"crewId":"crew-a","requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"addRow","tableId":t,"values":[{"type":"text","value":"Original receipt value"}]}})).unwrap();
    let added = ok(&mut s, |tx| objects::submit(tx, AUTHOR, add.clone()));
    let crewd_proto::managed_objects::Object::Row { row: original } = &added.object else {
        panic!("row")
    };
    assert_eq!(count(&s, "SELECT count(*) FROM managed_table_audit"), 2);
    alter(
        &mut s,
        AUTHOR,
        &t,
        json!({"kind":"addField","field":{"name":"New boolean","kind":{"kind":"boolean"},"nullable":true}}),
    );
    let current = get(s.conn(), HUMAN, &t, X).unwrap();
    let archive = request(
        &s,
        json!({"kind":"archiveTable","tableId":t,"expectedSchemaRevision":current.schema_revision,"reason":"Keep original evidence"}),
    );
    ok(&mut s, |tx| submit(tx, AUTHOR, archive, X));
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    assert_eq!(
        ok(&mut s, |tx| objects::submit(tx, AUTHOR, create.clone())),
        created
    );
    assert_eq!(
        ok(&mut s, |tx| objects::submit(tx, AUTHOR, add.clone())),
        added
    );
    assert!(
        matches!(objects::lookup(s.conn(),AUTHOR,&scope,"crew-a",&add.request_id).unwrap(),crewd_proto::managed_objects::Lookup::Known{receipt}if receipt==added)
    );
    assert!(lookup(s.conn(), AUTHOR, &scope, "crew-a", &add.request_id).is_err());
    assert!(old_domain::lookup(s.conn(), AUTHOR, &scope, "crew-a", &add.request_id).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
    let headers = history(
        s.conn(),
        HUMAN,
        History {
            source: HistorySource::Row {
                id: original.id.clone(),
            },
            cursor: None,
        },
        X,
    )
    .unwrap();
    assert_eq!(headers.items.len(), 1);
    assert!(headers.items[0].legacy_details_unavailable);
    assert!(!headers.items[0].reason_available);
    let result = history_value(
        s.conn(),
        HUMAN,
        HistoryValue {
            source: HistorySource::Row {
                id: original.id.clone(),
            },
            key: headers.items[0].key.clone(),
            path: HistoryPath::Scalar {
                field_id: "p1".into(),
                side: HistorySide::After,
            },
            cursor: None,
        },
        X,
    )
    .unwrap();
    assert!(matches!(result,ValueChunk::Scalar{utf8,..}if utf8=="Original receipt value"));
}
fn reseal_v3(body: &mut Value) {
    body["seal"] = Value::String(graph::hash(
        &serde_json::to_vec(&(
            &body["method"],
            &body["digest"],
            &body["input"],
            &body["result"],
            &body["principal"],
        ))
        .unwrap(),
    ));
}
#[test]
fn valid_json_receipt_tampering_cannot_replace_original_effect_or_event_and_never_mutates() {
    let mut s = fixture();
    create(
        &mut s,
        AUTHOR,
        "Guarded original evidence",
        vec![json!({"kind":"text"})],
        "ownerCrewWriters",
    );
    let (uuid, raw): (String, String) = s
        .conn()
        .query_row(
            "SELECT request_id,result_json FROM managed_v2_receipts WHERE method=?1",
            [SUBMIT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let original: Value = serde_json::from_str(&raw).unwrap();
    let intent: Submit = serde_json::from_value(original["input"].clone()).unwrap();
    let mut variants = Vec::new();
    for (path, value) in [
        ("/result/effect/schemaRevision", json!("2")),
        ("/result/effect/tableId", json!("T9223372036854775807")),
        ("/result/operationKind", json!("addRow")),
        ("/result/recordedAt", json!("2026-09-21T13:33:20Z")),
        ("/result/eventId", Value::Null),
        (
            "/result/scope/restoreEpoch",
            json!(uuid::Uuid::new_v4().to_string()),
        ),
    ] {
        let mut body = original.clone();
        *body.pointer_mut(path).unwrap() = value;
        reseal_v3(&mut body);
        variants.push(body);
    }
    for variant in variants {
        s.conn()
            .execute(
                "UPDATE managed_v2_receipts SET result_json=?1 WHERE request_id=?2",
                params![serde_json::to_string(&variant).unwrap(), uuid],
            )
            .unwrap();
        let before = database_snapshot(&s);
        let physical = s.conn().total_changes();
        assert!(lookup(s.conn(), AUTHOR, &intent.scope, "crew-a", &uuid).is_err());
        err(&mut s, |tx| submit(tx, AUTHOR, intent.clone(), X));
        assert_eq!(database_snapshot(&s), before);
        assert_eq!(s.conn().total_changes(), physical);
        s.conn()
            .execute(
                "UPDATE managed_v2_receipts SET result_json=?1 WHERE request_id=?2",
                params![raw, uuid],
            )
            .unwrap();
    }
    assert!(matches!(
        lookup(s.conn(), AUTHOR, &intent.scope, "crew-a", &uuid).unwrap(),
        Lookup::Known { .. }
    ));
    // The checksum binds the captured principal only for the new V3 codec.
    // Copying a real same-intent UUID to another read-authorized principal
    // cannot acknowledge that principal's never-committed operation.
    s.conn().execute("INSERT INTO managed_v2_receipts(owner_crew,principal,request_id,method,digest,input_json,result_json) SELECT owner_crew,'session:w-a2',request_id,method,digest,input_json,result_json FROM managed_v2_receipts WHERE request_id=?1",[&uuid]).unwrap();
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    assert!(lookup(s.conn(), OTHER, &intent.scope, "crew-a", &uuid).is_err());
    err(&mut s, |tx| submit(tx, OTHER, intent.clone(), X));
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
}
#[test]
fn retired_uuid_keeps_its_codec_and_old_v2_saved_bytes_keep_the_legacy_checksum() {
    let mut s = fixture();
    let table = create(
        &mut s,
        AUTHOR,
        "Retired original namespace",
        vec![json!({"kind":"text"})],
        "ownerCrewWriters",
    );
    let uuid: String = s
        .conn()
        .query_row(
            "SELECT request_id FROM managed_v2_receipts WHERE method=?1",
            [SUBMIT],
            |r| r.get(0),
        )
        .unwrap();
    let scope = objects::scope(s.conn()).unwrap();
    // Equivalent to the existing atomic dependency-retirement hook; no row
    // body or source action is recreated on any recovery path.
    let original: Submit = serde_json::from_str(
        &s.conn()
            .query_row(
                "SELECT input_json FROM managed_v2_receipts WHERE request_id=?1",
                [&uuid],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    s.conn().execute("UPDATE managed_v2_receipts SET input_json=NULL,result_json=NULL,retired=1 WHERE request_id=?1",[&uuid]).unwrap();
    let before = database_snapshot(&s);
    assert!(matches!(
        lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).unwrap(),
        Lookup::Historical { receipt: None }
    ));
    assert!(old_domain::lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).is_err());
    assert!(objects::lookup(s.conn(), AUTHOR, &scope, "crew-a", &uuid).is_err());
    err(&mut s, |tx| submit(tx, AUTHOR, original, X));
    assert_eq!(database_snapshot(&s), before);
    assert!(get(s.conn(), HUMAN, &table, X).is_ok());
    let mut legacy = fixture();
    let input:v2::Submit=serde_json::from_value(json!({"scope":objects::scope(legacy.conn()).unwrap(),"crewId":"crew-a","requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"createTable","title":"Original V2 envelope","purpose":"Unchanged original format","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters"}})).unwrap();
    let saved = ok(&mut legacy, |tx| {
        old_domain::submit(tx, AUTHOR, input.clone(), X)
    });
    let raw: String = legacy
        .conn()
        .query_row(
            "SELECT result_json FROM managed_v2_receipts WHERE request_id=?1",
            [&input.request_id],
            |r| r.get(0),
        )
        .unwrap();
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert!(body.get("principal").is_none());
    assert_eq!(
        body["seal"],
        graph::hash(
            &serde_json::to_vec(&(
                &body["method"],
                &body["digest"],
                &body["input"],
                &body["result"]
            ))
            .unwrap()
        )
    );
    let before = database_snapshot(&legacy);
    assert_eq!(
        ok(&mut legacy, |tx| old_domain::submit(tx, AUTHOR, input, X)),
        saved
    );
    assert_eq!(database_snapshot(&legacy), before);
    assert_eq!(
        legacy
            .conn()
            .query_row("SELECT result_json FROM managed_v2_receipts", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        raw
    );
}

mod history_privacy {
    include!("managed_tables_v3_privacy_tests.rs");
}
