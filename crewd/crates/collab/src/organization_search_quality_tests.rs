//! Independent adversarial tests; real domain writes, no replacement index or ACL.
use super::*;

fn qa_table(s: &mut Store, crew: &str, title: &str) -> String {
    let request = request(
        s,
        crew,
        json!({"kind":"createTable","title":title,"purpose":"Independent search authority fixture","fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman"}),
    );
    match ok(s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, request, &crate::managed_links::DenyExternal)
    })
    .effect
    {
        v2::Effect::TableCreated { table_id, .. } => table_id,
        _ => panic!("actual table creation"),
    }
}
fn qa_link(s: &mut Store, crew: &str, source: v2::SourceRef, target: v2::TargetRef) {
    let page = crate::managed_links::list(
        s.conn(),
        HUMAN,
        v2::ListLinks {
            query: v2::LinkQuery::Outgoing {
                source: source.clone(),
                include_adapters: false,
            },
            after: None,
            limit: 1,
        },
        &crate::managed_links::DenyExternal,
    )
    .unwrap();
    let r = v2::ManualSubmit {
        scope: managed_objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: v2::ManualOperation::Link {
            source,
            target: v2::ManualTargetWrite::Reference { target },
            expected_manual_link_revision: page.manual_link_revision.unwrap(),
        },
    };
    ok(s, |tx| {
        crate::managed_links::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
}
fn qa_expand(s: &Store, actor: Actor<'_>, target: v2::TargetRef) -> ExpansionPage {
    run_expand(
        s.conn(),
        actor,
        Expand {
            scope: None,
            target,
            cursor: None,
            limit: 200,
            include_system_defaults: false,
        },
        NOW + 100_000,
    )
    .unwrap()
}

#[test]
fn graph_filters_both_private_ends_before_128_candidates_and_reads_remain_pure() {
    let mut s = fixture();
    let work = ok_at(&mut s, NOW - 1, |tx| {
        crate::work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-a".into(),
                title: "Public graph tail".into(),
                ..Default::default()
            },
        )
    })
    .0
    .id;
    let target = v2::TargetRef::Work { id: work.clone() };
    let incoming = qa_table(&mut s, "crew-a", "Visible incoming source");
    qa_link(
        &mut s,
        "crew-a",
        v2::SourceRef::Table {
            id: incoming.clone(),
        },
        target.clone(),
    );
    // Each private H has its own source, so this is legal business data, not an
    // over-cap manual link set or a forged derived-edge table.
    for n in 0..140 {
        let h = ok_at(&mut s, NOW + n, |tx| {
            crate::asks::open(
                tx,
                "cap-x",
                crate::asks::OpenAsk {
                    question: format!("Private incoming source {n}"),
                    default: None,
                    resume_note: None,
                },
            )
        });
        qa_link(
            &mut s,
            "crew-x",
            v2::SourceRef::Ask { id: h.0.ask },
            target.clone(),
        );
    }
    let mut targets = Vec::new();
    for n in 0..140 {
        let id = post(
            &mut s,
            NOW + n,
            "crew-x/lobby",
            &format!("Private outgoing target {n}"),
        );
        targets.push(json!({"kind":"reference","target":{"kind":"message","id":id}}));
    }
    targets.push(json!({"kind":"reference","target":target}));
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"createTable","title":"Outgoing link table","purpose":"Real private-target graph","titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false},{"name":"Links A","kind":{"kind":"link","target":{"kind":"anyReadable"},"many":true},"nullable":true},{"name":"Links B","kind":{"kind":"link","target":{"kind":"anyReadable"},"many":true},"nullable":true},{"name":"Links C","kind":{"kind":"link","target":{"kind":"anyReadable"},"many":true},"nullable":true}]}),
    );
    let v2::Effect::TableCreated { table_id, .. } = ok(&mut s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    })
    .effect
    else {
        panic!("table")
    };
    let mut patches = vec![
        json!({"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Outgoing row"}}}),
    ];
    for (index, values) in targets.chunks(64).enumerate() {
        patches.push(json!({"fieldId":format!("p{}",index+2),"change":{"kind":"replaceLinks","values":values,"keepHiddenSetToken":null}}));
    }
    let r = request(
        &s,
        "crew-a",
        json!({"kind":"addRow","tableId":table_id,"expectedSchemaRevision":"1","patches":patches}),
    );
    let v2::Effect::RowAdded { row_id, .. } = ok(&mut s, |tx| {
        crate::managed_tables_v2::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    })
    .effect
    else {
        panic!("row")
    };
    ready(&mut s);
    let before = crate::testkit::database_snapshot(&s);
    let incoming_page = qa_expand(&s, Actor::Session("w-a1"), target.clone());
    // Two readable sources (the table and the row) must not be crowded out by
    // 140 private H sources; private population must not manufacture a cursor.
    assert_eq!(incoming_page.items.len(), 2);
    assert!(incoming_page.next_cursor.is_none());
    assert!(
        incoming_page
            .items
            .iter()
            .all(|e| e.direction == Direction::Incoming
                && matches!(
                    e.neighbor.target,
                    v2::TargetRef::Table { .. } | v2::TargetRef::Row { .. }
                ))
    );
    let outgoing_page = qa_expand(
        &s,
        Actor::Session("w-a1"),
        v2::TargetRef::Row { id: row_id.clone() },
    );
    assert_eq!(outgoing_page.items.len(), 1);
    assert_eq!(outgoing_page.items[0].neighbor.target, target);
    assert!(outgoing_page.next_cursor.is_none());
    // Human positive control proves the hidden populations really reached the
    // graph, rather than a projection failure accidentally hiding everything.
    let all_incoming = qa_expand(&s, HUMAN, v2::TargetRef::Work { id: work });
    assert_eq!(all_incoming.items.len(), 128);
    assert!(all_incoming.next_cursor.is_some());
    let all_outgoing = qa_expand(&s, HUMAN, v2::TargetRef::Row { id: row_id });
    assert_eq!(all_outgoing.items.len(), 128);
    assert!(all_outgoing.next_cursor.is_some());
    assert_eq!(crate::testkit::database_snapshot(&s), before);
}

