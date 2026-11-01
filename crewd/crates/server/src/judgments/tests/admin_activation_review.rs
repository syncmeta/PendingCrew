//! Independent automatic administration acceptance. No manual case start command.
use super::*;

async fn wait_launch_reason(r: &Rig, source: &Source, reason: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(16), async {
        loop {
            let v = view(&r.f, &source.id).await;
            if v["launchRecord"]["reason"] == reason {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected launch reason {reason}"))
}

async fn wait_case(r: &Rig, source: &Source, field: &str, phase: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(16), async {
        loop {
            let v = view(&r.f, &source.id).await;
            if v[field]["phase"] == phase {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {field}.{phase}"))
}

#[tokio::test]
async fn automatic_case_real_close_periodic_verification_and_actual_exit_are_separate_receipts() {
    use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
    let retire = Reply::ok(
        json!({"model":"fake-native","answers":{
        "can_retire":{"type":"noul","noul":0.99}}})
        .to_string(),
    );
    let (r, source, fake) = automatic_rig(vec![answer(0.99), retire]).await;
    let running = wait_case(&r, &source, "executionRecord", "running").await;
    let sid = running["executionRecord"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(r.exec.launches(&sid).len(), 1);
    assert!(r.exec.launches(&sid)[0].initial.is_none());
    acknowledge_initial_input(&r, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let work = current_work(&r.f, &source.work).await;
    let result = socket.call("agent.call", close_args(&work)).await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    wait_case(&r, &source, "outcomeRecord", "resolved").await;
    assert!(view(&r.f, &source.id).await["retirementRecord"].is_null());
    let id = sid.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(s.conn(), &id, "retained-automatic-admin")
        })
        .await
        .unwrap()
        .unwrap();
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.exec.emit(&sid, SessionEvent::Working(true));
    tokio::time::timeout(Duration::from_secs(4), async {
        while !r.handle.live(&sid).await.unwrap().working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.exec.emit(&sid, SessionEvent::Working(false));
    tokio::time::timeout(Duration::from_secs(4), async {
        while r.handle.live(&sid).await.unwrap().working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let exiting = wait_case(&r, &source, "retirementRecord", "exit_requested").await;
    assert_eq!(exiting["retirementRecord"]["adopted"], false);
    assert!(r.handle.live(&sid).await.unwrap().running);
    assert!(!r.f.state.admission.contains(&sid));
    r.exec.emit(
        &sid,
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit: false,
        }),
    );
    let parked = wait_case(&r, &source, "retirementRecord", "parked").await;
    assert_eq!(parked["retirementRecord"]["adopted"], true);
    assert_eq!(parked["outcomeRecord"]["phase"], "resolved");
    assert_eq!(fake.requests().len(), 2);
    assert_eq!(r.exec.launches(&sid).len(), 1);
    socket.close().await;
    r.close().await;
}

async fn admin_count(f: &Fixture) -> usize {
    let crew = f.crew.clone();
    f.state
        .db
        .call_result(move |s| {
            sessions::list(s.conn(), Some(&crew))
                .map(|rows| rows.iter().filter(|s| s.role == Role::Admin).count())
        })
        .await
        .unwrap()
        .unwrap()
}

async fn metadata(f: &Fixture) -> Vec<(String, String)> {
    f.state
        .db
        .call_result(|s| {
            let mut q = s
                .conn()
                .prepare("SELECT key,value FROM meta ORDER BY key")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn missing_live_source_waits_and_get_does_not_create_or_dispatch_anything() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new_with_admin_dispatch(true).await;
    f.configure(&fake.base_url).await;
    let r = Rig::new(f).await;
    let source = sources(&r.f, 1).await.remove(0);
    let waiting = wait_launch_reason(&r, &source, "source_not_running").await;
    assert_eq!(waiting["launchRecord"]["reason"], "source_not_running");
    assert_eq!(admin_count(&r.f).await, 0);
    r.f.runtime.shutdown().await;
    let before = metadata(&r.f).await;
    let read =
        r.f.rpc("adminCase.get", json!({"crewId":r.f.crew,"id":source.id}))
            .await
            .unwrap();
    assert_eq!(read["launchRecord"]["reason"], "source_not_running");
    assert_eq!(
        metadata(&r.f).await,
        before,
        "GET must never prepare, claim or launch"
    );
    assert_eq!(admin_count(&r.f).await, 0);
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}

#[tokio::test]
async fn low_probability_is_observed_without_allocating_administrative_session() {
    let (r, source, fake) = automatic_rig(vec![answer(0.01)]).await;
    let observed = wait_case(&r, &source, "decisionRecord", "answered").await;
    assert_eq!(observed["decisionRecord"]["adopted"], false);
    assert_eq!(admin_count(&r.f).await, 0);
    assert!(observed["executionRecord"].is_null());
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}

#[tokio::test]
async fn unconfigured_source_is_visible_without_provider_or_admin_allocation() {
    let f = Fixture::new_with_admin_dispatch(true).await;
    let source = sources(&f, 1).await.remove(0);
    let r = Rig::new(f).await;
    r.handle
        .resume(&source.owner, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(14), async {
        loop {
            if view(&r.f, &source.id).await["decisionRecord"]["reason"] == "unconfigured" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(admin_count(&r.f).await, 0);
    assert!(view(&r.f, &source.id).await["executionRecord"].is_null());
    r.close().await;
}

async fn automatic_rig(replies: Vec<Reply>) -> (Rig, Source, FakeJev) {
    automatic_options(replies, false, None).await
}
async fn automatic_options(
    replies: Vec<Reply>,
    unsupported: bool,
    cap: Option<u32>,
) -> (Rig, Source, FakeJev) {
    let fake = FakeJev::start(replies).await;
    let f = Fixture::new_with_admin_dispatch(true).await;
    f.configure(&fake.base_url).await;
    if let Some(cap) = cap {
        f.state
            .collab(move |tx| crewd_collab::settings::set_max_live_sessions(tx, Some(cap)))
            .await
            .unwrap();
    }
    let r = Rig::new(f).await;
    r.exec.lock().reject_admin_case_policy = unsupported;
    let source = sources(&r.f, 1).await.remove(0);
    r.handle
        .resume(&source.owner, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.exec.launches(&source.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.exec.emit(
        &source.owner,
        crewd_exec::session::SessionEvent::Working(true),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while !r.handle.live(&source.owner).await.unwrap().working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    (r, source, fake)
}

#[tokio::test]
async fn source_revoked_during_automatic_launch_receives_no_first_task() {
    let (r, source, _fake) = automatic_options(vec![answer(0.99)], false, Some(1)).await;
    let waiting = wait_launch_reason(&r, &source, "capacity_unavailable").await;
    assert_eq!(waiting["launchRecord"]["reason"], "capacity_unavailable");
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    r.exec.lock().next_held_launch = Some((entered, release_rx));
    r.handle
        .set_settings(orchestra::port::SettingsUpdate {
            max_live: Some(None),
            ..Default::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), enter)
        .await
        .unwrap()
        .unwrap();
    let pending = view(&r.f, &source.id).await;
    let sid = pending["executionRecord"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    r.f.state.admission.revoke(&source.owner);
    r.handle.check_admin_authority().await.unwrap();
    let _ = release.send(Ok(()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !r.exec.lock().stops.contains(&sid) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(r.exec.sent(&sid).is_empty());
    assert!(r.exec.launches(&sid).iter().all(|l| l.initial.is_none()));
    assert!(!r.f.state.admission.contains(&sid));
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["phase"],
        "unknown"
    );
    assert_eq!(admin_count(&r.f).await, 1);
    r.close().await;
}

#[tokio::test]
async fn unsupported_executor_waits_without_creating_an_admin_or_changing_executor() {
    let (r, source, fake) = automatic_options(vec![answer(0.99)], true, None).await;
    let waiting = wait_launch_reason(&r, &source, "executor_unsupported").await;
    assert_eq!(waiting["launchRecord"]["reason"], "executor_unsupported");
    assert_eq!(admin_count(&r.f).await, 0);
    assert!(waiting["executionRecord"].is_null());
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}

#[tokio::test]
async fn capacity_waiting_recovers_without_new_judgment_or_duplicate_admin() {
    let (r, source, fake) = automatic_options(vec![answer(0.99)], false, Some(1)).await;
    let waiting = wait_launch_reason(&r, &source, "capacity_unavailable").await;
    assert_eq!(waiting["launchRecord"]["reason"], "capacity_unavailable");
    assert_eq!(admin_count(&r.f).await, 0);
    r.handle
        .set_settings(orchestra::port::SettingsUpdate {
            max_live: Some(None),
            ..Default::default()
        })
        .await
        .unwrap();
    let running = wait_case(&r, &source, "executionRecord", "running").await;
    let sid = running["executionRecord"]["sessionId"].as_str().unwrap();
    assert_eq!(r.exec.launches(sid).len(), 1);
    assert_eq!(admin_count(&r.f).await, 1);
    assert_eq!(fake.requests().len(), 1);
    r.close().await;
}

#[tokio::test]
async fn production_periodic_decision_automatically_starts_exactly_one_bound_admin() {
    let (r, source, fake) = automatic_rig(vec![answer(0.99)]).await;
    let observed = tokio::time::timeout(Duration::from_secs(14), async {
        loop {
            let v = view(&r.f, &source.id).await;
            if v["executionRecord"]["phase"] == "running" {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let final_view = view(&r.f, &source.id).await;
    if observed.is_ok() {
        let (a, b) = tokio::join!(
            r.f.runtime.schedule_admin_launches(),
            r.f.runtime.schedule_admin_launches()
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(admin_count(&r.f).await, 1);
        assert_eq!(
            view(&r.f, &source.id).await["executionRecord"]["sessionId"],
            final_view["executionRecord"]["sessionId"]
        );
    }
    let launched = observed.as_ref().ok().map(|v| {
        let sid = v["executionRecord"]["sessionId"].as_str().unwrap();
        r.exec.launches(sid).len()
    });
    let requests = fake.requests().len();
    r.close().await;
    assert!(
        observed.is_ok(),
        "production periodic judgment must start its adopted case without a manual start command: {final_view}"
    );
    assert_eq!(launched, Some(1));
    assert_eq!(requests, 1);
}

#[tokio::test]
async fn actual_database_reopen_retains_unknown_binding_without_automatic_replay() {
    let (r, source, _fake) = automatic_rig(vec![answer(0.99)]).await;
    let running = wait_case(&r, &source, "executionRecord", "running").await;
    let sid = running["executionRecord"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    r.f.runtime.shutdown().await;
    assert!(r.handle.shutdown(Duration::from_secs(2)).await);
    r.bridge.abort();
    let Rig {
        mut f,
        handle,
        _actor,
        bridge,
        exec,
    } = r;
    _actor.finished().await.unwrap();
    drop((handle, bridge, exec));
    f.reopen().await;
    let reopened = Rig::new(f).await;
    reopened
        .handle
        .resume(&source.owner, orchestra::core::Origin::Human)
        .await
        .unwrap();
    reopened.f.runtime.schedule_admin_launches().await.unwrap();
    reopened.f.runtime.schedule_admin_launches().await.unwrap();
    let restored = view(&reopened.f, &source.id).await;
    assert_eq!(restored["executionRecord"]["sessionId"], sid);
    assert_eq!(restored["executionRecord"]["phase"], "unknown");
    assert!(reopened.exec.launches(&sid).is_empty());
    assert_eq!(admin_count(&reopened.f).await, 1);
    reopened.close().await;
}

#[tokio::test]
async fn zero_dispatch_capacity_retains_sixty_five_sources_and_eventually_visits_tail() {
    let fake = FakeJev::start((0..65).map(|_| answer(0.99)).collect()).await;
    let f = Fixture::new_with_admin_dispatch(true).await;
    let slots = f
        .runtime
        .admin_launch_slots
        .clone()
        .acquire_many_owned(8)
        .await
        .unwrap();
    f.configure(&fake.base_url).await;
    let cases = sources(&f, 65).await;
    for s in &cases {
        f.state.admission.allow(&s.owner);
    }
    for s in &cases {
        f.runtime
            .run_admin_case(f.crew.clone(), s.id.clone(), true)
            .await
            .unwrap();
    }
    f.runtime.schedule_admin_launches().await.unwrap();
    f.runtime.schedule_admin_launches().await.unwrap();
    for s in &cases {
        assert!(view(&f, &s.id).await["launchRecord"].is_null());
    }
    drop(slots);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            f.runtime.schedule_admin_launches().await.unwrap();
            let mut visited = 0;
            for s in &cases {
                if view(&f, &s.id).await["launchRecord"]["reason"] == "runtime_unavailable" {
                    visited += 1;
                }
            }
            if visited == 65 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all 64+1 sources get a dispatch opportunity across eight slots");
    assert_eq!(fake.requests().len(), 65);
    assert_eq!(
        admin_count(&f).await,
        0,
        "this is scheduling fairness, not 65 process acceptance"
    );
    f.close().await;
}
