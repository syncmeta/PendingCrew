//! Public bidirectional transfer through actual Unix RPC and attached session identity.
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

// Fresh public offers must expose the real committed organization event identity.
async fn request_seq(f: &Fixture, public: &str) -> String {
    let number = public[1..].parse::<i64>().unwrap();
    f.state
        .db
        .call_result(move |store| {
            let core: crewd_work::WorkId = crewd_store::work_organization::core_for_reference(
                store.conn(),
                crewd_store::object_refs::ObjectKind::Plan,
                number,
            )?
            .unwrap()
            .parse()
            .unwrap();
            let mut q = store
                .conn()
                .prepare("SELECT body FROM organization_work_events ORDER BY seq DESC")?;
            let bodies = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for body in bodies {
                let event: crewd_work::Event = serde_json::from_str(&body)?;
                if matches!(event.kind,crewd_work::EventKind::TransferRequested{id,..} if id==core)
                {
                    return Ok::<_, crewd_collab::CollabError>(event.seq.to_string());
                }
            }
            panic!("actual transfer request must have durable event");
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn real_owner_offer_and_cross_crew_target_accept_keep_original_note_history_and_cas() {
    let f = Fixture::new().await;
    let b = f.add_crew("transfer B", None).await;
    f.add_session(&b, "transfer-target", Role::Worker, "target")
        .await;
    f.add_session(&b, "transfer-outsider", Role::Captain, "outsider captain")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let mut target = attach(&f, &socket, "transfer-target").await;
    let mut foreign = attach(&f, &socket, "transfer-outsider").await;
    let made=rpc(&mut human,"work.create",json!({"crewId":f.crew,"title":"actual crosscrew transfer","owner":f.worker,"goal":"LONG_GOAL_NEVER_TRANSFER_RECEIPT","acceptance":"LONG_ACCEPTANCE_NEVER_TRANSFER_RECEIPT"})).await;
    let history_before = rpc(&mut human, "work.history", json!({"id":made["id"]})).await;
    let before = snapshot(&f).await;
    assert!(call(&mut human,"work.transferRequest",json!({"id":made["id"],"expectedRevision":made["revision"],"to":"transfer-target","note":"human cannot fake owner intent"})).await.is_err());
    assert_eq!(snapshot(&f).await, before);
    let note = format!("handoff context {}", "full original detail ".repeat(100))
        .trim()
        .to_owned();
    let offered=tool(&mut owner,"work_item_transfer_request",json!({"id":made["id"],"expected_revision":made["revision"],"to":"transfer-target","note":note})).await;
    assert_eq!(offered["owner"], f.worker, "offer is not acceptance");
    let seq = request_seq(&f, made["id"].as_str().unwrap()).await;
    assert_eq!(offered["pendingTransfer"]["to"], "transfer-target");
    assert_eq!(offered["pendingTransfer"]["requestSeq"], seq);
    assert!(
        offered["pendingTransfer"].get("note").is_none(),
        "compact pending offer must omit full note"
    );

    for field in ["goal", "acceptance", "evidence"] {
        assert!(
            offered.get(field).is_none(),
            "transfer MCP response remains compact"
        );
    }
    assert!(
        !offered.to_string().contains(&note),
        "offer response must not reinject long handoff detail"
    );
    let full = rpc(&mut human, "work.get", json!({"id":made["id"]})).await;
    assert_eq!(full["owner"], f.worker);
    assert_eq!(full["pendingTransfer"]["to"], "transfer-target");
    assert_eq!(full["pendingTransfer"]["requestedBy"], f.worker);
    assert_eq!(full["pendingTransfer"]["note"], note);
    assert_eq!(full["pendingTransfer"]["requestSeq"], seq);
    let history = rpc(&mut human, "work.history", json!({"id":made["id"]})).await;
    assert_eq!(
        history["total"], history_before["total"],
        "offer alone does not change owner history"
    );
    let before = snapshot(&f).await;
    assert!(
        call(
            &mut human,
            "work.transferAccept",
            json!({"id":made["id"],"expectedRevision":full["revision"]})
        )
        .await
        .is_err()
    );
    assert_eq!(snapshot(&f).await, before);
    let denied=rpc(&mut foreign,"agent.call",json!({"tool":"work_item_transfer_accept","args":{"id":made["id"],"expected_revision":full["revision"]}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    // Attached transport cannot use default Human RPC authority to cancel an offer.
    let mut facts = f.state.hub.subscribe_collab();
    let mut events = f.state.hub.subscribe();
    for (method, args) in [
        (
            "work.transferRequest",
            json!({"id":made["id"],"expectedRevision":full["revision"],"to":"transfer-target","note":"spoof"}),
        ),
        (
            "work.transferAccept",
            json!({"id":made["id"],"expectedRevision":full["revision"]}),
        ),
        (
            "work.transferWithdraw",
            json!({"id":made["id"],"expectedRevision":full["revision"]}),
        ),
    ] {
        let before = snapshot(&f).await;
        assert!(call(&mut foreign, method, args).await.is_err());
        assert_eq!(
            snapshot(&f).await,
            before,
            "attached ordinary RPC cannot borrow Human transfer rights"
        );
        assert!(matches!(
            facts.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
    // Ordinary owner progress changes current revision but must not silently cancel the offer.
    let report=tool(&mut owner,"work_item_update",json!({"id":made["id"],"expected_revision":full["revision"],"note":"ordinary progress while offer pending"})).await;
    let before = snapshot(&f).await;
    let stale=rpc(&mut target,"agent.call",json!({"tool":"work_item_transfer_accept","args":{"id":made["id"],"expected_revision":full["revision"]}})).await;
    assert_eq!(stale["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    let accepted = tool(
        &mut target,
        "work_item_transfer_accept",
        json!({"id":made["id"],"expected_revision":report["revision"]}),
    )
    .await;
    assert_eq!(accepted["owner"], "transfer-target");
    let full = rpc(&mut human, "work.get", json!({"id":made["id"]})).await;
    assert!(full["pendingTransfer"].is_null());
    let history = rpc(&mut human, "work.history", json!({"id":made["id"]})).await;
    assert_eq!(
        history["total"].as_u64().unwrap(),
        history_before["total"].as_u64().unwrap() + 1
    );
    let entry = &history["items"][0];
    assert_eq!(entry["from"], f.worker);
    assert_eq!(entry["to"], "transfer-target");
    assert_eq!(entry["by"]["id"], "transfer-target");
    assert_eq!(entry["how"], "transfer");
    assert_eq!(entry["note"], note);
    let before = snapshot(&f).await;
    let denied=rpc(&mut owner,"agent.call",json!({"tool":"work_item_update","args":{"id":made["id"],"expected_revision":full["revision"],"note":"old owner cannot report after accepted transfer"}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    drop((human, owner, target, foreign));
    close_socket(task, stop).await;
}

#[tokio::test]
async fn withdrawing_or_declining_an_offer_keeps_owner_and_only_real_object_authority() {
    let f = Fixture::new().await;
    let b = f.add_crew("decline B", None).await;
    f.add_session(&b, "decline-target", Role::Worker, "target")
        .await;
    f.add_session(&b, "decline-outsider", Role::Captain, "outsider")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let mut target = attach(&f, &socket, "decline-target").await;
    let mut captain = attach(&f, &socket, &f.captain).await;
    let mut outsider = attach(&f, &socket, "decline-outsider").await;
    for by in ["owner", "target", "captain", "human"] {
        let made = rpc(
            &mut human,
            "work.create",
            json!({"crewId":f.crew,"title":format!("withdraw by {by}"),"owner":f.worker}),
        )
        .await;
        let history = rpc(&mut human, "work.history", json!({"id":made["id"]})).await;
        let offered=tool(&mut owner,"work_item_transfer_request",json!({"id":made["id"],"expected_revision":made["revision"],"to":"decline-target","note":"target may decline"})).await;
        let before = snapshot(&f).await;
        let denied=rpc(&mut outsider,"agent.call",json!({"tool":"work_item_transfer_withdraw","args":{"id":made["id"],"expected_revision":offered["revision"]}})).await;
        assert_eq!(denied["isError"], true);
        assert_eq!(snapshot(&f).await, before);
        let params = json!({"id":made["id"],"expected_revision":offered["revision"]});
        match by {
            "owner" => {
                tool(&mut owner, "work_item_transfer_withdraw", params).await;
            }
            "target" => {
                tool(&mut target, "work_item_transfer_withdraw", params).await;
            }
            "captain" => {
                tool(&mut captain, "work_item_transfer_withdraw", params).await;
            }
            _ => {
                rpc(
                    &mut human,
                    "work.transferWithdraw",
                    json!({"id":made["id"],"expectedRevision":offered["revision"]}),
                )
                .await;
            }
        }
        let full = rpc(&mut human, "work.get", json!({"id":made["id"]})).await;
        assert_eq!(full["owner"], f.worker);
        assert!(full["pendingTransfer"].is_null());
        assert_eq!(
            rpc(&mut human, "work.history", json!({"id":made["id"]})).await["total"],
            history["total"]
        );
        let before = snapshot(&f).await;
        let old=rpc(&mut target,"agent.call",json!({"tool":"work_item_transfer_accept","args":{"id":made["id"],"expected_revision":offered["revision"]}})).await;
        assert_eq!(old["isError"], true);
        assert_eq!(snapshot(&f).await, before);
    }
    drop((human, owner, target, captain, outsider));
    close_socket(task, stop).await;
}

#[tokio::test]
async fn real_transfer_notification_sql_failure_and_frozen_target_roll_back_all_effects() {
    let f = Fixture::new().await;
    let b = f.add_crew("frozen target B", None).await;
    f.add_session(&b, "frozen-transfer-target", Role::Worker, "target")
        .await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&f, &socket, &f.worker).await;
    let mut target = attach(&f, &socket, "frozen-transfer-target").await;
    let made = rpc(
        &mut human,
        "work.create",
        json!({"crewId":f.crew,"title":"atomic transfer offer","owner":f.worker}),
    )
    .await;
    f.state.db.call(|s|s.conn().execute_batch("CREATE TRIGGER fail_transfer_wake BEFORE INSERT ON wakes WHEN NEW.session_id='frozen-transfer-target' BEGIN SELECT RAISE(ABORT,'synthetic transfer notification failure'); END;")).await.unwrap().unwrap();
    let before = snapshot(&f).await;
    let failed=rpc(&mut owner,"agent.call",json!({"tool":"work_item_transfer_request","args":{"id":made["id"],"expected_revision":made["revision"],"to":"frozen-transfer-target","note":"must rollback pending offer"}})).await;
    assert_eq!(failed["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    f.state
        .db
        .call(|s| s.conn().execute_batch("DROP TRIGGER fail_transfer_wake;"))
        .await
        .unwrap()
        .unwrap();
    let offered=tool(&mut owner,"work_item_transfer_request",json!({"id":made["id"],"expected_revision":made["revision"],"to":"frozen-transfer-target","note":"actual durable offer"})).await;
    rpc(&mut human, "crew.remove", json!({"crewId":b})).await;
    let before = snapshot(&f).await;
    let denied=rpc(&mut target,"agent.call",json!({"tool":"work_item_transfer_accept","args":{"id":made["id"],"expected_revision":offered["revision"]}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(snapshot(&f).await, before);
    let full = rpc(&mut human, "work.get", json!({"id":made["id"]})).await;
    assert_eq!(full["owner"], f.worker);
    assert_eq!(full["pendingTransfer"]["to"], "frozen-transfer-target");
    drop((human, owner, target));
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
struct TransferActorRig {
    f: Fixture,
    h: Handle,
    fake: RelayStandIn,
    actor: Option<Actor>,
    forward: JoinHandle<()>,
    owner: String,
}
impl Drop for TransferActorRig {
    fn drop(&mut self) {
        if let Some(a) = &self.actor {
            a.abort_handle().abort();
        }
        self.forward.abort();
    }
}
impl TransferActorRig {
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

async fn actual_offer(
    r: &TransferActorRig,
    human: &mut RpcClient,
    owner: &mut RpcClient,
) -> (Value, wakes::WakeRow) {
    let crew = r.f.me(&r.f.worker).await.crew_id;
    let made = rpc(
        human,
        "work.create",
        json!({"crewId":crew,"title":"actual target offer","owner":r.f.worker}),
    )
    .await;
    let before: Vec<_> = r.rows().await.into_iter().map(|w| w.id).collect();
    let offered=tool(owner,"work_item_transfer_request",json!({"id":made["id"],"expected_revision":made["revision"],"to":r.owner,"note":"LONG_TRANSFER_DETAIL_MUST_BE_ON_DEMAND_ONLY repeated repeated repeated"})).await;
    let fresh: Vec<_> = r
        .rows()
        .await
        .into_iter()
        .filter(|w| !before.contains(&w.id))
        .collect();
    assert_eq!(
        fresh.len(),
        1,
        "one actual offer creates one durable target notification"
    );
    (offered, fresh[0].clone())
}
#[tokio::test]
async fn offered_work_notification_actually_reaches_target_and_report_does_not_retire_offer() {
    let r = TransferActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&r.f, &socket, &r.f.worker).await;
    let (offer, row) = actual_offer(&r, &mut human, &mut owner).await;
    tool(&mut owner,"work_item_update",json!({"id":offer["id"],"expected_revision":offer["revision"],"note":"normal report while offer waits"})).await;
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
    assert!(sent[0].contains(offer["id"].as_str().unwrap()));
    assert!(!sent[0].contains("LONG_TRANSFER_DETAIL_MUST_BE_ON_DEMAND_ONLY"));
    assert_eq!(
        r.settle(row.id, wakes::WakeState::Inflight).await.attempts,
        1
    );
    r.fake.emit(
        &r.owner,
        SessionEvent::Delivery {
            id: r.fake.last_send_id(&r.owner),
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    r.settle(row.id, wakes::WakeState::Delivered).await;
    let current = rpc(&mut human, "work.get", json!({"id":offer["id"]})).await;
    assert_eq!(current["owner"], r.f.worker);
    assert_eq!(current["pendingTransfer"]["to"], r.owner);
    drop((human, owner));
    close_socket(task, stop).await;
    r.close().await;
}
#[tokio::test]
async fn actual_rendered_offer_cannot_claim_after_withdraw_accept_or_source_freeze() {
    for mode in ["withdraw", "accept", "freeze"] {
        let r = TransferActorRig::new().await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = r.f.spawn_owned_socket(stopping).await;
        let socket = r.f.data_dir().join("crewd.sock");
        let mut human = RpcClient::connect(&socket).await;
        let mut owner = attach(&r.f, &socket, &r.f.worker).await;
        let mut target = attach(&r.f, &socket, &r.owner).await;
        let (offer, row) = actual_offer(&r, &mut human, &mut owner).await;
        let mut gate = r.f.caller_gate(&r.owner, CallerStage::WakeRendered);
        r.fake.emit(&r.owner, SessionEvent::Working(false));
        let rendered = timeout(LIMIT, gate.entered()).await.unwrap();
        assert!(rendered.text.contains(offer["id"].as_str().unwrap()));
        match mode {
            "withdraw" => {
                tool(
                    &mut owner,
                    "work_item_transfer_withdraw",
                    json!({"id":offer["id"],"expected_revision":offer["revision"]}),
                )
                .await;
            }
            "accept" => {
                tool(
                    &mut target,
                    "work_item_transfer_accept",
                    json!({"id":offer["id"],"expected_revision":offer["revision"]}),
                )
                .await;
            }
            _ => {
                rpc(
                    &mut human,
                    "crew.remove",
                    json!({"crewId":r.f.me(&r.f.worker).await.crew_id}),
                )
                .await;
            }
        }
        gate.release();
        let dropped = r.settle(row.id, wakes::WakeState::Dropped).await;
        assert_eq!(dropped.attempts, 0);
        r.h.latches().await.unwrap();
        assert!(
            r.fake.sent(&r.owner).is_empty(),
            "{mode}: obsolete offer reached target"
        );
        assert_eq!(r.fake.launches(&r.owner).len(), 1);
        drop((human, owner, target));
        close_socket(task, stop).await;
        r.close().await;
    }
}
#[tokio::test]
async fn same_payload_reoffered_work_has_distinct_request_identity_and_only_new_offer_delivers() {
    let r = TransferActorRig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = r.f.spawn_owned_socket(stopping).await;
    let socket = r.f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let mut owner = attach(&r.f, &socket, &r.f.worker).await;
    let (first, old) = actual_offer(&r, &mut human, &mut owner).await;
    let withdrawn = tool(
        &mut owner,
        "work_item_transfer_withdraw",
        json!({"id":first["id"],"expected_revision":first["revision"]}),
    )
    .await;
    let second=tool(&mut owner,"work_item_transfer_request",json!({"id":first["id"],"expected_revision":withdrawn["revision"],"to":r.owner,"note":"LONG_TRANSFER_DETAIL_MUST_BE_ON_DEMAND_ONLY repeated repeated repeated"})).await;
    let rows = r.rows().await;
    let fresh: Vec<_> = rows.iter().filter(|w| w.id != old.id).collect();
    let first_seq = first["pendingTransfer"]["requestSeq"].as_str().unwrap();
    let second_seq = second["pendingTransfer"]["requestSeq"].as_str().unwrap();
    assert_ne!(
        first_seq, second_seq,
        "new same-payload offer has distinct real request event"
    );
    assert_eq!(
        old.source,
        format!(
            "work-transfer:{}:{}",
            first["id"].as_str().unwrap(),
            first_seq
        )
    );
    assert_eq!(
        fresh[0].source,
        format!(
            "work-transfer:{}:{}",
            second["id"].as_str().unwrap(),
            second_seq
        )
    );

    assert_eq!(fresh.len(), 1);
    let new_id = fresh[0].id;
    assert_ne!(
        fresh[0].source, old.source,
        "withdraw/reoffer same payload must use distinct real request identity"
    );
    r.working(false).await;
    r.settle(old.id, wakes::WakeState::Dropped).await;
    timeout(LIMIT, async {
        while r.fake.sent(&r.owner).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let sent = r.fake.sent(&r.owner);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains(second["id"].as_str().unwrap()));
    r.settle(new_id, wakes::WakeState::Inflight).await;
    r.fake.emit(
        &r.owner,
        SessionEvent::Delivery {
            id: r.fake.last_send_id(&r.owner),
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    r.settle(new_id, wakes::WakeState::Delivered).await;
    assert_eq!(r.fake.sent(&r.owner).len(), 1);
    drop((human, owner));
    close_socket(task, stop).await;
    r.close().await;
}
