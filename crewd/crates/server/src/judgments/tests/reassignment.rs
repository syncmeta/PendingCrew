use super::*;
use crewd_collab::{work, work_reassignment};
use crewd_proto::{ReassignmentPhase, ReassignmentReason, WorkCreateParams, WorkUpdateParams};

fn answer() -> Reply {
    Reply::ok(json!({"model":"fake-native","answers":{"replacement":{"type":"choice","choice":"s0","confidence":1.0,"probabilities":{"s0":1.0}}}}).to_string())
}
async fn lost(f: &Fixture) -> String {
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    // Seed the trusted runtime recovery fact; this does not exercise external restoration failure.
    // Commit without publishing WorkChanged: each test controls its dispatch point.
    let (id, _changes) = f
        .state
        .db
        .call_result(move |store| {
            crewd_collab::run(store, crewd_store::now_ms(), move |tx| {
                sessions::set_resume_failed(tx, &owner, true)?;
                Ok(work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: crew,
                        title: "失联交接验收".into(),
                        owner: Some(owner),
                        ..Default::default()
                    },
                )?
                .id)
            })
        })
        .await
        .unwrap()
        .unwrap();
    id
}
async fn result(f: &Fixture, id: &str) -> crewd_proto::ReassignmentView {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let id = id.to_owned();
            let crew = f.crew.clone();
            let v = f
                .state
                .db
                .call_result(move |s| work_reassignment::status(s.conn(), &crew, &id))
                .await
                .unwrap()
                .unwrap()
                .record;
            if let Some(v) = v
                && !matches!(
                    v.phase,
                    ReassignmentPhase::Prepared | ReassignmentPhase::Dispatched
                )
            {
                return v;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn native_choice_reassigns_original_work_once_and_late_update_stales_both_notices() {
    let fake = FakeJev::start(vec![answer()]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let id = lost(&f).await;
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    assert!(view.adopted, "{view:?}");
    assert_ne!(view.new_owner.as_deref(), Some(f.worker.as_str()));
    let wakes = vec![view.owner_wake_id.unwrap(), view.captain_wake_id.unwrap()];
    let wid = id.clone();
    let new_owner = view.new_owner.clone();
    f.state
        .collab(move |tx| {
            let item = work::get(tx.conn(), &wid)?;
            assert_eq!(item.owner, new_owner);
            assert_ne!(item.status, "done");
            let rows = crewd_collab::wakes::by_ids(tx.conn(), &wakes)?;
            assert_eq!(rows.len(), 2);
            for row in &rows {
                let batch = crewd_collab::render::build_batch(
                    tx.conn(),
                    &row.session,
                    crewd_collab::Locale::En,
                    std::slice::from_ref(row),
                    tx.now_ms(),
                )?;
                assert_eq!(batch.items.len(), 1, "{}", row.source);
                assert!(batch.items[0].note.as_ref().unwrap().contains(&wid));
            }
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: wid,
                    expected_revision: item.revision,
                    note: Some("交接后人工更新原工作".into()),
                    ..Default::default()
                },
            )?;
            for row in &rows {
                let batch = crewd_collab::render::build_batch(
                    tx.conn(),
                    &row.session,
                    crewd_collab::Locale::En,
                    std::slice::from_ref(row),
                    tx.now_ms(),
                )?;
                assert!(batch.items.is_empty());
                assert_eq!(batch.unusable[0].0, row.id);
            }
            Ok(())
        })
        .await
        .unwrap();
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    assert_eq!(fake.requests().len(), 1);
    f.close().await;
}
#[tokio::test]
async fn unconfigured_loss_does_not_change_owner() {
    let f = Fixture::new().await;
    let id = lost(&f).await;
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    let v = result(&f, &id).await;
    assert!(matches!(v.reason, ReassignmentReason::Unconfigured));
    assert!(!v.adopted);
    let item = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.owner.as_deref(), Some(f.worker.as_str()));
    f.close().await;
}
#[tokio::test]
async fn dispatched_reopen_is_unknown_without_replaying_provider() {
    let fake = FakeJev::start(vec![]).await;
    let mut f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.runtime.shutdown().await;
    let id = lost(&f).await;
    let wid = id.clone();
    f.state
        .collab(move |tx| {
            let s = work_reassignment::prepare(tx, &wid)?.unwrap();
            assert!(work_reassignment::dispatch(tx, &s)?);
            Ok(())
        })
        .await
        .unwrap();
    f.reopen().await;
    let v = result(&f, &id).await;
    assert!(matches!(v.phase, ReassignmentPhase::Unknown));
    assert!(matches!(v.reason, ReassignmentReason::RestartUnknown));
    assert!(!v.adopted);
    assert!(fake.requests().is_empty());
    f.close().await;
}

