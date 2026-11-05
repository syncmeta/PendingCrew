//! Real historical schema construction and actual collab commits; no downgraded latest fixtures.
use super::*;
use crate::{
    Change as Fact, Sender,
    testkit::{NOW, count, database_snapshot, ok, ok_at},
};
use crewd_proto::{
    Crew, Role, Session, WorkCreateParams, WorkEvidenceKind, WorkEvidenceParam, WorkUpdateParams,
    WorkUpdateStatus,
    managed_artifacts::{Origin, Subject},
    managed_objects::{Change as ObjectChange, Submit as ObjectSubmit},
};
use crewd_store::Store;
use serde_json::json;
const HUMAN: Actor<'static> = Actor::Human("history-human");
const AGENT: Actor<'static> = Actor::Session("w-a");
fn seed(s: &Store) {
    for (crew, number) in [("crew-a", 1), ("crew-x", 2)] {
        crewd_store::crews::upsert(
            s.conn(),
            &Crew {
                id: crew.into(),
                title: format!("Synthetic {crew}"),
                number: Some(number),
                created_at: crewd_store::ts_from_ms(NOW),
                updated_at: crewd_store::ts_from_ms(NOW),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::sessions::upsert(
        s.conn(),
        &Session {
            id: "w-a".into(),
            crew_id: "crew-a".into(),
            role: Role::Worker,
            executor: "codex".into(),
            ext: Some(2),
            updated_at: crewd_store::ts_from_ms(NOW),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_store::schema::backfill_v4(s.conn()).unwrap();
}
fn legacy() -> Store {
    use crewd_work::{Actor, Config, CrewId, Evidence, NewItem, Ts, WorkBook};
    let s = crewd_store::testing::open_legacy_store(8).unwrap();
    seed(&s);
    let mut b = WorkBook::new(CrewId::new("crew-a"), Config::default());
    let one = b
        .create_item(
            &Actor::worker("w-a"),
            Ts(NOW),
            NewItem::titled("Retained real legacy event").owned_by("w-a"),
        )
        .unwrap()
        .value;
    b.mark_done(
        &Actor::worker("w-a"),
        Ts(NOW + 1),
        one,
        Evidence::Commit("abc1234".into()),
    )
    .unwrap();
    let two = b
        .create_item(
            &Actor::worker("w-a"),
            Ts(NOW + 2),
            NewItem::titled("Real snapshot-only completion").owned_by("w-a"),
        )
        .unwrap()
        .value;
    b.mark_done(
        &Actor::worker("w-a"),
        Ts(NOW + 3),
        two,
        Evidence::Url("https://snapshot.example/only".into()),
    )
    .unwrap();
    let state = serde_json::to_string(b.state()).unwrap();
    let events = b.take_log();
    let tx = s.conn().unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO work_books(crew_id,state,updated_at) VALUES('crew-a',?1,?2)",
        rusqlite::params![state, crewd_store::ts_from_ms(NOW + 3)],
    )
    .unwrap();
    // A sparse retained journal, containing only the first genuine Done, not fabricated seq filler.
    crewd_store::work_events::append(&tx, "crew-a", &[events[1].clone()]).unwrap();
    crewd_store::work_events::backfill_references(&tx).unwrap();
    tx.commit().unwrap();
    crewd_store::schema::migrate(s.conn()).unwrap();
    s
}
fn work(s: &mut Store) -> crewd_proto::WorkPlan {
    ok(s, |tx| {
        crate::work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-a".into(),
                title: "Historical org work".into(),
                owner: Some("w-a".into()),
                ..Default::default()
            },
        )
    })
}
fn org_old() -> Store {
    let mut s = crewd_store::testing::open_legacy_store(27).unwrap();
    seed(&s);
    let w = work(&mut s);
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a".into()),
            WorkUpdateParams {
                id: w.id,
                expected_revision: w.revision,
                status: Some(WorkUpdateStatus::Done),
                evidence: Some(WorkEvidenceParam {
                    kind: WorkEvidenceKind::Commit,
                    value: "a".repeat(40),
                }),
                ..Default::default()
            },
        )
    });
    let req=ObjectSubmit{scope:objects::scope(s.conn()).unwrap(),crew_id:"crew-a".into(),request_id:uuid::Uuid::new_v4().to_string(),change:serde_json::from_value::<ObjectChange>(json!({"kind":"recordLesson","draft":{"happened":"Original retained fact","lesson":"Original preserved lesson","evidence":["https://lesson.example/original"],"learnedFrom":[],"appliesTo":[]}})).unwrap()};
    ok(&mut s, |tx| objects::submit(tx, HUMAN, req));
    crewd_store::schema::migrate(s.conn()).unwrap();
    s
}
fn begin_params(s: &Store, crew: &str) -> BeginParams {
    BeginParams {
        scope: objects::scope(s.conn()).unwrap(),
        crew_id: crew.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn step_params(job: &Job, limit: u16) -> StepParams {
    StepParams {
        scope: job.scope.clone(),
        crew_id: job.crew_id.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        job_id: job.id.clone(),
        expected_revision: job.revision.clone(),
        expected_cursor: job.cursor.clone(),
        limit,
    }
}
fn known(s: &Store, actor: Actor<'_>, scope: &Scope, crew: &str, uuid: &str) -> Receipt {
    match lookup(s.conn(), actor, scope, crew, uuid).unwrap() {
        Lookup::Known { receipt } => receipt,
        v => panic!("original receipt missing: {v:?}"),
    }
}
fn source(s: &Store) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    database_snapshot(s)
        .into_iter()
        .filter(|(table, _)| {
            matches!(
                table.as_str(),
                "legacy_work_books"
                    | "work_events"
                    | "organization_work_book"
                    | "organization_work_events"
                    | "managed_lessons"
                    | "sessions"
                    | "messages"
                    | "wakes"
                    | "object_refs"
            )
        })
        .collect()
}
fn finish(s: &mut Store, mut job: Job, limit: u16) -> Vec<Receipt> {
    let mut receipts = vec![];
    for _ in 0..30 {
        if job.cursor.phase == Phase::Complete {
            return receipts;
        }
        let req = step_params(&job, limit);
        let (saved, facts) = ok_at(s, NOW + 100, |tx| step(tx, HUMAN, req));
        let Effect::Advanced { delta, .. } = &saved.effect else {
            panic!("advanced")
        };
        assert!(db::number(&delta.scanned).unwrap() <= u64::from(limit));
        assert!(
            facts.iter().all(|fact| matches!(
                fact,
                Fact::ArtifactHistoryChanged { .. } | Fact::ArtifactChanged { .. }
            )),
            "no model progress, work event, chat or wake fact"
        );
        job = saved.job.clone();
        receipts.push(saved);
    }
    panic!("bounded synthetic job did not complete")
}

#[test]
fn legacy_sparse_event_and_frozen_snapshot_are_bounded_honest_and_hidden_by_default() {
    let mut s = legacy();
    let original = source(&s);
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_artifacts"),
        0,
        "migration never indexes old facts implicitly"
    );
    let req = begin_params(&s, "crew-a");
    let opened = ok(&mut s, |tx| begin(tx, HUMAN, req.clone()));
    assert_eq!(opened.job.cut.legacy_event_seq, "2");
    assert_eq!(opened.job.cut.legacy_retained_events, "1");
    assert_eq!(opened.job.cut.legacy_snapshot_seq, "4");
    assert_eq!(opened.job.cut.legacy_item, "2");
    assert_eq!(opened.job.counts, db::zero());
    assert_eq!(source(&s), original);
    let receipts = finish(&mut s, opened.job, 1);
    let last = &receipts.last().unwrap().job;
    assert_eq!(last.counts.scanned, "3");
    assert_eq!(last.counts.done_events, "1");
    assert_eq!(last.counts.legacy_snapshots, "2");
    assert_eq!(last.counts.artifacts_added, "2");
    assert_eq!(last.counts.edges_added, "2");
    assert_eq!(
        source(&s),
        original,
        "no author, evidence, sequence, workbook, progress, message or wake is rewritten"
    );
    for local in ["W1", "W2"] {
        let r = crewd_store::object_refs::resolve(
            s.conn(),
            crewd_store::object_refs::ObjectKind::Plan,
            "crew-a",
            &format!("work:{local}"),
        )
        .unwrap()
        .unwrap();
        let target = Subject::Work { id: r.id() };
        assert!(
            crate::managed_artifacts::subject(s.conn(), AGENT, &target, None)
                .unwrap()
                .items
                .is_empty()
        );
        let page =
            crate::managed_artifacts::subject_with_legacy(s.conn(), AGENT, &target, None, true)
                .unwrap();
        assert_eq!(page.items.len(), 1);
        if local == "W1" {
            assert!(
                matches!(&page.items[0].origin,Origin::LegacyWorkDone{crew_id,local_seq,legacy_work_id} if crew_id=="crew-a" && local_seq=="2" && legacy_work_id=="W1")
            );
        } else {
            assert!(
                matches!(&page.items[0].origin,Origin::LegacyWorkSnapshot{crew_id,legacy_work_id,snapshot_seq} if crew_id=="crew-a" && legacy_work_id=="W2" && snapshot_seq=="4")
            );
        }
    }
}

