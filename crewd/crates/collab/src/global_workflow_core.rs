//! Real SQLite transaction checks for the full organization use case.
use crate::testkit::{NOW, err, ok, ok_at, store};
use crate::{Change, CollabError, Sender, asks, crews, work};
use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
use crewd_store::{Store, object_refs::ObjectKind, work_organization as authority};
use crewd_work::{Actor, BlockOn, Continuation};
fn global() -> Store {
    let s = store();
    let exists: bool = s
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='organization_work_book')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !exists {
        let tx = s.conn().unchecked_transaction().unwrap();
        authority::migrate_in_tx(&tx, "2026-10-13T00:00:00Z").unwrap();
        tx.commit().unwrap();
    }
    s
}

fn create(s: &mut Store, crew: &str, owner: &str) -> crewd_proto::WorkPlan {
    ok(s, |tx| {
        work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: crew.into(),
                title: "synthetic organization work".into(),
                owner: Some(owner.into()),
                ..Default::default()
            },
        )
    })
}
fn core(s: &Store, id: &str) -> crewd_work::WorkId {
    authority::core_for_reference(s.conn(), ObjectKind::Plan, id[1..].parse().unwrap())
        .unwrap()
        .unwrap()
        .parse()
        .unwrap()
}
fn block(s: &mut Store, a: &str, b: &str) -> String {
    let a = core(s, a);
    let b = core(s, b);
    let h = ok(s, |tx| {
        asks::with_book(tx, "crew-a", |book, now| {
            book.block(
                &Actor::worker("w-a1"),
                now,
                a,
                vec![
                    BlockOn::Item(b),
                    BlockOn::NewAsk {
                        question: "synthetic decision".into(),
                        default: None,
                    },
                ],
                Continuation::Nothing,
            )
            .map(|ap| ap.value.asks[0])
        })
    });
    authority::reference_for_core(s.conn(), ObjectKind::HumanTask, &h.to_string())
        .unwrap()
        .unwrap()
        .id()
}
fn done(s: &mut Store, p: &crewd_proto::WorkPlan) -> Vec<Change> {
    ok_at(s, NOW, |tx| {
        work::update(
            tx,
            &Sender::Session("w-b1".into()),
            WorkUpdateParams {
                id: p.id.clone(),
                expected_revision: p.revision.clone(),
                status: Some(WorkUpdateStatus::Done),
                ..Default::default()
            },
        )
    })
    .1
}
#[test]
fn actual_owner_completion_releases_only_one_cross_crew_blocker_and_refreshes_target_crew() {
    let mut s = global();
    let b = create(&mut s, "crew-b", "w-b1");
    let a = create(&mut s, "crew-a", "w-a1");
    let h = block(&mut s, &a.id, &b.id);
    let changes = done(&mut s, &b);
    let after = work::get(s.conn(), &a.id).unwrap();
    assert_eq!(after.status, "blocked");
    assert_eq!(after.blocked_on.as_slice(), std::slice::from_ref(&h));
    assert!(changes.contains(&Change::WorkChanged {
        crew_id: "crew-a".into(),
        id: a.id.clone()
    }));
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT COUNT(*) FROM wakes WHERE session_id='w-a1' AND source LIKE 'work-done:%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    ok(&mut s, |tx| asks::answer(tx, "crew-a", &h, "approved"));
    assert_eq!(work::get(s.conn(), &a.id).unwrap().status, "active");
    assert!(asks::waits(s.conn(), NOW, None).unwrap().is_empty());
    assert_eq!(asks::get(s.conn(), "crew-a", &h).unwrap().ask, h);
}
#[test]
fn queue_refusal_rolls_back_source_done_target_release_events_and_revision() {
    let mut s = global();
    let b = create(&mut s, "crew-b", "w-b1");
    let a = create(&mut s, "crew-a", "w-a1");
    block(&mut s, &a.id, &b.id);
    let before = authority::load(s.conn()).unwrap().unwrap();
    let events: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    s.conn().execute_batch("CREATE TRIGGER injected_cross_crew_queue_failure BEFORE INSERT ON wakes WHEN NEW.source LIKE 'work-done:%' BEGIN SELECT RAISE(ABORT,'injected queue failure'); END;").unwrap();
    let failure = err(&mut s, |tx| {
        work::update(
            tx,
            &Sender::Session("w-b1".into()),
            WorkUpdateParams {
                id: b.id.clone(),
                expected_revision: b.revision.clone(),
                status: Some(WorkUpdateStatus::Done),
                ..Default::default()
            },
        )
    });
    assert!(matches!(failure, CollabError::Store(_)));
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
    assert_eq!(
        s.conn()
            .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        events
    );
    assert_eq!(work::get(s.conn(), &b.id).unwrap().status, "open");
}
#[test]
fn ordinary_captain_cannot_mutate_other_crew_but_foreign_owner_can() {
    let mut s = global();
    let b = create(&mut s, "crew-b", "w-b1");
    let failure = err(&mut s, |tx| {
        work::update(
            tx,
            &Sender::Session("cap-a".into()),
            WorkUpdateParams {
                id: b.id.clone(),
                expected_revision: b.revision.clone(),
                note: Some("forbidden change".into()),
                ..Default::default()
            },
        )
    });
    assert!(matches!(
        failure,
        CollabError::Work(crewd_work::Rejected::Forbidden(_))
    ));
    assert_eq!(work::get(s.conn(), &b.id).unwrap(), b);
    done(&mut s, &b);
}
#[test]
fn purge_removes_internal_subgraph_and_never_reuses_registry_or_core_numbers() {
    let mut s = global();
    let old = create(&mut s, "crew-x", "cap-x");
    let child = ok(&mut s, |tx| {
        work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-x".into(),
                title: "internal child".into(),
                owner: Some("cap-x".into()),
                parent: Some(old.id.clone()),
                ..Default::default()
            },
        )
    });
    let high = authority::load(s.conn()).unwrap().unwrap().last_item;
    ok(&mut s, |tx| crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crews::purge(tx, "crew-x"));
    assert!(crews::get(s.conn(), "crew-x").is_err());
    assert!(work::get(s.conn(), &old.id).is_err());
    assert!(work::get(s.conn(), &child.id).is_err());
    assert!(authority::load(s.conn()).unwrap().unwrap().items.is_empty());
    let new = create(&mut s, "crew-a", "w-a1");
    assert!(new.id[1..].parse::<i64>().unwrap() > child.id[1..].parse::<i64>().unwrap());
    assert!(core(&s, &new.id).0 > high);
}
#[test]
fn foreign_links_and_foreign_current_owners_block_real_purge_without_mutation() {
    for owner_case in [false, true] {
        let mut s = global();
        let x = create(&mut s, "crew-x", "cap-x");
        let mut p = WorkCreateParams {
            crew_id: "crew-a".into(),
            title: "foreign work".into(),
            owner: Some(if owner_case { "cap-x" } else { "w-a1" }.into()),
            ..Default::default()
        };
        if !owner_case {
            p.depends_on = vec![x.id.clone()];
        }
        let a = ok(&mut s, |tx| work::create(tx, &Sender::Human, p));
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        let before = authority::load(s.conn()).unwrap().unwrap();
        let failure = err(&mut s, |tx| crews::purge(tx, "crew-x"));
        assert!(
            matches!(failure,CollabError::Conflict(ref r) if r.code==crate::refusal::Code::CrewWorkReferenced && r.args.iter().any(|(key,value)|*key=="ids" && value.contains(&a.id)))
        );
        assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
        assert!(crews::get(s.conn(), "crew-x").is_ok());
    }
}
#[test]
fn actual_purge_sql_failure_rolls_back_crew_registry_and_organization_state() {
    let mut s = global();
    let x = create(&mut s, "crew-x", "cap-x");
    ok(&mut s, |tx| crews::archive(tx, "crew-x"));
    let before = authority::load(s.conn()).unwrap().unwrap();
    s.conn().execute_batch("CREATE TRIGGER injected_purge_authority_failure BEFORE UPDATE ON organization_work_book BEGIN SELECT RAISE(ABORT,'injected purge authority failure'); END;").unwrap();
    let failed = crate::run(&mut s, NOW, |tx| crews::purge(tx, "crew-x"));
    assert!(failed.is_err());
    assert!(crews::get(s.conn(), "crew-x").is_ok());
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
    assert!(work::get(s.conn(), &x.id).is_ok());
}

