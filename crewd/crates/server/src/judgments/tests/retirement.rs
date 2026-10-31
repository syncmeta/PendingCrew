use super::*;
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

struct Rig {
    f: Fixture,
    handle: orchestra::Handle,
    actor: orchestra::Actor,
    bridge: tokio::task::JoinHandle<()>,
    exec: StandIn,
}
impl Rig {
    async fn new() -> Self {
        let f = Fixture::new().await;
        let exec = StandIn::default();
        let rx = f.state.hub.subscribe_collab();
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
        assert!(f.state.orchestra.set(handle.clone()).is_ok());
        let bridge = orchestra::db::forward_collab(rx, handle.clone());
        handle
            .resume(&f.worker, orchestra::core::Origin::Human)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while exec.launches(&f.worker).is_empty() {
                handle.latches().await.unwrap();
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Real executor adapters persist native identity separately from Core::Spawned.
        // StandIn needs the corresponding durable fixture identity for resumability checks.
        let sid = f.worker.clone();
        f.state
            .db
            .call_result(move |s| {
                crewd_store::sessions::set_agent_session_id(s.conn(), &sid, &format!("agent-{sid}"))
            })
            .await
            .unwrap()
            .unwrap();
        if exec.launches(&f.worker).first().unwrap().initial.is_some() {
            exec.emit(
                &f.worker,
                SessionEvent::Delivery {
                    id: BRIEF_DELIVERY_ID,
                    subject: Subject::Brief,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
        }
        exec.emit(&f.worker, SessionEvent::Working(true));
        exec.emit(&f.worker, SessionEvent::Working(false));
        handle.latches().await.unwrap();
        Self {
            f,
            handle,
            actor,
            bridge,
            exec,
        }
    }
    async fn close(self) {
        self.f.runtime.shutdown().await;
        self.exec.lock().hang_on_stop.clear();
        assert!(
            self.handle
                .shutdown(std::time::Duration::from_secs(2))
                .await
        );
        self.actor.finished().await.unwrap();
        self.bridge.abort();
        let _ = self.bridge.await;
        self.f.close().await;
    }
}
fn answer() -> Reply {
    Reply::ok(
        json!({"model":"fake-native","answers":{"can_retire":{"type":"noul","noul":0.99}}})
            .to_string(),
    )
}
async fn witness(r: &Rig) -> crewd_collab::session_retirement::ActorWitness {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == &r.f.worker)
            {
                return w;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
async fn view(r: &Rig) -> crewd_proto::RetirementView {
    let crew = r.f.crew.clone();
    let sid = r.f.worker.clone();
    let result =
        r.f.state
            .db
            .call_result(move |s| {
                Ok::<_, crewd_collab::CollabError>((
                    crewd_collab::session_retirement::status(s.conn(), &crew, &sid)?,
                    sessions::get(s.conn(), &sid)?,
                    crewd_collab::wakes::list(s.conn(), Some(&sid), 100)?,
                ))
            })
            .await
            .unwrap()
            .unwrap();
    result.0.record.unwrap_or_else(|| {
        panic!(
            "no retirement record; session={:?}; wakes={:?}",
            result.1, result.2
        )
    })
}
async fn phase(r: &Rig, want: crewd_proto::RetirementPhase) -> crewd_proto::RetirementView {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            r.handle.latches().await.unwrap();
            let v = view(r).await;
            if v.phase == want {
                return v;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn native_retirement_requires_real_exited_then_resumes_same_session_without_human_stop() {
    let fake = FakeJev::start(vec![answer()]).await;
    let r = Rig::new().await;
    r.f.configure(&fake.base_url).await;
    r.exec.lock().hang_on_stop.insert(r.f.worker.clone());
    let sid = r.f.worker.clone();
    let before =
        r.f.state
            .db
            .call_result(move |s| crewd_collab::wakes::human_stop_floor(s.conn(), &sid))
            .await
            .unwrap()
            .unwrap();
    r.f.runtime
        .run_retirement(r.f.worker.clone(), witness(&r).await, true)
        .await
        .unwrap();
    let requested = phase(&r, crewd_proto::RetirementPhase::ExitRequested).await;
    assert!(!requested.adopted);
    assert!(r.handle.live(&r.f.worker).await.unwrap().running);
    assert_eq!(
        r.exec
            .lock()
            .stops
            .iter()
            .filter(|s| *s == &r.f.worker)
            .count(),
        1
    );
    r.exec.emit(
        &r.f.worker,
        SessionEvent::Exited(crewd_exec::session::ExitInfo {
            status: crewd_exec::session::ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit: false,
        }),
    );
    let parked = phase(&r, crewd_proto::RetirementPhase::Parked).await;
    assert!(parked.adopted);
    let sid = r.f.worker.clone();
    let after =
        r.f.state
            .db
            .call_result(move |s| crewd_collab::wakes::human_stop_floor(s.conn(), &sid))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(before, after);
    r.exec.lock().hang_on_stop.remove(&r.f.worker);
    r.handle
        .resume(&r.f.worker, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while r.exec.launches(&r.f.worker).len() < 2 {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}
#[tokio::test]
async fn stream_loss_after_stop_request_never_becomes_parked() {
    let fake = FakeJev::start(vec![answer()]).await;
    let r = Rig::new().await;
    r.f.configure(&fake.base_url).await;
    r.exec.lock().hang_on_stop.insert(r.f.worker.clone());
    r.f.runtime
        .run_retirement(r.f.worker.clone(), witness(&r).await, true)
        .await
        .unwrap();
    phase(&r, crewd_proto::RetirementPhase::ExitRequested).await;
    r.exec.end_stream(&r.f.worker);
    let v = phase(&r, crewd_proto::RetirementPhase::Unknown).await;
    assert!(!v.adopted);
    assert_eq!(v.reason, crewd_proto::RetirementReason::ExitUnknown);
    r.close().await;
}
async fn late_change(mode: &str) {
    let fake = FakeJev::start(vec![
        answer().delayed(std::time::Duration::from_millis(300)),
    ])
    .await;
    let r = Rig::new().await;
    r.f.configure(&fake.base_url).await;
    let w = witness(&r).await;
    let runtime = r.f.runtime.clone();
    let sid = r.f.worker.clone();
    let task = tokio::spawn(async move { runtime.run_retirement(sid, w, true).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while fake.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    match mode {
        "work" => {
            let crew = r.f.crew.clone();
            let sid = r.f.worker.clone();
            r.f.state
                .collab(move |tx| {
                    crewd_collab::work::create(
                        tx,
                        &Sender::Human,
                        crewd_proto::WorkCreateParams {
                            crew_id: crew,
                            title: "new obligation during retirement decision".into(),
                            owner: Some(sid),
                            ..Default::default()
                        },
                    )
                })
                .await
                .unwrap();
        }
        "busy" => r.exec.emit(&r.f.worker, SessionEvent::Working(true)),
        "stop" => {
            r.handle.stop(&r.f.worker).await.unwrap();
        }
        "input" => {
            let handle = r.handle.clone();
            let sid = r.f.worker.clone();
            let nudge =
                tokio::spawn(async move { handle.nudge(&sid, "new human input".into()).await });
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while !r
                    .exec
                    .sent(&r.f.worker)
                    .iter()
                    .any(|s| s == "new human input")
                {
                    r.handle.latches().await.unwrap();
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            r.exec.emit(
                &r.f.worker,
                SessionEvent::Delivery {
                    id: r.exec.last_send_id(&r.f.worker),
                    subject: Subject::Steer,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            tokio::time::timeout(std::time::Duration::from_secs(3), nudge)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        _ => unreachable!(),
    }
    r.handle.latches().await.unwrap();
    task.await.unwrap();
    let v = view(&r).await;
    assert!(!v.adopted, "{mode}: {v:?}");
    assert!(
        !matches!(
            v.phase,
            crewd_proto::RetirementPhase::Parked | crewd_proto::RetirementPhase::ExitRequested
        ),
        "{mode}: {v:?}"
    );
    if mode != "stop" {
        assert!(r.exec.lock().stops.is_empty(), "{mode}");
    }
    r.close().await;
}
#[tokio::test]
async fn new_work_invalidates_pending_retirement() {
    late_change("work").await;
}
#[tokio::test]
async fn busy_transition_invalidates_pending_retirement() {
    late_change("busy").await;
}
#[tokio::test]
async fn human_stop_invalidates_pending_retirement() {
    late_change("stop").await;
}
#[tokio::test]
async fn new_input_invalidates_pending_retirement() {
    late_change("input").await;
}
#[tokio::test]
async fn unconfigured_retirement_never_stops_a_live_session() {
    let r = Rig::new().await;
    r.f.runtime
        .run_retirement(r.f.worker.clone(), witness(&r).await, true)
        .await
        .unwrap();
    let v = view(&r).await;
    assert_eq!(v.reason, crewd_proto::RetirementReason::Unconfigured);
    assert!(!v.adopted);
    assert!(r.exec.lock().stops.is_empty());
    assert!(r.handle.live(&r.f.worker).await.unwrap().running);
    r.close().await;
}
#[tokio::test]
async fn dispatched_retirement_reopens_unknown_without_external_replay() {
    let fake = FakeJev::start(vec![]).await;
    let r = Rig::new().await;
    r.f.configure(&fake.base_url).await;
    let w = witness(&r).await;
    r.f.runtime.shutdown().await;
    let sid = r.f.worker.clone();
    r.f.state
        .collab(move |tx| {
            let snapshot = crewd_collab::session_retirement::prepare(tx, &sid, w)?.unwrap();
            assert!(crewd_collab::session_retirement::dispatch(tx, &snapshot)?);
            Ok(())
        })
        .await
        .unwrap();
    let Rig {
        mut f,
        handle,
        actor,
        bridge,
        ..
    } = r;
    assert!(handle.shutdown(std::time::Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    bridge.abort();
    let _ = bridge.await;
    f.reopen().await;
    let crew = f.crew.clone();
    let sid = f.worker.clone();
    let v = f
        .state
        .db
        .call_result(move |s| crewd_collab::session_retirement::status(s.conn(), &crew, &sid))
        .await
        .unwrap()
        .unwrap()
        .record
        .unwrap();
    assert_eq!(v.phase, crewd_proto::RetirementPhase::Unknown);
    assert_eq!(v.reason, crewd_proto::RetirementReason::RestartUnknown);
    assert!(!v.adopted);
    assert!(fake.requests().is_empty());
    f.close().await;
}
#[tokio::test]
async fn retirement_after_historical_human_stop_does_not_reactivate_pause() {
    let fake = FakeJev::start(vec![answer()]).await;
    let r = Rig::new().await;
    r.handle.stop(&r.f.worker).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while r.handle.live(&r.f.worker).await.unwrap().running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.handle
        .resume(&r.f.worker, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while r.exec.launches(&r.f.worker).len() < 2 {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if r.exec.launches(&r.f.worker)[1].initial.is_some() {
        r.exec.emit(
            &r.f.worker,
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
    }
    r.exec.emit(&r.f.worker, SessionEvent::Working(true));
    r.exec.emit(&r.f.worker, SessionEvent::Working(false));
    r.handle.latches().await.unwrap();
    r.f.configure(&fake.base_url).await;
    r.f.runtime
        .run_retirement(r.f.worker.clone(), witness(&r).await, true)
        .await
        .unwrap();
    phase(&r, crewd_proto::RetirementPhase::Parked).await;
    let sid = r.f.worker.clone();
    let paused =
        r.f.state
            .db
            .call_result(move |s| crewd_collab::wakes::automatic_work_paused(s.conn(), &sid))
            .await
            .unwrap()
            .unwrap();
    assert!(!paused, "parking resurrected a historical Human Stop");
    r.close().await;
}
#[tokio::test]
async fn parked_restart_stays_idle_then_ordinary_legal_wake_resumes_original_session() {
    let fake = FakeJev::start(vec![answer()]).await;
    let r = Rig::new().await;
    r.f.configure(&fake.base_url).await;
    r.f.runtime
        .run_retirement(r.f.worker.clone(), witness(&r).await, true)
        .await
        .unwrap();
    phase(&r, crewd_proto::RetirementPhase::Parked).await;
    let Rig {
        mut f,
        handle,
        actor,
        bridge,
        ..
    } = r;
    f.runtime.shutdown().await;
    assert!(handle.shutdown(std::time::Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    bridge.abort();
    let _ = bridge.await;
    f.reopen().await;
    let exec = StandIn::default();
    let rx = f.state.hub.subscribe_collab();
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
    assert!(f.state.orchestra.set(handle.clone()).is_ok());
    let bridge = orchestra::db::forward_collab(rx, handle.clone());
    handle.latches().await.unwrap();
    assert!(exec.launches(&f.worker).is_empty());
    let r = Rig {
        f,
        handle,
        actor,
        bridge,
        exec,
    };
    assert_eq!(view(&r).await.phase, crewd_proto::RetirementPhase::Parked);
    // Explicit admission for this daemon run, then the normal durable wake path.
    r.f.state.admission.allow(&r.f.worker);
    let sid = r.f.worker.clone();
    r.f.state
        .collab(move |tx| {
            crewd_collab::wakes::enqueue(
                tx,
                crewd_collab::wakes::WakeRequest {
                    session: sid,
                    kind: crewd_collab::wakes::WakeKind::System,
                    source: "synthetic-authorized-post-retirement".into(),
                    priority: crewd_collab::wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some("new authorized work after retirement".into()),
                    origin: None,
                },
            )
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while r.exec.launches(&r.f.worker).is_empty() {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(r.exec.launches(&r.f.worker).len(), 1);
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}
