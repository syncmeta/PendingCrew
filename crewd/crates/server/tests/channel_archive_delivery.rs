//! Production actor delivery boundary for explicit archive and restore.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;
const LIMIT: Duration = Duration::from_secs(10);
async fn rpc(c: &mut RpcClient, m: &str, p: Value) -> Value {
    tokio::time::timeout(LIMIT, c.call(m, p))
        .await
        .unwrap_or_else(|e| panic!("{m}: {e}"))
        .unwrap_or_else(|e| panic!("{m}: {e:?}"))
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

#[tokio::test]
async fn archive_waits_for_typed_delivery_ack_and_restoration_allows_only_new_messages() {
    use crewd_collab::wakes::{self, WakeState};
    use crewd_exec::{
        delivery::Subject,
        session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
    };
    let r = Rig::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = r.f.spawn_owned_socket(stopping).await;
    let mut human = RpcClient::connect(&r.f.data_dir().join("crewd.sock")).await;
    let made=rpc(&mut human,"session.start",json!({"crewId":r.f.crew,"role":"worker","executor":"claude","brief":"archive delivery acceptance","isolation":false})).await;
    let owner = made["id"].as_str().unwrap().to_owned();
    tokio::time::timeout(LIMIT, async {
        while r.fake.launches(&owner).is_empty() {
            r.h.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    r.fake.emit(
        &owner,
        SessionEvent::Delivery {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    r.fake.emit(&owner, SessionEvent::Working(true));
    r.fake.emit(&owner, SessionEvent::Working(false));
    let ch=rpc(&mut human,"channel.create",json!({"crewId":r.f.crew,"kind":"topic","name":"finished work with delivery","members":[{"kind":"session","id":owner}]})).await["id"].as_str().unwrap().to_owned();
    let w = rpc(
        &mut human,
        "work.create",
        json!({"crewId":r.f.crew,"channelId":ch,"title":"completed scope","requestId":uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    rpc(&mut human,"work.close",json!({"id":w["id"],"expectedRevision":w["revision"],"kind":"done","evidence":{"kind":"text","value":"synthetic completion proof"},"note":"finished"})).await;
    let mut prior_wake = None;
    for turn in 0..2 {
        let body = format!("archive lifecycle message {turn}");
        let sent =
            r.f.human_post(
                &ch,
                &body,
                vec![crewd_proto::Mention {
                    kind: crewd_proto::MentionKind::Session,
                    target_id: Some(owner.clone()),
                }],
            )
            .await;
        let source = wakes::source::message(&sent.id);
        let wake = tokio::time::timeout(LIMIT, async {
            loop {
                r.h.latches().await.unwrap();
                let o = owner.clone();
                let rows =
                    r.f.state
                        .db
                        .call_result(move |s| wakes::list(s.conn(), Some(&o), 100))
                        .await
                        .unwrap()
                        .unwrap();
                if let Some(row) = rows
                    .into_iter()
                    .find(|w| w.source == source && w.state == WakeState::Inflight)
                    && r.fake.sent(&owner).iter().any(|v| v.contains(&body))
                {
                    break row;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let check = rpc(&mut human, "channel.archiveCheck", json!({"channelId":ch})).await;
        assert_eq!(
            check["canArchive"], false,
            "inflight work must not disappear into archive: {check}"
        );
        assert!(check["facts"]["inflightWakes"].as_u64().unwrap() > 0);
        if let Some(id) = prior_wake {
            let old =
                r.f.state
                    .db
                    .call_result(move |s| wakes::by_ids(s.conn(), &[id]))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                old[0].state,
                WakeState::Delivered,
                "restoration must not replay old delivery"
            );
        }
        r.fake.emit(
            &owner,
            SessionEvent::Delivery {
                id: r.fake.last_send_id(&owner),
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        tokio::time::timeout(LIMIT, async {
            loop {
                let id = wake.id;
                let rows =
                    r.f.state
                        .db
                        .call_result(move |s| wakes::by_ids(s.conn(), &[id]))
                        .await
                        .unwrap()
                        .unwrap();
                if rows[0].state == WakeState::Delivered {
                    break;
                }
                r.h.latches().await.unwrap();
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let ready = rpc(&mut human, "channel.archiveCheck", json!({"channelId":ch})).await;
        assert_eq!(
            ready["canArchive"], true,
            "ACK should release archive boundary: {ready}"
        );
        let archived=rpc(&mut human,"channel.archiveVerified",json!({"channelId":ch,"expectedRevision":ready["revision"],"evidence":"reviewed completed work and acknowledged delivery","note":"idle collaboration finished"})).await;
        assert!(archived["channel"]["archivedAt"].is_string());
        let restored=rpc(&mut human,"channel.restoreVerified",json!({"channelId":ch,"expectedRevision":archived["revision"],"note":"new collaboration requested"})).await;
        assert!(restored["channel"]["archivedAt"].is_null());
        prior_wake = Some(wake.id);
        r.fake.emit(&owner, SessionEvent::Working(true));
        r.fake.emit(&owner, SessionEvent::Working(false));
    }
    drop(human);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
    r.close().await;
}
