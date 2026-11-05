//! Genuine object/work transactions; pointers remain unverified and never execute anything.
use super::*;
use crate::{
    Change as Fact, Sender,
    testkit::{NOW, count, database_snapshot, ok, ok_at},
};
use crewd_proto::{
    WorkCreateParams, WorkEvidenceKind, WorkEvidenceParam, WorkUpdateParams, WorkUpdateStatus,
    managed_objects::{Change as ObjectChange, Submit as ObjectSubmit},
};
use crewd_store::Store;
use serde_json::json;
const HUMAN: Actor<'static> = Actor::Human("artifact-fixture-human");
const AUTHOR: Actor<'static> = Actor::Session("w-a1");
fn fixture() -> Store {
    let s = crate::testkit::store();
    if crewd_store::meta_get(s.conn(), "dbId").unwrap().is_none() {
        crewd_store::meta_set(s.conn(), "dbId", "artifact-fixture-db").unwrap();
    }
    s
}
fn input(kind: Kind, value: &str) -> Input {
    Input {
        kind,
        value: value.into(),
        repository: None,
    }
}
fn request(s: &Store, crew: &str, change: Change) -> Submit {
    Submit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change,
    }
}
fn known(s: &Store, actor: Actor<'_>, r: &Submit) -> Receipt {
    match lookup(s.conn(), actor, &r.scope, &r.crew_id, &r.request_id).unwrap() {
        Lookup::Known { receipt } => receipt,
        v => panic!("expected known original: {v:?}"),
    }
}
fn work(s: &mut Store, crew: &str, owner: &str) -> crewd_proto::WorkPlan {
    ok(s, |tx| {
        crate::work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: crew.into(),
                title: "Synthetic artifact work".into(),
                owner: Some(owner.into()),
                ..Default::default()
            },
        )
    })
}
fn full() -> Input {
    input(Kind::Commit, &"A".repeat(40))
}

