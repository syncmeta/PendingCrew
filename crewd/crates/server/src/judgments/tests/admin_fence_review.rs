//! Independent Core acceptance of native authority invalidation; no synthetic exits.
use super::*;

async fn running() -> (Rig, Source, String, FakeJev) {
    let (f, source, permit, fake) = prepared().await;
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    (r, source, sid, fake)
}

async fn original_socket(r: &Rig, sid: &str) -> Socket {
    let token = r.exec.launches(sid)[0].token.clone();
    assert!(r.f.state.tokens.verify(sid, &token));
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(crate::conn::run(r.f.state.clone(), server));
    let (rd, wr) = client.into_split();
    let mut socket = Socket {
        rd: tokio::io::BufReader::new(rd),
        wr,
        id: 0,
        task,
    };
    socket
        .call("agent.attach", json!({"sessionId":sid,"token":token}))
        .await
        .unwrap();
    socket
}

async fn human_stop_witness(r: &Rig, sid: &str) -> serde_json::Value {
    let sid = sid.to_owned();
    r.f.state
        .db
        .call_result(move |s| -> crewd_collab::Result<_> {
            Ok(json!({
                "epoch": crewd_collab::judgments::stop_epoch(s.conn(), &sid)?,
                "floor": wakes::human_stop_floor(s.conn(), &sid)?,
            }))
        })
        .await
        .unwrap()
        .unwrap()
}

