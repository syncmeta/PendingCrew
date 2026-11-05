//! Actual administrative retirement after independently verified business outcomes.
use super::*;
use crewd_exec::session::SessionEvent;
use crewd_proto::{RetirementPhase, RetirementView};

fn retirement_answer() -> Reply {
    Reply::ok(
        json!({"model":"fake-native","answers":{"can_retire":{"type":"noul","noul":0.99}}})
            .to_string(),
    )
}
async fn running(resolved: bool, retirement_reply: Reply) -> (Rig, Source, String, FakeJev) {
    let fake = FakeJev::start(vec![answer(0.99), retirement_reply]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let case = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &case))
        .await
        .unwrap()
        .unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    if resolved {
        let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
        let p = current_work(&r.f, &source.work).await;
        let result = socket.call("agent.call", close_args(&p)).await.unwrap();
        assert_eq!(result["isError"], false, "{result}");
        socket.close().await;
        let crew = r.f.crew.clone();
        let case = source.id.clone();
        let result =
            r.f.state
                .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &case))
                .await
                .unwrap();
        assert_eq!(result.phase, crewd_proto::AdminCaseOutcomePhase::Resolved);
    }
    let admin = sid.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(
                s.conn(),
                &admin,
                "retained-actual-admin-runner",
            )
        })
        .await
        .unwrap()
        .unwrap();
    acknowledge_initial_input(&r, &sid).await;
    r.exec.emit(&sid, SessionEvent::Working(true));
    r.exec.emit(&sid, SessionEvent::Working(false));
    r.handle.latches().await.unwrap();
    (r, source, sid, fake)
}
async fn witness(r: &Rig, sid: &str) -> crewd_collab::session_retirement::ActorWitness {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == sid)
            {
                break w;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("real idle Core witness")
}
async fn record(r: &Rig, sid: &str) -> Option<RetirementView> {
    let crew = r.f.crew.clone();
    let sid = sid.to_owned();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_collab::session_retirement::status(s.conn(), &crew, &sid).map(|v| v.record)
        })
        .await
        .unwrap()
        .unwrap()
}
async fn phase(r: &Rig, sid: &str, want: RetirementPhase) -> RetirementView {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(v) = record(r, sid).await
                && v.phase == want
            {
                break v;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected retirement {want:?} for {sid}"))
}
#[tokio::test]
async fn unresolved_case_does_not_enter_retirement_provider_or_stop_process() {
    let (r, _source, sid, fake) = running(false, retirement_answer()).await;
    r.f.runtime
        .run_retirement(sid.clone(), witness(&r, &sid).await, true)
        .await
        .unwrap();
    assert_eq!(
        fake.requests().len(),
        1,
        "only original intervention judgment"
    );
    assert!(record(&r, &sid).await.is_none());
    assert!(!r.exec.lock().stops.contains(&sid));
    r.close().await;
}
#[tokio::test]
async fn resolved_case_requires_actual_exited_before_parked_and_stays_parked_after_file_reopen() {
    let (r, source, sid, fake) = running(true, retirement_answer()).await;
    let mut original_socket = Socket::attach(r.f.state.clone(), &sid).await;
    let allowed = original_socket
        .call(
            "agent.call",
            json!({"tool":"work_item_get","args":{"id":source.work}}),
        )
        .await
        .unwrap();
    assert_eq!(allowed["isError"], false, "{allowed}");
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.f.runtime
        .run_retirement(sid.clone(), witness(&r, &sid).await, true)
        .await
        .unwrap();
    let pending = phase(&r, &sid, RetirementPhase::ExitRequested).await;
    assert!(!pending.adopted);
    for args in [
        json!({"tool":"work_item_get","args":{"id":source.work}}),
        close_args(&current_work(&r.f, &source.work).await),
    ] {
        let denied = original_socket.call("agent.call", args).await.unwrap_err();
        assert_eq!(denied.code, crewd_proto::RpcError::FORBIDDEN, "{denied:?}");
    }
    assert!(!r.f.state.admission.contains(&sid));
    original_socket.close().await;
    assert!(r.handle.live(&sid).await.unwrap().running);
    assert_eq!(r.exec.lock().stops.iter().filter(|s| *s == &sid).count(), 1);
    r.exec.emit(
        &sid,
        SessionEvent::Exited(crewd_exec::session::ExitInfo {
            status: crewd_exec::session::ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit: false,
        }),
    );
    assert!(phase(&r, &sid, RetirementPhase::Parked).await.adopted);
    let parked_session = sid.clone();
    assert!(
        !r.f.state
            .db
            .call_result(move |s| wakes::automatic_work_paused(s.conn(), &parked_session))
            .await
            .unwrap()
            .unwrap(),
        "administrative retirement is not Human Stop"
    );
    assert_eq!(fake.requests().len(), 2);
    r.exec.lock().hang_on_stop.remove(&sid);
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
    reopened.handle.latches().await.unwrap();
    assert!(reopened.exec.launches(&sid).is_empty());
    assert_eq!(
        record(&reopened, &sid).await.unwrap().phase,
        RetirementPhase::Parked
    );
    assert_eq!(
        view(&reopened.f, &source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    let count = reopened
        .f
        .state
        .db
        .call_result(|s| {
            s.conn().query_row(
                "SELECT count(*) FROM sessions WHERE role='admin'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 1);
    reopened.close().await;
}
#[tokio::test]
async fn resolved_admin_stream_end_after_exit_request_is_unknown_not_parked() {
    let (r, source, sid, _fake) = running(true, retirement_answer()).await;
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.f.runtime
        .run_retirement(sid.clone(), witness(&r, &sid).await, true)
        .await
        .unwrap();
    phase(&r, &sid, RetirementPhase::ExitRequested).await;
    r.exec.end_stream(&sid);
    let unknown = phase(&r, &sid, RetirementPhase::Unknown).await;
    assert!(!unknown.adopted);
    assert_eq!(
        view(&r.f, &source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    r.close().await;
}

#[tokio::test]
async fn resolved_admin_stop_failure_is_unknown_without_fabricated_exit() {
    let (r, source, sid, _fake) = running(true, retirement_answer()).await;
    r.exec.lock().fail_on_stop.insert(sid.clone());
    r.f.runtime
        .run_retirement(sid.clone(), witness(&r, &sid).await, true)
        .await
        .unwrap();
    let unknown = phase(&r, &sid, RetirementPhase::Unknown).await;
    assert!(!unknown.adopted);
    assert!(r.handle.live(&sid).await.unwrap().running);
    assert_eq!(
        view(&r.f, &source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    r.exec.lock().fail_on_stop.remove(&sid);
    r.close().await;
}
#[tokio::test]
async fn late_work_source_stop_configuration_or_restore_prevents_admin_retirement() {
    for mutation in ["work", "source_stop", "config", "restore"] {
        let (r, source, sid, fake) = running(
            true,
            retirement_answer().delayed(Duration::from_millis(350)),
        )
        .await;
        let w = witness(&r, &sid).await;
        let runtime = r.f.runtime.clone();
        let admin = sid.clone();
        let pending =
            tokio::spawn(async move { runtime.run_retirement(admin, w, true).await.unwrap() });
        tokio::time::timeout(Duration::from_secs(3), async {
            while fake.requests().len() < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        match mutation {
            "work" => {
                let crew = r.f.crew.clone();
                let owner = sid.clone();
                r.f.state
                    .collab(move |tx| {
                        work::create(
                            tx,
                            &Sender::Human,
                            WorkCreateParams {
                                crew_id: crew,
                                owner: Some(owner),
                                title: "new actual obligation".into(),
                                ..Default::default()
                            },
                        )
                    })
                    .await
                    .unwrap();
            }
            "source_stop" => {
                let owner = source.owner.clone();
                r.f.state
                    .collab(move |tx| wakes::human_stop(tx, &owner))
                    .await
                    .unwrap();
            }
            "config" => {
                let current =
                    r.f.state
                        .db
                        .call_result(|s| crewd_collab::judgments::configuration(s.conn()))
                        .await
                        .unwrap()
                        .unwrap();
                r.f.rpc(
                    SETTINGS_SET,
                    serde_json::to_value(JudgmentSettingsSet {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: current.revision,
                        config: Some(Fixture::config(&fake.base_url)),
                        credential_update: CredentialUpdate::Keep,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            }
            "restore" => {
                r.f.state
                    .collab(|tx| {
                        crewd_store::judgment_state::restored(tx.conn())?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        pending.await.unwrap();
        r.handle.check_admin_authority().await.unwrap();
        if mutation == "work" {
            assert!(
                !r.exec.lock().stops.contains(&sid),
                "new obligation must prevent retirement without revoking the original case authority"
            );
            assert!(r.handle.live(&sid).await.unwrap().running);
        } else {
            assert!(
                r.exec.lock().stops.contains(&sid),
                "{mutation} must fence native execution independently of retirement"
            );
            assert!(!r.f.state.admission.contains(&sid));
            assert_eq!(view(&r.f, &source.id).await["execution"], "unknown");
        }
        assert!(
            record(&r, &sid)
                .await
                .is_none_or(|v| !v.adopted && v.phase != RetirementPhase::Parked)
        );
        r.close().await;
    }
}