#[test]
fn canonical_identity_is_scoped_structural_and_preserves_path_query_fragment() {
    let s = fixture();
    let a = preview(s.conn(), AUTHOR, "crew-a", &full()).unwrap();
    assert_eq!(
        a,
        Reference {
            namespace: Namespace::Global,
            key: format!("commit:{}", "a".repeat(40))
        }
    );
    assert_eq!(
        db::canonicalize("crew-x", &input(Kind::Commit, &"B".repeat(64)))
            .unwrap()
            .namespace,
        Namespace::Global
    );
    assert!(matches!(
        db::canonicalize("crew-a", &input(Kind::Commit, &"c".repeat(63)))
            .unwrap()
            .namespace,
        Namespace::Unassigned { .. }
    ));
    let short = db::canonicalize("crew-a", &input(Kind::Commit, "ABC1234")).unwrap();
    assert_eq!(
        short,
        Reference {
            namespace: Namespace::Unassigned {
                crew_id: "crew-a".into()
            },
            key: "commit:abc1234".into()
        }
    );
    assert_ne!(
        short,
        db::canonicalize("crew-x", &input(Kind::Commit, "ABC1234")).unwrap()
    );
    let mut scoped = input(Kind::File, "src/Exact.rs");
    scoped.repository = Some("opaque repository identity".into());
    assert_eq!(
        db::canonicalize("crew-a", &scoped).unwrap(),
        db::canonicalize("crew-x", &scoped).unwrap()
    );
    for bad in ["", " ", " leading", "trailing "] {
        let mut invalid = scoped.clone();
        invalid.repository = Some(bad.into());
        assert!(
            db::canonicalize("crew-a", &invalid).is_err(),
            "repository identity is rejected, never silently trimmed"
        );
    }
    let mut other = scoped.clone();
    other.repository = Some("Opaque repository identity".into());
    assert_ne!(
        db::canonicalize("crew-a", &scoped).unwrap(),
        db::canonicalize("crew-a", &other).unwrap()
    );
    for (value, expected) in [
        (
            "HTTPS://EXAMPLE.COM/Case?q=A%2fb#Frag",
            "url:https://example.com/Case?q=A%2fb#Frag",
        ),
        ("pr:HTTPS://EXAMPLE.COM", "url:https://example.com/"),
        (
            "https://[::1]:8443/Test?x=1#F",
            "url:https://[::1]:8443/Test?x=1#F",
        ),
        ("https://example.com?x=1", "url:https://example.com/?x=1"),
    ] {
        assert_eq!(
            db::canonicalize("crew-a", &input(Kind::PullRequest, value)).unwrap(),
            db::canonicalize("crew-x", &input(Kind::Link, expected)).unwrap()
        );
    }
    for value in ["../escape", "/absolute", "a/../b", "C:/absolute", "a\\b"] {
        assert!(db::canonicalize("crew-a", &input(Kind::File, value)).is_err());
    }
    for value in [
        "javascript:alert(1)",
        "https://user:secret@example.com/a",
        "https://example.com/a b",
        "https://",
        "https://example.com/%xx",
        " https://example.com/",
        "https://example.com/\n",
    ] {
        assert!(db::canonicalize("crew-a", &input(Kind::Link, value)).is_err());
    }
    assert!(preview(s.conn(), AUTHOR, "crew-x", &full()).is_err());
    let raw = (0..90)
        .map(|n| format!("https://example.com/item/{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        db::extract("crew-a", &raw).len(),
        90,
        "automatic extraction must not silently truncate token count"
    );
    assert_eq!(db::extract("crew-a", "ABC1234 commit:abc1234").len(), 1);
    let embedded = db::canonicalize(
        "crew-a",
        &input(Kind::Link, "https://example.test/pr/7?q=A#F"),
    )
    .unwrap();
    assert_eq!(
        db::extract("crew-a", "[fix](https://example.test/pr/7?q=A#F)"),
        vec![embedded.clone()]
    );
    assert_eq!(
        db::extract(
            "crew-a",
            "\u{4fee}\u{590d}\u{ff1a}https://example.test/pr/7?q=A#F"
        ),
        vec![embedded]
    );
    assert_eq!(
        db::extract("crew-a", "https://[::1]"),
        vec![db::canonicalize("crew-a", &input(Kind::Link, "https://[::1]")).unwrap()]
    );
}

#[test]
fn register_receipts_are_exact_and_semantic_duplicates_emit_no_progress_or_hint() {
    let mut s = fixture();
    let req = request(&s, "crew-a", Change::Register { input: full() });
    let before_wakes = count(&s, "SELECT count(*) FROM wakes");
    let (saved, facts) = ok_at(&mut s, NOW, |tx| submit(tx, AUTHOR, req.clone()));
    assert_eq!(
        saved.effect,
        Effect::Registered {
            created: true,
            claim_added: true
        }
    );
    assert_eq!(saved.change, req.change);
    assert_eq!(
        facts,
        vec![
            Fact::Progress {
                session_id: "w-a1".into(),
                kind: crate::ProgressKind::Ontology
            },
            Fact::ArtifactChanged {
                crew_id: "crew-a".into()
            }
        ]
    );
    let before = database_snapshot(&s);
    let (replayed, facts) = ok_at(&mut s, NOW + 1, |tx| submit(tx, AUTHOR, req.clone()));
    assert_eq!(saved, replayed);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let duplicate = request(&s, "crew-a", req.change.clone());
    let (duplicate, facts) = ok_at(&mut s, NOW + 2, |tx| submit(tx, AUTHOR, duplicate));
    assert_eq!(
        duplicate.effect,
        Effect::Registered {
            created: false,
            claim_added: false
        }
    );
    assert!(facts.is_empty());
    let mut conflict = req.clone();
    conflict.change = Change::Register {
        input: input(Kind::Test, "different test"),
    };
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW + 3, |tx| submit(tx, AUTHOR, conflict)).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(known(&s, AUTHOR, &req), saved);
    assert_eq!(count(&s, "SELECT count(*) FROM wakes"), before_wakes);
    assert!(matches!(
        lookup(s.conn(), HUMAN, &req.scope, "crew-a", &req.request_id).unwrap(),
        Lookup::NotFound
    ));
}