#[test]
fn archived_owner_history_remains_read_only_and_available() {
    let mut s = global();
    let p = create(&mut s, "crew-x", "cap-x");
    let prior = work::history(
        s.conn(),
        crewd_proto::WorkHistoryParams {
            id: p.id.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    ok(&mut s, |tx| crews::archive(tx, "crew-x"));
    let before = authority::load(s.conn()).unwrap().unwrap();
    assert_eq!(
        work::history(
            s.conn(),
            crewd_proto::WorkHistoryParams {
                id: p.id,
                ..Default::default()
            }
        )
        .unwrap(),
        prior
    );
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
}
fn raw(s: &Store, id: &str, crew: &str, number: i64) {
    crewd_store::todos::insert(
        s.conn(),
        &crewd_proto::Todo {
            requirement_id: None,
            id: id.into(),
            crew_id: crew.into(),
            ledger: crewd_proto::Ledger::Agent,
            number,
            text: "original requirement".into(),
            status: "pending".into(),
            created_at: "2026-10-13T00:00:00Z".into(),
            updated_at: None,
            deleted_at: None,
            attachments: vec![],
            responses: vec![],
        },
        None,
    )
    .unwrap();
}
#[test]
fn purge_of_highest_raw_source_preserves_requirement_core_and_public_highwater() {
    let mut s = global();
    raw(&s, "source-x", "crew-x", 1);
    let (old_core, old_public): (String, i64) = s
        .conn()
        .query_row(
            "SELECT core_id,number FROM object_refs WHERE raw_todo_id='source-x'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    ok(&mut s, |tx| crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crews::purge(tx, "crew-x"));
    assert!(
        authority::load(s.conn())
            .unwrap()
            .unwrap()
            .entries
            .is_empty()
    );
    raw(&s, "source-a", "crew-a", 1);
    let (new_core, new_public): (String, i64) = s
        .conn()
        .query_row(
            "SELECT core_id,number FROM object_refs WHERE raw_todo_id='source-a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(new_core[1..].parse::<i64>().unwrap() > old_core[1..].parse::<i64>().unwrap());
    assert!(new_public > old_public);
}
#[test]
fn public_h_cannot_fall_back_to_another_question_but_explicit_legacy_alias_still_reads() {
    let mut s = crate::testkit::v8_store();
    for (crew, owner) in [("crew-a", "w-a1"), ("crew-b", "w-b1")] {
        let mut b =
            crewd_work::WorkBook::new(crewd_work::CrewId::new(crew), crewd_work::Config::default());
        b.open_ask(&Actor::worker(owner), crewd_work::Ts(NOW), crew, None, None)
            .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,'t')",
                rusqlite::params![crew, serde_json::to_string(b.state()).unwrap()],
            )
            .unwrap();
    }
    let b =
        crewd_store::object_refs::bind(s.conn(), ObjectKind::HumanTask, "crew-b", "work:H1", "t")
            .unwrap();
    let a =
        crewd_store::object_refs::bind(s.conn(), ObjectKind::HumanTask, "crew-a", "work:H1", "t")
            .unwrap();
    {
        let tx = s.conn().unchecked_transaction().unwrap();
        authority::migrate_in_tx(&tx, "t").unwrap();
        tx.commit().unwrap();
    }
    let before = authority::load(s.conn()).unwrap().unwrap();
    assert!(asks::get(s.conn(), "crew-a", &b.id()).is_err());
    assert!(
        crate::run(&mut s, NOW, |tx| asks::answer(
            tx,
            "crew-a",
            &b.id(),
            "must not answer A"
        ))
        .is_err()
    );
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
    assert_eq!(
        asks::get(s.conn(), "crew-a", "work:H1").unwrap().ask,
        a.id()
    );
}
#[test]
fn legacy_ask_wake_preserves_receipt_source_and_renders_its_original_question() {
    let mut s = crate::testkit::v8_store();
    for (crew, owner, text) in [
        ("crew-a", "w-a1", "A original answer"),
        ("crew-b", "w-b1", "B different answer"),
    ] {
        let mut b =
            crewd_work::WorkBook::new(crewd_work::CrewId::new(crew), crewd_work::Config::default());
        let h = b
            .open_ask(&Actor::worker(owner), crewd_work::Ts(NOW), crew, None, None)
            .unwrap()
            .value;
        b.answer_ask(&Actor::Human, crewd_work::Ts(NOW), h, text)
            .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,'t')",
                rusqlite::params![crew, serde_json::to_string(b.state()).unwrap()],
            )
            .unwrap();
    }
    crewd_store::object_refs::bind(s.conn(), ObjectKind::HumanTask, "crew-b", "work:H1", "t")
        .unwrap();
    let a =
        crewd_store::object_refs::bind(s.conn(), ObjectKind::HumanTask, "crew-a", "work:H1", "t")
            .unwrap();
    ok(&mut s, |tx| {
        crate::wakes::enqueue(
            tx,
            crate::wakes::WakeRequest {
                session: "w-a1".into(),
                kind: crate::wakes::WakeKind::AskAnswer,
                source: "ask:crew-a/H1".into(),
                priority: crate::wakes::WakePriority::Human,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )
    });
    {
        let tx = s.conn().unchecked_transaction().unwrap();
        authority::migrate_in_tx(&tx, "t").unwrap();
        tx.commit().unwrap();
    }
    let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
    assert_eq!(rows[0].source, "ask:crew-a/H1");
    assert_eq!(rows[0].attempts, 0);
    let rendered =
        crate::render::build_batch(s.conn(), "w-a1", crate::Locale::En, &rows, NOW).unwrap();
    let text = crate::render::wake_text(crate::Locale::En, &rendered);
    assert!(text.contains("A original answer"));
    assert!(!text.contains("B different answer"));
    assert!(text.contains(&a.id()));
}

fn new_block(id: &str, revision: &str) -> crewd_proto::WorkBlockParams {
    crewd_proto::WorkBlockParams {
        id: id.into(),
        expected_revision: revision.into(),
        on: vec![crewd_proto::WorkBlockOn::NewHuman {
            question: "new decision".into(),
            default: None,
        }],
        can_continue: crewd_proto::WorkContinuation::Split {
            items: vec![WorkCreateParams {
                crew_id: "crew-a".into(),
                title: "independent part".into(),
                ..Default::default()
            }],
        },
    }
}
#[test]
fn public_block_new_human_split_is_atomic_and_inherits_original_owner() {
    let mut s = global();
    let a = create(&mut s, "crew-a", "w-a1");
    let out = ok(&mut s, |tx| {
        crate::work_block::block(
            tx,
            &Sender::Session("cap-a".into()),
            new_block(&a.id, &a.revision),
        )
    });
    assert_eq!(out.human_tasks.len(), 1);
    assert_eq!(out.split.len(), 1);
    let part = work::get(s.conn(), &out.split[0]).unwrap();
    assert_eq!(part.parent, Some(a.id));
    assert_eq!(part.owner, Some("w-a1".into()));
    assert_eq!(out.item.blocked_on, out.human_tasks);
    let question = asks::get(s.conn(), "crew-a", &out.human_tasks[0]).unwrap();
    assert!(question.message_id.is_some());
    assert_eq!(question.item, Some(out.item.id));
}
#[test]
fn public_block_announcement_failure_rolls_back_human_split_ids_and_messages() {
    let mut s = global();
    let a = create(&mut s, "crew-a", "w-a1");
    let before = authority::load(s.conn()).unwrap().unwrap();
    let sql = "SELECT (SELECT COUNT(*) FROM object_refs),(SELECT COUNT(*) FROM organization_work_events),(SELECT COUNT(*) FROM messages),(SELECT COUNT(*) FROM ask_meta)";
    let rows: (i64, i64, i64, i64) = s
        .conn()
        .query_row(sql, [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    s.conn().execute_batch("CREATE TRIGGER injected_block_announcement_failure BEFORE INSERT ON ask_meta BEGIN SELECT RAISE(ABORT,'injected block announcement failure'); END;").unwrap();
    assert!(
        crate::run(&mut s, NOW, |tx| crate::work_block::block(
            tx,
            &Sender::Session("w-a1".into()),
            new_block(&a.id, &a.revision)
        ))
        .is_err()
    );
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
    assert_eq!(
        s.conn()
            .query_row(sql, [], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?
            )))
            .unwrap(),
        rows
    );
}
#[test]
fn all_dropped_cross_crew_source_requires_ruling_and_does_not_auto_close() {
    let mut s = global();
    raw(&s, "source-a", "crew-a", 1);
    let source = crewd_store::todos::list(s.conn(), "crew-a", crewd_proto::Ledger::Agent).unwrap()
        [0]
    .requirement_id
    .clone()
    .unwrap();
    let p = ok(&mut s, |tx| {
        work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-b".into(),
                title: "plan for A source".into(),
                owner: Some("w-b1".into()),
                sources: vec![source],
                ..Default::default()
            },
        )
    });
    let id = core(&s, &p.id);
    ok(&mut s, |tx| {
        asks::with_book(tx, "crew-b", |book, now| {
            book.drop_item(
                &Actor::worker("w-b1"),
                now,
                id,
                crewd_work::DropReason {
                    kind: crewd_work::DropKind::NotNeeded,
                    note: "explicit owner decision".into(),
                    evidence: None,
                },
            )
        })
    });
    assert_eq!(
        crewd_store::todos::list(s.conn(), "crew-a", crewd_proto::Ledger::Agent).unwrap()[0].status,
        "needs_ruling"
    );
    assert!(
        authority::load(s.conn())
            .unwrap()
            .unwrap()
            .entries
            .values()
            .all(|e| e.closed.is_none())
    );
}

