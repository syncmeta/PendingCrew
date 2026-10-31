//! Archive permission acceptance through real owned Unix sockets.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;
const LIMIT: Duration = Duration::from_secs(10);
async fn call(c: &mut RpcClient, method: &str, params: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(LIMIT, c.call(method, params))
        .await
        .unwrap()
}
async fn rpc(c: &mut RpcClient, method: &str, params: Value) -> Value {
    call(c, method, params).await.unwrap()
}
async fn archived(f: &Fixture, id: &str) -> bool {
    let id = id.to_owned();
    f.state
        .db
        .call_result(move |s| {
            crewd_collab::channels::get(s.conn(), &id).map(|c| c.archived_at.is_some())
        })
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn attached_worker_cannot_archive_or_restore_through_human_rpc() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let channel = rpc(
        &mut human,
        "channel.create",
        json!({"crewId":f.crew,"kind":"topic","name":"Human owned review","members":[]}),
    )
    .await;
    let id = channel["id"].as_str().unwrap();
    let mut worker = RpcClient::connect(&socket).await;
    rpc(
        &mut worker,
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await;
    let tool = rpc(
        &mut worker,
        "agent.call",
        json!({"tool":"channel_archive","args":{"channel":id}}),
    )
    .await;
    assert_eq!(
        tool["isError"], true,
        "ordinary worker tool must retain its own identity: {tool}"
    );
    assert!(!archived(&f, id).await);
    let direct = call(
        &mut worker,
        "channel.update",
        json!({"channelId":id,"archived":true}),
    )
    .await;
    let changed = archived(&f, id).await;
    assert!(
        matches!(&direct,Err((code,_)) if *code==crewd_proto::RpcError::FORBIDDEN),
        "attached worker borrowed Human archive authority: reply={direct:?}; durable archived={changed}"
    );
    assert!(!changed);
    rpc(
        &mut human,
        "channel.update",
        json!({"channelId":id,"archived":true}),
    )
    .await;
    assert!(archived(&f, id).await);
    let direct = call(
        &mut worker,
        "channel.update",
        json!({"channelId":id,"archived":false}),
    )
    .await;
    assert!(
        matches!(&direct,Err((code,_)) if *code==crewd_proto::RpcError::FORBIDDEN),
        "attached worker unarchive: {direct:?}"
    );
    assert!(archived(&f, id).await);
    rpc(
        &mut human,
        "channel.update",
        json!({"channelId":id,"archived":false}),
    )
    .await;
    assert!(!archived(&f, id).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

async fn ready_topic(c: &mut RpcClient, f: &Fixture) -> String {
    let ch = rpc(
        c,
        "channel.create",
        json!({"crewId":f.crew,"kind":"topic","name":"completed work archive","members":[]}),
    )
    .await;
    let id = ch["id"].as_str().unwrap().to_owned();
    let work=rpc(c,"work.create",json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":id,"title":"synthetic completed work"})).await;
    rpc(c,"work.close",json!({"id":work["id"],"expectedRevision":work["revision"],"kind":"done","evidence":{"kind":"text","value":"independently verified fixture"},"note":"factual close for archive acceptance"})).await;
    id
}
async fn check(c: &mut RpcClient, id: &str) -> Value {
    rpc(c, "channel.archiveCheck", json!({"channelId":id})).await
}
fn archive_args(id: &str, v: &Value) -> Value {
    json!({"channelId":id,"expectedRevision":v["revision"],"evidence":"reviewed complete linked work","note":"archive the completed topic"})
}
#[tokio::test]
async fn verified_archive_requires_facts_and_cas_and_manual_restore_prevents_aba() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let id = ready_topic(&mut human, &f).await;
    let first = check(&mut human, &id).await;
    assert_eq!(
        first["canArchive"], true,
        "complete linked work fixture: {first}"
    );
    assert_eq!(first["facts"]["linkedWork"], 1);
    assert_eq!(first["facts"]["liveWork"], 0);
    for field in ["evidence", "note"] {
        let mut bad = archive_args(&id, &first);
        bad[field] = json!(" \n");
        assert!(
            call(&mut human, "channel.archiveVerified", bad)
                .await
                .is_err()
        );
        assert_eq!(check(&mut human, &id).await, first);
    }
    let done = rpc(
        &mut human,
        "channel.archiveVerified",
        archive_args(&id, &first),
    )
    .await;
    assert!(archived(&f, &id).await);
    assert_eq!(done["lastAction"]["action"], "archive");
    assert_eq!(done["canRestore"], true);
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &first)
        )
        .await
        .is_err()
    );
    assert_eq!(check(&mut human, &id).await, done);
    rpc(
        &mut human,
        "channel.update",
        json!({"channelId":id,"archived":false}),
    )
    .await;
    let restored = check(&mut human, &id).await;
    assert_ne!(
        restored["revision"], first["revision"],
        "legacy restore must not resurrect stale archive cut"
    );
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &first)
        )
        .await
        .is_err()
    );
    assert!(!archived(&f, &id).await);
    // A newly linked live item invalidates a previously good archive decision.
    rpc(&mut human,"work.create",json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":id,"title":"new activity must block archive"})).await;
    let live = check(&mut human, &id).await;
    assert_eq!(live["canArchive"], false);
    assert_eq!(live["facts"]["liveWork"], 1);
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &restored)
        )
        .await
        .is_err()
    );
    assert!(!archived(&f, &id).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
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
async fn attached_worker_cannot_borrow_any_human_channel_mutation() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let ch=rpc(&mut human,"channel.create",json!({"crewId":f.crew,"kind":"topic","name":"mutation boundary","members":[{"kind":"session","id":f.worker}]})).await;
    let id = ch["id"].as_str().unwrap();
    let mut worker = RpcClient::connect(&socket).await;
    rpc(
        &mut worker,
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await;
    let cases = [
        (
            "channel.create",
            json!({"crewId":f.crew,"kind":"topic","name":"unauthorized Human creation"}),
        ),
        (
            "channel.update",
            json!({"channelId":id,"purpose":"unauthorized rewrite"}),
        ),
        (
            "channel.addMembers",
            json!({"channelId":id,"members":[{"kind":"session","id":f.captain}]}),
        ),
        (
            "channel.setOwner",
            json!({"channelId":id,"owner":{"kind":"session","id":f.worker}}),
        ),
        (
            "channel.removeMember",
            json!({"channelId":id,"member":{"kind":"session","id":f.captain}}),
        ),
        ("channel.watch", json!({"channelId":id,"on":true})),
    ];
    let mut failures = vec![];
    for (method, args) in cases {
        let before = snapshot(&f).await;
        let reply = call(&mut worker, method, args).await;
        let unchanged = before == snapshot(&f).await;
        if !matches!(&reply,Err((code,_)) if *code==crewd_proto::RpcError::FORBIDDEN) || !unchanged
        {
            failures.push(format!(
                "{method}: reply={reply:?}; durable unchanged={unchanged}"
            ));
        }
    }
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[tokio::test]
async fn original_worker_tools_can_create_and_archive_own_topic() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut worker = RpcClient::connect(&socket).await;
    rpc(
        &mut worker,
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await;
    let made = rpc(
        &mut worker,
        "agent.call",
        json!({"tool":"channel_create","args":{"name":"legitimate owner tool","members":[]}}),
    )
    .await;
    assert_eq!(made["isError"], false, "{made}");
    let id = f
        .state
        .db
        .call_result(|s| {
            s.conn()
                .query_row(
                    "SELECT id FROM channels WHERE name='legitimate owner tool'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .map_err(crewd_store::StoreError::from)
        })
        .await
        .unwrap()
        .unwrap();
    let result = rpc(
        &mut worker,
        "agent.call",
        json!({"tool":"channel_archive","args":{"channel":id}}),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");
    assert!(archived(&f, &id).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

#[tokio::test]
async fn verified_archive_transaction_failure_preserves_all_facts_and_current_token() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let id = ready_topic(&mut human, &f).await;
    let first = check(&mut human, &id).await;
    assert_eq!(first["canArchive"], true, "{first}");
    f.state.db.call_result(|s|s.conn().execute_batch("CREATE TRIGGER archive_review_abort BEFORE UPDATE OF archived_at ON channels BEGIN SELECT RAISE(ABORT,'archive acceptance forced failure'); END;").map_err(crewd_store::StoreError::from)).await.unwrap().unwrap();
    let before = snapshot(&f).await;
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &first)
        )
        .await
        .is_err()
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "no partial lifecycle revision, audit or archive on SQL failure"
    );
    assert_eq!(check(&mut human, &id).await, first);
    f.state
        .db
        .call_result(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER archive_review_abort;")
                .map_err(crewd_store::StoreError::from)
        })
        .await
        .unwrap()
        .unwrap();
    rpc(
        &mut human,
        "channel.archiveVerified",
        archive_args(&id, &first),
    )
    .await;
    assert!(archived(&f, &id).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}
#[tokio::test]
async fn restored_database_epoch_rejects_old_archive_and_restore_tokens() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let id = ready_topic(&mut human, &f).await;
    let first = check(&mut human, &id).await;
    // Invoke the real restore epoch DAO; this is not a full restore command acceptance.
    f.state
        .db
        .call_result(|s| crewd_store::judgment_state::restored(s.conn()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &first)
        )
        .await
        .is_err()
    );
    assert!(!archived(&f, &id).await);
    let current = check(&mut human, &id).await;
    let archived_view = rpc(
        &mut human,
        "channel.archiveVerified",
        archive_args(&id, &current),
    )
    .await;
    f.state
        .db
        .call_result(|s| crewd_store::judgment_state::restored(s.conn()))
        .await
        .unwrap()
        .unwrap();
    assert!(call(&mut human,"channel.restoreVerified",json!({"channelId":id,"expectedRevision":archived_view["revision"],"note":"authorized reviewed restore"})).await.is_err());
    assert!(archived(&f, &id).await);
    let current = check(&mut human, &id).await;
    let restored=rpc(&mut human,"channel.restoreVerified",json!({"channelId":id,"expectedRevision":current["revision"],"note":"authorized reviewed restore"})).await;
    assert!(!archived(&f, &id).await);
    assert_eq!(restored["lastAction"]["action"], "restore");
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

#[tokio::test]
async fn new_message_invalidates_preflight_and_two_simultaneous_archives_have_one_winner() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready_topic(&mut human, &f).await;
    let before = check(&mut human, &id).await;
    assert_eq!(before["canArchive"], true, "{before}");
    rpc(
        &mut human,
        "message.post",
        json!({"channelId":id,"text":"new activity after preflight"}),
    )
    .await;
    let after = check(&mut human, &id).await;
    assert_ne!(before["revision"], after["revision"]);
    assert!(
        call(
            &mut human,
            "channel.archiveVerified",
            archive_args(&id, &before)
        )
        .await
        .is_err()
    );
    assert!(!archived(&f, &id).await);
    let other = ready_topic(&mut human, &f).await;
    let cut = check(&mut human, &other).await;
    assert_eq!(cut["canArchive"], true, "{cut}");
    let mut peer = RpcClient::connect(&socket).await;
    let args = archive_args(&other, &cut);
    let (one, two) = tokio::join!(
        call(&mut human, "channel.archiveVerified", args.clone()),
        call(&mut peer, "channel.archiveVerified", args)
    );
    assert_eq!(
        usize::from(one.is_ok()) + usize::from(two.is_ok()),
        1,
        "exactly one CAS writer: {one:?}; {two:?}"
    );
    let rejected = if one.is_err() { one } else { two };
    assert!(matches!(rejected,Err((code,_)) if code==crewd_proto::RpcError::CONFLICT));
    assert!(archived(&f, &other).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

#[tokio::test]
async fn corrupt_version_one_reservation_is_not_silently_treated_as_no_active_delivery() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let id = ready_topic(&mut human, &f).await;
    assert_eq!(check(&mut human, &id).await["canArchive"], true);
    let key = format!("cursor.reservation.v1.{}", f.worker);
    let damaged=json!({"version":1,"sessionId":f.worker,"incarnation":uuid::Uuid::new_v4().to_string(),"revision":1,"active":"corrupt-but-valid-json"}).to_string();
    f.state
        .db
        .call_result(move |s| crewd_store::meta_set(s.conn(), &key, &damaged))
        .await
        .unwrap()
        .unwrap();
    let before = snapshot(&f).await;
    let reply = call(&mut human, "channel.archiveCheck", json!({"channelId":id})).await;
    assert!(
        !matches!(&reply,Ok(v) if v["canArchive"]==true),
        "corrupt active reservation was ignored by preflight: {reply:?}"
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "inspection must preserve corrupt metadata for diagnosis"
    );
    assert!(!archived(&f, &id).await);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}