#[test]
fn original_step_replay_precedes_cas_and_concurrent_new_uuid_stale_cursor_cannot_advance() {
    let mut s = legacy();
    let begin_request = begin_params(&s, "crew-a");
    let opened = ok(&mut s, |tx| begin(tx, AGENT, begin_request.clone()));
    let before = database_snapshot(&s);
    let replay = ok_at(&mut s, NOW + 1, |tx| {
        begin(tx, AGENT, begin_request.clone())
    });
    assert_eq!(replay.0, opened);
    assert!(replay.1.is_empty());
    assert_eq!(database_snapshot(&s), before);
    let reuse = begin_params(&s, "crew-a");
    let joined = ok(&mut s, |tx| begin(tx, HUMAN, reuse));
    assert_eq!(joined.job.id, opened.job.id);
    assert_eq!(joined.effect, Effect::Begun { created: false });
    let req = step_params(&opened.job, 1);
    let saved = ok(&mut s, |tx| step(tx, AGENT, req.clone()));
    let mut stale = req.clone();
    stale.request_id = uuid::Uuid::new_v4().to_string();
    let before = database_snapshot(&s);
    let error = crate::run(&mut s, NOW, |tx| step(tx, HUMAN, stale)).unwrap_err();
    assert!(matches!(error,CollabError::Conflict(ref r) if r.code==Code::ArtifactHistoryChanged));
    assert_eq!(database_snapshot(&s), before);
    finish(&mut s, saved.job.clone(), 1);
    let before = database_snapshot(&s);
    let replay = ok_at(&mut s, NOW + 5, |tx| step(tx, AGENT, req.clone()));
    assert_eq!(replay.0, saved);
    assert!(replay.1.is_empty());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(
        known(&s, AGENT, &req.scope, "crew-a", &req.request_id),
        saved
    );
    let mut conflict = req.clone();
    conflict.limit = 2;
    assert!(crate::run(&mut s, NOW, |tx| step(tx, AGENT, conflict)).is_err());
    assert_eq!(database_snapshot(&s), before);
}

