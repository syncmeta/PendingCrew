//! Generated MCP helper -> actual socket/AgentConn -> Tx/Hub -> DbPort/Core.
//! Executor receipts are explicitly synthetic; no provider, real CLI or hook Progress.
#[allow(dead_code)]
#[path = "support/agent.rs"]
mod support;
use crewd_collab::{causal, cursor_reservations as cr, messages, wakes};
use crewd_exec::{
    delivery::Subject,
    session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
};
use crewd_proto::{Mention, MentionKind, Message};
use crewd_server::{
    AppState,
    agent::{
        port,
        runtime::{AgentConfig, AgentRuntime, Clock},
        testkit::{Fixture, RelayStandIn},
    },
    orchestra::{
        self, Actor, Handle, OrchestraConfig,
        clock::ManualClock,
        core::{CollabFact, Origin, StartSession},
        db::{DbPort, pump_collab},
        exec::Executors,
        hooks::Hooks,
    },
};
use crewd_wake::Ms;
use serde_json::json;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
const LIMIT: Duration = Duration::from_secs(8);
struct Rig {
    f: Option<Fixture>,
    h: Handle,
    exec: RelayStandIn,
    ids: Vec<String>,
    channel: String,
    other: String,
    clients: Vec<support::Mcp>,
    launches: Vec<support::Launch>,
    runtime: tempfile::TempDir,
    home: tempfile::TempDir,
    actor: Option<Actor>,
    bridge: Option<JoinHandle<()>>,
    socket: Option<JoinHandle<()>>,
    stop_socket: Option<oneshot::Sender<()>>,
    facts: mpsc::UnboundedReceiver<CollabFact>,
    progress: Arc<std::sync::atomic::AtomicUsize>,
    post_now: Arc<std::sync::atomic::AtomicI64>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(a) = &self.actor {
            a.abort_handle().abort();
        }
        if let Some(t) = &self.bridge {
            t.abort();
        }
        if let Some(t) = &self.socket {
            t.abort();
        }
    }
}
impl Rig {
    fn state(&self) -> &Arc<AppState> {
        &self.f.as_ref().unwrap().state
    }
    async fn new() -> Self {
        Self::new_with_sessions(3).await
    }
    async fn new_with_sessions(session_count: usize) -> Self {
        let cell = Arc::new(OnceLock::new());
        let post_now = Arc::new(std::sync::atomic::AtomicI64::new(crewd_store::now_ms()));
        let observed_clock = post_now.clone();
        let runtime = AgentRuntime::new(AgentConfig::default()).with_clock(Clock::new(move || {
            observed_clock.load(std::sync::atomic::Ordering::SeqCst)
        }));
        let mut f = Fixture::with_orchestra_runtime(cell.clone(), runtime).await;
        f.crew = f.add_crew("S42 isolated causal crew", None).await;
        let rx = f.state.hub.subscribe_collab();
        let exec = RelayStandIn::default();
        let now = Ms(crewd_store::now_ms() + 60_000);
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(now)),
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
        let (tx, facts) = mpsc::unbounded_channel();
        let sink = h.clone();
        let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = progress.clone();
        let bridge = tokio::spawn(pump_collab(rx, move |fact| {
            if matches!(&fact, CollabFact::Progress { .. }) {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            sink.collab(fact.clone());
            let _ = tx.send(fact);
        }));
        let (stop_socket, stop) = oneshot::channel();
        let socket = f.spawn_owned_socket(stop).await;
        let mut ids = Vec::new();
        for index in 0..session_count {
            ids.push(
                h.start(
                    StartSession {
                        crew_id: f.crew.clone(),
                        captain: index == 0,
                        executor: "claude".into(),
                        brief: "S42 warmup initial brief".into(),
                        title: Some(format!("S42 participant {index}")),
                        model: None,
                        effort: None,
                        isolation: false,
                    },
                    Origin::Human,
                )
                .await
                .unwrap()
                .id,
            );
        }
        let captain = f.me(&ids[0]).await;
        let members = ids
            .iter()
            .map(|id| port::MemberRef::Session(id.clone()))
            .collect::<Vec<_>>();
        let channel = port::create_channel(
            &f.state,
            &captain,
            port::ChannelKind::Topic,
            "S42 inherited route".into(),
            None,
            members.clone(),
        )
        .await
        .unwrap()
        .id;
        let other = port::create_channel(
            &f.state,
            &captain,
            port::ChannelKind::Topic,
            "S42 independent route".into(),
            None,
            members,
        )
        .await
        .unwrap()
        .id;
        let pin = ids.clone();
        f.state
            .collab(move |tx| {
                for id in pin {
                    cr::prepare_unread(tx, &id)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for name in ["cache", "claude-config"] {
            std::fs::create_dir(home.path().join(name)).unwrap();
        }
        Self {
            f: Some(f),
            h,
            exec,
            ids,
            channel,
            other,
            clients: vec![],
            launches: vec![],
            runtime,
            home,
            actor: Some(actor),
            bridge: Some(bridge),
            socket: Some(socket),
            stop_socket: Some(stop_socket),
            facts,
            progress,
            post_now,
        }
    }
    async fn warmup(&mut self) {
        for index in 0..3 {
            let id = self.ids[index].clone();
            timeout(LIMIT, async {
                loop {
                    if self.h.live(&id).await.unwrap().running {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            self.exec.emit(
                &id,
                SessionEvent::Delivery {
                    id: BRIEF_DELIVERY_ID,
                    subject: Subject::Brief,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            self.idle(&id).await;
            let launch = support::launch(
                self.f.as_ref().unwrap(),
                &id,
                &self.runtime.path().join(format!("launch-{index}")),
            );
            let client = support::Mcp::start_isolated(&launch.mcp, self.home.path()).await;
            self.launches.push(launch);
            self.clients.push(client);
        }
    }
    async fn idle(&self, id: &str) {
        self.exec.emit(id, SessionEvent::Working(true));
        timeout(LIMIT, async {
            loop {
                if self.h.live(id).await.unwrap().working {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        self.exec.emit(id, SessionEvent::Working(false));
        timeout(LIMIT, async {
            loop {
                if !self.h.live(id).await.unwrap().working {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn barrier(&mut self, m: &Message) {
        let wanted = CollabFact::MessageAppended {
            channel: m.channel_id.clone(),
            seq: m.seq,
        };
        timeout(LIMIT, async {
            loop {
                if self.facts.recv().await.expect("Hub forwarder closed") == wanted {
                    break;
                }
            }
        })
        .await
        .unwrap();
        self.h.latches().await.unwrap(); // FIFO after real fact entered actor, never fabricated fact
    }
    async fn posted(&mut self, channel: &str, marker: &str) -> Message {
        let (channel, marker) = (channel.to_owned(), marker.to_owned());
        let m = self
            .state()
            .db
            .call(move |s| {
                messages::list(s.conn(), &channel, None, None, 200).map(|ms| {
                    ms.into_iter()
                        .find(|m| m.text == marker)
                        .expect("actual helper committed marker")
                })
            })
            .await
            .unwrap()
            .unwrap();
        self.barrier(&m).await;
        m
    }
    async fn post(&mut self, from: usize, target: usize, channel: &str, marker: &str) -> Message {
        // Advance the existing S61 clock through a real refill interval; admission still runs.
        self.post_now
            .fetch_add(20_000, std::sync::atomic::Ordering::SeqCst);
        let (receipt,error)=self.clients[from].call("post",json!({"channel":channel,"message":marker,"mentions":[{"kind":"session","target_id":self.ids[target]}]})).await;
        assert!(!error, "actual helper post setup refused: {receipt}");
        self.posted(channel, marker).await
    }
    async fn human(&mut self, target: usize, channel: &str, marker: &str) -> Message {
        let m = self
            .f
            .as_ref()
            .unwrap()
            .human_post(
                channel,
                marker,
                vec![Mention {
                    kind: MentionKind::Session,
                    target_id: Some(self.ids[target].clone()),
                }],
            )
            .await;
        self.barrier(&m).await;
        m
    }
    async fn sent(&self, target: usize, marker: &str) -> bool {
        timeout(LIMIT, async {
            loop {
                self.h.latches().await.unwrap();
                if self
                    .exec
                    .sent(&self.ids[target])
                    .iter()
                    .any(|s| s.contains(marker))
                {
                    return true;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or(false)
    }
    async fn acknowledge(&self, target: usize, m: &Message) {
        let target = self.ids[target].clone();
        let delivery = self.exec.last_send_id(&target);
        self.exec.emit(
            &target,
            SessionEvent::Delivery {
                id: delivery,
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        let msg = m.id.clone();
        let wanted = target.clone();
        timeout(LIMIT, async {
            loop {
                let msg = msg.clone();
                let wanted = wanted.clone();
                let done = self
                    .state()
                    .db
                    .call(move |s| {
                        wakes::for_source(
                            s.conn(),
                            wakes::WakeKind::Message,
                            &wakes::source::message(&msg),
                        )
                        .map(|rs| {
                            rs.iter().any(|r| {
                                r.session == wanted && r.state == wakes::WakeState::Delivered
                            })
                        })
                    })
                    .await
                    .unwrap()
                    .unwrap();
                if done {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        self.idle(&target).await;
    }
    async fn hop(&self, m: &Message) -> Option<Vec<(String, u32)>> {
        let m = m.clone();
        self.state()
            .db
            .call(move |s| {
                let f = match causal::read_message(s.conn(), &m)? {
                    causal::MessageCause::Resolved(f) => f,
                    causal::MessageCause::Unresolved { .. } => return Ok(None),
                };
                Ok::<_, crewd_collab::CollabError>(Some(
                    f.components()
                        .iter()
                        .map(|c| (c.chain_id(), c.hop()))
                        .collect(),
                ))
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn close(mut self) -> Vec<support::McpCloseReceipt> {
        let mut receipts = Vec::new();
        for client in self.clients.drain(..) {
            receipts.push(client.close_observed().await);
        }
        assert!(
            timeout(LIMIT, self.h.shutdown(Duration::from_secs(2)))
                .await
                .unwrap()
        );
        timeout(LIMIT, self.actor.take().unwrap().finished())
            .await
            .unwrap()
            .unwrap();
        let _ = self.stop_socket.take().unwrap().send(());
        timeout(LIMIT, self.socket.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let fixture = self.f.take().unwrap();
        let db = fixture.state.db.clone();
        drop(fixture.state);
        timeout(LIMIT, self.bridge.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
        receipts
    }
}
fn panic_text(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
}

// Catch setup/observation panics while retaining the Rig for explicit normal cleanup.
async fn catch_setup<F: std::future::Future>(
    future: F,
) -> Result<F::Output, Box<dyn std::any::Any + Send>> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(panic) => std::task::Poll::Ready(Err(panic)),
        }
    })
    .await
}

#[tokio::test]
async fn s42_three_party_generated_mcp_chain_is_bounded_and_same_route_independent_message_passes()
{
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let other = rig.other.clone();
        let root = rig.human(0, &channel, "S42_HUMAN_ROOT").await;
        let root_sent = rig.sent(0, &root.text).await;
        if root_sent {
            rig.acknowledge(0, &root).await;
        }
        let root_chain = rig.hop(&root).await;
        let mut hops = Vec::new();
        let mut delivered = 0;
        let mut legacy = Vec::new();
        let mut last = None;
        for hop in 1..=17 {
            let from = (hop - 1) % 3;
            let target = hop % 3;
            let marker = format!("S42_CHAIN_HOP_{hop:02}");
            let m = rig.post(from, target, &channel, &marker).await;
            hops.push(rig.hop(&m).await);
            legacy.push(rig.h.latches().await.unwrap());
            let sent = if hop <= 16 {
                rig.sent(target, &marker).await
            } else {
                // Durable admission diagnostic plus actor FIFO is the causal barrier, not sleep.
                timeout(LIMIT, async {
                    loop {
                        let holds = rig
                            .h
                            .causal_holds(Some(rig.ids[target].clone()))
                            .await
                            .unwrap();
                        if !holds.is_empty()
                            || rig
                                .exec
                                .sent(&rig.ids[target])
                                .iter()
                                .any(|s| s.contains(&marker))
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .ok();
                rig.exec
                    .sent(&rig.ids[target])
                    .iter()
                    .any(|s| s.contains(&marker))
            };
            if sent {
                if hop <= 16 {
                    delivered += 1;
                }
                rig.acknowledge(target, &m).await;
            }
            if hop == 17 {
                last = Some((m, target, sent));
            }
        }
        let (last, target, sent17) = last.expect("all setup stages reached the boundary");
        let holds = rig
            .h
            .causal_holds(Some(rig.ids[target].clone()))
            .await
            .unwrap();
        let lastid = last.id.clone();
        let targetid = rig.ids[target].clone();
        let pending = rig
            .state()
            .db
            .call(move |s| {
                wakes::for_source(
                    s.conn(),
                    wakes::WakeKind::Message,
                    &wakes::source::message(&lastid),
                )
                .map(|rs| {
                    rs.iter()
                        .any(|r| r.session == targetid && r.state == wakes::WakeState::Pending)
                })
            })
            .await
            .unwrap()
            .unwrap();
        // A new Human cause conveyed to the same sender; unrelated channel bypasses old channel HOL.
        let independent = rig.human(1, &other, "S42_INDEPENDENT_HUMAN_ROOT").await;
        let independent_root_sent = rig.sent(1, &independent.text).await;
        if independent_root_sent {
            rig.acknowledge(1, &independent).await;
        }
        let positive = rig.post(1, 2, &other, "S42_SAME_ROUTE_UNRELATED").await;
        let positive_sent = rig.sent(2, &positive.text).await;
        if positive_sent {
            rig.acknowledge(2, &positive).await;
        }
        let independent_chain = rig.hop(&independent).await;
        let positive_chain = rig.hop(&positive).await;
        let progress = rig.progress.load(std::sync::atomic::Ordering::SeqCst);
        let expected_from = rig.ids[1].clone();
        let expected_target = rig.ids[2].clone();
        let notice_chain = root_chain
            .as_ref()
            .and_then(|cs| cs.first())
            .map(|c| c.0.clone());
        let crew = rig.f.as_ref().unwrap().crew.clone();
        let notification_count = rig
            .state()
            .db
            .call(move |s| {
                let home = crewd_collab::crews::home_of(s.conn(), &crew)?;
                let mut q = s
                    .conn()
                    .prepare("SELECT id FROM channels WHERE crew_id=?1 AND kind='human'")?;
                let mut channels = Vec::new();
                for row in q.query_map([home], |r| r.get::<_, String>(0))? {
                    channels.push(row?);
                }
                let mut n = 0;
                for channel in channels {
                    for m in messages::list(s.conn(), &channel, None, None, 200)? {
                        if notice_chain
                            .as_ref()
                            .is_some_and(|key| m.text.contains(key))
                        {
                            n += 1;
                        }
                    }
                }
                Ok::<_, crewd_collab::CollabError>(n)
            })
            .await
            .unwrap()
            .unwrap();
        (
            root_sent,
            delivered,
            root_chain,
            hops,
            sent17,
            pending,
            holds,
            expected_from,
            expected_target,
            legacy,
            notification_count,
            independent_root_sent,
            positive_sent,
            independent_chain,
            positive_chain,
            progress,
        )
    })
    .await;
    if let Err(panic) = &observation {
        eprintln!(
            "S42 setup/observation failed before cleanup: {}",
            panic_text(panic.as_ref())
        );
    }
    let cleanup = match catch_setup(rig.close()).await {
        Ok(receipts) => receipts,
        Err(cleanup_error) => {
            eprintln!(
                "S42 cleanup also failed: {}",
                panic_text(cleanup_error.as_ref())
            );
            match observation {
                Err(original) => std::panic::resume_unwind(original),
                Ok(_) => std::panic::resume_unwind(cleanup_error),
            }
        }
    };
    println!("S42 helper cleanup: {cleanup:?}");
    assert_eq!(cleanup.len(), 3, "three owned actual helpers reaped");
    assert!(
        cleanup
            .iter()
            .all(|r| r.normal && r.exit_code == Some(0) && !r.forced_kill),
        "normal actual helper exit, not fallback cleanup"
    );
    let (
        root_sent,
        delivered,
        root_chain,
        hops,
        sent17,
        pending,
        holds,
        expected_from,
        expected_target,
        legacy,
        notification_count,
        independent_root_sent,
        positive_sent,
        independent_chain,
        positive_chain,
        progress,
    ) = match observation {
        Ok(observation) => observation,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    assert!(root_sent, "S42 initial Human actual body");
    assert_eq!(delivered, 16, "S42 actual chain budget hop16 allowed");
    let root_components = root_chain.expect("S42 actual immutable Human cause");
    assert_eq!(
        root_components.len(),
        1,
        "Human root has one exact qualified component"
    );
    let root_chain = root_components[0].0.clone();
    assert_eq!(root_components[0].1, 0, "initial Human root hop");
    assert_eq!(
        hops.iter()
            .map(|v| v
                .as_ref()
                .and_then(|cs| cs.iter().find(|(key, _)| key == &root_chain))
                .map(|(_, hop)| *hop))
            .collect::<Vec<_>>(),
        (1..=17).map(Some).collect::<Vec<_>>(),
        "exact original Human root component, not maximum among unrelated roots"
    );
    assert!(
        hops.iter().all(|v| v
            .as_ref()
            .is_some_and(|cs| cs.len() == 1 && cs[0].0 == root_chain)),
        "single-root cycle must not hide extra independent components"
    );
    assert!(!sent17 && pending, "S42 hop17 held before claim/send");
    assert!(holds.iter().any(|h| h.chain_id == root_chain
        && h.from == expected_from
        && h.target == expected_target
        && h.hop == 17));
    assert!(
        legacy.iter().all(Vec::is_empty),
        "existing Edge/Session guard masks this causal oracle"
    );
    assert_eq!(
        notification_count, 1,
        "one durable human notice per exact chain-route episode"
    );
    assert!(
        independent_root_sent && positive_sent,
        "same origin/target unrelated channel must still convey"
    );
    let independent_components = independent_chain.expect("independent Human cause resolved");
    assert_eq!(
        independent_components.len(),
        1,
        "independent Human root exact component"
    );
    let independent_key = independent_components[0].0.clone();
    assert_eq!(independent_components[0].1, 0);
    assert_ne!(independent_key, root_chain);
    let positive_components = positive_chain.expect("independent descendant resolved");
    assert_eq!(
        positive_components,
        vec![(independent_key, 1)],
        "same-route positive inherits exact independent Human root without old component"
    );
    assert_eq!(progress, 0, "ToolProgress cannot mask causal enforcement");
}

// S42 actual-caller variants. Append-only: accepted baseline and Rig setup remain byte-identical.
impl Rig {
    async fn busy_variant(&self, index: usize) {
        let id = &self.ids[index];
        self.exec.emit(id, SessionEvent::Working(true));
        timeout(LIMIT, async {
            loop {
                if self.h.live(id).await.unwrap().working {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn meta_variant(&self, key: String) -> serde_json::Value {
        self.state()
            .db
            .call(move |s| {
                crewd_store::meta_get(s.conn(), &key).map(|raw| {
                    raw.map(|v| {
                        serde_json::from_str::<serde_json::Value>(&v)
                            .expect("actual typed private envelope")
                    })
                    .unwrap_or(serde_json::Value::Null)
                })
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn basis_variant(&self, index: usize) -> serde_json::Value {
        self.meta_variant(format!("causal.session.v1.{}", self.ids[index]))
            .await["value"]
            .clone()
    }
    async fn parent_variant(&self, index: usize) -> serde_json::Value {
        let state = self.basis_variant(index).await;
        let id = state["last_output"]
            .as_str()
            .expect("actual committed parent output");
        self.meta_variant(format!("causal.output.v1.{id}")).await["value"].clone()
    }
    async fn root_variant(&self, m: &Message) -> serde_json::Value {
        let m = m.clone();
        self.state()
            .db
            .call(move |s| match causal::read_message(s.conn(), &m)? {
                causal::MessageCause::Resolved(frontier) => {
                    Ok::<_, crewd_collab::CollabError>(serde_json::to_value(frontier).unwrap())
                }
                causal::MessageCause::Unresolved { .. } => Ok(serde_json::Value::Null),
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn rows_variant(&self, m: &Message) -> Vec<wakes::WakeRow> {
        let source = wakes::source::message(&m.id);
        self.state()
            .db
            .call(move |s| wakes::for_source(s.conn(), wakes::WakeKind::Message, &source))
            .await
            .unwrap()
            .unwrap()
    }
    async fn hook_variant(&self, index: usize, event: &str) -> Option<serde_json::Value> {
        support::fire_isolated(
            &self.launches[index].settings,
            event,
            json!({
                "hook_event_name":event,"tool_name":"mcp__crew__read_channel",
                "tool_input":{},"tool_response":{"synthetic":true},"error":"synthetic failure",
            }),
            self.home.path(),
        )
        .await
    }
    fn spawned_hook_variant(&self, index: usize, event: &str) -> VariantHook {
        let settings = self.launches[index].settings.clone();
        let home = self.home.path().to_owned();
        let event = event.to_owned();
        VariantHook(Some(tokio::spawn(async move {
            support::fire_isolated(
                &settings,
                &event,
                json!({"hook_event_name":event,
                "tool_name":"mcp__crew__read_channel","tool_input":{},
                "tool_response":{"synthetic":true},"error":"synthetic failure"}),
                &home,
            )
            .await
        })))
    }
    async fn cycle_variant(
        &mut self,
        channel: &str,
        prefix: &str,
        last_hop: usize,
    ) -> (
        Message,
        Vec<(Message, Option<Vec<(String, u32)>>, serde_json::Value, bool)>,
    ) {
        let root = self.human(0, channel, &format!("{prefix}_ROOT")).await;
        assert!(self.sent(0, &root.text).await, "actual Human root body");
        self.acknowledge(0, &root).await;
        let mut records = vec![];
        for hop in 1..=last_hop {
            let from = (hop - 1) % 3;
            let target = hop % 3;
            let parent = self.parent_variant(from).await;
            let m = self
                .post(from, target, channel, &format!("{prefix}_HOP_{hop:02}"))
                .await;
            let cause = self.hop(&m).await;
            let sent = if hop <= 16 {
                self.sent(target, &m.text).await
            } else {
                timeout(LIMIT, async {
                    loop {
                        let holds = self
                            .h
                            .causal_holds(Some(self.ids[target].clone()))
                            .await
                            .unwrap();
                        if !holds.is_empty()
                            || self
                                .exec
                                .sent(&self.ids[target])
                                .iter()
                                .any(|t| t.contains(&m.text))
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                self.exec
                    .sent(&self.ids[target])
                    .iter()
                    .any(|t| t.contains(&m.text))
            };
            if sent {
                self.acknowledge(target, &m).await;
            }
            records.push((m, cause, parent, sent));
        }
        (root, records)
    }
}
struct VariantHook(Option<JoinHandle<Option<serde_json::Value>>>);
impl Drop for VariantHook {
    fn drop(&mut self) {
        if let Some(t) = &self.0 {
            t.abort();
        }
    }
}
impl VariantHook {
    async fn finish(mut self) -> Option<serde_json::Value> {
        let out = timeout(LIMIT, self.0.as_mut().unwrap())
            .await
            .expect("owned generated hook joined")
            .unwrap();
        self.0.take();
        out
    }
}
fn variant_context(out: &Option<serde_json::Value>) -> &str {
    out.as_ref()
        .and_then(|v| v["hookSpecificOutput"]["additionalContext"].as_str())
        .unwrap_or("")
}
fn assert_variant_root(m: &Message, frontier: &serde_json::Value) {
    let cs = frontier.as_array().expect("actual resolved Human frontier");
    assert_eq!(cs.len(), 1);
    assert_eq!(
        cs[0]["key"]["root"],
        json!({"kind":"message","id":m.id,"seq":m.seq})
    );
    assert_eq!(cs[0]["key"]["reset"], json!({"kind":"initial"}));
    assert_eq!(cs[0]["hop"], 0);
}
fn assert_variant_parent(parent: &serde_json::Value, index: &str, message: &Message) {
    assert_eq!(parent["status"], "committed");
    assert_eq!(parent["receipt"]["session"], index);
    assert!(
        parent["success"].is_object(),
        "not queued/not-written evidence"
    );
    assert!(
        parent["receipt"]["capture"]["tuples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == message.id
                && t["channel"] == message.channel_id
                && t["seq"] == message.seq),
        "exact actual conveyed parent message tuple"
    );
}
async fn close_variant<T>(rig: Rig, observation: Result<T, Box<dyn std::any::Any + Send>>) -> T {
    if let Err(p) = &observation {
        eprintln!(
            "S42 variant setup/observation RED: {}",
            panic_text(p.as_ref())
        );
    }
    let cleanup = match catch_setup(rig.close()).await {
        Ok(receipts) => receipts,
        Err(error) => match observation {
            Err(original) => std::panic::resume_unwind(original),
            Ok(_) => std::panic::resume_unwind(error),
        },
    };
    println!("S42 variant actual helper cleanup: {cleanup:?}");
    assert_eq!(cleanup.len(), 3);
    assert!(
        cleanup
            .iter()
            .all(|r| r.normal && r.exit_code == Some(0) && !r.forced_kill)
    );
    match observation {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

#[tokio::test]
async fn s42_thirty_qualified_human_roots_complete_short_chains_with_old_guards_intact() {
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let mut rounds = vec![];
        for round in 0..30 {
            let (root, descendants) = rig
                .cycle_variant(&channel, &format!("S42_INDEPENDENT_{round:02}"), 3)
                .await;
            let root_meta = rig.root_variant(&root).await;
            let root_chain = rig.hop(&root).await;
            let legacy = rig.h.latches().await.unwrap();
            let held = rig.h.causal_holds(None).await.unwrap();
            // Consume remaining visible prefixes through real passive generated hooks, so
            // old channel leftovers cannot masquerade as a new root's ancestry next round.
            let mut drains = vec![];
            for index in 0..3 {
                drains.push(rig.hook_variant(index, "PostToolUse").await);
            }
            rounds.push((
                root,
                root_meta,
                root_chain,
                descendants,
                legacy,
                held,
                drains,
            ));
        }
        (
            rounds,
            rig.ids.clone(),
            rig.progress.load(std::sync::atomic::Ordering::SeqCst),
        )
    })
    .await;
    let (rounds, ids, progress) = close_variant(rig, observation).await;
    assert_eq!(rounds.len(), 30);
    let mut keys = std::collections::BTreeSet::new();
    let mut messages = std::collections::BTreeSet::new();
    for (root, meta, chain, descendants, legacy, held, _drains) in rounds {
        assert_variant_root(&root, &meta);
        assert!(messages.insert(root.id.clone()));
        let chain = chain.expect("qualified root");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].1, 0);
        let key = chain[0].0.clone();
        assert!(keys.insert(key.clone()), "no global root reuse");
        assert!(
            legacy.is_empty(),
            "old Edge/Session defense must not mask case"
        );
        assert!(held.is_empty());
        let mut parent_message = root;
        for (hop, (m, cause, parent, sent)) in descendants.into_iter().enumerate() {
            assert!(sent);
            assert_eq!(cause, Some(vec![(key.clone(), (hop + 1) as u32)]));
            assert_variant_parent(&parent, &ids[hop % 3], &parent_message);
            parent_message = m;
        }
    }
    assert_eq!(
        progress, 0,
        "no ToolProgress/WorkItem masking; only actual passive hooks"
    );
}

#[tokio::test]
async fn s42_direct_human_listener_auto_is_immune_but_session_descendant_counts_and_hol_remains() {
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let other = rig.other.clone();
        let (oldroot, cycle) = rig.cycle_variant(&channel, "S42_IMMUNITY_OLD", 17).await;
        let old_chain = rig.hop(&oldroot).await;
        let old_meta = rig.root_variant(&oldroot).await;
        let held_id = cycle.last().unwrap().0.id.clone();
        let (text, error) = rig.clients[2]
            .call("channel_listen", json!({"channel":other}))
            .await;
        assert!(!error, "actual listener tool setup: {text}");
        let suffix = rig.human(2, &channel, "S42_DIRECT_HUMAN_HOL_SUFFIX").await;
        let suffix_meta = rig.root_variant(&suffix).await;
        let positive = rig
            .human(1, &other, "S42_HUMAN_DIRECT_AND_LISTENER_AUTO")
            .await;
        let direct = rig.sent(1, &positive.text).await;
        if direct {
            rig.acknowledge(1, &positive).await;
        }
        let listener = rig.sent(2, &positive.text).await;
        if listener {
            rig.acknowledge(2, &positive).await;
        }
        let rows = rig.rows_variant(&positive).await;
        let qualified = rig.root_variant(&positive).await;
        let rootkey = rig.hop(&positive).await;
        let parent = rig.parent_variant(1).await;
        let child = rig
            .post(1, 2, &other, "S42_IMMUNE_ROOT_SESSION_DESCENDANT")
            .await;
        let child_sent = rig.sent(2, &child.text).await;
        if child_sent {
            rig.acknowledge(2, &child).await;
        }
        let descendant = rig.hop(&child).await;
        let holds = rig.h.causal_holds(Some(rig.ids[2].clone())).await.unwrap();
        let suffix_rows = rig.rows_variant(&suffix).await;
        let last = cycle.last().unwrap().0.clone();
        let held_rows = rig.rows_variant(&last).await;
        let wrong = rig
            .exec
            .sent(&rig.ids[2])
            .iter()
            .any(|body| body.contains(&suffix.text) || body.contains(&last.text));
        let legacy = rig.h.latches().await.unwrap();
        (
            oldroot,
            old_meta,
            old_chain,
            cycle,
            suffix,
            suffix_meta,
            positive,
            qualified,
            rootkey,
            rows,
            direct,
            listener,
            parent,
            child_sent,
            descendant,
            holds,
            suffix_rows,
            held_rows,
            wrong,
            legacy,
            rig.ids.clone(),
            held_id,
            rig.progress.load(std::sync::atomic::Ordering::SeqCst),
        )
    })
    .await;
    let (
        oldroot,
        old_meta,
        old_chain,
        cycle,
        suffix,
        suffix_meta,
        positive,
        qualified,
        rootkey,
        rows,
        direct,
        listener,
        parent,
        child_sent,
        descendant,
        holds,
        suffix_rows,
        held_rows,
        wrong,
        legacy,
        ids,
        _held_id,
        progress,
    ) = close_variant(rig, observation).await;
    assert_variant_root(&oldroot, &old_meta);
    let oldkey = old_chain.unwrap()[0].0.clone();
    assert_eq!(cycle.len(), 17);
    for (hop, (_, cause, _, sent)) in cycle.iter().enumerate() {
        assert_eq!(*cause, Some(vec![(oldkey.clone(), (hop + 1) as u32)]));
        assert_eq!(*sent, hop < 16);
    }
    assert_variant_root(&suffix, &suffix_meta);
    assert_variant_root(&positive, &qualified);
    assert!(direct && listener && child_sent);
    assert!(rows.iter().any(|r| r.session == ids[1]
        && r.priority == wakes::WakePriority::Human
        && r.state == wakes::WakeState::Delivered));
    assert!(rows.iter().any(|r| r.session == ids[2]
        && r.priority == wakes::WakePriority::Auto
        && r.state == wakes::WakeState::Delivered));
    let newkey = rootkey.unwrap()[0].0.clone();
    assert_ne!(newkey, oldkey);
    assert_eq!(descendant, Some(vec![(newkey.clone(), 1)]));
    assert_variant_parent(&parent, &ids[1], &positive);
    assert!(holds.iter().any(|h| h.chain_id == oldkey));
    assert!(!holds.iter().any(|h| h.chain_id == newkey));
    assert!(
        !wrong,
        "true Human immunity does not bypass unsafe same-channel prefix"
    );
    assert!(
        suffix_rows
            .iter()
            .any(|r| r.session == ids[2] && r.state == wakes::WakeState::Pending)
    );
    assert!(
        held_rows
            .iter()
            .any(|r| r.session == ids[2] && r.state == wakes::WakeState::Pending)
    );
    assert!(legacy.is_empty());
    assert_eq!(progress, 0);
}

async fn hook_cause_variant(event: &str) {
    use crewd_server::agent::testkit::{CallerStage, InjectionOutcome};
    let mut rig = Rig::new().await;
    let observation=catch_setup(async {
        rig.warmup().await;let channel=rig.channel.clone();let other=rig.other.clone();
        rig.busy_variant(0).await;rig.busy_variant(1).await;
        // Human source has a captain wake, but this reader has ZERO wake ids for its prefix.
        let root=rig.human(0,&channel,"S42_ZERO_READER_WAKE_ROOT").await;
        let rows=rig.rows_variant(&root).await;let root_meta=rig.root_variant(&root).await;let rootkey=rig.hop(&root).await;
        let first=rig.hook_variant(1,event).await;let parent=rig.parent_variant(1).await;
        let child=rig.post(1,2,&other,"S42_HOOK_ACTUAL_DESCENDANT").await;
        assert!(rig.sent(2,&child.text).await);rig.acknowledge(2,&child).await;let childcause=rig.hop(&child).await;
        let before_empty=rig.basis_variant(1).await;let empty=rig.hook_variant(1,event).await;let after_empty=rig.basis_variant(1).await;
        // Two REAL read writers invalidate two preparations without sleeps or fake CAS.
        let stale=rig.human(0,&channel,"S42_STALE_PREPARED_PREFIX").await;
        let stale_other=rig.human(0,&other,"S42_STALE_SECOND_CHANNEL_PREFIX").await;
        let mut gate=rig.f.as_ref().unwrap().caller_gate(&rig.ids[1],CallerStage::HookPrepared);
        let hook=rig.spawned_hook_variant(1,event);let prepared=timeout(LIMIT,gate.entered()).await.unwrap();
        let before_stale=rig.basis_variant(1).await;
        let(read,error)=rig.clients[1].call("read_channel",json!({"channel":channel,"limit":200})).await;
        // First pause registration was removed at entry, so rearm before letting attempt0 go.
        let mut second_gate=rig.f.as_ref().unwrap().caller_gate(&rig.ids[1],CallerStage::HookPrepared);
        gate.release();let prepared_second=timeout(LIMIT,second_gate.entered()).await.unwrap();
        let(read_second,error_second)=rig.clients[1].call("read_channel",json!({"channel":other,"limit":200})).await;
        second_gate.release();let stale_out=hook.finish().await;let after_stale=rig.basis_variant(1).await;
        // Real wake claim while a real hook is prepared. Pending output has no success receipt.
        let busy=rig.human(1,&channel,"S42_BUSY_ACTUAL_CLAIM_PREFIX").await;
        let mut gate=rig.f.as_ref().unwrap().caller_gate(&rig.ids[1],CallerStage::HookPrepared);
        let hook=rig.spawned_hook_variant(1,event);let busy_prepared=timeout(LIMIT,gate.entered()).await.unwrap();
        let before_busy=rig.basis_variant(1).await;rig.idle(&rig.ids[1]).await;assert!(rig.sent(1,&busy.text).await);
        let claimed=rig.basis_variant(1).await;let active=claimed["active"].as_str().expect("actual wake active parent").to_owned();
        let output=rig.meta_variant(format!("causal.output.v1.{active}")).await["value"].clone();
        gate.release();let busy_out=hook.finish().await;let after_busy=rig.basis_variant(1).await;
        let cursor_busy=port::cursor(rig.state(),&rig.ids[1],&channel).await.unwrap();
        rig.acknowledge(1,&busy).await;let actual_parent=rig.parent_variant(1).await;
        let busy_child=rig.post(1,2,&other,"S42_BUSY_REAL_DELIVERED_DESCENDANT").await;
        assert!(rig.sent(2,&busy_child.text).await);rig.acknowledge(2,&busy_child).await;
        let busykey=rig.hop(&busy).await;let busycause=rig.hop(&busy_child).await;
        // Exact owned SQLite failure at real commit; no causal/meta rows are seeded or rewritten.
        let failure=rig.human(0,&channel,"S42_HOOK_CURSOR_COMMIT_ABORT").await;
        let before_error=rig.basis_variant(1).await;let cursor_before=port::cursor(rig.state(),&rig.ids[1],&channel).await.unwrap();
        let sid=rig.ids[1].replace('\'',"''");let ch=channel.replace('\'',"''");let seq=failure.seq;
        let sql=format!("CREATE TRIGGER s42_variant_cursor_abort BEFORE UPDATE ON read_cursors WHEN NEW.reader='{sid}' AND NEW.channel_id='{ch}' AND NEW.seq>={seq} BEGIN SELECT RAISE(ABORT,'S42 owned hook cursor fault'); END;");
        rig.state().db.call(move |s|s.conn().execute_batch(&sql)).await.unwrap().unwrap();
        let failed=rig.hook_variant(1,event).await;
        rig.state().db.call(|s|s.conn().execute_batch("DROP TRIGGER s42_variant_cursor_abort")).await.unwrap().unwrap();
        let after_error=rig.basis_variant(1).await;let cursor_after=port::cursor(rig.state(),&rig.ids[1],&channel).await.unwrap();
        let completions=rig.f.as_ref().unwrap().injection_completions(&rig.ids[1]);
        (root,rows,root_meta,rootkey,first,parent,childcause,before_empty,empty,after_empty,stale,stale_other,prepared,prepared_second,before_stale,read,error,read_second,error_second,stale_out,after_stale,
            busy,busy_prepared,before_busy,output,busy_out,after_busy,cursor_busy,actual_parent,busykey,busycause,failed,before_error,after_error,cursor_before,cursor_after,
            completions,rig.ids.clone(),rig.progress.load(std::sync::atomic::Ordering::SeqCst))
    }).await;
    let (
        root,
        rows,
        root_meta,
        rootkey,
        first,
        parent,
        childcause,
        before_empty,
        empty,
        after_empty,
        stale,
        stale_other,
        prepared,
        prepared_second,
        before_stale,
        read,
        error,
        read_second,
        error_second,
        stale_out,
        after_stale,
        busy,
        busy_prepared,
        before_busy,
        output,
        busy_out,
        after_busy,
        cursor_busy,
        actual_parent,
        busykey,
        busycause,
        failed,
        before_error,
        after_error,
        cursor_before,
        cursor_after,
        completions,
        ids,
        progress,
    ) = close_variant(rig, observation).await;
    assert_variant_root(&root, &root_meta);
    assert!(
        !rows.iter().any(|r| r.session == ids[1]),
        "actual zero-id reader output"
    );
    assert_eq!(variant_context(&first).matches(&root.text).count(), 1);
    assert_eq!(
        first.as_ref().unwrap()["hookSpecificOutput"]["hookEventName"],
        event
    );
    assert_variant_parent(&parent, &ids[1], &root);
    assert!(parent["receipt"]["owner"].get("Hook").is_some());
    assert_eq!(
        parent["success"]["kind"]["Hook"]["operation"],
        parent["receipt"]["owner"]["Hook"]["operation"]
    );
    assert_eq!(childcause, Some(vec![(rootkey.unwrap()[0].0.clone(), 1)]));
    assert!(empty.is_none());
    assert_eq!(before_empty, after_empty);
    assert!(
        prepared
            .targets
            .contains(&(stale.channel_id.clone(), stale.seq))
    );
    assert!(prepared.text.contains(&stale.text));
    assert!(!error && read.contains(&stale.text));
    assert!(
        prepared
            .targets
            .contains(&(stale_other.channel_id.clone(), stale_other.seq))
    );
    assert!(
        prepared_second
            .targets
            .contains(&(stale_other.channel_id.clone(), stale_other.seq))
    );
    assert!(
        !prepared_second
            .targets
            .contains(&(stale.channel_id.clone(), stale.seq))
    );
    assert!(!error_second && read_second.contains(&stale_other.text));
    assert!(stale_out.is_none());
    assert_eq!(before_stale, after_stale);
    assert!(
        busy_prepared
            .targets
            .contains(&(busy.channel_id.clone(), busy.seq))
    );
    assert_eq!(output["status"], "pending");
    assert!(output["success"].is_null());
    assert!(busy_out.is_none());
    assert_eq!(before_busy["basis"], after_busy["basis"]);
    assert_eq!(before_busy["last_output"], after_busy["last_output"]);
    assert!(cursor_busy.is_some_and(|n| n < busy.seq));
    assert_variant_parent(&actual_parent, &ids[1], &busy);
    assert_eq!(busycause, Some(vec![(busykey.unwrap()[0].0.clone(), 1)]));
    assert!(failed.is_none());
    assert_eq!(before_error, after_error);
    assert_eq!(cursor_before, cursor_after);
    assert_eq!(completions.len(), 5);
    assert_eq!(completions[0].outcome, InjectionOutcome::Committed);
    assert_eq!(completions[1].outcome, InjectionOutcome::Empty);
    assert_eq!(completions[4].outcome, InjectionOutcome::InternalError);
    assert_eq!(
        completions[2].outcome,
        InjectionOutcome::Stale,
        "actual second CAS invalidated by second real read"
    );
    assert!(matches!(
        completions[3].outcome,
        InjectionOutcome::Empty | InjectionOutcome::Busy
    ));
    assert!(
        completions
            .iter()
            .all(|c| c.session == ids[1] && c.event == event)
    );
    assert_eq!(progress, 0);
}
#[tokio::test]
async fn s42_generated_posttool_and_failure_zero_id_causes_and_real_negative_competition() {
    // One named case, two freshly isolated event subcases; not two asserted cargo test results.
    hook_cause_variant("PostToolUse").await;
    hook_cause_variant("PostToolUseFailure").await;
}

// Depends on the separately reviewed passive attempt journal and existing append-only variants.
// SOURCE CANDIDATE: this ordering has not yet been compiled or executed.
async fn hook_real_busy_variant(event: &str) {
    use crewd_server::agent::testkit::{CallerStage, InjectionCommitOutcome, InjectionOutcome};
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let other = rig.other.clone();
        rig.busy_variant(2).await;
        let root = rig.human(0, &channel, "S42_BUSY_JOURNAL_ROOT").await;
        assert!(rig.sent(0, &root.text).await, "actual root body");
        rig.acknowledge(0, &root).await;
        let root_cause = rig.hop(&root).await.unwrap();
        let mut forks = vec![];
        let mut peers = vec![];
        for hop in 1..=16 {
            let from = (hop - 1) % 2;
            let target = hop % 2;
            // A genuinely separate @C output satisfies accepted old on_post semantics.
            // It is passive MCP chatter, NOT ToolProgress or a forged causal reset.
            let fork = rig
                .post(from, 2, &channel, &format!("S42_BUSY_FORK_{hop:02}"))
                .await;
            forks.push((fork.clone(), rig.hop(&fork).await));
            let peer = rig
                .post(from, target, &channel, &format!("S42_BUSY_PEER_{hop:02}"))
                .await;
            let cause = rig.hop(&peer).await;
            assert!(
                rig.sent(target, &peer.text).await,
                "actual peer body before causal threshold"
            );
            rig.acknowledge(target, &peer).await;
            peers.push((peer, cause));
        }
        // A's last successful parent is actual hop16. C has consumed no fork.
        let before = rig.basis_variant(2).await;
        let cursor_before = port::cursor(rig.state(), &rig.ids[2], &channel)
            .await
            .unwrap();
        let previous_completions = rig
            .f
            .as_ref()
            .unwrap()
            .injection_completions(&rig.ids[2])
            .len();
        rig.f.as_ref().unwrap().observe_injection_attempts();
        let mut gate = rig
            .f
            .as_ref()
            .unwrap()
            .caller_gate(&rig.ids[2], CallerStage::HookPrepared);
        let hook = rig.spawned_hook_variant(2, event);
        let prepared = timeout(LIMIT, gate.entered()).await.unwrap();
        assert!(
            forks.iter().any(|(m, _)| prepared.text.contains(&m.text)),
            "real safe prefix prepared"
        );
        // This NEW unsafe row lies strictly beyond the paused prefix's captured endpoint.
        let over = rig.post(0, 2, &channel, "S42_BUSY_NEW_HOP17").await;
        let over_cause = rig.hop(&over).await;
        let positive = rig
            .human(2, &other, "S42_BUSY_INDEPENDENT_REAL_CLAIM")
            .await;
        rig.idle(&rig.ids[2]).await;
        assert!(
            rig.sent(2, &positive.text).await,
            "actual independent body/claim, not a DB-seeded lease"
        );
        let holds = rig.h.causal_holds(Some(rig.ids[2].clone())).await.unwrap();
        let claimed = rig.basis_variant(2).await;
        let active = claimed["active"]
            .as_str()
            .expect("actual competing owner")
            .to_owned();
        let pending = rig.meta_variant(format!("causal.output.v1.{active}")).await["value"].clone();
        gate.release();
        let stdout = hook.finish().await;
        let after = rig.basis_variant(2).await;
        let cursor_after = port::cursor(rig.state(), &rig.ids[2], &channel)
            .await
            .unwrap();
        let journal = rig.f.as_ref().unwrap().injection_attempts(&rig.ids[2]);
        let completions = rig.f.as_ref().unwrap().injection_completions(&rig.ids[2]);
        let wrong = rig
            .exec
            .sent(&rig.ids[2])
            .iter()
            .any(|body| body.contains("S42_BUSY_FORK_") || body.contains(&over.text));
        // Positive receipt is emitted only AFTER the hook has returned. It cannot authorize
        // the old channel or be used to retroactively manufacture a successful hook output.
        rig.acknowledge(2, &positive).await;
        let actual_parent = rig.parent_variant(2).await;
        let descendant = rig
            .post(2, 1, &other, "S42_BUSY_POSITIVE_AFTER_DELIVERED")
            .await;
        assert!(rig.sent(1, &descendant.text).await);
        rig.acknowledge(1, &descendant).await;
        let positive_cause = rig.hop(&positive).await;
        let descendant_cause = rig.hop(&descendant).await;
        let legacy = rig.h.latches().await.unwrap();
        let progress = rig.progress.load(std::sync::atomic::Ordering::SeqCst);
        (
            rig.ids.clone(),
            root_cause,
            forks,
            peers,
            prepared,
            over,
            over_cause,
            holds,
            pending,
            before,
            after,
            cursor_before,
            cursor_after,
            stdout,
            journal,
            completions,
            previous_completions,
            wrong,
            positive,
            positive_cause,
            descendant_cause,
            actual_parent,
            legacy,
            progress,
        )
    })
    .await;
    let (
        ids,
        root_cause,
        forks,
        peers,
        prepared,
        over,
        over_cause,
        holds,
        pending,
        before,
        after,
        cursor_before,
        cursor_after,
        stdout,
        journal,
        completions,
        previous_completions,
        wrong,
        positive,
        positive_cause,
        descendant_cause,
        actual_parent,
        legacy,
        progress,
    ) = close_variant(rig, observation).await;
    let key = &root_cause[0].0;
    assert_eq!(root_cause, vec![(key.clone(), 0)]);
    for (i, (_, cause)) in forks.iter().enumerate() {
        assert_eq!(*cause, Some(vec![(key.clone(), (i + 1) as u32)]));
    }
    for (i, (_, cause)) in peers.iter().enumerate() {
        assert_eq!(*cause, Some(vec![(key.clone(), (i + 1) as u32)]));
    }
    assert_eq!(over_cause, Some(vec![(key.clone(), 17)]));
    assert!(
        prepared.targets.iter().all(|(_, end)| *end < over.seq),
        "old prepared endpoint excludes new unsafe row"
    );
    assert!(
        holds
            .iter()
            .any(|h| h.chain_id == *key && h.from == ids[0] && h.target == ids[2] && h.hop == 17),
        "real exact route hold"
    );
    assert_eq!(pending["status"], "pending");
    assert!(
        pending["success"].is_null(),
        "no successful competing-owner receipt at hook return"
    );
    assert_eq!(pending["receipt"]["session"], ids[2]);
    assert!(
        pending["receipt"]["capture"]["tuples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == positive.id)
    );
    assert_eq!(
        cursor_before, cursor_after,
        "Busy cannot advance paused channel cursor"
    );
    for field in ["epoch", "applied", "basis", "last_output"] {
        assert_eq!(
            before[field], after[field],
            "refusal cannot activate a new hook cause: {field}"
        );
    }
    assert!(
        stdout.is_none(),
        "real Busy/Empty refusal must print no old context"
    );
    assert!(
        !wrong,
        "held body cannot leak through independent-channel claim"
    );
    assert_eq!(
        journal.len(),
        1,
        "Empty reprepare is not a second commit or a fabricated Busy"
    );
    assert_eq!(journal[0].session, ids[2]);
    assert_eq!(journal[0].event, event);
    assert_eq!(journal[0].attempt, 0);
    assert_eq!(journal[0].targets, prepared.targets);
    assert_eq!(
        journal[0].outcome,
        InjectionCommitOutcome::Busy,
        "actual commit return, not inferred Empty"
    );
    assert_eq!(completions.len(), previous_completions + 1);
    assert_eq!(
        completions.last().unwrap().outcome,
        InjectionOutcome::Empty,
        "unchanged single retry honestly finds no eligible context"
    );
    let pkey = &positive_cause.as_ref().unwrap()[0].0;
    assert_ne!(pkey, key);
    assert_eq!(positive_cause, Some(vec![(pkey.clone(), 0)]));
    assert_eq!(descendant_cause, Some(vec![(pkey.clone(), 1)]));
    assert_variant_parent(&actual_parent, &ids[2], &positive);
    assert!(
        legacy.is_empty(),
        "do not disable or hide accepted old guards"
    );
    assert_eq!(progress, 0, "no ToolProgress used to make Busy observable");
}

#[tokio::test]
async fn s42_generated_hook_records_real_busy_before_competing_owner_empty_reprepare() {
    hook_real_busy_variant("PostToolUse").await;
    hook_real_busy_variant("PostToolUseFailure").await;
}

// S42 actual caller + deterministic transport loss; provider success stays synthetic RelayStandIn.
#[tokio::test]
async fn s42_generated_caller_lagged_rescan_dedup_and_file_snapshot_reopen() {
    let retained = tempfile::tempdir().unwrap();
    let path = retained.path().join("actual-caller-causal.sqlite");
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let root = rig.human(0, &channel, "S42_LAGGED_ROOT").await;
        assert!(rig.sent(0, &root.text).await, "actual root body setup");
        rig.acknowledge(0, &root).await;
        let root_cause = rig.hop(&root).await;
        // Explicitly stop this owned transport, not Core/DB or human intent.
        let bridge = rig.bridge.take().unwrap();
        bridge.abort();
        let stopped = bridge.await.unwrap_err();
        assert!(stopped.is_cancelled(), "owned bridge pause observed");
        // The replacement is owned immediately, including setup-panic cleanup. It
        // subscribes only on resume (or sender drop), so it cannot rescue the lost body.
        let (resume, paused) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (facts_tx, facts) = mpsc::unbounded_channel();
        let state = rig.state().clone();
        let sink = rig.h.clone();
        let progress = rig.progress.clone();
        rig.bridge = Some(tokio::spawn(async move {
            let _ = paused.await;
            let rx = state.hub.subscribe_collab();
            drop(state);
            let _ = ready_tx.send(());
            pump_collab(rx, move |fact| {
                if matches!(&fact, CollabFact::Progress { .. }) {
                    progress.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                sink.collab(fact.clone());
                let _ = facts_tx.send(fact);
            })
            .await;
        }));
        rig.facts = facts;
        let mut actual_changes = rig.state().hub.subscribe_collab();
        rig.post_now
            .fetch_add(20_000, std::sync::atomic::Ordering::SeqCst);
        let marker = "S42_LOST_ACTUAL_CALLER_BODY";
        let (receipt, error) = rig.clients[0]
            .call(
                "post",
                json!({"channel":channel,
            "message":marker,"mentions":[{"kind":"session","target_id":rig.ids[1]}]}),
            )
            .await;
        assert!(!error, "actual helper committed post: {receipt}");
        let ch = channel.clone();
        let m = rig
            .state()
            .db
            .call(move |s| {
                messages::list(s.conn(), &ch, None, None, 200).map(|ms| {
                    ms.into_iter()
                        .find(|m| m.text == marker)
                        .expect("actual committed marker")
                })
            })
            .await
            .unwrap()
            .unwrap();
        let (message_change, badge_change) = timeout(LIMIT, async {
            let mut message = None;
            let mut badge = None;
            while message.is_none() || badge.is_none() {
                let change = actual_changes.recv().await.unwrap();
                match &change {
                    crewd_collab::Change::MessageAppended { message_id, .. }
                        if message_id == &m.id =>
                    {
                        message = Some(change)
                    }
                    crewd_collab::Change::BadgeChanged { channel_id } if channel_id == &channel => {
                        badge = Some(change)
                    }
                    _ => {}
                }
            }
            (message.unwrap(), badge.unwrap())
        })
        .await
        .unwrap();
        // Capacity-two replay of *actual committed* notifications is an explicit transport
        // fault fixture. The only routable notification is lost; survivors are inert badges.
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        tx.send(message_change.clone()).unwrap();
        for _ in 0..4 {
            tx.send(badge_change.clone()).unwrap();
        }
        drop(tx);
        let mut pumped = Vec::new();
        timeout(
            LIMIT,
            pump_collab(rx, |fact| {
                rig.h.collab(fact.clone());
                pumped.push(fact);
            }),
        )
        .await
        .unwrap();
        rig.h.latches().await.unwrap(); // actual actor FIFO after Lagged
        let recovered = rig.sent(1, marker).await;
        if recovered {
            rig.acknowledge(1, &m).await;
        }
        let first_sends = rig
            .exec
            .sent(&rig.ids[1])
            .iter()
            .filter(|s| s.contains(marker))
            .count();
        let first_rows = rig.rows_variant(&m).await;
        let first_cause = rig.hop(&m).await;
        // Duplicate exact message tuple follows the original DB acknowledgement.
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        for _ in 0..3 {
            tx.send(message_change.clone()).unwrap();
        }
        drop(tx);
        timeout(LIMIT, pump_collab(rx, |fact| rig.h.collab(fact)))
            .await
            .unwrap();
        rig.h.latches().await.unwrap();
        let repeated_sends = rig
            .exec
            .sent(&rig.ids[1])
            .iter()
            .filter(|s| s.contains(marker))
            .count();
        let repeated_rows = rig.rows_variant(&m).await;
        let repeated_cause = rig.hop(&m).await;
        // Reconnect owned real Hub bridge before the next actual helper call.
        resume.send(()).unwrap();
        timeout(LIMIT, ready_rx).await.unwrap().unwrap();
        let child = rig
            .post(1, 2, &channel, "S42_AFTER_LAGGED_DESCENDANT")
            .await;
        let child_sent = rig.sent(2, &child.text).await;
        if child_sent {
            rig.acknowledge(2, &child).await;
        }
        let child_cause = rig.hop(&child).await;
        let snapshot_path = path.to_str().unwrap().to_owned();
        rig.state()
            .db
            .call(move |s| s.conn().execute("VACUUM INTO ?1", [snapshot_path]))
            .await
            .unwrap()
            .unwrap();
        (
            root,
            m,
            child,
            root_cause,
            first_cause,
            repeated_cause,
            child_cause,
            pumped,
            recovered,
            first_sends,
            repeated_sends,
            first_rows,
            repeated_rows,
            child_sent,
            rig.progress.load(std::sync::atomic::Ordering::SeqCst),
            rig.ids[1].clone(),
        )
    })
    .await;
    // All actual helpers, actor, socket and DB have normal cleanup before main oracles.
    let (
        root,
        m,
        child,
        root_cause,
        first_cause,
        repeated_cause,
        child_cause,
        pumped,
        recovered,
        first_sends,
        repeated_sends,
        first_rows,
        repeated_rows,
        child_sent,
        progress,
        target,
    ) = close_variant(rig, observation).await;
    assert_eq!(
        pumped,
        vec![CollabFact::Lagged],
        "S42 actual bounded transport overflow, no surviving routing notification"
    );
    assert!(
        recovered,
        "S42 Lagged rescanned actual committed unread caller body"
    );
    assert_eq!(first_sends, 1, "S42 Lagged exact body sent once");
    assert_eq!(
        repeated_sends, first_sends,
        "S42 repeated tuple must not resend acknowledged body"
    );
    assert_eq!(
        repeated_rows, first_rows,
        "S42 repeated tuple must not create or mutate wake rows"
    );
    assert_eq!(
        first_rows
            .iter()
            .filter(|w| w.session == target && w.state == wakes::WakeState::Delivered)
            .count(),
        1,
        "exact target actual wake Delivered once"
    );
    let key = root_cause.as_ref().unwrap()[0].0.clone();
    assert_eq!(first_cause, Some(vec![(key.clone(), 1)]));
    assert_eq!(
        repeated_cause, first_cause,
        "S42 notification replay cannot add hop"
    );
    assert!(
        child_sent,
        "actual caller after recovered body must still send"
    );
    assert_eq!(
        child_cause,
        Some(vec![(key, 2)]),
        "S42 actual descendant retains original cause after rescan"
    );
    assert_eq!(progress, 0, "rescan must not fabricate Work reset");
    // This is a closed/reopened actual DB snapshot, NOT a second daemon boot.
    let reopened = crewd_store::Store::open(&path).unwrap();
    for (message, expected) in [(root, root_cause), (m, first_cause), (child, child_cause)] {
        assert_eq!(
            messages::get(reopened.conn(), &message.id).unwrap(),
            message
        );
        let found = match causal::read_message(reopened.conn(), &message).unwrap() {
            causal::MessageCause::Resolved(f) => Some(
                f.components()
                    .iter()
                    .map(|v| (v.chain_id(), v.hop()))
                    .collect::<Vec<_>>(),
            ),
            causal::MessageCause::Unresolved { .. } => None,
        };
        assert_eq!(
            found, expected,
            "S42 real caller stamp survives file-backed snapshot reopen without new hop"
        );
    }
}

// S42 follow-up caller candidates: actual local human socket and real work storage boundary.
impl Rig {
    async fn human_rpc_variant(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let stream =
            tokio::net::UnixStream::connect(self.f.as_ref().unwrap().data_dir().join("crewd.sock"))
                .await
                .unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut lines = BufReader::new(rd).lines();
        let request = crewd_proto::Request {
            id: 1,
            method: method.into(),
            params,
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        timeout(LIMIT, wr.write_all(&bytes)).await.unwrap().unwrap();
        let line = timeout(LIMIT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("actual human RPC response");
        let result = match serde_json::from_str::<crewd_proto::ServerFrame>(&line).unwrap() {
            crewd_proto::ServerFrame::Response(response) => {
                assert_eq!(response.id, 1);
                response.into_result()
            }
            crewd_proto::ServerFrame::Event(event) => {
                panic!("unsubscribed human connection received {event:?}")
            }
        };
        wr.shutdown().await.unwrap();
        drop(wr);
        assert!(
            timeout(LIMIT, lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .is_none(),
            "human EOF completes normal connection"
        );
        result
    }
    async fn recovery_records_variant(&self) -> Vec<serde_json::Value> {
        self.state()
            .db
            .call(|s| {
                let mut q = s.conn().prepare(
                    "SELECT value FROM meta WHERE substr(key,1,length(?1))=?1 ORDER BY key",
                )?;
                let rows = q.query_map(["causal.recovery.v1."], |r| r.get::<_, String>(0))?;
                let values = rows
                    .map(|r| {
                        r.map(|raw| {
                            serde_json::from_str::<serde_json::Value>(&raw)
                                .expect("strict owned recovery envelope")["value"]
                                .clone()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, crewd_store::StoreError>(values)
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn additional_topic_variant(&self, name: &str) -> String {
        let captain = self.f.as_ref().unwrap().me(&self.ids[0]).await;
        port::create_channel(
            self.state(),
            &captain,
            port::ChannelKind::Topic,
            name.into(),
            None,
            self.ids
                .iter()
                .map(|id| port::MemberRef::Session(id.clone()))
                .collect(),
        )
        .await
        .unwrap()
        .id
    }
    async fn progress_receipts_variant(&mut self, index: usize, count: usize) {
        for ordinal in 0..count {
            let mut observed = vec![];
            let receipt = timeout(LIMIT, async { loop {
                let fact = self.facts.recv().await.expect("actual Hub receipt stream");
                let wanted = matches!(&fact, CollabFact::Progress { session } if session == &self.ids[index]);
                observed.push(fact);
                if wanted { break; }
            }}).await;
            eprintln!(
                "S42 actual progress receipt session={} ordinal={}/{} success={} total={} facts={observed:?} basis={}",
                self.ids[index],
                ordinal + 1,
                count,
                receipt.is_ok(),
                self.progress.load(std::sync::atomic::Ordering::SeqCst),
                self.basis_variant(index).await
            );
            receipt.expect("actual matching Progress fact must arrive within unchanged LIMIT");
        }
        self.h.latches().await.unwrap(); // FIFO actor receipt after actual Hub progress
    }
    async fn same_tx_work_items_variant(&mut self, index: usize) -> (i64, u64, u64) {
        let session = self.ids[index].clone();
        let (now, first, second) = self
            .state()
            .collab(move |tx| {
                let key = format!("causal.session.v1.{session}");
                let now = tx.now_ms();
                crewd_collab::sessions::note_progress(
                    tx,
                    &session,
                    crewd_collab::ProgressKind::WorkItem,
                )?;
                let first =
                    crewd_store::meta_get(tx.conn(), &key)?.expect("actual first work epoch");
                crewd_collab::sessions::note_progress(
                    tx,
                    &session,
                    crewd_collab::ProgressKind::WorkItem,
                )?;
                let second =
                    crewd_store::meta_get(tx.conn(), &key)?.expect("actual second work epoch");
                Ok((now, first, second))
            })
            .await
            .unwrap();
        // Both real work calls independently increment the durable epoch. Tx::note
        // coalesces identical session/kind notifications within this single commit;
        // one actual Progress receipt is a rescan signal, not a count of work calls.
        self.progress_receipts_variant(index, 1).await;
        let epoch = |raw: &str| {
            serde_json::from_str::<serde_json::Value>(raw).unwrap()["value"]["epoch"]
                .as_u64()
                .unwrap()
        };
        (now, epoch(&first), epoch(&second))
    }
    async fn completed_tool_hook_variant(
        &self,
        index: usize,
        tool: &str,
        input: serde_json::Value,
        response: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;
        // Consume the real tools/call result, not a reconstructed display string.
        assert!(
            response["content"].is_array(),
            "actual MCP completed result: {response}"
        );
        assert!(!response["isError"].as_bool().unwrap_or(false));
        let payload = json!({"hook_event_name":"PostToolUse", "tool_name":tool,
            "tool_input":input, "tool_response":response});
        let before = self
            .f
            .as_ref()
            .unwrap()
            .hook_observations(&self.ids[index])
            .len();
        let basis_before = self.basis_variant(index).await;
        let entries = self.launches[index].settings["hooks"]["PostToolUse"]
            .as_array()
            .unwrap();
        let commands = entries
            .iter()
            .filter(|entry| support::matches(entry, tool))
            .flat_map(|entry| entry["hooks"].as_array().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            commands.len(),
            1,
            "one actual generated completed-tool helper"
        );
        assert_eq!(commands[0]["type"], "command");
        let mut child = tokio::process::Command::new("/bin/sh")
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("CLAUDE_CONFIG_DIR", self.home.path().join("claude-config"))
            .env("SHELL", "/usr/bin/false")
            .args(["-c", commands[0]["command"].as_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(payload.to_string().as_bytes())
            .await
            .unwrap();
        drop(stdin);
        // Same helper wait bound as existing shared support; production QUICK remains5s.
        let out = timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        let observations = self.f.as_ref().unwrap().hook_observations(&self.ids[index]);
        eprintln!(
            "S42 actual completed hook tool={tool} session={} exit={:?} stderr={} before={} after={} basis_before={} basis_after={} progress={}",
            self.ids[index],
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
            before,
            observations.len(),
            basis_before,
            self.basis_variant(index).await,
            self.progress.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(out.status.success(), "actual helper failed");
        assert_eq!(
            observations.len(),
            before + 1,
            "exit0/empty stdout alone is not a completed hook receipt"
        );
        let actual = observations.last().unwrap();
        assert_eq!(actual.event, "PostToolUse");
        assert_eq!(actual.tool_name.as_deref(), Some(tool));
        let text = String::from_utf8(out.stdout).unwrap();
        let text = text.trim();
        if text.is_empty() {
            None
        } else {
            Some(serde_json::from_str(text).expect("actual hook stdout JSON"))
        }
    }
}

#[tokio::test]
async fn s42_actual_human_rpc_recovers_exact_chain_once_and_agents_cannot_recover() {
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone(); let other = rig.other.clone();
        let (root, mut first) = rig.cycle_variant(&channel, "S42_RECOVER_SELECTED", 16).await;
        // C really reads the latest safe prefix BEFORE B's unsafe17 exists. Recovery is
        // exact B->C; it must not also erase an unread A16 component on a different route.
        let safe_prefix = first.last().unwrap().0.clone();
        let (read, error) = rig.clients[2].call("read_channel", json!({"channel":channel,"limit":200})).await;
        assert!(!error && read.contains(&safe_prefix.text), "actual C read must include safe16");
        let read_cursor = port::cursor(rig.state(), &rig.ids[2], &channel).await.unwrap();
        assert_eq!(read_cursor, Some(safe_prefix.seq), "actual read_channel, never a direct cursor seed");
        let parent17 = rig.parent_variant(1).await;
        let row17 = rig.post(1, 2, &channel, "S42_RECOVER_SELECTED_HOP_17").await;
        let cause17 = rig.hop(&row17).await;
        let selected17 = cause17.as_ref().unwrap()[0].0.clone();
        timeout(LIMIT, async { loop {
            if rig.h.causal_holds(Some(rig.ids[2].clone())).await.unwrap().iter()
                .any(|h| h.chain_id == selected17 && h.from == rig.ids[1] && h.target == rig.ids[2]) { break; }
            tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(!rig.exec.sent(&rig.ids[2]).iter().any(|body| body.contains(&row17.text)));
        eprintln!("S42 actual selected prefix cursor={read_cursor:?} retained17={} cause={cause17:?}", row17.seq);
        first.push((row17, cause17, parent17, false));
        let (unrelated, second) = rig.cycle_variant(&other, "S42_RECOVER_UNRELATED", 17).await;
        let first_key = rig.hop(&root).await.unwrap()[0].0.clone();
        let second_key = rig.hop(&unrelated).await.unwrap()[0].0.clone();
        let retained = first.last().unwrap().0.clone(); let retained_other = second.last().unwrap().0.clone();
        let history_before = rig.root_variant(&retained).await;
        // Genuine legacy Edge episode with distinct Human roots: no causal chain exceeds2.
        // No limit/hold/meta seeding; ninth A->B is held by the accepted old Edge defense.
        let third = rig.additional_topic_variant("S42 real legacy control").await;
        let mut legacy_rounds = vec![];
        for round in 1..=9 {
            let fresh = rig.human(0, &third, &format!("S42_LEGACY_HUMAN_{round:02}")).await;
            assert!(rig.sent(0, &fresh.text).await); rig.acknowledge(0, &fresh).await;
            let peer = rig.post(0, 1, &third, &format!("S42_LEGACY_A_B_{round:02}")).await;
            let delivered = rig.sent(1, &peer.text).await;
            if delivered {
                rig.acknowledge(1, &peer).await;
                let back = rig.post(1, 0, &third, &format!("S42_LEGACY_B_A_{round:02}")).await;
                assert!(rig.sent(0, &back.text).await); rig.acknowledge(0, &back).await;
            }
            legacy_rounds.push((rig.hop(&fresh).await, rig.hop(&peer).await, delivered));
            if !delivered { break; }
        }
        let legacy_before = rig.h.latches().await.unwrap();
        let listed = rig.human_rpc_variant("loop.list", serde_json::Value::Null).await.unwrap();
        let holds_before = rig.h.causal_holds(None).await.unwrap();
        let records_before = rig.recovery_records_variant().await;
        let mut denial = vec![];
        for index in 0..3 {
            let tools = rig.clients[index].tools().await;
            for name in ["loop.recover", "loop_recover"] {
                let advertised = tools.iter().any(|t| t["name"] == name);
                let (text, error) = rig.clients[index].call(name, json!({"scope":"chain","chainId":first_key,"from":rig.ids[1],"targetSessionId":rig.ids[2]})).await;
                denial.push((name.to_owned(), advertised, text, error));
            }
        }
        let after_denial = rig.h.causal_holds(None).await.unwrap();
        let records_after_denial = rig.recovery_records_variant().await;
        let missing = rig.human_rpc_variant("loop.recover", json!({"scope":"chain","from":rig.ids[1],"targetSessionId":rig.ids[2]})).await;
        assert!(!holds_before.iter().any(|h| h.chain_id == first_key && h.from == rig.ids[1] && h.target == rig.ids[1]), "wrong-target control names an actually absent route");
        let wrong_target = rig.human_rpc_variant("loop.recover", json!({"scope":"chain","chainId":first_key,"from":rig.ids[1],"targetSessionId":rig.ids[1]})).await.unwrap();
        let wrong_from = rig.human_rpc_variant("loop.recover", json!({"scope":"chain","chainId":first_key,"from":rig.ids[0],"targetSessionId":rig.ids[2]})).await.unwrap();
        let before_exact = rig.h.causal_holds(None).await.unwrap();
        let recovered = rig.human_rpc_variant("loop.recover", json!({"scope":"chain","chainId":first_key,"from":rig.ids[1],"targetSessionId":rig.ids[2]})).await.unwrap();
        assert!(rig.sent(2, &retained.text).await, "actual retained17 row is delivered only after exact human RPC");
        let restored_parent = rig.basis_variant(2).await;
        let active = restored_parent["active"].as_str().expect("actual restored pending parent").to_owned();
        let recovered_output = rig.meta_variant(format!("causal.output.v1.{active}")).await["value"].clone();
        rig.acknowledge(2, &retained).await;
        let records_once = rig.recovery_records_variant().await;
        let repeat = rig.human_rpc_variant("loop.recover", json!({"scope":"chain","chainId":first_key,"from":rig.ids[1],"targetSessionId":rig.ids[2]})).await.unwrap();
        let count_before_scan = rig.exec.sent(&rig.ids[2]).iter().filter(|body| body.contains(&retained.text)).count();
        // Actual Core reload input, not a claim that the Hub really overflowed.
        rig.h.collab(CollabFact::Lagged); rig.h.latches().await.unwrap();
        rig.h.collab(CollabFact::Lagged); rig.h.latches().await.unwrap();
        let records_after_scan = rig.recovery_records_variant().await;
        let count_after_scan = rig.exec.sent(&rig.ids[2]).iter().filter(|body| body.contains(&retained.text)).count();
        let after = rig.h.causal_holds(None).await.unwrap();
        let legacy_after = rig.h.latches().await.unwrap();
        let history_after = rig.root_variant(&retained).await;
        let remaining_rows = rig.rows_variant(&retained_other).await;
        // A fresh real topic avoids the deliberately preserved legacy A/B episode's
        // unread prefix; it does not clear that latch or qualify/drop any inherited root.
        let downstream = rig.additional_topic_variant("S42 exact recovered downstream control").await;
        eprintln!("S42 actual recovered output={recovered_output} holds={after:?} legacy={legacy_after:?}");
        let descendant = rig.post(2, 0, &downstream, "S42_RECOVER_TRUE_DOWNSTREAM").await;
        let downstream_sent = rig.sent(0, &descendant.text).await;
        eprintln!("S42 actual recovered successor sent={downstream_sent} frontier={} holds={:?}",
            rig.root_variant(&descendant).await, rig.h.causal_holds(None).await.unwrap());
        assert!(downstream_sent); rig.acknowledge(0, &descendant).await;
        assert_eq!(legacy_after, rig.h.latches().await.unwrap(), "downstream positive must preserve legacy episode");
        let descendant_frontier = rig.root_variant(&descendant).await;
        let progress = rig.progress.load(std::sync::atomic::Ordering::SeqCst);
        (rig.ids.clone(), first_key, second_key, retained, listed, holds_before, after_denial,
            records_before, records_after_denial, denial, missing, wrong_target, wrong_from,
            before_exact, recovered, repeat, records_once, records_after_scan, after,
            legacy_before, legacy_after, history_before, history_after, recovered_output,
            count_before_scan, count_after_scan, remaining_rows, descendant_frontier,
            legacy_rounds, progress)
    }).await;
    let (
        ids,
        selected,
        unrelated,
        retained,
        listed,
        holds_before,
        after_denial,
        records_before,
        records_after_denial,
        denial,
        missing,
        wrong_target,
        wrong_from,
        before_exact,
        recovered,
        repeat,
        records_once,
        records_after_scan,
        after,
        legacy_before,
        legacy_after,
        history_before,
        history_after,
        recovered_output,
        count_before_scan,
        count_after_scan,
        remaining_rows,
        descendant_frontier,
        legacy_rounds,
        progress,
    ) = close_variant(rig, observation).await;
    assert_ne!(selected, unrelated);
    for key in [&selected, &unrelated] {
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["scope"] == "chain"
                    && row["chainId"] == *key
                    && row["from"] == ids[1]
                    && row["targetSessionId"] == ids[2])
        );
    }
    assert_eq!(holds_before, after_denial);
    assert_eq!(holds_before, before_exact);
    assert!(records_before.is_empty());
    assert_eq!(records_before, records_after_denial);
    for (name, advertised, text, error) in denial {
        assert!(!advertised);
        assert!(
            error && text.contains(&name),
            "actual unknown agent tool receipt: {text}"
        );
    }
    let error = missing.unwrap_err();
    assert_eq!(error.code, crewd_proto::RpcError::INVALID_PARAMS);
    assert_eq!(error.data.unwrap().refusal, "loop.chain_needs_identity");
    assert_eq!(wrong_target, json!({"recovered":false}));
    assert_eq!(wrong_from, json!({"recovered":false}));
    assert_eq!(recovered, json!({"recovered":true}));
    assert_eq!(repeat, json!({"recovered":false}));
    assert_eq!(records_once.len(), 1);
    assert_eq!(records_once, records_after_scan);
    let record = &records_once[0];
    let action = record["action"].as_str().unwrap();
    assert!(!action.is_empty());
    assert_eq!(record["order"], 1);
    assert_eq!(record["selected"]["chain_id"], selected);
    assert_eq!(record["selected"]["from"], ids[1]);
    assert_eq!(record["selected"]["target"], ids[2]);
    assert!(
        record["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tuple| tuple["id"] == retained.id && tuple["seq"] == retained.seq)
    );
    assert!(
        after
            .iter()
            .any(|h| h.chain_id == unrelated && h.from == ids[1] && h.target == ids[2])
    );
    assert!(
        !after
            .iter()
            .any(|h| h.chain_id == selected && h.from == ids[1] && h.target == ids[2])
    );
    assert!(
        !legacy_before.is_empty(),
        "genuine existing defense must have an observed episode"
    );
    assert_eq!(legacy_before, legacy_after);
    assert_eq!(legacy_rounds.len(), 9);
    assert!(
        legacy_rounds[..8]
            .iter()
            .all(|(_, _, delivered)| *delivered)
    );
    assert!(!legacy_rounds[8].2);
    for (_, peer, _) in &legacy_rounds {
        assert!(peer.as_ref().unwrap().iter().all(|(_, hop)| *hop == 1));
    }
    assert_eq!(
        history_before, history_after,
        "human recovery overlays retained cause, never rewrites historical17 stamp"
    );
    assert_eq!(history_before.as_array().unwrap().len(), 1);
    assert_eq!(history_before[0]["hop"], 17);
    assert_eq!(count_before_scan, 1);
    assert_eq!(count_after_scan, 1);
    assert!(
        remaining_rows
            .iter()
            .any(|r| r.session == ids[2] && r.state == wakes::WakeState::Pending)
    );
    assert!(
        recovered_output["receipt"]["capture"]["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |component| component["key"]["root"] == history_before[0]["key"]["root"]
                    && component["key"]["reset"] == json!({"kind":"recovery","action":action})
                    && component["hop"] == 0
            )
    );
    assert!(descendant_frontier.as_array().unwrap().iter().any(
        |component| component["key"]["root"] == history_before[0]["key"]["root"]
            && component["key"]["reset"] == json!({"kind":"recovery","action":action})
            && component["hop"] == 1
    ));
    assert_eq!(progress, 0, "no work event is used as automatic recovery");
}

#[tokio::test]
async fn s42_actual_work_tool_and_same_tx_work_items_reset_once_but_passive_calls_do_not_unlatch() {
    let mut rig = Rig::new().await;
    let observation = catch_setup(async {
        rig.warmup().await;
        let channel = rig.channel.clone();
        let other = rig.other.clone();
        let (_old, _) = rig.cycle_variant(&channel, "S42_WORK_HELD", 17).await;
        let original_holds = rig.h.causal_holds(Some(rig.ids[2].clone())).await.unwrap();
        let (fresh, _) = rig.cycle_variant(&other, "S42_WORK_FRESH", 3).await;
        let initial = rig.basis_variant(0).await;
        let passive = rig.post(0, 1, &other, "S42_PASSIVE_CHAT_HOP4").await;
        assert!(rig.sent(1, &passive.text).await);
        rig.acknowledge(1, &passive).await;
        let passive_cause = rig.hop(&passive).await;
        let read_reply = rig.clients[0]
            .request("tools/call", json!({"name":"read_channel","arguments":{"channel":other,"limit":200}}))
            .await;
        let read = &read_reply["result"];
        assert!(read["content"].is_array() && !read["isError"].as_bool().unwrap_or(false));
        rig.completed_tool_hook_variant(
            0,
            "mcp__crew__read_channel",
            json!({"channel":other,"limit":200}),
            read,
        )
        .await;
        let after_passive = rig.basis_variant(0).await;
        // Fresh independent traffic may legitimately discover additional existing unsafe routes.
        // Snapshot only after passive traffic; retain the ORIGINAL target-C hold separately.
        let holds = rig.h.causal_holds(None).await.unwrap();
        let created_reply = rig.clients[0]
            .request("tools/call", json!({"name":"channel_create","arguments":
                {"name":"S42 actual work tool","purpose":"synthetic work classification"}}))
            .await;
        let created = &created_reply["result"];
        assert!(created["content"].is_array() && !created["isError"].as_bool().unwrap_or(false),
            "real nonpassive tool succeeded: {created}");
        rig.completed_tool_hook_variant(
            0,
            "mcp__crew__channel_create",
            json!({"name":"S42 actual work tool","purpose":"synthetic work classification"}),
            created,
        )
        .await;
        rig.progress_receipts_variant(0, 1).await;
        let tool_state = rig.basis_variant(0).await;
        let observed_output = tool_state["last_output"].as_str().expect("actual inherited output");
        let tool_capture = rig.meta_variant(format!("causal.output.v1.{observed_output}")).await["value"].clone();
        let mut tool_roots = vec![];
        for component in tool_state["basis"].as_array().unwrap() {
            let root = &component["key"]["root"];
            assert_eq!(root["kind"], "message", "this actual capture uses durable message roots");
            let id = root["id"].as_str().unwrap().to_owned();
            let m = rig.state().db.call(move |s| messages::get(s.conn(), &id)).await.unwrap().unwrap();
            tool_roots.push(m);
        }
        eprintln!("S42 actual work producer prior_basis={} completed_capture={} real_root_messages={tool_roots:?}", tool_state["basis"], tool_capture);
        let tool_first = rig.post(0, 1, &other, "S42_TOOL_FIRST_ZERO").await;
        assert!(rig.sent(1, &tool_first.text).await);
        rig.acknowledge(1, &tool_first).await;
        let tool_first_frontier = rig.root_variant(&tool_first).await;
        let tool_second = rig.post(0, 1, &other, "S42_TOOL_SECOND_SAME_ZERO").await;
        assert!(rig.sent(1, &tool_second.text).await);
        rig.acknowledge(1, &tool_second).await;
        let tool_second_frontier = rig.root_variant(&tool_second).await;
        let tool_child = rig.post(1, 2, &other, "S42_TOOL_DOWNSTREAM_ONE").await;
        assert!(rig.sent(2, &tool_child.text).await);
        rig.acknowledge(2, &tool_child).await;
        let tool_child_frontier = rig.root_variant(&tool_child).await;
        let (same_tx_now, epoch_one, epoch_two) = rig.same_tx_work_items_variant(0).await;
        let work_state = rig.basis_variant(0).await;
        let work_first = rig.post(0, 1, &other, "S42_WORKITEM_FIRST_ZERO").await;
        assert!(rig.sent(1, &work_first.text).await);
        rig.acknowledge(1, &work_first).await;
        let work_first_frontier = rig.root_variant(&work_first).await;
        let work_second = rig
            .post(0, 1, &other, "S42_WORKITEM_SECOND_SAME_ZERO")
            .await;
        assert!(rig.sent(1, &work_second.text).await);
        rig.acknowledge(1, &work_second).await;
        let work_second_frontier = rig.root_variant(&work_second).await;
        let work_child = rig.post(1, 2, &other, "S42_WORKITEM_DOWNSTREAM_ONE").await;
        assert!(rig.sent(2, &work_child.text).await);
        rig.acknowledge(2, &work_child).await;
        let work_child_frontier = rig.root_variant(&work_child).await;
        let holds_after = rig.h.causal_holds(None).await.unwrap();
        let history = rig.root_variant(&fresh).await;
        let final_state = rig.basis_variant(0).await;
        let progress = rig.progress.load(std::sync::atomic::Ordering::SeqCst);
        eprintln!("S42 actual work canonical frontiers tool0={tool_first_frontier} tool_repeat0={tool_second_frontier} tool_child1={tool_child_frontier} work_prior={} work0={work_first_frontier} work_repeat0={work_second_frontier} work_child1={work_child_frontier}", work_state["basis"]);
        (
            rig.ids.clone(),
            fresh,
            initial,
            passive_cause,
            after_passive,
            tool_state,
            tool_capture,
            tool_roots,
            tool_first_frontier,
            tool_second_frontier,
            tool_child_frontier,
            same_tx_now,
            epoch_one,
            epoch_two,
            work_state,
            work_first_frontier,
            work_second_frontier,
            work_child_frontier,
            original_holds,
            holds,
            holds_after,
            history,
            final_state,
            progress,
        )
    })
    .await;
    let (
        ids,
        fresh,
        initial,
        passive_cause,
        after_passive,
        tool_state,
        tool_capture,
        tool_roots,
        tool_first_frontier,
        tool_second_frontier,
        tool_child_frontier,
        same_tx_now,
        epoch_one,
        epoch_two,
        work_state,
        work_first_frontier,
        work_second_frontier,
        work_child_frontier,
        original_holds,
        holds,
        holds_after,
        history,
        final_state,
        progress,
    ) = close_variant(rig, observation).await;
    let initial_epoch = initial["epoch"].as_u64().unwrap();
    assert_eq!(after_passive["epoch"], initial_epoch);
    assert!(passive_cause.unwrap().iter().all(|(_, hop)| *hop == 4));
    let tool_epoch = tool_state["epoch"].as_u64().unwrap();
    assert_eq!(tool_epoch, initial_epoch + 1);
    assert_eq!(
        tool_first_frontier, tool_second_frontier,
        "second ordinary post cannot reapply old epoch"
    );
    assert_eq!(
        tool_capture["status"], "committed",
        "actual prior successful hook receipt"
    );
    let captured_roots = tool_capture["receipt"]["capture"]["frontier"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| component["key"]["root"].clone())
        .collect::<Vec<_>>();
    assert!(!captured_roots.is_empty());
    let prior_roots = tool_state["basis"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| component["key"]["root"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        prior_roots, captured_roots,
        "all roots bind actual completed capture, not an obsolete earlier Human root"
    );
    assert_eq!(tool_roots.len(), captured_roots.len());
    for (message, root) in tool_roots.iter().zip(&captured_roots) {
        assert_eq!(
            *root,
            json!({"kind":"message","id":message.id,"seq":message.seq}),
            "every actual root has its exact durable fixture message identity"
        );
    }
    assert_work_frontier_variant(
        &tool_first_frontier,
        &ids[0],
        tool_epoch,
        0,
        &tool_state["basis"],
    );
    assert_work_frontier_variant(
        &tool_child_frontier,
        &ids[0],
        tool_epoch,
        1,
        &tool_state["basis"],
    );
    assert!(same_tx_now > 0);
    assert_eq!(epoch_one, tool_epoch + 1);
    assert_eq!(epoch_two, epoch_one + 1);
    assert_eq!(work_state["epoch"], epoch_two);
    assert_eq!(final_state["epoch"], epoch_two);
    assert_eq!(work_first_frontier, work_second_frontier);
    let work_roots = work_state["basis"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| component["key"]["root"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        work_roots, captured_roots,
        "same-Tx work keeps the complete actual inherited root set"
    );
    assert_work_frontier_variant(
        &work_first_frontier,
        &ids[0],
        epoch_two,
        0,
        &work_state["basis"],
    );
    assert_work_frontier_variant(
        &work_child_frontier,
        &ids[0],
        epoch_two,
        1,
        &work_state["basis"],
    );
    assert_variant_root(&fresh, &history);
    assert!(!original_holds.is_empty());
    for original in original_holds {
        assert!(
            holds.contains(&original) && holds_after.contains(&original),
            "passive traffic and work preserve ORIGINAL durable hold"
        );
    }
    assert_eq!(
        holds, holds_after,
        "real work never auto-recovers durable chain hold"
    );
    assert_eq!(
        progress, 2,
        "one ToolCall notification plus one coalesced same-Tx WorkItem notification; independent epoch increments are checked above"
    );
}

fn assert_work_frontier_variant(
    frontier: &serde_json::Value,
    session: &str,
    epoch: u64,
    hop: u32,
    prior_basis: &serde_json::Value,
) {
    let mut expected = prior_basis
        .as_array()
        .expect("actual inherited canonical basis")
        .clone();
    assert!(!expected.is_empty());
    for component in &mut expected {
        component["key"]["reset"] = json!({"kind":"work","session":session,"epoch":epoch});
        component["hop"] = json!(hop);
    }
    assert_eq!(
        frontier,
        &serde_json::Value::Array(expected),
        "entire actual inherited root set must survive exact qualified work reset and descendant hop; no free/extra/dropped root"
    );
}

// OWN unapplied candidate: 60 controlled executor sessions, 30 independent active work sources.
// Only pair zero uses generated MCP helpers. Other pairs use the genuine Session PostReq
// adapter -> AppState.collab -> Tx/Hub -> DbPort/Core (not fabricated messages/facts/stamps).
impl Rig {
    async fn warmup_work_pairs(&mut self) {
        for index in 0..self.ids.len() {
            let id = self.ids[index].clone();
            timeout(LIMIT, async {
                loop {
                    if self.h.live(&id).await.unwrap().running {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            self.exec.emit(
                &id,
                SessionEvent::Delivery {
                    id: BRIEF_DELIVERY_ID,
                    subject: Subject::Brief,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            self.idle(&id).await;
        }
        for index in 0..2 {
            let launch = support::launch(
                self.f.as_ref().unwrap(),
                &self.ids[index],
                &self
                    .runtime
                    .path()
                    .join(format!("work-pair-launch-{index}")),
            );
            let client = support::Mcp::start_isolated(&launch.mcp, self.home.path()).await;
            self.launches.push(launch);
            self.clients.push(client);
        }
    }
    async fn work_pair_topic(&self, source: usize, members: &[usize], name: &str) -> String {
        let me = self.f.as_ref().unwrap().me(&self.ids[source]).await;
        port::create_channel(
            self.state(),
            &me,
            port::ChannelKind::Topic,
            name.into(),
            None,
            members
                .iter()
                .map(|i| port::MemberRef::Session(self.ids[*i].clone()))
                .collect(),
        )
        .await
        .unwrap()
        .id
    }
    async fn work_pair_post(
        &mut self,
        source: usize,
        target: usize,
        channel: &str,
        text: &str,
    ) -> Message {
        if source < 2 {
            return self.post(source, target, channel, text).await;
        }
        let posted = port::post(
            self.state(),
            port::PostReq {
                channel_id: channel.into(),
                sender: port::PostSender::Session(self.ids[source].clone()),
                text: text.into(),
                mentions: vec![Mention {
                    kind: MentionKind::Session,
                    target_id: Some(self.ids[target].clone()),
                }],
                in_reply_to: None,
                category: Some("note".into()),
                attachments: vec![],
            },
        )
        .await
        .unwrap()
        .message;
        self.barrier(&posted).await;
        posted
    }
    async fn one_real_work_pair(&mut self, source: usize) -> (i64, u64, u64) {
        let id = self.ids[source].clone();
        let result = self
            .state()
            .collab(move |tx| {
                let key = format!("causal.session.v1.{id}");
                let before =
                    crewd_store::meta_get(tx.conn(), &key)?.expect("actual delivered Human basis");
                crewd_collab::sessions::note_progress(
                    tx,
                    &id,
                    crewd_collab::ProgressKind::WorkItem,
                )?;
                let after = crewd_store::meta_get(tx.conn(), &key)?.expect("actual Work epoch");
                let epoch = |s: &str| {
                    serde_json::from_str::<serde_json::Value>(s).unwrap()["value"]["epoch"]
                        .as_u64()
                        .unwrap()
                };
                Ok((tx.now_ms(), epoch(&before), epoch(&after)))
            })
            .await
            .unwrap();
        self.progress_receipts_variant(source, 1).await;
        result
    }
}

#[tokio::test]
async fn s42_thirty_actual_active_work_chains_are_independent_and_not_globally_throttled() {
    let mut rig = Rig::new_with_sessions(60).await;
    let observation = catch_setup(async {
        rig.warmup_work_pairs().await;
        let mut roots = vec![];
        let mut channels = vec![];
        // All actual Human inputs complete before any Work epoch or Session output.
        // Private root channels avoid a later input accidentally overwriting another source's basis.
        for pair in 0..30 {
            let source = pair * 2;
            let root_channel = rig
                .work_pair_topic(source, &[source], &format!("S42 private basis {pair}"))
                .await;
            let route = rig
                .work_pair_topic(
                    source,
                    &[source, source + 1],
                    &format!("S42 work route {pair}"),
                )
                .await;
            let root = rig
                .human(source, &root_channel, &format!("S42_WORK_HUMAN_{pair}"))
                .await;
            let sent = rig.sent(source, &root.text).await;
            if sent {
                rig.acknowledge(source, &root).await;
            }
            let parent = rig.parent_variant(source).await;
            roots.push((root, sent, parent));
            channels.push(route);
        }
        let mut work = vec![];
        // Thirty genuine durable WorkItem transactions, thirty real Hub receipts and actor barriers.
        for pair in 0..30 {
            let source = pair * 2;
            let basis = rig.basis_variant(source).await;
            let epoch = rig.one_real_work_pair(source).await;
            let after = rig.basis_variant(source).await;
            work.push((basis, epoch, after));
        }
        let active_before = {
            let mut active = vec![];
            for id in &rig.ids {
                active.push(rig.h.live(id).await.unwrap().running);
            }
            active
        };
        let mut delivered = vec![];
        for (pair, channel) in channels.iter().enumerate() {
            let source = pair * 2;
            let target = source + 1;
            let first = rig
                .work_pair_post(source, target, channel, &format!("S42_WORK_ZERO_{pair}"))
                .await;
            let sent_first = rig.sent(target, &first.text).await;
            if sent_first {
                rig.acknowledge(target, &first).await;
            }
            let first_frontier = rig.root_variant(&first).await;
            let child = rig
                .work_pair_post(target, source, channel, &format!("S42_WORK_ONE_{pair}"))
                .await;
            let sent_child = rig.sent(source, &child.text).await;
            if sent_child {
                rig.acknowledge(source, &child).await;
            }
            let child_frontier = rig.root_variant(&child).await;
            let rows_first = rig.rows_variant(&first).await;
            let rows_child = rig.rows_variant(&child).await;
            delivered.push((
                first,
                first_frontier,
                sent_first,
                rows_first,
                child,
                child_frontier,
                sent_child,
                rows_child,
            ));
        }
        (
            rig.ids.clone(),
            roots,
            work,
            delivered,
            active_before,
            rig.h.latches().await.unwrap(),
            rig.h.causal_holds(None).await.unwrap(),
            rig.progress.load(std::sync::atomic::Ordering::SeqCst),
        )
    })
    .await;
    if let Err(original) = &observation {
        eprintln!(
            "S42 work setup/observation failure: {}",
            panic_text(original.as_ref())
        );
    }
    let receipts = match catch_setup(rig.close()).await {
        Ok(receipts) => receipts,
        Err(cleanup_error) => {
            eprintln!(
                "S42 work cleanup failure: {}",
                panic_text(cleanup_error.as_ref())
            );
            match observation {
                Err(original) => std::panic::resume_unwind(original),
                Ok(_) => std::panic::resume_unwind(cleanup_error),
            }
        }
    };
    println!(
        "S42 sixty active controlled sessions / two actual generated helpers cleanup: {receipts:?}"
    );
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .all(|r| r.normal && r.exit_code == Some(0) && !r.forced_kill)
    );
    let (ids, roots, work, delivered, active, legacy, holds, progress) = match observation {
        Ok(value) => value,
        Err(original) => std::panic::resume_unwind(original),
    };
    // Functional assertions only after explicit normal helper/socket/actor/bridge/DB cleanup.
    assert_eq!(ids.len(), 60);
    assert_eq!(
        active,
        vec![true; 60],
        "60 actual live sessions, not thirty iterations of three sessions"
    );
    assert_eq!(roots.len(), 30);
    assert_eq!(work.len(), 30);
    assert_eq!(
        delivered.len(),
        30,
        "S42 thirty active Work chains completed"
    );
    assert_eq!(
        progress, 30,
        "S42 thirty actual WorkItem Tx -> Hub -> actor receipts"
    );
    assert!(
        legacy.is_empty(),
        "retained Edge/Session guards must not mask independence"
    );
    assert!(holds.is_empty(), "no global chain throttle");
    let mut identities = std::collections::BTreeSet::new();
    for pair in 0..30 {
        let source = &ids[pair * 2];
        let target = &ids[pair * 2 + 1];
        let (root, root_sent, parent) = &roots[pair];
        assert!(*root_sent);
        let (before, (now, old_epoch, new_epoch), after) = &work[pair];
        assert!(*now > 0);
        assert_eq!(*new_epoch, old_epoch + 1);
        assert_eq!(after["epoch"].as_u64(), Some(*new_epoch));
        assert_eq!(
            parent["status"], "committed",
            "actual Human delivery output is committed before Work"
        );
        assert_eq!(
            before["last_output"], parent["receipt"]["id"],
            "basis belongs to this exact original successful output"
        );
        let capture = &parent["receipt"]["capture"];
        assert_eq!(capture["session"], *source);
        assert_eq!(
            capture["tuples"],
            json!([{
                "id": root.id, "channel": root.channel_id, "seq": root.seq,
                "kind": root.sender_kind, "sender": root.sender_session_id
            }]),
            "complete actual captured tuple set contains only the directed Human input; do not silently discard System notices"
        );
        assert_eq!(capture["windows"].as_array().unwrap().len(), 1);
        assert_eq!(capture["windows"][0]["channel"], root.channel_id);
        assert_eq!(capture["windows"][0]["toInclusive"], root.seq);
        assert_eq!(capture["frontier"].as_array().unwrap().len(), 1);
        assert_eq!(
            capture["frontier"][0]["key"], before["basis"][0]["key"],
            "installed basis retains complete actual output root identity"
        );
        assert_eq!(before["basis"].as_array().unwrap().len(), 1);
        assert_eq!(
            before["basis"][0]["key"]["root"],
            json!({"kind":"message","id":root.id,"seq":root.seq})
        );
        let (
            first,
            first_cause,
            first_sent,
            first_rows,
            child,
            child_cause,
            child_sent,
            child_rows,
        ) = &delivered[pair];
        assert!(
            *first_sent && *child_sent,
            "both actual controlled sends required"
        );
        assert_eq!(first.sender_session_id.as_deref(), Some(source.as_str()));
        assert_eq!(child.sender_session_id.as_deref(), Some(target.as_str()));
        assert_work_frontier_variant(first_cause, source, *new_epoch, 0, &before["basis"]);
        assert_work_frontier_variant(child_cause, source, *new_epoch, 1, &before["basis"]);
        assert!(
            identities.insert(first_cause.to_string()),
            "qualified component must be distinct across actual work sessions"
        );
        assert!(
            first_rows
                .iter()
                .any(|w| &w.session == target && w.state == wakes::WakeState::Delivered)
        );
        assert!(
            child_rows
                .iter()
                .any(|w| &w.session == source && w.state == wakes::WakeState::Delivered)
        );
    }
}
