//! Verified owner loss through actual Unix RPC and attached session identity.
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
struct LostActorRig {
    f: Fixture,
    h: Handle,
    fake: RelayStandIn,
    fail: FailSwitch,
    actor: Option<Actor>,
    forward: JoinHandle<()>,
    owner: String,
}
impl Drop for LostActorRig {
    fn drop(&mut self) {
        if let Some(a) = &self.actor {
            a.abort_handle().abort();
        }
        self.forward.abort();
    }
}
impl LostActorRig {
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
        let fail = FailSwitch {
            fake: fake.clone(),
            failed: Default::default(),
        };
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(fail.clone())),
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
            fail,
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

#[derive(Clone)]
struct FailSwitch {
    fake: RelayStandIn,
    failed: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}
impl crewd_server::orchestra::exec::Executor for FailSwitch {
    fn launch(
        &self,
        req: crewd_server::orchestra::exec::LaunchRequest,
    ) -> crewd_server::orchestra::exec::BoxFut<
        '_,
        Result<crewd_server::orchestra::exec::Launched, crewd_server::orchestra::exec::LaunchError>,
    > {
        if self.failed.lock().unwrap().contains(&req.session_id) {
            Box::pin(async {
                Err(crewd_server::orchestra::exec::LaunchError::Failed(
                    "ACTUAL_SYNTHETIC_RESUME_FAILURE".into(),
                ))
            })
        } else {
            crewd_server::orchestra::exec::Executor::launch(&self.fake, req)
        }
    }
}
impl LostActorRig {
    async fn fail_actual_owner(&self) {
        self.fail
            .failed
            .lock()
            .unwrap()
            .insert(self.f.worker.clone());
        self.h.resume(&self.f.worker, Origin::Human).await.unwrap();
        timeout(LIMIT, async {
            loop {
                let id = self.f.worker.clone();
                let row = self
                    .f
                    .state
                    .db
                    .call_result(move |s| crewd_store::sessions::get(s.conn(), &id))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if row.resume_failed_at.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            self.fake.launches(&self.f.worker).is_empty(),
            "real executor error rejected launch, no fake Spawned"
        );
        assert!(!self.h.live(&self.f.worker).await.unwrap().running);
    }
}
async fn lost_plan(r: &LostActorRig, human: &mut RpcClient) -> Value {
    let crew = r.f.me(&r.f.worker).await.crew_id;
    rpc(
        human,
        "work.create",
        json!({"crewId":crew,"title":"verified lost owner public workflow","owner":r.f.worker}),
    )
    .await
}
fn reassign(p: &Value, owner: &str) -> Value {
    json!({"id":p["id"],"expectedRevision":p["revision"],"owner":owner,"note":"LONG_EXPLANATION_ON_DEMAND actual failed resume verified"})
}

#[tokio::test]
async fn healthy_stopped_and_attached_human_channel_cannot_claim_loss() {
    let r = LostActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut worker = attach(&r.f, &socket, &r.f.worker).await;
    let p = lost_plan(&r, &mut human).await;
    let before = snapshot(&r.f).await;
    let err = call(&mut human, "work.reassignLost", reassign(&p, &r.owner))
        .await
        .unwrap_err();
    assert_eq!(err.0, crewd_proto::RpcError::INVALID_PARAMS);
    assert_eq!(
        snapshot(&r.f).await,
        before,
        "stopped/unknown is not verified loss"
    );
    let err = call(&mut worker, "work.reassignLost", reassign(&p, &r.owner))
        .await
        .unwrap_err();
    assert_eq!(err.0, crewd_proto::RpcError::FORBIDDEN);
    assert_eq!(
        snapshot(&r.f).await,
        before,
        "attached ordinary RPC cannot borrow Human privilege"
    );
    let denied=rpc(&mut worker,"agent.call",json!({"tool":"work_item_reassign_lost","args":{"id":p["id"],"expected_revision":p["revision"],"owner":r.owner,"note":"worker cannot use privileged loss reassignment"}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&r.f).await, before);
    close_socket(task, stop).await;
    r.close().await;
}
#[tokio::test]
async fn actual_failed_resume_authorizes_public_reassignment_and_delivers_compact_new_owner_wake() {
    let r = LostActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let p = lost_plan(&r, &mut human).await;
    r.fail_actual_owner().await;
    let assigned = rpc(&mut human, "work.reassignLost", reassign(&p, &r.owner)).await;
    assert_eq!(assigned["owner"], r.owner);
    let history = rpc(&mut human, "work.history", json!({"id":p["id"]})).await;
    assert_eq!(history["items"][0]["how"], "lost");
    assert_eq!(history["items"][0]["from"], r.f.worker);
    assert_eq!(history["items"][0]["to"], r.owner);
    let row = r
        .rows()
        .await
        .into_iter()
        .find(|w| w.source.starts_with("work-lost:"))
        .unwrap();
    assert_eq!(
        row.source,
        format!(
            "work-lost:{}:{}",
            p["id"].as_str().unwrap(),
            assigned["revision"].as_str().unwrap()
        )
    );
    r.working(false).await;
    timeout(LIMIT, async {
        while r.fake.sent(&r.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let actual = r.fake.sent(&r.owner).join("\n");
    assert!(actual.contains(p["id"].as_str().unwrap()));
    assert!(actual.contains("verified_owner_lost"));
    assert!(!actual.contains("LONG_EXPLANATION_ON_DEMAND"));
    assert!(!actual.contains("actual failed resume verified"));
    assert_ne!(
        r.rows()
            .await
            .into_iter()
            .find(|w| w.id == row.id)
            .unwrap()
            .state,
        wakes::WakeState::Delivered,
        "queue presence/send is not delivery"
    );
    let send = r.fake.last_send_id(&r.owner);
    r.fake.emit(
        &r.owner,
        SessionEvent::Delivery {
            id: send,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    let done = r.settle(row.id, wakes::WakeState::Delivered).await;
    assert_eq!(done.attempts, 1);
    close_socket(task, stop).await;
    r.close().await;
}
#[tokio::test]
async fn actual_lost_reassignment_sql_fault_rolls_back_and_render_claim_rechecks_completion() {
    let r = LostActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let p = lost_plan(&r, &mut human).await;
    r.fail_actual_owner().await;
    r.f.state.db.call_result(|s| {s.conn().execute_batch("CREATE TRIGGER reject_lost_wake BEFORE INSERT ON wakes WHEN NEW.source LIKE 'work-lost:%' BEGIN SELECT RAISE(ABORT,'independent lost wake failure'); END;")?;Ok::<_,crewd_collab::CollabError>(())}).await.unwrap().unwrap();
    let before = snapshot(&r.f).await;
    assert!(
        call(&mut human, "work.reassignLost", reassign(&p, &r.owner))
            .await
            .is_err()
    );
    assert_eq!(
        snapshot(&r.f).await,
        before,
        "authority/history/event/revision/wake rollback together"
    );
    r.f.state
        .db
        .call_result(|s| {
            s.conn().execute_batch("DROP TRIGGER reject_lost_wake")?;
            Ok::<_, crewd_collab::CollabError>(())
        })
        .await
        .unwrap()
        .unwrap();
    let assigned = rpc(&mut human, "work.reassignLost", reassign(&p, &r.owner)).await;
    let row = r
        .rows()
        .await
        .into_iter()
        .find(|w| w.source.starts_with("work-lost:"))
        .unwrap();
    let mut gate = r.f.caller_gate(&r.owner, CallerStage::WakeRendered);
    r.fake.emit(&r.owner, SessionEvent::Working(false));
    let rendered = timeout(LIMIT, gate.entered()).await.unwrap();
    assert!(rendered.text.contains(p["id"].as_str().unwrap()));
    let mut owner = attach(&r.f, &socket, &r.owner).await;
    tool(&mut owner,"work_item_update",json!({"id":p["id"],"expected_revision":assigned["revision"],"status":"done","note":"actual owner completed before claim"})).await;
    gate.release();
    let dropped = r.settle(row.id, wakes::WakeState::Dropped).await;
    assert_eq!(dropped.attempts, 0);
    assert!(r.fake.sent(&r.owner).is_empty());
    close_socket(task, stop).await;
    r.close().await;
}

#[tokio::test]
async fn prior_explicit_human_stop_preserves_reassignment_without_relaunching_target() {
    let r = LostActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let created = lost_plan(&r, &mut human).await;
    let mut original_owner = attach(&r.f, &socket, &r.f.worker).await;
    let p = tool(&mut original_owner, "work_item_update",json!({"id":created["id"],"expected_revision":created["revision"],"status":"active","note":"real owner active report before loss","next_check_at":crewd_store::ts_from_ms(crewd_store::now_ms()+60*60_000)})).await;
    r.h.stop(&r.owner).await.unwrap();
    timeout(LIMIT, async {
        while r.h.live(&r.owner).await.unwrap().running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.fail_actual_owner().await;
    let assigned = rpc(&mut human, "work.reassignLost", reassign(&p, &r.owner)).await;
    assert_eq!(
        assigned["owner"], r.owner,
        "verified reassignment itself remains committed"
    );
    assert!(
        r.rows()
            .await
            .into_iter()
            .all(|w| !w.source.starts_with("work-lost:")),
        "a prior explicit Human stop must prevent the new lost assignment from scheduling another model turn"
    );
    for _ in 0..20 {
        r.h.latches().await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            r.fake.launches(&r.owner).len(),
            1,
            "checkpoint/stop checks must respect prior Human stop even though reassignment makes the active plan due now"
        );
    }
    assert_eq!(r.fake.launches(&r.owner).len(), 1);
    assert!(!r.h.live(&r.owner).await.unwrap().running);
    r.h.resume(&r.owner, Origin::Human).await.unwrap();
    timeout(LIMIT, async {
        while r.fake.launches(&r.owner).len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        r.h.live(&r.owner).await.unwrap().running,
        "only actual Human resume enables the stopped target again"
    );
    close_socket(task, stop).await;
    r.close().await;
}

// Silent-loss timing is checked at the existing production Tx boundary. This does
// not claim two wall-clock runner turns: no DB flag/counter is seeded, and only
// real checkpoint enqueue/claim/ack commands produce the two durable receipts.
#[tokio::test]
async fn durable_checkpoint_receipts_require_grace_before_verified_silent_reassignment() {
    use crewd_collab::{Sender, cursor_reservations as cr};
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let first_due = crewd_store::now_ms() + 60_000;
    let p = rpc(&mut human, "work.create", json!({"crewId":f.crew,"title":"actual durable checkpoint authority","owner":f.worker,"nextCheckAt":crewd_store::ts_from_ms(first_due)})).await;
    let number = p["id"].as_str().unwrap()[1..].parse::<i64>().unwrap();
    for (index, at) in [first_due, first_due + 15 * 60_000].into_iter().enumerate() {
        let owner = f.worker.clone();
        let count = f
            .state
            .db
            .call_result(move |s| {
                assert_eq!(
                    crewd_collab::run(s, at, crewd_collab::checkpoints::enqueue_due)?.0,
                    1
                );
                let row = wakes::list(s.conn(), Some(&owner), 100)?
                    .into_iter()
                    .find(|w| {
                        w.kind == wakes::WakeKind::Checkpoint
                            && w.state == wakes::WakeState::Pending
                    })
                    .unwrap();
                let state = crewd_store::work_organization::load(s.conn())?.unwrap();
                let core = crewd_store::work_organization::core_for_reference(
                    s.conn(),
                    crewd_store::object_refs::ObjectKind::Plan,
                    number,
                )?
                .unwrap()
                .parse::<crewd_work::WorkId>()
                .unwrap();
                assert_eq!(
                    state.items[&core].missed_checks, index as u32,
                    "enqueue alone does not count as delivery"
                );
                let mut prepared = cr::snapshot(s.conn(), &owner, &[], at)?;
                prepared.capture_work_followups(s.conn(), std::slice::from_ref(&row), at)?;
                let lease = match crewd_collab::run(s, at, |tx| {
                    cr::claim_reserved(tx, &prepared, &[], &[row.id], &[], cr::Owner::new(1)?)
                })?
                .0
                {
                    cr::Decision::Applied(l) => l,
                    other => panic!("unexpected actual claim: {other:?}"),
                };
                assert!(matches!(
                    crewd_collab::run(s, at, |tx| cr::ack_reserved_delivered(
                        tx,
                        &lease,
                        wakes::Riders::Release
                    ))?
                    .0,
                    cr::Decision::Applied(())
                ));
                assert!(
                    matches!(
                        crewd_collab::run(s, at, |tx| cr::ack_reserved_delivered(
                            tx,
                            &lease,
                            wakes::Riders::Release
                        ))?
                        .0,
                        cr::Decision::Stale
                    ),
                    "duplicate successful receipt never increments again"
                );
                let row = wakes::by_ids(s.conn(), &[row.id])?.pop().unwrap();
                assert_eq!(row.state, wakes::WakeState::Delivered);
                let state = crewd_store::work_organization::load(s.conn())?.unwrap();
                Ok::<_, crewd_collab::CollabError>(state.items[&core].missed_checks)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, index as u32 + 1);
    }
    let current = rpc(&mut human, "work.get", json!({"id":p["id"]})).await;
    let before = snapshot(&f).await;
    assert_eq!(
        call(
            &mut human,
            "work.reassignLost",
            reassign(&current, &f.captain)
        )
        .await
        .unwrap_err()
        .0,
        crewd_proto::RpcError::INVALID_PARAMS
    );
    assert_eq!(
        snapshot(&f).await,
        before,
        "two durable deliveries without elapsed grace are not loss"
    );
    let params: crewd_proto::WorkAssignParams =
        serde_json::from_value(reassign(&current, &f.captain)).unwrap();
    let assigned = f
        .state
        .db
        .call_result(move |s| {
            crewd_collab::run(s, first_due + 30 * 60_000, |tx| {
                crewd_collab::work_lost::reassign(tx, &Sender::Human, params)
            })
        })
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(assigned.owner.as_deref(), Some(f.captain.as_str()));
    let history = rpc(&mut human, "work.history", json!({"id":p["id"]})).await;
    assert_eq!(history["items"][0]["how"], "lost");
    close_socket(task, stop).await;
}
