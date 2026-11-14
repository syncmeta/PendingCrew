use super::*;
use crate::{
    Sender, channels, crews, requirements,
    testkit::{NOW, ok, ok_at, store},
    work,
};
use crewd_proto::{
    ChannelKind, TodoCreateParams, WorkCloseParams, WorkClosureKind, WorkCreateParams,
    WorkEvidenceKind, WorkEvidenceParam, WorkPlan,
};
use crewd_store::Store;

fn plan(s: &mut Store, crew: &str, channel: Option<String>) -> WorkPlan {
    ok(s, |tx| {
        work::create(
            tx,
            &Sender::Human,
            WorkCreateParams {
                crew_id: crew.into(),
                request_id: channel.as_ref().map(|_| uuid::Uuid::new_v4().to_string()),
                channel_id: channel,
                title: "bounded periodic source fixture".into(),
                ..Default::default()
            },
        )
    })
}

fn plans(s: &mut Store, crew: &str, n: usize) -> Vec<String> {
    (0..n).map(|_| plan(s, crew, None).id).collect()
}

fn entry(s: &mut Store, crew: &str) -> String {
    ok(s, |tx| {
        requirements::create(
            tx,
            &Sender::Human,
            TodoCreateParams {
                request_id: uuid::Uuid::new_v4().to_string(),
                crew_id: crew.into(),
                channel_id: None,
                text: "independent periodic entry fixture".into(),
            },
        )
    })
    .todo
    .requirement_id
    .unwrap()
}

fn topic(s: &mut Store, crew: &str) -> String {
    ok(s, |tx| {
        channels::create(
            tx,
            &Sender::Human,
            channels::NewChannel {
                crew_id: crew.into(),
                kind: ChannelKind::Topic,
                name: "bounded periodic topic fixture".into(),
                ..Default::default()
            },
        )
    })
    .id
}

fn old_finished_topic(s: &mut Store) -> String {
    let id = topic(s, "crew-a");
    let work = plan(s, "crew-a", Some(id.clone()));
    ok(s, |tx| {
        work::close_factual(
            tx,
            &Sender::Human,
            WorkCloseParams {
                id: work.id,
                expected_revision: work.revision,
                kind: WorkClosureKind::Done,
                note: "real fixture closure".into(),
                evidence: WorkEvidenceParam {
                    kind: WorkEvidenceKind::Text,
                    value: "verified fixture evidence".into(),
                },
            },
        )
    });
    crewd_store::messages::insert(
        s.conn(),
        &crewd_proto::Message {
            id: uuid::Uuid::new_v4().to_string(),
            channel_id: id.clone(),
            sender_kind: crewd_proto::SenderKind::Human,
            text: "old synthetic activity".into(),
            created_at: crewd_store::ts_from_ms(NOW - archive::IDLE_MS),
            ..Default::default()
        },
        None,
    )
    .unwrap()
    .expect("fresh synthetic history message inserted");
    id
}

/// Move only the derived round-robin cursor. Run/proof state is always produced
/// by the actual tick and its original patrol_one savepoint.
fn tick_crew(s: &mut Store, crew: &str, at: i64) -> PatrolRun {
    let previous: Option<String> = s
        .conn()
        .query_row(
            "SELECT max(id) FROM crews WHERE archived_at IS NULL AND id<?1",
            [crew],
            |r| r.get(0),
        )
        .unwrap();
    meta_set(s.conn(), CURSOR, previous.as_deref().unwrap_or("")).unwrap();
    ok_at(s, at, tick);
    assert_eq!(meta_get(s.conn(), CURSOR).unwrap().as_deref(), Some(crew));
    status(s.conn(), crew).unwrap().run.unwrap()
}

struct ArmedFault;
impl Drop for ArmedFault {
    fn drop(&mut self) {
        TEST_FAULT.with(|point| point.set(None));
    }
}
fn arm(point: TestFaultPoint) -> ArmedFault {
    TEST_FAULT.with(|current| {
        assert!(current.get().is_none());
        current.set(Some(point));
    });
    TEST_FAULT_OBSERVATION.with(|seen| *seen.borrow_mut() = None);
    ArmedFault
}
fn observation() -> TestFaultObservation {
    TEST_FAULT_OBSERVATION.with(|seen| seen.borrow_mut().take().expect("real fault site reached"))
}

