//! Actual Human Unix RPC and attached Agent search. Synthetic, provider-free.
//! Test-only statistics, clocks and normal-process migration are separate gates.
#![cfg(feature = "testkit")]
use crewd_proto::Role;
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};
const LIMIT: Duration = Duration::from_secs(15);
const NEEDLE: &str = "organization-boundary-needle";

async fn call(peer: &mut RpcClient, method: &str, params: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(LIMIT, peer.call(method, params))
        .await
        .unwrap()
}
async fn rpc(peer: &mut RpcClient, method: &str, params: Value) -> Value {
    call(peer, method, params).await.unwrap()
}
async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
    rpc(peer, "agent.call", json!({"tool":name,"args":args})).await
}
fn decoded(value: &Value) -> Value {
    assert_eq!(value["isError"], false, "{value}");
    serde_json::from_str(value["text"].as_str().unwrap()).unwrap()
}
async fn attached(f: &Fixture, socket: &Path, session: &str) -> RpcClient {
    let mut peer = RpcClient::connect(socket).await;
    rpc(
        &mut peer,
        "agent.attach",
        json!({"sessionId":session,"token":f.token(session)}),
    )
    .await;
    peer
}
async fn ready_human(peer: &mut RpcClient, query: Value) -> Value {
    tokio::time::timeout(LIMIT, async {
        loop {
            let page = rpc(peer, "organization.search", query.clone()).await;
            if page["indexState"] == "ready" {
                return page;
            }
            assert_eq!(page["indexState"], "building");
            assert_eq!(page["items"], json!([]));
            assert!(page["snapshot"].is_null() && page["nextCursor"].is_null());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual maintenance must complete the original synthetic source work")
}
async fn ready_agent(peer: &mut RpcClient, name: &str, args: Value) -> Value {
    tokio::time::timeout(LIMIT, async {
        loop {
            let page = decoded(&tool(peer, name, args.clone()).await);
            if page["indexState"] == "ready" {
                return page;
            }
            assert_eq!(page["indexState"], "building");
            let content = if name == "search" { "groups" } else { "items" };
            assert_eq!(page[content], json!([]));
            assert!(page["snapshot"].is_null() && page["nextCursor"].is_null());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("an attached read must observe real maintenance, not fake-ready state")
}
fn hits(page: &Value) -> Vec<Value> {
    page["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| {
            let kind = group["kind"].clone();
            group["items"].as_array().unwrap().iter().map(move |hit| {
                assert_eq!(hit["kind"], kind);
                if kind == "row" {
                    assert!(group["tableId"].is_string() && group["tableTitle"].is_string());
                    assert_eq!(hit["tableId"], group["tableId"]);
                }
                hit.clone()
            })
        })
        .collect()
}
async fn all_hits(peer: &mut RpcClient, mut args: Value) -> Vec<Value> {
    let mut result = Vec::new();
    let mut cursors = BTreeSet::new();
    let mut snapshot = None;
    for _ in 0..128 {
        let page = ready_agent(peer, "search", args.clone()).await;
        if let Some(original) = &snapshot {
            assert_eq!(&page["snapshot"], original);
        } else {
            snapshot = Some(page["snapshot"].clone());
        }
        result.extend(hits(&page));
        if page["nextCursor"].is_null() {
            let keys = result
                .iter()
                .map(|x| serde_json::to_string(&x["target"]).unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                keys.len(),
                result.len(),
                "pagination cannot repeat the same source hit"
            );
            return result;
        }
        let cursor = page["nextCursor"].as_str().unwrap().to_owned();
        assert!(cursors.insert(cursor.clone()));
        args["cursor"] = cursor.into();
    }
    panic!("the small fixture cannot require an unbounded continuation")
}
async fn business(f: &Fixture) -> Vec<String> {
    f.state
        .db
        .call(|store| {
            let mut result = Vec::new();
            for name in [
                "messages",
                "wakes",
                "sessions",
                "organization_work_book",
                "organization_work_events",
                "managed_decisions",
                "managed_tables",
                "managed_rows",
                "managed_v2_receipts",
            ] {
                let mut query = store.conn().prepare(&format!("SELECT * FROM {name}"))?;
                let count = query.column_count();
                let mut rows = query
                    .query_map([], |row| {
                        let mut fields = Vec::new();
                        for i in 0..count {
                            fields.push(format!("{:?}", row.get_ref(i)?));
                        }
                        Ok(fields.join("|"))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.sort();
                result.push(name.to_string());
                result.extend(rows);
            }
            Ok::<_, crewd_store::StoreError>(result)
        })
        .await
        .unwrap()
        .unwrap()
}
async fn stop(server: tokio::task::JoinHandle<()>, shutdown: tokio::sync::oneshot::Sender<()>) {
    shutdown.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}

#[tokio::test]
async fn attached_search_uses_real_organization_objects_b15_h_acl_and_original_cursor() {
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    f.enable_organization_search();
    let foreign = f.add_crew("Independent search owner", None).await;
    let third = f.add_crew("Independent message scope", None).await;
    let colleague = "search-colleague";
    let other = "search-foreign-worker";
    f.add_session(&f.crew, colleague, Role::Worker, "Search colleague")
        .await;
    f.add_session(&foreign, other, Role::Worker, "Search foreign")
        .await;
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let scope = rpc(&mut human, "managedObject.scope", json!({})).await;
    let created=rpc(&mut human,"managedTable.v2.submit",json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"createTable","title":format!("{NEEDLE} table"),"purpose":"Actual organization readable source","titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}]}})).await;
    let table = created["effect"]["tableId"].as_str().unwrap().to_owned();
    let row=rpc(&mut human,"managedTable.v2.submit",json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"addRow","tableId":table,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":format!("{NEEDLE} row")}}}]}})).await["effect"]["rowId"].as_str().unwrap().to_owned();
    let decision=rpc(&mut human,"managedObject.submit",json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"recordDecision","draft":{"question":format!("{NEEDLE} decision"),"choice":"Keep actual object references","rationale":"Read is not a write grant","about":[{"kind":"row","id":row}]}}})).await["object"]["decision"]["id"].as_str().unwrap().to_owned();
    let own_h=rpc(&mut human,"humanTask.create",json!({"crewId":f.crew,"channelId":format!("{}/lobby",f.crew),"requestId":uuid::Uuid::new_v4().to_string(),"text":format!("{NEEDLE} own Human item")})).await["ask"]["ask"].as_str().unwrap().to_owned();
    let private_h=rpc(&mut human,"humanTask.create",json!({"crewId":foreign,"channelId":format!("{foreign}/lobby"),"requestId":uuid::Uuid::new_v4().to_string(),"text":format!("{NEEDLE} PRIVATE_HUMAN_BODY")})).await["ask"]["ask"].as_str().unwrap().to_owned();
    // A third crew is essential: own-crew B15 would survive explicit membership removal.
    let member_topic=rpc(&mut human,"channel.create",json!({"crewId":third,"kind":"topic","name":"Actual member scope","members":[{"kind":"session","id":f.worker}]})).await["id"].as_str().unwrap().to_owned();
    let private_topic = rpc(
        &mut human,
        "channel.create",
        json!({"crewId":third,"kind":"topic","name":"Actual private scope","members":[]}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let member_message = rpc(
        &mut human,
        "message.post",
        json!({"channelId":member_topic,"text":format!("{NEEDLE} member readable")}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let private_message = rpc(
        &mut human,
        "message.post",
        json!({"channelId":private_topic,"text":format!("{NEEDLE} PRIVATE_MESSAGE_BODY")}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ready = ready_human(&mut human, json!({"query":NEEDLE,"in":"all","limit":200})).await;
    assert_eq!(ready["scope"], scope);
    let mut worker = attached(&f, &socket, &f.worker).await;
    let mut same_crew = attached(&f, &socket, colleague).await;
    let mut foreign_worker = attached(&f, &socket, other).await;
    let before = business(&f).await;
    let mut changes = f.state.hub.subscribe_collab();
    let args = json!({"query":NEEDLE,"in":"all","limit":1});
    let found = all_hits(&mut worker, args.clone()).await;
    for id in [&table, &row, &decision, &own_h, &member_message] {
        assert!(
            found.iter().any(|x| x["target"]["id"] == id.as_str()),
            "missing authorized exact source {id}: {found:?}"
        );
    }
    for id in [&private_h, &private_message] {
        assert!(!found.iter().any(|x| x["target"]["id"] == id.as_str()));
    }
    let output = serde_json::to_string(&found).unwrap();
    assert!(!output.contains("PRIVATE_HUMAN_BODY") && !output.contains("PRIVATE_MESSAGE_BODY"));
    let first = ready_agent(&mut worker, "search", args.clone()).await;
    let cursor = first["nextCursor"]
        .as_str()
        .expect("several authorized sources require continuation");
    assert_eq!(
        tool(
            &mut same_crew,
            "search",
            json!({"query":NEEDLE,"in":"all","limit":1,"cursor":cursor})
        )
        .await["isError"],
        true,
        "a same-crew reader is still a distinct principal"
    );
    assert_eq!(
        tool(
            &mut worker,
            "search",
            json!({"query":"changed-query","in":"all","limit":1,"cursor":cursor})
        )
        .await["isError"],
        true
    );
    let filtered = all_hits(
        &mut worker,
        json!({"query":NEEDLE,"in":"objects","type":"decision","limit":20}),
    )
    .await;
    assert!(!filtered.is_empty() && filtered.iter().all(|x| x["kind"] == "decision"));
    let rows = all_hits(
        &mut worker,
        json!({"query":NEEDLE,"in":"objects","type":table,"limit":20}),
    )
    .await;
    assert!(rows.iter().any(|x| x["target"]["id"] == row));
    assert!(
        rows.iter()
            .all(|x| x["kind"] == "row" && x["tableId"] == table)
    );
    let foreign_hits = all_hits(&mut foreign_worker, args.clone()).await;
    assert!(
        foreign_hits.iter().any(|x| x["target"]["id"] == private_h),
        "foreign Human task is a real authorized positive for its own crew"
    );
    assert!(!foreign_hits.iter().any(|x| x["target"]["id"] == own_h
        || x["target"]["id"] == member_message
        || x["target"]["id"] == private_message));
    let expansion = ready_agent(
        &mut worker,
        "organization_search_expand",
        json!({"target":{"kind":"decision","id":decision},"limit":20}),
    )
    .await;
    assert!(
        expansion["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["direction"] == "outgoing"
                && x["neighbor"]["target"] == json!({"kind":"row","id":row})),
        "one hop must use the original decision about relation: {expansion}"
    );
    assert_eq!(
        business(&f).await,
        before,
        "all search/filter/cursor/expand reads preserve business rows, original request ledgers, messages, wakes and progress"
    );
    assert!(
        !std::iter::from_fn(|| changes.try_recv().ok())
            .any(|x| matches!(x, crewd_collab::Change::Progress { .. })),
        "a passive search is never progress"
    );
    rpc(
        &mut human,
        "channel.removeMember",
        json!({"channelId":member_topic,"member":{"kind":"session","id":f.worker}}),
    )
    .await;
    let after_membership = business(&f).await;
    let revoked = all_hits(&mut worker, args.clone()).await;
    assert!(!revoked.iter().any(|x| x["target"]["id"] == member_message));
    assert!(revoked.iter().any(|x| x["target"]["id"] == table));
    let continued = tool(
        &mut worker,
        "search",
        json!({"query":NEEDLE,"in":"all","limit":1,"cursor":cursor}),
    )
    .await;
    if continued["isError"] == false {
        assert!(
            !serde_json::to_string(&hits(&decoded(&continued)))
                .unwrap()
                .contains(&member_message)
        );
    }
    assert_eq!(business(&f).await, after_membership);
    f.state.tokens.revoke(&f.worker);
    for (name, args) in [
        ("search", json!({"query":NEEDLE,"in":"all"})),
        (
            "organization_search_expand",
            json!({"target":{"kind":"table","id":table}}),
        ),
    ] {
        assert_eq!(
            tool(&mut worker, name, args).await["isError"],
            true,
            "the original attachment must not retain live token authority"
        );
    }
    assert_eq!(business(&f).await, after_membership);
    stop(server, shutdown).await;
}

#[tokio::test]
async fn search_human_and_agent_raw_socket_boundaries_precede_parameter_parsing() {
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    f.enable_organization_search();
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut worker = attached(&f, &socket, &f.worker).await;
    ready_human(&mut human, json!({"query":"no-synthetic-match","limit":20})).await;
    let before = business(&f).await;
    for method in ["organization.search", "organization.search.expand"] {
        assert_eq!(
            call(&mut worker, method, json!([])).await.unwrap_err().0,
            crewd_proto::RpcError::FORBIDDEN,
            "attached callers cannot borrow the Human endpoint, even with malformed params"
        );
    }
    for tool_name in ["search", "organization_search_expand"] {
        assert_eq!(
            call(
                &mut human,
                "agent.call",
                json!({"tool":tool_name,"args":[]})
            )
            .await
            .unwrap_err()
            .0,
            crewd_proto::RpcError::METHOD_NOT_FOUND,
            "an unattached socket is not an Agent tool authority"
        );
    }
    let scope = rpc(&mut human, "managedObject.scope", json!({})).await;
    let error=call(&mut human,"organization.search",json!({"scope":{"dbId":scope["dbId"],"restoreEpoch":uuid::Uuid::new_v4().to_string()},"query":"no-synthetic-match","limit":20})).await.unwrap_err();
    assert!(
        error.0 < 0 && error.0 != crewd_proto::RpcError::METHOD_NOT_FOUND,
        "a wrong parameter epoch is rejected, not mistaken for an absent search method"
    );
    assert_eq!(business(&f).await, before);
    stop(server, shutdown).await;
}

#[tokio::test]
async fn captured_search_and_expansion_revalidate_role_and_token_before_fifo_read() {
    use crewd_server::agent::testkit::CallerStage;
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    f.enable_organization_search();
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let scope = rpc(&mut human, "managedObject.scope", json!({})).await;
    let created=rpc(&mut human,"managedTable.v2.submit",json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"createTable","title":"captured-read-boundary","purpose":"The original captured identity must be current at FIFO read","titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}]}})).await;
    let table = created["effect"]["tableId"].as_str().unwrap().to_owned();
    ready_human(
        &mut human,
        json!({"query":"captured-read-boundary","limit":20}),
    )
    .await;
    let mut worker = attached(&f, &socket, &f.worker).await;
    let mut role_gate = f.caller_gate(&f.worker, CallerStage::ManagedReadCaptured);
    let pending = tokio::spawn(async move {
        let result = tool(
            &mut worker,
            "search",
            json!({"query":"captured-read-boundary","in":"all","limit":20}),
        )
        .await;
        (worker, result)
    });
    let evidence = tokio::time::timeout(LIMIT, role_gate.entered())
        .await
        .unwrap();
    assert!(evidence.targets.is_empty() && evidence.text.is_empty());
    // A real leadership transfer changes the actual role, without editing SQL or
    // trusting an asserted role in the request. Both roles could read a fresh query.
    f.set_role(&f.worker, Role::Captain).await;
    let after_role = business(&f).await;
    role_gate.release();
    let (mut worker, denied) = tokio::time::timeout(LIMIT, pending).await.unwrap().unwrap();
    assert_eq!(
        denied["isError"], true,
        "a different current role cannot authorize the old captured read"
    );
    assert_eq!(business(&f).await, after_role);
    let current = ready_agent(
        &mut worker,
        "search",
        json!({"query":"captured-read-boundary","in":"all","limit":20}),
    )
    .await;
    assert!(
        hits(&current)
            .iter()
            .any(|hit| hit["target"]["id"] == table),
        "the newly current captain can submit its own fresh read"
    );
    let mut token_gate = f.caller_gate(&f.worker, CallerStage::ManagedReadCaptured);
    let pending = tokio::spawn(async move {
        let result = tool(
            &mut worker,
            "organization_search_expand",
            json!({"target":{"kind":"table","id":table},"limit":20}),
        )
        .await;
        (worker, result)
    });
    tokio::time::timeout(LIMIT, token_gate.entered())
        .await
        .unwrap();
    f.state.tokens.revoke(&f.worker);
    let after_token = business(&f).await;
    token_gate.release();
    let (_worker, denied) = tokio::time::timeout(LIMIT, pending).await.unwrap().unwrap();
    assert_eq!(
        denied["isError"], true,
        "token revocation after capture must win before source/edge projection"
    );
    assert_eq!(business(&f).await, after_token);
    stop(server, shutdown).await;
}

#[tokio::test]
async fn attached_search_timestamp_bounds_survive_runtime_enable_without_losing_all_scope() {
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let scope = rpc(&mut human, "managedObject.scope", json!({})).await;
    let needle = "timestamp-compatibility-needle";
    let body = format!("{needle} original-public-message-positive-control");
    let table=rpc(&mut human,"managedTable.v2.submit",json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"createTable","title":format!("{needle} table"),"purpose":"Keep original search RFC3339 timestamp compatibility","titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}]}})).await["effect"]["tableId"].as_str().unwrap().to_owned();
    let message = rpc(
        &mut human,
        "message.post",
        json!({"channelId":format!("{}/lobby",f.crew),"text":body}),
    )
    .await;
    let message_id = message["id"].as_str().unwrap().to_owned();
    let created =
        chrono::DateTime::parse_from_rfc3339(message["createdAt"].as_str().unwrap()).unwrap();
    let day = created.with_timezone(&chrono::Utc).date_naive();
    // Adjacent dates avoid an irrelevant midnight-crossing flake between the
    // two real seed writes. No SQL timestamp is forged for this comparison.
    let after = format!("{}T00:00:00Z", day.pred_opt().unwrap());
    let before = format!("{}T00:00:00Z", day.succ_opt().unwrap());
    let original = json!({"query":needle,"after":after,"before":before,"limit":50});
    let mut inclusive = original.clone();
    inclusive["after"] = message["createdAt"].clone();
    let mut excluded = original.clone();
    excluded["before"] = message["createdAt"].clone();
    let mut worker = attached(&f, &socket, &f.worker).await;
    let before_reads = business(&f).await;
    // The same actual attached socket first exercises the pre-runtime message
    // path. Its old successful response is rendered text rather than JSON.
    let legacy = tool(&mut worker, "search", original.clone()).await;
    assert_eq!(legacy["isError"], false, "{legacy}");
    assert!(legacy["text"].as_str().unwrap().contains(&body), "{legacy}");
    let legacy_inclusive = tool(&mut worker, "search", inclusive.clone()).await;
    assert_eq!(legacy_inclusive["isError"], false, "{legacy_inclusive}");
    assert!(legacy_inclusive["text"].as_str().unwrap().contains(&body));
    let legacy_exclusive = tool(&mut worker, "search", excluded.clone()).await;
    assert_eq!(legacy_exclusive["isError"], false, "{legacy_exclusive}");
    assert!(!legacy_exclusive["text"].as_str().unwrap().contains(&body));
    f.enable_organization_search();
    // No in/type changes: the original tool now uses the unified default All
    // engine. The table positive proves this cannot pass via message fallback.
    let unified = ready_agent(&mut worker, "search", original).await;
    let current = hits(&unified);
    assert!(current.iter().any(|hit| hit["target"]["id"] == message_id));
    assert!(
        current
            .iter()
            .any(|hit| hit["kind"] == "table" && hit["target"]["id"] == table)
    );
    let unified_inclusive = ready_agent(&mut worker, "search", inclusive).await;
    assert!(
        hits(&unified_inclusive)
            .iter()
            .any(|hit| hit["target"]["id"] == message_id)
    );
    let unified_exclusive = ready_agent(&mut worker, "search", excluded).await;
    assert!(
        !hits(&unified_exclusive)
            .iter()
            .any(|hit| hit["target"]["id"] == message_id)
    );
    assert_eq!(business(&f).await, before_reads);
    stop(server, shutdown).await;
}
