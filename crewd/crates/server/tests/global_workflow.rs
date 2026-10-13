//! Independent cross-crew workflow acceptance through actual socket/attached tool identity.
//! Only synthetic fixture preparation touches the authoritative organization book directly.
#![cfg(feature = "testkit")]
use crewd_proto::Role;
use crewd_server::agent::testkit::{Fixture, RpcClient};
use crewd_store::object_refs::ObjectKind;
use serde_json::{Value, json};
use std::time::Duration;

async fn rpc(c: &mut RpcClient, method: &str, params: Value) -> Value {
    tokio::time::timeout(Duration::from_secs(5), c.call(method, params))
        .await
        .unwrap()
        .unwrap()
}
async fn tool(c: &mut RpcClient, name: &str, args: Value) -> Value {
    let out = rpc(c, "agent.call", json!({"tool":name,"args":args})).await;
    assert_eq!(out["isError"], false, "{out}");
    let text = out["text"].as_str().unwrap();
    // Tool localization surrounds the common JSON receipt, not a second API shape.
    let begin = text.find('{').expect("structured work receipt");
    let end = text.rfind('}').unwrap() + 1;
    serde_json::from_str(&text[begin..end]).unwrap()
}
async fn attach(f: &Fixture, socket: &std::path::Path, session: &str) -> RpcClient {
    let mut c = RpcClient::connect(socket).await;
    let out = rpc(
        &mut c,
        "agent.attach",
        json!({"sessionId":session,"token":f.token(session)}),
    )
    .await;
    assert!(out["crewId"].is_string());
    c
}
async fn drain(c: &mut RpcClient) -> Vec<Value> {
    let mut events = Vec::new();
    while let Some(e) = c.next_event(Duration::from_millis(30)).await {
        events.push(serde_json::to_value(e).unwrap());
    }
    events
}
fn assert_changed(events: &[Value], crew: &str, id: &Value) {
    assert!(
        events.iter().any(|e| e["event"] == "work.changed"
            && e["data"]["crewId"] == crew
            && e["data"]["id"] == *id),
        "missing affected crew {crew} / {id}: {events:?}"
    );
    for e in events
        .iter()
        .filter(|e| e["event"] == "work.changed" && e["data"]["id"] == *id)
    {
        assert_eq!(e["data"]["crewId"], crew, "event must use owning crew");
    }
}