#[test]
fn fixed_cut_org_events_and_original_lesson_evidence_exclude_later_sources() {
    let mut s = org_old();
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 0);
    let req = begin_params(&s, "crew-a");
    let opened = ok(&mut s, |tx| begin(tx, HUMAN, req));
    let original = source(&s);
    let old_lesson_cut = opened.job.cut.lesson_number.clone();
    let later = work(&mut s);
    ok(&mut s, |tx| {
        crate::work::update(
            tx,
            &Sender::Session("w-a".into()),
            WorkUpdateParams {
                id: later.id,
                expected_revision: later.revision,
                status: Some(WorkUpdateStatus::Done),
                evidence: Some(WorkEvidenceParam {
                    kind: WorkEvidenceKind::Url,
                    value: "https://new.example/excluded".into(),
                }),
                ..Default::default()
            },
        )
    });
    let now_source = source(&s);
    let receipts = finish(&mut s, opened.job, 64);
    let job = &receipts.last().unwrap().job;
    assert_eq!(job.counts.done_events, "1");
    assert_eq!(job.counts.lessons, "1");
    assert_eq!(job.cut.lesson_number, old_lesson_cut);
    assert_eq!(source(&s), now_source);
    assert_ne!(source(&s), original);
    assert_eq!(job.counts.artifacts_added, "2");
    assert_eq!(job.counts.edges_added, "2");
}