#[test]
fn manual_link_unlink_cas_and_original_receipt_do_not_delete_a_newer_edge() {
    let mut s = fixture();
    let w = work(&mut s, "crew-x", "cap-x");
    let target = Subject::Work { id: w.id };
    let req = request(
        &s,
        "crew-a",
        Change::Link {
            input: full(),
            subject: target.clone(),
            relation: Relation::Evidence,
        },
    );
    let (saved, facts) = ok_at(&mut s, NOW, |tx| submit(tx, AUTHOR, req.clone()));
    let Effect::Linked { edge, .. } = &saved.effect else {
        panic!("manual edge")
    };
    assert_eq!(edge.crew_id, "crew-a");
    assert_eq!(
        edge.origin,
        Origin::Manual {
            actor: ResourceActor {
                kind: "session".into(),
                session_id: Some("w-a1".into())
            }
        }
    );
    assert_eq!(
        facts,
        vec![
            Fact::Progress {
                session_id: "w-a1".into(),
                kind: crate::ProgressKind::Ontology
            },
            Fact::ArtifactChanged {
                crew_id: "crew-a".into()
            }
        ]
    );
    let mut stale = request(
        &s,
        "crew-a",
        Change::Unlink {
            reference: saved.artifact.reference.clone(),
            subject: target.clone(),
            relation: Relation::Evidence,
            expected_edge_id: "999999999".into(),
        },
    );
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, AUTHOR, stale.clone())).is_err());
    assert_eq!(database_snapshot(&s), before);
    stale.request_id = uuid::Uuid::new_v4().to_string();
    if let Change::Unlink {
        expected_edge_id, ..
    } = &mut stale.change
    {
        *expected_edge_id = edge.id.clone();
    }
    let unlinked = ok(&mut s, |tx| submit(tx, AUTHOR, stale.clone()));
    assert_eq!(
        unlinked.effect,
        Effect::Unlinked {
            edge_id: edge.id.clone()
        }
    );
    assert!(
        get(s.conn(), HUMAN, &saved.artifact.reference).is_err(),
        "last edge without a pin collects anchor"
    );
    assert_eq!(
        known(&s, AUTHOR, &req),
        saved,
        "original effect survives manual unlink/GC"
    );
    let replacement = request(&s, "crew-a", req.change.clone());
    let again = ok(&mut s, |tx| submit(tx, AUTHOR, replacement));
    let Effect::Linked { edge: new, .. } = &again.effect else {
        panic!("new manual edge")
    };
    assert!(new.id.parse::<i64>().unwrap() > edge.id.parse::<i64>().unwrap());
    let before = database_snapshot(&s);
    let (same, facts) = ok_at(&mut s, NOW, |tx| submit(tx, AUTHOR, stale.clone()));
    assert_eq!(same, unlinked);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let page = subject(s.conn(), HUMAN, &target, None).unwrap();
    assert_eq!(page.items, [new.as_ref().clone()]);
}