#[test]
fn empty_crew_naturally_covers_all_three_sources_in_one_selected_tick() {
    let mut s = store();
    let wakes = crate::wakes::list(s.conn(), None, 100).unwrap();
    let done = tick_crew(&mut s, "crew-a", NOW);
    assert_eq!(done.scan, WorkPatrolScan::Channels);
    assert_eq!(done.after, None);
    assert_eq!(done.completed_at_ms, Some(NOW));
    assert_eq!(
        done.scanned_counts,
        BTreeMap::from([
            ("work".into(), 0),
            ("entries".into(), 0),
            ("channels".into(), 0)
        ])
    );
    assert!(done.findings.is_empty() && done.counts.is_empty() && !done.truncated);
    assert!(uuid::Uuid::parse_str(&done.run_id).is_ok());
    assert_eq!(
        serde_json::to_value(secretary_observation(s.conn(), "crew-a").unwrap().unwrap()).unwrap(),
        serde_json::to_value(&done).unwrap()
    );
    assert_eq!(crate::wakes::list(s.conn(), None, 100).unwrap(), wakes);
}

#[test]
fn small_mixed_sources_complete_with_real_scoped_findings_and_no_execution() {
    let mut s = store();
    let local = plans(&mut s, "crew-a", 4);
    let foreign = plans(&mut s, "crew-b", 7);
    let entries = (0..3).map(|_| entry(&mut s, "crew-a")).collect::<Vec<_>>();
    topic(&mut s, "crew-a");
    topic(&mut s, "crew-a");
    let before = crate::wakes::list(s.conn(), None, 100).unwrap();
    let done = tick_crew(&mut s, "crew-a", NOW);
    assert_eq!(done.completed_at_ms, Some(NOW));
    assert_eq!(done.scanned_counts["work"], 4);
    assert_eq!(done.scanned_counts["entries"], 3);
    assert_eq!(done.scanned_counts["channels"], 2);
    assert_eq!(done.findings["unowned"], local);
    assert_eq!(done.findings["unclaimed"], entries);
    assert!(
        !done.findings["unowned"]
            .iter()
            .any(|id| foreign.contains(id))
    );
    assert!(completed_semantic_revision(&done).is_ok());
    assert_eq!(crate::wakes::list(s.conn(), None, 100).unwrap(), before);
}

#[test]
fn full_twenty_five_cannot_probe_or_assume_the_next_phase_empty() {
    let mut s = store();
    plans(&mut s, "crew-a", 25);
    let partial = {
        let _armed = arm(TestFaultPoint::BeforeEntriesRead);
        let partial = tick_crew(&mut s, "crew-a", NOW);
        assert_eq!(
            TEST_FAULT.with(|point| point.get()),
            Some(TestFaultPoint::BeforeEntriesRead)
        );
        partial
    };
    assert_eq!(partial.scanned_counts["work"], 25);
    assert_eq!(partial.scan, WorkPatrolScan::Entries);
    assert!(partial.after.is_none() && partial.completed_at_ms.is_none());
    assert!(!partial.scanned_counts.contains_key("entries"));
    assert!(!partial.scanned_counts.contains_key("channels"));
    assert!(error_saved(s.conn(), "crew-a").unwrap().is_none());
    let raw = meta_get(s.conn(), &key("crew-a")).unwrap();
    {
        let _armed = arm(TestFaultPoint::BeforeEntriesRead);
        tick_crew(&mut s, "crew-a", NOW + 30_000);
        assert!(TEST_FAULT.with(|point| point.get()).is_none());
        assert_eq!(observation().draft.run_id, partial.run_id);
    }
    assert_eq!(meta_get(s.conn(), &key("crew-a")).unwrap(), raw);
    assert!(error_saved(s.conn(), "crew-a").unwrap().is_some());
    let done = tick_crew(&mut s, "crew-a", NOW + 90_000);
    assert_eq!(done.run_id, partial.run_id);
    assert_eq!(done.scanned_counts["work"], 25);
    assert_eq!(done.scanned_counts["entries"], 0);
    assert_eq!(done.scanned_counts["channels"], 0);
    assert!(done.completed_at_ms.is_some());
}

