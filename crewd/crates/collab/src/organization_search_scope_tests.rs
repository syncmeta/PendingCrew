//! Omitted scope is equivalent to the exact current scope, never another scope.
use super::*;

fn wrong_scopes(
    scope: &crewd_proto::managed_objects::Scope,
) -> Vec<crewd_proto::managed_objects::Scope> {
    let mut db = scope.clone();
    db.db_id = uuid::Uuid::new_v4().to_string();
    let mut epoch = scope.clone();
    epoch.restore_epoch = uuid::Uuid::new_v4().to_string();
    vec![db, epoch]
}
fn scope_link(s: &mut Store, t: &str, o: &str) {
    let source = v2::SourceRef::Table { id: t.into() };
    let view = crate::managed_links::list(
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
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: v2::ManualOperation::Link {
            source,
            target: v2::ManualTargetWrite::Reference {
                target: v2::TargetRef::Row { id: o.into() },
            },
            expected_manual_link_revision: view.manual_link_revision.unwrap(),
        },
    };
    ok(s, |tx| {
        crate::managed_links::submit(tx, HUMAN, r, &crate::managed_links::DenyExternal)
    });
}
#[test]
fn search_scope_omitted_and_exact_are_one_query_in_both_directions_after_legal_writes() {
    for initially_explicit in [false, true] {
        let mut s = fixture();
        let t = table(&mut s);
        let tail = row(&mut s, &t, NOW, "needle tail", "");
        row(&mut s, &t, NOW + 1, "needle head", "");
        ready(&mut s);
        let scope = managed_objects::scope(s.conn()).unwrap();
        let mut first_query = rows_query(&t);
        first_query.scope = initially_explicit.then_some(scope);
        let first = qpage(&s, HUMAN, first_query);
        assert_eq!(first.items.len(), 1);
        let token = first.next_cursor.clone().expect("tail continuation");
        edit(&mut s, &tail, "needle changed tail", NOW + 10);
        post(
            &mut s,
            NOW + 11,
            "crew-a/lobby",
            "ordinary unrelated message",
        );
        row(&mut s, &t, NOW + 12, "needle new source", "");
        ready(&mut s);
        let before = crate::testkit::database_snapshot(&s);
        let mut continuation = rows_query(&t);
        continuation.cursor = Some(token);
        continuation.scope = (!initially_explicit).then_some(first.scope.clone());
        for wrong in wrong_scopes(&first.scope) {
            let mut bad = continuation.clone();
            bad.scope = Some(wrong);
            assert!(
                matches!(run_search(s.conn(),HUMAN,bad,NOW+100_001),Err(crate::CollabError::Conflict(r)) if r.code==crate::refusal::Code::SearchCursorExpired)
            );
        }
        let page = run_search(s.conn(), HUMAN, continuation, NOW + 100_001).unwrap();
        assert_eq!(page.snapshot, first.snapshot);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].target, v2::TargetRef::Row { id: tail });
        assert_eq!(page.items[0].title, "needle changed tail");
        assert!(page.next_cursor.is_none());
        assert_eq!(crate::testkit::database_snapshot(&s), before);
    }
}
#[test]
fn expand_scope_omitted_and_exact_are_one_query_in_both_directions_after_legal_writes() {
    for initially_explicit in [false, true] {
        let mut s = fixture();
        let t = table(&mut s);
        let a = row(&mut s, &t, NOW, "neighbor a", "");
        let b = row(&mut s, &t, NOW + 1, "neighbor b", "");
        scope_link(&mut s, &t, &a);
        scope_link(&mut s, &t, &b);
        ready(&mut s);
        let scope = managed_objects::scope(s.conn()).unwrap();
        let base = Expand {
            scope: initially_explicit.then_some(scope),
            target: v2::TargetRef::Table { id: t.clone() },
            cursor: None,
            limit: 1,
            include_system_defaults: false,
        };
        let first = run_expand(s.conn(), HUMAN, base.clone(), NOW + 100_000).unwrap();
        assert_eq!(first.items.len(), 1);
        let token = first.next_cursor.clone().expect("second neighbor");
        let first_id = match &first.items[0].neighbor.target {
            v2::TargetRef::Row { id } => id,
            _ => panic!("row neighbor"),
        };
        let unseen = if first_id == &a { b } else { a };
        edit(&mut s, &unseen, "neighbor changed current title", NOW + 20);
        post(
            &mut s,
            NOW + 21,
            "crew-a/lobby",
            "ordinary unrelated graph message",
        );
        let later = row(&mut s, &t, NOW + 22, "neighbor after cut", "");
        scope_link(&mut s, &t, &later);
        ready(&mut s);
        let before = crate::testkit::database_snapshot(&s);
        let mut continuation = base;
        continuation.cursor = Some(token);
        continuation.scope = (!initially_explicit).then_some(first.scope.clone());
        for wrong in wrong_scopes(&first.scope) {
            let mut bad = continuation.clone();
            bad.scope = Some(wrong);
            assert!(
                matches!(run_expand(s.conn(),HUMAN,bad,NOW+100_001),Err(crate::CollabError::Conflict(r)) if r.code==crate::refusal::Code::SearchCursorExpired)
            );
        }
        let page = run_expand(s.conn(), HUMAN, continuation, NOW + 100_001).unwrap();
        assert_eq!(page.snapshot, first.snapshot);
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].neighbor.target,
            v2::TargetRef::Row { id: unseen }
        );
        assert_eq!(
            page.items[0].neighbor.summary,
            "neighbor changed current title"
        );
        assert_eq!(page.items[0].direction, Direction::Outgoing);
        assert!(page.next_cursor.is_none());
        assert_eq!(crate::testkit::database_snapshot(&s), before);
    }
}