#[test]
fn real_worker_update_and_human_close_capture_only_fresh_done_event_evidence() {
    let mut s = fixture();
    let a = work(&mut s, "crew-a", "w-a1");
    let x = work(&mut s, "crew-x", "cap-x");
    let empty = work(&mut s, "crew-a", "w-a1");
    let raw = format!("{} https://EXAMPLE.COM/Proof?X=1#Keep", "A".repeat(40));
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a1".into()),
            WorkUpdateParams {
                id: a.id.clone(),
                expected_revision: a.revision.clone(),
                status: Some(WorkUpdateStatus::Done),
                evidence: Some(WorkEvidenceParam {
                    kind: WorkEvidenceKind::Text,
                    value: raw.clone(),
                }),
                ..Default::default()
            },
        )
    });
    let closed = ok(&mut s, |tx| {
        crate::work_close::close(
            tx,
            &Sender::Human,
            crewd_proto::WorkCloseParams {
                id: x.id.clone(),
                expected_revision: x.revision.clone(),
                kind: crewd_proto::WorkClosureKind::Done,
                evidence: WorkEvidenceParam {
                    kind: WorkEvidenceKind::Commit,
                    value: "a".repeat(40),
                },
                note: "Synthetic verified factual close".into(),
            },
        )
    });
    assert_eq!(closed.status, "done");
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a1".into()),
            WorkUpdateParams {
                id: empty.id.clone(),
                expected_revision: empty.revision.clone(),
                status: Some(WorkUpdateStatus::Done),
                ..Default::default()
            },
        )
    });
    assert_eq!(
        crate::work::get(s.conn(), &a.id)
            .unwrap()
            .evidence
            .unwrap()
            .value,
        raw
    );
    let reference = db::canonicalize("crew-a", &full()).unwrap();
    let page = backlinks(s.conn(), HUMAN, &reference, None).unwrap();
    assert_eq!(page.items.len(), 2);
    for edge in &page.items {
        let Origin::WorkDone { event_seq } = &edge.origin else {
            panic!("real automatic Done origin")
        };
        let body: String = s
            .conn()
            .query_row(
                "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
                rusqlite::params![event_seq.parse::<i64>().unwrap(), edge.crew_id],
                |r| r.get(0),
            )
            .unwrap();
        let event: crewd_work::Event = serde_json::from_str(&body).unwrap();
        assert!(matches!(event.kind, crewd_work::EventKind::Done { .. }));
    }
    assert_eq!(
        subject(s.conn(), AUTHOR, &Subject::Work { id: empty.id }, None)
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 2);
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_artifact_claims"),
        0,
        "automatic capture is not an explicit pin"
    );
    let edge = &page.items[0];
    let reject = request(
        &s,
        &edge.crew_id,
        Change::Unlink {
            reference: reference.clone(),
            subject: edge.subject.clone(),
            relation: Relation::Evidence,
            expected_edge_id: edge.id.clone(),
        },
    );
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, reject)).is_err());
    assert_eq!(
        database_snapshot(&s),
        before,
        "automatic facts cannot be manually removed"
    );
}

#[test]
fn lesson_evidence_uses_same_canonicalizer_and_keeps_original_strings() {
    let mut s = fixture();
    let raw = vec![
        "pr:HTTPS://EXAMPLE.COM/pull/1".to_string(),
        "url:https://example.com/pull/1".into(),
        "file:src/Exact.rs".into(),
        "not a pointer".into(),
    ];
    let change:ObjectChange=serde_json::from_value(json!({"kind":"recordLesson","draft":{"happened":"Synthetic occurrence","lesson":"Keep original evidence","detection":null,"learnedFrom":[],"evidence":raw,"appliesTo":[]}})).unwrap();
    let req = ObjectSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change,
    };
    let saved = ok(&mut s, |tx| objects::submit(tx, HUMAN, req));
    let crewd_proto::managed_objects::Object::Lesson { lesson } = saved.object else {
        panic!("actual lesson")
    };
    assert_eq!(lesson.draft.evidence, raw);
    let page = subject(
        s.conn(),
        AUTHOR,
        &Subject::Lesson {
            id: lesson.id.clone(),
        },
        None,
    )
    .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(
        page.items
            .iter()
            .all(|edge| edge.origin == Origin::LessonEvidence)
    );
    let before = database_snapshot(&s);
    ok(&mut s, |tx| {
        db::capture_lesson(tx.conn(), &lesson, &tx.now_ts())?;
        Ok(())
    });
    assert_eq!(
        database_snapshot(&s),
        before,
        "same genuine lesson facts never mint duplicate edges"
    );
}