async fn invalidate(r: &Rig, source: &Source, fake: &FakeJev, kind: &str) {
    match kind {
        "admission" => r.f.state.admission.revoke(&source.owner),
        "stop" => {
            let owner = source.owner.clone();
            r.f.state
                .collab(move |tx| wakes::human_stop(tx, &owner))
                .await
                .unwrap();
        }
        "config" => {
            let settings =
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
                    expected_revision: settings.revision,
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
        "target" => {
            let w = current_work(&r.f, &source.work).await;
            r.f.state
                .collab(move |tx| {
                    work::update(
                        tx,
                        &Sender::Human,
                        crewd_proto::WorkUpdateParams {
                            id: w.id,
                            expected_revision: w.revision,
                            note: Some("independent authoritative fact changed".into()),
                            ..Default::default()
                        },
                    )
                })
                .await
                .unwrap();
        }
        _ => panic!("unknown invalidation"),
    }
}

async fn observed_stop(r: &Rig, sid: &str) -> bool {
    r.handle.check_admin_authority().await.unwrap();
    wait_for_stop(r, sid).await
}

async fn wait_for_stop(r: &Rig, sid: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if r.exec.lock().stops.iter().any(|s| s == sid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn source_revocation_while_starting_fences_late_launch_without_first_task() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    r.exec.lock().next_held_launch = Some((entered_tx, release_rx));
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    tokio::time::timeout(Duration::from_secs(3), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(
        r.exec.launches(&sid).is_empty(),
        "no process born at held launch gate"
    );
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.f.state.admission.revoke(&source.owner);
    r.handle.check_admin_authority().await.unwrap();
    release_tx.send(Ok(())).unwrap();
    let stopped = observed_stop(&r, &sid).await;
    let launches = r.exec.launches(&sid);
    let sent = r.exec.lock().sends.get(&sid).cloned().unwrap_or_default();
    let child_admitted = r.f.state.admission.contains(&sid);
    let case = view(&r.f, &source.id).await;
    let token_valid = launches
        .first()
        .is_some_and(|l| r.f.state.tokens.verify(&sid, &l.token));
    r.close().await;
    assert!(stopped, "late physical birth must still receive stop");
    assert_eq!(launches.len(), 1);
    assert!(
        launches[0].initial.is_none(),
        "executor cannot receive first task before confirmed authority"
    );
    assert!(
        sent.is_empty(),
        "invalidated starting generation must never receive first task"
    );
    assert!(!child_admitted && !token_valid);
    assert_eq!(case["executionRecord"]["sessionId"], sid);
    assert_eq!(case["executionRecord"]["phase"], "unknown");
    assert_ne!(case["retirementRecord"]["phase"], "parked");
}

#[tokio::test]
async fn revoked_authority_fences_token_and_admission_before_held_process_exit() {
    for kind in [
        "admission",
        "admin_admission",
        "stop",
        "config",
        "restore",
        "target",
    ] {
        let (r, source, sid, fake) = running().await;
        let token = r.exec.launches(&sid)[0].token.clone();
        let original_stop = human_stop_witness(&r, &sid).await;
        let mut socket = original_socket(&r, &sid).await;
        let allowed = socket
            .call(
                "agent.call",
                json!({"tool":"work_item_get","args":{"id":source.work}}),
            )
            .await
            .unwrap();
        assert_eq!(
            allowed["isError"], false,
            "positive current authority control"
        );
        r.exec.lock().hang_on_stop.insert(sid.clone());
        if kind == "admin_admission" {
            r.f.state.admission.revoke(&sid);
        } else {
            invalidate(&r, &source, &fake, kind).await;
        }
        let stopped = observed_stop(&r, &sid).await;
        let admission = r.f.state.admission.contains(&sid);
        let token_valid = r.f.state.tokens.verify(&sid, &token);
        let process_live = r.exec.lock().running.contains(&sid);
        let case = view(&r.f, &source.id).await;
        let final_stop = human_stop_witness(&r, &sid).await;
        let denied = socket
            .call(
                "agent.call",
                json!({"tool":"work_item_get","args":{"id":source.work}}),
            )
            .await;
        socket.close().await;
        r.close().await;
        assert!(stopped, "{kind}: native stop was never requested");
        assert!(
            !admission && !token_valid,
            "{kind}: authority remained after stop request"
        );
        assert!(process_live, "held stop must not pretend physical exit");
        assert_eq!(
            final_stop, original_stop,
            "native authority fence must not manufacture Human Stop"
        );
        assert!(
            denied.is_err() || denied.as_ref().is_ok_and(|v| v["isError"] == true),
            "revoked original socket must refuse: {denied:?}"
        );
        assert_eq!(
            case["executionRecord"]["sessionId"], sid,
            "original binding retained"
        );
        assert_eq!(case["executionRecord"]["phase"], "unknown");
        assert_ne!(case["outcomeRecord"]["phase"], "resolved");
        assert_ne!(case["retirementRecord"]["phase"], "parked");
    }
}

#[tokio::test]
async fn periodic_guard_detects_silent_source_admission_revocation_without_manual_refresh() {
    let (r, source, sid, _fake) = running().await;
    r.exec.lock().hang_on_stop.insert(sid.clone());
    r.f.state.admission.revoke(&source.owner);
    // No actor messages at all: observe the executor mutex while the dedicated
    // production timer runs, so query-triggered guards cannot hide a missing timer.
    let stopped = wait_for_stop(&r, &sid).await;
    let admitted = r.f.state.admission.contains(&sid);
    r.close().await;
    assert!(
        stopped,
        "dedicated native authority timer must not wait for the 30s heartbeat"
    );
    assert!(!admitted);
}

#[tokio::test]
async fn legitimate_case_effect_checkpoint_does_not_revoke_its_own_native_authority() {
    let (r, source, sid, _fake) = running().await;
    let mut socket = original_socket(&r, &sid).await;
    let w = current_work(&r.f, &source.work).await;
    let result = socket.call("agent.call", close_args(&w)).await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    r.handle.check_admin_authority().await.unwrap();
    let stopped = r.exec.lock().stops.contains(&sid);
    let admitted = r.f.state.admission.contains(&sid);
    let running = r.exec.lock().running.contains(&sid);
    socket.close().await;
    r.close().await;
    assert!(
        !stopped && admitted && running,
        "legitimate typed effects advance the checkpoint rather than self-revoke"
    );
}

#[tokio::test]
async fn driver_stop_error_keeps_unknown_execution_without_faking_exit_or_retirement() {
    let (r, source, sid, fake) = running().await;
    let token = r.exec.launches(&sid)[0].token.clone();
    r.exec.lock().fail_on_stop.insert(sid.clone());
    invalidate(&r, &source, &fake, "admission").await;
    let stopped = observed_stop(&r, &sid).await;
    let process_live = r.exec.lock().running.contains(&sid);
    let authority = r.f.state.admission.contains(&sid) || r.f.state.tokens.verify(&sid, &token);
    let case = view(&r.f, &source.id).await;
    r.exec.lock().fail_on_stop.clear();
    r.close().await;
    assert!(stopped, "driver refusal must follow an actual stop attempt");
    assert!(process_live && !authority);
    assert_eq!(case["executionRecord"]["sessionId"], sid);
    assert_eq!(case["executionRecord"]["phase"], "unknown");
    assert_ne!(case["outcomeRecord"]["phase"], "resolved");
    assert_ne!(case["retirementRecord"]["phase"], "parked");
}

#[tokio::test]
async fn ordinary_human_admin_is_not_fenced_when_a_case_source_loses_admission() {
    let (r, source, case_sid, fake) = running().await;
    let ordinary = r
        .handle
        .start(
            orchestra::core::StartSession {
                crew_id: r.f.crew.clone(),
                role: Role::Admin,
                executor: "claude".into(),
                brief: "independent Human administrator".into(),
                title: None,
                model: None,
                effort: None,
                isolation: false,
                codex_options: None,
            },
            orchestra::core::Origin::Human,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while r.exec.launches(&ordinary.id).is_empty() {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.exec.lock().hang_on_stop.insert(case_sid.clone());
    invalidate(&r, &source, &fake, "admission").await;
    let case_stopped = observed_stop(&r, &case_sid).await;
    let ordinary_live = r.exec.lock().running.contains(&ordinary.id);
    let ordinary_stopped = r.exec.lock().stops.contains(&ordinary.id);
    r.close().await;
    assert!(case_stopped);
    assert!(
        ordinary_live && !ordinary_stopped,
        "native fence is bound-case authority, not Admin role policy"
    );
}

#[tokio::test]
async fn known_case_missing_or_corrupt_binding_never_downgrades_to_ordinary() {
    for corrupt in [false, true] {
        let (r, source, sid, _fake) = running().await;
        let token = r.exec.launches(&sid)[0].token.clone();
        let mut socket = original_socket(&r, &sid).await;
        r.exec.lock().hang_on_stop.insert(sid.clone());
        let admin = sid.clone();
        let case = source.id.clone();
        let evidence = r.f.state.db.call_result(move |s| -> crewd_collab::Result<_> {
            let record_key = format!("admin.case.execution.v1:{case}");
            let index_key = format!("admin.case.execution.session.v1:{admin}");
            let original_record = crewd_store::meta_get(s.conn(), &record_key)?.unwrap();
            let original_marker: String = s.conn().query_row("SELECT extra FROM sessions WHERE id=?1", [&admin], |row| row.get(0))?;
            if corrupt {
                s.conn().execute("UPDATE sessions SET extra=json_set(extra,'$.adminCaseExecutionV1',json('{\"version\":999}')) WHERE id=?1", [&admin])?;
                assert!(execution::session_binding(s.conn(), &admin).is_err());
            } else {
                s.conn().execute("DELETE FROM meta WHERE key=?1", [&index_key])?;
                s.conn().execute("UPDATE sessions SET extra=json_remove(extra,'$.adminCaseExecutionV1') WHERE id=?1", [&admin])?;
                assert!(execution::session_binding(s.conn(), &admin).is_err(), "saved execution is durable counter-evidence to missing hints");
            }
            let damaged_marker: String = s.conn().query_row("SELECT extra FROM sessions WHERE id=?1", [&admin], |row| row.get(0))?;
            Ok((original_record, original_marker, damaged_marker))
        }).await.unwrap().unwrap();
        let stopped = observed_stop(&r, &sid).await;
        let authority = r.f.state.admission.contains(&sid) || r.f.state.tokens.verify(&sid, &token);
        let process_live = r.exec.lock().running.contains(&sid);
        let denied = socket
            .call(
                "agent.call",
                json!({"tool":"work_item_get","args":{"id":source.work}}),
            )
            .await;
        let raw_denied = socket.call("work.get", json!({"id":source.work})).await;
        let admin = sid.clone();
        let case = source.id.clone();
        let after = r
            .f
            .state
            .db
            .call_result(move |s| -> crewd_collab::Result<_> {
                let record =
                    crewd_store::meta_get(s.conn(), &format!("admin.case.execution.v1:{case}"))?
                        .unwrap();
                let marker: String = s.conn().query_row(
                    "SELECT extra FROM sessions WHERE id=?1",
                    [&admin],
                    |row| row.get(0),
                )?;
                let retirement =
                    crewd_store::meta_get(s.conn(), &format!("session.retirement.v1:{admin}"))?;
                Ok((record, marker, retirement))
            })
            .await
            .unwrap()
            .unwrap();
        socket.close().await;
        r.close().await;
        assert!(
            stopped && process_live && !authority,
            "corrupt={corrupt}: fail-closed native stop without fabricated exit"
        );
        assert!(
            denied.is_err() || denied.as_ref().is_ok_and(|v| v["isError"] == true),
            "corrupt={corrupt}: revoked original socket must refuse: {denied:?}"
        );
        assert_ne!(evidence.1, evidence.2, "actual marker fault injected");
        assert!(
            raw_denied.is_err(),
            "known case cannot downgrade to ordinary raw RPC: {raw_denied:?}"
        );
        assert_eq!(
            after.0, evidence.0,
            "invalid binding cannot overwrite the original execution evidence"
        );
        assert_eq!(
            after.1, evidence.2,
            "do not silently repair or erase corrupt marker evidence"
        );
        assert!(
            after.2.is_none(),
            "authority failure cannot manufacture a retirement record"
        );
    }
}

async fn codex_waiting_profile() -> (Rig, Source, String, FakeJev) {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let worker = f.worker.clone();
    f.state
        .db
        .call_result(move |s| {
            s.conn()
                .execute("UPDATE sessions SET executor='codex' WHERE id=?1", [worker])
        })
        .await
        .unwrap()
        .unwrap();
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
    let exec = StandIn::default();
    let rx = f.state.hub.subscribe_collab();
    let mut cfg = orchestra::OrchestraConfig::for_tests();
    cfg.admission = Some(f.state.admission.clone());
    let (handle, actor) = orchestra::spawn(
        Arc::new(DbPort::new(
            f.state.clone(),
            f.dir.path().join(crewd_store::DB_FILE),
        )),
        Executors::new().with("codex", Arc::new(exec.clone())),
        Hooks {
            worktrees: Arc::new(StandInWorktrees::default()),
            ..Default::default()
        },
        Arc::new(SystemClock),
        cfg,
    )
    .await
    .unwrap();
    assert!(f.state.orchestra.set(handle.clone()).is_ok());
    let bridge = orchestra::db::forward_collab(rx, handle.clone());
    let r = Rig {
        f,
        handle,
        _actor: actor,
        bridge,
        exec,
    };
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    (r, source, sid, fake)
}

#[tokio::test]
async fn initial_codex_profile_missing_or_blank_model_never_receives_first_task() {
    for model in [None, Some("   ".to_owned())] {
        let (r, source, sid, _fake) = codex_waiting_profile().await;
        assert!(r.exec.launches(&sid)[0].initial.is_none());
        assert!(
            r.exec.sent(&sid).is_empty(),
            "Spawned alone must not release the first task"
        );
        let token = r.exec.launches(&sid)[0].token.clone();
        r.exec.lock().hang_on_stop.insert(sid.clone());
        r.exec.emit(
            &sid,
            crewd_exec::session::SessionEvent::Profile(crewd_exec::session::Profile {
                model,
                ..Default::default()
            }),
        );
        let stopped = observed_stop(&r, &sid).await;
        let sent = r.exec.sent(&sid);
        let authority = r.f.state.admission.contains(&sid) || r.f.state.tokens.verify(&sid, &token);
        let case = view(&r.f, &source.id).await;
        r.close().await;
        assert!(stopped && !authority);
        assert!(
            sent.is_empty(),
            "unknown initial model cannot authorize any first task"
        );
        assert_eq!(case["executionRecord"]["sessionId"], sid);
        assert_eq!(case["executionRecord"]["phase"], "unknown");
        assert_ne!(case["retirementRecord"]["phase"], "parked");
    }
}

#[tokio::test]
async fn source_revocation_stops_native_process_before_blocked_database_is_released() {
    struct ReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
    let (r, source, sid, _fake) = running().await;
    let token = r.exec.launches(&sid)[0].token.clone();
    let before = current_work(&r.f, &source.work).await;
    r.exec.lock().hang_on_stop.insert(sid.clone());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = ReleaseOnDrop(Some(release_tx));
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_release = released.clone();
    let db = r.f.state.db.clone();
    let blocked = tokio::spawn(async move {
        db.call(move |_| {
            let _ = entered_tx.send(());
            let explicitly_released = release_rx.recv_timeout(Duration::from_secs(8)).is_ok();
            observed_release.store(true, std::sync::atomic::Ordering::SeqCst);
            explicitly_released
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    r.f.state.admission.revoke(&source.owner);
    // No Core command and no DB query: only observe the real executor while
    // the production native-authority cadence must act independently of SQLite.
    let stopped_before_release = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if r.exec.lock().stops.contains(&sid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let was_still_blocked = !released.load(std::sync::atomic::Ordering::SeqCst);
    let authority_before_release =
        r.f.state.admission.contains(&sid) || r.f.state.tokens.verify(&sid, &token);
    let process_still_live = r.exec.lock().running.contains(&sid);
    drop(release);
    assert!(
        blocked.await.unwrap().unwrap(),
        "DB job released explicitly rather than by safety timeout"
    );
    r.handle.check_admin_authority().await.unwrap();
    let case = view(&r.f, &source.id).await;
    let after = current_work(&r.f, &source.work).await;
    r.close().await;
    assert!(
        was_still_blocked && stopped_before_release,
        "native authority stop must not wait for the blocked DB actor"
    );
    assert!(
        !authority_before_release && process_still_live,
        "credentials revoke before unconfirmed physical exit"
    );
    assert_eq!(case["executionRecord"]["sessionId"], sid);
    assert_eq!(case["executionRecord"]["phase"], "unknown");
    assert_ne!(case["retirementRecord"]["phase"], "parked");
    assert_ne!(case["outcomeRecord"]["phase"], "resolved");
    assert_eq!(
        after.revision, before.revision,
        "fencing cannot manufacture work progress"
    );
}

#[tokio::test]
async fn resolved_business_waits_for_actual_initial_delivery_before_retirement_witness() {
    use crewd_exec::{
        delivery::Subject,
        session::{DeliveryOutcome, SessionEvent},
    };
    for delivered in [true, false] {
        let (r, source, sid, _fake) = running().await;
        let mut socket = original_socket(&r, &sid).await;
        let w = current_work(&r.f, &source.work).await;
        let closed = socket.call("agent.call", close_args(&w)).await.unwrap();
        assert_eq!(closed["isError"], false);
        socket.close().await;
        let crew = r.f.crew.clone();
        let case = source.id.clone();
        let outcome =
            r.f.state
                .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &case))
                .await
                .unwrap();
        assert_eq!(outcome.phase, crewd_proto::AdminCaseOutcomePhase::Resolved);
        let admin = sid.clone();
        r.f.state
            .db
            .call_result(move |s| {
                crewd_store::sessions::set_agent_session_id(
                    s.conn(),
                    &admin,
                    "retained-initial-delivery-probe",
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert!(
            r.exec.launches(&sid)[0].initial.is_none(),
            "actual first input uses deferred send"
        );
        let sends_before = r.exec.lock().sends.get(&sid).cloned().unwrap();
        assert_eq!(sends_before.len(), 1);
        let delivery = sends_before[0].0;
        assert_ne!(delivery, crewd_exec::session::BRIEF_DELIVERY_ID);
        r.exec.emit(
            &sid,
            SessionEvent::Delivery {
                id: delivery,
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Stalled(
                    "actual first input is not yet acknowledged".into(),
                ),
            },
        );
        r.exec.emit(&sid, SessionEvent::Working(true));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !r.handle.live(&sid).await.unwrap().working {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        r.exec.emit(&sid, SessionEvent::Working(false));
        tokio::time::timeout(Duration::from_secs(3), async {
            while r.handle.live(&sid).await.unwrap().working {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !r.handle
                .retirement_candidates()
                .await
                .unwrap()
                .iter()
                .any(|(s, _)| s == &sid),
            "Resolved plus idle is insufficient while first input remains unconfirmed"
        );
        r.exec.lock().hang_on_stop.insert(sid.clone());
        r.exec.emit(
            &sid,
            SessionEvent::Delivery {
                id: delivery,
                subject: Subject::Steer,
                outcome: if delivered {
                    DeliveryOutcome::Delivered
                } else {
                    DeliveryOutcome::Undelivered("actual first input failed".into())
                },
            },
        );
        if delivered {
            r.exec.emit(&sid, SessionEvent::Working(true));
            tokio::time::timeout(Duration::from_secs(3), async {
                while !r.handle.live(&sid).await.unwrap().working {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            r.exec.emit(&sid, SessionEvent::Working(false));
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if r.handle
                        .retirement_candidates()
                        .await
                        .unwrap()
                        .iter()
                        .any(|(s, _)| s == &sid)
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("actual Delivered plus idle releases the pending-input retirement blocker");
            assert!(!r.exec.lock().stops.contains(&sid));
        } else {
            assert!(observed_stop(&r, &sid).await);
            assert!(
                !r.handle
                    .retirement_candidates()
                    .await
                    .unwrap()
                    .iter()
                    .any(|(s, _)| s == &sid)
            );
            assert_eq!(
                view(&r.f, &source.id).await["executionRecord"]["phase"],
                "unknown"
            );
        }
        assert_eq!(
            r.exec.lock().sends.get(&sid).unwrap().len(),
            sends_before.len(),
            "no first-input replay"
        );
        assert_eq!(
            view(&r.f, &source.id).await["outcomeRecord"]["phase"],
            "resolved",
            "delivery failure cannot erase verified business evidence"
        );
        r.close().await;
    }
}
