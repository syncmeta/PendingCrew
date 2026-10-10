//! Generated Stop command -> actual crewd helper -> socket -> Tx/Hub -> DbPort/Core.
//! Only the executor is StandIn. No provider, real CLI, MCP tool-progress or direct Stop shortcut.
#[allow(dead_code)]
#[path = "support/agent.rs"]
mod support;

use crewd_collab::{messages, wakes};
use crewd_exec::{
    delivery::Subject,
    session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
};
use crewd_proto::{MentionKind, Message, SenderKind};
use crewd_server::{
    agent::{
        port,
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
use crewd_wake::{LatchScope, Ms};
use serde_json::json;
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};

const LIMIT: Duration = Duration::from_secs(10);
const QUESTION: &str = "下一步应该怎么办？";

struct Rig {
    f: Fixture,
    h: Handle,
    fake: RelayStandIn,
    actor: Option<Actor>,
    forwarder: JoinHandle<()>,
    socket: JoinHandle<()>,
    stop_socket: Option<oneshot::Sender<()>>,
    forwarded: mpsc::UnboundedReceiver<CollabFact>,
    worker: String,
    captain: String,
    lobby: String,
    launch: support::Launch,
    _runtime: tempfile::TempDir,
    home: tempfile::TempDir,
    progress_seen: Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        // Assertion/setup failure safety; normal close below joins every owned task and DB.
        if let Some(actor) = &self.actor {
            actor.abort_handle().abort();
        }
        self.forwarder.abort();
        self.socket.abort();
    }
}
impl Rig {
    async fn new() -> Self {
        let cell = Arc::new(OnceLock::new());
        let mut f = Fixture::with_orchestra(cell.clone()).await;
        f.crew = f.add_crew("合成环路", None).await;
        let rx = f.state.hub.subscribe_collab(); // before boot, same production ordering
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // Fixed within the actual messages' six-hour planning and ten-minute breaker windows.
        let clock = Arc::new(ManualClock::new(Ms(now + 60_000)));
        let fake = RelayStandIn::default();
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(fake.clone())),
            Hooks::default(),
            clock,
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
        let (seen, forwarded) = mpsc::unbounded_channel();
        let progress_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let progress = progress_seen.clone();
        let sink = h.clone();
        let forwarder = tokio::spawn(pump_collab(rx, move |fact| {
            if matches!(fact, CollabFact::Progress { .. }) {
                progress.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            sink.collab(fact.clone());
            // Receipt only AFTER the real fact has entered the actor mailbox.
            let _ = seen.send(fact);
        }));
        let (stop_socket, stop) = oneshot::channel();
        let socket = f.spawn_owned_socket(stop).await;
        let start = |captain, title: &str| StartSession {
            crew_id: f.crew.clone(),
            captain,
            executor: "claude".into(),
            brief: "合成初始任务".into(),
            title: Some(title.into()),
            model: None,
            effort: None,
            isolation: false,
        };
        let captain = h
            .start(start(true, "合成机长"), Origin::Human)
            .await
            .unwrap()
            .id;
        let worker = h
            .start(start(false, "合成成员"), Origin::Human)
            .await
            .unwrap()
            .id;
        let runtime = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for name in ["cache", "claude-config"] {
            std::fs::create_dir(home.path().join(name)).unwrap();
        }
        let launch = support::launch(&f, &worker, runtime.path());
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let rig = Self {
            f,
            h,
            fake,
            actor: Some(actor),
            forwarder,
            socket,
            stop_socket: Some(stop_socket),
            forwarded,
            worker,
            captain,
            lobby,
            launch,
            _runtime: runtime,
            home,
            progress_seen,
        };
        for id in [rig.captain.clone(), rig.worker.clone()] {
            rig.wait_running(&id).await;
            assert_eq!(rig.fake.launches(&id).len(), 1);
            assert!(rig.fake.launches(&id)[0].initial.is_some());
            rig.fake.emit(
                &id,
                SessionEvent::Delivery {
                    id: BRIEF_DELIVERY_ID,
                    subject: Subject::Brief,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            rig.idle_turn(&id).await;
        }
        // Fresh memo defaults to No; this non-question Stop is a real caller warmup, not an eligibility repair.
        rig.stop("warmup", "收到。").await;
        rig
    }
    async fn wait_running(&self, id: &str) {
        timeout(LIMIT, async {
            loop {
                if self.h.live(id).await.unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn idle_turn(&self, id: &str) {
        self.fake.emit(id, SessionEvent::Working(true));
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
        self.fake.emit(id, SessionEvent::Working(false));
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
    async fn barrier(&mut self, wanted: CollabFact) {
        timeout(LIMIT, async {
            loop {
                let fact = self.forwarded.recv().await.expect("forwarder closed");
                if fact == wanted {
                    break;
                }
            }
        })
        .await
        .unwrap();
        // FIFO request after observed sink enqueue, not a sleep or an inferred helper success.
        self.h.latches().await.unwrap();
    }
    async fn stop(&self, prompt: &str, text: &str) {
        support::fire_isolated(
            &self.launch.settings,
            "Stop",
            json!({
                "hook_event_name": "Stop", "prompt_id": prompt,
                "last_assistant_message": text, "stop_hook_active": false,
            }),
            self.home.path(),
        )
        .await;
    }
    async fn enqueue(&mut self, round: usize) -> i64 {
        let worker = self.worker.clone();
        let own_timer = round.is_multiple_of(2);
        let id = self
            .f
            .state
            .collab(move |tx| {
                wakes::enqueue(
                    tx,
                    wakes::WakeRequest {
                        session: worker.clone(),
                        kind: if own_timer {
                            wakes::WakeKind::Timer
                        } else {
                            wakes::WakeKind::System
                        },
                        source: if own_timer {
                            wakes::source::timer(&format!("relay-round-{round}"))
                        } else {
                            wakes::source::system(&format!("relay-round-{round}"))
                        },
                        priority: wakes::WakePriority::System,
                        not_before_ms: Some(0),
                        note: Some(format!("合成轮次 {round}")),
                        origin: own_timer.then_some(worker),
                    },
                )
            })
            .await
            .unwrap();
        let wakes::Enqueued::New(id) = id else {
            panic!("each round must actually enqueue a fresh wake")
        };
        self.barrier(CollabFact::WakeEnqueued {
            session: self.worker.clone(),
        })
        .await;
        id
    }
    async fn delivered(&self, wake: i64, count: usize) {
        timeout(LIMIT, async {
            loop {
                if self.fake.sent(&self.worker).len() == count {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let id = self.fake.last_send_id(&self.worker);
        self.fake.emit(
            &self.worker,
            SessionEvent::Delivery {
                id,
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        timeout(LIMIT, async {
            loop {
                let w = self
                    .f
                    .state
                    .db
                    .call(move |s| wakes::list(s.conn(), None, 200))
                    .await
                    .unwrap()
                    .unwrap();
                if w.iter()
                    .any(|w| w.id == wake && w.state == wakes::WakeState::Delivered)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        self.idle_turn(&self.worker).await;
    }
    async fn latest_message(&self) -> Message {
        port::messages_raw(&self.f.state, &self.lobby, None, 200)
            .await
            .unwrap()
            .into_iter()
            .last()
            .unwrap()
    }
    async fn message_barrier(&mut self, m: &Message) {
        self.barrier(CollabFact::MessageAppended {
            channel: m.channel_id.clone(),
            seq: m.seq,
        })
        .await;
    }
    async fn progress_at(&self) -> Option<String> {
        let id = self.worker.clone();
        self.f
            .state
            .db
            .call(move |s| {
                crewd_store::sessions::get(s.conn(), &id).map(|r| r.unwrap().last_progress_at)
            })
            .await
            .unwrap()
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
        let _ = self.stop_socket.take().unwrap().send(());
        timeout(LIMIT, &mut self.socket).await.unwrap().unwrap();
        self.forwarder.abort();
        let _ = timeout(LIMIT, &mut self.forwarder).await.unwrap();
        let db = self.f.state.db.clone();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn stop_helper_question_relay_does_not_reset_system_timer_breaker() {
    let mut rig = Rig::new().await;
    let initial_progress = rig.progress_at().await;
    for round in 1..=8 {
        let wake = rig.enqueue(round).await;
        rig.delivered(wake, round).await;
        let before = port::latest_seq(&rig.f.state, &rig.lobby).await.unwrap();
        rig.stop(&format!("prompt-{round}"), QUESTION).await;
        let m = rig.latest_message().await;
        assert!(
            before.is_none_or(|b| m.seq > b),
            "helper must commit a new question each prompt"
        );
        assert_eq!(m.sender_kind, SenderKind::Session);
        assert_eq!(m.sender_session_id.as_deref(), Some(rig.worker.as_str()));
        assert_eq!(m.category.as_deref(), Some("question"));
        assert!(m.mentions.iter().any(|m| m.kind == MentionKind::Captain
            && m.target_id.as_deref() == Some(rig.captain.as_str())));
        rig.message_barrier(&m).await;
    }
    let ninth = rig.enqueue(9).await;
    let live = rig.h.latches().await.unwrap();
    let durable = rig
        .f
        .state
        .db
        .call(|s| wakes::latches::list(s.conn()))
        .await
        .unwrap()
        .unwrap();
    let rows = rig
        .f
        .state
        .db
        .call(|s| wakes::list(s.conn(), None, 200))
        .await
        .unwrap()
        .unwrap();
    let sends = rig.fake.sent(&rig.worker).len();
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let observed_progress = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    let stop_count = rig
        .f
        .hook_observations(&rig.worker)
        .iter()
        .filter(|h| h.event == "Stop")
        .count();
    let held = live
        .iter()
        .any(|l| l.scope == LatchScope::Session && l.id == rig.worker);
    let persisted = durable
        .iter()
        .any(|l| l.scope == wakes::latches::Scope::Session(rig.worker.clone()));
    let pending = rows
        .iter()
        .any(|w| w.id == ninth && w.state == wakes::WakeState::Pending && w.attempts == 0);
    rig.close().await; // close even on the expected baseline assertion failure
    assert!(
        progress_unchanged && observed_progress == 0,
        "ToolProgress must not mask on_post"
    );
    assert_eq!(
        stop_count, 9,
        "warmup plus eight real generated Stop commands"
    );
    assert!(
        held && persisted && pending && sends == 8,
        "auto-relay must not reset streak: target_latch={held}, durable={persisted}, ninth_pending={pending}, actual_sends={sends}"
    );
}

#[tokio::test]
async fn ordinary_question_from_the_same_session_is_real_progress() {
    let mut rig = Rig::new().await;
    // Obtain exactly the generated auto-relay body/category/@ shape before any counted wake.
    rig.stop("ordinary-shape", QUESTION).await;
    let shape = rig.latest_message().await;
    rig.message_barrier(&shape).await;
    let initial_progress = rig.progress_at().await;
    for round in 1..=8 {
        let wake = rig.enqueue(round).await;
        rig.delivered(wake, round).await;
        let worker = rig.worker.clone();
        let shape = shape.clone();
        let m = rig
            .f
            .state
            .collab(move |tx| {
                messages::post(
                    tx,
                    crewd_collab::Sender::Session(worker),
                    messages::NewPost {
                        channel_id: shape.channel_id,
                        text: shape.text,
                        category: shape.category,
                        mentions: shape.mentions,
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .message;
        rig.message_barrier(&m).await;
    }
    let ninth = rig.enqueue(9).await;
    rig.delivered(ninth, 9).await;
    let latches = rig.h.latches().await.unwrap();
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let observed_progress = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    let sends = rig.fake.sent(&rig.worker).len();
    rig.close().await;
    assert!(
        progress_unchanged && observed_progress == 0,
        "positive must exercise on_post, not a tool progress reset"
    );
    assert!(
        latches.is_empty(),
        "ordinary question still counts as actual call-out progress"
    );
    assert_eq!(sends, 9);
}

#[tokio::test]
async fn generated_stop_history_remains_typed_after_db_reopen() {
    use crewd_server::orchestra::port::Port;
    use crewd_wake::{MessageRef, plan::MessageProvenance};
    let f = Fixture::new().await;
    let (stop, stopped) = oneshot::channel();
    let mut socket = f.spawn_owned_socket(stopped).await;
    let runtime = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = support::launch(&f, &f.worker, runtime.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let mut history = Vec::new();
    for round in 1..=2 {
        support::fire_isolated(
            &config.settings,
            "Stop",
            json!({
                "hook_event_name": "Stop", "prompt_id": format!("history-{round}"),
                "last_assistant_message": QUESTION,
            }),
            home.path(),
        )
        .await;
        let row = port::messages_raw(&f.state, &lobby, None, 200)
            .await
            .unwrap()
            .into_iter()
            .last()
            .unwrap();
        assert_eq!(row.sender_session_id.as_deref(), Some(f.worker.as_str()));
        assert!(!history.iter().any(|m: &Message| m.id == row.id));
        history.push(row);
    }
    let shape = history[0].clone();
    let worker = f.worker.clone();
    let ordinary = f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Session(worker),
                messages::NewPost {
                    channel_id: shape.channel_id,
                    text: shape.text,
                    category: shape.category,
                    mentions: shape.mentions,
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    let _ = stop.send(());
    timeout(LIMIT, &mut socket).await.unwrap().unwrap();
    let db = f.state.db.clone();
    timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
        .await
        .unwrap()
        .unwrap();
    let path = f.data_dir().join("crewd.db");
    let db = crewd_store::Db::spawn(crewd_store::Store::open(&path).unwrap()).unwrap();
    let state = Arc::new(crewd_server::AppState::new(db.clone()));
    let adapter = DbPort::new(state, path);
    let mut observed = Vec::new();
    for message in &history {
        let facts = adapter
            .message(&MessageRef {
                channel: message.channel_id.clone(),
                seq: message.seq,
            })
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(facts.provenance, MessageProvenance::StopRelay);
        observed.push(facts.id);
    }
    let normal = adapter
        .message(&MessageRef {
            channel: ordinary.channel_id.clone(),
            seq: ordinary.seq,
        })
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(normal.provenance, MessageProvenance::Ordinary);
    let unread = adapter.unread(&f.captain).await.unwrap();
    assert!(observed.iter().all(|id| {
        unread
            .iter()
            .any(|m| m.id == *id && m.provenance == MessageProvenance::StopRelay)
    }));
    assert!(
        unread
            .iter()
            .any(|m| m.id == ordinary.id && m.provenance == MessageProvenance::Ordinary)
    );
    timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn generated_stop_marker_failure_rolls_back_tx_and_publishes_no_hub_message() {
    let f = Fixture::new().await;
    let (stop, stopped) = oneshot::channel();
    let mut socket = f.spawn_owned_socket(stopped).await;
    let runtime = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let config = support::launch(&f, &f.worker, runtime.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let before = port::latest_seq(&f.state, &lobby).await.unwrap();
    let mut events = f.state.hub.subscribe_collab();
    f.state.db.call(|s| s.conn().execute_batch("CREATE TRIGGER fail_synthetic_stop_marker BEFORE INSERT ON meta WHEN substr(NEW.key,1,22)='message.provenance.v1.' BEGIN SELECT RAISE(ABORT,'synthetic marker failure'); END")).await.unwrap().unwrap();
    support::fire_isolated(
        &config.settings,
        "Stop",
        json!({
            "hook_event_name": "Stop", "prompt_id": "failed-source-mark",
            "last_assistant_message": QUESTION,
        }),
        home.path(),
    )
    .await;
    let after = port::latest_seq(&f.state, &lobby).await.unwrap();
    let worker = f.worker.clone();
    let rolled_back = f
        .state
        .db
        .call(move |s| {
            let row = crewd_store::sessions::get(s.conn(), &worker)?.unwrap();
            let memo = crewd_store::meta_get(s.conn(), &format!("agent.turn.{worker}"))?;
            Ok::<_, crewd_store::StoreError>(row.awaiting_question.is_none() && memo.is_none())
        })
        .await
        .unwrap()
        .unwrap();
    let hub_empty = matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    );
    // Real helper intentionally swallows Stop failure: DB/Hub assertions, not its exit0, prove rollback.
    f.state
        .db
        .call(|s| {
            s.conn()
                .execute_batch("DROP TRIGGER fail_synthetic_stop_marker")
        })
        .await
        .unwrap()
        .unwrap();
    support::fire_isolated(
        &config.settings,
        "Stop",
        json!({
            "hook_event_name": "Stop", "prompt_id": "failed-source-mark",
            "last_assistant_message": QUESTION,
        }),
        home.path(),
    )
    .await;
    let recovered = port::latest_seq(&f.state, &lobby).await.unwrap();
    let _ = stop.send(());
    timeout(LIMIT, &mut socket).await.unwrap().unwrap();
    let db = f.state.db.clone();
    timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert!(
        rolled_back && hub_empty,
        "failed source marker must roll back message, awaiting question, memo and publication"
    );
    assert!(
        recovered > before,
        "same real caller must post after removing only its synthetic failure trigger"
    );
}

#[tokio::test]
async fn real_mcp_post_cannot_supply_internal_stop_provenance() {
    let f = Fixture::new().await;
    let (stop, stopped) = oneshot::channel();
    let mut socket = f.spawn_owned_socket(stopped).await;
    let runtime = tempfile::tempdir().unwrap();
    let config = support::launch(&f, &f.worker, runtime.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let before = port::latest_seq(&f.state, &lobby).await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut helper = support::Mcp::start_isolated(&config.mcp, home.path()).await;
    let (_, is_error) = helper.call("post", json!({"channel": lobby, "message": QUESTION, "category": "question", "provenance": {"kind": "stop_relay"}})).await;
    let after_rejected = port::latest_seq(&f.state, &lobby).await.unwrap();
    let (text, ordinary_error) = helper
        .call(
            "post",
            json!({"channel": lobby, "message": QUESTION, "category": "question"}),
        )
        .await;
    assert!(
        !ordinary_error,
        "the same valid post without forged provenance must succeed: {text}"
    );
    let ordinary = port::messages_raw(&f.state, &lobby, None, 200)
        .await
        .unwrap()
        .into_iter()
        .last()
        .unwrap();
    let ordinary_for_read = ordinary.clone();
    let provenance = f
        .state
        .db
        .call(move |s| crewd_collab::message_provenance::read(s.conn(), &ordinary_for_read))
        .await
        .unwrap()
        .unwrap();
    helper.close().await;
    let after = port::latest_seq(&f.state, &lobby).await.unwrap();
    let _ = stop.send(());
    timeout(LIMIT, &mut socket).await.unwrap().unwrap();
    let db = f.state.db.clone();
    timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        is_error,
        "public tool arguments cannot grant internal provenance"
    );
    assert_eq!(after_rejected, before);
    assert!(after > before);
    assert_eq!(
        ordinary.sender_session_id.as_deref(),
        Some(f.worker.as_str())
    );
    assert_eq!(
        provenance,
        crewd_collab::message_provenance::MessageProvenance::Ordinary
    );
}
