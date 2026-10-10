//! S72 actual attach/call, real committed Hub facts and spawned DbPort/Core.
//! Only the executor is synthetic; no provider or subprocess is started.
use crate::agent::rpc::{AgentConn, handle};
use crate::agent::{
    port,
    testkit::{Fixture, RelayStandIn},
    text::{self, Locale},
};
use crate::orchestra::{
    self, Actor, Handle, OrchestraConfig,
    clock::ManualClock,
    core::{CollabFact, Origin},
    db::{DbPort, pump_collab},
    exec::Executors,
    hooks::Hooks,
};
use crewd_collab::{cursors, messages, wakes};
use crewd_exec::session::SessionEvent;
use crewd_proto::{Message, Request, ServerFrame, agent::CallResult};
use crewd_wake::Ms;
use serde_json::{Value, json};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

const LIMIT: Duration = Duration::from_secs(8);
const LISTENER: &str = "s-s72-listener";
const LABEL: &str = "S72_LISTENER_WITNESS";
const MARKER: &str = "S72_REAL_AGENT_CALL_POST_6da8";

mod matrix;

#[derive(Debug, PartialEq)]
struct Surface {
    channels: i64,
    members: i64,
    listens: i64,
    cursor: Option<i64>,
    messages: i64,
    wakes: i64,
    sessions: i64,
    homes: i64,
    tokens: usize,
    cursors: Vec<(String, String, i64)>,
    reservation_provenance: Vec<(String, String)>,
}

struct Rig {
    f: Option<Fixture>,
    h: Handle,
    exec: RelayStandIn,
    actor: Option<Actor>,
    bridge: Option<JoinHandle<()>>,
    forwarded: mpsc::UnboundedReceiver<CollabFact>,
    conn: AgentConn,
    output: mpsc::Sender<ServerFrame>,
    _notifications: mpsc::Receiver<ServerFrame>,
    channel: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Failure fallback only. The ordinary path below actually joins every owned task.
        if let Some(actor) = &self.actor {
            actor.abort_handle().abort();
        }
        if let Some(bridge) = &self.bridge {
            bridge.abort();
        }
    }
}

impl Rig {
    fn fixture(&self) -> &Fixture {
        self.f.as_ref().expect("fixture still owned before close")
    }

