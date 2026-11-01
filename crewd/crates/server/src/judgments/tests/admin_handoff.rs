//! A delivered captain request is distinct from an explicit authorized acceptance.
#[path = "admin_custody_review.rs"]
mod custody_review;
use super::*;

async fn running() -> (Rig, Source, String, String, FakeJev) {
    running_from(prepared().await).await
}
async fn running_from(
    (f, source, permit, fake): (Fixture, Source, execution::AdminLaunchPermit, FakeJev),
) -> (Rig, Source, String, String, FakeJev) {
    let crew = f.crew.clone();
    let captain = f
        .state
        .db
        .call_result(move |s| crews::captain_of(s.conn(), &crew))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let r = Rig::new(f).await;
    let admin = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &admin).await;
    // The real runner launch mints its token. Attach only after that launch,
    // otherwise a prelaunch socket correctly becomes unauthorized at Spawned.
    r.handle
        .resume(&captain, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let cap = captain.clone();
            let row =
                r.f.state
                    .db
                    .call_result(move |s| sessions::get(s.conn(), &cap))
                    .await
                    .unwrap()
                    .unwrap();
            if row.desired == Desired::Running && !r.exec.launches(&captain).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    if r.exec.launches(&captain).first().unwrap().initial.is_some() {
        r.exec.emit(
            &captain,
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
    }
    r.exec.emit(&captain, SessionEvent::Working(true));
    r.exec.emit(&captain, SessionEvent::Working(false));
    r.handle.latches().await.unwrap();

    assert!(r.f.state.admission.contains(&admin));
    assert!(r.f.state.admission.contains(&source.owner));
    assert!(r.f.state.admission.contains(&captain));
    let who = admin.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_collab::admin_case_handoff::validate_operation(
                s.conn(),
                &Sender::Session(who),
                &crewd_collab::admin_case_handoff::Operation::Request {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    reason: "Original issue requires explicit captain coordination".into(),
                },
                crewd_store::now_ms(),
            )
            .map(|_| ())
        })
        .await
        .unwrap()
        .expect("read-only backend request-authority diagnostic");
    (r, source, admin, captain, fake)
}

