//! Factual closure via actual Unix socket and authenticated agent tools.
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

use crewd_proto::Role;
fn close_args(p: &Value, kind: &str) -> Value {
    json!({"id":p["id"],"expectedRevision":p["revision"],"kind":kind,"evidence":{"kind":"text","value":"verified synthetic artifact"},"note":"核实后关闭"})
}
fn tool_args(p: &Value, kind: &str) -> Value {
    let mut a = close_args(p, kind);
    a["expected_revision"] = a["expectedRevision"].take();
    a.as_object_mut().unwrap().remove("expectedRevision");
    a
}
async fn make(c: &mut RpcClient, f: &Fixture, title: &str) -> Value {
    rpc(
        c,
        "work.create",
        json!({"crewId":f.crew,"title":title,"owner":f.worker}),
    )
    .await
}
async fn reported(f: &Fixture) -> i64 {
    f.state.db.call_result(|s|s.conn().query_row("SELECT count(*) FROM organization_work_events WHERE json_extract(body,'$.kind.type')='reported'",[],|r|r.get(0)).map_err(crewd_store::StoreError::from)).await.unwrap().unwrap()
}
#[tokio::test]
async fn factual_close_requires_evidence_and_exact_cas_without_hidden_report() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    for kind in ["done", "duplicate", "superseded"] {
        let p = make(&mut human, &f, kind).await;
        for bad in [
            json!(null),
            json!({"kind":"text","value":"  "}),
            json!({"kind":"commit","value":"\n"}),
            json!({"kind":"url","value":""}),
        ] {
            let before = snapshot(&f).await;
            let mut args = close_args(&p, kind);
            args["evidence"] = bad;
            assert!(call(&mut human, "work.close", args).await.is_err());
            assert_eq!(snapshot(&f).await, before);
        }
        let mut absent = close_args(&p, kind);
        absent.as_object_mut().unwrap().remove("evidence");
        let before = snapshot(&f).await;
        assert!(call(&mut human, "work.close", absent).await.is_err());
        assert_eq!(snapshot(&f).await, before);
        let mut blank_note = close_args(&p, kind);
        blank_note["note"] = json!(" ");
        let before = snapshot(&f).await;
        assert!(call(&mut human, "work.close", blank_note).await.is_err());
        assert_eq!(snapshot(&f).await, before);
        let reports = reported(&f).await;
        let out = rpc(&mut human, "work.close", close_args(&p, kind)).await;
        assert_eq!(out["id"], p["id"]);
        assert_eq!(
            out["status"],
            if kind == "done" { "done" } else { "dropped" }
        );
        assert_eq!(reported(&f).await, reports);
        assert_eq!(out["closure"]["kind"], kind);
        assert_eq!(out["closure"]["note"], "核实后关闭");
        let before = snapshot(&f).await;
        assert!(
            call(&mut human, "work.close", close_args(&out, kind))
                .await
                .is_err()
        );
        assert_eq!(
            snapshot(&f).await,
            before,
            "fresh CAS cannot close a terminal work again"
        );
        let before = snapshot(&f).await;
        assert!(
            call(&mut human, "work.close", close_args(&p, kind))
                .await
                .is_err()
        );
        assert_eq!(
            snapshot(&f).await,
            before,
            "stale retry must not duplicate events or wakes"
        );
    }
    drop(human);
    close_socket(task, stop).await;
}
#[tokio::test]
async fn attached_admin_closes_own_crew_but_never_borrows_foreign_or_human_authority() {
    let f = Fixture::new().await;
    f.add_session(&f.crew, "close-admin", Role::Admin, "Admin")
        .await;
    let other = f.add_crew("other close crew", None).await;
    f.add_session(&other, "foreign-admin", Role::Admin, "Foreign Admin")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut admin = attach(&f, &socket, "close-admin").await;
    let mut foreign = attach(&f, &socket, "foreign-admin").await;
    let mut cap = attach(&f, &socket, &f.captain).await;
    for kind in ["done", "duplicate", "superseded"] {
        let p = make(&mut human, &f, kind).await;
        let before = snapshot(&f).await;
        let denied = rpc(
            &mut foreign,
            "agent.call",
            json!({"tool":"work_item_close","args":tool_args(&p,kind)}),
        )
        .await;
        assert_eq!(denied["isError"], true);
        assert_eq!(snapshot(&f).await, before);
        assert!(
            call(&mut foreign, "work.close", close_args(&p, kind))
                .await
                .is_err()
        );
        assert_eq!(snapshot(&f).await, before);
        let reports = reported(&f).await;
        let out = tool(&mut admin, "work_item_close", tool_args(&p, kind)).await;
        assert_eq!(out["id"], p["id"]);
        assert_eq!(reported(&f).await, reports);
        let history = rpc(
            &mut human,
            "work.closureHistory",
            json!({"id":p["id"],"limit":1}),
        )
        .await;
        assert_eq!(history["items"].as_array().unwrap().len(), 1);
        assert_eq!(history["items"][0]["actor"]["id"], "close-admin");
        assert_eq!(history["items"][0]["actor"]["role"], "admin");
        assert_eq!(history["items"][0]["closure"], out["closure"]);
        let older = rpc(
            &mut human,
            "work.closureHistory",
            json!({"id":p["id"],"beforeSeq":history["items"][0]["seq"],"limit":1}),
        )
        .await;
        assert!(older["items"].as_array().unwrap().is_empty());
        let p = make(&mut human, &f, "captain factual closure").await;
        tool(&mut cap, "work_item_close", tool_args(&p, kind)).await;
    }
    drop((human, admin, foreign, cap));
    close_socket(task, stop).await;
}
#[tokio::test]
async fn failure_writing_closure_event_rolls_back_original_work_and_all_side_effects() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let p = make(&mut human, &f, "rollback closure").await;
    f.state.db.call_result(|s|s.conn().execute_batch("CREATE TRIGGER fail_factual_event BEFORE INSERT ON organization_work_events WHEN json_extract(NEW.body,'$.kind.type') IN ('done','dropped') BEGIN SELECT RAISE(ABORT,'synthetic factual event failure'); END;").map_err(crewd_store::StoreError::from)).await.unwrap().unwrap();
    let before = snapshot(&f).await;
    assert!(
        call(&mut human, "work.close", close_args(&p, "done"))
            .await
            .is_err()
    );
    assert_eq!(snapshot(&f).await, before);
    f.state
        .db
        .call_result(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER fail_factual_event")
                .map_err(crewd_store::StoreError::from)
        })
        .await
        .unwrap()
        .unwrap();
    rpc(&mut human, "work.close", close_args(&p, "done")).await;
    let prerequisite = make(&mut human, &f, "rollback release notice").await;
    rpc(&mut human,"work.create",json!({"crewId":f.crew,"title":"ready when prerequisite closes","owner":f.worker,"dependsOn":[prerequisite["id"]]})).await;
    f.state.db.call_result(|s|s.conn().execute_batch("CREATE TRIGGER fail_factual_wake BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'synthetic closure wake failure'); END;").map_err(crewd_store::StoreError::from)).await.unwrap().unwrap();
    let before = snapshot(&f).await;
    assert!(
        call(&mut human, "work.close", close_args(&prerequisite, "done"))
            .await
            .is_err()
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "wake failure must roll back closure event and dependent changes"
    );
    f.state
        .db
        .call_result(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER fail_factual_wake")
                .map_err(crewd_store::StoreError::from)
        })
        .await
        .unwrap()
        .unwrap();
    rpc(&mut human, "work.close", close_args(&prerequisite, "done")).await;
    drop(human);
    close_socket(task, stop).await;
}
#[tokio::test]
async fn factual_drop_does_not_silently_satisfy_a_hard_dependency() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let p = make(&mut human, &f, "hard prerequisite").await;
    rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"hard dependent","owner":f.worker,"dependsOn":[p["id"]]}),
    )
    .await;
    for kind in ["duplicate", "superseded"] {
        let before = snapshot(&f).await;
        assert!(
            call(&mut human, "work.close", close_args(&p, kind))
                .await
                .is_err()
        );
        assert_eq!(snapshot(&f).await, before);
    }
    rpc(&mut human, "work.close", close_args(&p, "done")).await;
    drop(human);
    close_socket(task, stop).await;
}

