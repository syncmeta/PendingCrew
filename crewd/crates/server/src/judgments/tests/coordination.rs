use super::*;
use crewd_collab::{work, work_coordination};
use crewd_proto::{CoordinationPhase, CoordinationReason, WorkCreateParams, WorkUpdateParams};

async fn reports(f: &Fixture) -> String {
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    f.state
        .collab(move |tx| {
            let mut item = work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew,
                    title: "迁移检查".into(),
                    owner: Some(owner.clone()),
                    ..Default::default()
                },
            )?;
            for note in ["仍在检查，尚无结果", "仍在检查，尚无新增结果"] {
                item = work::update(
                    tx,
                    &Sender::Session(owner.clone()),
                    WorkUpdateParams {
                        id: item.id,
                        expected_revision: item.revision,
                        note: Some(note.into()),
                        next_check_at: Some(crewd_store::ts_from_ms(tx.now_ms() + 60_000)),
                        ..Default::default()
                    },
                )?;
            }
            Ok(item.id)
        })
        .await
        .unwrap()
}
async fn result(f: &Fixture, id: &str) -> crewd_proto::CoordinationView {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let id = id.to_owned();
            let crew = f.crew.clone();
            let view = f
                .state
                .db
                .call_result(move |s| work_coordination::status(s.conn(), &crew, &id, None))
                .await
                .unwrap()
                .unwrap()
                .record;
            if let Some(v) = view
                && !matches!(
                    v.phase,
                    CoordinationPhase::Prepared | CoordinationPhase::Dispatched
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
async fn real_runtime_native_coordination_is_unique_and_work_change_invalidates_note() {
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"meaningful_progress":{"type":"noul","noul":0.1},"needs_coordination":{"type":"noul","noul":0.9}}}).to_string())]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let id = reports(&f).await;
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    assert!(view.adopted);
    assert!(view.wake_id.is_some());
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    assert_eq!(fake.requests().len(), 1);
    let wake = view.wake_id.unwrap();
    let work_id = id.clone();
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
            assert_eq!(batch.items.len(), 1);
            assert!(batch.items[0].note.as_ref().unwrap().contains(&work_id));
            let item = work::get(tx.conn(), &work_id)?;
            work::update(
                tx,
                &Sender::Session(row.session.clone()),
                WorkUpdateParams {
                    id: work_id,
                    expected_revision: item.revision,
                    note: Some("机长复核并给出下一步".into()),
                    ..Default::default()
                },
            )?;
            let batch = crewd_collab::render::build_batch(
                tx.conn(),
                &row.session,
                crewd_collab::Locale::En,
                std::slice::from_ref(&row),
                tx.now_ms(),
            )?;
            assert!(batch.items.is_empty());
            assert_eq!(batch.unusable[0].0, wake);
            Ok(())
        })
        .await
        .unwrap();
    f.close().await;
}
#[tokio::test]
async fn unconfigured_runtime_does_not_create_coordination_wake() {
    let f = Fixture::new().await;
    let id = reports(&f).await;
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    assert!(matches!(view.reason, CoordinationReason::Unconfigured));
    assert!(!view.adopted && view.wake_id.is_none());
    f.close().await;
}

