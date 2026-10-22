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
    let denied=rpc(&mut owner,"agent.call",json!({"tool":"channel_work_link","args":{"channel_id":a,"expected_link_revision":initial["linkRevision"],"object":{"kind":"work","id":foreign["id"]}}})).await;
    assert_eq!(
        denied["isError"], true,
        "group membership does not grant source crew authority"
    );
    let linked=rpc(&mut human,"channel.workLink",json!({"channelId":a,"expectedLinkRevision":initial["linkRevision"],"object":{"kind":"work","id":foreign["id"]}})).await;
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
    rpc(&mut human,"channel.workUnlink",json!({"channelId":a,"expectedLinkRevision":linked["linkRevision"],"object":{"kind":"work","id":foreign["id"]}})).await;
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
    // Public closed detail must remain readable after disappearing from pending lists.
    let wrong_crew = f.add_crew("wrong closed-question scope", None).await;
    let before = snapshot(&f).await;
    let closed = rpc(&mut human, "ask.get", json!({"crewId":f.crew,"ask":h})).await;
    assert_eq!(closed["ask"], h);
    assert_eq!(closed["crewId"], f.crew);
    assert_eq!(closed["state"], "answered");
    assert_eq!(closed["answer"], "actual human decision");
    assert!(closed["closedAt"].is_string());
    assert_eq!(snapshot(&f).await, before);
    assert_eq!(
        call(&mut human, "ask.get", json!({"crewId":wrong_crew,"ask":h}))
            .await
            .unwrap_err()
            .0,
        crewd_proto::RpcError::NOT_FOUND
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "wrong crew cannot turn a global H identifier into an unrelated-scope detail"
    );
    assert_eq!(
        call(&mut owner, "ask.get", json!({"crewId":f.crew,"ask":h}))
            .await
            .unwrap_err()
            .0,
        crewd_proto::RpcError::FORBIDDEN
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "attached session cannot borrow ordinary Human question detail"
    );
    close_socket(task, stop).await;
}

