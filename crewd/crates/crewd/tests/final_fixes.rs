//! Public frozen-Todo regression; synthetic data, real attached socket, no real runner.
use crewd_collab::{
    Locale,
    refusal::{Code, Refusal},
};
use crewd_server::agent::{
    testkit::{Fixture, RpcClient},
    text,
};
use serde_json::json;

#[tokio::test]
async fn frozen_todo_write_is_refused_on_an_already_attached_connection() {
    let f = Fixture::new().await;
    f.add_agent_todo(1, "synthetic frozen todo").await;
    let mut rpc = RpcClient::connect(&f.spawn_socket().await).await;
    rpc.call(
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await
    .unwrap();
    let active = rpc
        .call(
            "agent.call",
            json!({"tool":"respond_todo","args":{"number":1,"response":"active response"}}),
        )
        .await
        .unwrap();
    assert_eq!(active["isError"], false);
    f.rpc("crew.remove", json!({"crewId":f.crew})).await;
    let before = f
        .state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT updated_at FROM todos WHERE number=1", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    let v = rpc
        .call(
            "agent.call",
            json!({"tool":"respond_todo","args":{"number":1,"response":"after freeze"}}),
        )
        .await
        .unwrap();
    let r: crewd_proto::agent::CallResult = serde_json::from_value(v).unwrap();
    assert!(r.is_error, "frozen Todo write was accepted: {}", r.text);
    let refusal = Refusal::new(Code::CrewRemoved)
        .arg("crew", f.crew.as_str())
        .text(Locale::Zh);
    assert_eq!(
        r.text,
        text::r(Locale::Zh, "r.failed", &[("detail", &refusal)])
    );
    let after = f
        .state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT updated_at FROM todos WHERE number=1", [], |r| {
                    r.get::<_, Option<String>>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    let count: i64 = f
        .state
        .db
        .call(|s| {
            s.conn().query_row(
                "SELECT COUNT(*) FROM todo_responses WHERE text='after freeze'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 0);
    let read = rpc
        .call(
            "agent.call",
            json!({"tool":"read_channel","args":{"channel":format!("{}/lobby",f.crew)}}),
        )
        .await
        .unwrap();
    assert_eq!(
        read["isError"], false,
        "freeze must not remove valid read access"
    );
}
