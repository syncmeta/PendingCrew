//! Outcome proof through the same actual Core and attached socket fixture.
use super::*;

/// Insert an additional original target before the authoritative stop escalation.
/// Both targets then belong to a single immutable case, rather than manufacturing case metadata.
async fn launched_pair() -> (Rig, Source, String, String, FakeJev) {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    let extra = f
        .state
        .collab(move |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew,
                    owner: Some(owner),
                    title: "second original target".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    let source = sources(&f, 1).await.remove(0);
    let v = view(&f, &source.id).await;
    assert_eq!(v["targets"].as_array().unwrap().len(), 2);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let id = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &id))
        .await
        .unwrap()
        .unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    (r, source, extra.id, sid, fake)
}

async fn verify(r: &Rig, source: &Source) -> serde_json::Value {
    let crew = r.f.crew.clone();
    let id = source.id.clone();
    serde_json::to_value(
        r.f.state
            .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &id))
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn close_target(socket: &mut Socket, r: &Rig, id: &str) {
    let work = current_work(&r.f, id).await;
    let response = socket.call("agent.call", close_args(&work)).await.unwrap();
    assert_eq!(response["isError"], false, "{response}");
}
#[tokio::test]
async fn running_or_claiming_success_is_not_resolution_and_partial_targets_remain_open() {
    let (r, source, extra, sid, _fake) = launched_pair().await;
    let v = verify(&r, &source).await;
    assert_ne!(
        v["phase"], "resolved",
        "a physical launch is not administrative work: {v}"
    );
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    let declaration=socket.call("agent.call",json!({"tool":"post","args":{"channel":"lobby","text":"All administrative issues resolved"}})).await;
    assert!(declaration.is_err() || declaration.as_ref().is_ok_and(|v| v["isError"] == true));
    assert_ne!(verify(&r, &source).await["phase"], "resolved");
    close_target(&mut socket, &r, &source.work).await;
    let partial = verify(&r, &source).await;
    assert_ne!(
        partial["phase"], "resolved",
        "second original target remains: {partial}"
    );
    assert_ne!(
        serde_json::to_value(current_work(&r.f, &extra).await).unwrap()["status"],
        "done"
    );
    close_target(&mut socket, &r, &extra).await;
    let complete = verify(&r, &source).await;
    assert_eq!(complete["phase"], "resolved", "{complete}");
    assert_eq!(
        complete["resolvedByAdmin"].as_array().unwrap().len(),
        2,
        "{complete}"
    );
    assert!(
        complete["resolvedByAdmin"]
            .as_array()
            .unwrap()
            .contains(&json!(source.work))
    );
    assert!(
        complete["resolvedByAdmin"]
            .as_array()
            .unwrap()
            .contains(&json!(extra))
    );
    assert_eq!(complete["resolvedByOthers"], json!([]));
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn human_closure_is_attributed_to_other_not_the_case_executor() {
    let (r, source, extra, sid, _fake) = launched_pair().await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    let p = current_work(&r.f, &extra).await;
    r.f.state
        .collab(move |tx| {
            work::close_factual(
                tx,
                &Sender::Human,
                crewd_proto::WorkCloseParams {
                    id: p.id,
                    expected_revision: p.revision,
                    kind: crewd_proto::WorkClosureKind::Done,
                    evidence: crewd_proto::WorkEvidenceParam {
                        kind: crewd_proto::WorkEvidenceKind::Text,
                        value: "external human verification".into(),
                    },
                    note: "human resolved remaining issue".into(),
                },
            )
        })
        .await
        .unwrap();
    let outcome = verify(&r, &source).await;
    assert_eq!(outcome["phase"], "resolved", "{outcome}");
    assert_eq!(
        outcome["resolvedByAdmin"],
        json!([source.work]),
        "{outcome}"
    );
    assert_eq!(outcome["resolvedByOthers"], json!([extra]), "{outcome}");
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn source_stop_configuration_and_restore_invalidate_completed_effects_before_outcome() {
    for mutation in ["stop", "config", "restore"] {
        let (r, source, extra, sid, fake) = launched_pair().await;
        let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
        close_target(&mut socket, &r, &source.work).await;
        close_target(&mut socket, &r, &extra).await;
        match mutation {
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
            _ => unreachable!(),
        }
        let outcome = verify(&r, &source).await;
        assert_eq!(outcome["phase"], "stale", "{mutation}: {outcome}");
        socket.close().await;
        r.close().await;
    }
}
async fn persisted_snapshot(f: &Fixture) -> (Vec<(String, String)>, Vec<String>) {
    f.state
        .db
        .call_result(|s| {
            let mut q = s
                .conn()
                .prepare("SELECT key,value FROM meta ORDER BY key")?;
            let meta = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut q = s
                .conn()
                .prepare("SELECT body FROM organization_work_events ORDER BY seq")?;
            let events = q
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, crewd_store::StoreError>((meta, events))
        })
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn real_status_rpc_is_passive_after_actual_effects() {
    let (r, source, extra, sid, _fake) = launched_pair().await;
    // Pause the old pump so readonly RPC assertions cannot race unrelated periodic writes.
    r.f.runtime.shutdown().await;
    let before = persisted_snapshot(&r.f).await;
    let result =
        r.f.rpc("adminCase.get", json!({"crewId":r.f.crew,"id":source.id}))
            .await
            .unwrap();
    assert_ne!(result["outcomeRecord"]["phase"], "resolved");
    assert_eq!(persisted_snapshot(&r.f).await, before);
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    close_target(&mut socket, &r, &extra).await;
    let before = persisted_snapshot(&r.f).await;
    r.f.rpc("adminCase.get", json!({"crewId":r.f.crew,"id":source.id}))
        .await
        .unwrap();
    assert_eq!(
        persisted_snapshot(&r.f).await,
        before,
        "read must not verify and save an outcome"
    );
    socket.close().await;
    r.close().await;
}
#[tokio::test]
async fn production_periodic_runtime_records_actual_completion_without_direct_verify() {
    let (r, source, extra, sid, _fake) = launched_pair().await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    close_target(&mut socket, &r, &extra).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let v =
                r.f.rpc("adminCase.get", json!({"crewId":r.f.crew,"id":source.id}))
                    .await
                    .unwrap();
            if v["outcomeRecord"]["phase"] == "resolved" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("production periodic outcome verification");
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn adopted_but_unexecuted_case_cannot_create_success_outcome() {
    let (f, source, _permit, _fake) = prepared().await;
    f.runtime.shutdown().await;
    let before = persisted_snapshot(&f).await;
    let crew = f.crew.clone();
    let id = source.id.clone();
    assert!(
        f.state
            .collab(move |tx| crewd_collab::admin_case_outcome::verify(tx, &crew, &id))
            .await
            .is_err()
    );
    assert_eq!(persisted_snapshot(&f).await, before);
    let read = f
        .rpc("adminCase.get", json!({"crewId":f.crew,"id":source.id}))
        .await
        .unwrap();
    assert!(read["outcomeRecord"].is_null());
    f.close().await;
}

async fn silent_original_target(f: &Fixture) -> String {
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    f.state
        .db
        .call_result(move |store| {
            let old = crewd_store::now_ms() - 86_400_000;
            let (id, _) = crewd_collab::run(store, old, |tx| {
                sessions::set_desired(tx, &owner, Desired::Running)?;
                let p = work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: crew,
                        owner: Some(owner.clone()),
                        title: "silent original target".into(),
                        ..Default::default()
                    },
                )?;
                work::update(
                    tx,
                    &Sender::Session(owner.clone()),
                    crewd_proto::WorkUpdateParams {
                        id: p.id.clone(),
                        expected_revision: p.revision,
                        status: Some(crewd_proto::WorkUpdateStatus::Active),
                        next_check_at: Some(crewd_store::ts_from_ms(old + 1_000)),
                        note: Some("historical owner progress".into()),
                        ..Default::default()
                    },
                )?;
                Ok(p.id)
            })?;
            for step in 1..=crewd_work::Config::default().missed_checks_limit {
                crewd_collab::run(store, old + i64::from(step) * 3_600_000, |tx| {
                    assert_eq!(crewd_collab::checkpoints::enqueue_due(tx)?, 1);
                    let rows = wakes::list(tx.conn(), Some(&owner), 100)?;
                    let pending: Vec<_> = rows
                        .iter()
                        .filter(|r| {
                            r.state == wakes::WakeState::Pending
                                && r.source.starts_with("work-check:")
                        })
                        .map(|r| r.id)
                        .collect();
                    assert_eq!(pending.len(), 1);
                    wakes::claim(tx, &pending)?;
                    // Original-source construction uses the real durable checkpoint ACK path,
                    // not a physical runner. The new-owner delivery is tested separately below.
                    wakes::ack_delivered(tx, &owner, &pending, wakes::Riders::Release, &[], &[])?;
                    Ok(())
                })?;
            }
            Ok::<_, crewd_collab::CollabError>(id)
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn verified_silent_reassignment_waits_for_actual_new_owner_delivery_and_schedule() {
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let silent = silent_original_target(&f).await;
    let source = sources(&f, 1).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let id = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &id))
        .await
        .unwrap()
        .unwrap();
    let replacement = f
        .state
        .db
        .call_result({
            let crew = f.crew.clone();
            move |s| crews::captain_of(s.conn(), &crew)
        })
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    let current = current_work(&r.f, &silent).await;
    let reassigned=socket.call("agent.call",json!({"tool":"work_item_reassign_lost","args":{"id":silent,"expected_revision":current.revision,"owner":replacement,"note":"verified unanswered checkpoints"}})).await.unwrap();
    assert_eq!(reassigned["isError"], false, "{reassigned}");
    let pending = verify(&r, &source).await;
    assert_ne!(
        pending["phase"], "resolved",
        "owner assignment alone is not delivery: {pending}"
    );
    let owner = replacement.clone();
    let target = silent.clone();
    let (row, note) =
        r.f.state
            .db
            .call_result(move |s| {
                let row = wakes::list(s.conn(), Some(&owner), 100)?
                    .into_iter()
                    .find(|r| r.source.starts_with(&format!("work-lost:{target}:")))
                    .expect("real lost-owner notice");
                let batch = crewd_collab::render::build_batch(
                    s.conn(),
                    &owner,
                    crewd_collab::Locale::En,
                    std::slice::from_ref(&row),
                    crewd_store::now_ms(),
                )?;
                Ok::<_, crewd_collab::CollabError>((row, batch.items[0].note.clone().unwrap()))
            })
            .await
            .unwrap()
            .unwrap();
    let (delivery, subject) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut acked_initial = false;
        loop {
            if let Some(initial) = r
                .exec
                .launches(&replacement)
                .first()
                .and_then(|l| l.initial.clone())
            {
                if initial.contains(&note) {
                    break (BRIEF_DELIVERY_ID, Subject::Brief);
                }
                if !acked_initial {
                    r.exec.emit(
                        &replacement,
                        SessionEvent::Delivery {
                            id: BRIEF_DELIVERY_ID,
                            subject: Subject::Brief,
                            outcome: DeliveryOutcome::Delivered,
                        },
                    );
                    r.exec.emit(&replacement, SessionEvent::Working(true));
                    r.exec.emit(&replacement, SessionEvent::Working(false));
                    acked_initial = true;
                }
            }
            if r.exec
                .sent(&replacement)
                .iter()
                .any(|text| text.contains(&note))
            {
                break (r.exec.last_send_id(&replacement), Subject::Steer);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual Core delivers lost-owner notice");
    r.exec.emit(
        &replacement,
        SessionEvent::Delivery {
            id: delivery,
            subject,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let id = row.id;
            let rows =
                r.f.state
                    .db
                    .call_result(move |s| wakes::by_ids(s.conn(), &[id]))
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
    let p = current_work(&r.f, &silent).await;
    let owner = replacement.clone();
    r.f.state
        .collab(move |tx| {
            work::update(
                tx,
                &Sender::Session(owner),
                crewd_proto::WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    status: Some(crewd_proto::WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(crewd_store::now_ms() + 600_000)),
                    note: Some("replacement scheduled next check".into()),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    let outcome = verify(&r, &source).await;
    assert_eq!(outcome["phase"], "resolved", "{outcome}");
    assert_ne!(
        current_work(&r.f, &silent).await.status,
        "done",
        "administrative issue resolved without inventing completion of the work"
    );
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn resolving_in_scope_work_does_not_claim_the_foreign_targets_are_resolved() {
    let fake = FakeJev::start(vec![answer(0.99)]).await;
    let f = Fixture::new().await;
    f.configure(&fake.base_url).await;
    let source = source_fixture(&f, 1, true).await.remove(0);
    f.runtime
        .run_admin_case(f.crew.clone(), source.id.clone(), true)
        .await
        .unwrap();
    let crew = f.crew.clone();
    let id = source.id.clone();
    let permit = f
        .state
        .collab(move |tx| execution::launch_permit(tx, &crew, &id))
        .await
        .unwrap()
        .unwrap();
    let r = Rig::new(f).await;
    let sid = r.handle.start_admin_case(permit).await.unwrap().id;
    wait_started(&r, &source.id, &sid).await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    let outcome = verify(&r, &source).await;
    assert_ne!(outcome["phase"], "resolved", "{outcome}");
    assert_eq!(
        outcome["outOfScope"].as_array().unwrap().len(),
        1,
        "{outcome}"
    );
    assert_eq!(outcome["resolvedByAdmin"], json!([source.work]));
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn verified_business_outcome_survives_execution_unknown_without_claiming_retirement() {
    let (r, source, extra, sid, _fake) = launched_pair().await;
    let mut socket = Socket::attach(r.f.state.clone(), &sid).await;
    close_target(&mut socket, &r, &source.work).await;
    close_target(&mut socket, &r, &extra).await;
    assert_eq!(verify(&r, &source).await["phase"], "resolved");
    let crew = r.f.crew.clone();
    let case = source.id.clone();
    r.f.state
        .collab(move |tx| {
            let witness = execution::binding(tx.conn(), &crew, &case)?.expect("bound execution");
            // Deliberate DAO receipt: tests business/execution-state independence only,
            // not physical process exit, successful retirement, or Parked.
            assert!(execution::mark_unknown(tx, &witness)?);
            Ok(())
        })
        .await
        .unwrap();
    let crew = r.f.crew.clone();
    let case = source.id.clone();
    let outcome =
        r.f.state
            .db
            .call_result(move |s| {
                crewd_collab::admin_case_outcome::status_at(
                    s.conn(),
                    &crew,
                    &case,
                    crewd_store::now_ms(),
                )
            })
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(serde_json::to_value(outcome).unwrap()["phase"], "resolved");
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["phase"],
        "unknown"
    );
    socket.close().await;
    r.close().await;
}

#[tokio::test]
async fn other_actors_can_resolve_every_problem_without_crediting_the_admin() {
    let (r, source, extra, _sid, _fake) = launched_pair().await;
    for id in [&source.work, &extra] {
        let p = current_work(&r.f, id).await;
        r.f.state
            .collab(move |tx| {
                work::close_factual(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkCloseParams {
                        id: p.id,
                        expected_revision: p.revision,
                        kind: crewd_proto::WorkClosureKind::Done,
                        evidence: crewd_proto::WorkEvidenceParam {
                            kind: crewd_proto::WorkEvidenceKind::Text,
                            value: "Human independently verified outcome".into(),
                        },
                        note: "Human closed the original issue".into(),
                    },
                )
            })
            .await
            .unwrap();
    }
    let outcome = verify(&r, &source).await;
    assert_eq!(outcome["phase"], "resolved", "{outcome}");
    assert_eq!(outcome["resolvedByAdmin"], json!([]));
    let others = outcome["resolvedByOthers"].as_array().unwrap();
    assert_eq!(others.len(), 2);
    assert!(others.contains(&json!(source.work)) && others.contains(&json!(extra)));
    assert_eq!(
        view(&r.f, &source.id).await["executionRecord"]["effectsCount"],
        0
    );
    r.close().await;
}
