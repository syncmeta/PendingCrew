//! Public frozen-Todo regression; synthetic data, real attached socket, no real runner.
use crewd_collab::{
    Locale,
    refusal::{Code, Refusal},
};
use crewd_proto::MentionKind;
use crewd_server::{
    AppState,
    agent::{
        testkit::{Fixture, RpcClient},
        text,
    },
};
use serde_json::{Value, json};

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

async fn run(st: &AppState, who: &str, name: &str, args: Value) -> crewd_proto::agent::CallResult {
    let me = crewd_server::agent::port::identity(st, who)
        .await
        .unwrap()
        .unwrap();
    crewd_server::agent::tools::call(st, &me, name, args).await
}

#[tokio::test]
async fn implicit_and_explicit_human_mentions_share_one_reservation() {
    for mode in ["implicit", "explicit", "mixed"] {
        let f = Fixture::new().await;
        let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        let h = f
            .human_post(&lobby.id, "synthetic human question", vec![])
            .await;
        for i in 0..4 {
            let mut a = json!({"message":format!("{mode}-answer-{i}")});
            if mode != "explicit" {
                a["reply_to"] = json!(h.seq);
            } else {
                a["channel"] = json!(lobby.id);
            }
            if mode == "explicit" || (mode == "mixed" && i % 2 == 1) {
                a["mentions"] = json!([{"kind":"human"}]);
            }
            let r = run(&f.state, &f.worker, "post", a).await;
            assert_eq!(r.is_error, i == 3, "{mode} {i}: {}", r.text);
            if i == 3 {
                assert_eq!(
                    r.text,
                    crewd_server::agent::text::r(Locale::Zh, "r.human_mention_throttled", &[])
                );
            }
        }
        let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
            .await
            .unwrap();
        let sent: Vec<_> = rows
            .iter()
            .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
            .collect();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|m| {
            m.mentions
                .iter()
                .filter(|a| a.kind == MentionKind::Human)
                .count()
                == 1
        }));
    }
}

#[tokio::test]
async fn implicit_human_reservation_refunds_failed_first_write_but_not_partial_success() {
    for partial in [false, true] {
        let f = Fixture::new().await;
        let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        let h = f.human_post(&lobby.id, "synthetic human", vec![]).await;
        let huge = "x".repeat(300 * 1024);
        let args = if partial {
            json!({"reply_to":h.seq,"messages":[{"text":"partial first"},{"text":huge},{"text":"never sent"}]})
        } else {
            json!({"reply_to":h.seq,"message":huge})
        };
        assert!(run(&f.state, &f.worker, "post", args).await.is_error);
        for i in 0..(3 - usize::from(partial)) {
            let args = json!({"reply_to":h.seq,"message":format!("valid-{i}")});
            let r = run(&f.state, &f.worker, "post", args.clone()).await;
            assert!(!r.is_error, "{partial}/{i}: {}", r.text);
            if i == 0 {
                let duplicate = run(&f.state, &f.worker, "post", args).await;
                assert!(duplicate.is_error, "duplicate unexpectedly persisted");
            }
        }
        let r = run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"fourth human ping"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.human_mention_throttled", &[])
        );
        let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn concurrent_implicit_human_mentions_are_serialized_before_reservation() {
    let f = Fixture::new().await;
    let lobby = crewd_server::agent::port::lobby_of(&f.state, &f.crew)
        .await
        .unwrap();
    let h = f
        .human_post(&lobby.id, "synthetic concurrent question", vec![])
        .await;
    let (a, b, c, d) = tokio::join!(
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-a"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-b"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-c"})
        ),
        run(
            &f.state,
            &f.worker,
            "post",
            json!({"reply_to":h.seq,"message":"concurrent-d"})
        ),
    );
    assert_eq!([a, b, c, d].iter().filter(|r| r.is_error).count(), 1);
    let rows = crewd_server::agent::port::messages_raw(&f.state, &lobby.id, None, 20)
        .await
        .unwrap();
    let replies: Vec<_> = rows
        .iter()
        .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
        .collect();
    assert_eq!(replies.len(), 3);
    assert!(replies.iter().all(|m| {
        m.mentions
            .iter()
            .filter(|x| x.kind == MentionKind::Human)
            .count()
            == 1
    }));
}