async fn wait_request(fake: &FakeJev) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while fake.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn owner_recovers_while_provider_pending_cannot_be_reassigned() {
    let fake = FakeJev::start(vec![
        answer().delayed(std::time::Duration::from_millis(300)),
    ])
    .await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let id = lost(&f).await;
    let wid = id.clone();
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move { runtime.run_reassignment(wid, true).await.unwrap() });
    wait_request(&fake).await;
    let owner = f.worker.clone();
    f.state
        .collab(move |tx| sessions::set_resume_failed(tx, &owner, false))
        .await
        .unwrap();
    task.await.unwrap();
    let v = result(&f, &id).await;
    assert!(!v.adopted, "{v:?}");
    let item = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.owner.as_deref(), Some(f.worker.as_str()));
    assert!(v.owner_wake_id.is_none() && v.captain_wake_id.is_none());
    f.close().await;
}
#[tokio::test]
async fn selected_candidate_stops_while_provider_pending_cannot_receive_assignment() {
    let fake = FakeJev::start(vec![
        answer().delayed(std::time::Duration::from_millis(300)),
    ])
    .await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let id = lost(&f).await;
    let wid = id.clone();
    let target = f
        .state
        .collab(move |tx| {
            Ok(work_reassignment::prepare(tx, &wid)?.unwrap().candidates[0]
                .id
                .clone())
        })
        .await
        .unwrap();
    let wid = id.clone();
    let runtime = f.runtime.clone();
    let task = tokio::spawn(async move { runtime.run_reassignment(wid, true).await.unwrap() });
    wait_request(&fake).await;
    f.state
        .collab(move |tx| {
            crewd_collab::wakes::human_stop(tx, &target)?;
            Ok(())
        })
        .await
        .unwrap();
    task.await.unwrap();
    let v = result(&f, &id).await;
    assert!(!v.adopted, "{v:?}");
    let item = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.owner.as_deref(), Some(f.worker.as_str()));
    assert!(v.owner_wake_id.is_none() && v.captain_wake_id.is_none());
    f.close().await;
}