#[tokio::test]
async fn coordination_split_socket_issues_children_and_recovers_original_receipt() {
    use super::admin_execution::Socket;
    use crewd_collab::wakes;
    let mut f = Fixture::new().await;
    let id = reports(&f).await;
    let work_id = id.clone();
    let (parent, view) = f
        .state
        .collab(move |tx| {
            let snapshot = work_coordination::prepare(tx, &work_id)?.unwrap();
            assert!(work_coordination::dispatch(tx, &snapshot)?);
            work_coordination::settle(
                tx,
                &snapshot,
                Some(0.1),
                Some(0.9),
                true,
                CoordinationReason::Answered,
            )?;
            let view = work_coordination::status(tx.conn(), &snapshot.crew, &work_id, None)?
                .record
                .unwrap();
            let wake = view.wake_id.unwrap();
            wakes::claim(tx, &[wake])?;
            wakes::ack_delivered(
                tx,
                &snapshot.captain,
                &[wake],
                wakes::Riders::Release,
                &[],
                &[],
            )?;
            Ok((work::get(tx.conn(), &work_id)?, view))
        })
        .await
        .unwrap();
    let captain = view.recipient.unwrap();
    let p = json!({
        "id": id,
        "expected_revision": parent.revision,
        "coordination_decision_id": view.decision_id,
        "children": [
            {"title":"separate first child", "goal":"implement first scope", "acceptance":"verify first scope", "owner":f.worker},
            {"title":"separate second child", "goal":"implement second scope", "acceptance":"verify second scope", "owner":null}
        ]
    });
    let call = |args| json!({"tool":work_coordination::SPLIT_TOOL,"args":args});
    let mut worker_socket = Socket::attach(f.state.clone(), &f.worker).await;
    let worker = worker_socket
        .call("agent.call", call(p.clone()))
        .await
        .unwrap();
    assert_eq!(worker["isError"], true, "{worker}");
    worker_socket.close().await;
    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    let mut missing = p.clone();
    missing["children"][1]
        .as_object_mut()
        .unwrap()
        .remove("owner");
    let missing = captain_socket
        .call("agent.call", call(missing))
        .await
        .unwrap();
    assert_eq!(missing["isError"], true, "{missing}");
    let first = captain_socket
        .call("agent.call", call(p.clone()))
        .await
        .unwrap();
    assert_eq!(first["isError"], false, "{first}");
    let (_, body) = first["text"].as_str().unwrap().split_once('\n').unwrap();
    let receipt: crewd_proto::CoordinationSplit = serde_json::from_str(body).unwrap();
    assert_eq!(receipt.children.len(), 2);
    assert_eq!(
        receipt.children[0].owner.as_deref(),
        Some(f.worker.as_str())
    );
    assert!(receipt.children[1].owner.is_none());
    let same_decision = captain_socket.call("agent.call", json!({"tool":"work_item_assign","args":{
        "id":id,"expected_revision":parent.revision,"owner":captain,
        "coordination_decision_id":view.decision_id,"note":"same decision is already consumed"
    }})).await.unwrap();
    assert_eq!(same_decision["isError"], true, "{same_decision}");
    captain_socket.close().await;
    f.reopen().await;
    let mut human = Socket::owned(f.state.clone());
    let query = json!({"crewId":f.crew,"id":id,"decisionId":view.decision_id});
    let result = human.call("work.coordinationStatus", query).await.unwrap();
    assert_eq!(result["lastSplit"], json!(receipt));
    assert_eq!(result["requestedSplit"], json!(receipt));
    assert!(result.get("requestedAssignment").is_none());
    assert_eq!(
        human.call("work.get", json!({"id":id})).await.unwrap(),
        json!(parent)
    );
    for child in &receipt.children {
        let work = human
            .call("work.get", json!({"id":child.id}))
            .await
            .unwrap();
        assert_eq!(work["parent"], id);
        assert_eq!(work["owner"], json!(child.owner));
        assert_eq!(work["status"], "open");
        assert_eq!(work["sources"], json!([]));
    }
    f.state.admission.allow(&captain);
    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    assert_eq!(
        captain_socket.call("agent.call", call(p)).await.unwrap(),
        first
    );
    let work = human
        .call("work.list", json!({"crewId":f.crew}))
        .await
        .unwrap();
    assert_eq!(work["items"].as_array().unwrap().len(), 3);
    captain_socket.close().await;
    human.close().await;
    f.close().await;
}
#[tokio::test]
async fn stopped_owner_never_calls_configured_provider() {
    let fake = FakeJev::start(vec![]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let owner = f.worker.clone();
    f.state
        .collab(move |tx| {
            crewd_collab::wakes::human_stop(tx, &owner)?;
            Ok(())
        })
        .await
        .unwrap();
    let id = reports(&f).await;
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    assert!(!view.adopted && view.wake_id.is_none());
    assert!(matches!(view.reason, CoordinationReason::Stopped));
    assert!(fake.requests().is_empty());
    f.close().await;
}
#[tokio::test]
async fn dispatched_coordination_reopens_unknown_without_provider_replay() {
    let fake = FakeJev::start(vec![]).await;
    let mut f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.runtime.shutdown().await;
    let id = reports(&f).await;
    let source = id.clone();
    f.state
        .collab(move |tx| {
            let snapshot = work_coordination::prepare(tx, &source)?.unwrap();
            assert!(work_coordination::dispatch(tx, &snapshot)?);
            Ok(())
        })
        .await
        .unwrap();
    f.reopen().await;
    let view = result(&f, &id).await;
    assert!(matches!(view.phase, CoordinationPhase::Unknown));
    assert!(matches!(view.reason, CoordinationReason::RestartUnknown));
    assert!(fake.requests().is_empty());
    f.close().await;
}

#[tokio::test]
async fn coordination_assignment_uses_actual_captain_socket_and_typed_ack_then_reopens() {
    use super::admin_execution::Socket;
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
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"meaningful_progress":{"type":"noul","noul":0.1},"needs_coordination":{"type":"noul","noul":0.9}}}).to_string())]).await;
    let mut f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let rx = f.state.hub.subscribe_collab();
    let exec = StandIn::default();
    let hooks = Hooks {
        worktrees: Arc::new(StandInWorktrees::default()),
        ..Default::default()
    };
    let (handle, actor) = orchestra::spawn(
        Arc::new(DbPort::new(
            f.state.clone(),
            f.dir.path().join("unused-in-memory-db"),
        )),
        Executors::new().with("claude", Arc::new(exec.clone())),
        hooks,
        Arc::new(SystemClock),
        orchestra::OrchestraConfig::for_tests(),
    )
    .await
    .unwrap();
    let bridge = orchestra::db::forward_collab(rx, handle.clone());
    let id = reports(&f).await;
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    let view = result(&f, &id).await;
    let captain = view.recipient.clone().unwrap();
    let decision = &view.decision_id;
    let wake = view.wake_id.unwrap();
    let marker = f
        .state
        .db
        .call_result(move |s| crewd_collab::wakes::by_ids(s.conn(), &[wake]))
        .await
        .unwrap()
        .unwrap()
        .remove(0)
        .source;
    let mut brief_ack = false;
    let (delivery, subject) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            handle.latches().await.unwrap();
            if let Some(initial) = exec
                .launches(&captain)
                .first()
                .and_then(|l| l.initial.clone())
            {
                if initial.contains("请核实并协调") {
                    assert!(initial.contains(decision));
                    break (BRIEF_DELIVERY_ID, Subject::Brief);
                }
                if !brief_ack {
                    exec.emit(
                        &captain,
                        SessionEvent::Delivery {
                            id: BRIEF_DELIVERY_ID,
                            subject: Subject::Brief,
                            outcome: DeliveryOutcome::Delivered,
                        },
                    );
                    brief_ack = true;
                }
            }
            if exec
                .sent(&captain)
                .iter()
                .any(|text| text.contains("请核实并协调") && text.contains(decision))
            {
                break (exec.last_send_id(&captain), Subject::Steer);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let source = marker.clone();
    let before = f
        .state
        .db
        .call_result(move |s| {
            crewd_collab::wakes::list(s.conn(), None, 100)
                .map(|rows| rows.into_iter().find(|r| r.source == source).unwrap())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.state, crewd_collab::wakes::WakeState::Inflight);
    let work_id = id.clone();
    let initial = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &work_id))
        .await
        .unwrap()
        .unwrap();
    let args = json!({
        "id":id, "expected_revision":initial.revision, "owner":captain,
        "note":"verified assignment for the coordination request",
        "coordination_decision_id":view.decision_id,
    });
    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    let tool = |args| json!({"tool":"work_item_assign","args":args});
    let pending = captain_socket
        .call("agent.call", tool(args.clone()))
        .await
        .unwrap();
    assert_eq!(pending["isError"], true, "{pending}");
    let mut malformed = args.clone();
    malformed["coordination_decision_id"] = serde_json::Value::Null;
    let invalid = captain_socket
        .call("agent.call", tool(malformed))
        .await
        .unwrap();
    assert_eq!(invalid["isError"], true, "{invalid}");
    exec.emit(
        &captain,
        SessionEvent::Delivery {
            id: delivery,
            subject,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            handle.latches().await.unwrap();
            let source = marker.clone();
            let delivered = f
                .state
                .db
                .call_result(move |s| {
                    crewd_collab::wakes::list(s.conn(), None, 100).map(|rows| {
                        rows.into_iter().any(|r| {
                            r.source == source
                                && r.state == crewd_collab::wakes::WakeState::Delivered
                        })
                    })
                })
                .await
                .unwrap()
                .unwrap();
            if delivered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.runtime.run_coordination(id.clone(), true).await.unwrap();
    let work_id = id.clone();
    let state = f
        .state
        .db
        .call_result(move |s| work::get(s.conn(), &work_id))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(state.status, "done");
    assert_eq!(state.owner.as_deref(), Some(f.worker.as_str()));
    handle.latches().await.unwrap();
    assert_eq!(fake.requests().len(), 1);
    let inputs = exec
        .sent(&captain)
        .iter()
        .filter(|text| text.contains("请核实并协调"))
        .count()
        + exec
            .launches(&captain)
            .iter()
            .filter(|l| {
                l.initial
                    .as_ref()
                    .is_some_and(|s| s.contains("请核实并协调"))
            })
            .count();
    assert_eq!(inputs, 1);
    let receipt = captain_socket
        .call("agent.call", tool(args.clone()))
        .await
        .unwrap();
    assert_eq!(receipt["isError"], false, "{receipt}");
    let (_, body) = receipt["text"].as_str().unwrap().split_once('\n').unwrap();
    let body: crewd_proto::CoordinationAssignment = serde_json::from_str(body).unwrap();
    assert_eq!(body.id, id);
    assert_eq!(body.decision_id, view.decision_id);
    assert_eq!(body.captain, captain);
    assert_eq!(body.owner, captain);
    assert_eq!(body.previous_owner, f.worker);
    let mut human = Socket::owned(f.state.clone());
    let query = json!({"crewId": f.crew,"id":id,"decisionId":view.decision_id});
    let result = human
        .call("work.coordinationStatus", query.clone())
        .await
        .unwrap();
    assert_eq!(result["lastAssignment"], json!(body));
    assert_eq!(result["requestedAssignment"], json!(body));
    f.state.tokens.revoke(&captain);
    let refused = captain_socket.call("agent.call", tool(args.clone())).await;
    assert!(refused.is_err() || refused.unwrap()["isError"] == true);
    captain_socket.close().await;
    human.close().await;
    f.runtime.shutdown().await;
    assert!(handle.shutdown(std::time::Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    bridge.abort();
    let _ = bridge.await;
    f.reopen().await;
    let mut human = Socket::owned(f.state.clone());
    let result = human.call("work.coordinationStatus", query).await.unwrap();
    assert_eq!(result["requestedAssignment"], json!(body));
    f.state.admission.allow(&captain);
    let mut captain_socket = Socket::attach(f.state.clone(), &captain).await;
    let replayed = captain_socket.call("agent.call", tool(args)).await.unwrap();
    assert_eq!(replayed, receipt);
    let wakes = f
        .state
        .db
        .call_result(move |s| {
            crewd_collab::wakes::list(s.conn(), None, 100).map(|rows| {
                rows.iter()
                    .filter(|row| row.source == format!("work-assigned:{id}:{}", body.revision))
                    .count()
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wakes, 1);
    captain_socket.close().await;
    human.close().await;
    f.close().await;
}

#[tokio::test]
async fn durable_report_observer_recovers_missed_hub_change_without_duplicate_dispatch() {
    let fake=FakeJev::start(vec![Reply::ok(json!({"model":"fake-native","answers":{"meaningful_progress":{"type":"noul","noul":0.1},"needs_coordination":{"type":"noul","noul":0.9}}}).to_string())]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    f.runtime.resume_pending_coordination().await.unwrap();
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    // Commit real work events but deliberately discard the hub publication batch.
    let (id, changes) = f
        .state
        .db
        .call_result(move |store| {
            crewd_collab::run(store, crewd_store::now_ms(), move |tx| {
                let mut item = work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: crew,
                        title: "迁移检查".into(),
                        owner: Some(owner.clone()),
                        ..Default::default()
                    },
                )?;
                for note in ["仍在检查，尚无结果", "仍在检查，尚无新增结果"] {
                    item = work::update(
                        tx,
                        &Sender::Session(owner.clone()),
                        WorkUpdateParams {
                            id: item.id,
                            expected_revision: item.revision,
                            note: Some(note.into()),
                            next_check_at: Some(crewd_store::ts_from_ms(tx.now_ms() + 60_000)),
                            ..Default::default()
                        },
                    )?;
                }
                Ok(item.id)
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!changes.is_empty());
    drop(changes);
    f.runtime.resume_pending_coordination().await.unwrap();
    let first = result(&f, &id).await;
    assert!(first.adopted && first.wake_id.is_some());
    for _ in 0..3 {
        f.runtime.resume_pending_coordination().await.unwrap();
    }
    let second = result(&f, &id).await;
    assert_eq!(first.decision_id, second.decision_id);
    assert_eq!(first.wake_id, second.wake_id);
    assert_eq!(fake.requests().len(), 1);
    f.close().await;
}
