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

#[tokio::test]
async fn full_report_block_assignment_checkpoint_contract_matrix_has_only_domain_refusals_and_zero_effects()
 {
    use crewd_proto::{Role, RpcError};
    let f = Fixture::new().await;
    f.add_session(&f.crew, "matrix-other", Role::Worker, "other")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let create =
        |title: &str, owner: Option<&str>| json!({"crewId":f.crew,"title":title,"owner":owner});
    let unowned = rpc(&mut human, "work.create", create("unassigned", None)).await;
    let owned = rpc(
        &mut human,
        "work.create",
        create("assigned", Some(&f.worker)),
    )
    .await;
    let blocker = rpc(
        &mut human,
        "work.create",
        create("dependency", Some(&f.worker)),
    )
    .await;
    let dependant = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"depends","owner":f.worker,"dependsOn":[blocker["id"]]}),
    )
    .await;
    let blockable = rpc(
        &mut human,
        "work.create",
        create("blockable", Some(&f.worker)),
    )
    .await;
    let blocked=rpc(&mut human,"work.block",json!({"id":blockable["id"],"expectedRevision":blockable["revision"],"on":[{"type":"new_human","question":"legitimate pending question"}],"canContinue":{"type":"nothing"}})).await["item"].clone();
    let closed = rpc(&mut human, "work.create", create("closed", Some(&f.worker))).await;
    let closed = rpc(
        &mut human,
        "work.update",
        json!({"id":closed["id"],"expectedRevision":closed["revision"],"status":"done"}),
    )
    .await;
    let parent = rpc(
        &mut human,
        "work.create",
        create("live parent", Some(&f.worker)),
    )
    .await;
    let child = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"live child","owner":f.worker,"parent":parent["id"]}),
    )
    .await;
    let now = crewd_store::now_ms();
    let future = crewd_store::ts_from_ms(now + 60 * 60 * 1000);
    let far = crewd_store::ts_from_ms(now + 3 * 60 * 60 * 1000);
    let past = crewd_store::ts_from_ms(now - 1000);
    let update = |item: &Value, status: Option<&str>, check: Option<&str>| json!({"id":item["id"],"expectedRevision":item["revision"],"status":status,"note":"valid report","nextCheckAt":check});
    let blocked_on = |item: &Value, target: &Value| json!({"id":item["id"],"expectedRevision":item["revision"],"on":[{"type":"work","id":target["id"]}],"canContinue":{"type":"nothing"}});
    let cases = vec![
        (
            "ownerless active",
            "work.update",
            update(&unowned, Some("active"), Some(&future)),
            RpcError::CONFLICT,
            "work.owner_required",
        ),
        (
            "ownerless open check",
            "work.update",
            update(&unowned, Some("open"), Some(&future)),
            RpcError::CONFLICT,
            "work.owner_required",
        ),
        (
            "ownerless retained open check",
            "work.update",
            update(&unowned, None, Some(&future)),
            RpcError::CONFLICT,
            "work.owner_required",
        ),
        (
            "assigned active without check",
            "work.update",
            update(&owned, Some("active"), None),
            RpcError::INVALID_PARAMS,
            "work.check_required",
        ),
        (
            "past check",
            "work.update",
            update(&owned, Some("open"), Some(&past)),
            RpcError::INVALID_PARAMS,
            "work.check_not_future",
        ),
        (
            "beyond horizon",
            "work.update",
            update(&owned, Some("open"), Some(&far)),
            RpcError::INVALID_PARAMS,
            "work.check_too_far",
        ),
        (
            "blocked note with check",
            "work.update",
            update(&blocked, None, Some(&future)),
            RpcError::INVALID_PARAMS,
            "work.check_not_allowed",
        ),
        (
            "pending prerequisites",
            "work.update",
            update(&dependant, Some("active"), Some(&future)),
            RpcError::CONFLICT,
            "work.dependencies_pending",
        ),
        (
            "terminal report",
            "work.update",
            update(&closed, Some("active"), Some(&future)),
            RpcError::CONFLICT,
            "work.terminal",
        ),
        (
            "terminal assign",
            "work.assign",
            json!({"id":closed["id"],"expectedRevision":closed["revision"],"owner":"matrix-other","note":"valid assignment"}),
            RpcError::CONFLICT,
            "work.terminal",
        ),
        (
            "same owner assign",
            "work.assign",
            json!({"id":owned["id"],"expectedRevision":owned["revision"],"owner":f.worker,"note":"valid assignment"}),
            RpcError::CONFLICT,
            "work.owner_unchanged",
        ),
        (
            "stale revision",
            "work.update",
            json!({"id":owned["id"],"expectedRevision":"0","status":"open","note":"valid report"}),
            RpcError::CONFLICT,
            "work.revision_conflict",
        ),
        (
            "self blocker",
            "work.block",
            blocked_on(&owned, &owned),
            RpcError::INVALID_PARAMS,
            "work.relation_invalid",
        ),
        (
            "cycle blocker",
            "work.block",
            blocked_on(&child, &parent),
            RpcError::INVALID_PARAMS,
            "work.relation_invalid",
        ),
        (
            "close live parent",
            "work.update",
            json!({"id":parent["id"],"expectedRevision":parent["revision"],"status":"done"}),
            RpcError::CONFLICT,
            "work.dependents_active",
        ),
        (
            "drop live parent",
            "work.update",
            json!({"id":parent["id"],"expectedRevision":parent["revision"],"status":"dropped","note":"explicit drop"}),
            RpcError::CONFLICT,
            "work.dependents_active",
        ),
    ];
    for (label, method, params, code, refusal) in cases {
        let before = snapshot(&f).await;
        let error = refused(&socket, method, params).await;
        assert_eq!(error["code"], code, "{label}: {error}");
        assert_eq!(error["data"]["refusal"], refusal, "{label}: {error}");
        assert_eq!(snapshot(&f).await, before, "{label}: durable side effects");
    }
    // Explicit owner authorization is still checked before input-state refusals.
    let mut other = RpcClient::connect(&socket).await;
    rpc(
        &mut other,
        "agent.attach",
        json!({"sessionId":"matrix-other","token":f.token("matrix-other")}),
    )
    .await;
    let before = snapshot(&f).await;
    let denied = other
        .call("work.update", update(&owned, Some("active"), None))
        .await
        .unwrap_err();
    assert_eq!(
        denied.0,
        RpcError::INVALID_PARAMS,
        "attached agents cannot use Human-only public RPC: {denied:?}"
    );
    assert_eq!(snapshot(&f).await, before);
    let denied=rpc(&mut other,"agent.call",json!({"tool":"work_item_update","args":{"id":owned["id"],"expected_revision":owned["revision"],"status":"active","note":"valid report"}})).await;
    assert_eq!(
        denied["isError"], true,
        "non-owner actual agent tool must refuse: {denied}"
    );
    assert!(!denied["text"].as_str().unwrap().contains("内部错误"));
    assert_eq!(snapshot(&f).await, before);
    let before = snapshot(&f).await;
    let missing=human.call("work.assign",json!({"id":owned["id"],"expectedRevision":owned["revision"],"owner":"no-such-session","note":"valid assignment"})).await.unwrap_err();
    assert_eq!(missing.0, RpcError::NOT_FOUND);
    assert_eq!(snapshot(&f).await, before);
    // Positive combinations are preserved; Open is not globally prohibited from checks.
    let open = rpc(
        &mut human,
        "work.update",
        update(&owned, Some("open"), Some(&future)),
    )
    .await;
    assert_eq!(open["status"], "open");
    assert_eq!(open["nextCheckAt"], future);
    let active = rpc(
        &mut human,
        "work.update",
        update(&open, Some("active"), None),
    )
    .await;
    assert_eq!(active["status"], "active");
    assert_eq!(active["nextCheckAt"], future);
    let active = rpc(&mut human, "work.update", update(&active, None, None)).await;
    assert_eq!(active["nextCheckAt"], future);
    let blocked_note = rpc(&mut human, "work.update", update(&blocked, None, None)).await;
    assert_eq!(blocked_note["status"], "blocked");
    assert!(blocked_note["nextCheckAt"].is_null());
    let reopened = rpc(
        &mut human,
        "work.update",
        update(&blocked_note, Some("open"), None),
    )
    .await;
    assert_eq!(reopened["status"], "open");
    assert!(reopened["blockedOn"].as_array().unwrap().is_empty());
    let before_assign = crewd_store::now_ms();
    let moved=rpc(&mut human,"work.assign",json!({"id":active["id"],"expectedRevision":active["revision"],"owner":"matrix-other","note":"explicit human handoff"})).await;
    let after_assign = crewd_store::now_ms();
    let check = chrono::DateTime::parse_from_rfc3339(moved["nextCheckAt"].as_str().unwrap())
        .unwrap()
        .timestamp_millis();
    assert!((before_assign..=after_assign).contains(&check));
    assert_eq!(moved["owner"], "matrix-other");
    assert_eq!(moved["status"], "active");
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
