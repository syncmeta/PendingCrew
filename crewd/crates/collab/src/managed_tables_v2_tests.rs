use super::*;
use crate::{
    Change, managed_links as links,
    testkit::{NOW, count, err, ok, ok_at},
};
use crewd_store::Store;
use serde_json::{Value, json};
const HUMAN: Actor<'static> = Actor::Human("synthetic-local");
const A: Actor<'static> = Actor::Session("w-a1");
const OTHER: Actor<'static> = Actor::Session("w-a2");
const X: &links::DenyExternal = &links::DenyExternal;
fn fixture() -> Store {
    let mut s = crate::testkit::store();
    if crewd_store::meta_get(s.conn(), "dbId").unwrap().is_none() {
        crewd_store::meta_set(s.conn(), "dbId", "synthetic-v2-db").unwrap();
    }
    if !db::enabled(s.conn()).unwrap() {
        ok(&mut s, |tx| {
            tx.conn().execute_batch(db::DDL)?;
            db::migrate_v1(tx.conn())?;
            tx.conn().execute_batch(graph::DDL)?;
            Ok(())
        });
    }
    s
}
fn request(s: &Store, crew: &str, op: Value) -> Submit {
    Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: serde_json::from_value(op).unwrap(),
    }
}
fn create(
    s: &mut Store,
    actor: Actor<'_>,
    crew: &str,
    link: Option<Value>,
    policy: &str,
) -> String {
    let mut fs = vec![
        json!({"name":"Name","kind":{"kind":"text"},"nullable":false}),
        json!({"name":"Count","kind":{"kind":"integer"},"nullable":true}),
    ];
    if let Some(kind) = link {
        fs.push(json!({"name":"Related","kind":kind,"nullable":true}));
    }
    let r = request(
        s,
        crew,
        json!({"kind":"createTable","title":"Synthetic table","purpose":"Scoped fixture","fields":fs,"titleFieldIndex":0,"rowWritePolicy":policy}),
    );
    let result = ok(s, |tx| submit(tx, actor, r, X));
    let Effect::TableCreated { table_id, .. } = result.effect else {
        panic!("table effect")
    };
    table_id
}
fn add(s: &mut Store, actor: Actor<'_>, crew: &str, table: &str, links: Vec<Value>) -> String {
    let mut ps = vec![
        json!({"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Original"}}}),
        json!({"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"integer","value":"-9223372036854775808"}}}),
    ];
    if !links.is_empty() {
        ps.push(json!({"fieldId":"p3","change":{"kind":"replaceLinks","values":links,"keepHiddenSetToken":null}}))
    }
    let r = request(
        s,
        crew,
        json!({"kind":"addRow","tableId":table,"expectedSchemaRevision":"1","patches":ps}),
    );
    let result = ok(s, |tx| submit(tx, actor, r, X));
    let Effect::RowAdded { row_id, .. } = result.effect else {
        panic!("row effect")
    };
    row_id
}
fn patch(s: &Store, id: &str, ps: Value) -> Submit {
    let (t, rev, _, _) = db::row_base(s.conn(), num(id, 'O').unwrap())
        .unwrap()
        .unwrap();
    request(
        s,
        &t.crew_id,
        json!({"kind":"patchRow","rowId":id,"expectedSchemaRevision":t.schema_revision,"expectedRowRevision":rev,"patches":ps}),
    )
}
fn table_link() -> Value {
    json!({"kind":"link","target":{"kind":"table"},"many":true})
}
fn value(t: Value) -> Value {
    json!({"kind":"reference","target":t})
}
fn lobby(s: &Store, crew: &str) -> String {
    s.conn()
        .query_row(
            "SELECT id FROM channels WHERE crew_id=?1 AND kind='lobby'",
            [crew],
            |r| r.get(0),
        )
        .unwrap()
}
fn message(s: &mut Store, crew: &str) -> String {
    let channel = lobby(s, crew);
    ok(s, |tx| {
        crate::messages::post(
            tx,
            crate::Sender::Human,
            crate::messages::NewPost {
                channel_id: channel,
                text: "Private synthetic message".into(),
                ..Default::default()
            },
        )
        .map(|r| r.message.id)
    })
}
#[test]
fn source_author_policy_applies_to_legacy_write_and_schema_paths() {
    let mut s = fixture();
    let t = create(&mut s, A, "crew-a", None, "authorCaptainHuman");
    let id = add(&mut s, A, "crew-a", &t, vec![]);
    let v1 = crewd_proto::managed_objects::Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: crewd_proto::managed_objects::Change::EditRow {
            row_id: id.clone(),
            expected_revision: "1".into(),
            values: vec![
                crewd_proto::managed_objects::Cell::Text("Other writer".into()),
                crewd_proto::managed_objects::Cell::Null,
            ],
        },
    };
    let before = row(s.conn(), HUMAN, &id, X).unwrap();
    assert!(matches!(
        err(&mut s, |tx| objects::submit(tx, OTHER, v1.clone())),
        CollabError::Forbidden(_)
    ));
    assert_eq!(row(s.conn(), HUMAN, &id, X).unwrap(), before);
    let schema = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":t,"expectedSchemaRevision":"1","change":{"kind":"renameTable","title":"Unauthorized"}}),
    );
    assert!(matches!(
        err(&mut s, |tx| submit(tx, OTHER, schema, X)),
        CollabError::Forbidden(_)
    ));
    let management = crate::testkit::org_captain(&mut s);
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Acting captain"}}}]),
    );
    ok(&mut s, |tx| submit(tx, Actor::Session(&management), p, X));
    assert_eq!(row(s.conn(), HUMAN, &id, X).unwrap().revision, "2");
}
#[test]
fn fresh_v1_records_real_author_and_preserves_crew_writers() {
    let mut s = fixture();
    let r=crewd_proto::managed_objects::Submit{scope:objects::scope(s.conn()).unwrap(),crew_id:"crew-a".into(),request_id:uuid::Uuid::new_v4().to_string(),change:serde_json::from_value(json!({"kind":"createTable","title":"Old client","fields":[{"name":"Value","fieldType":"real","nullable":true}]})).unwrap()};
    let result = ok(&mut s, |tx| objects::submit(tx, A, r));
    let crewd_proto::managed_objects::Object::Table { table } = result.object else {
        panic!("table")
    };
    let t = get(s.conn(), HUMAN, &table.id, X).unwrap();
    assert_eq!(
        t.author,
        Author::Session {
            session_id: "w-a1".into()
        }
    );
    assert_eq!(t.row_write_policy, RowWritePolicy::OwnerCrewWriters);
    assert_eq!(t.title_field_id, "p1");
    let r = crewd_proto::managed_objects::Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: crewd_proto::managed_objects::Change::AddRow {
            table_id: t.id.clone(),
            values: vec![crewd_proto::managed_objects::Cell::Real(-0.0)],
        },
    };
    let result = ok(&mut s, |tx| objects::submit(tx, OTHER, r));
    let crewd_proto::managed_objects::Object::Row { row: r } = result.object else {
        panic!("row")
    };
    let row = row(s.conn(), HUMAN, &r.id, X).unwrap();
    assert_eq!(
        row.author,
        Author::Session {
            session_id: "w-a2".into()
        }
    );
    let Cell::Real(n) = row.scalar_cells[0].value else {
        panic!("real")
    };
    assert_eq!(n.to_bits(), (-0.0f64).to_bits());
}
#[test]
fn stable_fields_evolve_without_rewriting_rows_and_v1_original_survives() {
    let mut s = fixture();
    let old=crewd_proto::managed_objects::Submit{scope:objects::scope(s.conn()).unwrap(),crew_id:"crew-a".into(),request_id:uuid::Uuid::new_v4().to_string(),change:serde_json::from_value(json!({"kind":"createTable","title":"Old original","fields":[{"name":"Value","fieldType":"text","nullable":true},{"name":"Other","fieldType":"text","nullable":true}]})).unwrap()};
    let receipt = ok(&mut s, |tx| objects::submit(tx, HUMAN, old.clone()));
    let crewd_proto::managed_objects::Object::Table { table: t } = &receipt.object else {
        panic!("table")
    };
    let table = t.id.clone();
    let mut revision = 1;
    for change in [
        json!({"kind":"addField","field":{"name":"Links","kind":{"kind":"link","target":{"kind":"human"},"many":false},"nullable":true}}),
        json!({"kind":"renameField","fieldId":"p2","name":"Renamed"}),
        json!({"kind":"setTitleField","fieldId":"p2"}),
        json!({"kind":"hideField","fieldId":"p1"}),
        json!({"kind":"restoreField","fieldId":"p1"}),
        json!({"kind":"widenSingleLink","fieldId":"p3"}),
    ] {
        let r = request(
            &s,
            "crew-a",
            json!({"kind":"alterTable","tableId":table,"expectedSchemaRevision":revision.to_string(),"change":change}),
        );
        ok(&mut s, |tx| submit(tx, HUMAN, r, X));
        revision += 1;
    }
    let t = get(s.conn(), HUMAN, &table, X).unwrap();
    assert_eq!(
        t.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        ["p1", "p2", "p3"]
    );
    assert_eq!(t.title_field_id, "p2");
    assert_eq!(t.schema_revision, "7");
    assert!(!t.v1_readable);
    assert!(!t.v1_writable);
    assert!(
        matches!(objects::get(s.conn(),HUMAN,&table),Err(CollabError::Conflict(ref e))if e.code==Code::ManagedV1Unsupported)
    );
    assert_eq!(ok(&mut s, |tx| objects::submit(tx, HUMAN, old)), receipt);
}
#[test]
fn patch_keep_clear_and_same_value_noop_have_exact_cas_and_no_extra_events() {
    let mut s = fixture();
    let target = create(&mut s, HUMAN, "crew-b", None, "ownerCrewWriters");
    let t = create(&mut s, A, "crew-a", Some(table_link()), "ownerCrewWriters");
    let id = add(
        &mut s,
        A,
        "crew-a",
        &t,
        vec![value(json!({"kind":"table","id":target}))],
    );
    let original = row(s.conn(), HUMAN, &id, X).unwrap();
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Original"}}},{"fieldId":"p3","change":{"kind":"keep"}}]),
    );
    let count_before = count(&s, "SELECT count(*) FROM managed_link_events");
    let (noop, changes) = ok_at(&mut s, NOW + 1, |tx| submit(tx, A, p.clone(), X));
    assert!(changes.is_empty());
    assert!(noop.event_id.is_none());
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        count_before
    );
    assert_eq!(ok(&mut s, |tx| submit(tx, A, p, X)), noop);
    assert_eq!(row(s.conn(), HUMAN, &id, X).unwrap(), original);
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"null"}}},{"fieldId":"p3","change":{"kind":"replaceLinks","values":[],"keepHiddenSetToken":null}}]),
    );
    let result = ok(&mut s, |tx| submit(tx, A, p.clone(), X));
    assert!(result.event_id.is_some());
    let after = row(s.conn(), HUMAN, &id, X).unwrap();
    assert_eq!(after.revision, "2");
    assert_eq!(after.scalar_cells[0], original.scalar_cells[0]);
    assert!(after.link_cells[0].visible.is_empty());
    let mut stale = p;
    stale.request_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        err(&mut s, |tx| submit(tx, A, stale, X)),
        CollabError::Conflict(_)
    ));
}
#[test]
fn hidden_sets_are_one_opaque_keep_bound_to_exact_cell_and_cannot_probe_capacity() {
    let mut s = fixture();
    let secret = message(&mut s, "crew-b");
    let t = create(
        &mut s,
        HUMAN,
        "crew-a",
        Some(json!({"kind":"link","target":{"kind":"anyReadable"},"many":true})),
        "ownerCrewWriters",
    );
    let id = add(
        &mut s,
        HUMAN,
        "crew-a",
        &t,
        vec![value(json!({"kind":"message","id":secret}))],
    );
    let projected = row(s.conn(), A, &id, X).unwrap();
    let cell = &projected.link_cells[0];
    assert!(cell.visible.is_empty());
    let token = cell.hidden_set.as_ref().unwrap().keep_token.clone();
    assert_eq!(token.len(), 64);
    let encoded = serde_json::to_string(&projected).unwrap();
    assert!(!encoded.contains(&secret));
    assert!(!encoded.contains("edgeId"));
    assert!(links::resolve(s.conn(), A, &TargetRef::Message { id: secret.clone() }, X).is_err());
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Allowed scalar"}}}]),
    );
    ok(&mut s, |tx| submit(tx, A, p, X));
    let updated = row(s.conn(), A, &id, X).unwrap();
    let current = updated.link_cells[0]
        .hidden_set
        .as_ref()
        .unwrap()
        .keep_token
        .clone();
    assert_ne!(current, token);
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p3","change":{"kind":"replaceLinks","values":[],"keepHiddenSetToken":current}}]),
    );
    let r = ok(&mut s, |tx| submit(tx, A, p, X));
    assert!(r.event_id.is_none());
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p3","change":{"kind":"replaceLinks","values":[{"kind":"reference","target":{"kind":"human"}}],"keepHiddenSetToken":current}}]),
    );
    assert!(matches!(
        err(&mut s, |tx| submit(tx, A, p, X)),
        CollabError::Invalid(_)
    ));
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p3","change":{"kind":"replaceLinks","values":[],"keepHiddenSetToken":token}}]),
    );
    assert!(matches!(
        err(&mut s, |tx| submit(tx, A, p, X)),
        CollabError::Conflict(_)
    ));
    let page = links::list(
        s.conn(),
        A,
        ListLinks {
            query: LinkQuery::Outgoing {
                source: SourceRef::Row { id: id.clone() },
                include_adapters: false,
            },
            after: None,
            limit: 16,
        },
        X,
    )
    .unwrap();
    assert!(page.items.is_empty());
    assert!(page.outgoing_hidden_set.is_some());
    assert!(page.next_after.is_none());
}
#[test]
fn target_purge_advances_row_and_makes_required_link_incomplete_without_ghost() {
    let mut s = fixture();
    let target = create(&mut s, HUMAN, "crew-b", None, "ownerCrewWriters");
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Required table","purpose":"No ghost","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false},{"name":"Target","kind":{"kind":"link","target":{"kind":"table"},"many":false},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters"}),
    );
    let result = ok(&mut s, |tx| submit(tx, HUMAN, r, X));
    let Effect::TableCreated { table_id, .. } = result.effect else {
        panic!()
    };
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"addRow","tableId":table_id,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Stable"}}},{"fieldId":"p2","change":{"kind":"replaceLinks","values":[value(json!({"kind":"table","id":target}))],"keepHiddenSetToken":null}}]}),
    );
    let original = r.clone();
    let result = ok(&mut s, |tx| submit(tx, HUMAN, r, X));
    let Effect::RowAdded { row_id, .. } = result.effect else {
        panic!()
    };
    ok(&mut s, |tx| {
        tx.conn().execute(
            "DELETE FROM object_refs WHERE number=?1",
            [num(&target, 'T')?],
        )?;
        Ok(())
    });
    let current = row(s.conn(), HUMAN, &row_id, X).unwrap();
    assert_eq!(current.revision, "2");
    assert!(current.link_cells[0].incomplete_required_link);
    assert!(current.link_cells[0].visible.is_empty());
    assert!(matches!(
        lookup(
            s.conn(),
            HUMAN,
            &original.scope,
            "crew-a",
            &original.request_id
        )
        .unwrap(),
        RequestLookup::Historical { receipt: None }
    ));
    assert!(
        err(&mut s, |tx| submit(tx, HUMAN, original, X))
            .refusal()
            .is_some()
    );
    let p = patch(
        &s,
        &row_id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Other field"}}}]),
    );
    ok(&mut s, |tx| submit(tx, HUMAN, p, X));
}
#[test]
fn source_and_artifact_allocation_rollback_with_receipt_failure() {
    let mut s = fixture();
    let before = count(&s, "SELECT count(*) FROM object_refs");
    s.conn().execute_batch("CREATE TRIGGER fail_v2_receipt BEFORE INSERT ON managed_v2_receipts BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;").unwrap();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Rollback","purpose":"Fault","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, A, r, X)).is_err());
    assert_eq!(count(&s, "SELECT count(*) FROM object_refs"), before);
    assert_eq!(count(&s, "SELECT count(*) FROM managed_link_events"), 0);
    assert_eq!(count(&s, "SELECT count(*) FROM managed_v2_receipts"), 0);
    assert!(
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at
            .is_none()
    );
}
#[test]
fn uuid_replay_is_original_after_later_schema_and_scope_change_never_reexecutes() {
    let mut s = fixture();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Original receipt","purpose":"Recovery","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let original = ok(&mut s, |tx| submit(tx, A, r.clone(), X));
    let Effect::TableCreated { table_id, .. } = &original.effect else {
        panic!()
    };
    let alter = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":table_id,"expectedSchemaRevision":"1","change":{"kind":"renameTable","title":"New current"}}),
    );
    ok(&mut s, |tx| submit(tx, A, alter, X));
    let count_before = count(&s, "SELECT count(*) FROM managed_link_events");
    assert_eq!(ok(&mut s, |tx| submit(tx, A, r.clone(), X)), original);
    let mut changed = r.clone();
    let Operation::CreateTable { title, .. } = &mut changed.operation else {
        panic!()
    };
    *title = "Different".into();
    assert!(
        err(&mut s, |tx| submit(tx, A, changed, X))
            .refusal()
            .is_some()
    );
    crewd_store::meta_set(
        s.conn(),
        "personalDraft.restoreStamp.v1",
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    assert!(
        matches!(lookup(s.conn(),A,&r.scope,"crew-a",&r.request_id).unwrap(),RequestLookup::Historical{receipt:Some(ref receipt)}if receipt==&original)
    );
    let mut fresh = r;
    fresh.request_id = uuid::Uuid::new_v4().to_string();
    assert!(
        err(&mut s, |tx| submit(tx, A, fresh, X))
            .refusal()
            .is_some()
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        count_before
    );
}
fn manual(s: &Store, crew: &str, source: SourceRef, expected: &str, target: Value) -> ManualSubmit {
    ManualSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: ManualOperation::Link {
            source,
            target: serde_json::from_value(target).unwrap(),
            expected_manual_link_revision: expected.into(),
        },
    }
}
#[test]
fn manual_relates_exact_replay_cas_pagination_and_column_authority_are_distinct() {
    let mut s = fixture();
    let source = create(&mut s, A, "crew-a", None, "authorCaptainHuman");
    let source = SourceRef::Table { id: source };
    let mut expected = 0;
    let mut edge_ids = Vec::new();
    for i in 0..5 {
        let target = create(&mut s, HUMAN, "crew-b", None, "ownerCrewWriters");
        let req = manual(
            &s,
            "crew-a",
            source.clone(),
            &expected.to_string(),
            value(json!({"kind":"table","id":target})),
        );
        let before = count(&s, "SELECT count(*) FROM managed_link_events");
        let (receipt, changes) = ok_at(&mut s, NOW + i, |tx| links::submit(tx, A, req.clone(), X));
        assert_eq!(receipt.original_manual_link_revision, expected.to_string());
        expected += 1;
        assert_eq!(receipt.result_manual_link_revision, expected.to_string());
        assert!(
            changes
                .iter()
                .any(|e| matches!(e,Change::ManagedLinksChanged{crew_id}if crew_id=="crew-a"))
        );
        let ManualEffect::Linked {
            edge_id,
            added: true,
        } = receipt.effect.clone()
        else {
            panic!()
        };
        edge_ids.push(edge_id);
        assert_eq!(ok(&mut s, |tx| links::submit(tx, A, req, X)), receipt);
        assert_eq!(
            count(&s, "SELECT count(*) FROM managed_link_events"),
            before + 1
        );
    }
    let mut after = None;
    let mut collected = Vec::new();
    loop {
        let page = links::list(
            s.conn(),
            HUMAN,
            ListLinks {
                query: LinkQuery::Outgoing {
                    source: source.clone(),
                    include_adapters: false,
                },
                after: after.clone(),
                limit: 2,
            },
            X,
        )
        .unwrap();
        collected.extend(page.items.iter().map(|v| v.id.clone()));
        if page.next_after.is_none() {
            break;
        }
        after = page.next_after;
    }
    assert_eq!(collected, edge_ids);
    let cursor = links::list(
        s.conn(),
        HUMAN,
        ListLinks {
            query: LinkQuery::Outgoing {
                source: source.clone(),
                include_adapters: false,
            },
            after: None,
            limit: 2,
        },
        X,
    )
    .unwrap()
    .next_after
    .unwrap();
    assert!(
        links::list(
            s.conn(),
            HUMAN,
            ListLinks {
                query: LinkQuery::Incoming {
                    target: TargetRef::Human {},
                    include_adapters: false
                },
                after: Some(cursor),
                limit: 2
            },
            X
        )
        .is_err()
    );
    let forbidden = manual(
        &s,
        "crew-a",
        source.clone(),
        "5",
        value(json!({"kind":"human"})),
    );
    assert!(matches!(
        err(&mut s, |tx| links::submit(tx, OTHER, forbidden, X)),
        CollabError::Forbidden(_)
    ));
    let edge = edge_ids[0].clone();
    let req = ManualSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: ManualOperation::Unlink {
            source: source.clone(),
            expected_manual_link_revision: "4".into(),
            expected_edge_id: edge.clone(),
        },
    };
    assert!(matches!(
        err(&mut s, |tx| links::submit(tx, A, req, X)),
        CollabError::Conflict(_)
    ));
    let req = ManualSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: ManualOperation::Unlink {
            source,
            expected_manual_link_revision: "5".into(),
            expected_edge_id: edge,
        },
    };
    ok(&mut s, |tx| links::submit(tx, A, req, X));
}
#[test]
fn artifact_source_hold_survives_other_crew_purge_and_last_edge_collects_anchor() {
    let mut s = fixture();
    let input = crewd_proto::managed_artifacts::Input {
        kind: crewd_proto::managed_artifacts::Kind::File,
        value: "src/retained.rs".into(),
        repository: None,
    };
    let register = crewd_proto::managed_artifacts::Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: crewd_proto::managed_artifacts::Change::Register { input },
    };
    let receipt = ok(&mut s, |tx| {
        crate::managed_artifacts::submit(tx, HUMAN, register)
    });
    let reference = receipt.artifact.reference;
    let target = create(&mut s, HUMAN, "crew-b", None, "ownerCrewWriters");
    let source = SourceRef::Artifact {
        reference: reference.clone(),
        metadata_crew_id: "crew-b".into(),
    };
    let r = manual(
        &s,
        "crew-b",
        source.clone(),
        "0",
        value(json!({"kind":"table","id":target})),
    );
    let receipt = ok(&mut s, |tx| links::submit(tx, Actor::Session("w-b1"), r, X));
    let ManualEffect::Linked { edge_id, .. } = &receipt.effect else {
        panic!()
    };
    ok(&mut s, |tx| {
        tx.conn()
            .execute("DELETE FROM crews WHERE id='crew-a'", [])?;
        Ok(())
    });
    assert!(
        crewd_store::managed_artifacts::get(s.conn(), &reference)
            .unwrap()
            .is_some()
    );
    assert_eq!(graph::outgoing(s.conn(), &source).unwrap().len(), 1);
    let unlink = ManualSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-b".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: ManualOperation::Unlink {
            source: source.clone(),
            expected_manual_link_revision: "1".into(),
            expected_edge_id: edge_id.clone(),
        },
    };
    ok(&mut s, |tx| {
        links::submit(tx, Actor::Session("w-b1"), unlink, X)
    });
    assert!(
        crewd_store::managed_artifacts::get(s.conn(), &reference)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        graph::stamp(s.conn(), &source, "crew-b").unwrap(),
        2,
        "nonholding stamp survives anchor GC"
    );
}
#[test]
fn original_receipt_and_real_rows_survive_disk_reopen_and_official_restore() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap();
    let backup = data.join("retained.db");
    let mut s = fixture();
    let t = create(&mut s, A, "crew-a", None, "authorCaptainHuman");
    let id = add(&mut s, A, "crew-a", &t, vec![]);
    let req = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Committed original"}}}]),
    );
    let original = ok(&mut s, |tx| submit(tx, A, req.clone(), X));
    let current = row(s.conn(), A, &id, X).unwrap();
    crewd_store::meta_set(s.conn(), "knowledge.sqlite.v1", "1").unwrap();
    s.conn()
        .execute(
            "VACUUM INTO ?1",
            [data.join(crewd_store::DB_FILE).to_str().unwrap()],
        )
        .unwrap();
    drop(s);
    crewd_store::team_data::private_dir(&data.join("knowledge")).unwrap();
    let s = Store::open_data_dir(&data).unwrap();
    assert!(
        matches!(lookup(s.conn(),A,&req.scope,"crew-a",&req.request_id).unwrap(),RequestLookup::Known{ref receipt}if receipt==&original)
    );
    assert_eq!(row(s.conn(), A, &id, X).unwrap(), current);
    drop(s);
    crewd_store::backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &backup).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let later = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Later current"}}}]),
    );
    ok(&mut s, |tx| submit(tx, A, later, X));
    drop(s);
    crewd_store::backup::restore(&data, &backup).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    assert_eq!(row(s.conn(), A, &id, X).unwrap(), current);
    assert!(
        matches!(lookup(s.conn(),A,&req.scope,"crew-a",&req.request_id).unwrap(),RequestLookup::Historical{receipt:Some(ref receipt)}if receipt==&original)
    );
    let before = count(&s, "SELECT count(*) FROM managed_link_events");
    let mut forbidden = req;
    forbidden.request_id = uuid::Uuid::new_v4().to_string();
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, A, forbidden, X)).is_err());
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        before
    );
}
#[test]
fn receipt_corruption_never_masquerades_as_original_or_uses_current_row() {
    let mut s = fixture();
    let t = create(&mut s, A, "crew-a", None, "authorCaptainHuman");
    let id = add(&mut s, A, "crew-a", &t, vec![]);
    let r = patch(
        &s,
        &id,
        json!([{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Known"}}}]),
    );
    let receipt = ok(&mut s, |tx| submit(tx, A, r.clone(), X));
    let raw: String = s
        .conn()
        .query_row(
            "SELECT result_json FROM managed_v2_receipts WHERE request_id=?1",
            [&r.request_id],
            |r| r.get(0),
        )
        .unwrap();
    let mut json: Value = serde_json::from_str(&raw).unwrap();
    json["result"]["effect"]["resultRowRevision"] = json!("999");
    s.conn()
        .execute(
            "UPDATE managed_v2_receipts SET result_json=?2 WHERE request_id=?1",
            rusqlite::params![r.request_id, serde_json::to_string(&json).unwrap()],
        )
        .unwrap();
    assert!(lookup(s.conn(), A, &r.scope, "crew-a", &r.request_id).is_err());
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, A, r.clone(), X)).is_err());
    s.conn()
        .execute(
            "UPDATE managed_v2_receipts SET result_json=?2 WHERE request_id=?1",
            rusqlite::params![r.request_id, raw],
        )
        .unwrap();
    assert!(
        matches!(lookup(s.conn(),A,&r.scope,"crew-a",&r.request_id).unwrap(),RequestLookup::Known{receipt:r}if r==receipt)
    );
}
#[test]
fn internally_tagged_unit_inputs_are_closed_including_recursive_field_shapes() {
    for bad in [
        json!({"kind":"human","id":"forged"}),
        json!({"kind":"human","extra":null}),
    ] {
        assert!(serde_json::from_value::<TargetRef>(bad).is_err());
    }
    for bad in [
        json!({"kind":"text","target":{"kind":"human"}}),
        json!({"kind":"integer","many":true}),
    ] {
        assert!(serde_json::from_value::<FieldKind>(bad).is_err());
    }
    assert!(serde_json::from_value::<CellPatch>(json!({"kind":"keep","values":[]})).is_err());
    assert!(
        serde_json::from_value::<TargetConstraint>(json!({"kind":"work","tableId":"T1"})).is_err()
    );
}
#[test]
fn newly_added_nullable_missing_cell_null_is_effective_noop_and_schema_audit_is_exact() {
    let mut s = fixture();
    let t = create(&mut s, A, "crew-a", None, "authorCaptainHuman");
    let id = add(&mut s, A, "crew-a", &t, vec![]);
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":t,"expectedSchemaRevision":"1","change":{"kind":"addField","field":{"name":"Optional","kind":{"kind":"text"},"nullable":true}}}),
    );
    let altered = ok(&mut s, |tx| submit(tx, A, r, X));
    let audit: (String, String) = s
        .conn()
        .query_row(
            "SELECT before_json,after_json FROM managed_table_schema_events WHERE event_id=?1",
            [altered.event_id.as_ref().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let before: Table = serde_json::from_str(&audit.0).unwrap();
    let after: Table = serde_json::from_str(&audit.1).unwrap();
    assert_eq!(before.fields.len(), 2);
    assert_eq!(after.fields.len(), 3);
    assert_eq!(before.schema_revision, "1");
    assert_eq!(after.schema_revision, "2");
    let original = row(s.conn(), A, &id, X).unwrap();
    let before = count(&s, "SELECT count(*) FROM managed_link_events");
    let p = patch(
        &s,
        &id,
        json!([{"fieldId":"p3","change":{"kind":"setScalar","value":{"type":"null"}}}]),
    );
    let (r, changes) = ok_at(&mut s, NOW + 500, |tx| submit(tx, A, p, X));
    assert!(r.event_id.is_none());
    assert!(changes.is_empty());
    assert_eq!(row(s.conn(), A, &id, X).unwrap(), original);
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        before
    );
    assert!(
        !db::scalars(s.conn(), num(&id, 'O').unwrap())
            .unwrap()
            .iter()
            .any(|v| v.field_id == "p3")
    );
}
#[test]
fn hiding_nonnullable_field_refuses_instead_of_stranding_future_row_creation() {
    let mut s = fixture();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Required fields","purpose":"No impossible rows","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false},{"name":"Required","kind":{"kind":"integer"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters"}),
    );
    let created = ok(&mut s, |tx| submit(tx, A, r, X));
    let Effect::TableCreated { table_id, .. } = created.effect else {
        panic!()
    };
    let before = get(s.conn(), A, &table_id, X).unwrap();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":table_id,"expectedSchemaRevision":"1","change":{"kind":"hideField","fieldId":"p2"}}),
    );
    assert!(matches!(
        err(&mut s, |tx| submit(tx, A, r, X)),
        CollabError::Invalid(_)
    ));
    assert_eq!(get(s.conn(), A, &table_id, X).unwrap(), before);
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_table_schema_events"),
        0
    );
    let id = add(&mut s, A, "crew-a", &table_id, vec![]);
    assert_eq!(row(s.conn(), A, &id, X).unwrap().revision, "1");
}
#[test]
fn one_real_crew_purge_clears_two_targets_and_advances_surviving_source_once() {
    let mut s = fixture();
    let a = create(&mut s, HUMAN, "crew-x", None, "ownerCrewWriters");
    let b = create(&mut s, HUMAN, "crew-x", None, "ownerCrewWriters");
    let table = create(&mut s, A, "crew-a", Some(table_link()), "ownerCrewWriters");
    let id = add(
        &mut s,
        A,
        "crew-a",
        &table,
        vec![
            value(json!({"kind":"table","id":a})),
            value(json!({"kind":"table","id":b})),
        ],
    );
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    let ((), changes) = ok_at(&mut s, NOW + 900, |tx| crate::crews::purge(tx, "crew-x"));
    let after = row(s.conn(), A, &id, X).unwrap();
    assert_eq!(after.revision, "2");
    assert!(after.link_cells[0].visible.is_empty());
    assert_eq!(
        changes
            .iter()
            .filter(|v| matches!(v,Change::ManagedLinksChanged{crew_id}if crew_id=="crew-a"))
            .count(),
        1
    );
    assert!(graph::deletions_empty(s.conn()).unwrap());
    assert_eq!(count(&s, "SELECT count(*) FROM managed_links"), 0);
}
#[test]
fn counter_exhaustion_refuses_whole_deletion_transaction_without_sqlite_real_promotion() {
    let mut s = fixture();
    let target = create(&mut s, HUMAN, "crew-b", None, "ownerCrewWriters");
    let table = create(&mut s, A, "crew-a", Some(table_link()), "ownerCrewWriters");
    let id = add(
        &mut s,
        A,
        "crew-a",
        &table,
        vec![value(json!({"kind":"table","id":target}))],
    );
    s.conn()
        .execute(
            "UPDATE managed_rows SET revision=9223372036854775807 WHERE number=?1",
            [num(&id, 'O').unwrap()],
        )
        .unwrap();
    let before = count(&s, "SELECT count(*) FROM managed_links");
    assert!(
        crate::run(&mut s, NOW, |tx| {
            tx.conn().execute(
                "DELETE FROM object_refs WHERE number=?1",
                [num(&target, 'T')?],
            )?;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(count(&s, "SELECT count(*) FROM managed_links"), before);
    assert!(
        db::table(s.conn(), num(&target, 'T').unwrap())
            .unwrap()
            .is_some()
    );
    let typ: String = s
        .conn()
        .query_row(
            "SELECT typeof(revision) FROM managed_rows WHERE number=?1",
            [num(&id, 'O').unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(typ, "integer");
    assert!(graph::deletions_empty(s.conn()).unwrap());
}
#[test]
fn unadvertised_knowledge_constraints_refuse_empty_columns_while_core_any_readable_stays_usable() {
    let mut s = fixture();
    for kind in ["document", "knowledgeView"] {
        let before = count(&s, "SELECT count(*) FROM object_refs");
        let r = request(
            &s,
            "crew-a",
            json!({"kind":"createTable","title":"Unavailable target","purpose":"No false capability","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false},{"name":"Private","kind":{"kind":"link","target":{"kind":kind},"many":true},"nullable":true}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters"}),
        );
        assert!(matches!(
            err(&mut s, |tx| submit(tx, A, r, X)),
            CollabError::Invalid(_)
        ));
        assert_eq!(count(&s, "SELECT count(*) FROM object_refs"), before);
    }
    let table = create(
        &mut s,
        A,
        "crew-a",
        Some(json!({"kind":"link","target":{"kind":"anyReadable"},"many":true})),
        "ownerCrewWriters",
    );
    let before = get(s.conn(), A, &table, X).unwrap();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":table,"expectedSchemaRevision":"1","change":{"kind":"addField","field":{"name":"Private","kind":{"kind":"link","target":{"kind":"document"},"many":false},"nullable":true}}}),
    );
    assert!(matches!(
        err(&mut s, |tx| submit(tx, A, r, X)),
        CollabError::Invalid(_)
    ));
    assert_eq!(get(s.conn(), A, &table, X).unwrap(), before);
    let id = add(
        &mut s,
        A,
        "crew-a",
        &table,
        vec![value(json!({"kind":"human"}))],
    );
    assert_eq!(
        row(s.conn(), A, &id, X).unwrap().link_cells[0].visible[0].target,
        TargetRef::Human {}
    );
}
#[test]
fn archived_core_target_accepts_new_reference_but_archived_source_rejects_mutation() {
    let mut s = fixture();
    let target = create(&mut s, HUMAN, "crew-x", None, "ownerCrewWriters");
    let source = create(
        &mut s,
        Actor::Session("w-b1"),
        "crew-b",
        None,
        "authorCaptainHuman",
    );
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    let source = SourceRef::Table { id: source };
    let r = manual(
        &s,
        "crew-b",
        source.clone(),
        "0",
        value(json!({"kind":"table","id":target})),
    );
    let receipt = ok(&mut s, |tx| {
        links::submit(tx, Actor::Session("w-b1"), r.clone(), X)
    });
    assert!(receipt.event_id.is_some());
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
    let page = links::list(
        s.conn(),
        Actor::Session("w-b1"),
        ListLinks {
            query: LinkQuery::Outgoing {
                source: source.clone(),
                include_adapters: false,
            },
            after: None,
            limit: 16,
        },
        X,
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(!page.items[0].editable);
    assert_eq!(
        ok(&mut s, |tx| links::submit(tx, Actor::Session("w-b1"), r, X)),
        receipt
    );
    let r = manual(&s, "crew-b", source, "1", value(json!({"kind":"human"})));
    assert!(matches!(
        err(&mut s, |tx| links::submit(tx, Actor::Session("w-b1"), r, X)),
        CollabError::Conflict(_) | CollabError::Forbidden(_)
    ));
}
#[test]
fn adapter_reverse_scan_has_a_physical_source_budget_and_one_shared_checked_book() {
    let mut s = fixture();
    for i in 0..70 {
        let p=serde_json::from_value(json!({"crewId":"crew-a","title":format!("Bounded source {i}"),"owner":"w-a1","goal":"Synthetic","acceptance":"Check"})).unwrap();
        ok(&mut s, |tx| {
            crate::work::create(tx, &crate::Sender::Human, p)
        });
    }
    let q = ListLinks {
        query: LinkQuery::Incoming {
            target: TargetRef::Human {},
            include_adapters: true,
        },
        after: None,
        limit: 1,
    };
    let (page, stats) = links::list_measured(s.conn(), HUMAN, q.clone(), X).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(
        stats.sources_visited,
        usize::from(links::ADAPTER_SOURCE_BUDGET)
    );
    assert_eq!(
        stats.workbook_loads, 1,
        "one shared checked cut, not one full parse per W"
    );
    assert!(
        page.next_after.is_some(),
        "bounded empty page must not claim the organization was fully scanned"
    );
    let mut q = q;
    q.after = page.next_after;
    let (page, stats) = links::list_measured(s.conn(), HUMAN, q, X).unwrap();
    assert!(page.items.is_empty());
    assert!(stats.sources_visited <= usize::from(links::ADAPTER_SOURCE_BUDGET));
    assert_eq!(stats.workbook_loads, 1);
    assert!(page.next_after.is_none());
}
#[test]
fn adapter_reverse_cursor_resumes_inside_one_source_without_duplicate_or_missing_edges() {
    let mut s = fixture();
    let p=serde_json::from_value(json!({"crewId":"crew-a","title":"Referenced work","goal":"Synthetic","acceptance":"Check"})).unwrap();
    let work = ok(&mut s, |tx| {
        crate::work::create(tx, &crate::Sender::Human, p)
    });
    let r=crewd_proto::managed_objects::Submit{scope:objects::scope(s.conn()).unwrap(),crew_id:"crew-a".into(),request_id:uuid::Uuid::new_v4().to_string(),change:serde_json::from_value(json!({"kind":"recordRisk","draft":{"summary":"Two different authoritative relations","impact":"low","likelihood":null,"mitigation":null,"about":[{"kind":"work","id":work.id}],"mitigatedBy":[{"kind":"work","id":work.id}]},"owner":{"kind":"human"}})).unwrap()};
    ok(&mut s, |tx| objects::submit(tx, HUMAN, r));
    let mut after = None;
    let mut ids = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 10);
        let (page, stats) = links::list_measured(
            s.conn(),
            HUMAN,
            ListLinks {
                query: LinkQuery::Incoming {
                    target: TargetRef::Work {
                        id: work.id.clone(),
                    },
                    include_adapters: true,
                },
                after,
                limit: 1,
            },
            X,
        )
        .unwrap();
        assert!(stats.sources_visited <= 64);
        assert_eq!(stats.workbook_loads, 1);
        ids.extend(page.items.into_iter().map(|v| v.id));
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.iter().all(|v| v.starts_with("adapter:")));
}
#[test]
fn unbounded_legacy_artifact_child_table_uses_real_raw_budget_and_exact_keyset_continuation() {
    let mut s = fixture();
    let p = serde_json::from_value(
        json!({"crewId":"crew-a","title":"Many preserved pointers","owner":"w-a1"}),
    )
    .unwrap();
    let work = ok(&mut s, |tx| {
        crate::work::create(tx, &crate::Sender::Human, p)
    });
    let mut last_ref = None;
    for i in 0..130 {
        let r = crewd_proto::managed_artifacts::Submit {
            scope: objects::scope(s.conn()).unwrap(),
            crew_id: "crew-a".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change: crewd_proto::managed_artifacts::Change::Link {
                input: crewd_proto::managed_artifacts::Input {
                    kind: crewd_proto::managed_artifacts::Kind::Link,
                    value: format!("https://bounded.example/{i}"),
                    repository: None,
                },
                subject: crewd_proto::managed_artifacts::Subject::Work {
                    id: work.id.clone(),
                },
                relation: crewd_proto::managed_artifacts::Relation::Evidence,
            },
        };
        let receipt = ok(&mut s, |tx| crate::managed_artifacts::submit(tx, HUMAN, r));
        last_ref = Some(receipt.artifact.reference);
    }
    // A second real provenance for the first same reference/relation must consume
    // a raw slot without duplicating its public adapter identity on later pages.
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &crate::Sender::Session("w-a1".into()),
            crewd_proto::WorkUpdateParams {
                id: work.id.clone(),
                expected_revision: work.revision.clone(),
                status: Some(crewd_proto::WorkUpdateStatus::Done),
                evidence: Some(crewd_proto::WorkEvidenceParam {
                    kind: crewd_proto::WorkEvidenceKind::Url,
                    value: "https://bounded.example/0".into(),
                }),
                ..Default::default()
            },
        )
    });
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_artifact_edges"),
        131
    );
    let source = SourceRef::Work {
        id: work.id.clone(),
    };
    let mut after = None;
    let mut ids = std::collections::HashSet::new();
    let mut artifact_count = 0;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 10);
        let (page, stats) = links::list_measured(
            s.conn(),
            HUMAN,
            ListLinks {
                query: LinkQuery::Outgoing {
                    source: source.clone(),
                    include_adapters: true,
                },
                after,
                limit: 64,
            },
            X,
        )
        .unwrap();
        assert!(stats.raw_rows <= 64);
        assert_eq!(stats.workbook_loads, 1);
        for edge in page.items {
            assert!(ids.insert(edge.id));
            if matches!(edge.target, TargetRef::Artifact { .. }) {
                artifact_count += 1
            }
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(artifact_count, 130);
    assert!(pages >= 3);
    let target = TargetRef::Artifact {
        reference: last_ref.unwrap(),
    };
    let mut after = None;
    let mut found = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 10);
        let (page, stats) = links::list_measured(
            s.conn(),
            HUMAN,
            ListLinks {
                query: LinkQuery::Incoming {
                    target: target.clone(),
                    include_adapters: true,
                },
                after,
                limit: 1,
            },
            X,
        )
        .unwrap();
        assert!(stats.raw_rows <= 64);
        assert!(stats.sources_visited <= 64);
        assert_eq!(stats.workbook_loads, 1);
        if pages == 1 {
            assert!(page.items.is_empty());
            assert!(page.next_after.is_some());
            assert_eq!(stats.raw_rows, 64);
        }
        found.extend(page.items.into_iter().map(|v| v.id));
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(found.len(), 1);
    assert!(pages >= 3);
}
#[test]
fn authoritative_decision_replacement_h_origin_and_occurrence_project_readonly_without_new_business_effects()
 {
    let mut s = fixture();
    let call = |s: &mut Store, change: Value| {
        let r = crewd_proto::managed_objects::Submit {
            scope: objects::scope(s.conn()).unwrap(),
            crew_id: "crew-a".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change: serde_json::from_value(change).unwrap(),
        };
        ok(s, |tx| objects::submit(tx, HUMAN, r))
    };
    let draft = json!({"question":"Which route?","choice":"A","rationale":"Original fact","alternatives":null,"reversibility":null,"about":[{"kind":"crew","id":"crew-a"}]});
    let old = call(&mut s, json!({"kind":"recordDecision","draft":draft}));
    let crewd_proto::managed_objects::Object::Decision { decision: old } = old.object else {
        panic!()
    };
    let new = call(
        &mut s,
        json!({"kind":"replaceDecision","decisionId":old.id,"expectedRevision":"1","draft":draft}),
    );
    let crewd_proto::managed_objects::Object::Decision { decision: new } = new.object else {
        panic!()
    };
    let r=crewd_proto::decision_asks::Open{scope:objects::scope(s.conn()).unwrap(),request_id:uuid::Uuid::new_v4().to_string(),draft:serde_json::from_value(json!({"question":"Choose exact option?","context":"Synthetic structured request","options":[{"label":"First"},{"label":"Second"}],"about":[{"kind":"crew","id":"crew-a"}],"default":null,"reversibility":null})).unwrap()};
    let (opened, _) = ok(&mut s, |tx| crate::decision_asks::open(tx, "w-a1", r));
    ok(&mut s, |tx| {
        crate::asks::answer(tx, "crew-a", &opened.opened.ask, "A")
    });
    let captured = crate::decision_asks::get(s.conn(), "crew-a", &opened.opened.ask)
        .unwrap()
        .unwrap()
        .capture
        .unwrap();
    let risk = call(
        &mut s,
        json!({"kind":"recordRisk","draft":{"summary":"Occurrence source","impact":"low","likelihood":null,"mitigation":null,"about":[{"kind":"crew","id":"crew-a"}],"mitigatedBy":[]},"owner":{"kind":"human"}}),
    );
    let crewd_proto::managed_objects::Object::Risk { risk } = risk.object else {
        panic!()
    };
    let occurrence = call(
        &mut s,
        json!({"kind":"recordRiskOccurrence","riskId":risk.id,"expectedRevision":"1","lessonDraft":{"happened":"Actual synthetic occurrence","lesson":"Keep exact source","detection":null,"learnedFrom":[],"evidence":[],"appliesTo":[]}}),
    );
    let lesson = occurrence.occurrence.unwrap().lesson_id;
    let before = (
        count(&s, "SELECT count(*) FROM object_refs"),
        count(&s, "SELECT count(*) FROM messages"),
        count(&s, "SELECT count(*) FROM wakes"),
        count(&s, "SELECT count(*) FROM organization_work_events"),
        count(&s, "SELECT count(*) FROM managed_link_events"),
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
    );
    let projections = [
        (
            SourceRef::Decision { id: old.id.clone() },
            "decision.supersededBy",
            TargetRef::Decision { id: new.id.clone() },
        ),
        (
            SourceRef::Decision { id: new.id },
            "decision.supersedes",
            TargetRef::Decision { id: old.id },
        ),
        (
            SourceRef::Decision {
                id: captured.decision_id.clone(),
            },
            "decision.origin",
            TargetRef::Ask {
                id: opened.opened.ask.clone(),
            },
        ),
        (
            SourceRef::Risk {
                id: risk.id.clone(),
            },
            "risk.becameLesson",
            TargetRef::Lesson { id: lesson.clone() },
        ),
        (
            SourceRef::Lesson { id: lesson },
            "lesson.risk",
            TargetRef::Risk { id: risk.id },
        ),
    ];
    for (source, name, target) in projections {
        let q = ListLinks {
            query: LinkQuery::Outgoing {
                source,
                include_adapters: true,
            },
            after: None,
            limit: 64,
        };
        let page = links::list(s.conn(), HUMAN, q.clone(), X).unwrap();
        let edge = page
            .items
            .iter()
            .find(|e| matches!(&e.origin,EdgeOrigin::Adapter{adapter,..}if adapter==name))
            .unwrap();
        assert_eq!(edge.target, target);
        assert!(!edge.editable);
        assert!(edge.id.starts_with("adapter:"));
        assert_eq!(links::list(s.conn(), HUMAN, q, X).unwrap(), page);
    }
    let private = links::list(
        s.conn(),
        Actor::Session("w-b1"),
        ListLinks {
            query: LinkQuery::Outgoing {
                source: SourceRef::Decision {
                    id: captured.decision_id,
                },
                include_adapters: true,
            },
            after: None,
            limit: 64,
        },
        X,
    )
    .unwrap();
    assert!(
        !serde_json::to_string(&private)
            .unwrap()
            .contains(&opened.opened.ask)
    );
    let after = (
        count(&s, "SELECT count(*) FROM object_refs"),
        count(&s, "SELECT count(*) FROM messages"),
        count(&s, "SELECT count(*) FROM wakes"),
        count(&s, "SELECT count(*) FROM organization_work_events"),
        count(&s, "SELECT count(*) FROM managed_link_events"),
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
    );
    assert_eq!(after, before);
}
#[test]
fn evolved_v2_row_preserves_original_message_source_and_uuid_without_using_legacy_object_codec() {
    let mut s = fixture();
    let table = create(&mut s, A, "crew-a", None, "ownerCrewWriters");
    let message = message(&mut s, "crew-a");
    let source = crewd_proto::managed_message_sources::RecordParams {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        source_message_id: message,
        change: crewd_proto::managed_objects::Change::AddRow {
            table_id: table.clone(),
            values: vec![
                crewd_proto::managed_objects::Cell::Text("Captured original".into()),
                crewd_proto::managed_objects::Cell::Null,
            ],
        },
    };
    let saved = ok(&mut s, |tx| {
        crate::managed_message_sources::record(
            tx,
            A,
            serde_json::from_value(serde_json::to_value(&source).unwrap()).unwrap(),
        )
    });
    let crewd_proto::managed_objects::Object::Row { row: old } = &saved.object else {
        panic!()
    };
    let row_id = old.id.clone();
    let original = crate::managed_message_sources::source(s.conn(), A, &row_id).unwrap();
    let original_bytes: String = s
        .conn()
        .query_row(
            "SELECT result_json FROM managed_message_receipts WHERE request_id=?1",
            [&source.request_id],
            |r| r.get(0),
        )
        .unwrap();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":table,"expectedSchemaRevision":"1","change":{"kind":"addField","field":{"name":"New link","kind":{"kind":"link","target":{"kind":"human"},"many":false},"nullable":true}}}),
    );
    ok(&mut s, |tx| submit(tx, A, r, X));
    assert!(
        matches!(objects::get(s.conn(),A,&row_id),Err(CollabError::Conflict(ref r))if r.code==Code::ManagedV1Unsupported)
    );
    assert_eq!(
        crate::managed_message_sources::source(s.conn(), A, &row_id).unwrap(),
        original
    );
    assert!(
        matches!(crate::managed_message_sources::lookup(s.conn(),A,&source.scope,"crew-a",&source.request_id).unwrap(),crewd_proto::managed_message_sources::Lookup::Known{receipt}if serde_json::to_value(&receipt).unwrap()==serde_json::to_value(&saved).unwrap())
    );
    let (before, facts) = ok_at(&mut s, NOW + 500, |tx| {
        crate::managed_message_sources::record(
            tx,
            A,
            serde_json::from_value(serde_json::to_value(&source).unwrap()).unwrap(),
        )
    });
    assert!(facts.is_empty());
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(saved).unwrap()
    );
    assert_eq!(
        s.conn()
            .query_row::<String, _, _>(
                "SELECT result_json FROM managed_message_receipts WHERE request_id=?1",
                [&source.request_id],
                |r| r.get(0)
            )
            .unwrap(),
        original_bytes
    );
    assert!(
        matches!(crate::managed_message_sources::objects_from_message(s.conn(),A,&source.source_message_id,None),Err(CollabError::Conflict(ref r))if r.code==Code::ManagedV1Unsupported),
        "legacy Object[] must not contain a V2 row disguised as V1"
    );
    assert_eq!(
        crate::managed_message_sources::source(s.conn(), Actor::Session("w-b1"), &row_id).unwrap(),
        crewd_proto::managed_message_sources::Source::Restricted
    );
}
#[test]
fn v2_aware_reference_authority_keeps_artifact_decision_and_lesson_targets_usable() {
    let mut s = fixture();
    let table = create(&mut s, A, "crew-a", Some(table_link()), "ownerCrewWriters");
    let id = add(&mut s, A, "crew-a", &table, vec![]);
    let r = crewd_proto::managed_artifacts::Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change: crewd_proto::managed_artifacts::Change::Link {
            input: crewd_proto::managed_artifacts::Input {
                kind: crewd_proto::managed_artifacts::Kind::File,
                value: "src/reference.rs".into(),
                repository: None,
            },
            subject: crewd_proto::managed_artifacts::Subject::Row { id: id.clone() },
            relation: crewd_proto::managed_artifacts::Relation::Relates,
        },
    };
    let saved = ok(&mut s, |tx| crate::managed_artifacts::submit(tx, A, r));
    let refs = crate::managed_artifacts::subject(
        s.conn(),
        Actor::Session("w-b1"),
        &crewd_proto::managed_artifacts::Subject::Row { id: id.clone() },
        None,
    )
    .unwrap();
    assert_eq!(refs.items.len(), 1);
    let reverse =
        crate::managed_artifacts::backlinks(s.conn(), HUMAN, &saved.artifact.reference, None)
            .unwrap();
    assert_eq!(reverse.items.len(), 1);
    let call = |s: &mut Store, change: Value| {
        let r = crewd_proto::managed_objects::Submit {
            scope: objects::scope(s.conn()).unwrap(),
            crew_id: "crew-a".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change: serde_json::from_value(change).unwrap(),
        };
        ok(s, |tx| objects::submit(tx, HUMAN, r))
    };
    let d = call(
        &mut s,
        json!({"kind":"recordDecision","draft":{"question":"Reference current row?","choice":"Yes","rationale":"Typed existence only","alternatives":null,"reversibility":null,"about":[{"kind":"row","id":id}]}}),
    );
    assert!(matches!(
        d.object,
        crewd_proto::managed_objects::Object::Decision { .. }
    ));
    let l = call(
        &mut s,
        json!({"kind":"recordLesson","draft":{"happened":"Existing structured table","lesson":"Reference without legacy body","detection":null,"learnedFrom":[],"evidence":[],"appliesTo":[{"kind":"table","id":table}]}}),
    );
    assert!(matches!(
        l.object,
        crewd_proto::managed_objects::Object::Lesson { .. }
    ));
    assert!(objects::read_authority(s.conn(), HUMAN, "T999999999").is_err());
    assert!(objects::read_authority(s.conn(), A, "O01").is_err());
}
#[test]
fn real_positive_and_negative_zero_roundtrip_as_distinct_changes_with_exact_noop_replay() {
    let mut s = fixture();
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Signed zero","purpose":"Exact IEEE storage","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false},{"name":"Reading","kind":{"kind":"real"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    let result = ok(&mut s, |tx| submit(tx, A, r, X));
    let Effect::TableCreated { table_id, .. } = result.effect else {
        panic!()
    };
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"addRow","tableId":table_id,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Measured"}}},{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"real","value":0.0f64}}}]}),
    );
    let result = ok_at(&mut s, NOW + 1, |tx| submit(tx, A, r, X)).0;
    let Effect::RowAdded { row_id, .. } = result.effect else {
        panic!()
    };
    let bits = |s: &Store| {
        let cells = db::scalars(s.conn(), num(&row_id, 'O').unwrap()).unwrap();
        let Cell::Real(value) = cells.iter().find(|v| v.field_id == "p2").unwrap().value else {
            panic!()
        };
        value.to_bits()
    };
    assert_eq!(bits(&s), 0.0f64.to_bits());
    let count_before = count(&s, "SELECT count(*) FROM managed_link_events");
    let negative = patch(
        &s,
        &row_id,
        json!([{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"real","value":-0.0f64}}}]),
    );
    let (negative_receipt, changes) =
        ok_at(&mut s, NOW + 2, |tx| submit(tx, A, negative.clone(), X));
    assert_eq!(bits(&s), (-0.0f64).to_bits());
    assert_eq!(row(s.conn(), A, &row_id, X).unwrap().revision, "2");
    assert!(negative_receipt.event_id.is_some());
    assert!(changes.iter().any(|f|matches!(f,Change::Progress{session_id,kind:crate::ProgressKind::Ontology}if session_id=="w-a1")));
    let progress = crate::sessions::get(s.conn(), "w-a1")
        .unwrap()
        .last_progress_at;
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        count_before + 1
    );
    let same = patch(
        &s,
        &row_id,
        json!([{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"real","value":-0.0f64}}}]),
    );
    let (same_receipt, changes) = ok_at(&mut s, NOW + 3, |tx| submit(tx, A, same, X));
    assert!(same_receipt.event_id.is_none());
    assert!(changes.is_empty());
    assert_eq!(bits(&s), (-0.0f64).to_bits());
    assert_eq!(row(s.conn(), A, &row_id, X).unwrap().revision, "2");
    assert_eq!(
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
        progress
    );
    let (replayed, changes) = ok_at(&mut s, NOW + 4, |tx| submit(tx, A, negative, X));
    assert_eq!(replayed, negative_receipt);
    assert!(changes.is_empty());
    assert_eq!(bits(&s), (-0.0f64).to_bits());
    let positive = patch(
        &s,
        &row_id,
        json!([{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"real","value":0.0f64}}}]),
    );
    let (receipt, changes) = ok_at(&mut s, NOW + 5, |tx| submit(tx, A, positive, X));
    assert!(receipt.event_id.is_some());
    assert_eq!(bits(&s), 0.0f64.to_bits());
    assert_eq!(row(s.conn(), A, &row_id, X).unwrap().revision, "3");
    assert!(!changes.is_empty());
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_link_events"),
        count_before + 2
    );
    assert_eq!(
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
        Some(crewd_store::ts_from_ms(NOW + 5))
    );
}
#[test]
fn populated_table_required_link_add_refuses_without_schema_rows_edges_receipts_or_progress_change()
{
    let mut s = fixture();
    let table = create(&mut s, A, "crew-a", None, "ownerCrewWriters");
    let id = add(&mut s, A, "crew-a", &table, vec![]);
    let original_table = get(s.conn(), A, &table, X).unwrap();
    let original_row = row(s.conn(), A, &id, X).unwrap();
    let before = (
        count(&s, "SELECT count(*) FROM managed_links"),
        count(&s, "SELECT count(*) FROM managed_v2_receipts"),
        count(&s, "SELECT count(*) FROM managed_table_schema_events"),
        count(&s, "SELECT count(*) FROM managed_link_events"),
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
    );
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":table,"expectedSchemaRevision":"1","change":{"kind":"addField","field":{"name":"Required target","kind":{"kind":"link","target":{"kind":"human"},"many":false},"nullable":false}}}),
    );
    assert!(matches!(
        crate::run(&mut s, NOW + 100, |tx| submit(tx, A, r.clone(), X)),
        Err(CollabError::Invalid(_))
    ));
    assert_eq!(get(s.conn(), A, &table, X).unwrap(), original_table);
    assert_eq!(row(s.conn(), A, &id, X).unwrap(), original_row);
    let after = (
        count(&s, "SELECT count(*) FROM managed_links"),
        count(&s, "SELECT count(*) FROM managed_v2_receipts"),
        count(&s, "SELECT count(*) FROM managed_table_schema_events"),
        count(&s, "SELECT count(*) FROM managed_link_events"),
        crate::sessions::get(s.conn(), "w-a1")
            .unwrap()
            .last_progress_at,
    );
    assert_eq!(after, before);
    assert!(matches!(
        lookup(s.conn(), A, &r.scope, "crew-a", &r.request_id).unwrap(),
        RequestLookup::NotFound {}
    ));
    let empty = create(&mut s, A, "crew-a", None, "ownerCrewWriters");
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"alterTable","tableId":empty,"expectedSchemaRevision":"1","change":{"kind":"addField","field":{"name":"Required target","kind":{"kind":"link","target":{"kind":"human"},"many":false},"nullable":false}}}),
    );
    ok(&mut s, |tx| submit(tx, A, r, X));
    assert_eq!(get(s.conn(), A, &empty, X).unwrap().schema_revision, "2");
}
