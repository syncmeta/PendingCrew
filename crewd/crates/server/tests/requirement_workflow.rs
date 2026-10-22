//! Raw human requirements through actual Unix RPC and attached-agent read permissions.
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
async fn public_requirement_create_retry_cross_crew_source_done_and_agent_read_authority() {
    let f = Fixture::new().await;
    let b = f.add_crew("requirement B", None).await;
    f.add_session(&b, "requirement-b-owner", Role::Worker, "B owner")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let args = json!({"requestId":"00000000-0000-4000-8000-000000000001","crewId":f.crew,"text":"original requirement: keep exact scope"});
    let created = rpc(&mut human, "todo.create", args.clone()).await;
    assert_eq!(created["status"], "unclaimed");
    assert_eq!(created["text"], args["text"]);
    assert!(created["closure"].is_null());
    let id = created["requirementId"].clone();
    assert!(id.as_str().unwrap().starts_with('#'));
    let before = snapshot(&f).await;
    let retried = rpc(&mut human, "todo.create", args.clone()).await;
    assert_eq!(retried, created);
    assert_eq!(
        snapshot(&f).await,
        before,
        "idempotent retry must have no durable effect"
    );
    for bad in [
        json!({"requestId":args["requestId"],"crewId":f.crew,"text":"changed intent"}),
        json!({"requestId":args["requestId"],"crewId":b,"text":args["text"]}),
    ] {
        let before = snapshot(&f).await;
        let err = call(&mut human, "todo.create", bad).await.unwrap_err();
        assert_eq!(err.0, -32009);
        assert_eq!(
            snapshot(&f).await,
            before,
            "request-key conflict must not mutate any durable state"
        );
    }
    assert_eq!(rpc(&mut human, "todo.get", json!({"id":id})).await, created);
    let plan=rpc(&mut human,"work.create",json!({"crewId":b,"title":"cross crew implementation","owner":"requirement-b-owner","sources":[id]})).await;
    assert_eq!(plan["sources"], json!([id]));
    let running = rpc(&mut human, "todo.get", json!({"id":id})).await;
    assert_eq!(running["status"], "in_progress");
    assert_eq!(running["text"], created["text"]);
    let before = snapshot(&f).await;
    let denied = call(
        &mut human,
        "todo.close",
        json!({"id":id,"note":"cannot close live work","expectedStatus":"in_progress"}),
    )
    .await
    .unwrap_err();
    assert!(denied.0 < 0);
    assert_eq!(
        snapshot(&f).await,
        before,
        "active linked work forbids human close atomically"
    );
    let mut owner = attach(&f, &socket, "requirement-b-owner").await;
    assert_eq!(
        tool(&mut owner, "todo_get", json!({"id":id})).await,
        running
    );
    let tools = rpc(&mut owner, "agent.tools", json!(null)).await;
    let names: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"todo_get"));
    assert!(!names.contains(&"todo_create"));
    assert!(!names.contains(&"todo_close"));
    for name in ["todo_create", "todo_close"] {
        let before = snapshot(&f).await;
        let denied=rpc(&mut owner,"agent.call",json!({"tool":name,"args":{"id":id,"crewId":f.crew,"text":"agent spoof","note":"agent spoof"}})).await;
        assert_eq!(denied["isError"], true);
        assert_eq!(snapshot(&f).await, before);
    }
    // Attached agent transport must not borrow the local Human-only mutation surface.
    let mut observed_facts = f.state.hub.subscribe_collab();
    let mut observed_events = f.state.hub.subscribe();
    assert_eq!(rpc(&mut owner, "todo.get", json!({"id":id})).await, running);
    for (method, params) in [
        (
            "todo.create",
            json!({"requestId":"00000000-0000-4000-8000-000000000010","crewId":f.crew,"text":"attached transport spoof"}),
        ),
        (
            "todo.close",
            json!({"id":id,"expectedStatus":"in_progress","note":"attached transport spoof"}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert!(call(&mut owner, method, params).await.is_err());
        assert_eq!(
            snapshot(&f).await,
            before,
            "attached direct Human mutation must have no durable effects"
        );
        assert!(
            matches!(
                observed_facts.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "attached mutation refusal cannot publish collab facts"
        );
        assert!(
            matches!(
                observed_events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "attached mutation refusal cannot publish client events"
        );
    }
    tool(
        &mut owner,
        "work_item_update",
        json!({"id":plan["id"],"expected_revision":plan["revision"],"status":"done"}),
    )
    .await;
    let done = rpc(&mut human, "todo.get", json!({"id":id})).await;
    assert_eq!(done["status"], "done");
    assert_eq!(done["text"], created["text"]);
    assert!(
        done["closure"].is_null(),
        "derived done must not invent human closure"
    );
    let before = snapshot(&f).await;
    assert_eq!(
        rpc(&mut human, "todo.create", args).await,
        done,
        "retry returns current derived view"
    );
    assert_eq!(snapshot(&f).await, before);
    drop((human, owner));
    close_socket(task, stop).await;
}

#[tokio::test]
async fn all_dropped_requirement_needs_human_ruling_and_retains_exact_closure_note() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let created=rpc(&mut human,"todo.create",json!({"requestId":"00000000-0000-4000-8000-000000000002","crewId":f.crew,"text":"needs deliberate ruling"})).await;
    let id = created["requirementId"].clone();
    let plan = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"withdrawn implementation","owner":f.worker,"sources":[id]}),
    )
    .await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let dropped = tool(
        &mut owner,
        "work_item_update",
        json!({
            "id":plan["id"],"expected_revision":plan["revision"],"status":"dropped",
            "note":"superseded synthetic implementation"
        }),
    )
    .await;
    assert_eq!(dropped["status"], "dropped");
    let ruling = rpc(&mut human, "todo.get", json!({"id":id})).await;
    assert_eq!(ruling["status"], "needs_ruling");
    assert!(ruling["closure"].is_null());
    let before = snapshot(&f).await;
    assert!(
        call(
            &mut human,
            "todo.close",
            json!({"id":id,"expectedStatus":"unclaimed","note":"stale ruling"})
        )
        .await
        .is_err()
    );
    assert_eq!(snapshot(&f).await, before);
    let note =
        "Human chose to retire original requirement after its implementation was superseded.";
    let closed = rpc(
        &mut human,
        "todo.close",
        json!({"id":id,"expectedStatus":"needs_ruling","note":note}),
    )
    .await;
    assert_eq!(closed["status"], "closed");
    assert_eq!(closed["closure"]["note"], note);
    assert!(closed["closure"]["at"].is_string());
    assert_eq!(rpc(&mut human, "todo.get", json!({"id":id})).await, closed);
    drop((human, owner));
    close_socket(task, stop).await;
}

