//! Real pre-index schema30 writes; no fabricated version pragma or timestamps.
use super::*;
use crewd_proto::managed_objects::{Object, Receipt, Submit};

fn legacy_thirty() -> Store {
    let s = crewd_store::testing::open_legacy_store(30).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "crew-a".into(),
            title: "Historical synthetic crew".into(),
            created_at: crewd_store::ts_from_ms(NOW - 100),
            updated_at: crewd_store::ts_from_ms(NOW - 100),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_store::schema::backfill_v4(s.conn()).unwrap();
    assert!(!index::enabled(s.conn()).unwrap());
    s
}
fn old_submit(s: &mut Store, at: i64, change: Value) -> Receipt {
    let request = Submit {
        scope: managed_objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: serde_json::from_value(change).unwrap(),
    };
    ok_at(s, at, |tx| managed_objects::submit(tx, HUMAN, request)).0
}
fn decision_id(r: &Receipt) -> String {
    let Object::Decision { decision } = &r.object else {
        panic!("decision")
    };
    decision.id.clone()
}
fn draft(choice: &str) -> Value {
    json!({"question":"Historical decision needle","choice":choice,"rationale":"Retained exact replacement fact","about":[{"kind":"crew","id":"crew-a"}]})
}
#[test]
fn actual_thirty_superseded_decision_uses_successor_birth_not_original_birth_or_index_time() {
    let mut s = legacy_thirty();
    let first = old_submit(
        &mut s,
        NOW,
        json!({"kind":"recordDecision","draft":draft("initial")}),
    );
    let old = decision_id(&first);
    let next = old_submit(
        &mut s,
        NOW + 50_000,
        json!({"kind":"replaceDecision","decisionId":old,"expectedRevision":"1","draft":draft("replacement")}),
    );
    let replacement = decision_id(&next);
    crewd_store::schema::migrate(s.conn()).unwrap();
    assert!(
        index::mutation_time(s.conn(), &index::source_object(old[1..].parse().unwrap()))
            .unwrap()
            .is_none()
    );
    ready(&mut s);
    let page = qpage(&s, HUMAN, query("Historical decision needle"));
    assert_eq!(page.items.len(), 2);
    for id in [old.clone(), replacement] {
        let hit = page
            .items
            .iter()
            .find(|h| h.target == v2::TargetRef::Decision { id: id.clone() })
            .unwrap();
        assert_eq!(hit.changed_at, Some(crewd_store::ts_from_ms(NOW + 50_000)));
    }
    let hit = page
        .items
        .iter()
        .find(|h| h.target == v2::TargetRef::Decision { id: old.clone() })
        .unwrap();
    assert_eq!(hit.status, Status::Historical);
    // Unrelated maintenance and a real process-shaped reopen preserve proof.
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("recency.sqlite");
    s.conn()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(s);
    let mut reopened = Store::open(&path).unwrap();
    ready(&mut reopened);
    let again = qpage(&reopened, HUMAN, query("Historical decision needle"));
    assert_eq!(again.items, page.items);
}
#[test]
fn actual_thirty_row_edit_and_lesson_stale_keep_current_revision_receipt_times() {
    let mut s = legacy_thirty();
    let table = old_submit(
        &mut s,
        NOW,
        json!({"kind":"createTable","title":"Legacy row table","fields":[{"name":"Name","fieldType":"text","nullable":false}]}),
    );
    let Object::Table { table } = table.object else {
        panic!("table")
    };
    let row = old_submit(
        &mut s,
        NOW + 100,
        json!({"kind":"addRow","tableId":table.id,"values":[{"type":"text","value":"legacy recency row"}]}),
    );
    let Object::Row { row } = row.object else {
        panic!("row")
    };
    let edit = old_submit(
        &mut s,
        NOW + 20_000,
        json!({"kind":"editRow","rowId":row.id,"expectedRevision":"1","values":[{"type":"text","value":"legacy recency edited"}]}),
    );
    let Object::Row { row: edited } = edit.object else {
        panic!("edited row")
    };
    let lesson = old_submit(
        &mut s,
        NOW + 200,
        json!({"kind":"recordLesson","draft":{"happened":"Synthetic older observation","lesson":"legacy recency lesson","learnedFrom":[],"evidence":[],"appliesTo":[]}}),
    );
    let Object::Lesson { lesson } = lesson.object else {
        panic!("lesson")
    };
    old_submit(
        &mut s,
        NOW + 30_000,
        json!({"kind":"staleLesson","lessonId":lesson.id,"expectedRevision":"1","reason":"Synthetic changed context"}),
    );
    crewd_store::schema::migrate(s.conn()).unwrap();
    ready(&mut s);
    let row_hit = qpage(&s, HUMAN, query("legacy recency edited"))
        .items
        .into_iter()
        .find(|h| {
            h.target
                == v2::TargetRef::Row {
                    id: edited.id.clone(),
                }
        })
        .unwrap();
    assert_eq!(
        row_hit.changed_at,
        Some(crewd_store::ts_from_ms(NOW + 20_000))
    );
    let lesson_hit = qpage(&s, HUMAN, query("legacy recency lesson"))
        .items
        .into_iter()
        .find(|h| {
            h.target
                == v2::TargetRef::Lesson {
                    id: lesson.id.clone(),
                }
        })
        .unwrap();
    assert_eq!(
        lesson_hit.changed_at,
        Some(crewd_store::ts_from_ms(NOW + 30_000))
    );
    let Object::Lesson { lesson: current } =
        managed_objects::get(s.conn(), HUMAN, &lesson.id).unwrap()
    else {
        panic!("lesson")
    };
    assert_eq!(current.last_checked_at, lesson.last_checked_at);
    assert_ne!(lesson_hit.changed_at, Some(current.last_checked_at));
}
