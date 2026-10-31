//! Human-created Admin role is preserved through the real socket and Core.
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
fn close_args(p: &Value) -> Value {
    json!({"id":p["id"],"expected_revision":p["revision"],"kind":"done","evidence":{"kind":"text","value":"verified fixture evidence"},"note":"factual admin closure"})
}
#[tokio::test]
async fn human_admin_start_preserves_role_without_worker_ext_or_captain_replacement() {
    let r = Rig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let before = rpc(&mut human, "crew.get", json!({"crewId":r.f.crew})).await;
    let made = rpc(&mut human, "session.start", start(&r.f.crew, "admin")).await;
    assert_eq!(
        made["role"], "admin",
        "Human Admin request must not silently become worker"
    );
    assert!(made["ext"].is_null());
    let id = made["id"].as_str().unwrap().to_owned();
    let durable = rpc(&mut human, "session.get", json!({"sessionId":id})).await;
    assert_eq!(durable["role"], "admin");
    assert!(durable["ext"].is_null());
    let after = rpc(&mut human, "crew.get", json!({"crewId":r.f.crew})).await;
    assert_eq!(after["captainSessionId"], before["captainSessionId"]);
    tokio::time::timeout(LIMIT, async {
        while r.fake.launches(&id).is_empty() {
            r.h.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut admin = attach(&r.f, &socket, &id).await;
    let p = rpc(
        &mut human,
        "work.create",
        json!({"crewId":r.f.crew,"title":"other owner's work","owner":r.f.worker}),
    )
    .await;
    let done = tool(&mut admin, "work_item_close", close_args(&p)).await;
    assert_eq!(done["status"], "done");
    let owned = rpc(
        &mut human,
        "work.create",
        json!({"crewId":r.f.crew,"title":"must not forcibly assign","owner":r.f.worker}),
    )
    .await;
    let denied=rpc(&mut admin,"agent.call",json!({"tool":"work_item_assign","args":{"id":owned["id"],"expected_revision":owned["revision"],"owner":r.f.captain,"note":"admin lacks forced assignment"}})).await;
    assert_eq!(denied["isError"], true);
    let other = r.f.add_crew("separate admin scope", None).await;
    let foreign = rpc(
        &mut human,
        "work.create",
        json!({"crewId":other,"title":"foreign work","owner":r.f.worker}),
    )
    .await;
    let denied = rpc(
        &mut admin,
        "agent.call",
        json!({"tool":"work_item_close","args":close_args(&foreign)}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    let unchanged = rpc(&mut human, "work.get", json!({"id":foreign["id"]})).await;
    assert_eq!(unchanged["revision"], foreign["revision"]);
    drop((human, admin));
    close_socket(task, stop).await;
    r.close().await;
}
#[tokio::test]
async fn attached_worker_cannot_create_admin_via_human_rpc_public_command_or_agent_tool() {
    let r = Rig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut worker = attach(&r.f, &socket, &r.f.worker).await;
    let before = snapshot(&r.f).await;
    let direct = call(&mut worker, "session.start", start(&r.f.crew, "admin")).await;
    assert!(
        matches!(&direct, Err((code, _)) if *code == crewd_proto::RpcError::FORBIDDEN),
        "attached worker borrowed Human session.start authority: {direct:?}"
    );
    assert_eq!(snapshot(&r.f).await, before);
    let command=call(&mut worker,"command.submit",json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":"synthetic","runtimeBindingId":"synthetic","command":{"operation":"session.start","params":start(&r.f.crew,"admin")}})).await;
    assert_eq!(command.unwrap_err().0, crewd_proto::RpcError::FORBIDDEN);
    assert_eq!(snapshot(&r.f).await, before);
    let denied=rpc(&mut worker,"agent.call",json!({"tool":"start_session","args":{"brief":"must not create admin","isolation":false,"role":"admin"}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&r.f).await, before);
    drop(worker);
    close_socket(task, stop).await;
    r.close().await;
}
