//! Actual local socket status readback; synthetic session, no executor or provider.
//! Provider invocation and real runner consumption need separate runtime fixtures.
use crewd_proto::{Crew, ServerFrame};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn retirement_status_is_read_only_and_rejects_foreign_crew() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    let mut ids = Vec::new();
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
        crewd_store::channels::ensure_lobby(store.conn(), crew, "2026-10-01T00:00:00.000Z")
            .unwrap();
        let (session, _) = crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            crewd_collab::sessions::register(
                tx,
                crewd_collab::sessions::NewSession {
                    crew_id: crew.into(),
                    role: crewd_proto::Role::Worker,
                    executor: "claude".into(),
                    title: Some(format!("session {crew}")),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        ids.push(session.id);
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
    let id = json!(ids[0]);
    let status = call(
        &mut wr,
        &mut lines,
        2,
        "session.retirementStatus",
        json!({"crewId":"a","sessionId":id}),
    )
    .await;
    assert_eq!(status["sessionId"], id);
    assert_eq!(status["crewId"], "a");
    assert!(
        status["record"].is_null(),
        "status reads must not invent a retirement decision"
    );
    // Trusted local Human can read both crews, but a forged object/crew pair is invalid.
    wr.write_all(
        format!(
            "{}\n",
            json!({"id":3,"method":"session.retirementStatus","params":{"crewId":"b","sessionId":id}})
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ServerFrame::Response(response) = serde_json::from_str(&response).unwrap() else {
        panic!("unexpected event")
    };
    assert!(
        response.into_result().is_err(),
        "foreign crew/object pair was accepted"
    );
    let after = call(
        &mut wr,
        &mut lines,
        4,
        "session.get",
        json!({"sessionId":id}),
    )
    .await;
    assert_eq!(after["id"], id);
    assert_eq!(after["title"], "session a");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Reopening uses the same persisted source identities and performs no writes.
    let reopened = crewd_store::Store::open(&path.join("crewd.db")).unwrap();
    let records: i64 = reopened
        .conn()
        .query_row(
            "SELECT count(*) FROM meta WHERE key LIKE 'session.retirement.v1:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        records, 0,
        "status reads must not create retirement records"
    );
}
