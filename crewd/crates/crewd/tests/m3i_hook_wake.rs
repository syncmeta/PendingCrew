//! S44: generated PostToolUse/Failure -> helper -> socket -> real hook/wake shared cursor.
//! Only the executor is StandIn. Inflight receipts are deliberately held; no provider is started.
#[allow(dead_code)]
#[path = "support/agent.rs"]
mod support;

use crewd_collab::messages;
use crewd_exec::{
    delivery::Subject,
    session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
};
use crewd_proto::{Mention, MentionKind, Message};
use crewd_server::{
    agent::{
        port,
        testkit::{CallerStage, Fixture, InjectionCompletion, InjectionOutcome, RelayStandIn},
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
use crewd_wake::{Ms, WakeState};
use serde_json::json;
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};

const LIMIT: Duration = Duration::from_secs(10);
const MARKER: &str = "S44_SYNTHETIC_SINGLE_MESSAGE_8f26";
const OTHER_MARKER: &str = "S44_UNRESERVED_OTHER_CHANNEL_a934";
const PASSIVE: &str = "mcp__crew__read_channel";

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
        f.crew = f.add_crew("合成共享游标", None).await;
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
        let home = tempfile::tempdir().unwrap();
        for name in ["cache", "claude-config"] {
            std::fs::create_dir(home.path().join(name)).unwrap();
        }
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
    async fn running(&self, wanted: bool) {
        timeout(LIMIT, async {
            loop {
                if self.h.live(&self.worker).await.unwrap().running == wanted {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn working(&self, wanted: bool) {
        self.fake.emit(&self.worker, SessionEvent::Working(wanted));
        timeout(LIMIT, async {
            loop {
                if self.h.live(&self.worker).await.unwrap().working == wanted {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn post(&mut self) -> Message {
        let channel = self.lobby.clone();
        let worker = self.worker.clone();
        let m = self
            .f
            .state
            .collab(move |tx| {
                messages::post(
                    tx,
                    crewd_collab::Sender::Human,
                    messages::NewPost {
                        channel_id: channel,
                        text: MARKER.into(),
                        category: Some("question".into()),
                        mentions: vec![Mention {
                            kind: MentionKind::Session,
                            target_id: Some(worker),
                        }],
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .message;
        self.barrier(CollabFact::MessageAppended {
            channel: m.channel_id.clone(),
            seq: m.seq,
        })
        .await;
        // MessageAppended is the real Hub fact; Core planning creates the wake itself.
        // There is no additional collab WakeEnqueued fact for this route.
        m
    }
    async fn other_message(&mut self) -> (Message, i64) {
        self.other_message_with_text(OTHER_MARKER).await
    }
    async fn other_message_with_text(&mut self, text: &str) -> (Message, i64) {
        let text = text.to_string();
        let captain = self.f.me(&self.captain).await;
        let ch = port::create_channel(
            &self.f.state,
            &captain,
            port::ChannelKind::Topic,
            "合成阶段正控".into(),
            None,
            vec![port::MemberRef::Session(self.worker.clone())],
        )
        .await
        .unwrap();
        let before = port::cursor(&self.f.state, &self.worker, &ch.id)
            .await
            .unwrap()
            .expect("actual channel join pins before posting");
        let other = self
            .f
            .state
            .collab(move |tx| {
                messages::post(
                    tx,
                    crewd_collab::Sender::Human,
                    messages::NewPost {
                        channel_id: ch.id,
                        text,
                        mentions: vec![],
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .message;
        self.barrier(CollabFact::MessageAppended {
            channel: other.channel_id.clone(),
            seq: other.seq,
        })
        .await;
        (other, before)
    }
    fn spawned_hook(&self, launch: &support::Launch) -> OwnedHook {
        let settings = launch.settings.clone();
        let home = self.home.path().to_path_buf();
        let channel = self.lobby.clone();
        OwnedHook(Some(tokio::spawn(async move {
            let started = Instant::now();
            let out = support::fire_isolated(
                &settings,
                "PostToolUse",
                json!({
                    "hook_event_name": "PostToolUse", "tool_name": PASSIVE,
                    "tool_input": {"channel": channel},
                }),
                &home,
            )
            .await;
            (out, started.elapsed())
        })))
    }
    async fn cursor(&self) -> i64 {
        port::cursor(&self.f.state, &self.worker, &self.lobby)
            .await
            .unwrap()
            .expect("session membership must pin an initial cursor before posting")
    }
    async fn active_lease(&self) -> serde_json::Value {
        let session = self.worker.clone();
        self.f
            .state
            .db
            .call(move |store| {
                let raw = crewd_store::meta_get(
                    store.conn(),
                    &format!("cursor.reservation.v1.{session}"),
                )?
                .expect("actual claim has durable private reservation metadata");
                Ok::<_, crewd_store::StoreError>(
                    serde_json::from_str::<serde_json::Value>(&raw)?["active"].clone(),
                )
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn receipt(&self, m: &Message, wanted: WakeState) {
        timeout(LIMIT, async {
            loop {
                let rows = self.h.receipts(&m.id).await.unwrap();
                if rows
                    .iter()
                    .any(|r| r.session == self.worker && r.state == wanted)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn hook(
        &self,
        launch: &support::Launch,
        event: &str,
    ) -> (Option<serde_json::Value>, Duration) {
        let started = Instant::now();
        let out = support::fire_isolated(
            &launch.settings,
            event,
            json!({
                "hook_event_name": event, "tool_name": PASSIVE,
                "tool_input": {"channel": self.lobby}, "error": "synthetic failure payload",
            }),
            self.home.path(),
        )
        .await;
        (out, started.elapsed())
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
        let joined = timeout(LIMIT, &mut self.forwarder).await.unwrap();
        assert!(
            joined.is_ok() || joined.as_ref().is_err_and(|e| e.is_cancelled()),
            "forwarder must actually join without a panic"
        );
        let db = self.f.state.db.clone();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
    }
}

fn assert_completed_injections(
    completions: &[InjectionCompletion],
    observations: &[crewd_server::agent::testkit::HookObservation],
) {
    assert_eq!(
        completions.len(),
        observations.len(),
        "every actual handler must classify termination exactly once"
    );
    for (completion, observation) in completions.iter().zip(observations) {
        assert_eq!(completion.session, observation.session);
        assert_eq!(completion.event, observation.event);
        assert!(
            matches!(
                completion.outcome,
                InjectionOutcome::Committed
                    | InjectionOutcome::Empty
                    | InjectionOutcome::Stale
                    | InjectionOutcome::Busy
            ),
            "actual injection must complete normally rather than swallow error/deadline: {completion:?}"
        );
    }
}

fn context(out: &Option<serde_json::Value>) -> String {
    out.as_ref()
        .and_then(|v| v["hookSpecificOutput"]["additionalContext"].as_str())
        .unwrap_or("")
        .to_string()
}

async fn resumed_held_case(event: &str) {
    let mut rig = Rig::new().await;
    let initial_progress = rig.progress_at().await;
    let start_cursor = rig.cursor().await;
    rig.h.stop(&rig.worker).await.unwrap();
    rig.running(false).await; // stop's reply alone is not an Exited receipt
    assert_eq!(rig.fake.launches(&rig.worker).len(), 1);
    let m = rig.post().await;
    rig.running(true).await;
    rig.h.latches().await.unwrap();
    let launches = rig.fake.launches(&rig.worker);
    assert_eq!(
        launches.len(),
        2,
        "new human @ must actually relaunch the stopped target"
    );
    let held = launches[1]
        .initial
        .clone()
        .expect("wake is carried by the second launch brief");
    assert_eq!(
        held.matches(MARKER).count(),
        1,
        "setup: actual resumed brief must contain our message"
    );
    assert!(
        held.contains(&m.seq.to_string()),
        "setup: rendered brief must include actual seq"
    );
    rig.receipt(&m, WakeState::Inflight).await;
    // Create/join only after the actual held wake has been rendered and claimed. The normal
    // channel join pins the worker's cursor at its empty tail, before the unreserved post.
    let captain = rig.f.me(&rig.captain).await;
    let other_channel = port::create_channel(
        &rig.f.state,
        &captain,
        port::ChannelKind::Topic,
        "合成未预留正控".into(),
        None,
        vec![port::MemberRef::Session(rig.worker.clone())],
    )
    .await
    .unwrap();
    let other_cursor_initial = port::cursor(&rig.f.state, &rig.worker, &other_channel.id)
        .await
        .unwrap()
        .expect("real join must pin before posting");
    let channel_id = other_channel.id.clone();
    let other = rig
        .f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Human,
                messages::NewPost {
                    channel_id,
                    text: OTHER_MARKER.into(),
                    mentions: vec![],
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    rig.barrier(CollabFact::MessageAppended {
        channel: other.channel_id.clone(),
        seq: other.seq,
    })
    .await;
    let other_stored = port::messages_raw(&rig.f.state, &other.channel_id, None, 200)
        .await
        .unwrap()
        .iter()
        .any(|m| m.id == other.id && m.seq == other.seq && m.text == OTHER_MARKER);
    let other_cursor_before_hook = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .expect("joined cursor remains present");
    // Prior launch's token was revoked by on_exited. Generate a new real token/settings now.
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let cursor_before_hook = rig.cursor().await;
    let (first, elapsed) = rig.hook(&launch, event).await;
    let first_context = context(&first);
    let other_cursor_after_hook = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .expect("hook cursor remains present");
    let cursor_while_held = rig.cursor().await;
    let receipt_while_held = rig.h.receipts(&m.id).await.unwrap();
    // Do not assert expected-negative observations until normal teardown has completed.
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Delivery {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    rig.receipt(&m, WakeState::Delivered).await;
    let cursor_after_delivery = rig.cursor().await;
    let (second, second_elapsed) = rig.hook(&launch, event).await;
    let second_context = context(&second);
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let progress_seen = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    let later_sends = rig.fake.sent(&rig.worker);
    let has_matching_receipt = receipt_while_held
        .iter()
        .any(|r| r.session == rig.worker && r.state == WakeState::Inflight);
    rig.close().await;
    assert_completed_injections(&completions, &observations);
    eprintln!(
        "S44 {event}: seq={} brief_marker={} hook_marker={} later_hook_marker={} cursors={start_cursor}/{cursor_before_hook}/{cursor_while_held}/{cursor_after_delivery} elapsed={elapsed:?}/{second_elapsed:?} inflight={has_matching_receipt} later_sends={}",
        m.seq,
        held.matches(MARKER).count(),
        first_context.matches(MARKER).count(),
        second_context.matches(MARKER).count(),
        later_sends.len()
    );
    assert!(
        elapsed < Duration::from_secs(3) && second_elapsed < Duration::from_secs(3),
        "helper silence past original commit deadline is not a mutual exclusion proof"
    );
    assert_eq!(
        observations.len(),
        2,
        "both generated helper calls must reach actual server hook handler"
    );
    assert!(
        observations
            .iter()
            .all(|o| o.event == event && o.tool_name.as_deref() == Some(PASSIVE))
    );
    for out in [&first, &second].into_iter().flatten() {
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], event);
    }
    assert!(
        progress_unchanged && progress_seen == 0,
        "passive tool must not introduce ToolProgress"
    );
    assert!(start_cursor < m.seq && cursor_before_hook < m.seq && has_matching_receipt);
    assert!(cursor_after_delivery >= m.seq);
    assert!(second_context.matches(MARKER).count() == 0 && later_sends.is_empty());
    // Positive output and actual commit within this same handler invocation must hold before
    // its silence for the reserved channel can count as protection from duplicate delivery.
    eprintln!(
        "S44 other-channel: seq={} stored={other_stored} held_marker={} first_hook_marker={} second_hook_marker={} cursors={other_cursor_initial}/{other_cursor_before_hook}/{other_cursor_after_hook}",
        other.seq,
        held.matches(OTHER_MARKER).count(),
        first_context.matches(OTHER_MARKER).count(),
        second_context.matches(OTHER_MARKER).count()
    );
    assert!(
        other_stored && other_cursor_initial < other.seq && other_cursor_before_hook < other.seq
    );
    assert_eq!(
        held.matches(OTHER_MARKER).count(),
        0,
        "other message was posted after the held wake render"
    );
    assert_eq!(
        first_context.matches(OTHER_MARKER).count(),
        1,
        "same first hook must actually inject the unreserved other channel, not swallow an internal error"
    );
    assert!(
        other_cursor_after_hook >= other.seq,
        "same first hook must commit the other channel's actual cursor target"
    );
    assert_eq!(second_context.matches(OTHER_MARKER).count(), 0);
    assert_eq!(
        held.matches(MARKER).count() + first_context.matches(MARKER).count(),
        1,
        "same actual message must occur in held wake and hook stdout combined exactly once"
    );
    assert!(
        cursor_while_held < m.seq,
        "unconfirmed reserved prefix must not become durable read cursor"
    );
}

#[tokio::test]
async fn post_tool_use_does_not_repeat_held_resumed_wake() {
    resumed_held_case("PostToolUse").await;
}

#[tokio::test]
async fn post_tool_use_failure_does_not_repeat_held_resumed_wake() {
    resumed_held_case("PostToolUseFailure").await;
}

#[tokio::test]
async fn unreserved_message_is_injected_by_generated_hook_once() {
    let mut rig = Rig::new().await;
    let initial_progress = rig.progress_at().await;
    let cursor_before = rig.cursor().await;
    rig.working(true).await; // real actor readiness holds the wake; no forwarder is disabled
    let m = rig.post().await;
    rig.receipt(&m, WakeState::Pending).await;
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let (first, elapsed) = rig.hook(&launch, "PostToolUse").await;
    let cursor_after = rig.cursor().await;
    let (second, second_elapsed) = rig.hook(&launch, "PostToolUse").await;
    rig.working(false).await;
    rig.receipt(&m, WakeState::Delivered).await; // injected check settles queue, without a duplicate send
    let sends = rig.fake.sent(&rig.worker);
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let progress_seen = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    rig.close().await;
    assert_completed_injections(&completions, &observations);
    assert!(elapsed < Duration::from_secs(3) && second_elapsed < Duration::from_secs(3));
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|o| o.event == "PostToolUse" && o.tool_name.as_deref() == Some(PASSIVE))
    );
    assert_eq!(context(&first).matches(MARKER).count(), 1);
    assert!(context(&first).contains(&m.seq.to_string()));
    assert_eq!(
        first.unwrap()["hookSpecificOutput"]["hookEventName"],
        "PostToolUse"
    );
    assert_eq!(context(&second).matches(MARKER).count(), 0);
    assert!(cursor_before < m.seq && cursor_after >= m.seq);
    assert!(
        sends.is_empty(),
        "already hook-announced message must not become executor body"
    );
    assert!(progress_unchanged && progress_seen == 0);
}

// Own the future running the generated helper. Normal finish actually joins it; failure cleanup
// aborts the future and leaves support's kill_on_drop as fallback, not a normal child-reap claim.
struct OwnedHook(Option<JoinHandle<(Option<serde_json::Value>, Duration)>>);
impl OwnedHook {
    async fn finish(&mut self) -> (Option<serde_json::Value>, Duration) {
        let out = timeout(LIMIT, self.0.as_mut().unwrap())
            .await
            .unwrap()
            .unwrap();
        let _ = self.0.take();
        out
    }
}
impl Drop for OwnedHook {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

#[tokio::test]
async fn hook_prepared_before_wake_claim_never_double_outputs() {
    let mut rig = Rig::new().await;
    let initial_progress = rig.progress_at().await;
    rig.working(true).await;
    let m = rig.post().await;
    rig.receipt(&m, WakeState::Pending).await;
    let (other, other_before) = rig.other_message().await;
    let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::HookPrepared);
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let mut hook = rig.spawned_hook(&launch);
    let prepared = timeout(Duration::from_secs(2), gate.entered())
        .await
        .unwrap();
    // Gate receipt proves the actual successful prepare had both messages, not an empty fixture.
    assert!(prepared.targets.contains(&(m.channel_id.clone(), m.seq)));
    assert!(
        prepared
            .targets
            .contains(&(other.channel_id.clone(), other.seq))
    );
    assert_eq!(prepared.text.matches(MARKER).count(), 1);
    assert_eq!(prepared.text.matches(OTHER_MARKER).count(), 1);
    let cursor_at_prepare = rig.cursor().await;
    rig.working(false).await; // wake does not use the hook-only session mutex
    rig.receipt(&m, WakeState::Inflight).await;
    let actual_body = rig.fake.sent(&rig.worker);
    assert_eq!(
        actual_body.len(),
        1,
        "setup: actual wake must send while prepared hook is paused"
    );
    let delivery = rig.fake.last_send_id(&rig.worker);
    gate.release();
    let (first, elapsed) = hook.finish().await;
    let first_completions = rig.f.injection_completions(&rig.worker);
    // A second real caller while the wake remains held proves retry/no-output cannot masquerade
    // as whole-handler correctness. Across the two calls, the other channel must be injected once.
    let (second, second_elapsed) = rig.hook(&launch, "PostToolUse").await;
    let before_ack_cursor = rig.cursor().await;
    let other_after = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .unwrap();
    let first_context = context(&first);
    let second_context = context(&second);
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Delivery {
            id: delivery,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    rig.receipt(&m, WakeState::Delivered).await;
    let final_cursor = rig.cursor().await;
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let progress_seen = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    rig.close().await;
    assert_completed_injections(&completions, &observations);
    assert_eq!(
        first_completions.len(),
        1,
        "first actually joined helper must correspond to exactly one completion"
    );
    assert_eq!(first_completions[0].event, "PostToolUse");
    assert_eq!(first_completions[0].session, completions[0].session);
    assert!(
        matches!(
            first_completions[0].outcome,
            InjectionOutcome::Committed
                | InjectionOutcome::Empty
                | InjectionOutcome::Stale
                | InjectionOutcome::Busy
        ),
        "first internal error/deadline cannot be hidden by a successful second helper: {:?}",
        first_completions[0]
    );
    let wake_count: usize = actual_body.iter().map(|t| t.matches(MARKER).count()).sum();
    let hook_count = first_context.matches(MARKER).count() + second_context.matches(MARKER).count();
    eprintln!(
        "S44 hook-first seq={} prepare_targets={:?} actual_wake={wake_count} actual_hook={hook_count} cursors={cursor_at_prepare}/{before_ack_cursor}/{final_cursor} other={other_before}/{other_after} elapsed={elapsed:?}/{second_elapsed:?}",
        m.seq, prepared.targets
    );
    assert!(
        elapsed < Duration::from_secs(3) && second_elapsed < Duration::from_secs(3),
        "3s prepare/commit deadline must not provide false protection"
    );
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|o| o.event == "PostToolUse" && o.tool_name.as_deref() == Some(PASSIVE))
    );
    assert!(progress_unchanged && progress_seen == 0);
    assert_eq!(
        actual_body
            .iter()
            .map(|t| t.matches(OTHER_MARKER).count())
            .sum::<usize>(),
        0
    );
    assert_eq!(
        first_context.matches(OTHER_MARKER).count() + second_context.matches(OTHER_MARKER).count(),
        1,
        "unreserved channel must be output by an actual successful helper call"
    );
    assert!(other_before < other.seq && other_after >= other.seq);
    assert!(cursor_at_prepare < m.seq && final_cursor >= m.seq);
    assert_eq!(
        wake_count + hook_count,
        1,
        "old hook-prepared snapshot must not also output the actually claimed wake prefix"
    );
    assert!(
        before_ack_cursor < m.seq,
        "held prefix is not confirmed by ownership alone"
    );
}

#[tokio::test]
async fn wake_rendered_before_hook_commit_never_sends_stale_body() {
    let mut rig = Rig::new().await;
    let initial_progress = rig.progress_at().await;
    rig.working(true).await;
    let m = rig.post().await;
    rig.receipt(&m, WakeState::Pending).await;
    let (other, other_before) = rig.other_message().await;
    let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::WakeRendered);
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    // Do not await rig.working(false): the actor intentionally waits at the render receipt.
    rig.fake.emit(&rig.worker, SessionEvent::Working(false));
    let rendered = timeout(Duration::from_secs(2), gate.entered())
        .await
        .unwrap();
    assert!(rendered.targets.contains(&(m.channel_id.clone(), m.seq)));
    assert_eq!(rendered.text.matches(MARKER).count(), 1);
    assert_eq!(rendered.text.matches(OTHER_MARKER).count(), 0);
    let before_hook_cursor = rig.cursor().await; // independent DB, not an actor command
    let (first, elapsed) = rig.hook(&launch, "PostToolUse").await;
    let first_context = context(&first);
    let after_hook_cursor = rig.cursor().await;
    let other_after = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .unwrap();
    // Actual helper has completed and the shared DB was independently read while actor paused.
    // No h.live/latches/receipt call here: those would wait for the same paused actor.
    gate.release();
    rig.h.latches().await.unwrap();
    timeout(LIMIT, async {
        loop {
            let rows = rig.h.receipts(&m.id).await.unwrap();
            if rows.iter().any(|r| {
                r.session == rig.worker
                    && matches!(r.state, WakeState::Inflight | WakeState::Delivered)
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let before_ack_receipts = rig.h.receipts(&m.id).await.unwrap();
    let actual_body = rig.fake.sent(&rig.worker);
    if before_ack_receipts
        .iter()
        .any(|r| r.session == rig.worker && r.state == WakeState::Inflight)
    {
        assert_eq!(
            actual_body.len(),
            1,
            "setup: inflight running wake has an actual executor send"
        );
        rig.fake.emit(
            &rig.worker,
            SessionEvent::Delivery {
                id: rig.fake.last_send_id(&rig.worker),
                subject: Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        );
    }
    rig.receipt(&m, WakeState::Delivered).await;
    let final_cursor = rig.cursor().await;
    let (second, second_elapsed) = rig.hook(&launch, "PostToolUse").await;
    let second_context = context(&second);
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let progress_unchanged = rig.progress_at().await == initial_progress;
    let progress_seen = rig.progress_seen.load(std::sync::atomic::Ordering::SeqCst);
    rig.close().await;
    assert_completed_injections(&completions, &observations);
    let wake_count: usize = actual_body.iter().map(|t| t.matches(MARKER).count()).sum();
    let hook_count = first_context.matches(MARKER).count();
    eprintln!(
        "S44 wake-first seq={} rendered_targets={:?} actual_wake={wake_count} actual_hook={hook_count} cursors={before_hook_cursor}/{after_hook_cursor}/{final_cursor} other={other_before}/{other_after} elapsed={elapsed:?}/{second_elapsed:?} preack={before_ack_receipts:?}",
        m.seq, rendered.targets
    );
    assert!(elapsed < Duration::from_secs(3) && second_elapsed < Duration::from_secs(3));
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|o| o.event == "PostToolUse" && o.tool_name.as_deref() == Some(PASSIVE))
    );
    assert!(progress_unchanged && progress_seen == 0);
    assert_eq!(
        first_context.matches(OTHER_MARKER).count(),
        1,
        "same first hook must output unreserved channel while actor remains paused"
    );
    assert!(other_before < other.seq && other_after >= other.seq);
    assert_eq!(second_context.matches(OTHER_MARKER).count(), 0);
    assert_eq!(second_context.matches(MARKER).count(), 0);
    assert!(before_hook_cursor < m.seq && final_cursor >= m.seq);
    assert_eq!(
        wake_count + hook_count,
        1,
        "rendered wake snapshot must revalidate after a real hook commit"
    );
}

#[tokio::test]
async fn s44_actual_mcp_read_keeps_exact_lease_while_stale_hook_rechecks() {
    let mut rig = Rig::new().await;
    rig.working(true).await;
    let m = rig.post().await;
    rig.receipt(&m, WakeState::Pending).await;
    let (other, other_before) = rig.other_message().await;
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    // Real MCP helper and hook use independent connections to the same actor/DB.
    let mut mcp = support::Mcp::start_isolated(&launch.mcp, rig.home.path()).await;
    let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::HookPrepared);
    let mut hook = rig.spawned_hook(&launch);
    let prepared = timeout(Duration::from_secs(2), gate.entered())
        .await
        .unwrap();
    assert!(prepared.targets.contains(&(m.channel_id.clone(), m.seq)));
    assert_eq!(prepared.text.matches(MARKER).count(), 1);
    rig.working(false).await;
    rig.receipt(&m, WakeState::Inflight).await;
    let sends = rig.fake.sent(&rig.worker);
    assert_eq!(
        sends.len(),
        1,
        "real actor must hold a sent, unconfirmed body"
    );
    let delivery = rig.fake.last_send_id(&rig.worker);
    let read_active = |session: String| {
        move |store: &mut crewd_store::Store| {
            let raw =
                crewd_store::meta_get(store.conn(), &format!("cursor.reservation.v1.{session}"))?
                    .expect("actual wake claim has durable metadata");
            Ok::<_, crewd_store::StoreError>(
                serde_json::from_str::<serde_json::Value>(&raw).unwrap()["active"].clone(),
            )
        }
    };
    let active_before = rig
        .f
        .state
        .db
        .call(read_active(rig.worker.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !active_before.is_null(),
        "setup must have an exact active lease"
    );
    let before = rig.cursor().await;
    let (read_text, read_error) = mcp
        .call("read_channel", json!({"channel": rig.lobby, "limit": 200}))
        .await;
    let after = rig.cursor().await;
    let active_after = rig
        .f
        .state
        .db
        .call(read_active(rig.worker.clone()))
        .await
        .unwrap()
        .unwrap();
    gate.release();
    let (out, elapsed) = hook.finish().await;
    let other_after = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .unwrap();
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Delivery {
            id: delivery,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    rig.receipt(&m, WakeState::Delivered).await;
    let active_final = rig
        .f
        .state
        .db
        .call(read_active(rig.worker.clone()))
        .await
        .unwrap()
        .unwrap();
    let final_cursor = rig.cursor().await;
    mcp.close().await;
    rig.close().await;
    assert!(
        !read_error && read_text.matches(MARKER).count() == 1,
        "actual MCP must display the held prefix: {read_text}"
    );
    assert!(
        before < m.seq && after >= m.seq,
        "explicit displayed latest page advances its real cursor"
    );
    assert_eq!(
        active_before, active_after,
        "explicit read must keep exact active receipt identity"
    );
    assert_completed_injections(&completions, &observations);
    assert_eq!(completions.len(), 1);
    assert!(elapsed < Duration::from_secs(3));
    assert_eq!(
        context(&out).matches(MARKER).count(),
        0,
        "old prepared hook must revalidate after actual MCP cursor write"
    );
    assert_eq!(
        context(&out).matches(OTHER_MARKER).count(),
        1,
        "same actual helper must commit the unaffected channel"
    );
    assert!(other_before < other.seq && other_after >= other.seq);
    assert!(
        active_final.is_null() && final_cursor >= m.seq,
        "matching late delivery must still release its own lease"
    );
}

#[tokio::test]
async fn s44_public_stop_keeps_lease_until_late_delivery_after_other_channel_commit() {
    let mut rig = Rig::new().await;
    rig.working(true).await;
    let m = rig.post().await;
    rig.receipt(&m, WakeState::Pending).await;
    rig.working(false).await;
    rig.receipt(&m, WakeState::Inflight).await;
    let delivery = rig.fake.last_send_id(&rig.worker);
    let cursor_before = rig.cursor().await;
    let active_before = rig.active_lease().await;
    assert!(!active_before.is_null());
    let (other, other_before) = rig.other_message().await;
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let (hook, elapsed) = rig.hook(&launch, "PostToolUse").await;
    let other_after = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .unwrap();
    let active_after_other = rig.active_lease().await;
    rig.fake.hold_stop(&rig.worker, true);
    rig.h.stop(&rig.worker).await.unwrap();
    // Witness an actual executor stop request, with no synthetic Exited emitted.
    rig.h.latches().await.unwrap();
    let stop_requests = rig.fake.stop_requests(&rig.worker);
    let still_running = rig.h.live(&rig.worker).await.unwrap().running;
    let cursor_after_stop = rig.cursor().await;
    let active_after_stop = rig.active_lease().await;
    let receipt_after_stop = rig.h.receipts(&m.id).await.unwrap();
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Delivery {
            id: delivery,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    rig.receipt(&m, WakeState::Delivered).await;
    let cursor_after_delivery = rig.cursor().await;
    let active_after_delivery = rig.active_lease().await;
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Exited(crewd_exec::session::ExitInfo {
            status: crewd_exec::session::ExitStatusKind::Code(0),
            stopped_by_user: true,
            hit_limit: false,
        }),
    );
    rig.running(false).await;
    let launches = rig.fake.launches(&rig.worker).len();
    let sends = rig.fake.sent(&rig.worker);
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    rig.close().await;
    assert_completed_injections(&completions, &observations);
    assert_eq!(completions.len(), 1);
    assert!(elapsed < Duration::from_secs(3));
    assert_eq!(context(&hook).matches(MARKER).count(), 0);
    assert_eq!(context(&hook).matches(OTHER_MARKER).count(), 1);
    assert!(other_before < other.seq && other_after >= other.seq);
    assert_eq!(
        active_before, active_after_other,
        "unrelated actual hook changes revision but keeps exact receipt"
    );
    assert_eq!(stop_requests, 1);
    assert!(still_running && cursor_before < m.seq && cursor_after_stop == cursor_before);
    assert!(
        !active_after_stop.is_null(),
        "successful Stop command alone must retain ownership"
    );
    for field in ["token", "executorGeneration", "windows"] {
        assert!(
            !active_before[field].is_null(),
            "actual serialized active field must exist"
        );
        assert_eq!(
            active_before[field], active_after_stop[field],
            "Stop must keep exact active {field}"
        );
    }
    assert!(
        receipt_after_stop
            .iter()
            .any(|r| r.state == WakeState::Inflight)
    );
    assert!(active_after_delivery.is_null() && cursor_after_delivery >= m.seq);
    assert_eq!(
        launches, 1,
        "late delivery/Exit may not relaunch the human-stopped session"
    );
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].matches(MARKER).count(), 1);
}

#[tokio::test]
async fn s44_actual_expired_listener_rejects_paused_wake_before_send() {
    let mut rig = Rig::new().await;
    rig.working(true).await;
    let captain = rig.f.me(&rig.captain).await;
    let topic = port::create_channel(
        &rig.f.state,
        &captain,
        port::ChannelKind::Topic,
        "合成仅收听表面".into(),
        None,
        vec![],
    )
    .await
    .unwrap();
    port::set_listen(&rig.f.state, &rig.worker, &topic.id, None, vec![])
        .await
        .unwrap();
    let before = port::cursor(&rig.f.state, &rig.worker, &topic.id)
        .await
        .unwrap()
        .unwrap();
    let channel = topic.id.clone();
    let m = rig
        .f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: MARKER.into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    rig.barrier(CollabFact::MessageAppended {
        channel: m.channel_id.clone(),
        seq: m.seq,
    })
    .await;
    rig.receipt(&m, WakeState::Pending).await;
    let (other, other_before) = rig.other_message().await;
    let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::WakeRendered);
    rig.fake.emit(&rig.worker, SessionEvent::Working(false));
    let rendered = timeout(Duration::from_secs(2), gate.entered())
        .await
        .unwrap();
    assert!(rendered.targets.contains(&(m.channel_id.clone(), m.seq)));
    assert_eq!(rendered.text.matches(MARKER).count(), 1);
    // Actual durable expiry while the production actor is paused after successful render.
    // This is a timestamp fixture, not elapsed-time or real operating-system evidence.
    let (session, channel) = (rig.worker.clone(), topic.id.clone());
    let expired_rows = rig
        .f
        .state
        .db
        .call(move |store| {
            store.conn().execute(
                "UPDATE listens SET until=0 WHERE session_id=?1 AND channel_id=?2",
                [session, channel],
            )
        })
        .await
        .unwrap()
        .unwrap();
    gate.release();
    rig.h.latches().await.unwrap();
    // A fresh rescan uses the accepted renderer's unusable-surface policy.
    // Rejection itself never claims; subsequent rendering drops the obsolete row.
    rig.receipt(&m, WakeState::Dropped).await;
    let receipts = rig.h.receipts(&m.id).await.unwrap();
    let sends = rig.fake.sent(&rig.worker);
    let launches = rig.fake.launches(&rig.worker).len();
    let after = port::cursor(&rig.f.state, &rig.worker, &topic.id)
        .await
        .unwrap()
        .unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let (hook, elapsed) = rig.hook(&launch, "PostToolUse").await;
    let other_after = port::cursor(&rig.f.state, &rig.worker, &other.channel_id)
        .await
        .unwrap()
        .unwrap();
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let rejected_worker = rig.worker.clone();
    let expected_refusal = format!("{} is no longer in channel {}", rig.worker, topic.id);
    rig.close().await;
    assert_eq!(expired_rows, 1);
    assert!(before < m.seq && after == before);
    eprintln!(
        "S44 actual expired surface receipts={receipts:?} sends={} launches={launches} cursor={before}/{after}",
        sends.len()
    );
    assert!(receipts.iter().any(|r| r.session == rejected_worker
        && r.state == WakeState::Dropped
        && r.attempts == 0
        && r.last_error.as_deref() == Some(expected_refusal.as_str())));
    assert!(
        sends.is_empty(),
        "old rendered listener body may not reach the executor"
    );
    assert_eq!(launches, 1, "surface rejection may not create a new launch");
    assert_completed_injections(&completions, &observations);
    assert_eq!(completions.len(), 1);
    assert!(elapsed < Duration::from_secs(3));
    assert_eq!(context(&hook).matches(MARKER).count(), 0);
    assert_eq!(context(&hook).matches(OTHER_MARKER).count(), 1);
    assert!(other_before < other.seq && other_after >= other.seq);
}

#[tokio::test]
async fn s44_actual_mcp_read_is_the_only_write_invalidating_prepared_hook() {
    let mut rig = Rig::new().await;
    rig.working(true).await;
    // Both unreserved channels already exist and are captured before the pause.
    // No wake claim, membership mutation or other cursor writer occurs during it.
    const READ_MARKER: &str = "S44_MCP_ONLY_INVALIDATOR_f8e3";
    let (read_message, read_before) = rig.other_message_with_text(READ_MARKER).await;
    let (positive, positive_before) = rig.other_message().await;
    let runtime = tempfile::tempdir().unwrap();
    let launch = support::launch(&rig.f, &rig.worker, runtime.path());
    let mut mcp = support::Mcp::start_isolated(&launch.mcp, rig.home.path()).await;
    let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::HookPrepared);
    let mut hook = rig.spawned_hook(&launch);
    let prepared = timeout(Duration::from_secs(2), gate.entered())
        .await
        .unwrap();
    assert!(
        prepared
            .targets
            .contains(&(read_message.channel_id.clone(), read_message.seq))
    );
    assert!(
        prepared
            .targets
            .contains(&(positive.channel_id.clone(), positive.seq))
    );
    assert_eq!(prepared.text.matches(OTHER_MARKER).count(), 1);
    assert_eq!(prepared.text.matches(READ_MARKER).count(), 1);
    let (read_text, read_error) = mcp
        .call(
            "read_channel",
            json!({"channel": read_message.channel_id, "limit": 200}),
        )
        .await;
    let read_after = port::cursor(&rig.f.state, &rig.worker, &read_message.channel_id)
        .await
        .unwrap()
        .unwrap();
    gate.release();
    let (out, elapsed) = hook.finish().await;
    let positive_after = port::cursor(&rig.f.state, &rig.worker, &positive.channel_id)
        .await
        .unwrap()
        .unwrap();
    let completions = rig.f.injection_completions(&rig.worker);
    let observations = rig.f.hook_observations(&rig.worker);
    let sends = rig.fake.sent(&rig.worker);
    mcp.close().await;
    rig.close().await;
    assert!(!read_error && read_text.matches(READ_MARKER).count() == 1);
    assert!(read_before < read_message.seq && read_after >= read_message.seq);
    assert_completed_injections(&completions, &observations);
    assert_eq!(completions.len(), 1);
    assert!(elapsed < Duration::from_secs(3));
    assert_eq!(
        context(&out).matches(OTHER_MARKER).count(),
        1,
        "read prefix must vanish from old preparation; actual other-channel output must survive reprepare"
    );
    assert_eq!(
        context(&out).matches(READ_MARKER).count(),
        0,
        "MCP displayed prefix may not be re-output from stale preparation"
    );
    assert!(positive_before < positive.seq && positive_after >= positive.seq);
    assert!(
        sends.is_empty(),
        "working guard excludes a second invalidating wake claim"
    );
}

#[tokio::test]
async fn s44_actual_member_join_preserves_held_frontier_and_pins_new_member() {
    let mut rig = Rig::new().await;
    rig.working(true).await;
    let captain = rig.f.me(&rig.captain).await;
    let topic = port::create_channel(
        &rig.f.state,
        &captain,
        port::ChannelKind::Topic,
        "合成持有群成员".into(),
        None,
        vec![port::MemberRef::Session(rig.worker.clone())],
    )
    .await
    .unwrap();
    let before = port::cursor(&rig.f.state, &rig.worker, &topic.id)
        .await
        .unwrap()
        .unwrap();
    let (channel, worker) = (topic.id.clone(), rig.worker.clone());
    let m = rig
        .f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: MARKER.into(),
                    mentions: vec![Mention {
                        kind: MentionKind::Session,
                        target_id: Some(worker),
                    }],
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    rig.barrier(CollabFact::MessageAppended {
        channel: m.channel_id.clone(),
        seq: m.seq,
    })
    .await;
    rig.receipt(&m, WakeState::Pending).await;
    rig.working(false).await;
    rig.receipt(&m, WakeState::Inflight).await;
    let delivery = rig.fake.last_send_id(&rig.worker);
    let active = rig.active_lease().await;
    let channel = topic.id.clone();
    let suffix = rig
        .f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: "S44_HELD_SUFFIX_52e6".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    rig.barrier(CollabFact::MessageAppended {
        channel: suffix.channel_id.clone(),
        seq: suffix.seq,
    })
    .await;
    port::add_members(
        &rig.f.state,
        &captain,
        &topic.id,
        vec![port::MemberRef::Session(rig.worker.clone())],
    )
    .await
    .unwrap();
    let after_join = port::cursor(&rig.f.state, &rig.worker, &topic.id)
        .await
        .unwrap()
        .unwrap();
    let active_after_join = rig.active_lease().await;
    let unread = port::unread(&rig.f.state, &rig.worker).await.unwrap();
    // A genuinely new membership, in contrast, pins after preexisting messages.
    let fresh = port::create_channel(
        &rig.f.state,
        &captain,
        port::ChannelKind::Topic,
        "合成新加入正控".into(),
        None,
        vec![],
    )
    .await
    .unwrap();
    let channel = fresh.id.clone();
    let sender = rig.captain.clone();
    let old = rig
        .f
        .state
        .collab(move |tx| {
            messages::post(
                tx,
                crewd_collab::Sender::Session(sender),
                messages::NewPost {
                    channel_id: channel,
                    text: "S44_OLD_JOIN_HISTORY_713a".into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .message;
    let (reader, channel) = ("human".to_string(), fresh.id.clone());
    rig.f
        .state
        .collab(move |tx| crewd_collab::cursors::advance(tx, &reader, &channel, 0))
        .await
        .unwrap();
    let human_before = port::cursor(&rig.f.state, "human", &fresh.id)
        .await
        .unwrap();
    port::add_members(
        &rig.f.state,
        &captain,
        &fresh.id,
        vec![
            port::MemberRef::Session(rig.worker.clone()),
            port::MemberRef::Human,
        ],
    )
    .await
    .unwrap();
    let fresh_cursor = port::cursor(&rig.f.state, &rig.worker, &fresh.id)
        .await
        .unwrap()
        .unwrap();
    let human_after = port::cursor(&rig.f.state, "human", &fresh.id)
        .await
        .unwrap();
    rig.fake.emit(
        &rig.worker,
        SessionEvent::Delivery {
            id: delivery,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        },
    );
    rig.receipt(&m, WakeState::Delivered).await;
    let confirmed = port::cursor(&rig.f.state, &rig.worker, &topic.id)
        .await
        .unwrap()
        .unwrap();
    rig.close().await;
    assert!(before < m.seq && m.seq < suffix.seq);
    assert_eq!(
        after_join, before,
        "actual repeated member addition may not skip held prefix/suffix"
    );
    assert_eq!(active, active_after_join);
    assert!(
        unread.iter().all(|(channel, _)| channel != &topic.id),
        "held whole channel remains blocked for automatic unread"
    );
    assert_eq!(
        fresh_cursor, old.seq,
        "actual first join pins the existing history tail"
    );
    assert_eq!(human_before, Some(0));
    assert_eq!(
        human_after, human_before,
        "joining a human is not a display receipt"
    );
    assert_eq!(
        confirmed, m.seq,
        "late receipt confirms captured prefix, not the newer suffix"
    );
}

#[tokio::test]
async fn work_reminder_completed_after_actual_render_never_reaches_executor() {
    use crewd_collab::{Sender, checkpoints, stop_checks, wakes, work};
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    for mode in ["stop", "checkpoint", "dependency"] {
        let rig = Rig::new().await;
        rig.working(true).await;
        let crew = rig.f.crew.clone();
        let owner = rig.worker.clone();
        let p = rig
            .f
            .state
            .collab(move |tx| {
                let parent = if mode == "dependency" {
                    Some(work::create(
                        tx,
                        &Sender::Human,
                        WorkCreateParams {
                            crew_id: crew.clone(),
                            title: "synthetic dependency".into(),
                            owner: Some(owner.clone()),
                            ..Default::default()
                        },
                    )?)
                } else {
                    None
                };
                let p = work::create(
                    tx,
                    &Sender::Human,
                    WorkCreateParams {
                        crew_id: crew,
                        title: "synthetic rendered work race".into(),
                        owner: Some(owner.clone()),
                        depends_on: parent
                            .as_ref()
                            .map(|p| vec![p.id.clone()])
                            .unwrap_or_default(),
                        ..Default::default()
                    },
                )?;
                if let Some(parent) = parent {
                    work::update(
                        tx,
                        &Sender::Session(owner.clone()),
                        WorkUpdateParams {
                            id: parent.id,
                            expected_revision: parent.revision,
                            status: Some(WorkUpdateStatus::Done),
                            ..Default::default()
                        },
                    )?;
                }
                if mode == "checkpoint" {
                    work::update(
                        tx,
                        &Sender::Session(owner),
                        WorkUpdateParams {
                            id: p.id,
                            expected_revision: p.revision,
                            status: Some(WorkUpdateStatus::Active),
                            next_check_at: Some(crewd_store::ts_from_ms(tx.now_ms() + 1)),
                            note: Some("synthetic next step".into()),
                            ..Default::default()
                        },
                    )
                } else {
                    Ok(p)
                }
            })
            .await
            .unwrap();
        if mode == "stop" {
            let owner = rig.worker.clone();
            rig.f
                .state
                .collab(move |tx| {
                    let c = stop_checks::candidate(tx.conn(), &owner, tx.now_ms())?
                        .expect("live open work candidate");
                    stop_checks::accept(tx, &c)
                })
                .await
                .unwrap();
        } else if mode == "checkpoint" {
            let due = p.next_check_at.as_deref().unwrap();
            timeout(LIMIT, async {
                while crewd_store::now_ts().as_str() < due {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            rig.f.state.collab(checkpoints::enqueue_due).await.unwrap();
        }
        let owner = rig.worker.clone();
        let row = rig
            .f
            .state
            .db
            .call_result(move |s| {
                Ok::<_, crewd_collab::CollabError>(
                    wakes::list(s.conn(), Some(&owner), 100)?
                        .into_iter()
                        .find(|r| {
                            r.source.starts_with("work-stop:")
                                || r.source.starts_with("work-check:")
                                || r.source.starts_with("work-done:")
                        })
                        .expect("durable work reminder"),
                )
            })
            .await
            .unwrap()
            .unwrap();
        let mut gate = rig.f.caller_gate(&rig.worker, CallerStage::WakeRendered);
        rig.fake.emit(&rig.worker, SessionEvent::Working(false));
        let rendered = timeout(LIMIT, gate.entered()).await.unwrap();
        let id = p.id.clone();
        let rev = p.revision.clone();
        rig.f
            .state
            .collab(move |tx| {
                work::update(
                    tx,
                    &Sender::Human,
                    WorkUpdateParams {
                        id,
                        expected_revision: rev,
                        status: Some(WorkUpdateStatus::Done),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
        gate.release();
        let id = row.id;
        let settled = timeout(LIMIT, async {
            loop {
                let rows = rig
                    .f
                    .state
                    .db
                    .call_result(move |s| wakes::by_ids(s.conn(), &[id]))
                    .await
                    .unwrap()
                    .unwrap();
                if matches!(
                    rows[0].state,
                    wakes::WakeState::Dropped | wakes::WakeState::Inflight
                ) {
                    break rows[0].clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        rig.h.latches().await.unwrap();
        let sent = rig.fake.sent(&rig.worker);
        let launches = rig.fake.launches(&rig.worker).len();
        rig.close().await;
        assert!(
            rendered.text.contains(&p.id),
            "{mode}: actual renderer did not capture work body"
        );
        let settled = settled.expect("obsolete work reminder not settled");
        assert_eq!(
            settled.state,
            wakes::WakeState::Dropped,
            "{mode}: obsolete body entered inflight"
        );
        assert_eq!(settled.attempts, 0, "{mode}: obsolete body was claimed");
        assert!(
            !sent.iter().any(|s| s.contains(&p.id)),
            "{mode}: obsolete body reached executor"
        );
        assert_eq!(launches, 1, "{mode}: stale body triggered another launch");
    }
}