#[test]
fn artifact_insert_failure_rolls_back_actual_done_book_event_and_all_other_effects() {
    for table in ["managed_artifacts", "managed_artifact_edges"] {
        let mut s = fixture();
        let w = work(&mut s, "crew-a", "w-a1");
        s.conn().execute_batch(&format!("CREATE TRIGGER artifact_fault BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic artifact fault'); END")).unwrap();
        let before = database_snapshot(&s);
        let req = WorkUpdateParams {
            id: w.id.clone(),
            expected_revision: w.revision.clone(),
            status: Some(WorkUpdateStatus::Done),
            evidence: Some(WorkEvidenceParam {
                kind: WorkEvidenceKind::Commit,
                value: "a".repeat(40),
            }),
            ..Default::default()
        };
        let failure = crate::run(&mut s, NOW, |tx| {
            crate::work::update(tx, &Sender::Session("w-a1".into()), req.clone())
        })
        .unwrap_err();
        assert!(format!("{failure:?}").contains("synthetic artifact fault"));
        assert_eq!(database_snapshot(&s), before);
        assert_ne!(crate::work::get(s.conn(), &w.id).unwrap().status, "done");
        s.conn()
            .execute_batch("DROP TRIGGER artifact_fault")
            .unwrap();
        ok(&mut s, |tx| {
            crate::work::update(tx, &Sender::Session("w-a1".into()), req)
        });
        assert_eq!(
            subject(s.conn(), HUMAN, &Subject::Work { id: w.id }, None)
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

#[test]
fn target_purge_retires_receipt_body_and_uuid_but_shared_explicit_pins_survive() {
    let mut s = fixture();
    let w = work(&mut s, "crew-x", "cap-x");
    let register = request(&s, "crew-a", Change::Register { input: full() });
    let pinned = ok(&mut s, |tx| submit(tx, HUMAN, register.clone()));
    let b = request(&s, "crew-b", Change::Register { input: full() });
    ok(&mut s, |tx| submit(tx, HUMAN, b));
    let linked = request(
        &s,
        "crew-a",
        Change::Link {
            input: full(),
            subject: Subject::Work { id: w.id },
            relation: Relation::Relates,
        },
    );
    ok(&mut s, |tx| submit(tx, HUMAN, linked.clone()));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert_eq!(known(&s, HUMAN, &register), pinned);
    assert!(get(s.conn(), HUMAN, &pinned.artifact.reference).is_ok());
    assert!(matches!(
        lookup(s.conn(), HUMAN, &linked.scope, "crew-a", &linked.request_id).unwrap(),
        Lookup::NotFound
    ));
    let retired:(bool,Option<String>)=s.conn().query_row("SELECT retired,result_json FROM managed_artifact_receipts WHERE crew_id='crew-a' AND request_id=?1",[&linked.request_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(retired, (true, None));
    let mut reused = linked.clone();
    reused.change = Change::Register {
        input: input(Kind::Test, "must not reuse UUID"),
    };
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, reused)).is_err());
    assert_eq!(database_snapshot(&s), before);
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
    assert!(
        get(s.conn(), HUMAN, &pinned.artifact.reference).is_ok(),
        "another crew's pin keeps shared anchor"
    );
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-b"));
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 0);
}

#[test]
fn relation_matrix_and_target_edit_authority_are_not_broadened_by_linking() {
    let mut s = fixture();
    let change:ObjectChange=serde_json::from_value(json!({"kind":"recordDecision","draft":{"question":"Synthetic choice","choice":"A","rationale":"Factual fixture","about":[{"kind":"crew","id":"crew-a"}]}})).unwrap();
    let req = ObjectSubmit {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: "crew-a".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        change,
    };
    let saved = ok(&mut s, |tx| {
        objects::submit(tx, Actor::Session("w-a2"), req)
    });
    let crewd_proto::managed_objects::Object::Decision { decision } = saved.object else {
        panic!("actual decision")
    };
    let req = request(
        &s,
        "crew-a",
        Change::Link {
            input: full(),
            subject: Subject::Decision {
                id: decision.id.clone(),
            },
            relation: Relation::Relates,
        },
    );
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, AUTHOR, req.clone())).is_err());
    assert_eq!(database_snapshot(&s), before);
    let allowed = ok(&mut s, |tx| submit(tx, Actor::Session("w-a2"), req));
    assert!(matches!(allowed.effect, Effect::Linked { added: true, .. }));
    for relation in [Relation::Evidence, Relation::AppliesTo] {
        let req = request(
            &s,
            "crew-a",
            Change::Link {
                input: input(Kind::File, "src/file.rs"),
                subject: Subject::Decision {
                    id: decision.id.clone(),
                },
                relation,
            },
        );
        let before = database_snapshot(&s);
        assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, req)).is_err());
        assert_eq!(database_snapshot(&s), before);
    }
}

