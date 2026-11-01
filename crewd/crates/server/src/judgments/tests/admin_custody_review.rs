//! Independent real socket/Core responsibility transfer, separate from Work ownership.
use super::*;

struct CustodyRig {
    r: Rig,
    source: Source,
    admin: String,
    old: String,
    next: String,
    original_request: String,
    _fake: FakeJev,
}

async fn accepted_then_replaced() -> CustodyRig {
    let (r, source, admin, old, fake) = running().await;
    let mut a = Socket::attach(r.f.state.clone(), &admin).await;
    let mut c = Socket::attach(r.f.state.clone(), &old).await;
    let original_request = uuid::Uuid::new_v4().to_string();
    let offered = request(&mut a, &original_request).await;
    delivered(&r, &old, offered["wakeId"].as_i64().unwrap()).await;
    let offered = get(&mut c, &original_request).await;
    accept(&mut c, &original_request, &offered["revision"]).await;
    let baseline_crew = r.f.crew.clone();
    let baseline_case = source.id.clone();
    r.f.state
        .collab(move |tx| {
            crewd_collab::admin_case_outcome::verify(tx, &baseline_crew, &baseline_case)
        })
        .await
        .unwrap();
    let source_owner = source.owner.clone();
    r.f.state
        .collab(move |tx| crewd_collab::wakes::human_stop(tx, &source_owner))
        .await
        .unwrap();
    a.close().await;
    c.close().await;
    let crew = r.f.crew.clone();
    let next =
        r.f.state
            .collab(move |tx| {
                sessions::register(
                    tx,
                    sessions::NewSession {
                        crew_id: crew,
                        role: Role::Worker,
                        executor: "claude".into(),
                        title: Some("successor captain".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
    r.f.rpc(
        "crew.setCaptain",
        json!({"crewId":r.f.crew,"sessionId":next}),
    )
    .await
    .unwrap();
    r.handle
        .resume(&next, orchestra::core::Origin::Human)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let id = next.clone();
            let row =
                r.f.state
                    .db
                    .call_result(move |s| sessions::get(s.conn(), &id))
                    .await
                    .unwrap()
                    .unwrap();
            if row.desired == Desired::Running && !r.exec.launches(&next).is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    acknowledge_initial_input(&r, &next).await;
    CustodyRig {
        r,
        source,
        admin,
        old,
        next,
        original_request,
        _fake: fake,
    }
}

async fn custody(r: &CustodyRig) -> serde_json::Value {
    r.r.f
        .rpc(
            "adminCase.custodyGet",
            json!({"crewId":r.r.f.crew,"caseId":r.source.id}),
        )
        .await
        .unwrap()
}
async fn propose(r: &CustodyRig, request: &str, revision: &serde_json::Value) -> serde_json::Value {
    r.r.f.rpc("adminCase.custodyPropose", json!({"crewId":r.r.f.crew,"caseId":r.source.id,
        "requestId":request,"expectedRevision":revision,"reason":"Human explicitly delegates the retained administrative responsibility"})).await.unwrap()
}
async fn retirement_eligible(r: &CustodyRig, session: &str) -> bool {
    // Role-change notices are real pending input too; acknowledge the actual
    // current send before asking the actor for an idle witness.
    if !r.r.exec.sent(session).is_empty() {
        r.r.exec.emit(
            session,
            crewd_exec::session::SessionEvent::Delivery {
                id: r.r.exec.last_send_id(session),
                subject: crewd_exec::delivery::Subject::Steer,
                outcome: crewd_exec::session::DeliveryOutcome::Delivered,
            },
        );
    }
    let actor = actual_idle_witness(&r.r, session).await;
    let session = session.to_owned();
    r.r.f
        .state
        .collab(move |tx| crewd_collab::session_retirement::prepare(tx, &session, actor))
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn real_accepted_responsibility_has_readable_custody_after_captain_replacement() {
    let r = accepted_then_replaced().await;
    let crew = r.r.f.crew.clone();
    let case = r.source.id.clone();
    let result =
        r.r.f
            .state
            .db
            .call_result(move |s| {
                crewd_collab::admin_case_custody::get(
                    s.conn(),
                    &Sender::Human,
                    &crew,
                    &case,
                    crewd_store::now_ms(),
                )
            })
            .await
            .unwrap();
    let old = r.old.clone();
    r.r.close().await;
    let result = result.expect(
        "a real accepted original handoff must remain inspectable when its captain changes",
    );
    assert_eq!(result.current_custodian_session_id, old);
}

#[tokio::test]
async fn attached_current_captain_cannot_borrow_the_human_proposal_rpc() {
    let r = accepted_then_replaced().await;
    let before = custody(&r).await;
    let mut next = Socket::attach(r.r.f.state.clone(), &r.next).await;
    let attempted = next.call("adminCase.custodyPropose", json!({"crewId":r.r.f.crew,
        "caseId":r.source.id,"requestId":uuid::Uuid::new_v4().to_string(),
        "expectedRevision":before["revision"],"reason":"agent attempts to self-authorize succession"})).await;
    let after = custody(&r).await;
    next.close().await;
    r.r.close().await;
    assert_eq!(
        attempted.unwrap_err().code,
        crewd_proto::RpcError::FORBIDDEN
    );
    assert_eq!(
        after["currentCustodianSessionId"],
        before["currentCustodianSessionId"]
    );
    assert_eq!(
        after["pending"], before["pending"],
        "denied agent request creates no pending succession"
    );
}

#[tokio::test]
async fn human_proposal_and_actual_delivery_do_not_transfer_custody_until_captain_accepts() {
    let r = accepted_then_replaced().await;
    let before = current_work(&r.r.f, &r.source.work).await;
    let status = custody(&r).await;
    assert_eq!(status["currentCustodianSessionId"], r.old);
    assert_eq!(status["originalRequestId"], r.original_request);
    assert!(
        !retirement_eligible(&r, &r.old).await,
        "former captain retains unresolved obligation"
    );
    // Drain the actual Captain role-change input before queuing the new notice.
    if !r.r.exec.sent(&r.next).is_empty() {
        r.r.exec.emit(
            &r.next,
            crewd_exec::session::SessionEvent::Delivery {
                id: r.r.exec.last_send_id(&r.next),
                subject: crewd_exec::delivery::Subject::Steer,
                outcome: crewd_exec::session::DeliveryOutcome::Delivered,
            },
        );
    }
    let _ = actual_idle_witness(&r.r, &r.next).await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let proposed = propose(&r, &request_id, &status["revision"]).await;
    assert_eq!(proposed["currentCustodianSessionId"], r.old);
    assert_eq!(proposed["pending"]["toSessionId"], r.next);
    let mut next = Socket::attach(r.r.f.state.clone(), &r.next).await;
    refused(&mut next, "admin_case_custody_accept", json!({"request_id":request_id,
        "expected_revision":proposed["pending"]["revision"],"note":"explicit successor acceptance"})).await;
    delivered(
        &r.r,
        &r.next,
        proposed["pending"]["wakeId"].as_i64().unwrap(),
    )
    .await;
    let delivered_status = custody(&r).await;
    assert_eq!(delivered_status["pending"]["phase"], "delivered");
    assert_eq!(
        delivered_status["currentCustodianSessionId"], r.old,
        "ACK is not responsibility acceptance"
    );
    assert!(!retirement_eligible(&r, &r.old).await);
    let accept_args = json!({"request_id":request_id,"expected_revision":delivered_status["pending"]["revision"],
        "note":"explicit successor acceptance"});
    tool(&mut next, "admin_case_custody_accept", accept_args.clone()).await;
    let accepted = custody(&r).await;
    assert_eq!(accepted["currentCustodianSessionId"], r.next);
    assert!(
        retirement_eligible(&r, &r.old).await,
        "old custody releases only after atomic successor acceptance"
    );
    assert!(
        !retirement_eligible(&r, &r.next).await,
        "successor cannot retire while responsibility is unresolved"
    );
    tool(&mut next, "admin_case_custody_accept", accept_args).await;
    assert_eq!(
        current_work(&r.r.f, &r.source.work).await,
        before,
        "custody acceptance never changes Work ownership, events or completion"
    );
    assert_ne!(
        view(&r.r.f, &r.source.id).await["outcomeRecord"]["phase"],
        "resolved"
    );
    assert!(
        r.r.f.state.tokens.is_admin_case(&r.admin),
        "original administrative classification remains permanent"
    );
    let old_outcome = view(&r.r.f, &r.source.id).await["outcomeRecord"].clone();
    assert_eq!(old_outcome["phase"], "stale");
    let target_ids: std::collections::BTreeSet<String> =
        view(&r.r.f, &r.source.id).await["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect();
    for id in target_ids {
        let work = current_work(&r.r.f, &id).await;
        let result = next.call("agent.call", close_args(&work)).await.unwrap();
        assert_eq!(result["isError"], false, "{result}");
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if custody(&r).await["phase"] == "discharged" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("production periodic verifier must discharge current custody");
    assert!(retirement_eligible(&r, &r.next).await);
    let after_outcome = view(&r.r.f, &r.source.id).await["outcomeRecord"].clone();
    assert_eq!(after_outcome["phase"], "stale");
    // The ordinary outcome verifier also runs periodically; its timestamp may
    // advance, but fresh custody authority must not resolve that stale outcome.
    next.close().await;
    r.r.close().await;
}
