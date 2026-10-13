//! Channel-local work references and immutable creation through actual authenticated Unix RPC.
//! All state is synthetic; no provider, app, or daily database is used.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;
const LIMIT: Duration = Duration::from_secs(10);
async fn call(c: &mut RpcClient, m: &str, p: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(LIMIT, c.call(m, p)).await.unwrap()
}
async fn rpc(c: &mut RpcClient, m: &str, p: Value) -> Value {
    call(c, m, p).await.unwrap()
}
async fn tool(c: &mut RpcClient, name: &str, args: Value) -> Value {
    let v = rpc(c, "agent.call", json!({"tool":name,"args":args})).await;
    assert_eq!(v["isError"], false, "{v}");
    let text = v["text"].as_str().unwrap();
    serde_json::from_str(&text[text.find('{').unwrap()..=text.rfind('}').unwrap()]).unwrap()
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
// Every durable fixture table is compared, including allocator and request-key records.
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
async fn close_socket(task: tokio::task::JoinHandle<()>, stop: tokio::sync::oneshot::Sender<()>) {
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

async fn channel(c: &mut RpcClient, crew: &str, name: &str, worker: &str) -> String {
    rpc(c,"channel.create",json!({"crewId":crew,"kind":"topic","name":name,"members":[{"kind":"session","id":worker}]})).await["id"].as_str().unwrap().to_string()
}
async fn page(c: &mut RpcClient, id: &str) -> Value {
    rpc(c, "channel.workList", json!({"channelId":id,"limit":100})).await
}
async fn wakes(f: &Fixture) -> i64 {
    f.state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get(0))
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn two_channels_project_one_work_lifecycle_and_unlinked_uuid_confirmation_never_relinks() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let a = channel(&mut human, &f.crew, "group A", &f.worker).await;
    let b = channel(&mut human, &f.crew, "group B", &f.worker).await;
    assert!(
        page(&mut human, &a).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let req_payload = json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"text":"immutable channel requirement","channelId":a});
    let requirement = rpc(&mut human, "todo.create", req_payload.clone()).await;
    assert_eq!(requirement["channelAssociation"]["channelId"], a);
    assert_eq!(requirement["channelAssociation"]["linked"], true);
    let work_payload = json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"title":"same W in two real groups","owner":f.worker,"sources":[requirement["requirementId"]]});
    let work = rpc(&mut human, "work.create", work_payload.clone()).await;
    assert_eq!(work["channelAssociation"]["linked"], true);
    assert!(
        page(&mut human, &b).await["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "group B never borrows crew-wide work list"
    );
    let empty = page(&mut human, &b).await;
    let count = wakes(&f).await;
    rpc(&mut human,"channel.workLink",json!({"channelId":b,"expectedLinkRevision":empty["linkRevision"],"object":{"kind":"work","id":work["id"]}})).await;
    assert_eq!(
        wakes(&f).await,
        count,
        "reference linking alone never wakes a model"
    );
    let done = tool(
        &mut owner,
        "work_item_update",
        json!({"id":work["id"],"expected_revision":work["revision"],"status":"done"}),
    )
    .await;
    assert_eq!(done["status"], "done");
    for group in [&a, &b] {
        let p = page(&mut human, group).await;
        let row = p["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["object"]["id"] == work["id"])
            .unwrap();
        assert_eq!(row["status"], "done");
        assert_eq!(row["revision"], done["revision"]);
    }
    assert_eq!(
        rpc(
            &mut human,
            "todo.get",
            json!({"id":requirement["requirementId"]})
        )
        .await["status"],
        "done",
        "requirement state derives from the sole source work authority"
    );
    for (payload, method, object) in [
        (
            work_payload,
            "work.create",
            json!({"kind":"work","id":work["id"]}),
        ),
        (
            req_payload,
            "todo.create",
            json!({"kind":"requirement","id":requirement["requirementId"]}),
        ),
    ] {
        let current = page(&mut human, &a).await;
        let count = wakes(&f).await;
        let removed = rpc(
            &mut human,
            "channel.workUnlink",
            json!({"channelId":a,"expectedLinkRevision":current["linkRevision"],"object":object}),
        )
        .await;
        assert_eq!(removed["linked"], false);
        assert_eq!(wakes(&f).await, count);
        let before = snapshot(&f).await;
        let retry = rpc(&mut human, method, payload).await;
        assert_eq!(
            retry["channelAssociation"]["linked"], false,
            "old immutable UUID confirmation must not restore a deliberately removed association"
        );
        assert_eq!(
            snapshot(&f).await,
            before,
            "confirmation returns current facts without replaying creation/link/events/wakes"
        );
        assert!(
            !page(&mut human, &a).await["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["object"] == object)
        );
    }
    assert_eq!(
        rpc(&mut human, "work.get", json!({"id":work["id"]})).await["status"],
        "done",
        "unbinding group references preserves contents/lifecycle"
    );
    close_socket(task, stop).await;
}

#[tokio::test]
async fn trusted_actor_visibility_and_both_cursor_revisions_are_enforced() {
    let f = Fixture::new().await;
    let foreign = f.add_crew("unreadable foreign group", None).await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let a = channel(&mut human, &f.crew, "visible current group", &f.worker).await;
    let hidden = rpc(
        &mut human,
        "channel.create",
        json!({"crewId":foreign,"kind":"topic","name":"never disclose private unrelated group"}),
    )
    .await;
    let w = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"cursor source item","owner":f.worker}),
    )
    .await;
    let another = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"second cursor item"}),
    )
    .await;
    let initial = page(&mut human, &a).await;
    let linked=tool(&mut owner,"channel_work_link",json!({"channel_id":a,"expected_link_revision":initial["linkRevision"],"object":{"kind":"work","id":w["id"]}})).await;
    tool(&mut owner,"channel_work_link",json!({"channel_id":a,"expected_link_revision":linked["linkRevision"],"object":{"kind":"work","id":another["id"]}})).await;
    let first = tool(
        &mut owner,
        "channel_work_list",
        json!({"channel_id":a,"limit":1}),
    )
    .await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert!(first["next"].is_string());
    tool(
        &mut owner,
        "work_item_update",
        json!({"id":w["id"],"expected_revision":w["revision"],"status":"done"}),
    )
    .await;
    let before = snapshot(&f).await;
    let stale = rpc(
        &mut owner,
        "agent.call",
        json!({"tool":"channel_work_list","args":{"channel_id":a,"after":first["next"],"limit":1}}),
    )
    .await;
    assert_eq!(
        stale["isError"], true,
        "source lifecycle revision invalidates a page even if link set did not change"
    );
    assert_eq!(snapshot(&f).await, before);
    for (method, params) in [
        ("channel.workList", json!({"channelId":a})),
        (
            "channel.workLink",
            json!({"channelId":a,"expectedLinkRevision":"0","object":{"kind":"work","id":w["id"]}}),
        ),
        (
            "channel.workUnlink",
            json!({"channelId":a,"expectedLinkRevision":"0","object":{"kind":"work","id":w["id"]}}),
        ),
        (
            "work.create",
            json!({"crewId":f.crew,"title":"cannot borrow Human group creation","requestId":uuid::Uuid::new_v4().to_string(),"channelId":a}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert_eq!(
            call(&mut owner, method, params).await.unwrap_err().0,
            crewd_proto::RpcError::FORBIDDEN
        );
        assert_eq!(snapshot(&f).await, before);
    }
    for (name, args) in [
        ("channel_work_list", json!({"channel_id":hidden["id"]})),
        (
            "channel_work_link",
            json!({"channel_id":hidden["id"],"expected_link_revision":"0","object":{"kind":"work","id":w["id"]}}),
        ),
    ] {
        let before = snapshot(&f).await;
        let r = rpc(&mut owner, "agent.call", json!({"tool":name,"args":args})).await;
        assert_eq!(r["isError"], true);
        assert_eq!(snapshot(&f).await, before);
    }
    let current = page(&mut human, &a).await;
    let before = snapshot(&f).await;
    assert_eq!(call(&mut human,"channel.workUnlink",json!({"channelId":a,"expectedLinkRevision":initial["linkRevision"],"object":{"kind":"work","id":another["id"]}})).await.unwrap_err().0,crewd_proto::RpcError::CONFLICT);
    assert_eq!(snapshot(&f).await, before);
    assert_ne!(current["sourceRevision"], first["sourceRevision"]);
    let fresh = tool(
        &mut owner,
        "channel_work_list",
        json!({"channel_id":a,"limit":1}),
    )
    .await;
    assert!(fresh["next"].is_string());
    rpc(&mut human,"channel.workUnlink",json!({"channelId":a,"expectedLinkRevision":current["linkRevision"],"object":{"kind":"work","id":another["id"]}})).await;
    let before = snapshot(&f).await;
    let stale = rpc(
        &mut owner,
        "agent.call",
        json!({"tool":"channel_work_list","args":{"channel_id":a,"after":fresh["next"],"limit":1}}),
    )
    .await;
    assert_eq!(
        stale["isError"], true,
        "a reference-set change independently expires the link revision cursor"
    );
    assert_eq!(snapshot(&f).await, before);
    close_socket(task, stop).await;
}

#[tokio::test]
async fn actual_foreign_source_reference_requires_explicit_unbind_before_purge_and_creation_is_atomic()
 {
    let f = Fixture::new().await;
    let source = f
        .add_crew("foreign source kept until explicit unlink", None)
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let a = channel(&mut human, &f.crew, "foreign reference group", &f.worker).await;
    let foreign = rpc(
        &mut human,
        "work.create",
        json!({"crewId":source,"title":"global readable source remains foreign-owned"}),
    )
    .await;
    let initial = page(&mut human, &a).await;
    let linked=tool(&mut owner,"channel_work_link",json!({"channel_id":a,"expected_link_revision":initial["linkRevision"],"object":{"kind":"work","id":foreign["id"]}})).await;
    assert_eq!(
        rpc(&mut human, "work.get", json!({"id":foreign["id"]})).await,
        foreign,
        "reference never changes source ownership/status"
    );
    rpc(&mut human, "crew.remove", json!({"crewId":source})).await;
    let before = snapshot(&f).await;
    assert_eq!(
        call(&mut human, "crew.purge", json!({"crewId":source}))
            .await
            .unwrap_err()
            .0,
        crewd_proto::RpcError::CONFLICT
    );
    assert_eq!(snapshot(&f).await, before);
    tool(&mut owner,"channel_work_unlink",json!({"channel_id":a,"expected_link_revision":linked["linkRevision"],"object":{"kind":"work","id":foreign["id"]}})).await;
    rpc(&mut human, "crew.purge", json!({"crewId":source})).await;
    assert!(
        page(&mut human, &a).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.state.db.call(|s|s.conn().execute_batch("CREATE TEMP TRIGGER fail_group_reference BEFORE INSERT ON channel_work_links BEGIN SELECT RAISE(ABORT,'real group link SQL fault'); END;")).await.unwrap().unwrap();
    for (method, payload) in [
        (
            "todo.create",
            json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"text":"must roll back full requirement"}),
        ),
        (
            "work.create",
            json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"title":"must roll back full plan"}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert!(call(&mut human, method, payload).await.is_err());
        assert_eq!(
            snapshot(&f).await,
            before,
            "real SQL link failure rolls back object/request witness/global IDs/events/wakes together"
        );
    }
    f.state
        .db
        .call(|s| s.conn().execute_batch("DROP TRIGGER fail_group_reference;"))
        .await
        .unwrap()
        .unwrap();
    close_socket(task, stop).await;
}