#[test]
fn original_artifact_receipts_reopen_and_restore_without_rescanning_or_reexecution() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap();
    let snapshot = data.join("before-later.db");
    let mut s = Store::open_data_dir(&data).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &crewd_proto::Crew {
            id: "crew-a".into(),
            title: "Artifact disk fixture".into(),
            created_at: crewd_store::ts_from_ms(NOW),
            updated_at: crewd_store::ts_from_ms(NOW),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let req = request(&s, "crew-a", Change::Register { input: full() });
    let saved = ok(&mut s, |tx| submit(tx, HUMAN, req.clone()));
    crewd_store::meta_set(s.conn(), "knowledge.sqlite.v1", "1").unwrap();
    crewd_store::team_data::private_dir(&data.join("knowledge")).unwrap();
    drop(s);
    crewd_store::backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &snapshot).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let before = database_snapshot(&s);
    assert_eq!(known(&s, HUMAN, &req), saved);
    let (same, facts) = ok_at(&mut s, NOW, |tx| submit(tx, HUMAN, req.clone()));
    assert_eq!(same, saved);
    assert!(facts.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let later = request(
        &s,
        "crew-a",
        Change::Register {
            input: input(Kind::Test, "later unretained"),
        },
    );
    ok(&mut s, |tx| submit(tx, HUMAN, later.clone()));
    drop(s);
    crewd_store::backup::restore(&data, &snapshot).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let before = database_snapshot(&s);
    assert!(
        matches!(lookup(s.conn(),HUMAN,&req.scope,"crew-a",&req.request_id).unwrap(),Lookup::Historical{receipt:Some(ref receipt)} if receipt==&saved)
    );
    assert!(matches!(
        lookup(s.conn(), HUMAN, &later.scope, "crew-a", &later.request_id).unwrap(),
        Lookup::Historical { receipt: None }
    ));
    assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, req.clone())).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 1);
    assert_eq!(count(&s, "SELECT count(*) FROM messages"), 0);
    assert_eq!(count(&s, "SELECT count(*) FROM wakes"), 0);
}

#[test]
fn link_existing_preserves_foreign_unassigned_identity_without_creating_missing_foreign_anchor() {
    let mut s = fixture();
    let w = work(&mut s, "crew-x", "cap-x");
    let pin = request(
        &s,
        "crew-a",
        Change::Register {
            input: input(Kind::File, "src/historic.rs"),
        },
    );
    let saved = ok(&mut s, |tx| submit(tx, HUMAN, pin));
    let target = Subject::Work { id: w.id };
    let req = request(
        &s,
        "crew-x",
        Change::LinkExisting {
            reference: saved.artifact.reference.clone(),
            subject: target.clone(),
            relation: Relation::Evidence,
        },
    );
    let (linked, facts) = ok_at(&mut s, NOW, |tx| {
        submit(tx, Actor::Session("cap-x"), req.clone())
    });
    assert!(matches!(
        linked.effect,
        Effect::Linked {
            created: false,
            added: true,
            ..
        }
    ));
    assert_eq!(facts.iter().filter(|fact|matches!(fact,Fact::Progress{session_id,kind:crate::ProgressKind::Ontology} if session_id=="cap-x")).count(),1);
    let mut missing = saved.artifact.reference.clone();
    missing.key = "file:src/not-registered.rs".into();
    let absent = request(
        &s,
        "crew-x",
        Change::LinkExisting {
            reference: missing,
            subject: target.clone(),
            relation: Relation::Evidence,
        },
    );
    let before = database_snapshot(&s);
    assert!(
        crate::run(&mut s, NOW, |tx| submit(
            tx,
            Actor::Session("cap-x"),
            absent
        ))
        .is_err()
    );
    assert_eq!(database_snapshot(&s), before);
    let replay = ok_at(&mut s, NOW, |tx| {
        submit(tx, Actor::Session("cap-x"), req.clone())
    });
    assert_eq!(replay.0, linked);
    assert!(replay.1.is_empty());
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
    assert_eq!(
        get(s.conn(), HUMAN, &saved.artifact.reference).unwrap(),
        saved.artifact
    );
    let page = subject(s.conn(), HUMAN, &target, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].crew_id, "crew-x");
    assert!(
        matches!(&page.items[0].reference.namespace,Namespace::Unassigned{crew_id} if crew_id=="crew-a")
    );
    assert_eq!(known(&s, Actor::Session("cap-x"), &req), linked);
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 0);
}