#[test]
fn twenty_five_fact_allowance_is_shared_across_mixed_phase_pages() {
    let mut s = store();
    plans(&mut s, "crew-a", 10);
    for _ in 0..10 {
        entry(&mut s, "crew-a");
        topic(&mut s, "crew-a");
    }
    let first = tick_crew(&mut s, "crew-a", NOW);
    assert_eq!(first.scanned_counts["work"], 10);
    assert_eq!(first.scanned_counts["entries"], 10);
    assert_eq!(first.scanned_counts["channels"], 5);
    assert_eq!(first.scanned_counts.values().sum::<u64>(), 25);
    assert!(first.completed_at_ms.is_none() && first.after.is_some());
    let done = tick_crew(&mut s, "crew-a", NOW + 30_000);
    assert_eq!(done.run_id, first.run_id);
    assert_eq!(done.started_at_ms, first.started_at_ms);
    assert_eq!(done.captured_seq, first.captured_seq);
    assert_eq!(done.scanned_counts["channels"], 10);
    assert!(done.completed_at_ms.is_some());
}

#[test]
fn combined_phase_slice_still_verifies_only_two_old_archive_candidates() {
    let mut s = store();
    plans(&mut s, "crew-a", 10);
    for _ in 0..10 {
        entry(&mut s, "crew-a");
    }
    let mut topics = (0..5)
        .map(|_| old_finished_topic(&mut s))
        .collect::<Vec<_>>();
    topics.sort();
    let first = tick_crew(&mut s, "crew-a", NOW);
    assert_eq!(first.scanned_counts["work"], 10);
    assert_eq!(first.scanned_counts["entries"], 10);
    assert_eq!(first.scanned_counts["channels"], 2);
    assert_eq!(first.findings["idle_archive_candidates"], topics[..2]);
    assert_eq!(first.after.as_deref(), Some(topics[1].as_str()));
    assert!(first.completed_at_ms.is_none());
    let second = tick_crew(&mut s, "crew-a", NOW + 30_000);
    assert_eq!(second.run_id, first.run_id);
    assert_eq!(second.scanned_counts["channels"], 4);
    let done = tick_crew(&mut s, "crew-a", NOW + 60_000);
    assert_eq!(done.run_id, first.run_id);
    assert_eq!(done.scanned_counts["channels"], 5);
    assert_eq!(done.findings["idle_archive_candidates"], topics);
    assert!(done.completed_at_ms.is_some());
    assert!(
        topics
            .iter()
            .all(|id| channels::get(s.conn(), id).unwrap().archived_at.is_none())
    );
}