#[tokio::test]
async fn frozen_requirement_mutations_and_real_storage_failure_are_atomic_without_id_reuse() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let created=rpc(&mut human,"todo.create",json!({"requestId":"00000000-0000-4000-8000-000000000003","crewId":f.crew,"text":"frozen evidence remains readable"})).await;
    let id = created["requirementId"].clone();
    rpc(&mut human, "crew.remove", json!({"crewId":f.crew})).await;
    let before = snapshot(&f).await;
    for (m, p) in [
        (
            "todo.create",
            json!({"requestId":"00000000-0000-4000-8000-000000000004","crewId":f.crew,"text":"must not allocate"}),
        ),
        (
            "todo.close",
            json!({"id":id,"expectedStatus":"unclaimed","note":"must not close frozen"}),
        ),
    ] {
        assert!(call(&mut human, m, p).await.is_err());
        assert_eq!(snapshot(&f).await, before);
    }
    assert_eq!(rpc(&mut human, "todo.get", json!({"id":id})).await, created);
    rpc(&mut human, "crew.restore", json!({"crewId":f.crew})).await;
    f.state.db.call(|s|s.conn().execute_batch("CREATE TRIGGER fail_requirement_wake BEFORE INSERT ON wakes WHEN NEW.source LIKE 'requirement-new:%' BEGIN SELECT RAISE(ABORT,'synthetic requirement wake failure'); END;")).await.unwrap().unwrap();
    let before = snapshot(&f).await;
    let retry = json!({"requestId":"00000000-0000-4000-8000-000000000005","crewId":f.crew,"text":"retry after actual SQLite failure"});
    assert!(
        call(&mut human, "todo.create", retry.clone())
            .await
            .is_err()
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "fault after ID/requirement preparation must roll back all tables"
    );
    f.state
        .db
        .call(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER fail_requirement_wake;")
        })
        .await
        .unwrap()
        .unwrap();
    let next = rpc(&mut human, "todo.create", retry).await;
    assert!(
        next["requirementId"].as_str().unwrap()[1..]
            .parse::<i64>()
            .unwrap()
            > id.as_str().unwrap()[1..].parse::<i64>().unwrap(),
        "previously published identity is never reused"
    );
    let third=rpc(&mut human,"todo.create",json!({"requestId":"00000000-0000-4000-8000-000000000006","crewId":f.crew,"text":"independent next request"})).await;
    assert_ne!(third["requirementId"], next["requirementId"]);
    drop(human);
    close_socket(task, stop).await;
}