    async fn new(listening: bool) -> Self {
        let cell = Arc::new(OnceLock::new());
        let f = Fixture::with_orchestra(cell.clone()).await;
        f.add_session(&f.crew, LISTENER, crate::agent::Role::Worker, LABEL)
            .await;
        let captain = f.me(&f.captain).await;
        let channel = port::create_channel(
            &f.state,
            &captain,
            port::ChannelKind::Topic,
            "S72 synthetic topic".into(),
            None,
            vec![port::MemberRef::Session(f.worker.clone())],
        )
        .await
        .unwrap()
        .id;
        if listening {
            let channel = channel.clone();
            f.state
                .collab(move |tx| crewd_collab::listens::set(tx, LISTENER, &channel, None, vec![]))
                .await
                .unwrap();
        }
        // Subscribe before boot; no fabricated CollabFact enters this fixture.
        let rx = f.state.hub.subscribe_collab();
        let exec = RelayStandIn::default();
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(f.state.clone(), f.data_dir().join("crewd.db"))),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(Ms(crewd_store::now_ms() + 60_000))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        assert!(cell.set(h.clone()).is_ok());
        assert!(f.state.orchestra.set(h.clone()).is_ok());
        let (seen, forwarded) = mpsc::unbounded_channel();
        let sink = h.clone();
        let bridge = tokio::spawn(pump_collab(rx, move |fact| {
            sink.collab(fact.clone());
            let _ = seen.send(fact);
        }));
        let (output, notifications) = mpsc::channel(32);
        let mut rig = Self {
            f: Some(f),
            h,
            exec,
            actor: Some(actor),
            bridge: Some(bridge),
            forwarded,
            conn: AgentConn::default(),
            output,
            _notifications: notifications,
            channel,
        };
        timeout(LIMIT, rig.h.resume(LISTENER, Origin::Human))
            .await
            .unwrap()
            .unwrap();
        timeout(LIMIT, async {
            loop {
                if rig.h.live(LISTENER).await.unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Keep the actual target busy, so planning is visible as a durable Pending row,
        // independently of any eventual delivery or renderer cursor advancement.
        rig.exec.emit(LISTENER, SessionEvent::Working(true));
        timeout(LIMIT, async {
            loop {
                if rig.h.live(LISTENER).await.unwrap().working {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let state = rig.fixture().state.clone();
        let poster = rig.fixture().worker.clone();
        let token = rig.fixture().token(&poster);
        let response = timeout(
            LIMIT,
            handle(
                &state,
                &mut rig.conn,
                Request {
                    id: 1,
                    method: "agent.attach".into(),
                    params: json!({
                        "sessionId": poster, "token": token,
                    }),
                },
                &rig.output,
            ),
        )
        .await
        .unwrap()
        .into_result()
        .unwrap();
        assert_eq!(
            response["role"], "worker",
            "genuine attached worker fixture"
        );
        rig
    }

    async fn surface(&self) -> Surface {
        let channel = self.channel.clone();
        let tokens = self.fixture().state.tokens.test_len();
        self.fixture()
            .state
            .db
            .call(move |store| -> crewd_collab::Result<Surface> {
                let c = store.conn();
                let count = |table: &str| -> rusqlite::Result<i64> {
                    c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                };
                let mut cursors_statement = c.prepare("SELECT reader,channel_id,seq FROM read_cursors ORDER BY reader,channel_id")?;
                let cursors = cursors_statement.query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                let mut meta_statement = c.prepare("SELECT key,value FROM meta WHERE key LIKE 'cursor.reservation.v1.%' OR key LIKE 'message.provenance.v1.%' ORDER BY key")?;
                let reservation_provenance = meta_statement.query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Surface {
                    channels: count("channels")?,
                    members: count("channel_members")?,
                    listens: count("listens")?,
                    cursor: cursors::cursor(c, LISTENER, &channel)?,
                    messages: count("messages")?,
                    wakes: count("wakes")?,
                    sessions: count("sessions")?,
                    homes: c.query_row("SELECT COUNT(*) FROM channels WHERE kind='human'", [], |r| r.get(0))?,
                    tokens, cursors, reservation_provenance,
                })
            })
            .await
            .unwrap()
            .unwrap()
    }

    async fn call(&mut self, tool: &str, args: Value) -> CallResult {
        let state = self.fixture().state.clone();
        let result = timeout(
            LIMIT,
            handle(
                &state,
                &mut self.conn,
                Request {
                    id: 2,
                    method: "agent.call".into(),
                    params: json!({
                        "tool": tool, "args": args,
                    }),
                },
                &self.output,
            ),
        )
        .await
        .unwrap()
        .into_result()
        .unwrap();
        serde_json::from_value(result).unwrap()
    }

    async fn committed(
        &mut self,
        channel: &str,
        texts: &[&str],
    ) -> (Vec<Message>, Vec<wakes::WakeRow>) {
        let state = self.fixture().state.clone();
        let channel = channel.to_string();
        let posted = state
            .db
            .call(move |store| messages::list(store.conn(), &channel, None, None, 100))
            .await
            .unwrap()
            .unwrap()
            .into_iter()
            .filter(|m| texts.contains(&m.text.as_str()))
            .collect::<Vec<_>>();
        let wanted = posted
            .iter()
            .map(|m| CollabFact::MessageAppended {
                channel: m.channel_id.clone(),
                seq: m.seq,
            })
            .collect::<Vec<_>>();
        timeout(LIMIT, async {
            for wanted in wanted {
                loop {
                    if self.forwarded.recv().await.expect("Hub bridge closed") == wanted {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        // The observed sink enqueue happens before this FIFO request to the real actor.
        timeout(LIMIT, self.h.latches()).await.unwrap().unwrap();
        let sources = posted
            .iter()
            .map(|m| wakes::source::message(&m.id))
            .collect::<Vec<_>>();
        let rows = state
            .db
            .call(move |store| -> crewd_collab::Result<Vec<wakes::WakeRow>> {
                let mut rows = Vec::new();
                for source in sources {
                    rows.extend(wakes::for_source(
                        store.conn(),
                        wakes::WakeKind::Message,
                        &source,
                    )?);
                }
                Ok(rows)
            })
            .await
            .unwrap()
            .unwrap();
        (posted, rows)
    }

    async fn busy(&self, id: &str) {
        if !timeout(LIMIT, self.h.live(id))
            .await
            .unwrap()
            .unwrap()
            .running
        {
            timeout(LIMIT, self.h.resume(id, Origin::Human))
                .await
                .unwrap()
                .unwrap();
        }
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

    fn runtime_counts(&self, ids: &[&str]) -> (usize, usize) {
        (
            ids.iter().map(|id| self.exec.launches(id).len()).sum(),
            ids.iter().map(|id| self.exec.sent(id).len()).sum(),
        )
    }

    async fn post(&mut self) -> (CallResult, Vec<Message>, Vec<wakes::WakeRow>) {
        let channel = self.channel.clone();
        let result = self
            .call("post", json!({"channel":channel,"message":MARKER}))
            .await;
        let (posted, rows) = self.committed(&channel, &[MARKER]).await;
        (result, posted, rows)
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
        // The watcher owns AppState; controlled cancellation + await proves it ended,
        // rather than relying on AgentConn::Drop's unobserved abort.
        if let Some(watcher) = self.conn.watcher.take() {
            watcher.abort();
            let joined = timeout(LIMIT, watcher).await.unwrap();
            assert!(joined.is_ok() || joined.as_ref().is_err_and(|e| e.is_cancelled()));
        }
        let fixture = self.f.take().unwrap();
        let db = fixture.state.db.clone();
        // Actor/Core/DbPort and watcher have released their AppState Arcs. Drop the last
        // AppState owner to close Hub naturally; keep Fixture's remaining TempDir alive
        // until the database has actually shut down below.
        drop(fixture.state);
        timeout(LIMIT, self.bridge.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown()))
            .await
            .unwrap()
            .unwrap();
    }
}

struct Observed {
    result: CallResult,
    posted: Vec<Message>,
    rows: Vec<wakes::WakeRow>,
    before: Surface,
    after: Surface,
    extra_launches: usize,
    extra_sends: usize,
}

async fn observe(listening: bool) -> Observed {
    let mut rig = Rig::new(listening).await;
    let before = rig.surface().await;
    let launches = rig.exec.launches(LISTENER).len();
    let sends = rig.exec.sent(LISTENER).len();
    let (result, posted, rows) = rig.post().await;
    let after = rig.surface().await;
    let extra_launches = rig.exec.launches(LISTENER).len() - launches;
    let extra_sends = rig.exec.sent(LISTENER).len() - sends;
    rig.close().await;
    eprintln!(
        "S72 after cleanup: posted={:?} wakes={rows:?} before={before:?} after={after:?} extra_launches={extra_launches} extra_sends={extra_sends} receipt={result:?}",
        posted.iter().map(|m| (&m.id, m.seq)).collect::<Vec<_>>()
    );
    Observed {
        result,
        posted,
        rows,
        before,
        after,
        extra_launches,
        extra_sends,
    }
}

fn assert_effects(observed: &Observed, expected_wakes: i64) {
    let Observed {
        result,
        posted,
        before,
        after,
        extra_launches,
        extra_sends,
        ..
    } = observed;
    assert!(
        !result.is_error,
        "actual agent.call post must succeed: {}",
        result.text
    );
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].sender_session_id.as_deref(), Some("s-w1"));
    assert!(
        posted[0].mentions.is_empty(),
        "the listener was not forged as a mention"
    );
    assert_eq!(after.messages, before.messages + 1, "no duplicated post");
    assert_eq!(
        after.wakes,
        before.wakes + expected_wakes,
        "only actual Core message planning enqueued wakes"
    );
    assert_eq!(
        (after.sessions, after.homes, after.tokens),
        (before.sessions, before.homes, before.tokens),
        "no session/home/token creation by receipt"
    );
    assert_eq!(after.cursors, before.cursors, "no reader cursor advance");
    assert_eq!(
        after.reservation_provenance, before.reservation_provenance,
        "readonly prediction adds no reservations or trusted relay metadata"
    );
    assert_eq!(
        (after.channels, after.members, after.listens, after.cursor),
        (
            before.channels,
            before.members,
            before.listens,
            before.cursor
        ),
        "receipt has no surface/cursor effects"
    );
    assert_eq!(
        (*extra_launches, *extra_sends),
        (0, 0),
        "busy target remains queued, no receipt delivery"
    );
}

#[tokio::test]
async fn s72_agent_call_listener_receipt_matches_enqueued_target() {
    let observed = observe(true).await;
    assert_effects(&observed, 1);
    let Observed {
        result,
        posted,
        rows,
        ..
    } = observed;
    assert_eq!(
        rows.len(),
        1,
        "real Core must enqueue the eligible listener once"
    );
    assert_eq!(rows[0].session, LISTENER);
    assert_eq!(rows[0].priority, wakes::WakePriority::Auto);
    assert_eq!(rows[0].state, wakes::WakeState::Pending);
    assert_eq!(rows[0].source, wakes::source::message(&posted[0].id));
    assert_eq!(
        result.text.matches(LABEL).count(),
        1,
        "S72 planned listener missing from actual agent.call receipt: {}",
        result.text
    );
}

#[tokio::test]
async fn s72_agent_call_without_listener_reports_no_targets() {
    let observed = observe(false).await;
    assert_effects(&observed, 0);
    let Observed { result, rows, .. } = observed;
    assert!(
        rows.is_empty(),
        "ordinary Session post has no human fallback"
    );
    assert!(
        result
            .text
            .contains(&text::r(Locale::Zh, "r.posted_wakes_none", &[])),
        "{}",
        result.text
    );
    assert!(!result.text.contains(LABEL));
}

fn assert_surfaces_unchanged(before: &Surface, after: &Surface) {
    assert_eq!(
        (
            before.channels,
            before.members,
            before.listens,
            before.sessions,
            before.homes,
            before.tokens
        ),
        (
            after.channels,
            after.members,
            after.listens,
            after.sessions,
            after.homes,
            after.tokens
        ),
        "post receipt must not mint tokens or alter the surface"
    );
    assert_eq!(
        before.cursors, after.cursors,
        "post receipt must not consume unread"
    );
    assert_eq!(
        before.reservation_provenance, after.reservation_provenance,
        "no reservation or trusted relay identity is created by prediction"
    );
}

fn assert_unavailable(result: &CallResult, loc: Locale, channel: &str) {
    let retry_advice = text::get(loc, "r.internal").split_once("{n}").unwrap().1;
    assert!(
        !result.text.contains(retry_advice),
        "S72 committed prediction warning must not advise retrying the post: {}",
        result.text
    );
    let numbered = text::r(loc, "r.posted_wakes_internal", &[("n", "INCIDENT")]);
    let (number_prefix, number_suffix) = numbered.split_once("INCIDENT").unwrap();
    assert!(
        result.text.contains(number_prefix) && result.text.contains(number_suffix),
        "S72 prediction error keeps a localized logged incident number: {}",
        result.text
    );
    let expected = text::r(
        loc,
        "r.posted_wakes_unavailable",
        &[("detail", "DETAIL"), ("channel", channel)],
    );
    let (prefix, suffix) = expected.split_once("DETAIL").unwrap();
    assert!(
        result.text.contains(prefix) && result.text.contains(suffix),
        "localized unavailable/read_channel/do-not-resend warning: {}",
        result.text
    );
    assert!(
        !result.text.contains("synthetic prediction read failure"),
        "internal read details must not leak"
    );
    assert!(
        !result
            .text
            .contains(&text::r(loc, "r.posted_wakes_none", &[])),
        "unavailable is not nobody"
    );
}

#[tokio::test]
async fn s72_agent_call_prediction_read_failure_keeps_committed_success() {
    for loc in [Locale::Zh, Locale::En] {
        for fail_read in [false, true] {
            let mut rig = Rig::new(true).await;
            rig.fixture()
                .state
                .collab(move |tx| crewd_collab::settings::set_locale(tx, loc))
                .await
                .unwrap();
            let channel = rig.channel.clone();
            let before = rig.surface().await;
            let runtime_before = rig.runtime_counts(&[LISTENER]);
            let guard = crate::wake_adapter::set_prediction_test_control(
                &rig.fixture().state,
                &rig.fixture().worker,
                crate::wake_adapter::PredictionTestControl {
                    now: None,
                    fail_read,
                },
            );
            let result = rig
                .call("post", json!({"channel":channel,"message":MARKER}))
                .await;
            drop(guard);
            let (posted, rows) = rig.committed(&channel, &[MARKER]).await;
            let after = rig.surface().await;
            // An actual repeat request is refused by the existing gate; the failed read
            // never retries a post internally or tells the caller nothing was sent.
            let duplicate = rig
                .call("post", json!({"channel":channel,"message":MARKER}))
                .await;
            let after_repeat = rig.surface().await;
            let runtime_after = rig.runtime_counts(&[LISTENER]);
            rig.close().await;
            assert!(
                !result.is_error,
                "S72 successful committed post survives prediction read error: {}",
                result.text
            );
            assert_eq!(posted.len(), 1);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].session, LISTENER);
            assert_eq!(rows[0].state, wakes::WakeState::Pending);
            assert_eq!(rows[0].priority, wakes::WakePriority::Auto);
            assert_eq!(rows[0].source, wakes::source::message(&posted[0].id));
            assert!(
                result.text.contains(&format!("#{}", posted[0].seq)),
                "original committed ref remains visible"
            );
            assert_eq!(after.messages, before.messages + 1);
            assert_eq!(after.wakes, before.wakes + 1);
            assert_surfaces_unchanged(&before, &after);
            assert_eq!(
                after_repeat, after,
                "no whole-message resend or duplicate side effect"
            );
            assert_eq!(
                runtime_after, runtime_before,
                "read-failure receipt causes no launch/send"
            );
            assert!(duplicate.is_error);
            assert!(duplicate.text.contains(&text::r(
                loc,
                "r.post_duplicate",
                &[("seq", &posted[0].seq.to_string())]
            )));
            if fail_read {
                assert_unavailable(&result, loc, &channel);
                assert_eq!(result.text.matches(LABEL).count(), 0);
            } else {
                assert_eq!(result.text.matches(LABEL).count(), 1);
                assert!(
                    !result.text.contains(
                        text::get(loc, "r.posted_wakes_unavailable")
                            .split('{')
                            .next()
                            .unwrap()
                    )
                );
            }
        }
    }
}

#[tokio::test]
async fn s72_agent_call_partial_prefix_prediction_and_read_failure_preserve_refund() {
    for loc in [Locale::Zh, Locale::En] {
        for fail_read in [false, true] {
            let mut rig = Rig::new(true).await;
            rig.fixture()
                .state
                .collab(move |tx| crewd_collab::settings::set_locale(tx, loc))
                .await
                .unwrap();
            let channel = rig.channel.clone();
            let before = rig.surface().await;
            let runtime_before = rig.runtime_counts(&[LISTENER]);
            let guard = crate::wake_adapter::set_prediction_test_control(
                &rig.fixture().state,
                &rig.fixture().worker,
                crate::wake_adapter::PredictionTestControl {
                    now: None,
                    fail_read,
                },
            );
            let large = "x".repeat(300 * 1024);
            let tail = "S72_REJECTED_TAIL_MUST_NOT_BE_POSTED";
            let result=rig.call("post",json!({"channel":channel,"messages":[{"text":MARKER},{"text":large},{"text":tail}]})).await;
            drop(guard);
            let (posted, rows) = rig.committed(&channel, &[MARKER, tail]).await;
            let after = rig.surface().await;
            let repeated=rig.call("post",json!({"channel":channel,"messages":[{"text":MARKER},{"text":large},{"text":tail}]})).await;
            let after_repeat = rig.surface().await;
            let mut followups = Vec::new();
            for i in 0..5 {
                let body = format!("S72_REFUND_FOLLOWUP_{i}");
                let response = rig
                    .call("post", json!({"channel":channel,"message":body}))
                    .await;
                let (messages, wakes) = rig.committed(&channel, &[&body]).await;
                followups.push((response, messages, wakes));
            }
            let throttled = rig
                .call(
                    "post",
                    json!({"channel":channel,"message":"S72_SIXTH_FOLLOWUP_REFUSED"}),
                )
                .await;
            let final_surface = rig.surface().await;
            let runtime_after = rig.runtime_counts(&[LISTENER]);
            rig.close().await;
            assert!(
                result.is_error,
                "partial remains an error even if prediction succeeds"
            );
            assert_eq!(
                posted.len(),
                1,
                "only the actual committed prefix, not failed/unattempted tails"
            );
            assert_eq!(posted[0].text, MARKER);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].session, LISTENER);
            assert_eq!(rows[0].source, wakes::source::message(&posted[0].id));
            assert_eq!(rows[0].priority, wakes::WakePriority::Auto);
            assert_eq!(rows[0].state, wakes::WakeState::Pending);
            let partial = text::r(
                loc,
                "r.post_partial",
                &[
                    ("sent", &format!("#{}", posted[0].seq)),
                    ("index", "2"),
                    ("total", "3"),
                    ("detail", "DETAIL"),
                ],
            );
            let (prefix, suffix) = partial.split_once("DETAIL").unwrap();
            assert!(
                result.text.contains(prefix) && result.text.contains(suffix),
                "original failed index/detail/no-whole-batch-retry contract: {}",
                result.text
            );
            if fail_read {
                assert_unavailable(&result, loc, &channel);
                assert_eq!(result.text.matches(LABEL).count(), 0);
            } else {
                assert_eq!(
                    result.text.matches(LABEL).count(),
                    1,
                    "actual prefix is predicted even though tail failed"
                );
            }
            assert_eq!(after.messages, before.messages + 1);
            assert_eq!(after.wakes, before.wakes + 1);
            assert_surfaces_unchanged(&before, &after);
            assert!(repeated.is_error);
            assert_eq!(
                after_repeat, after,
                "failed read or tail must not cause a replay of committed prefix"
            );
            for (response, messages, wakes) in followups {
                assert!(
                    !response.is_error,
                    "both unposted tokens refunded: {}",
                    response.text
                );
                assert_eq!(messages.len(), 1);
                assert_eq!(wakes.len(), 1);
                assert_eq!(wakes[0].source, wakes::source::message(&messages[0].id));
            }
            assert!(
                throttled.is_error,
                "six tokens consumed by the prefix and five actual posts"
            );
            assert!(
                throttled.text.contains(
                    text::get(loc, "r.post_throttled")
                        .split("{retry_secs}")
                        .next()
                        .unwrap()
                ),
                "sixth post is actually rate-limited: {}",
                throttled.text
            );
            assert_eq!(final_surface.messages, before.messages + 6);
            assert_eq!(final_surface.wakes, before.wakes + 6);
            assert_surfaces_unchanged(&before, &final_surface);
            assert_eq!(
                runtime_after, runtime_before,
                "committed prefix, prediction error and refunds do not cause extra launch/send"
            );
        }
    }
}

#[tokio::test]
async fn s72_agent_call_dm_peer_and_third_listener_use_the_same_plan() {
    for expired in [false, true] {
        let mut rig = Rig::new(false).await;
        let peer = rig.fixture().captain.clone();
        rig.busy(&peer).await;
        let poster = rig.fixture().me(&rig.fixture().worker).await;
        let (dm, _) = port::ensure_dm(&rig.fixture().state, &poster, &peer)
            .await
            .unwrap();
        let channel = dm.id;
        let listen_channel = channel.clone();
        rig.fixture()
            .state
            .collab(move |tx| {
                crewd_collab::listens::set(tx, LISTENER, &listen_channel, None, vec![])?;
                if expired {
                    tx.conn().execute(
                        "UPDATE listens SET until=?1 WHERE session_id=?2 AND channel_id=?3",
                        rusqlite::params![crewd_store::now_ms() - 1, LISTENER, listen_channel],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        let peer_id = peer.clone();
        let (peer_label, listener_label) = rig
            .fixture()
            .state
            .db
            .call(move |s| -> crewd_collab::Result<_> {
                Ok((
                    crewd_collab::render::target_label(s.conn(), Locale::Zh, &peer_id)?,
                    crewd_collab::render::target_label(s.conn(), Locale::Zh, LISTENER)?,
                ))
            })
            .await
            .unwrap()
            .unwrap();
        let before = rig.surface().await;
        let runtime_before = rig.runtime_counts(&[LISTENER, &peer]);
        let result = rig.call("post", json!({"to":peer,"message":MARKER})).await;
        let (posted, rows) = rig.committed(&channel, &[MARKER]).await;
        let after = rig.surface().await;
        let runtime_after = rig.runtime_counts(&[LISTENER, &peer]);
        rig.close().await;
        assert!(!result.is_error, "{}", result.text);
        assert_eq!(posted.len(), 1);
        assert!(posted[0].mentions.is_empty());
        let mut labels = vec![peer_label];
        if !expired {
            labels.push(listener_label);
        }
        assert!(
            result.text.contains(&text::r(
                Locale::Zh,
                "r.posted_wakes",
                &[("names", &labels.join(", "))]
            )),
            "DM includes actual safe peer/listener labels in the prospective target list: {}",
            result.text
        );
        assert_eq!(result.text.matches(LABEL).count(), usize::from(!expired));
        let mut ids = rows.iter().map(|w| w.session.clone()).collect::<Vec<_>>();
        ids.sort();
        let mut expected = vec![peer];
        if !expired {
            expected.push(LISTENER.into());
        }
        expected.sort();
        assert_eq!(ids, expected, "actual DM peer plus eligible listeners");
        for row in &rows {
            assert_eq!(row.source, wakes::source::message(&posted[0].id));
            assert_eq!(row.priority, wakes::WakePriority::Auto);
            assert_eq!(row.state, wakes::WakeState::Pending);
        }
        assert_eq!(after.messages, before.messages + 1);
        assert_eq!(after.wakes, before.wakes + rows.len() as i64);
        assert_surfaces_unchanged(&before, &after);
        assert_eq!(
            runtime_after, runtime_before,
            "DM prospective receipt has no runtime delivery side effect"
        );
    }
}

#[tokio::test]
async fn s72_agent_call_reply_uses_persisted_sender_and_live_membership() {
    for kind in ["member", "departed", "system"] {
        let mut rig = Rig::new(true).await;
        let origin = "s-s72-reply-origin";
        let label = "S72_ORIGINAL_SENDER";
        rig.fixture()
            .add_session(
                &rig.fixture().crew,
                origin,
                crate::agent::Role::Worker,
                label,
            )
            .await;
        let captain = rig.fixture().me(&rig.fixture().captain).await;
        port::add_members(
            &rig.fixture().state,
            &captain,
            &rig.channel,
            vec![port::MemberRef::Session(origin.into())],
        )
        .await
        .unwrap();
        rig.busy(origin).await;
        let channel = rig.channel.clone();
        let seed_channel = channel.clone();
        let seed = rig
            .fixture()
            .state
            .collab(move |tx| {
                messages::post(
                    tx,
                    if kind == "system" {
                        crewd_collab::Sender::System
                    } else {
                        crewd_collab::Sender::Session(origin.into())
                    },
                    messages::NewPost {
                        channel_id: seed_channel,
                        text: "S72_REAL_PERSISTED_REPLY_SEED".into(),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .message;
        rig.committed(&channel, &["S72_REAL_PERSISTED_REPLY_SEED"])
            .await;
        if kind == "departed" {
            port::remove_member(
                &rig.fixture().state,
                &captain,
                &channel,
                port::MemberRef::Session(origin.into()),
            )
            .await
            .unwrap();
        }
        let before = rig.surface().await;
        let runtime_before = rig.runtime_counts(&[LISTENER, origin]);
        let result = rig
            .call("post", json!({"reply_to":seed.id,"message":MARKER}))
            .await;
        let (posted, rows) = rig.committed(&channel, &[MARKER]).await;
        let after = rig.surface().await;
        let runtime_after = rig.runtime_counts(&[LISTENER, origin]);
        rig.close().await;
        assert!(!result.is_error, "{}", result.text);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].in_reply_to.as_deref(), Some(seed.id.as_str()));
        assert_eq!(result.text.matches(LABEL).count(), 1);
        assert_eq!(
            result.text.matches(label).count(),
            usize::from(kind == "member")
        );
        let mut expected = vec![LISTENER.to_string()];
        if kind == "member" {
            expected.push(origin.into());
        }
        expected.sort();
        let mut ids = rows.iter().map(|w| w.session.clone()).collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, expected);
        for row in &rows {
            assert_eq!(row.source, wakes::source::message(&posted[0].id));
            assert_eq!(row.priority, wakes::WakePriority::Auto);
            assert_eq!(row.state, wakes::WakeState::Pending);
        }
        assert_eq!(after.messages, before.messages + 1);
        assert_eq!(after.wakes, before.wakes + rows.len() as i64);
        assert_surfaces_unchanged(&before, &after);
        assert_eq!(
            runtime_after, runtime_before,
            "reply prospective receipt has no runtime delivery side effect"
        );
    }
}
