//! Verified archive agent acceptance through real owned Unix sockets.
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
async fn attach(f: &Fixture, socket: &std::path::Path, id: &str) -> RpcClient {
    let mut c = RpcClient::connect(socket).await;
    rpc(
        &mut c,
        "agent.attach",
        json!({"sessionId":id,"token":f.token(id)}),
    )
    .await;
    c
}
async fn tool(c: &mut RpcClient, name: &str, args: Value) -> Value {
    rpc(c, "agent.call", json!({"tool":name,"args":args})).await
}
async fn ok_tool(c: &mut RpcClient, name: &str, args: Value) -> Value {
    let result = tool(c, name, args).await;
    assert_eq!(result["isError"], false, "{name}: {result}");
    serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
}
async fn check(c: &mut RpcClient, id: &str) -> Value {
    ok_tool(c, "channel_archive_check", json!({"channel":id})).await
}
fn archive_args(id: &str, v: &Value) -> Value {
    json!({"channel":id,"expected_revision":v["revision"],"evidence":"verified fixture work complete","note":"reviewed archive"})
}
fn restore_args(id: &str, v: &Value) -> Value {
    json!({"channel":id,"expected_revision":v["revision"],"note":"reviewed restore"})
}
async fn ready(c: &mut RpcClient, f: &Fixture, owner: &str) -> String {
    let ch=rpc(c,"channel.create",json!({"crewId":f.crew,"kind":"topic","name":"agent verified fixture","members":[{"kind":"session","id":owner}]})).await;
    let id = ch["id"].as_str().unwrap().to_owned();
    rpc(
        c,
        "channel.setOwner",
        json!({"channelId":id,"owner":{"kind":"session","id":owner}}),
    )
    .await;
    let work=rpc(c,"work.create",json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":id,"title":"fixture completed source"})).await;
    rpc(c,"work.close",json!({"id":work["id"],"expectedRevision":work["revision"],"kind":"done","evidence":{"kind":"text","value":"reviewed output"},"note":"factual completion"})).await;
    id
}
async fn denied(f: &Fixture, c: &mut RpcClient, name: &str, args: Value) {
    let before = snapshot(f).await;
    let result = tool(c, name, args).await;
    assert_eq!(result["isError"], true, "must reject {name}: {result}");
    assert_eq!(
        snapshot(f).await,
        before,
        "refusal must leave every durable table unchanged: {name}"
    );
}
async fn finish(stop: tokio::sync::oneshot::Sender<()>, task: tokio::task::JoinHandle<()>) {
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}
#[tokio::test]
async fn owner_and_captain_can_verify_archive_and_restore_exact_archived_id() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, &f.worker).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let mut captain = attach(&f, &socket, &f.captain).await;
    for client in [&mut owner, &mut captain] {
        let cut = check(client, &id).await;
        assert_eq!(cut["canArchive"], true, "{cut}");
        let done = ok_tool(client, "channel_archive_verified", archive_args(&id, &cut)).await;
        assert!(archived(&f, &id).await);
        let reread = check(client, &id).await;
        assert_eq!(reread["revision"], done["revision"]);
        assert_eq!(reread["canRestore"], true);
        let restored = ok_tool(
            client,
            "channel_restore_verified",
            restore_args(&id, &reread),
        )
        .await;
        assert!(!archived(&f, &id).await);
        assert_eq!(restored["lastAction"]["action"], "restore");
    }
    // Existing manual archive tool keeps its deliberately simpler contract.
    let manual = tool(&mut owner, "channel_archive", json!({"channel":id})).await;
    assert_eq!(manual["isError"], false, "{manual}");
    assert!(archived(&f, &id).await);
    finish(stop, task).await;
}
#[tokio::test]
async fn nonowner_worker_and_admin_have_read_facts_but_no_archive_authority() {
    let f = Fixture::new().await;
    f.add_session(&f.crew, "s-peer", crewd_proto::Role::Worker, "peer")
        .await;
    f.add_session(&f.crew, "s-admin", crewd_proto::Role::Admin, "admin")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, &f.worker).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let owner_cut = check(&mut owner, &id).await;
    for sid in ["s-peer", "s-admin"] {
        let mut client = attach(&f, &socket, sid).await;
        let cut = check(&mut client, &id).await;
        assert_eq!(
            cut["canArchive"], false,
            "readability must not grant management: {cut}"
        );
        denied(
            &f,
            &mut client,
            "channel_archive_verified",
            archive_args(&id, &cut),
        )
        .await;
        denied(
            &f,
            &mut client,
            "channel_archive_verified",
            archive_args(&id, &owner_cut),
        )
        .await;
    }
    let done = ok_tool(
        &mut owner,
        "channel_archive_verified",
        archive_args(&id, &owner_cut),
    )
    .await;
    for sid in ["s-peer", "s-admin"] {
        let mut client = attach(&f, &socket, sid).await;
        let cut = check(&mut client, &id).await;
        assert_eq!(cut["canRestore"], false);
        denied(
            &f,
            &mut client,
            "channel_restore_verified",
            restore_args(&id, &done),
        )
        .await;
    }
    finish(stop, task).await;
}
#[tokio::test]
async fn cross_crew_owner_keeps_original_right_without_granting_its_captain() {
    let f = Fixture::new().await;
    let foreign = f.add_crew("foreign", None).await;
    f.add_session(
        &foreign,
        "foreign-owner",
        crewd_proto::Role::Worker,
        "foreign owner",
    )
    .await;
    f.add_session(
        &foreign,
        "foreign-cap",
        crewd_proto::Role::Captain,
        "foreign captain",
    )
    .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, "foreign-owner").await;
    let mut owner = attach(&f, &socket, "foreign-owner").await;
    let mut cap = attach(&f, &socket, "foreign-cap").await;
    let cut = check(&mut owner, &id).await;
    assert_eq!(cut["canArchive"], true);
    let foreign_cut = check(&mut cap, &id).await;
    assert_eq!(foreign_cut["canArchive"], false);
    denied(
        &f,
        &mut cap,
        "channel_archive_verified",
        archive_args(&id, &cut),
    )
    .await;
    let done = ok_tool(
        &mut owner,
        "channel_archive_verified",
        archive_args(&id, &cut),
    )
    .await;
    assert_eq!(done["lastAction"]["actor"], "foreign-owner");
    ok_tool(
        &mut owner,
        "channel_restore_verified",
        restore_args(&id, &done),
    )
    .await;
    assert!(!archived(&f, &id).await);
    finish(stop, task).await;
}
#[tokio::test]
async fn late_activity_owner_change_and_stale_cas_do_not_mutate_channel() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, &f.worker).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let old = check(&mut owner, &id).await;
    rpc(
        &mut human,
        "message.post",
        json!({"channelId":id,"text":"new archive review activity"}),
    )
    .await;
    denied(
        &f,
        &mut owner,
        "channel_archive_verified",
        archive_args(&id, &old),
    )
    .await;
    let fresh = check(&mut owner, &id).await;
    rpc(
        &mut human,
        "channel.setOwner",
        json!({"channelId":id,"owner":{"kind":"human"}}),
    )
    .await;
    denied(
        &f,
        &mut owner,
        "channel_archive_verified",
        archive_args(&id, &fresh),
    )
    .await;
    assert!(!archived(&f, &id).await);
    finish(stop, task).await;
}
#[tokio::test]
async fn removed_callers_crew_cannot_use_existing_cross_crew_ownership() {
    let f = Fixture::new().await;
    let foreign = f.add_crew("later removed owner crew", None).await;
    f.add_session(
        &foreign,
        "removed-owner",
        crewd_proto::Role::Worker,
        "owner",
    )
    .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, "removed-owner").await;
    let mut owner = attach(&f, &socket, "removed-owner").await;
    let cut = check(&mut owner, &id).await;
    assert_eq!(cut["canArchive"], true);
    rpc(&mut human, "crew.remove", json!({"crewId":foreign})).await;
    assert!(!archived(&f, &id).await, "target crew remains active");
    for (name, args) in [
        ("channel_archive_check", json!({"channel":id})),
        ("channel_archive_verified", archive_args(&id, &cut)),
    ] {
        let before = snapshot(&f).await;
        let result = call(&mut owner, "agent.call", json!({"tool":name,"args":args})).await;
        assert!(
            !matches!(&result,Ok(v) if v["isError"]==false),
            "removed caller may not execute: {result:?}"
        );
        assert_eq!(snapshot(&f).await, before);
    }
    assert!(!archived(&f, &id).await);
    finish(stop, task).await;
}

