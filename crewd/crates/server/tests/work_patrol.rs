//! Actual Unix-socket projection, with synthetic data and no executor/model.
use crewd_proto::{Crew, ServerFrame, WorkCreateParams};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn patrol_socket_has_bounded_cursor_and_work_readback() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    for (i, crew) in ["a", "b"].into_iter().enumerate() {
        crewd_store::crews::upsert(
            store.conn(),
            &Crew {
                id: crew.into(),
                title: crew.into(),
                number: Some(i as i64 + 1),
                created_at: "2026-10-01T00:00:00.000Z".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            crewd_collab::work::create(
                tx,
                &crewd_collab::Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: format!("work {crew}"),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    }
    drop(store);
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let path = dir.path().to_owned();
    let server = tokio::spawn(crewd_server::serve(path.clone(), async {
        let _ = stopping.await;
    }));
    let stream = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(stream) = UnixStream::connect(path.join("crewd.sock")).await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    async fn call(
        wr: &mut tokio::net::unix::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        wr.write_all(format!("{}\n", json!({"id":id,"method":method,"params":params})).as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ServerFrame::Response(response) = serde_json::from_str(&line).unwrap() else {
            panic!("unexpected event");
        };
        assert_eq!(response.id, id);
        response.into_result().unwrap()
    }
    let first = call(&mut wr, &mut lines, 1, "work.patrol", json!({"limit":1})).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["unowned"], true);
    assert_eq!(first["liveOnly"], true);
    assert_eq!(first["includeRemoved"], false);
    let id = first["items"][0]["id"].clone();
    assert_eq!(first["nextAfter"], id);
    let detail = call(&mut wr, &mut lines, 2, "work.get", json!({"id":id})).await;
    assert_eq!(detail["crewId"], "a");
    let second = call(
        &mut wr,
        &mut lines,
        3,
        "work.patrol",
        json!({"limit":1,"after":first["nextAfter"]}),
    )
    .await;
    assert_eq!(second["items"][0]["crewId"], "b");
    assert_ne!(second["items"][0]["id"], first["items"][0]["id"]);
    assert!(second["nextAfter"].is_null());
    let empty = call(
        &mut wr,
        &mut lines,
        4,
        "work.patrol",
        json!({"after":second["items"][0]["id"]}),
    )
    .await;
    assert_eq!(empty["items"], json!([]));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
