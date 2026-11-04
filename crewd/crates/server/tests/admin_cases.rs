//! Human administrative records travel through the real owned socket boundary.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;

async fn call(c: &mut RpcClient, method: &str, params: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(Duration::from_secs(10), c.call(method, params))
        .await
        .expect("bounded socket response")
}

#[tokio::test]
async fn list_is_passive_and_attached_agents_cannot_borrow_human_case_access() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let hello = call(&mut human, "system.hello", json!({})).await.unwrap();
    for method in [
        "adminCase.custodyGet",
        "adminCase.custodyTransferGet",
        "adminCase.custodyPropose",
        "adminCase.automaticExecution",
        "adminCase.effects",
        "adminCase.humanBrief",
        "adminCase.legacyRecovery",
        "adminCase.legacyList",
        "adminCase.legacyGet",
        "adminCase.legacyPreview",
        "adminCase.legacyConfirm",
        "adminCase.legacyReceipt",
        "adminCase.similarRequirements",
        "adminCase.requirementReviews",
        "adminCase.requirementReview",
    ] {
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == method),
            "the real client handshake must advertise {method}"
        );
    }
    let changes = f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
    let unknown_request = uuid::Uuid::new_v4().to_string();
    let missing = call(
        &mut human,
        "adminCase.custodyTransferGet",
        json!({"requestId":unknown_request}),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.0, crewd_proto::RpcError::NOT_FOUND);
    let page = call(
        &mut human,
        "adminCase.list",
        json!({"crewId": f.crew, "limit": 1}),
    )
    .await
    .expect("Human may inspect current crew administrative records");
    assert_eq!(page["crewId"], f.crew);
    assert_eq!(page["items"], json!([]));
    assert!(page["nextAfter"].is_null());
    assert_eq!(
        changes,
        f.state.db.call(|s| s.conn().total_changes()).await.unwrap(),
        "reading an empty list must not create a case, counter, or runtime state"
    );
    for role in [crewd_proto::Role::Worker, crewd_proto::Role::Admin] {
        let sid = f.register(&f.crew, role, "case reader").await;
        let mut agent = RpcClient::connect(&socket).await;
        call(
            &mut agent,
            "agent.attach",
            json!({"sessionId":sid,"token":f.token(&sid)}),
        )
        .await
        .unwrap();
        for (method, params) in [
            ("adminCase.list", json!({"crewId":f.crew})),
            (
                "adminCase.get",
                json!({"crewId":f.crew,"id":"AC00000000000000000001"}),
            ),
            (
                "adminCase.effects",
                json!({"crewId":f.crew,"id":"AC00000000000000000001"}),
            ),
            ("adminCase.humanBrief", json!({"crewId":f.crew})),
            (
                "adminCase.custodyGet",
                json!({"crewId":f.crew,"caseId":"AC00000000000000000001"}),
            ),
            (
                "adminCase.custodyTransferGet",
                json!({"requestId":unknown_request}),
            ),
            (
                "adminCase.custodyPropose",
                json!({"crewId":f.crew,"caseId":"AC00000000000000000001",
                    "requestId":unknown_request,"expectedRevision":"untrusted","reason":"cannot self authorize"}),
            ),
            ("adminCase.legacyList", json!({})),
            ("adminCase.legacyGet", json!({"messageId":unknown_request})),
            (
                "adminCase.legacyPreview",
                json!({"messageId":unknown_request,"crewId":f.crew,"sessionId":sid}),
            ),
            (
                "adminCase.legacyConfirm",
                json!({"messageId":unknown_request,"crewId":f.crew,"sessionId":sid,
                    "requestId":unknown_request,"expectedPreviewRevision":"untrusted","note":"cannot self authorize"}),
            ),
            (
                "adminCase.legacyReceipt",
                json!({"requestId":unknown_request}),
            ),
            ("adminCase.similarRequirements", json!({"crewId":f.crew})),
            ("adminCase.requirementReviews", json!({"crewId":f.crew})),
        ] {
            assert_eq!(
                call(&mut agent, method, params).await.unwrap_err().0,
                crewd_proto::RpcError::FORBIDDEN,
                "{method} must retain authenticated Human access"
            );
        }
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn exact_requirement_review_uses_the_original_human_command_receipt() {
    use crewd_collab::{Sender, judgments, requirement_similarity as similarity, requirements};
    use crewd_proto::{TodoCreateParams, judgments::{JudgmentConfig, JudgmentSettingsSet}};
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let crew = f.crew.clone();
    let channel = format!("{crew}/lobby");
    let (ids, decision) = f.state.collab(move |tx| {
        let previous = judgments::configuration(tx.conn())?;
        judgments::set_configuration(tx, &Sender::Human, JudgmentSettingsSet {
            request_id: uuid::Uuid::new_v4().to_string(), credential_update: Default::default(),
            expected_revision: previous.revision, config: Some(JudgmentConfig {
                enabled: true, provider: "typesafe".into(), base_url: "https://example.invalid".into(),
                model: "fixture".into(), choice_min: 0.5, wake_yes: 0.5,
                credential_source: "file".into(), similar_requirements: Some(true),
            }),
        })?;
        let mut ids = Vec::new();
        for text in ["original first fixture requirement", "original second fixture requirement"] {
            let created = requirements::create(tx, &Sender::Human, TodoCreateParams {
                request_id: uuid::Uuid::new_v4().to_string(), crew_id: crew.clone(),
                channel_id: Some(channel.clone()), text: text.into(),
            })?;
            ids.push(created.todo.requirement_id.unwrap());
        }
        similarity::observe_page(tx)?;
        let (pending, _) = similarity::pending_page(tx, None)?;
        assert_eq!(pending.len(), 1);
        Ok((ids, pending[0].clone()))
    }).await.unwrap();
    let scored = f.state.collab(move |tx| similarity::dispatch(tx, &decision)).await.unwrap().unwrap();
    f.state.collab(move |tx| similarity::settle(tx, &scored, similarity::Outcome::Answered {
        probability: 0.95, suggested: true,
    })).await.unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let identity = call(&mut human, "system.hello", json!({})).await.unwrap()["runtimeIdentity"].clone();
    let scoped = json!({"crewId":f.crew});
    let rows = call(&mut human, "adminCase.similarRequirements", scoped.clone()).await.unwrap();
    let candidate = &rows["items"][0];
    assert_eq!(candidate["firstId"], ids[0]);
    assert_eq!(candidate["secondId"], ids[1]);
    let packet = json!({
        "requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],
        "runtimeBindingId":identity["runtimeBindingId"],
        "command":{"operation":"adminCase.requirementReview","params":{
            "crewId":f.crew,"firstId":ids[0],"secondId":ids[1],"expectedReviewRevision":candidate["reviewRevision"],
            "disposition":"group","candidateRevision":candidate["candidateRevision"],"note":"keep both originals"
        }}
    });
    let mut agent = RpcClient::connect(&socket).await;
    call(&mut agent, "agent.attach", json!({"sessionId":f.worker,"token":f.token(&f.worker)})).await.unwrap();
    assert_eq!(call(&mut agent, "command.submit", packet.clone()).await.unwrap_err().0, crewd_proto::RpcError::FORBIDDEN);
    let committed = call(&mut human, "command.submit", packet.clone()).await.unwrap();
    assert_eq!(committed["phase"], "committed");
    assert_eq!(committed["result"]["revision"], "1");
    assert_eq!(committed["result"]["disposition"], "group");
    assert_eq!(call(&mut human, "command.submit", packet.clone()).await.unwrap(), committed);
    let recovered = call(&mut human, "command.lookup", json!({
        "requestId":packet["requestId"],"dbId":identity["dbId"],
    })).await.unwrap();
    assert_eq!(recovered, committed);
    let mut changed = packet.clone();
    changed["command"]["params"]["note"] = json!("changed replay");
    assert_eq!(call(&mut human, "command.submit", changed).await.unwrap_err().0, crewd_proto::RpcError::CONFLICT);
    let reviews = call(&mut human, "adminCase.requirementReviews", scoped.clone()).await.unwrap();
    assert_eq!(reviews["items"], json!([{"review":committed["result"],"effective":true}]));
    assert_eq!(call(&mut human, "adminCase.similarRequirements", scoped).await.unwrap()["items"], json!([]));
    for id in &ids {
        let row = call(&mut human, "todo.get", json!({"id":id})).await.unwrap();
        assert_eq!(row["status"], "unclaimed");
    }
    let clear = json!({
        "requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],
        "runtimeBindingId":identity["runtimeBindingId"],
        "command":{"operation":"adminCase.requirementReview","params":{
            "crewId":f.crew,"firstId":ids[0],"secondId":ids[1],"expectedReviewRevision":"1",
            "disposition":"clear","candidateRevision":null,"note":" preserve exact note "
        }}
    });
    let cleared = call(&mut human, "command.submit", clear).await.unwrap();
    assert_eq!(cleared["result"]["revision"], "2");
    assert_eq!(cleared["result"]["note"], " preserve exact note ");
    assert!(cleared["result"].get("candidateRevision").is_none());
    assert!(cleared["command"]["params"].get("candidateRevision").is_none());
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
}

#[tokio::test]
async fn human_brief_socket_reads_the_original_question_without_mutating_it() {
    use crewd_collab::asks;
    let f = Fixture::new().await;
    let worker = f.worker.clone();
    let original = f.state.collab(move |tx| asks::open(tx, &worker, asks::OpenAsk {
        question: "original fixture question".into(), default: None, resume_note: None,
    })).await.unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let before = f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
    let params = json!({"crewId": f.crew});
    let brief = call(&mut human, "adminCase.humanBrief", params.clone()).await.unwrap();
    assert_eq!(brief["crewId"], f.crew);
    assert_eq!(brief["caseCoverage"], "complete");
    assert_eq!(brief["patrol"]["state"], "not_recorded");
    assert_eq!(brief["humanWaits"].as_array().unwrap().len(), 1);
    let row = &brief["humanWaits"][0];
    assert_eq!(row["kind"], "question");
    assert_eq!(row["askId"], original.ask);
    assert_eq!(row["question"], original.question);
    assert_eq!(row["sourceMessageId"], original.message_id.unwrap());
    assert_eq!(f.state.db.call(|s| s.conn().total_changes()).await.unwrap(), before);
    let crew = f.crew.clone();
    let id = original.ask;
    f.state.collab(move |tx| asks::answer(tx, &crew, &id, "confirmed")).await.unwrap();
    assert_eq!(call(&mut human, "adminCase.humanBrief", params).await.unwrap()["humanWaits"], json!([]));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
}

#[tokio::test]
async fn legacy_get_does_not_expose_recent_ordinary_messages_or_dms() {
    use crewd_collab::{Sender, channels, messages};
    use crewd_proto::MemberRef;

    let f = Fixture::new().await;
    f.state
        .collab(crewd_collab::admin_case_legacy::tick)
        .await
        .unwrap();
    let ordinary = f
        .human_post(
            &crewd_store::channels::lobby_id(&f.crew),
            "ordinary chat outside historical recovery",
            Vec::new(),
        )
        .await;
    let crew = f.crew.clone();
    let captain = f.captain.clone();
    let worker = f.worker.clone();
    let dm = f
        .state
        .collab(move |tx| {
            let (channel, _) = channels::ensure_dm(
                tx,
                &crew,
                &MemberRef::Session {
                    id: captain.clone(),
                },
                &MemberRef::Session { id: worker },
            )?;
            Ok(messages::post(
                tx,
                Sender::Session(captain),
                messages::NewPost {
                    channel_id: channel.id,
                    text: "direct message outside historical recovery".into(),
                    ..Default::default()
                },
            )?
            .message)
        })
        .await
        .unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let page = call(&mut human, "adminCase.legacyList", json!({}))
        .await
        .unwrap();
    assert!(page["scan"]["throughSeq"].as_i64().unwrap() < ordinary.seq);
    assert!(ordinary.seq < dm.seq);
    for message in [ordinary, dm] {
        assert_eq!(
            call(
                &mut human,
                "adminCase.legacyGet",
                json!({"messageId": message.id}),
            )
            .await
            .expect_err("history read must not expose unrelated message text")
            .0,
            crewd_proto::RpcError::NOT_FOUND
        );
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn committed_stop_escalation_round_trips_original_evidence_without_starting_execution() {
    let f = Fixture::new().await;
    f.management_crew().await;
    let crew = f.crew.clone();
    let worker = f.worker.clone();
    let work = f
        .state
        .collab(move |tx| {
            crewd_collab::sessions::set_desired(tx, &worker, crewd_proto::Desired::Running)?;
            crewd_collab::work::create(
                tx,
                &crewd_collab::Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: crew,
                    owner: Some(worker),
                    title: "case original missing schedule".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    for pass in 0..4 {
        let worker = f.worker.clone();
        f.state
            .collab(move |tx| {
                let c =
                    crewd_collab::stop_checks::candidate(tx.conn(), &worker, tx.now_ms())?.unwrap();
                let accepted = crewd_collab::stop_checks::accept(tx, &c)?;
                if pass < 3 {
                    let crewd_collab::stop_checks::Accepted::Rewake(id) = accepted else {
                        panic!("expected stop reminder")
                    };
                    // DAO acceptance of a simulated receipt; not a real provider or runner claim.
                    crewd_collab::wakes::claim(tx, &[id])?;
                    crewd_collab::wakes::ack_delivered(
                        tx,
                        &worker,
                        &[id],
                        crewd_collab::wakes::Riders::Release,
                        &[],
                        &[],
                    )?;
                } else {
                    assert_eq!(accepted, crewd_collab::stop_checks::Accepted::Escalated);
                }
                Ok(())
            })
            .await
            .unwrap();
    }
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let changes = f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
    let page = call(&mut human, "adminCase.list", json!({"crewId":f.crew}))
        .await
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let case = &page["items"][0];
    assert_eq!(case["source"]["sessionId"], f.worker);
    assert_eq!(case["targets"][0]["id"], work.id);
    assert_eq!(case["targets"][0]["revision"], work.revision);
    assert_eq!(case["targets"][0]["scope"], "in_scope");
    assert_eq!(case["decision"], "observed");
    assert_eq!(case["execution"], "not_started");
    assert_eq!(
        case.get("launchRecord"),
        Some(&Value::Null),
        "passive inspection exposes the scheduling field without preparing a launch"
    );
    assert_eq!(case["retirement"], "not_started");
    let detail = call(
        &mut human,
        "adminCase.get",
        json!({"crewId":f.crew,"id":case["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(&detail, case);
    assert!(
        call(
            &mut human,
            "adminCase.get",
            json!({"crewId":"nonexistent-crew","id":case["id"]})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &mut human,
            "adminCase.list",
            json!({"crewId":f.crew,"limit":0})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &mut human,
            "adminCase.list",
            json!({"crewId":f.crew,"after":"malformed"})
        )
        .await
        .is_err()
    );
    assert_eq!(
        changes,
        f.state.db.call(|s| s.conn().total_changes()).await.unwrap()
    );
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
}