#[test]
fn six_arbitrary_crews_are_fair_while_one_large_crew_resumes() {
    let mut s = store();
    for title in [
        "ordinary sixth-crew fixture A",
        "ordinary sixth-crew fixture B",
    ] {
        ok(&mut s, |tx| {
            crews::create(
                tx,
                crews::NewCrew {
                    title: title.into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )
        });
    }
    let active: Vec<String> = s
        .conn()
        .prepare("SELECT id FROM crews WHERE archived_at IS NULL ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(active.len(), 6);
    plans(&mut s, "crew-a", 60);
    meta_set(s.conn(), CURSOR, "").unwrap();
    let mut large_id = None;
    for turn in 0..18 {
        ok_at(&mut s, NOW + turn * 30_000, tick);
        let selected = &active[(turn as usize) % active.len()];
        assert_eq!(meta_get(s.conn(), CURSOR).unwrap().as_ref(), Some(selected));
        let observed = run(s.conn(), selected).unwrap().unwrap();
        if selected == "crew-a" {
            if let Some(id) = &large_id {
                assert_eq!(&observed.run_id, id);
            } else {
                large_id = Some(observed.run_id.clone());
            }
            let visits = turn / 6 + 1;
            assert_eq!(
                observed.scanned_counts["work"],
                (visits * 25).min(60) as u64
            );
            assert_eq!(observed.completed_at_ms.is_some(), visits == 3);
        } else {
            assert!(observed.completed_at_ms.is_some());
        }
    }
    assert_eq!(
        run(s.conn(), "crew-a").unwrap().unwrap().scanned_counts["work"],
        60
    );
}

#[test]
fn empty_selector_wrap_is_bounded_and_mints_no_run() {
    let mut s = Store::open_in_memory().unwrap();
    meta_set(s.conn(), CURSOR, "arbitrary-cursor-after-all-crews").unwrap();
    ok(&mut s, tick);
    assert_eq!(meta_get(s.conn(), CURSOR).unwrap().as_deref(), Some(""));
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'patrol.run.v1:%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    ok(&mut s, tick);
    assert_eq!(meta_get(s.conn(), CURSOR).unwrap().as_deref(), Some(""));
}

#[test]
fn phase_two_fault_rolls_back_the_whole_slice_and_preserves_retry_identity() {
    let mut s = store();
    plans(&mut s, "crew-a", 26);
    entry(&mut s, "crew-a");
    let first = tick_crew(&mut s, "crew-a", NOW);
    let raw = meta_get(s.conn(), &key("crew-a")).unwrap();
    {
        let _armed = arm(TestFaultPoint::BeforeEntriesRead);
        let failed = tick_crew(&mut s, "crew-a", NOW + 30_000);
        let seen = observation();
        assert_eq!(seen.draft.run_id, first.run_id);
        assert_eq!(seen.draft.scanned_counts["work"], 26);
        assert_eq!(seen.saved_before, raw);
        assert_eq!(failed.scanned_counts["work"], 25);
        assert!(!failed.findings.contains_key("unclaimed"));
    }
    assert_eq!(meta_get(s.conn(), &key("crew-a")).unwrap(), raw);
    assert_eq!(
        error_saved(s.conn(), "crew-a").unwrap().unwrap().retry_at,
        NOW + 90_000
    );
    let backed_off = tick_crew(&mut s, "crew-a", NOW + 89_999);
    assert_eq!(
        serde_json::to_value(backed_off).unwrap(),
        serde_json::to_value(first.clone()).unwrap()
    );
    let done = tick_crew(&mut s, "crew-a", NOW + 90_000);
    assert_eq!(done.run_id, first.run_id);
    assert_eq!(done.captured_seq, first.captured_seq);
    assert_eq!(done.scanned_counts["work"], 26);
    assert_eq!(done.scanned_counts["entries"], 1);
    assert_eq!(done.counts["unowned"], 26);
    assert_eq!(done.counts["unclaimed"], 1);
    assert!(done.completed_at_ms.is_some());
    assert!(error_saved(s.conn(), "crew-a").unwrap().is_none());
}

#[test]
fn late_test_fault_rolls_back_real_derived_writes_without_replacing_committed_run() {
    let mut s = store();
    let channel = topic(&mut s, "crew-a");
    plan(&mut s, "crew-a", Some(channel));
    plans(&mut s, "crew-a", 25);
    entry(&mut s, "crew-a");
    let first = tick_crew(&mut s, "crew-a", NOW);
    let raw = meta_get(s.conn(), &key("crew-a")).unwrap();
    let before_completed = meta_get(s.conn(), &completed_key("crew-a")).unwrap();
    assert!(before_completed.is_none());
    {
        let _armed = arm(TestFaultPoint::BeforeRunSave);
        tick_crew(&mut s, "crew-a", NOW + 30_000);
        let seen = observation();
        assert_eq!(seen.saved_before, raw);
        assert_eq!(seen.draft.run_id, first.run_id);
        assert_eq!(seen.draft.scanned_counts["work"], 26);
        assert_eq!(seen.draft.scanned_counts["entries"], 1);
        assert_eq!(seen.draft.scanned_counts["channels"], 1);
        assert!(seen.draft.completed_at_ms.is_some());
        assert!(
            seen.responsibility_rows > 0,
            "actual earlier derived writes reached before the explicit test fault"
        );
        let completed: CompletedObservation =
            serde_json::from_str(seen.completed_before.as_deref().unwrap()).unwrap();
        assert_eq!(completed.run.run_id, first.run_id);
        assert_eq!(completed.checksum, completed_checksum(&completed).unwrap());
    }
    assert_eq!(meta_get(s.conn(), &key("crew-a")).unwrap(), raw);
    assert_eq!(
        meta_get(s.conn(), &completed_key("crew-a")).unwrap(),
        before_completed
    );
    assert_eq!(
        s.conn()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'channel.responsibility.v1:6:crew-a:%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(secretary_observation(s.conn(), "crew-a").is_err());
    assert_eq!(
        meta_get(s.conn(), CURSOR).unwrap().as_deref(),
        Some("crew-a")
    );
    assert!(error_saved(s.conn(), "crew-a").unwrap().is_some());
    let done = tick_crew(&mut s, "crew-a", NOW + 90_000);
    assert_eq!(done.run_id, first.run_id);
    assert_eq!(
        serde_json::to_value(secretary_observation(s.conn(), "crew-a").unwrap().unwrap()).unwrap(),
        serde_json::to_value(&done).unwrap()
    );
}

#[test]
fn file_reopen_keeps_partial_identity_and_epoch_restore_still_invalidates_it() {
    let mut s = store();
    plans(&mut s, "crew-a", 26);
    let first = tick_crew(&mut s, "crew-a", NOW);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crewd.db");
    s.conn()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(s);
    let mut reopened = Store::open(&path).unwrap();
    let done = tick_crew(&mut reopened, "crew-a", NOW + 30_000);
    assert_eq!(done.run_id, first.run_id);
    assert_eq!(done.started_at_ms, first.started_at_ms);
    assert_eq!(done.captured_seq, first.captured_seq);
    assert_eq!(done.scanned_counts["work"], 26);
    assert!(done.completed_at_ms.is_some());
    crewd_store::judgment_state::restored(reopened.conn()).unwrap();
    assert!(run(reopened.conn(), "crew-a").unwrap().is_none());
    let fresh = tick_crew(&mut reopened, "crew-a", NOW + 60_000);
    assert_ne!(fresh.run_id, done.run_id);
    assert!(fresh.completed_at_ms.is_none());
}

#[test]
fn manual_patrol_still_uses_one_page_and_matches_the_completed_factual_projection() {
    let mut s = store();
    plans(&mut s, "crew-a", 27);
    entry(&mut s, "crew-a");
    topic(&mut s, "crew-a");
    let first = tick_crew(&mut s, "crew-a", NOW);
    assert_eq!(first.scan, WorkPatrolScan::Work);
    let completed = tick_crew(&mut s, "crew-a", NOW);
    assert!(completed.completed_at_ms.is_some());
    let periodic_raw = meta_get(s.conn(), &key("crew-a")).unwrap();
    let started = ok(&mut s, |tx| crate::patrol_manual::start(tx, "crew-a"));
    ok(&mut s, crate::patrol_manual::tick);
    let first_manual = status(s.conn(), "crew-a").unwrap().manual_run.unwrap();
    assert_eq!(first_manual.run_id, started.run_id);
    assert_eq!(first_manual.scanned_counts["work"], 25);
    assert_eq!(first_manual.scan, WorkPatrolScan::Work);
    assert!(!first_manual.scanned_counts.contains_key("entries"));
    for _ in 0..4 {
        ok(&mut s, crate::patrol_manual::tick);
    }
    let manual = status(s.conn(), "crew-a").unwrap().manual_run.unwrap();
    assert!(manual.completed_at_ms.is_some());
    assert_ne!(manual.run_id, completed.run_id);
    assert_eq!(manual.scanned_counts, completed.scanned_counts);
    assert_eq!(manual.counts, completed.counts);
    assert_eq!(manual.findings, completed.findings);
    assert_eq!(
        completed_semantic_revision(&manual).unwrap(),
        completed_semantic_revision(&completed).unwrap()
    );
    assert_eq!(meta_get(s.conn(), &key("crew-a")).unwrap(), periodic_raw);
}