#[test]
fn derived_edge_or_original_step_receipt_failure_rolls_back_job_cursor_and_all_partial_capture() {
    for table in ["managed_artifact_edges", "artifact_history_receipts"] {
        let mut s = org_old();
        let req = begin_params(&s, "crew-a");
        let opened = ok(&mut s, |tx| begin(tx, HUMAN, req));
        let step_request = step_params(&opened.job, 64);
        s.conn().execute_batch(&format!("CREATE TRIGGER history_fault BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic history fault'); END")).unwrap();
        let before = database_snapshot(&s);
        let error =
            crate::run(&mut s, NOW, |tx| step(tx, HUMAN, step_request.clone())).unwrap_err();
        assert!(format!("{error:?}").contains("synthetic history fault"));
        assert_eq!(database_snapshot(&s), before);
        assert!(matches!(
            lookup(
                s.conn(),
                HUMAN,
                &step_request.scope,
                "crew-a",
                &step_request.request_id
            )
            .unwrap(),
            Lookup::NotFound
        ));
        s.conn()
            .execute_batch("DROP TRIGGER history_fault")
            .unwrap();
        let saved = ok(&mut s, |tx| step(tx, HUMAN, step_request.clone()));
        assert_eq!(saved.job.counts.artifacts_added, "2");
        assert_eq!(
            known(
                &s,
                HUMAN,
                &step_request.scope,
                "crew-a",
                &step_request.request_id
            ),
            saved
        );
    }
}

#[test]
fn human_maintains_archived_facts_while_agent_remains_live_own_crew_only() {
    let mut s = legacy();
    let foreign = begin_params(&s, "crew-x");
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| begin(tx, AGENT, foreign)).is_err());
    assert_eq!(database_snapshot(&s), before);
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
    let req = begin_params(&s, "crew-a");
    assert!(crate::run(&mut s, NOW, |tx| begin(tx, AGENT, req.clone())).is_err());
    assert!(
        status(
            s.conn(),
            AGENT,
            StatusParams {
                crew_id: "crew-a".into(),
                job_id: None
            }
        )
        .is_err()
    );
    let opened = ok(&mut s, |tx| begin(tx, HUMAN, req));
    finish(&mut s, opened.job, 64);
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifact_edges"), 2);
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
    assert_eq!(count(&s, "SELECT count(*) FROM artifact_history_jobs"), 0);
    assert_eq!(
        count(&s, "SELECT count(*) FROM artifact_history_receipts"),
        0
    );
}