#[tokio::test]
async fn store17_group_knowledge_reference_uses_real_view_authority() {
    let f = Fixture::new().await;
    f.enable_knowledge();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut h = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let a = channel(&mut h, &f.crew, "knowledge A", &f.worker).await;
    let b = channel(&mut h, &f.crew, "knowledge B", &f.worker).await;
    let workspace = rpc(&mut h, "data.workspace.get", json!({"crewId":f.crew})).await;
    let saved=rpc(&mut h,"data.submit",json!({"context":workspace["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"apply","message":"save original knowledge","edits":[{"kind":"createView","alias":"page","title":"shared real View","definition":{"kind":"document","placements":[],"layout":{}},"attributes":{},"sources":[]}]}})).await;
    assert_eq!(saved["phase"], "committed");
    let source = json!({"context":saved["context"],"reference":saved["created"]["page"]["object"]["reference"]});
    let before = rpc(&mut h, "channel.knowledgeList", json!({"channelId":a})).await;
    let params = json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"expectedLinkRevision":before["linkRevision"],"source":source});
    f.state.db.call(|s|s.conn().execute_batch("CREATE TEMP TRIGGER fail_knowledge_association BEFORE INSERT ON channel_knowledge_links BEGIN SELECT RAISE(ABORT,'association SQL fault'); END;")).await.unwrap().unwrap();
    let core_before = snapshot(&f).await;
    assert!(
        call(&mut h, "channel.knowledgeLink", params.clone())
            .await
            .is_err()
    );
    assert_eq!(
        snapshot(&f).await,
        core_before,
        "failed association is a Core rollback, not a content transaction"
    );
    assert_eq!(
        rpc(
            &mut h,
            "resource.requestLookup",
            json!({"requestId":params["requestId"]})
        )
        .await["status"],
        "notFound"
    );
    assert_eq!(
        rpc(
            &mut h,
            "data.object.get",
            json!({"context":source["context"],"reference":source["reference"]})
        )
        .await["object"]["title"],
        "shared real View",
        "saved content survives association failure"
    );
    f.state
        .db
        .call(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER fail_knowledge_association;")
        })
        .await
        .unwrap()
        .unwrap();
    // Explicit fixture continuation after a known rolled-back refusal, not an automatic Unknown resend.
    let linked = rpc(&mut h, "channel.knowledgeLink", params.clone()).await;
    assert_eq!(linked["linked"], true);
    assert_eq!(
        rpc(&mut h, "channel.knowledgeLink", params.clone()).await,
        linked
    );
    let lookup = rpc(
        &mut h,
        "resource.requestLookup",
        json!({"requestId":params["requestId"]}),
    )
    .await;
    assert_eq!(lookup["result"], linked);
    let ap = rpc(&mut h, "channel.knowledgeList", json!({"channelId":a})).await;
    assert_eq!(ap["items"][0]["title"], "shared real View");
    assert_eq!(ap["items"][0]["state"], "ready");
    assert!(
        rpc(&mut h, "channel.knowledgeList", json!({"channelId":b})).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let original = rpc(
        &mut h,
        "data.object.get",
        json!({"context":ap["items"][0]["source"]["context"],"reference":source["reference"]}),
    )
    .await;
    assert_eq!(original["object"]["title"], "shared real View");
    let bp = rpc(&mut h, "channel.knowledgeList", json!({"channelId":b})).await;
    rpc(&mut h,"channel.knowledgeLink",json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":b,"expectedLinkRevision":bp["linkRevision"],"source":source})).await;
    let folder=rpc(&mut h,"viewFolder.create",json!({"crewId":f.crew,"channelId":a,"view":"documents","requestId":uuid::Uuid::new_v4().to_string(),"expectedTreeRevision":"0","name":"only A"})).await;
    let placed=rpc(&mut h,"viewFolder.assign",json!({"crewId":f.crew,"channelId":a,"view":"documents","expectedTreeRevision":folder["treeRevision"],"folderId":folder["folders"][0]["id"],"object":{"kind":"knowledgeView","id":source["reference"]["id"],"knowledge":source}})).await;
    assert_eq!(placed["assignments"].as_array().unwrap().len(), 1);
    assert!(
        rpc(
            &mut h,
            "viewFolder.list",
            json!({"crewId":f.crew,"channelId":b,"view":"documents"})
        )
        .await["folders"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let changed=rpc(&mut h,"data.submit",json!({"context":saved["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"apply","message":"edit one shared View","edits":[{"kind":"updateView","id":source["reference"]["id"],"expectedRevision":"1","title":"both groups same changed View","definition":{"kind":"document","placements":[],"layout":{}},"attributes":{}}]}})).await;
    assert_eq!(changed["phase"], "committed");
    for ch in [&a, &b] {
        let p = rpc(&mut h, "channel.knowledgeList", json!({"channelId":ch})).await;
        assert_eq!(p["items"][0]["title"], "both groups same changed View");
        assert_eq!(p["items"][0]["viewKind"], "document");
        assert_eq!(
            p["items"][0]["source"]["context"]["generation"],
            changed["context"]["generation"]
        );
    }
    close_socket(task, stop).await;
}
#[tokio::test]
async fn store17_channel_tags_are_real_local_references() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut h = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let a = channel(&mut h, &f.crew, "tags A", &f.worker).await;
    let t=rpc(&mut h,"todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"tagged"})).await;
    let p = page(&mut h, &a).await;
    let n = wakes(&f).await;
    let tags=rpc(&mut h,"channel.workTagsSet",json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"expectedLinkRevision":p["linkRevision"],"object":{"kind":"requirement","id":t["requirementId"]},"tags":[" 紧急 ","后端","紧急"]})).await;
    assert_eq!(tags["tags"], json!(["紧急", "后端"]));
    assert_eq!(wakes(&f).await, n);
    let b = channel(&mut h, &f.crew, "tags B", &f.worker).await;
    let bp = page(&mut h, &b).await;
    rpc(&mut h,"channel.workLink",json!({"channelId":b,"expectedLinkRevision":bp["linkRevision"],"object":{"kind":"requirement","id":t["requirementId"]}})).await;
    assert_eq!(page(&mut h, &b).await["items"][0]["tags"], json!([]));
    for i in 0..2 {
        let t=rpc(&mut h,"todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":format!("count source {i}")})).await;
        let p = page(&mut h, &a).await;
        rpc(&mut h,"channel.workTagsSet",json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"expectedLinkRevision":p["linkRevision"],"object":{"kind":"requirement","id":t["requirementId"]},"tags":["紧急"]})).await;
    }
    let p = rpc(
        &mut h,
        "channel.workList",
        json!({"channelId":a,"kinds":["requirement"],"tag":"紧急","limit":1}),
    )
    .await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    assert_eq!(p["total"], 3);
    assert_eq!(
        p["tagCounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["tag"] == "紧急")
            .unwrap()["count"],
        3
    );
    assert!(p["next"].is_string());
    let snap = snapshot(&f).await;
    assert!(
        call(
            &mut h,
            "channel.workList",
            json!({"channelId":a,"kinds":["humanTask"],"tag":"紧急","after":p["next"]})
        )
        .await
        .is_err()
    );
    assert_eq!(snapshot(&f).await, snap);
    close_socket(task, stop).await;
}
#[tokio::test]
async fn store17_human_task_creation_has_manual_origin_without_wake() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut h = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let a = channel(&mut h, &f.crew, "manual H", &f.worker).await;
    let n = wakes(&f).await;
    let t=rpc(&mut h,"humanTask.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"manual task"})).await;
    assert_eq!(t["todo"]["origin"], "humanTask");
    assert_eq!(t["todo"]["owner"], json!({"kind":"human"}));
    assert_eq!(wakes(&f).await, n);
    let snap = snapshot(&f).await;
    assert!(call(&mut h,"ask.answer",json!({"crewId":f.crew,"ask":t["ask"]["ask"],"text":"must not fabricate an Agent answer"})).await.is_err());
    assert_eq!(snapshot(&f).await, snap);
    let mut agent = attach(&f, &f.data_dir().join("crewd.sock"), &f.worker).await;
    assert!(call(&mut agent,"humanTask.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"Agent cannot borrow Human creation"})).await.is_err());
    assert_eq!(snapshot(&f).await, snap);
    close_socket(task, stop).await;
}

#[tokio::test]
async fn store17_global_todo_owner_manual_done_and_original_receipts_do_not_start_work() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut h = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let a = channel(&mut h, &f.crew, "facts A", &f.worker).await;
    let b = channel(&mut h, &f.crew, "facts B", &f.worker).await;
    let req=rpc(&mut h,"todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"existing Plan is retained"})).await;
    let object = json!({"kind":"requirement","id":req["requirementId"]});
    let bp = page(&mut h, &b).await;
    rpc(
        &mut h,
        "channel.workLink",
        json!({"channelId":b,"expectedLinkRevision":bp["linkRevision"],"object":object}),
    )
    .await;
    let existing=rpc(&mut h,"work.create",json!({"crewId":f.crew,"title":"original independent Plan","owner":f.worker,"sources":[req["requirementId"]]})).await;
    let before = wakes(&f).await;
    let mut observations = f.state.hub.subscribe_collab();
    let assigned=rpc(&mut h,"todo.assign",json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"object":object,"expectedRevision":"0","owner":{"kind":"human"}})).await;
    assert_eq!(
        page(&mut h, &b).await["items"][0]["todo"]["owner"],
        json!({"kind":"human"})
    );
    let params = json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"object":object,"expectedRevision":assigned["revision"],"state":"done"});
    let done = rpc(&mut h, "todo.setState", params.clone()).await;
    assert_eq!(
        rpc(
            &mut h,
            "resource.requestLookup",
            json!({"requestId":params["requestId"]})
        )
        .await["result"],
        done
    );
    assert_eq!(rpc(&mut h, "todo.setState", params.clone()).await, done);
    assert_eq!(
        rpc(&mut h, "todo.get", json!({"id":req["requirementId"]})).await["status"],
        "done"
    );
    assert_eq!(page(&mut h, &b).await["items"][0]["status"], "done");
    assert_eq!(
        rpc(&mut h, "work.get", json!({"id":existing["id"]})).await,
        existing,
        "source resolution does not alter or settle an existing Plan"
    );
    let snap = snapshot(&f).await;
    assert!(call(&mut h,"work.create",json!({"crewId":f.crew,"title":"must not claim resolved source","owner":f.worker,"sources":[req["requirementId"]]})).await.is_err());
    assert_eq!(snapshot(&f).await, snap);
    let reopened=rpc(&mut h,"todo.setState",json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":b,"object":object,"expectedRevision":done["revision"],"state":"open"})).await;
    assert_eq!(reopened["manualState"], "open");
    assert_eq!(wakes(&f).await, before);
    let mut attributes = 0;
    while let Ok(event) = observations.try_recv() {
        match event {
            crewd_collab::Change::TodoAttributesChanged { .. } => attributes += 1,
            crewd_collab::Change::WorkChanged { .. }
            | crewd_collab::Change::MessageAppended { .. }
            | crewd_collab::Change::AskAnswered { .. } => {
                panic!("metadata changes must not enter automatic routing or wake consumers")
            }
            _ => {}
        }
    }
    assert_eq!(attributes, 3);
    let mut mismatch = params;
    mismatch["state"] = json!("open");
    let snap = snapshot(&f).await;
    assert!(call(&mut h, "todo.setState", mismatch).await.is_err());
    assert_eq!(snapshot(&f).await, snap);
    close_socket(task, stop).await;
}

#[tokio::test]
async fn store17_mutation_receipt_fault_rolls_back_source_facts_number_tags_and_events() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let mut h = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
    let a = channel(&mut h, &f.crew, "atomic receipt", &f.worker).await;
    let req=rpc(&mut h,"todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"original source before injected failure"})).await;
    let object = json!({"kind":"requirement","id":req["requirementId"]});
    let page = page(&mut h, &a).await;
    f.state.db.call(|s|s.conn().execute_batch("CREATE TEMP TRIGGER deny_resource_receipt BEFORE INSERT ON resource_mutation_requests BEGIN SELECT RAISE(ABORT,'original receipt failure'); END;")).await.unwrap().unwrap();
    for (method, params) in [
        (
            "todo.assign",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"object":object,"expectedRevision":"0","owner":{"kind":"human"}}),
        ),
        (
            "todo.setState",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"object":object,"expectedRevision":"0","state":"done"}),
        ),
        (
            "humanTask.create",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"channelId":a,"text":"must not consume an H identity"}),
        ),
        (
            "channel.workTagsSet",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"channelId":a,"object":object,"expectedLinkRevision":page["linkRevision"],"tags":["紧急"]}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert!(call(&mut h, method, params.clone()).await.is_err());
        assert_eq!(
            snapshot(&f).await,
            before,
            "whole original transaction must roll back for {method}"
        );
        assert_eq!(
            rpc(
                &mut h,
                "resource.requestLookup",
                json!({"requestId":params["requestId"]})
            )
            .await["status"],
            "notFound"
        );
    }
    f.state
        .db
        .call(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER deny_resource_receipt;")
        })
        .await
        .unwrap()
        .unwrap();
    close_socket(task, stop).await;
}
