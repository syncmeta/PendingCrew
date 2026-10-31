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
    let changes = f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
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
