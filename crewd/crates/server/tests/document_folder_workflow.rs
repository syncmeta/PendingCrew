//! Documents and four independent folder views through actual authenticated Unix RPC.
//! All state is synthetic; no provider, app, or daily database is used.
#![cfg(feature = "testkit")]
use crewd_proto::Role;
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

#[tokio::test]
async fn immutable_document_request_survives_collaborative_edits_and_attached_reads_never_borrow_human()
 {
    let f = Fixture::new().await;
    let foreign_crew = f.add_crew("foreign document crew", None).await;
    f.add_session(
        &foreign_crew,
        "foreign-document-worker",
        Role::Worker,
        "foreign",
    )
    .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut member = attach(&f, &socket, &f.worker).await;
    let mut foreign = attach(&f, &socket, "foreign-document-worker").await;
    let request = uuid::Uuid::new_v4().to_string();
    let body = format!("HEADER {}", "🌙".repeat(5000));
    let params =
        json!({"requestId":request,"crewId":f.crew,"title":"immutable original","body":body});
    let made = rpc(&mut human, "document.create", params.clone()).await;
    assert_eq!(made["reused"], false);
    assert_eq!(made["requestId"], request);
    let id = made["document"]["id"].as_str().unwrap().to_owned();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert!(made["document"].get("body").is_none());
    let detail = rpc(&mut human, "document.get", json!({"id":id})).await;
    assert_eq!(detail["body"], body);
    assert_eq!(detail["bodyLength"], body.len());
    let page = tool(
        &mut member,
        "document_get",
        json!({"id":id,"offset":0,"limit":16384}),
    )
    .await;
    let chunk = page["bodyChunk"].as_str().unwrap();
    assert!(chunk.len() <= 16384);
    assert!(body.starts_with(chunk));
    assert!(page.get("body").is_none());
    assert!(page["document"].get("body").is_none());
    let next = page["nextOffset"].as_u64().unwrap();
    assert_eq!(next as usize, chunk.len());
    let bad=rpc(&mut member,"agent.call",json!({"tool":"document_get","args":{"id":id,"offset":8,"limit":10,"expected_revision":detail["revision"]}})).await;
    assert_eq!(
        bad["isError"], true,
        "offset inside UTF8 scalar must refuse"
    );
    for (method, input) in [
        ("document.get", json!({"id":id})),
        ("document.list", json!({"crewId":f.crew})),
        ("document.create", params.clone()),
        (
            "document.update",
            json!({"id":id,"expectedRevision":detail["revision"],"title":"bypass","body":"bad"}),
        ),
        (
            "document.delete",
            json!({"id":id,"expectedRevision":detail["revision"]}),
        ),
        (
            "viewFolder.list",
            json!({"crewId":f.crew,"view":"documents"}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert_eq!(
            call(&mut member, method, input).await.unwrap_err().0,
            crewd_proto::RpcError::FORBIDDEN
        );
        assert_eq!(snapshot(&f).await, before);
    }
    let denied = rpc(
        &mut foreign,
        "agent.call",
        json!({"tool":"document_get","args":{"id":id}}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    let changed=tool(&mut member,"document_update",json!({"id":id,"expected_revision":detail["revision"],"title":"collaboratively edited","body":"exact updated text"})).await;
    assert!(
        !changed.to_string().contains(&body),
        "mutation must not feed back old full body"
    );
    let before_patch = rpc(&mut human, "document.get", json!({"id":id})).await;
    assert_eq!(before_patch["title"], "collaboratively edited");
    assert_eq!(before_patch["body"], "exact updated text");
    let patch=tool(&mut member,"document_patch",json!({"id":id,"expected_revision":before_patch["revision"],"edits":[{"start":6,"end":13,"text":"patched"}]})).await;
    assert!(
        patch.get("body").is_none(),
        "small patch never echoes the full document"
    );
    let current = rpc(&mut human, "document.get", json!({"id":id})).await;
    assert_eq!(current["body"], "exact patched text");
    let patch_before = snapshot(&f).await;
    for input in [
        json!({"id":id,"expected_revision":current["revision"],"edits":[{"start":0,"end":2,"text":"a"},{"start":1,"end":3,"text":"b"}]}),
        json!({"id":id,"expected_revision":before_patch["revision"],"edits":[{"start":0,"end":1,"text":"a"}]}),
    ] {
        let rejected = rpc(
            &mut member,
            "agent.call",
            json!({"tool":"document_patch","args":input}),
        )
        .await;
        assert_eq!(rejected["isError"], true);
        assert_eq!(
            snapshot(&f).await,
            patch_before,
            "overlap/stale patch must not alter body/revision/audit/IDs/wakes"
        );
    }
    let before = snapshot(&f).await;
    let replay = rpc(&mut human, "document.create", params.clone()).await;
    assert_eq!(replay["document"]["id"], id);
    assert_eq!(replay["reused"], true);
    assert_eq!(replay["document"]["revision"], current["revision"]);
    assert_eq!(replay["document"]["title"], current["title"]);
    assert_eq!(
        snapshot(&f).await,
        before,
        "retry checks immutable request witness, no duplicate event/revision"
    );
    let mut mismatch = params;
    mismatch["body"] = json!("changed original payload");
    assert_eq!(
        call(&mut human, "document.create", mismatch)
            .await
            .unwrap_err()
            .0,
        crewd_proto::RpcError::CONFLICT
    );
    assert_eq!(snapshot(&f).await, before);
    let stale=rpc(&mut member,"agent.call",json!({"tool":"document_get","args":{"id":id,"offset":next,"expected_revision":detail["revision"]}})).await;
    assert_eq!(
        stale["isError"], true,
        "later chunk cannot mix body revisions"
    );
    let list = rpc(&mut human, "document.list", json!({"crewId":f.crew})).await;
    assert!(list["items"][0].get("body").is_none());
    rpc(
        &mut human,
        "document.delete",
        json!({"id":id,"expectedRevision":current["revision"]}),
    )
    .await;
    assert!(
        rpc(&mut human, "document.list", json!({"crewId":f.crew})).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        rpc(&mut human, "document.get", json!({"id":id})).await["body"],
        "exact patched text",
        "soft deletion preserves audit content"
    );
    close_socket(task, stop).await;
}

#[tokio::test]
async fn folder_compact_pagination_and_delete_promotion_leave_the_same_work_graph_unchanged() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut member = attach(&f, &socket, &f.worker).await;
    let plan = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"same plan classified independently","owner":f.worker}),
    )
    .await;
    let document = rpc(&mut human,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":f.crew,"title":"classified document","body":"retained actual document"})).await;
    let channel = rpc(&mut human,"channel.create",json!({"crewId":f.crew,"kind":"topic","name":"classified message topic","members":[{"kind":"session","id":f.worker}]})).await;
    for view in ["todo", "progress", "documents", "messages"] {
        let object = match view {
            "documents" => json!({"kind":"document","id":document["document"]["id"]}),
            "messages" => json!({"kind":"channel","id":channel["id"]}),
            _ => json!({"kind":"work","id":plan["id"]}),
        };
        let initial = rpc(
            &mut human,
            "viewFolder.list",
            json!({"crewId":f.crew,"view":view}),
        )
        .await;
        let root=rpc(&mut human,"viewFolder.create",json!({"crewId":f.crew,"view":view,"requestId":uuid::Uuid::new_v4().to_string(),"expectedTreeRevision":initial["treeRevision"],"name":"parent"})).await;
        let parent = root["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "parent")
            .unwrap()["id"]
            .clone();
        let child=rpc(&mut human,"viewFolder.create",json!({"crewId":f.crew,"view":view,"requestId":uuid::Uuid::new_v4().to_string(),"expectedTreeRevision":root["treeRevision"],"name":"child","parentId":parent})).await;
        let child_id = child["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "child")
            .unwrap()["id"]
            .clone();
        let one = tool(
            &mut member,
            "view_folder_list",
            json!({"crew_id":f.crew,"view":view,"part":"folders","limit":1}),
        )
        .await;
        assert_eq!(one["items"].as_array().unwrap().len(), 1);
        let cursor = one["next"].clone();
        assert!(cursor.is_string());
        assert!(one.get("assignments").is_none());
        let assigned=tool(&mut member,"view_folder_assign",json!({"crew_id":f.crew,"view":view,"expected_tree_revision":child["treeRevision"],"object":object,"folder_id":parent})).await;
        assert!(
            assigned.get("folders").is_none() && assigned.get("assignments").is_none(),
            "agent mutation receipt must stay compact"
        );
        let stale=rpc(&mut member,"agent.call",json!({"tool":"view_folder_list","args":{"crew_id":f.crew,"view":view,"part":"folders","after":cursor,"limit":1}})).await;
        assert_eq!(
            stale["isError"], true,
            "cursor binds original tree revision"
        );
        let current = rpc(
            &mut human,
            "viewFolder.list",
            json!({"crewId":f.crew,"view":view}),
        )
        .await;
        let deleted=rpc(&mut human,"viewFolder.delete",json!({"crewId":f.crew,"view":view,"id":parent,"expectedTreeRevision":current["treeRevision"]})).await;
        assert!(deleted["assignments"].as_array().unwrap().is_empty());
        let promoted = deleted["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == child_id)
            .unwrap();
        assert!(promoted["parentId"].is_null());
        assert_eq!(
            rpc(&mut human, "work.get", json!({"id":plan["id"]})).await,
            plan,
            "classification never mutates plan graph/revision/status/owner"
        );
    }
    close_socket(task, stop).await;
}

// DM creation uses the existing authoritative channel Tx command. The later
// membership mutation is deliberately a Store-layer fact fixture because the
// current public protocol forbids leaving DMs. This proves live public read ACL,
// not a new UI/public-DM-leave operation.
#[tokio::test]
async fn persisted_dm_membership_changes_filter_real_folder_refs_without_hidden_counts() {
    let f = Fixture::new().await;
    let b = f.add_crew("foreign DM owner", None).await;
    for id in ["dm-source", "dm-other"] {
        f.add_session(&b, id, Role::Worker, id).await;
    }
    let crew = b.clone();
    let peer = f.worker.clone();
    let (visible, hidden) = f
        .state
        .db
        .call_result(move |s| {
            let source = crewd_proto::MemberRef::Session {
                id: "dm-source".into(),
            };
            let visible = crewd_collab::run(s, crewd_store::now_ms(), |tx| {
                crewd_collab::channels::ensure_dm(
                    tx,
                    &crew,
                    &source,
                    &crewd_proto::MemberRef::Session { id: peer },
                )
            })?
            .0
            .0
            .id;
            let hidden = crewd_collab::run(s, crewd_store::now_ms(), |tx| {
                crewd_collab::channels::ensure_dm(
                    tx,
                    &crew,
                    &source,
                    &crewd_proto::MemberRef::Session {
                        id: "dm-other".into(),
                    },
                )
            })?
            .0
            .0
            .id;
            Ok::<_, crewd_collab::CollabError>((visible, hidden))
        })
        .await
        .unwrap()
        .unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut member = attach(&f, &socket, &f.worker).await;
    let initial = rpc(
        &mut human,
        "viewFolder.list",
        json!({"crewId":f.crew,"view":"messages"}),
    )
    .await;
    let created=rpc(&mut human,"viewFolder.create",json!({"crewId":f.crew,"view":"messages","requestId":uuid::Uuid::new_v4().to_string(),"expectedTreeRevision":initial["treeRevision"],"name":"manually authored neutral folder"})).await;
    let folder = created["folders"][0]["id"].clone();
    let assigned=tool(&mut member,"view_folder_assign",json!({"crew_id":f.crew,"view":"messages","expected_tree_revision":created["treeRevision"],"object":{"kind":"channel","id":visible},"folder_id":folder})).await;
    let refs = tool(
        &mut member,
        "view_folder_list",
        json!({"crew_id":f.crew,"view":"messages","part":"assignments","limit":100}),
    )
    .await;
    assert_eq!(refs["items"].as_array().unwrap().len(), 1);
    assert_eq!(refs["items"][0]["object"]["id"], visible);
    let before = snapshot(&f).await;
    let denied=rpc(&mut member,"agent.call",json!({"tool":"view_folder_assign","args":{"crew_id":f.crew,"view":"messages","expected_tree_revision":assigned["treeRevision"],"object":{"kind":"channel","id":hidden},"folder_id":folder}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    let id = visible.clone();
    let reader = f.worker.clone();
    f.state.db.call_result(move |s|{
        let tx=s.conn().unchecked_transaction()?;
        assert!(crewd_collab::channels::readable_by(&tx,&id,&reader)?);
        assert_eq!(tx.execute("UPDATE channel_members SET left_at=?3 WHERE channel_id=?1 AND member_kind='session' AND member_id=?2 AND left_at IS NULL",rusqlite::params![id,reader,crewd_store::ts_from_ms(crewd_store::now_ms())])?,1);
        assert!(!crewd_collab::channels::readable_by(&tx,&id,&reader)?);
        tx.commit()?;Ok::<_,crewd_collab::CollabError>(())
    }).await.unwrap().unwrap();
    let filtered = tool(
        &mut member,
        "view_folder_list",
        json!({"crew_id":f.crew,"view":"messages","part":"assignments","limit":100}),
    )
    .await;
    assert!(filtered["items"].as_array().unwrap().is_empty());
    assert_eq!(
        filtered["treeRevision"], assigned["treeRevision"],
        "scope revision is not a hidden object count"
    );
    assert!(!filtered.to_string().contains(&visible) && !filtered.to_string().contains(&hidden));
    assert!(filtered.get("hiddenCount").is_none());
    let before = snapshot(&f).await;
    assert_eq!(
        call(
            &mut member,
            "viewFolder.list",
            json!({"crewId":f.crew,"view":"messages"})
        )
        .await
        .unwrap_err()
        .0,
        crewd_proto::RpcError::FORBIDDEN,
        "attached connection cannot borrow local Human to reveal filtered refs"
    );
    assert_eq!(snapshot(&f).await, before);
    rpc(&mut human, "crew.remove", json!({"crewId":b})).await;
    let before = snapshot(&f).await;
    assert_eq!(
        call(&mut human, "crew.purge", json!({"crewId":b}))
            .await
            .unwrap_err()
            .0,
        crewd_proto::RpcError::CONFLICT
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "purge cannot silently discard foreign structured refs, even currently invisible to the agent"
    );
    let cleared = rpc(&mut human, "viewFolder.assign", json!({"crewId":f.crew,"view":"messages","expectedTreeRevision":assigned["treeRevision"],"object":{"kind":"channel","id":visible},"folderId":null})).await;
    assert!(cleared["assignments"].as_array().unwrap().is_empty());
    rpc(&mut human, "crew.purge", json!({"crewId":b})).await;
    assert_eq!(
        rpc(
            &mut human,
            "viewFolder.list",
            json!({"crewId":f.crew,"view":"messages"})
        )
        .await,
        cleared,
        "explicit unbind allows source purge while retaining foreign folder metadata"
    );
    close_socket(task, stop).await;
}

#[tokio::test]
async fn resource_event_subscriptions_cannot_keep_human_privilege_after_attach() {
    let f = Fixture::new().await;
    let b = f.add_crew("private resource event crew", None).await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut observer = RpcClient::connect(&socket).await;
    rpc(
        &mut observer,
        "events.subscribe",
        json!({"topics":["documents","viewFolders"]}),
    )
    .await;
    let mut switched = RpcClient::connect(&socket).await;
    rpc(
        &mut switched,
        "events.subscribe",
        json!({"topics":["documents","viewFolders"]}),
    )
    .await;
    rpc(
        &mut switched,
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await;
    assert_eq!(
        call(
            &mut switched,
            "events.subscribe",
            json!({"topics":["documents","viewFolders"]})
        )
        .await
        .unwrap_err()
        .0,
        crewd_proto::RpcError::FORBIDDEN
    );
    let made=rpc(&mut human,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":b,"title":"foreign private event metadata","body":"private body not broadcast"})).await;
    let event = observer
        .next_event(LIMIT)
        .await
        .expect("actual Human subscriber receives committed document event");
    assert_eq!(event.event, "document.changed");
    assert_eq!(event.data["id"], made["document"]["id"]);
    assert!(
        !event
            .data
            .to_string()
            .contains("private body not broadcast")
    );
    assert!(
        switched
            .next_event(Duration::from_millis(150))
            .await
            .is_none(),
        "subscribe-before-attach cannot leak future Human resource stream"
    );
    close_socket(task, stop).await;
}

// Genuine public topic membership changes complement the explicit DM Store fixture.
#[tokio::test]
async fn public_topic_leave_revokes_read_without_extending_foreign_folder_scope() {
    let f = Fixture::new().await;
    let b = f.add_crew("foreign topic owner", None).await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut member = attach(&f, &socket, &f.worker).await;
    let topic = rpc(&mut human, "channel.create", json!({"crewId":b,"kind":"topic","name":"foreign topic actual membership","members":[{"kind":"session","id":f.worker}]})).await;
    let read = rpc(
        &mut member,
        "agent.call",
        json!({"tool":"read_channel","args":{"channel":topic["id"]}}),
    )
    .await;
    assert_eq!(read["isError"], false);
    let initial = rpc(
        &mut human,
        "viewFolder.list",
        json!({"crewId":f.crew,"view":"messages"}),
    )
    .await;
    let created = rpc(&mut human, "viewFolder.create", json!({"crewId":f.crew,"view":"messages","requestId":uuid::Uuid::new_v4().to_string(),"expectedTreeRevision":initial["treeRevision"],"name":"retain neutral classification container"})).await;
    let before = snapshot(&f).await;
    let refused = rpc(&mut member, "agent.call", json!({"tool":"view_folder_assign","args":{"crew_id":f.crew,"view":"messages","expected_tree_revision":created["treeRevision"],"object":{"kind":"channel","id":topic["id"]},"folder_id":created["folders"][0]["id"]}})).await;
    assert_eq!(
        refused["isError"], true,
        "foreign topic visibility does not extend the agreed DM-only classification scope"
    );
    assert_eq!(snapshot(&f).await, before);
    rpc(
        &mut human,
        "channel.removeMember",
        json!({"channelId":topic["id"],"member":{"kind":"session","id":f.worker}}),
    )
    .await;
    let read = rpc(
        &mut member,
        "agent.call",
        json!({"tool":"read_channel","args":{"channel":topic["id"]}}),
    )
    .await;
    assert_eq!(
        read["isError"], true,
        "public removal revokes the sole actual crewmate-derived read permission"
    );
    close_socket(task, stop).await;
}
