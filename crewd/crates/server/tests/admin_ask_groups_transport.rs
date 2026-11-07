//! Real owned Unix transport, with a genuine schema and no provider/executor.
#![cfg(feature = "testkit")]
use crewd_proto::{Role, RpcError, admin_ask_groups as w};
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;

async fn call(client: &mut RpcClient, method: &str, params: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(Duration::from_secs(10), client.call(method, params))
        .await
        .expect("bounded owned Unix response")
}

#[tokio::test]
async fn unavailable_runtime_is_not_advertised_reads_stay_passive_and_attached_agents_cannot_borrow_human_scope()
 {
    let f = Fixture::new().await;
    assert!(
        f.state
            .db
            .call_result(|s| crewd_store::admin_ask_groups::available(s.conn()))
            .await
            .unwrap()
            .unwrap(),
        "requires the genuine registered schema-33 chain"
    );
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let hello = call(&mut human, "system.hello", json!({})).await.unwrap();
    assert!(
        !hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == w::CAPABILITY),
        "protocol presence alone is not a live runtime"
    );
    let before = f.state.db.call(|s| s.conn().total_changes()).await.unwrap();
    let page = call(&mut human, w::LIST, json!({"crewId":f.crew}))
        .await
        .unwrap();
    assert_eq!(page["availability"], "unconfigured");
    assert_eq!(page["items"], json!([]));
    assert_eq!(
        before,
        f.state.db.call(|s| s.conn().total_changes()).await.unwrap(),
        "a Human read cannot initialize a scan or model request"
    );
    for role in [Role::Worker, Role::Captain, Role::Admin] {
        let id = if role == Role::Captain {
            f.captain.clone()
        } else {
            f.register(&f.crew, role, "typed reader").await
        };
        let mut agent = RpcClient::connect(&socket).await;
        call(
            &mut agent,
            "agent.attach",
            json!({"sessionId":id,"token":f.token(&id)}),
        )
        .await
        .unwrap();
        let denied = call(&mut agent, w::LIST, json!({"crewId":f.crew}))
            .await
            .unwrap_err();
        assert_eq!(denied.0, RpcError::FORBIDDEN);
        assert!(
            call(&mut agent, "events.subscribe", json!({"topics":[w::TOPIC]}))
                .await
                .is_err(),
            "Human-only topic is not an Agent permission"
        );
    }
    drop(human);
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