#[test]
fn numeric_edge_pages_and_duplicate_work_fact_preserve_first_origin_and_sequence() {
    let mut s = fixture();
    let w = work(&mut s, "crew-a", "w-a1");
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a1".into()),
            WorkUpdateParams {
                id: w.id.clone(),
                expected_revision: w.revision.clone(),
                status: Some(WorkUpdateStatus::Done),
                evidence: Some(WorkEvidenceParam {
                    kind: WorkEvidenceKind::Commit,
                    value: "a".repeat(40),
                }),
                ..Default::default()
            },
        )
    });
    let target = Subject::Work { id: w.id };
    let first = subject(s.conn(), HUMAN, &target, None)
        .unwrap()
        .items
        .remove(0);
    let reference = first.reference.clone();
    let Origin::WorkDone {
        event_seq: original_seq,
    } = &first.origin
    else {
        panic!("actual Done origin")
    };
    let later_seq = (original_seq.parse::<u64>().unwrap() + 100).to_string();
    let before = database_snapshot(&s);
    // Storage-level dedupe probe, not a claim that a second Done event occurred.
    let repeated = ok(&mut s, |tx| {
        Ok(db::link(
            tx.conn(),
            "crew-a",
            &reference,
            &target,
            Relation::Evidence,
            &Origin::WorkDone {
                event_seq: later_seq,
            },
            &tx.now_ts(),
        )?)
    });
    assert_eq!(repeated, (first.clone(), false));
    assert_eq!(
        database_snapshot(&s),
        before,
        "duplicate fact does not advance sqlite_sequence or replace first provenance"
    );
    let manual = request(
        &s,
        "crew-a",
        Change::LinkExisting {
            reference: reference.clone(),
            subject: target.clone(),
            relation: Relation::Evidence,
        },
    );
    let linked = ok(&mut s, |tx| submit(tx, AUTHOR, manual.clone()));
    let Effect::Linked { edge, .. } = &linked.effect else {
        panic!("independent manual origin")
    };
    assert!(edge.id.parse::<i64>().unwrap() > first.id.parse::<i64>().unwrap());
    let duplicate = request(&s, "crew-a", manual.change.clone());
    let sequence: i64 = s
        .conn()
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='managed_artifact_edges'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (same, facts) = ok_at(&mut s, NOW, |tx| submit(tx, AUTHOR, duplicate));
    assert!(matches!(
        same.effect,
        Effect::Linked {
            created: false,
            added: false,
            ..
        }
    ));
    assert!(facts.is_empty());
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='managed_artifact_edges'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        sequence
    );
    for n in 0..16 {
        let req = request(
            &s,
            "crew-a",
            Change::Link {
                input: input(Kind::Test, &format!("numeric-page-{n}")),
                subject: target.clone(),
                relation: Relation::Relates,
            },
        );
        ok(&mut s, |tx| submit(tx, AUTHOR, req));
    }
    let page1 = subject(s.conn(), AUTHOR, &target, None).unwrap();
    assert_eq!(page1.items.len(), 16);
    let page2 = subject(s.conn(), AUTHOR, &target, page1.next_after.as_deref()).unwrap();
    assert_eq!(page2.items.len(), 2);
    assert!(page2.next_after.is_none());
    let ids = page1
        .items
        .iter()
        .chain(&page2.items)
        .map(|edge| edge.id.parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    for invalid in ["0", "01", "+1", "-1", "9223372036854775808", "not-a-number"] {
        assert!(subject(s.conn(), HUMAN, &target, Some(invalid)).is_err());
    }
}

