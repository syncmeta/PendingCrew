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