#[test]
fn organization_default_batch_refusal_rolls_back_all_crews_and_success_keeps_each_answer() {
    let mut s = global();
    let a = create(&mut s, "crew-a", "w-a1");
    let b = create(&mut s, "crew-b", "w-a1");
    for p in [&a, &b] {
        ok(&mut s, |tx| {
            crate::work_block::block(
                tx,
                &Sender::Session("w-a1".into()),
                crewd_proto::WorkBlockParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    on: vec![crewd_proto::WorkBlockOn::NewHuman {
                        question: format!("decision for {}", p.crew_id),
                        default: Some(crewd_proto::AskDefault {
                            text: format!("default for {}", p.crew_id),
                            after_minutes: 1,
                        }),
                    }],
                    can_continue: crewd_proto::WorkContinuation::Nothing,
                },
            )
        });
    }
    let before = authority::load(s.conn()).unwrap().unwrap();
    s.conn().execute_batch("CREATE TRIGGER injected_batch_default_failure BEFORE INSERT ON wakes WHEN NEW.kind='ask_answer' BEGIN SELECT RAISE(ABORT,'injected batch delivery failure'); END;").unwrap();
    assert!(crate::run(&mut s, NOW + 60_000, asks::apply_all_due_defaults).is_err());
    assert_eq!(authority::load(s.conn()).unwrap().unwrap(), before);
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT COUNT(*) FROM wakes WHERE kind='ask_answer'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    s.conn()
        .execute_batch("DROP TRIGGER injected_batch_default_failure;")
        .unwrap();
    let applied = ok_at(&mut s, NOW + 60_000, asks::apply_all_due_defaults).0;
    assert_eq!(applied.len(), 2);
    assert_eq!(work::get(s.conn(), &a.id).unwrap().status, "active");
    assert_eq!(work::get(s.conn(), &b.id).unwrap().status, "active");
    let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW + 60_000).unwrap();
    let batch =
        crate::render::build_batch(s.conn(), "w-a1", crate::Locale::En, &rows, NOW + 60_000)
            .unwrap();
    let text = crate::render::wake_text(crate::Locale::En, &batch);
    assert!(text.contains("default for crew-a"));
    assert!(text.contains("default for crew-b"));
}