/// Exercise Core's real render/reservation/driver delivery path. This is only
/// notification delivery; tests must separately submit the captain's acceptance.
async fn delivered(r: &Rig, captain: &str, wake: i64) {
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    let recipient = captain.to_owned();
    let note =
        r.f.state
            .db
            .call_result(move |s| {
                let row = wakes::by_ids(s.conn(), &[wake])?.remove(0);
                assert_eq!(row.session, recipient);
                let batch = crewd_collab::render::build_batch(
                    s.conn(),
                    &recipient,
                    crewd_collab::Locale::En,
                    &[row],
                    crewd_store::now_ms(),
                )?;
                Ok::<_, crewd_collab::CollabError>(
                    batch.items[0].note.clone().expect("typed handoff context"),
                )
            })
            .await
            .unwrap()
            .unwrap();
    let (id, subject) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut initial_ack = false;
        loop {
            if let Some(initial) = r
                .exec
                .launches(captain)
                .first()
                .and_then(|l| l.initial.clone())
            {
                if initial.contains(&note) {
                    break (BRIEF_DELIVERY_ID, Subject::Brief);
                }
                if !initial_ack {
                    r.exec.emit(
                        captain,
                        SessionEvent::Delivery {
                            id: BRIEF_DELIVERY_ID,
                            subject: Subject::Brief,
                            outcome: DeliveryOutcome::Delivered,
                        },
                    );
                    r.exec.emit(captain, SessionEvent::Working(true));
                    r.exec.emit(captain, SessionEvent::Working(false));
                    initial_ack = true;
                }
            }
            if r.exec.sent(captain).iter().any(|s| s.contains(&note)) {
                break (r.exec.last_send_id(captain), Subject::Steer);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual captain driver receives handoff request");
    r.exec.emit(
        captain,
        SessionEvent::Delivery {
            id,
            subject,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let rows =
                r.f.state
                    .db
                    .call_result(move |s| wakes::by_ids(s.conn(), &[wake]))
                    .await
                    .unwrap()
                    .unwrap();
            if rows[0].state == wakes::WakeState::Delivered {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn tool(socket: &mut Socket, name: &str, args: serde_json::Value) -> serde_json::Value {
    let result = socket
        .call("agent.call", json!({"tool":name,"args":args}))
        .await
        .unwrap_or_else(|error| panic!("tool {name} RPC failed: {error:?}"));
    assert_eq!(result["isError"], false, "{name}: {result}");
    serde_json::from_str(result["text"].as_str().expect("typed JSON receipt")).unwrap()
}
async fn refused(socket: &mut Socket, name: &str, args: serde_json::Value) {
    let result = socket
        .call("agent.call", json!({"tool":name,"args":args}))
        .await;
    assert!(
        result.is_err() || result.as_ref().is_ok_and(|v| v["isError"] == true),
        "{name} unexpectedly succeeded: {result:?}"
    );
}
async fn get(socket: &mut Socket, id: &str) -> serde_json::Value {
    tool(socket, "admin_case_handoff_get", json!({"request_id":id})).await
}
async fn request(socket: &mut Socket, id: &str) -> serde_json::Value {
    tool(
        socket,
        "admin_case_handoff_request",
        json!({"request_id":id,"reason":"Original issue requires explicit captain coordination"}),
    )
    .await
}
async fn accept(socket: &mut Socket, id: &str, revision: &serde_json::Value) -> serde_json::Value {
    tool(socket,"admin_case_handoff_accept",json!({"request_id":id,"expected_revision":revision,"note":"I explicitly accept coordination responsibility"})).await
}
#[tokio::test]
async fn actual_captain_delivery_is_not_acceptance_and_acceptance_does_not_finish_original_work() {
    let (r, source, admin, captain, _fake) = running().await;
    let before = current_work(&r.f, &source.work).await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    let wake = offered["wakeId"]
        .as_i64()
        .expect("unique typed captain wake");
    assert_ne!(offered["phase"], "accepted");
    let repeat = request(&mut a, &id).await;
    assert_eq!(
        repeat["wakeId"], offered["wakeId"],
        "identical request must keep its notification identity"
    );
    refused(&mut c,"admin_case_handoff_accept",json!({"request_id":id,"expected_revision":offered["revision"],"note":"premature accept before delivery"})).await;
    delivered(&r, &captain, wake).await;
    let delivered = get(&mut c, &id).await;
    assert_eq!(delivered["phase"], "delivered", "{delivered}");
    assert_eq!(current_work(&r.f, &source.work).await, before);
    let accepted = accept(&mut c, &id, &delivered["revision"]).await;
    assert_eq!(accepted["phase"], "accepted", "{accepted}");
    assert_eq!(
        current_work(&r.f, &source.work).await,
        before,
        "captain accepts a case, not forced Work ownership or completion"
    );
    assert_ne!(
        view(&r.f, &source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    let forbidden = a.call("agent.call", close_args(&before)).await;
    assert!(
        forbidden.is_err() || forbidden.as_ref().is_ok_and(|v| v["isError"] == true),
        "accepted handoff must seal old Admin effects: {forbidden:?}"
    );
    assert_eq!(current_work(&r.f, &source.work).await, before);
    a.close().await;
    c.close().await;
    r.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_captain_acceptance_rechecks_actual_token_and_has_no_acceptance_effect() {
    let (r, source, admin, captain, _fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    let before = current_work(&r.f, &source.work).await;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *r.f.state.test_db_write_barrier.lock().unwrap() = Some(crate::DbWriteBarrier {
        entered: entered_tx,
        release: release_rx,
    });
    let request = id.clone();
    let pending = tokio::spawn(async move {
        let result=c.call("agent.call",json!({"tool":"admin_case_handoff_accept","args":{"request_id":request,"expected_revision":current["revision"],"note":"attempt during revoked authorization"}})).await;
        (c, result)
    });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    r.f.state.tokens.revoke(&captain);
    release_tx.send(()).unwrap();
    let (c, result) = pending.await.unwrap();
    assert!(
        result.is_err() || result.as_ref().is_ok_and(|v| v["isError"] == true),
        "{result:?}"
    );
    assert_eq!(get(&mut a, &id).await["phase"], "delivered");
    assert_eq!(current_work(&r.f, &source.work).await, before);
    a.close().await;
    c.close().await;
    r.close().await;
}
#[tokio::test]
async fn nonrecipient_and_direct_internal_call_cannot_claim_captain_acceptance() {
    let (r, source, admin, captain, _fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let mut worker = Socket::attach(r.f.state.clone(), &source.owner).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    let args = json!({"request_id":id,"expected_revision":current["revision"],"note":"explicit acceptance"});
    refused(&mut worker, "admin_case_handoff_accept", args.clone()).await;
    refused(&mut a, "admin_case_handoff_accept", args.clone()).await;
    let foreign =
        r.f.state
            .collab(|tx| {
                let crew = crews::create(
                    tx,
                    crews::NewCrew {
                        title: "foreign team".into(),
                        working_directory: "/tmp/foreign-handoff-fixture".into(),
                        ..Default::default()
                    },
                )?;
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: crew.id,
                        role: Role::Captain,
                        executor: "claude".into(),
                        title: Some("foreign captain".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
    r.f.state.admission.allow(&foreign.id);
    let mut outsider = Socket::attach(r.f.state.clone(), &foreign.id).await;
    refused(
        &mut outsider,
        "admin_case_handoff_get",
        json!({"request_id":id}),
    )
    .await;
    refused(&mut outsider, "admin_case_handoff_accept", args.clone()).await;
    let list = tool(
        &mut outsider,
        "admin_case_handoff_list",
        json!({"limit":10}),
    )
    .await;
    assert_eq!(list["items"], json!([]));
    outsider.close().await;

    let me = crate::agent::port::identity(&r.f.state, &captain)
        .await
        .unwrap()
        .unwrap();
    let direct =
        crate::agent::tools::call(&r.f.state, &me, "admin_case_handoff_accept", args).await;
    assert!(
        direct.is_error,
        "a caller-shaped identity cannot replace actual RPC authority"
    );
    assert_eq!(get(&mut c, &id).await["phase"], "delivered");
    assert_eq!(
        accept(&mut c, &id, &current["revision"]).await["phase"],
        "accepted"
    );
    a.close().await;
    c.close().await;
    worker.close().await;
    r.close().await;
}

#[tokio::test]
async fn changed_original_facts_source_stop_or_captain_invalidates_the_old_acceptance() {
    for mutation in ["work", "source_stop", "captain"] {
        let (r, source, admin, captain, _fake) = running().await;
        let mut a = Socket::attach(r.f.state.clone(), &admin).await;
        let mut c = Socket::attach(r.f.state.clone(), &captain).await;
        let id = uuid::Uuid::new_v4().to_string();
        let offered = request(&mut a, &id).await;
        delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
        let before = get(&mut c, &id).await;
        match mutation {
            "work" => {
                let w = current_work(&r.f, &source.work).await;
                r.f.state
                    .collab(move |tx| {
                        work::update(
                            tx,
                            &Sender::Human,
                            crewd_proto::WorkUpdateParams {
                                id: w.id,
                                expected_revision: w.revision,
                                note: Some("new original-work fact".into()),
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
            "captain" => {
                let crew = r.f.crew.clone();
                let next = source.owner.clone();
                r.f.state
                    .collab(move |tx| crews::set_captain(tx, &crew, &next))
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        refused(&mut c,"admin_case_handoff_accept",json!({"request_id":id,"expected_revision":before["revision"],"note":"attempt against obsolete request facts"})).await;
        let status = get(&mut a, &id).await;
        assert_eq!(status["phase"], "stale", "{mutation}: {status}");
        a.close().await;
        c.close().await;
        r.close().await;
    }
}

#[tokio::test]
async fn accepted_captain_obligation_survives_source_staleness_and_blocks_idle_retirement() {
    use crewd_exec::session::SessionEvent;
    let (r, source, admin, captain, fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let cap = captain.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(s.conn(), &cap, "retained-captain-fixture")
        })
        .await
        .unwrap()
        .unwrap();
    r.exec.emit(&captain, SessionEvent::Working(true));
    r.exec.emit(&captain, SessionEvent::Working(false));
    let witness = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == &captain)
            {
                break w;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("physical idle captain witness");
    let cap = captain.clone();
    let proof = witness.clone();
    assert!(
        r.f.state
            .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &cap, proof))
            .await
            .unwrap()
            .is_some(),
        "before explicit acceptance otherwise idle Captain may be considered for retirement"
    );
    let current = get(&mut c, &id).await;
    assert_eq!(
        accept(&mut c, &id, &current["revision"]).await["phase"],
        "accepted"
    );
    for make_stale in [false, true] {
        if make_stale {
            let source = source.owner.clone();
            r.f.state
                .collab(move |tx| wakes::human_stop(tx, &source))
                .await
                .unwrap();
        }
        let cap = captain.clone();
        let proof = witness.clone();
        assert!(
            r.f.state
                .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &cap, proof))
                .await
                .unwrap()
                .is_none(),
            "accepted unresolved captain responsibility cannot disappear when source becomes stale"
        );
    }
    assert_eq!(
        fake.requests().len(),
        1,
        "eligibility assertions never call retirement model"
    );
    a.close().await;
    c.close().await;
    r.close().await;
}

#[tokio::test]
async fn accepted_responsibility_remains_discoverable_after_real_database_reopen() {
    let (r, source, admin, captain, _fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    accept(&mut c, &id, &current["revision"]).await;
    let before = current_work(&r.f, &source.work).await;
    a.close().await;
    c.close().await;
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
    // This is fresh explicit fixture admission, never reconstructed from an old token.
    f.state.admission.allow(&captain);
    let mut c = Socket::attach(f.state.clone(), &captain).await;
    let list = tool(&mut c, "admin_case_handoff_list", json!({"limit":10})).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["requestId"] == id),
        "durable accepted responsibility disappeared: {list}"
    );
    assert_eq!(current_work(&f, &source.work).await, before);
    c.close().await;
    f.close().await;
}

#[tokio::test]
async fn accepted_captain_progress_keeps_handoff_current_and_admin_retirement_eligible() {
    use crewd_exec::session::SessionEvent;
    let (r, source, admin, captain, _fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    accept(&mut c, &id, &current["revision"]).await;
    let admin_id = admin.clone();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(
                s.conn(),
                &admin_id,
                "retained-handoff-admin",
            )
        })
        .await
        .unwrap()
        .unwrap();
    acknowledge_initial_input(&r, &admin).await;
    r.exec.emit(&admin, SessionEvent::Working(true));
    r.exec.emit(&admin, SessionEvent::Working(false));
    let witness = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == &admin)
            {
                break w;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let adm = admin.clone();
    let proof = witness.clone();
    assert!(
        r.f.state
            .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &adm, proof))
            .await
            .unwrap()
            .is_some(),
        "explicit accepted handoff releases temporary Admin's obligation"
    );
    let before = current_work(&r.f, &source.work).await;
    let result=c.call("agent.call",json!({"tool":"work_item_update","args":{"id":source.work,"expected_revision":before.revision,"status":"active","next_check_at":crewd_store::ts_from_ms(crewd_store::now_ms()+600_000),"note":"Captain explicitly schedules the ongoing work"}})).await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    assert_ne!(
        current_work(&r.f, &source.work).await.revision,
        before.revision
    );
    assert_eq!(
        get(&mut c, &id).await["phase"],
        "accepted",
        "ordinary progress cannot revoke accepted responsibility"
    );
    let adm = admin.clone();
    assert!(
        r.f.state
            .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &adm, witness))
            .await
            .unwrap()
            .is_some(),
        "captain progress must not re-block the original administrative retirement"
    );
    assert_ne!(
        view(&r.f, &source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    a.close().await;
    c.close().await;
    r.close().await;
}

#[tokio::test]
async fn former_captain_can_read_own_stale_responsibility_but_not_accept_and_other_workers_cannot_enumerate()
 {
    let (r, source, admin, captain, _fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    accept(&mut c, &id, &current["revision"]).await;
    let crew = r.f.crew.clone();
    let next = source.owner.clone();
    let other =
        r.f.state
            .collab(move |tx| {
                crews::set_captain(tx, &crew, &next)?;
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: crew,
                        role: Role::Worker,
                        executor: "claude".into(),
                        title: Some("other worker".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
    let previous = crate::agent::port::identity(&r.f.state, &captain)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(previous.role, Role::Worker);
    let stale = get(&mut c, &id).await;
    assert_eq!(stale["phase"], "stale");
    let list = tool(&mut c, "admin_case_handoff_list", json!({"limit":10})).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["requestId"] == id && v["phase"] == "stale")
    );
    refused(&mut c,"admin_case_handoff_accept",json!({"request_id":id,"expected_revision":current["revision"],"note":"I explicitly accept coordination responsibility"})).await;
    r.f.state.admission.allow(&other.id);
    let mut stranger = Socket::attach(r.f.state.clone(), &other.id).await;
    refused(
        &mut stranger,
        "admin_case_handoff_get",
        json!({"request_id":id}),
    )
    .await;
    refused(
        &mut stranger,
        "admin_case_handoff_list",
        json!({"limit":10}),
    )
    .await;
    stranger.close().await;
    a.close().await;
    c.close().await;
    r.close().await;
}

async fn actual_idle_witness(r: &Rig, id: &str) -> crewd_collab::session_retirement::ActorWitness {
    use crewd_exec::session::SessionEvent;
    let sid = id.to_owned();
    r.f.state
        .db
        .call_result(move |s| {
            crewd_store::sessions::set_agent_session_id(s.conn(), &sid, &format!("retained-{sid}"))
        })
        .await
        .unwrap()
        .unwrap();
    acknowledge_initial_input(r, id).await;
    r.exec.emit(id, SessionEvent::Working(true));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !r.handle.live(id).await.unwrap().working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Core observed Working(true) after preceding delivery receipt");
    r.exec.emit(id, SessionEvent::Working(false));
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.handle.live(id).await.unwrap().working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Core observed subsequent Working(false)");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some((_, w)) = r
                .handle
                .retirement_candidates()
                .await
                .unwrap()
                .into_iter()
                .find(|(s, _)| s == id)
            {
                break w;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual idle actor witness")
}
async fn retirement_phase(
    r: &Rig,
    id: &str,
    want: crewd_proto::RetirementPhase,
) -> crewd_proto::RetirementView {
    let waited = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let crew = r.f.crew.clone();
            let sid = id.to_owned();
            let status =
                r.f.state
                    .db
                    .call_result(move |s| {
                        crewd_collab::session_retirement::status(s.conn(), &crew, &sid)
                    })
                    .await
                    .unwrap()
                    .unwrap();
            if let Some(record) = status.record {
                if record.phase == want {
                    break record;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if let Ok(record) = waited {
        return record;
    }
    let crew = r.f.crew.clone();
    let sid = id.to_owned();
    let status =
        r.f.state
            .db
            .call_result(move |s| crewd_collab::session_retirement::status(s.conn(), &crew, &sid))
            .await
            .unwrap()
            .unwrap();
    let live = r.handle.live(id).await.unwrap();
    panic!(
        "expected {want:?}; actual={status:?}; live={live:?}; stops={:?}",
        r.exec.lock().stops
    );
}
#[tokio::test]
async fn accepted_unresolved_handoff_physically_parks_admin_while_captain_keeps_responsibility() {
    use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
    use crewd_proto::RetirementPhase;
    let fake = FakeJev::start(vec![
        answer(0.99),
        Reply::ok(
            json!({"model":"fake-native","answers":{"can_retire":{"type":"noul","noul":0.99}}})
                .to_string(),
        ),
    ])
    .await;
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
    let (r, source, admin, captain, fake) = running_from((f, source, permit, fake)).await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &captain).await;
    let id = uuid::Uuid::new_v4().to_string();
    let before = current_work(&r.f, &source.work).await;
    let offered = request(&mut a, &id).await;
    delivered(&r, &captain, offered["wakeId"].as_i64().unwrap()).await;
    let current = get(&mut c, &id).await;
    assert_eq!(
        accept(&mut c, &id, &current["revision"]).await["phase"],
        "accepted"
    );
    let crew = r.f.crew.clone();
    let case = source.id.clone();
    let outcome =
        r.f.state
            .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &case))
            .await
            .unwrap();
    assert_eq!(
        outcome.phase,
        crewd_proto::AdminCaseOutcomePhase::Unresolved
    );
    r.exec.lock().hang_on_stop.insert(admin.clone());
    let witness = actual_idle_witness(&r, &admin).await;
    assert!(
        !r.f.runtime.closed.load(Ordering::Acquire),
        "fixture did not shut down runtime"
    );
    let diagnostic_actor = witness.clone();
    let before_status = {
        let crew = r.f.crew.clone();
        let sid = admin.clone();
        r.f.state
            .db
            .call_result(move |s| crewd_collab::session_retirement::status(s.conn(), &crew, &sid))
            .await
            .unwrap()
            .unwrap()
    };
    eprintln!(
        "before run: actor={diagnostic_actor:?}; status={before_status:?}; requests={}",
        fake.requests().len()
    );
    r.f.runtime
        .run_retirement(admin.clone(), witness, true)
        .await
        .unwrap();
    let immediate = {
        let crew = r.f.crew.clone();
        let sid = admin.clone();
        r.f.state
            .db
            .call_result(move |s| crewd_collab::session_retirement::status(s.conn(), &crew, &sid))
            .await
            .unwrap()
            .unwrap()
    };
    let actors = r.handle.retirement_candidates().await.unwrap();
    let questions: Vec<_> = fake
        .requests()
        .iter()
        .map(|req| {
            let text = String::from_utf8_lossy(&req.body);
            (
                text.contains("needs_intervention"),
                text.contains("can_retire"),
            )
        })
        .collect();
    eprintln!(
        "after run: status={immediate:?}; initial={diagnostic_actor:?}; current={:?}; question(needs_intervention,can_retire)={questions:?}; closed={}",
        actors.into_iter().find(|(s, _)| s == &admin),
        r.f.runtime.closed.load(Ordering::Acquire)
    );
    assert!(
        !retirement_phase(&r, &admin, RetirementPhase::ExitRequested)
            .await
            .adopted
    );
    assert!(r.handle.live(&admin).await.unwrap().running);
    assert_eq!(fake.requests().len(), 2);
    r.exec.emit(
        &admin,
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit: false,
        }),
    );
    assert!(
        retirement_phase(&r, &admin, RetirementPhase::Parked)
            .await
            .adopted
    );
    assert_eq!(
        view(&r.f, &source.id).await["retirementRecord"]["phase"],
        "parked"
    );
    assert_eq!(
        view(&r.f, &source.id).await["outcomeRecord"]["phase"],
        "unresolved"
    );
    assert_eq!(
        get(&mut c, &id).await["phase"],
        "accepted",
        "captain can still read responsibility after Admin admission is revoked and process exits"
    );
    let list = tool(&mut c, "admin_case_handoff_list", json!({"limit":10})).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["requestId"] == id)
    );
    assert_eq!(current_work(&r.f, &source.work).await, before);
    let witness = actual_idle_witness(&r, &captain).await;
    let cap = captain.clone();
    assert!(
        r.f.state
            .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &cap, witness))
            .await
            .unwrap()
            .is_none(),
        "unresolved accepted responsibility survives the temporary executor's physical exit"
    );
    r.exec.lock().hang_on_stop.remove(&admin);
    a.close().await;
    c.close().await;
    r.close().await;
}
