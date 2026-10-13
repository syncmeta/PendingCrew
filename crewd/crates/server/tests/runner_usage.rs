//! Explicit Unix-socket usage readback; synthetic storage, no account runner.
use crewd_proto::{Crew, ServerFrame, Session};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn usage_socket_retains_exact_raw_json_and_js_safe_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    crewd_store::crews::upsert(
        store.conn(),
        &Crew {
            id: "c".into(),
            title: "synthetic".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    for id in ["s", "other"] {
        crewd_store::sessions::upsert(
            store.conn(),
            &Session {
                id: id.into(),
                crew_id: "c".into(),
                executor: "codex".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    store
        .conn()
        .execute(
            "INSERT INTO sqlite_sequence(name,seq) VALUES('runner_usage',9007199254740992)",
            [],
        )
        .unwrap();
    let first = r#"{"last":{"inputTokens":12},"total":{"totalTokens":9007199254740993},"future":{"kept":true}}"#;
    let second = r#"{"last":{"cacheWriteInputTokens":0},"total":{"totalTokens":9007199254740994}}"#;
    let tx = store.conn().unchecked_transaction().unwrap();
    for raw in [first, second] {
        crewd_store::runner_usage::append(
            &tx,
            "s",
            crewd_store::runner_usage::Snapshot {
                provider: "codex",
                thread_id: "thread",
                turn_id: None,
                token_usage_json: raw,
            },
            100,
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(store);
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let path = dir.path().to_owned();
    let server = tokio::spawn(crewd_server::serve(path.clone(), async {
        let _ = stopping.await;
    }));
    let socket = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(s) = UnixStream::connect(path.join("crewd.sock")).await {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (rd, mut wr) = socket.into_split();
    let mut lines = BufReader::new(rd).lines();
    async fn call(
        wr: &mut tokio::net::unix::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        id: u64,
        params: Value,
    ) -> Result<Value, crewd_proto::RpcError> {
        wr.write_all(
            format!(
                "{}\n",
                json!({"id":id,"method":"session.usage","params":params})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ServerFrame::Response(r) = serde_json::from_str(&line).unwrap() else {
            panic!("unexpected event");
        };
        assert_eq!(r.id, id);
        r.into_result()
    }
    let a = call(&mut wr, &mut lines, 1, json!({"sessionId":"s","limit":1}))
        .await
        .unwrap();
    assert_eq!(a["items"].as_array().unwrap().len(), 1);
    assert_eq!(a["items"][0]["id"], "9007199254740993");
    assert_eq!(a["nextAfter"], "9007199254740993");
    assert!(a["items"][0]["turnId"].is_null());
    let raw = a["items"][0]["tokenUsageJson"].as_str().unwrap();
    assert!(raw.contains("9007199254740993"));
    assert!(!raw.contains("cacheWriteInputTokens"));
    assert_eq!(
        serde_json::from_str::<Value>(raw).unwrap(),
        serde_json::from_str::<Value>(first).unwrap()
    );
    let b = call(
        &mut wr,
        &mut lines,
        2,
        json!({"sessionId":"s","limit":1,"after":a["nextAfter"]}),
    )
    .await
    .unwrap();
    assert!(b["nextAfter"].is_null());
    assert_eq!(b["items"][0]["id"], "9007199254740994");
    assert!(
        b["items"][0]["tokenUsageJson"]
            .as_str()
            .unwrap()
            .contains("\"cacheWriteInputTokens\":0")
    );
    let empty = call(&mut wr, &mut lines, 3, json!({"sessionId":"other"}))
        .await
        .unwrap();
    assert_eq!(empty["items"], json!([]));
    for (i, params) in [
        json!({"sessionId":"s","limit":0}),
        json!({"sessionId":"s","limit":201}),
        json!({"sessionId":"s","after":"01"}),
        json!({"sessionId":"s","after":9007199254740993u64}),
        json!({"sessionId":"s","after":"9223372036854775808"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            call(&mut wr, &mut lines, 10 + i as u64, params)
                .await
                .unwrap_err()
                .code,
            -32602
        );
    }
    assert_eq!(
        call(&mut wr, &mut lines, 20, json!({"sessionId":"missing"}))
            .await
            .unwrap_err()
            .code,
        -32004
    );
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