#[tokio::test]
async fn reassignment_notifications_reach_real_actor_and_only_typed_acks_deliver() {
    use crate::orchestra::{
        self,
        clock::SystemClock,
        db::DbPort,
        exec::Executors,
        hooks::Hooks,
        testkit::{StandIn, StandInWorktrees},
    };
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    let fake = FakeJev::start(vec![answer()]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let rx = f.state.hub.subscribe_collab();
    let exec = StandIn::default();
    let (handle, actor) = orchestra::spawn(
        Arc::new(DbPort::new(
            f.state.clone(),
            f.dir.path().join("unused-in-memory-db"),
        )),
        Executors::new().with("claude", Arc::new(exec.clone())),
        Hooks {
            worktrees: Arc::new(StandInWorktrees::default()),
            ..Default::default()
        },
        Arc::new(SystemClock),
        orchestra::OrchestraConfig::for_tests(),
    )
    .await
    .unwrap();
    let bridge = orchestra::db::forward_collab(rx, handle.clone());
    let id = lost(&f).await;
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    assert!(view.adopted, "{view:?}");
    let ids = vec![view.owner_wake_id.unwrap(), view.captain_wake_id.unwrap()];
    let check_ids = ids.clone();
    let rows = f
        .state
        .collab(move |tx| {
            let rows = crewd_collab::wakes::by_ids(tx.conn(), &check_ids)?;
            rows.into_iter()
                .map(|row| {
                    let batch = crewd_collab::render::build_batch(
                        tx.conn(),
                        &row.session,
                        crewd_collab::Locale::En,
                        std::slice::from_ref(&row),
                        tx.now_ms(),
                    )?;
                    Ok((row, batch.items[0].note.clone().unwrap()))
                })
                .collect::<crewd_collab::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (index, (row, note)) in rows.iter().enumerate() {
        let mut brief_ack = false;
        let delivery_result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                handle.latches().await.unwrap();
                if let Some(initial) = exec
                    .launches(&row.session)
                    .first()
                    .and_then(|l| l.initial.clone())
                {
                    if initial.contains(note) {
                        break (BRIEF_DELIVERY_ID, Subject::Brief);
                    }
                    if !brief_ack {
                        exec.emit(
                            &row.session,
                            SessionEvent::Delivery {
                                id: BRIEF_DELIVERY_ID,
                                subject: Subject::Brief,
                                outcome: DeliveryOutcome::Delivered,
                            },
                        );
                        brief_ack = true;
                    }
                }
                if exec.sent(&row.session).iter().any(|s| s.contains(note)) {
                    break (exec.last_send_id(&row.session), Subject::Steer);
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let (delivery, subject) = match delivery_result {
            Ok(value) => value,
            Err(e) => {
                let wid = id.clone();
                let ids = ids.clone();
                let debug = f
                    .state
                    .db
                    .call_result(move |s| {
                        Ok::<_, crewd_collab::CollabError>((
                            work::get(s.conn(), &wid)?,
                            crewd_collab::wakes::by_ids(s.conn(), &ids)?,
                        ))
                    })
                    .await
                    .unwrap()
                    .unwrap();
                let why = handle.wakes(Some(row.session.clone())).await.unwrap();
                let live = handle.live(&row.session).await.unwrap();
                panic!(
                    "why={why:?} live={live:?}; notification {index} {} note={note:?}; inputs={:?}, sent={:?}; work+wakes={debug:?}; {e}",
                    row.source,
                    exec.launches(&row.session)
                        .iter()
                        .map(|l| &l.initial)
                        .collect::<Vec<_>>(),
                    exec.sent(&row.session)
                );
            }
        };
        let wake = row.id;
        let before = f
            .state
            .db
            .call_result(move |s| crewd_collab::wakes::by_ids(s.conn(), &[wake]))
            .await
            .unwrap()
            .unwrap()
            .remove(0);
        if index == 0 {
            assert_eq!(before.state, crewd_collab::wakes::WakeState::Inflight);
        }
        // Both notices may share one captain batch; the first ACK can settle both.
        assert!(matches!(
            before.state,
            crewd_collab::wakes::WakeState::Inflight | crewd_collab::wakes::WakeState::Delivered
        ));
        exec.emit(
            &row.session,
            SessionEvent::Delivery {
                id: delivery,
                subject,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        // A real executor enters work and then becomes idle; false alone is no edge.
        exec.emit(&row.session, SessionEvent::Working(true));
        exec.emit(&row.session, SessionEvent::Working(false));
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            handle.latches().await.unwrap();
            let ids = ids.clone();
            let done = f
                .state
                .db
                .call_result(move |s| {
                    Ok::<bool, crewd_collab::CollabError>(
                        crewd_collab::wakes::by_ids(s.conn(), &ids)?
                            .iter()
                            .all(|r| r.state == crewd_collab::wakes::WakeState::Delivered),
                    )
                })
                .await
                .unwrap()
                .unwrap();
            if done {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.runtime.resume_pending_reassignment().await.unwrap();
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    let item = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.owner, view.new_owner);
    assert_ne!(item.status, "done");
    assert_eq!(fake.requests().len(), 1);
    for (row, note) in rows {
        let count = exec
            .sent(&row.session)
            .iter()
            .filter(|s| s.contains(&note))
            .count()
            + exec
                .launches(&row.session)
                .iter()
                .filter(|l| l.initial.as_ref().is_some_and(|s| s.contains(&note)))
                .count();
        assert_eq!(count, 1, "{}", row.source);
    }
    f.runtime.shutdown().await;
    assert!(handle.shutdown(std::time::Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    bridge.abort();
    let _ = bridge.await;
    f.close().await;
}

#[tokio::test]
async fn failed_captain_is_reassigned_but_notice_is_held_until_recovery_at_port_and_claim() {
    use crate::orchestra::{db::DbPort, port::Port};
    let fake = FakeJev::start(vec![answer()]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let crew = f.crew.clone();
    let ((id, captain), _) = f
        .state
        .db
        .call_result(move |store| {
            crewd_collab::run(store, crewd_store::now_ms(), move |tx| {
                let captain = crews::get(tx.conn(), &crew)?.captain_session_id.unwrap();
                sessions::set_resume_failed(tx, &captain, true)?;
                let item = work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: crew,
                        title: "机长失联原工作".into(),
                        owner: Some(captain.clone()),
                        ..Default::default()
                    },
                )?;
                Ok((item.id, captain))
            })
        })
        .await
        .unwrap()
        .unwrap();
    f.runtime.run_reassignment(id.clone(), true).await.unwrap();
    let v = result(&f, &id).await;
    assert!(v.adopted, "{v:?}");
    assert_eq!(v.new_owner.as_deref(), Some(f.worker.as_str()));
    let wake = v.captain_wake_id.unwrap();
    let port = DbPort::new(f.state.clone(), f.dir.path().join("unused-in-memory-db"));
    assert!(
        !port
            .pending(&captain)
            .await
            .unwrap()
            .iter()
            .any(|w| w.id == wake)
    );
    assert!(
        f.state
            .collab(move |tx| crewd_collab::wakes::claim(tx, &[wake]))
            .await
            .is_err()
    );
    let cap = captain.clone();
    f.state
        .collab(move |tx| {
            let row = crewd_collab::wakes::by_ids(tx.conn(), &[wake])?.remove(0);
            assert_eq!(row.state, crewd_collab::wakes::WakeState::Pending);
            assert_eq!(row.attempts, 0);
            assert!(sessions::get(tx.conn(), &cap)?.resume_failed_at.is_some());
            sessions::set_resume_failed(tx, &cap, false)
        })
        .await
        .unwrap();
    assert!(
        port.pending(&captain)
            .await
            .unwrap()
            .iter()
            .any(|w| w.id == wake)
    );
    f.state
        .collab(move |tx| {
            let row = crewd_collab::wakes::by_ids(tx.conn(), &[wake])?.remove(0);
            let batch = crewd_collab::render::build_batch(
                tx.conn(),
                &row.session,
                crewd_collab::Locale::En,
                std::slice::from_ref(&row),
                tx.now_ms(),
            )?;
            assert_eq!(
                batch.items.len(),
                1,
                "recovered captain must retain the original notification"
            );
            crewd_collab::wakes::claim(tx, &[wake])?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(fake.requests().len(), 1);
    f.close().await;
}