#[tokio::test]
async fn token_revoked_after_tool_preparation_is_rechecked_before_archive_transaction() {
    use crewd_server::agent::testkit::CallerStage;
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let id = ready(&mut human, &f, &f.worker).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let cut = check(&mut owner, &id).await;
    assert_eq!(cut["canArchive"], true);
    let mut gate = f.caller_gate(&f.worker, CallerStage::ArchivePrepared);
    let args = archive_args(&id, &cut);
    let request = tokio::spawn(async move {
        let response = call(
            &mut owner,
            "agent.call",
            json!({"tool":"channel_archive_verified","args":args}),
        )
        .await;
        (owner, response)
    });
    let evidence = tokio::time::timeout(LIMIT, gate.entered()).await.unwrap();
    assert_eq!(
        evidence.text, id,
        "pause must belong to this actual prepared archive call"
    );
    f.state.tokens.revoke(&f.worker);
    let before = snapshot(&f).await;
    gate.release();
    let (mut owner, response) = tokio::time::timeout(LIMIT, request).await.unwrap().unwrap();
    assert!(
        !matches!(&response,Ok(v) if v["isError"]==false),
        "revoked prepared call must not commit: {response:?}"
    );
    assert_eq!(snapshot(&f).await, before);
    assert!(!archived(&f, &id).await);
    // The same already-attached socket keeps its revoked token. Even resolution
    // failure must not reveal readable channel names or identifiers.
    let denied_check = tool(
        &mut owner,
        "channel_archive_check",
        json!({"channel":"nonexistent-after-token-revocation"}),
    )
    .await;
    assert_eq!(denied_check["isError"], true, "{denied_check}");
    let text = denied_check["text"].as_str().unwrap();
    assert!(
        !text.contains("agent verified fixture") && !text.contains(&id),
        "revoked resolver leaked choices: {text}"
    );
    assert_eq!(snapshot(&f).await, before);
    finish(stop, server).await;
}