#[test]
fn shared_artifact_keeps_all_metadata_contexts_and_purge_removes_exact_cached_targets() {
    use crewd_proto::managed_artifacts as artifact;
    let mut s = fixture();
    let doomed = qa_table(&mut s, "crew-x", "Doomed target");
    let kept = qa_table(&mut s, "crew-b", "Kept neighbor");
    let input = artifact::Input {
        kind: artifact::Kind::Link,
        value: "https://example.invalid/shared-search-artifact".into(),
        repository: None,
    };
    let mut refs = Vec::new();
    for crew in ["crew-x", "crew-b"] {
        let r = artifact::Submit {
            scope: managed_objects::scope(s.conn()).unwrap(),
            crew_id: crew.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change: artifact::Change::Register {
                input: input.clone(),
            },
        };
        refs.push(
            ok(&mut s, |tx| crate::managed_artifacts::submit(tx, HUMAN, r))
                .artifact
                .reference,
        );
    }
    assert_eq!(refs[0], refs[1]);
    let reference = refs.remove(0);
    for (crew, table) in [("crew-x", &doomed), ("crew-b", &kept)] {
        qa_link(
            &mut s,
            crew,
            v2::SourceRef::Artifact {
                reference: reference.clone(),
                metadata_crew_id: crew.into(),
            },
            v2::TargetRef::Table { id: table.clone() },
        );
    }
    // Foreign target deletion must also invalidate a surviving crew's source
    // cache; hiding it at final point-read alone would leave private residue.
    qa_link(
        &mut s,
        "crew-b",
        v2::SourceRef::Table { id: kept.clone() },
        v2::TargetRef::Table { id: doomed.clone() },
    );
    ready(&mut s);
    let target = v2::TargetRef::Artifact {
        reference: reference.clone(),
    };
    let before = qa_expand(&s, HUMAN, target.clone());
    let contexts: std::collections::BTreeSet<_> = before
        .items
        .iter()
        .filter_map(|e| match &e.edge.source {
            v2::SourceRef::Artifact {
                metadata_crew_id, ..
            } => Some(metadata_crew_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        contexts,
        std::collections::BTreeSet::from(["crew-b", "crew-x"])
    );
    let canonical =
        crewd_store::organization_search::key(&v2::TargetRef::Table { id: doomed }).unwrap();
    let cached: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM organization_search_edges WHERE target=?1",
            [&canonical],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cached, 2);
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    let residue: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM organization_search_edges WHERE target=?1",
            [&canonical],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        residue, 0,
        "current and retained target cache must be physically cleared in the purge transaction"
    );
    assert!(crate::managed_artifacts::get(s.conn(), HUMAN, &reference).is_ok());
    ready(&mut s);
    let after = qa_expand(&s, HUMAN, target);
    assert_eq!(after.items.len(), 1);
    assert!(
        matches!(&after.items[0].edge.source,v2::SourceRef::Artifact{metadata_crew_id,..} if metadata_crew_id=="crew-b")
    );
    assert_eq!(
        after.items[0].neighbor.target,
        v2::TargetRef::Table { id: kept }
    );
}

#[test]
fn actual_post_batch_and_maintenance_share_one_credit_budget_with_outer_flush() {
    let mut s = fixture();
    let before: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM organization_search_versions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (_, changes) = crate::run(&mut s, NOW, |tx| {
        for n in 0..100 {
            crate::messages::post(
                tx,
                Sender::Human,
                crate::messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: format!("Batch source {n}"),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    })
    .unwrap();
    assert!(
        !changes.is_empty(),
        "real original posts are business effects"
    );
    let after: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM organization_search_versions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let pending: i64 = s
        .conn()
        .query_row("SELECT count(*) FROM organization_search_dirty", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        after - before,
        64,
        "one normal write transaction receives exactly one source-fact budget"
    );
    assert_eq!(pending, 36);
    let (more, changes) =
        crate::run(&mut s, NOW + 1, |tx| search::maintenance_step(tx, 7)).unwrap();
    assert!(more);
    assert!(
        changes.is_empty(),
        "maintenance must not emit a business event"
    );
    let after_step: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM organization_search_versions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let pending_after: i64 = s
        .conn()
        .query_row("SELECT count(*) FROM organization_search_dirty", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        after_step - after,
        7,
        "outer run flush must not silently reset the requested seven credits to 64"
    );
    assert_eq!(pending_after, 29);
    ready(&mut s);
    let mut q = query("Batch source");
    q.limit = 200;
    let p = qpage(&s, HUMAN, q);
    assert_eq!(p.items.len(), 100);
    assert!(p.next_cursor.is_none());
}
