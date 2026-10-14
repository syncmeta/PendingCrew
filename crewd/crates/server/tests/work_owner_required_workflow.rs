//! Regression for the actual unassigned-plan window report/block failure.
//! Uses the genuine public socket and compares every durable table, including
//! allocator/events/asks/wakes/request keys; no provider/model/real DB is used.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;
async fn rpc(c: &mut RpcClient, method: &str, params: Value) -> Value {
    tokio::time::timeout(Duration::from_secs(5), c.call(method, params))
        .await
        .unwrap()
        .unwrap()
}
async fn refused(socket: &std::path::Path, method: &str, params: Value) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let (rd, mut wr) = stream.into_split();
    let mut rd = tokio::io::BufReader::new(rd);
    let mut bytes = serde_json::to_vec(&json!({"id":1,"method":method,"params":params})).unwrap();
    bytes.push(b'\n');
    wr.write_all(&bytes).await.unwrap();
    loop {
        let mut line = String::new();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), rd.read_line(&mut line))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["id"] == 1 {
            return frame["error"].clone();
        }
    }
}
async fn snapshot(f: &Fixture) -> Vec<String> {
    f.state
        .db
        .call(|s| {
            let mut q = s
                .conn()
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let tables = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut out = Vec::new();
            for name in tables {
                assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
                let mut stmt = s.conn().prepare(&format!("SELECT * FROM \"{name}\""))?;
                let count = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        let mut v = Vec::new();
                        for i in 0..count {
                            v.push(format!("{:?}", r.get_ref(i)?));
                        }
                        Ok(v.join("|"))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut rows = rows;
                rows.sort();
                out.push(name);
                out.extend(rows);
            }
            Ok::<_, crewd_store::StoreError>(out)
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn unassigned_report_and_block_are_typed_conflicts_with_zero_effects_then_assign_allows_progress()
 {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let made = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"actual unassigned window work"}),
    )
    .await;
    assert!(made["owner"].is_null());
    let future = crewd_store::ts_from_ms(crewd_store::now_ms() + 60 * 60 * 1000);
    let before = snapshot(&f).await;
    for (method, params) in [
        (
            "work.update",
            json!({"id":made["id"],"expectedRevision":made["revision"],"status":"active","note":"valid original progress","nextCheckAt":future}),
        ),
        (
            "work.block",
            json!({"id":made["id"],"expectedRevision":made["revision"],"on":[{"type":"new_human","question":"must not allocate an ask"}],"canContinue":{"type":"split","items":[{"crewId":f.crew,"title":"must not allocate a child"}]}}),
        ),
    ] {
        let error = refused(&socket, method, params).await;
        assert_eq!(error["code"], crewd_proto::RpcError::CONFLICT, "{error}");
        assert_eq!(error["data"]["refusal"], "work.owner_required", "{error}");
        assert!(!error["message"].as_str().unwrap().contains("内部"));
        assert_eq!(
            snapshot(&f).await,
            before,
            "rejected {method} changed durable state"
        );
    }
    let opened=rpc(&mut human,"work.update",json!({"id":made["id"],"expectedRevision":made["revision"],"status":"open","note":"unassigned progress may remain open"})).await;
    assert!(opened["owner"].is_null());
    assert_eq!(opened["status"], "open");
    let assigned=rpc(&mut human,"work.assign",json!({"id":opened["id"],"expectedRevision":opened["revision"],"owner":f.worker,"note":"explicit human assignment"})).await;
    assert_eq!(assigned["owner"], f.worker);
    let active=rpc(&mut human,"work.update",json!({"id":assigned["id"],"expectedRevision":assigned["revision"],"status":"active","note":"actual progress after assignment","nextCheckAt":future})).await;
    assert_eq!(active["status"], "active");
    let blocked=rpc(&mut human,"work.block",json!({"id":active["id"],"expectedRevision":active["revision"],"on":[{"type":"new_human","question":"legitimate assigned blocker"}],"canContinue":{"type":"nothing"}})).await;
    assert_eq!(blocked["item"]["status"], "blocked");
    assert_eq!(blocked["humanTasks"].as_array().unwrap().len(), 1);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || f.state.db.clone().shutdown()),
    )
    .await
    .unwrap()
    .unwrap();
}