#[test]
fn strict_job_and_immutable_receipt_readers_refuse_valid_json_scope_and_count_tampering() {
    let mut s = legacy();
    let req = begin_params(&s, "crew-a");
    let opened = ok(&mut s, |tx| begin(tx, HUMAN, req.clone()));
    let raw: String = s
        .conn()
        .query_row(
            "SELECT body FROM artifact_history_jobs WHERE id=?1",
            [&opened.job.id],
            |r| r.get(0),
        )
        .unwrap();
    let mut corrupt: serde_json::Value = serde_json::from_str(&raw).unwrap();
    corrupt["scope"]["restoreEpoch"] = json!(uuid::Uuid::new_v4().to_string());
    s.conn()
        .execute(
            "UPDATE artifact_history_jobs SET body=?1 WHERE id=?2",
            rusqlite::params![serde_json::to_string(&corrupt).unwrap(), opened.job.id],
        )
        .unwrap();
    let before = database_snapshot(&s);
    assert!(
        status(
            s.conn(),
            HUMAN,
            StatusParams {
                crew_id: "crew-a".into(),
                job_id: None
            }
        )
        .is_err()
    );
    let another = begin_params(&s, "crew-a");
    assert!(crate::run(&mut s, NOW, |tx| begin(tx, HUMAN, another)).is_err());
    assert_eq!(database_snapshot(&s), before);
    s.conn()
        .execute(
            "UPDATE artifact_history_jobs SET body=?1 WHERE id=?2",
            rusqlite::params![raw, opened.job.id],
        )
        .unwrap();
    let reused_request = begin_params(&s, "crew-a");
    let reused = ok(&mut s, |tx| begin(tx, HUMAN, reused_request.clone()));
    assert_eq!(reused.effect, Effect::Begun { created: false });
    let mut fake_complete = serde_json::to_value(&reused).unwrap();
    fake_complete["job"]["cursor"] = json!({"phase":"complete","after":"0"});
    s.conn()
        .execute(
            "UPDATE artifact_history_receipts SET body=?1 WHERE request_id=?2",
            rusqlite::params![
                serde_json::to_string(&fake_complete).unwrap(),
                reused_request.request_id
            ],
        )
        .unwrap();
    let before = database_snapshot(&s);
    assert!(
        lookup(
            s.conn(),
            HUMAN,
            &reused_request.scope,
            "crew-a",
            &reused_request.request_id
        )
        .is_err(),
        "a reused Begin never originally returned a complete job"
    );
    assert_eq!(database_snapshot(&s), before);
    s.conn()
        .execute(
            "UPDATE artifact_history_receipts SET body=?1 WHERE request_id=?2",
            rusqlite::params![
                serde_json::to_string(&reused).unwrap(),
                reused_request.request_id
            ],
        )
        .unwrap();
    assert_eq!(
        known(
            &s,
            HUMAN,
            &reused_request.scope,
            "crew-a",
            &reused_request.request_id
        ),
        reused
    );
    let step_request = step_params(&opened.job, 1);
    let saved = ok(&mut s, |tx| step(tx, HUMAN, step_request.clone()));
    let original = serde_json::to_string(&saved).unwrap();
    let mut bad_delta = serde_json::to_value(&saved).unwrap();
    bad_delta["effect"]["delta"]["artifactsAdded"] = json!("1000");
    let mut bad_phase = serde_json::to_value(&saved).unwrap();
    bad_phase["job"]["cursor"] = json!({"phase":"legacyEvents","after":"0"});
    for corrupt in [bad_delta, bad_phase] {
        s.conn()
            .execute(
                "UPDATE artifact_history_receipts SET body=?1 WHERE request_id=?2",
                rusqlite::params![
                    serde_json::to_string(&corrupt).unwrap(),
                    step_request.request_id
                ],
            )
            .unwrap();
        let before = database_snapshot(&s);
        assert!(
            lookup(
                s.conn(),
                HUMAN,
                &step_request.scope,
                "crew-a",
                &step_request.request_id
            )
            .is_err()
        );
        assert!(crate::run(&mut s, NOW, |tx| step(tx, HUMAN, step_request.clone())).is_err());
        assert_eq!(database_snapshot(&s), before);
        s.conn()
            .execute(
                "UPDATE artifact_history_receipts SET body=?1 WHERE request_id=?2",
                rusqlite::params![original, step_request.request_id],
            )
            .unwrap();
        assert_eq!(
            known(
                &s,
                HUMAN,
                &step_request.scope,
                "crew-a",
                &step_request.request_id
            ),
            saved
        );
    }
}