#[tokio::test]
async fn closure_releases_real_blockers_and_delivers_one_owner_notice_only_after_typed_ack() {
    use crewd_collab::wakes::{self, WakeState};
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    use crewd_server::{
        agent::testkit::RelayStandIn,
        orchestra::{
            self,
            clock::SystemClock,
            core::{Origin, StartSession},
            db::{DbPort, pump_collab},
            exec::Executors,
            hooks::Hooks,
        },
    };
    use std::sync::{Arc, OnceLock};
    let cell = Arc::new(OnceLock::new());
    let f = Fixture::with_orchestra(cell.clone()).await;
    let fake = RelayStandIn::default();
    let rx = f.state.hub.subscribe_collab();
    let (h, actor) = orchestra::spawn(
        Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
        Executors::new().with("claude", Arc::new(fake.clone())),
        Hooks::default(),
        Arc::new(SystemClock),
        orchestra::OrchestraConfig {
            stagger_ms: 0,
            retry_jitter: 0.0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(cell.set(h.clone()).is_ok());
    assert!(f.state.orchestra.set(h.clone()).is_ok());
    let sink = h.clone();
    let forward = tokio::spawn(pump_collab(rx, move |fact| sink.collab(fact)));
    let owner = h
        .start(
            StartSession {
                crew_id: f.crew.clone(),
                role: crewd_proto::Role::Worker,
                executor: "claude".into(),
                brief: "synthetic closure notification recipient".into(),
                title: Some("close worker".into()),
                model: None,
                effort: None,
                isolation: false,
                codex_options: None,
            },
            Origin::Human,
        )
        .await
        .unwrap()
        .id;
    tokio::time::timeout(LIMIT, async {
        while fake.launches(&owner).is_empty() {
            h.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fake.emit(
        &owner,
        SessionEvent::Delivery {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    fake.emit(&owner, SessionEvent::Working(true));
    fake.emit(&owner, SessionEvent::Working(false));
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    for kind in ["done", "duplicate", "superseded"] {
        let p = rpc(
            &mut human,
            "work.create",
            json!({"crewId":f.crew,"title":format!("{kind} blocker"),"owner":f.worker}),
        )
        .await;
        let mut released = Vec::new();
        for n in 0..2 {
            let child = rpc(
                &mut human,
                "work.create",
                json!({"crewId":f.crew,"title":format!("blocked child {n}"),"owner":owner}),
            )
            .await;
            rpc(&mut human,"work.block",json!({"id":child["id"],"expectedRevision":child["revision"],"on":[{"type":"work","id":p["id"]}],"canContinue":{"type":"nothing"}})).await;
            released.push(child["id"].as_str().unwrap().to_owned());
        }
        let closed = rpc(&mut human, "work.close", close_args(&p, kind)).await;
        for id in &released {
            let item = rpc(&mut human, "work.get", json!({"id":id})).await;
            assert!(item["blockedOn"].as_array().unwrap().is_empty());
        }
        let o = owner.clone();
        let target = released[0].clone();
        let rows = f
            .state
            .db
            .call_result(move |s| wakes::list(s.conn(), Some(&o), 100))
            .await
            .unwrap()
            .unwrap();
        let matches: Vec<_> = rows
            .into_iter()
            .filter(|w| {
                w.note
                    .as_ref()
                    .is_some_and(|n| n.contains("unblockedWork") && n.contains(&target))
            })
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "one source/owner wake for two released works"
        );
        let row = matches[0].clone();
        let ids = released.clone();
        tokio::time::timeout(LIMIT, async {
            loop {
                h.latches().await.unwrap();
                if fake
                    .sent(&owner)
                    .iter()
                    .any(|s| ids.iter().all(|id| s.contains(id)) && s.contains("unblockedWork"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let wid = row.id;
        let before = f
            .state
            .db
            .call_result(move |s| wakes::by_ids(s.conn(), &[wid]))
            .await
            .unwrap()
            .unwrap()
            .remove(0);
        assert_eq!(before.state, WakeState::Inflight);
        fake.emit(
            &owner,
            SessionEvent::Delivery {
                id: fake.last_send_id(&owner),
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        tokio::time::timeout(LIMIT, async {
            loop {
                let wid = row.id;
                let state = f
                    .state
                    .db
                    .call_result(move |s| wakes::by_ids(s.conn(), &[wid]))
                    .await
                    .unwrap()
                    .unwrap()
                    .remove(0)
                    .state;
                if state == WakeState::Delivered {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            call(&mut human, "work.close", close_args(&p, kind))
                .await
                .is_err()
        );
        let current = rpc(&mut human, "work.get", json!({"id":p["id"]})).await;
        assert_eq!(current["revision"], closed["revision"]);
        fake.emit(&owner, SessionEvent::Working(true));
        fake.emit(&owner, SessionEvent::Working(false));
    }
    drop(human);
    close_socket(task, stop).await;
    assert!(h.shutdown(Duration::from_secs(2)).await);
    actor.finished().await.unwrap();
    forward.abort();
    let _ = forward.await;
}
#[tokio::test]
async fn attached_foreign_admin_cannot_bypass_factual_close_via_legacy_human_write_rpcs() {
    let f = Fixture::new().await;
    let other = f.add_crew("foreign legacy write scope", None).await;
    f.add_session(&other, "legacy-foreign-admin", Role::Admin, "foreign admin")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut foreign = attach(&f, &socket, "legacy-foreign-admin").await;
    let p = make(&mut human, &f, "protected original work").await;
    let dependency = make(&mut human, &f, "existing unrelated dependency").await;
    for (method, args) in [
        (
            "work.create",
            json!({"crewId":f.crew,"title":"must not create as Human","owner":f.worker}),
        ),
        (
            "work.update",
            json!({"id":p["id"],"expectedRevision":p["revision"],"status":"done"}),
        ),
        (
            "work.block",
            json!({"id":p["id"],"expectedRevision":p["revision"],"on":[{"type":"work","id":dependency["id"]}],"canContinue":{"type":"nothing"}}),
        ),
        (
            "work.assign",
            json!({"id":p["id"],"expectedRevision":p["revision"],"owner":"legacy-foreign-admin","note":"must not borrow Human forced assignment"}),
        ),
    ] {
        let before = snapshot(&f).await;
        let denied = call(&mut foreign, method, args).await;
        assert!(denied.is_err(), "{method}: {denied:?}");
        assert_eq!(
            snapshot(&f).await,
            before,
            "{method} mutated original work/ledger/wakes"
        );
    }
    drop((human, foreign));
    close_socket(task, stop).await;
}