#[tokio::test]
async fn public_human_task_answer_is_shared_by_both_channel_projections() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let a = channel(&mut human, &f.crew, "actual H group A", &f.worker).await;
    let b = channel(&mut human, &f.crew, "actual H group B", &f.worker).await;
    let work = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"real public blocker","owner":f.worker}),
    )
    .await;
    let blocked=tool(&mut owner,"work_item_block",json!({"id":work["id"],"expected_revision":work["revision"],"on":[{"type":"new_human","question":"actual shared human ruling"}],"can_continue":{"type":"nothing"}})).await;
    let h = blocked["humanTasks"][0].clone();
    assert!(h.as_str().unwrap().starts_with('H'));
    for group in [&a, &b] {
        let initial = page(&mut human, group).await;
        let count = wakes(&f).await;
        rpc(&mut human,"channel.workLink",json!({"channelId":group,"expectedLinkRevision":initial["linkRevision"],"object":{"kind":"humanTask","id":h}})).await;
        assert_eq!(wakes(&f).await, count);
        let listed = page(&mut human, group).await;
        assert_eq!(listed["items"][0]["object"]["id"], h);
        assert_eq!(listed["items"][0]["status"], "open");
    }
    rpc(
        &mut human,
        "ask.answer",
        json!({"crewId":f.crew,"ask":h,"text":"actual human decision"}),
    )
    .await;
    for group in [&a, &b] {
        let listed = page(&mut human, group).await;
        assert_eq!(listed["items"][0]["object"]["id"], h);
        assert_eq!(
            listed["items"][0]["status"], "answered",
            "public answer updates both projections from one authoritative H"
        );
    }
    assert!(
        rpc(&mut human, "work.get", json!({"id":work["id"]})).await["blockedOn"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    close_socket(task, stop).await;
}