#[tokio::test]
async fn attached_owner_finishes_cross_crew_child_without_losing_other_human_blocker() {
    let f = Fixture::new().await;
    // Explicit synthetic migration is permitted before production schema-9 activation.
    // It is idempotent after activation and never changes the legacy archive to mimic new state.
    f.state
        .db
        .call(|s| {
            let tx = s.conn().unchecked_transaction()?;
            crewd_store::work_organization::migrate_in_tx(&tx, "2026-10-13T00:00:00.000Z")?;
            tx.commit()?;
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    let a_crew = f.crew.clone();
    let b_crew = f.add_crew("independent B", None).await;
    f.add_session(&b_crew, "b-owner", Role::Worker, "B owner")
        .await;
    f.add_session(&b_crew, "b-captain", Role::Captain, "B captain")
        .await;
    let c_crew = f.add_crew("independent C", None).await;
    f.add_session(&c_crew, "foreign-captain", Role::Captain, "foreign captain")
        .await;
    // An actual source requirement is registered in B; public # identity is obtained from registry.
    let b = b_crew.clone();
    let source = f
        .state
        .collab(move |tx| {
            crewd_collab::asks::with_book(tx, &b, |book, now| {
                book.register_entry(now, crewd_work::EntryNo(1))?;
                Ok(())
            })?;
            Ok(crewd_store::work_organization::reference_for_core(
                tx.conn(),
                ObjectKind::Requirement,
                "#1",
            )?
            .unwrap()
            .id())
        })
        .await
        .unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut observer = RpcClient::connect(&socket).await;
    rpc(
        &mut observer,
        "events.subscribe",
        json!({"topics":["work"]}),
    )
    .await;
    let parent = rpc(
        &mut human,
        "work.create",
        json!({"crewId":a_crew,"title":"A parent","owner":f.worker}),
    )
    .await;
    let child = rpc(&mut human, "work.create", json!({"crewId":b_crew,"title":"B child","owner":"b-owner","parent":parent["id"],"sources":[source]})).await;
    let a = rpc(&mut human, "work.create", json!({"crewId":a_crew,"title":"A dependency","owner":f.worker,"dependsOn":[child["id"]],"sources":[source]})).await;
    let sibling = rpc(&mut human, "work.create", json!({"crewId":a_crew,"title":"A second dependency","owner":f.worker,"dependsOn":[child["id"]]})).await;
    let same = rpc(
        &mut human,
        "work.create",
        json!({"crewId":a_crew,"title":"same crew child","owner":f.worker,"parent":parent["id"]}),
    )
    .await;
    let ids = [
        parent["id"].clone(),
        child["id"].clone(),
        a["id"].clone(),
        same["id"].clone(),
        sibling["id"].clone(),
        json!(source),
    ];
    let unique: std::collections::BTreeSet<_> = ids.iter().map(Value::to_string).collect();
    assert_eq!(unique.len(), ids.len(), "global identities may not collide");
    assert_eq!(child["parent"], parent["id"]);
    assert_eq!(child["sources"], json!([source]));
    assert_eq!(a["dependsOn"], json!([child["id"]]));
    assert_eq!(same["parent"], parent["id"]);
    let crew = a_crew.clone();
    let owner = f.worker.clone();
    let aid = a["id"].as_str().unwrap()[1..].parse::<i64>().unwrap();
    let sid = sibling["id"].as_str().unwrap()[1..].parse::<i64>().unwrap();
    let bid = child["id"].as_str().unwrap()[1..].parse::<i64>().unwrap();
    let h = f
        .state
        .collab(move |tx| {
            let a_core = crewd_store::work_organization::core_for_reference(
                tx.conn(),
                ObjectKind::Plan,
                aid,
            )?
            .unwrap()
            .parse()
            .unwrap();
            let b_core = crewd_store::work_organization::core_for_reference(
                tx.conn(),
                ObjectKind::Plan,
                bid,
            )?
            .unwrap()
            .parse()
            .unwrap();
            let s_core = crewd_store::work_organization::core_for_reference(
                tx.conn(),
                ObjectKind::Plan,
                sid,
            )?
            .unwrap()
            .parse()
            .unwrap();
            let h = crewd_collab::asks::with_book(tx, &crew, |book, now| {
                let result = book.block(
                    &crewd_work::Actor::worker(&owner),
                    now,
                    a_core,
                    vec![
                        crewd_work::BlockOn::Item(b_core),
                        crewd_work::BlockOn::NewAsk {
                            question: "choose final format".into(),
                            default: None,
                        },
                    ],
                    crewd_work::Continuation::Nothing,
                )?;
                let h = result.value.asks[0];
                book.block(
                    &crewd_work::Actor::worker(&owner),
                    now,
                    s_core,
                    vec![
                        crewd_work::BlockOn::Item(b_core),
                        crewd_work::BlockOn::Ask(h),
                    ],
                    crewd_work::Continuation::Nothing,
                )?;
                Ok(h)
            })?;
            Ok(crewd_store::work_organization::reference_for_core(
                tx.conn(),
                ObjectKind::HumanTask,
                &h.to_string(),
            )?
            .unwrap()
            .id())
        })
        .await
        .unwrap();
    let blocked = rpc(&mut human, "work.get", json!({"id":a["id"]})).await;
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(blocked["blockedOn"].as_array().unwrap().len(), 2);
    drain(&mut observer).await;
    let mut foreign = attach(&f, &socket, "foreign-captain").await;
    let denied = rpc(&mut foreign, "agent.call", json!({"tool":"work_item_update","args":{"id":child["id"],"expected_revision":child["revision"],"status":"done","evidence":{"kind":"text","value":"spoofed"}}})).await;
    assert_eq!(
        denied["isError"], true,
        "foreign captain cannot override another crew"
    );
    assert_eq!(
        rpc(&mut human, "work.get", json!({"id":child["id"]})).await["revision"],
        child["revision"]
    );
    let next = crewd_store::ts_from_ms(crewd_store::now_ms() + 60_000);
    let mut b_owner = attach(&f, &socket, "b-owner").await;
    let report = tool(&mut b_owner, "work_item_update", json!({"id":child["id"],"expected_revision":child["revision"],"status":"active","note":"implemented B","next_check_at":next})).await;
    let done = tool(
        &mut b_owner,
        "work_item_update",
        json!({"id":child["id"],"expected_revision":report["revision"],"status":"done"}),
    )
    .await;
    assert_eq!(done["status"], "done");
    assert!(
        done["nextCheckAt"].is_null(),
        "finished child must retire checkpoint"
    );
    let still = rpc(&mut human, "work.get", json!({"id":a["id"]})).await;
    assert_eq!(still["status"], "blocked");
    assert_eq!(
        still["blockedOn"],
        json!([h]),
        "B completion must retain independent H"
    );
    let changed = drain(&mut observer).await;
    assert_changed(&changed, &a_crew, &a["id"]);
    assert_changed(&changed, &a_crew, &sibling["id"]);
    assert_changed(&changed, &b_crew, &child["id"]);
    // Answer the actual published H through the human socket, never fabricate owner consent.
    rpc(
        &mut human,
        "ask.answer",
        json!({"crewId":a_crew,"ask":h,"text":"use agreed format"}),
    )
    .await;
    let ready = rpc(&mut human, "work.get", json!({"id":a["id"]})).await;
    assert_eq!(ready["status"], "active");
    assert_eq!(ready["blockedOn"], json!([]));
    assert!(
        ready["nextCheckAt"].is_string(),
        "ready transition must schedule checkpoint without owner re-report"
    );
    let answered = drain(&mut observer).await;
    assert_changed(&answered, &a_crew, &a["id"]);
    assert_changed(&answered, &a_crew, &sibling["id"]);
    let sibling_ready = rpc(&mut human, "work.get", json!({"id":sibling["id"]})).await;
    assert_eq!(sibling_ready["blockedOn"], json!([]));
    assert_eq!(sibling_ready["status"], "active");
    let mut a_owner = attach(&f, &socket, &f.worker).await;
    let scheduled = tool(&mut a_owner, "work_item_update", json!({"id":a["id"],"expected_revision":ready["revision"],"status":"active","note":"resumed after B and H","next_check_at":next})).await;
    assert_eq!(scheduled["nextCheckAt"], next);
    // This fixture has no runner: committed queue rows remain available as actual side-effect evidence.
    let worker = f.worker.clone();
    f.state
        .db
        .call(move |s| {
            let count: i64 = s.conn().query_row(
                "SELECT count(*) FROM wakes WHERE session_id=?1 AND kind='ask_answer'",
                [worker],
                |r| r.get(0),
            )?;
            assert_eq!(
                count, 1,
                "last H creates one wake for owner, not one per affected work"
            );
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    drop((human, observer, foreign, a_owner, b_owner));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

// Capture authoritative state and every durable effect relevant to refused writes.
async fn effect_snapshot(f: &Fixture) -> Vec<String> {
    f.state.db.call(|s| {
        let mut out = Vec::new();
        for query in [
            "SELECT json_array(state,updated_at) FROM organization_work_book ORDER BY singleton",
            "SELECT json_array(seq,crew_id,body) FROM organization_work_events ORDER BY seq",
            "SELECT json_array(number,kind,crew_id,source_id,core_id) FROM object_refs ORDER BY number",
            "SELECT json_array(name,seq) FROM sqlite_sequence ORDER BY name",
            "SELECT json_array(id,session_id,kind,source,state,note,attempts) FROM wakes ORDER BY id",
            "SELECT json_array(id,channel_id,text) FROM messages ORDER BY id",
        ] {
            out.push(query.to_owned());
            let mut stmt = s.conn().prepare(query)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows { out.push(row?); }
        }
        Ok::<_, crewd_store::StoreError>(out)
    }).await.unwrap().unwrap()
}

#[tokio::test]
async fn public_attached_block_creates_human_and_split_then_reuses_cross_crew_refs_atomically() {
    let f = Fixture::new().await;
    f.state
        .db
        .call(|s| {
            let tx = s.conn().unchecked_transaction()?;
            crewd_store::work_organization::migrate_in_tx(&tx, "2026-10-13T00:00:00.000Z")?;
            tx.commit()?;
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    let b = f.add_crew("public block B", None).await;
    f.add_session(&b, "public-b-owner", Role::Worker, "B owner")
        .await;
    f.add_session(&b, "public-b-captain", Role::Captain, "B captain")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut observer = RpcClient::connect(&socket).await;
    rpc(
        &mut observer,
        "events.subscribe",
        json!({"topics":["work","ask"]}),
    )
    .await;
    let parent = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"public blocked parent","owner":f.worker}),
    )
    .await;
    let another = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"reuse W and H","owner":f.worker}),
    )
    .await;
    let dependency = rpc(
        &mut human,
        "work.create",
        json!({"crewId":b,"title":"foreign dependency","owner":"public-b-owner"}),
    )
    .await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let receipt = tool(
        &mut owner,
        "work_item_block",
        json!({
            "id":parent["id"],"expected_revision":parent["revision"],
            "on":[{"type":"new_human","question":"public H created by attached owner"}],
            "can_continue":{"type":"split","items":[{"title":"independent split portion"}]}
        }),
    )
    .await;
    assert_eq!(receipt["item"]["status"], "blocked");
    assert_eq!(receipt["humanTasks"].as_array().unwrap().len(), 1);
    assert_eq!(receipt["split"].as_array().unwrap().len(), 1);
    let h = receipt["humanTasks"][0].clone();
    let child = receipt["split"][0].clone();
    assert!(h.as_str().unwrap().starts_with('H'));
    assert!(child.as_str().unwrap().starts_with('W'));
    assert_ne!(child, parent["id"]);
    assert_ne!(child, dependency["id"]);
    let split = rpc(&mut human, "work.get", json!({"id":child})).await;
    assert_eq!(split["parent"], parent["id"]);
    assert_eq!(split["owner"], f.worker);
    assert_eq!(split["crewId"], f.crew);
    assert_eq!(
        split["blockedOn"],
        json!([]),
        "independent split must actually be runnable"
    );
    let ask = rpc(&mut human, "ask.list", json!({})).await;
    assert!(
        ask.as_array().unwrap().iter().any(|a| a["ask"]["ask"] == h
            && a["ask"]["question"] == "public H created by attached owner"
            && a["ask"]["askedBy"] == f.worker
            && a["ask"]["item"] == parent["id"]),
        "created H must be visible through public ask query: {ask}"
    );
    let reused = tool(
        &mut owner,
        "work_item_block",
        json!({
            "id":another["id"],"expected_revision":another["revision"],
            "on":[{"type":"work","id":dependency["id"]},{"type":"human","id":h}],
            "can_continue":{"type":"nothing"}
        }),
    )
    .await;
    assert_eq!(reused["item"]["status"], "blocked");
    let blockers = reused["item"]["blockedOn"].as_array().unwrap();
    assert_eq!(blockers.len(), 2);
    assert!(blockers.contains(&dependency["id"]));
    assert!(blockers.contains(&h));
    assert_eq!(
        reused["humanTasks"],
        json!([]),
        "existing H reuse must not allocate another H"
    );
    assert_eq!(reused["split"], json!([]));
    drain(&mut observer).await;
    // Stale CAS attempt would allocate both H and split if the guard were after mutation.
    let before = effect_snapshot(&f).await;
    let stale = rpc(
        &mut owner,
        "agent.call",
        json!({"tool":"work_item_block","args":{
            "id":parent["id"],"expected_revision":parent["revision"],
            "on":[{"type":"new_human","question":"must never be allocated"}],
            "can_continue":{"type":"split","items":[{"title":"must never exist"}]}
        }}),
    )
    .await;
    assert_eq!(stale["isError"], true);
    assert_eq!(
        effect_snapshot(&f).await,
        before,
        "CAS refusal must preserve state/events/IDs/wakes/messages"
    );
    assert!(
        drain(&mut observer).await.is_empty(),
        "CAS refusal cannot publish changes"
    );
    let mut foreign = attach(&f, &socket, "public-b-captain").await;
    let before = effect_snapshot(&f).await;
    let denied = rpc(
        &mut foreign,
        "agent.call",
        json!({"tool":"work_item_block","args":{
            "id":parent["id"],"expected_revision":receipt["item"]["revision"],
            "on":[{"type":"new_human","question":"foreign authority spoof"}],
            "can_continue":{"type":"split","items":[{"title":"foreign spoof split"}]}
        }}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    assert_eq!(
        effect_snapshot(&f).await,
        before,
        "foreign captain refusal must preserve all durable effects"
    );
    assert!(drain(&mut observer).await.is_empty());
    drop((human, observer, owner, foreign));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
