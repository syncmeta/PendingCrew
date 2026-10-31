//! Formal Human command receipts preserve Admin identity and reject role-changing replays.
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
use crewd_server::{
    agent::testkit::RelayStandIn,
    orchestra::{
        self,
        clock::SystemClock,
        db::{DbPort, pump_collab},
        exec::Executors,
        hooks::Hooks,
    },
};
use std::sync::{Arc, OnceLock};
struct Rig {
    f: Fixture,
    h: orchestra::Handle,
    actor: orchestra::Actor,
    forward: tokio::task::JoinHandle<()>,
    fake: RelayStandIn,
}
impl Rig {
    async fn new() -> Self {
        let cell = Arc::new(OnceLock::new());
        let f = Fixture::with_orchestra(cell.clone()).await;
        f.enable_public_command_identity().await;
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
        Self {
            f,
            h,
            actor,
            forward,
            fake,
        }
    }
    async fn close(self) {
        assert!(self.h.shutdown(Duration::from_secs(2)).await);
        self.actor.finished().await.unwrap();
        self.forward.abort();
        let _ = self.forward.await;
    }
}
fn start(crew: &str, role: &str) -> Value {
    json!({"crewId":crew,"role":role,"executor":"claude","brief":"synthetic role acceptance","title":"role test","isolation":false})
}
async fn submit(r: &Rig, c: &mut RpcClient, role: &str) -> (Value, Value, String) {
    let hello = rpc(c, "system.hello", json!({})).await;
    let identity = &hello["runtimeIdentity"];
    let packet = json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":{"operation":"session.start","params":start(&r.f.crew,role)}});
    let first = rpc(c, "command.submit", packet.clone()).await;
    assert_eq!(first["command"]["params"]["role"], role);
    let crew = r.f.crew.clone();
    let cap = r.f.captain.clone();
    let worker = r.f.worker.clone();
    let rows =
        r.f.state
            .db
            .call_result(move |s| crewd_store::sessions::list_for_crew(s.conn(), &crew))
            .await
            .unwrap()
            .unwrap();
    let made: Vec<_> = rows
        .into_iter()
        .filter(|s| s.id != cap && s.id != worker)
        .collect();
    assert_eq!(made.len(), 1);
    let id = made[0].id.clone();
    let receipt = tokio::time::timeout(LIMIT, async {
        loop {
            let v = rpc(
                c,
                "command.lookup",
                json!({"requestId":packet["requestId"],"dbId":packet["dbId"]}),
            )
            .await;
            if v["phase"] != "dispatched" {
                break v;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // RelayStandIn has no OS birth proof. Registration/role is real; external launch
    // confirmation must remain Unknown, never be inferred from its synthetic Spawned.
    assert_eq!(receipt["phase"], "unknown");
    assert_eq!(receipt["command"]["params"]["role"], role);
    tokio::time::timeout(LIMIT, async {
        while r.fake.launches(&id).is_empty() {
            r.h.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    (packet, receipt, id)
}
#[tokio::test]
async fn formal_human_admin_command_preserves_persisted_role_and_exact_repeat_identity() {
    let r = Rig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let mut c = RpcClient::connect(&r.f.data_dir().join("crewd.sock")).await;
    let before = rpc(&mut c, "crew.get", json!({"crewId":r.f.crew})).await;
    let (packet, receipt, id) = submit(&r, &mut c, "admin").await;
    let row = rpc(&mut c, "session.get", json!({"sessionId":id})).await;
    assert_eq!(row["role"], "admin");
    assert!(row["ext"].is_null());
    assert_eq!(
        rpc(&mut c, "crew.get", json!({"crewId":r.f.crew})).await["captainSessionId"],
        before["captainSessionId"]
    );
    assert_eq!(rpc(&mut c, "command.submit", packet).await, receipt);
    assert_eq!(r.fake.launches(&id).len(), 1);
    drop(c);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    r.close().await;
}
#[tokio::test]
async fn formal_receipt_rejects_worker_admin_role_mutation_in_both_directions() {
    for (from, to) in [("worker", "admin"), ("admin", "worker")] {
        let r = Rig::new().await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = r.f.spawn_owned_socket(stopping).await;
        let mut c = RpcClient::connect(&r.f.data_dir().join("crewd.sock")).await;
        let (packet, receipt, id) = submit(&r, &mut c, from).await;
        let mut altered = packet.clone();
        altered["command"]["params"]["role"] = json!(to);
        let denied = call(&mut c, "command.submit", altered).await;
        assert_eq!(
            denied.as_ref().unwrap_err().0,
            crewd_proto::RpcError::CONFLICT,
            "{from}->{to}: {denied:?}"
        );
        eprintln!("verified rejected receipt role mutation {from}->{to}: {denied:?}");
        assert_eq!(
            rpc(
                &mut c,
                "command.lookup",
                json!({"requestId":packet["requestId"],"dbId":packet["dbId"]})
            )
            .await,
            receipt
        );
        let row = rpc(&mut c, "session.get", json!({"sessionId":id})).await;
        assert_eq!(row["role"], from);
        assert_eq!(r.fake.launches(&id).len(), 1);
        let crew = r.f.crew.clone();
        let count =
            r.f.state
                .db
                .call_result(move |s| {
                    crewd_store::sessions::list_for_crew(s.conn(), &crew).map(|rows| rows.len())
                })
                .await
                .unwrap()
                .unwrap();
        assert_eq!(count, 3);
        drop(c);
        stop.send(()).unwrap();
        tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
        r.close().await;
    }
}