#[test]
fn saved_artifact_receipt_rejects_valid_json_tampering_without_latest_state_reads() {
    let mut s = fixture();
    let w = work(&mut s, "crew-a", "w-a1");
    let req = request(
        &s,
        "crew-a",
        Change::Link {
            input: full(),
            subject: Subject::Work { id: w.id },
            relation: Relation::Evidence,
        },
    );
    let saved = ok(&mut s, |tx| submit(tx, AUTHOR, req.clone()));
    let raw = serde_json::to_value(&saved).unwrap();
    let mut wrong_effect = raw.clone();
    wrong_effect["effect"] = json!({"kind":"registered","created":true,"claimAdded":true});
    let mut wrong_ref = raw.clone();
    wrong_ref["artifact"]["reference"]["key"] = json!(format!("commit:{}", "b".repeat(40)));
    let mut wrong_edge = raw.clone();
    wrong_edge["effect"]["edge"]["relation"] = json!("relates");
    for corrupt in [wrong_effect, wrong_ref, wrong_edge] {
        s.conn().execute_batch("SAVEPOINT corrupt_receipt").unwrap();
        s.conn()
            .execute(
                "UPDATE managed_artifact_receipts SET result_json=?1 WHERE request_id=?2",
                rusqlite::params![serde_json::to_string(&corrupt).unwrap(), req.request_id],
            )
            .unwrap();
        let before = database_snapshot(&s);
        assert!(lookup(s.conn(), AUTHOR, &req.scope, "crew-a", &req.request_id).is_err());
        // Do not call collab::run inside this synthetic enclosing transaction.
        assert_eq!(database_snapshot(&s), before);
        s.conn()
            .execute_batch("ROLLBACK TO corrupt_receipt; RELEASE corrupt_receipt")
            .unwrap();
        assert_eq!(known(&s, AUTHOR, &req), saved);
    }
    s.conn().execute_batch("SAVEPOINT corrupt_receipt").unwrap();
    s.conn().execute("UPDATE managed_artifact_receipts SET target_number=target_number+1000 WHERE request_id=?1",[&req.request_id]).unwrap();
    assert!(lookup(s.conn(), AUTHOR, &req.scope, "crew-a", &req.request_id).is_err());
    s.conn()
        .execute_batch("ROLLBACK TO corrupt_receipt; RELEASE corrupt_receipt")
        .unwrap();
    assert_eq!(known(&s, AUTHOR, &req), saved);
    let Effect::Linked { edge, .. } = &saved.effect else {
        panic!("manual edge")
    };
    let unlink = request(
        &s,
        "crew-a",
        Change::Unlink {
            reference: saved.artifact.reference.clone(),
            subject: edge.subject.clone(),
            relation: edge.relation,
            expected_edge_id: edge.id.clone(),
        },
    );
    let unlinked = ok(&mut s, |tx| submit(tx, AUTHOR, unlink.clone()));
    assert!(get(s.conn(), AUTHOR, &saved.artifact.reference).is_err());
    assert_eq!(
        known(&s, AUTHOR, &req),
        saved,
        "original link remains readable after edge and anchor GC"
    );
    assert_eq!(
        known(&s, AUTHOR, &unlink),
        unlinked,
        "original unlink is also verified without a current anchor"
    );
}