use crewd_collab::wakes;
use crewd_exec::{
    delivery::Subject,
    session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
};
use crewd_server::{
    agent::testkit::{CallerStage, RelayStandIn},
    orchestra::{
        self, Actor, Handle, OrchestraConfig,
        clock::ManualClock,
        core::{Origin, StartSession},
        db::{DbPort, pump_collab},
        exec::Executors,
        hooks::Hooks,
    },
};
use crewd_wake::Ms;
use std::sync::{Arc, OnceLock};
use tokio::{task::JoinHandle, time::timeout};
struct ReqActorRig {
    f: Fixture,
    h: Handle,
    fake: RelayStandIn,
    actor: Option<Actor>,
    forward: JoinHandle<()>,
    owner: String,
}
impl Drop for ReqActorRig {
    fn drop(&mut self) {
        if let Some(a) = &self.actor {
            a.abort_handle().abort();
        }
        self.forward.abort();
    }
}
impl ReqActorRig {
    async fn new() -> Self {
        let cell = Arc::new(OnceLock::new());
        let mut f = Fixture::with_orchestra(cell.clone()).await;
        f.crew = f.add_crew("fresh requirement actor crew", None).await;
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
        let rx = f.state.hub.subscribe_collab();
        let fake = RelayStandIn::default();
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(fake.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(Ms(crewd_store::now_ms() + 60_000))),
            OrchestraConfig {
                stagger_ms: 0,
                heartbeat_ms: 10,
                retry_jitter: 0.0,
                jitter_seed: Some(1),
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
                    codex_options: None,

                    crew_id: f.crew.clone(),
                    captain: true,
                    executor: "claude".into(),
                    brief: "synthetic actor acceptance".into(),
                    title: Some("actor owner".into()),
                    model: None,
                    effort: None,
                    isolation: false,
                },
                Origin::Human,
            )
            .await
            .unwrap()
            .id;
        timeout(LIMIT, async {
            while !h.live(&owner).await.unwrap().running {
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
        let r = Self {
            f,
            h,
            fake,
            actor: Some(actor),
            forward,
            owner,
        };
        r.working(true).await;
        r
    }
    async fn working(&self, on: bool) {
        self.fake.emit(&self.owner, SessionEvent::Working(on));
        timeout(LIMIT, async {
            while self.h.live(&self.owner).await.unwrap().working != on {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn rows(&self) -> Vec<wakes::WakeRow> {
        let owner = self.owner.clone();
        self.f
            .state
            .db
            .call_result(move |s| wakes::list(s.conn(), Some(&owner), 100))
            .await
            .unwrap()
            .unwrap()
    }
    async fn settle(&self, id: i64, state: wakes::WakeState) -> wakes::WakeRow {
        timeout(LIMIT, async {
            loop {
                let row = self.rows().await.into_iter().find(|r| r.id == id).unwrap();
                if row.state == state {
                    return row;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(mut self) {
        assert!(
            timeout(LIMIT, self.h.shutdown(Duration::from_secs(2)))
                .await
                .unwrap()
        );
        timeout(LIMIT, self.actor.take().unwrap().finished())
            .await
            .unwrap()
            .unwrap();
        self.forward.abort();
        let _ = (&mut self.forward).await;
        let db = self.f.state.db.clone();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn new_requirement_reaches_actual_captain_executor_then_commits_delivery_receipt() {
    let r = ReqActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let created=rpc(&mut human,"todo.create",json!({"requestId":"00000000-0000-4000-8000-000000000007","crewId":r.f.crew,"text":"ACTUAL_NEW_REQUIREMENT_CAPTAIN_DELIVERY"})).await;
    let public = created["requirementId"].as_str().unwrap();
    let rows = r.rows().await;
    let wake = rows
        .iter()
        .find(|w| w.source == format!("requirement-new:{public}"))
        .unwrap();
    let id = wake.id;
    assert_eq!(wake.session, r.owner);
    r.working(false).await;
    timeout(LIMIT, async {
        while r.fake.sent(&r.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let sent = r.fake.sent(&r.owner);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains(public));
    assert!(
        !sent[0].contains("ACTUAL_NEW_REQUIREMENT_CAPTAIN_DELIVERY"),
        "notification carries identity only; explicit todo_get retrieves original text"
    );
    assert_eq!(r.settle(id, wakes::WakeState::Inflight).await.attempts, 1);
    r.fake.emit(
        &r.owner,
        SessionEvent::Delivery {
            id: r.fake.last_send_id(&r.owner),
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    r.settle(id, wakes::WakeState::Delivered).await;
    drop(human);
    close_socket(task, stop).await;
    r.close().await;
}
#[tokio::test]
async fn requirement_changed_after_real_render_never_claims_or_reaches_executor() {
    for mode in ["claimed", "closed"] {
        let r = ReqActorRig::new().await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = r.f.spawn_owned_socket(stopping).await;
        let socket = r.f.data_dir().join("crewd.sock");
        let mut human = RpcClient::connect(&socket).await;
        let request = if mode == "claimed" {
            "00000000-0000-4000-8000-000000000008"
        } else {
            "00000000-0000-4000-8000-000000000009"
        };
        let created=rpc(&mut human,"todo.create",json!({"requestId":request,"crewId":r.f.crew,"text":"STALE_NEW_REQUIREMENT_MUST_NOT_SEND"})).await;
        let public = created["requirementId"].as_str().unwrap();
        let rows = r.rows().await;
        let id = rows
            .iter()
            .find(|w| w.source == format!("requirement-new:{public}"))
            .unwrap()
            .id;
        let mut gate = r.f.caller_gate(&r.owner, CallerStage::WakeRendered);
        r.fake.emit(&r.owner, SessionEvent::Working(false));
        let rendered = timeout(LIMIT, gate.entered()).await.unwrap();
        assert!(rendered.text.contains(public));
        if mode == "claimed" {
            rpc(&mut human,"work.create",json!({"crewId":r.f.crew,"title":"claimed actual requirement","owner":r.owner,"sources":[public]})).await;
        } else {
            rpc(&mut human,"todo.close",json!({"id":public,"expectedStatus":"unclaimed","note":"human withdrew before delivery"})).await;
        }
        gate.release();
        let dropped = r.settle(id, wakes::WakeState::Dropped).await;
        assert_eq!(dropped.attempts, 0);
        r.h.latches().await.unwrap();
        assert!(
            r.fake.sent(&r.owner).is_empty(),
            "{mode}: outdated requirement reached executor"
        );
        assert_eq!(r.fake.launches(&r.owner).len(), 1);
        drop(human);
        close_socket(task, stop).await;
        r.close().await;
    }
}