#[test]
fn real_disk_reopen_and_official_restore_make_old_job_historical_without_implicit_reindex() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().canonicalize().unwrap();
    let retained = data.join("partial-job.db");
    let mut memory = org_old();
    let begin_request = begin_params(&memory, "crew-a");
    let opened = ok(&mut memory, |tx| begin(tx, HUMAN, begin_request));
    let original_request = step_params(&opened.job, 1);
    let partial = ok(&mut memory, |tx| step(tx, HUMAN, original_request.clone()));
    assert_ne!(partial.job.cursor.phase, Phase::Complete);
    assert_eq!(partial.job.counts.artifacts_added, "0");
    crewd_store::meta_set(memory.conn(), "knowledge.sqlite.v1", "1").unwrap();
    memory
        .conn()
        .execute(
            "VACUUM INTO ?1",
            [data.join(crewd_store::DB_FILE).to_str().unwrap()],
        )
        .unwrap();
    drop(memory);
    crewd_store::team_data::private_dir(&data.join("knowledge")).unwrap();
    let s = Store::open_data_dir(&data).unwrap();
    let before = database_snapshot(&s);
    assert_eq!(
        known(
            &s,
            HUMAN,
            &original_request.scope,
            "crew-a",
            &original_request.request_id
        ),
        partial
    );
    let current = status(
        s.conn(),
        HUMAN,
        StatusParams {
            crew_id: "crew-a".into(),
            job_id: Some(partial.job.id.clone()),
        },
    )
    .unwrap();
    assert!(current.can_continue);
    assert_eq!(current.job.as_ref(), Some(&partial.job));
    assert_eq!(database_snapshot(&s), before);
    drop(s);
    crewd_store::backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &retained).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let later_request = step_params(&partial.job, 64);
    let later = ok(&mut s, |tx| step(tx, HUMAN, later_request.clone()));
    assert_eq!(later.job.cursor.phase, Phase::Complete);
    assert_eq!(later.job.counts.artifacts_added, "2");
    drop(s);
    crewd_store::backup::restore(&data, &retained).unwrap();
    let mut s = Store::open_data_dir(&data).unwrap();
    let before = database_snapshot(&s);
    assert!(
        matches!(lookup(s.conn(),HUMAN,&original_request.scope,"crew-a",&original_request.request_id).unwrap(),Lookup::Historical{receipt:Some(ref receipt)} if receipt==&partial)
    );
    assert!(matches!(
        lookup(
            s.conn(),
            HUMAN,
            &later_request.scope,
            "crew-a",
            &later_request.request_id
        )
        .unwrap(),
        Lookup::Historical { receipt: None }
    ));
    let historic = status(
        s.conn(),
        HUMAN,
        StatusParams {
            crew_id: "crew-a".into(),
            job_id: Some(partial.job.id.clone()),
        },
    )
    .unwrap();
    assert!(!historic.can_continue);
    assert_eq!(historic.job.as_ref(), Some(&partial.job));
    let current = status(
        s.conn(),
        HUMAN,
        StatusParams {
            crew_id: "crew-a".into(),
            job_id: None,
        },
    )
    .unwrap();
    assert!(current.job.is_none());
    assert!(!current.can_continue);
    assert!(crate::run(&mut s, NOW, |tx| step(tx, HUMAN, original_request.clone())).is_err());
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(count(&s, "SELECT count(*) FROM managed_artifacts"), 0);
    let fresh = begin_params(&s, "crew-a");
    let restarted = ok(&mut s, |tx| begin(tx, HUMAN, fresh));
    assert_ne!(restarted.job.id, partial.job.id);
    assert_eq!(restarted.job.counts, db::zero());
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_artifacts"),
        0,
        "only a new explicit step may reindex restored facts"
    );
}

#[test]
fn corrupt_source_rolls_back_and_missing_target_is_counted_without_ghost_identity() {
    let mut s = org_old();
    let req = begin_params(&s, "crew-a");
    let opened = ok(&mut s, |tx| begin(tx, HUMAN, req));
    let seq: i64 = s
        .conn()
        .query_row(
            "SELECT MAX(seq) FROM organization_work_events WHERE crew_id='crew-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let original: String = s
        .conn()
        .query_row(
            "SELECT body FROM organization_work_events WHERE seq=?1",
            [seq],
            |r| r.get(0),
        )
        .unwrap();
    let mut malformed: serde_json::Value = serde_json::from_str(&original).unwrap();
    malformed["seq"] = json!(seq + 1);
    s.conn()
        .execute(
            "UPDATE organization_work_events SET body=?1 WHERE seq=?2",
            rusqlite::params![serde_json::to_string(&malformed).unwrap(), seq],
        )
        .unwrap();
    let request = step_params(&opened.job, 64);
    let before = database_snapshot(&s);
    assert!(crate::run(&mut s, NOW, |tx| step(tx, HUMAN, request.clone())).is_err());
    assert_eq!(database_snapshot(&s), before);
    s.conn()
        .execute(
            "UPDATE organization_work_events SET body=?1 WHERE seq=?2",
            rusqlite::params![original, seq],
        )
        .unwrap();
    // Simulate a retired registry target, leaving the retained fact to be accounted rather than recreated.
    s.conn()
        .execute(
            "DELETE FROM object_refs WHERE kind='plan' AND crew_id='crew-a'",
            [],
        )
        .unwrap();
    let references = count(&s, "SELECT count(*) FROM object_refs");
    let saved = ok(&mut s, |tx| step(tx, HUMAN, request));
    assert_eq!(saved.job.counts.missing_targets, "1");
    assert_eq!(saved.job.counts.done_events, "1");
    assert_eq!(count(&s, "SELECT count(*) FROM object_refs"), references);
    assert_eq!(
        count(&s, "SELECT count(*) FROM managed_artifacts"),
        1,
        "only surviving original lesson is indexed"
    );
}
