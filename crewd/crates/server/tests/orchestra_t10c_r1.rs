//! R1 regressions through public dispatch/OrchestraControl, real DbPort and synthetic SQLite.
//! Executor events/results are controlled; one test also exercises actual sans-IO exec Core.
use crewd_exec::delivery::Subject;
use crewd_exec::driver::{Closed, SessionStatus};
use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{AnswerOutcome, DeliveryOutcome, HeldDraft, SessionEvent};
use crewd_server::orchestra::clock::ManualClock;
use crewd_server::orchestra::control::OrchestraControl;
use crewd_server::orchestra::core::{Origin, StartSession};
use crewd_server::orchestra::db::DbPort;
use crewd_server::orchestra::exec::*;
use crewd_server::orchestra::hooks::Hooks;
use crewd_server::orchestra::{self, Handle, OrchestraConfig};
use crewd_server::{AppState, dispatch};
use crewd_term::{DecisionEvent, DecisionKind, PendingDecision, Snapshot};
use crewd_wake::{MIN, Ms};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};
#[derive(Default)]
struct State {
    sessions: HashMap<String, mpsc::UnboundedSender<SessionEvent>>,
    observers: HashMap<String, Arc<dyn crewd_exec::driver::DeliverySuccessObserver>>,
    boxes: HashMap<String, PendingDecision>,
    sends: Vec<(String, u64)>,
    answers: Vec<(String, AnswerSteps)>,
    choices: Vec<(String, usize)>,
    raws: usize,
    interrupts: usize,
    screens: Vec<usize>,
    next: u64,
    answer_holds: HashMap<String, VecDeque<oneshot::Receiver<()>>>,
    active_answers: usize,
    max_active_answers: usize,
    answer_attempts: usize,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);
impl Fake {
    fn emit(&self, id: &str, e: SessionEvent) {
        let mut s = self.0.lock().unwrap();
        match &e {
            SessionEvent::Decision(DecisionEvent::Appeared(d)) => {
                s.boxes.insert(id.into(), d.clone());
            }
            SessionEvent::Decision(DecisionEvent::Cleared) => {
                s.boxes.remove(id);
            }
            _ => {}
        }
        let observer = s.observers.get(id).cloned();
        let events = s.sessions[id].clone();
        drop(s);
        // Match the actual producer seam: only accepted Delivered, before channel emission.
        if let SessionEvent::Delivery {
            id: delivery,
            outcome: DeliveryOutcome::Delivered,
            ..
        } = &e
            && let Some(observer) = observer
        {
            observer.emitted(*delivery);
        }
        events.send(e).unwrap();
    }
    fn delivery(&self, id: &str, n: u64, outcome: DeliveryOutcome) {
        self.emit(
            id,
            SessionEvent::Delivery {
                id: n,
                subject: Subject::Steer,
                outcome,
            },
        );
    }
    fn hold_answer(&self, id: &str) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel();
        self.0
            .lock()
            .unwrap()
            .answer_holds
            .entry(id.into())
            .or_default()
            .push_back(rx);
        tx
    }
    fn begin_answer(&self, id: &str) -> Option<oneshot::Receiver<()>> {
        let mut s = self.0.lock().unwrap();
        s.answer_attempts += 1;
        s.active_answers += 1;
        s.max_active_answers = s.max_active_answers.max(s.active_answers);
        s.answer_holds.get_mut(id).and_then(VecDeque::pop_front)
    }
    async fn finish_answer(&self, hold: Option<oneshot::Receiver<()>>) {
        if let Some(h) = hold {
            let _ = h.await;
        }
        self.0.lock().unwrap().active_answers -= 1;
    }
    fn last_id(&self) -> u64 {
        self.0.lock().unwrap().sends.last().unwrap().1
    }
}
struct Basic {
    fake: Fake,
    id: String,
}
struct Structured(Basic);
impl Basic {
    fn base(&self) -> &Basic {
        self
    }
    fn send_impl(&self) -> u64 {
        let mut s = self.fake.0.lock().unwrap();
        s.next += 1;
        let n = s.next;
        s.sends.push((self.id.clone(), n));
        n
    }
    fn answer_impl(&self, expect: String, steps: AnswerSteps) -> Result<AnswerOutcome, Closed> {
        let mut s = self.fake.0.lock().unwrap();
        if s.boxes
            .get(&self.id)
            .is_none_or(|d| d.fingerprint() != expect)
        {
            return Ok(AnswerOutcome::Conflict {
                now: s.boxes.get(&self.id).cloned(),
            });
        }
        s.answers.push((self.id.clone(), steps));
        s.boxes.remove(&self.id);
        s.sessions[&self.id]
            .send(SessionEvent::Decision(DecisionEvent::Cleared))
            .unwrap();
        Ok(AnswerOutcome::Answered {
            cleared: true,
            now: None,
        })
    }
}
impl Structured {
    fn base(&self) -> &Basic {
        &self.0
    }
}
macro_rules! common {
    ($access:ident) => {
        fn send(&self, _text: String, _context: &Context) -> Result<u64, ExecSendError> {
            Ok(self.$access().send_impl())
        }
        fn send_bound(
            &self,
            _text: String,
            context: &Context,
            binding: &dyn crewd_exec::driver::DeliveryBinding,
        ) -> Result<u64, ExecSendError> {
            check_context(self.accepts_context(), context)?;
            let b = self.$access();
            let n = {
                let mut s = b.fake.0.lock().unwrap();
                s.next += 1;
                s.next
            };
            // The original capability must register the actual ID before synthetic enqueue.
            binding.register(n)?;
            b.fake.0.lock().unwrap().sends.push((b.id.clone(), n));
            Ok(n)
        }
        fn raw(&self, _bytes: Vec<u8>) -> Result<(), Closed> {
            self.$access().fake.0.lock().unwrap().raws += 1;
            Ok(())
        }
        fn interrupt(&self) -> Result<(), Closed> {
            self.$access().fake.0.lock().unwrap().interrupts += 1;
            Ok(())
        }
        fn answer(
            &self,
            expect: String,
            steps: AnswerSteps,
        ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
            let b = self.$access();
            let fake = b.fake.clone();
            let hold = fake.begin_answer(&b.id);
            let r = b.answer_impl(expect, steps);
            Box::pin(async move {
                fake.finish_answer(hold).await;
                r
            })
        }
        fn stop(&self) -> Result<(), Closed> {
            Ok(())
        }
        fn clear_quota_health(&self) -> Result<(), Closed> {
            Ok(())
        }
        fn screen_text(&self, n: usize) -> BoxFut<'_, Result<String, Closed>> {
            self.$access().fake.0.lock().unwrap().screens.push(n);
            Box::pin(async { Ok("synthetic screen".into()) })
        }
        fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>> {
            Box::pin(async { Ok(None) })
        }
        fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
            Box::pin(async { Err(Closed) })
        }
    };
}
impl ExecHandle for Basic {
    common!(base);
}
impl ExecHandle for Structured {
    common!(base);
    fn answer_choice(
        &self,
        expect: String,
        choice: usize,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        let b = &self.0;
        let fake = b.fake.clone();
        let hold = fake.begin_answer(&b.id);
        let mut s = b.fake.0.lock().unwrap();
        let d = s.boxes.get(&b.id).cloned();
        let r = if d
            .as_ref()
            .is_some_and(|d| d.fingerprint() == expect && choice < d.options.len())
        {
            s.choices.push((b.id.clone(), choice));
            s.boxes.remove(&b.id);
            s.sessions[&b.id]
                .send(SessionEvent::Decision(DecisionEvent::Cleared))
                .unwrap();
            AnswerOutcome::Answered {
                cleared: true,
                now: None,
            }
        } else {
            AnswerOutcome::Conflict { now: d }
        };
        drop(s);
        Box::pin(async move {
            fake.finish_answer(hold).await;
            Ok(r)
        })
    }
}
struct FakeExecutor {
    fake: Fake,
    structured: bool,
}
impl Executor for FakeExecutor {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut s = self.fake.0.lock().unwrap();
            s.sessions.insert(req.session_id.clone(), tx);
            if let Some(observer) = req.delivery_observer {
                s.observers.insert(req.session_id.clone(), observer);
            } else {
                s.observers.remove(&req.session_id);
            }
        }
        let b = Basic {
            fake: self.fake.clone(),
            id: req.session_id,
        };
        let h: Arc<dyn ExecHandle> = if self.structured {
            Arc::new(Structured(b))
        } else {
            Arc::new(b)
        };
        Box::pin(async move { Ok((h, rx)) })
    }
}
struct Rig {
    state: Arc<AppState>,
    h: Handle,
    fake: Fake,
    clock: Arc<ManualClock>,
    crew: String,
    abort: tokio::task::AbortHandle,
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.abort.abort();
    }
}
impl Rig {
    async fn new(structured: bool) -> Self {
        let db = crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap();
        let cell = Arc::new(OnceLock::new());
        let control = Arc::new(OrchestraControl::new(cell.clone(), db.clone()));
        let state = Arc::new(AppState::new(db).with_sessions(control));
        let crew = state
            .collab(|tx| {
                crewd_collab::crews::create(
                    tx,
                    crewd_collab::crews::NewCrew {
                        title: "Synthetic review".into(),
                        working_directory: env!("CARGO_MANIFEST_DIR").into(),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .id;
        let port = Arc::new(DbPort::new(
            state.clone(),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("unused-synthetic.sqlite"),
        ));
        let fake = Fake::default();
        let clock = Arc::new(ManualClock::new(Ms(2_000_000_000_000)));
        let cfg = OrchestraConfig {
            stagger_ms: 0,
            heartbeat_ms: 5,
            retry_jitter: 0.0,
            jitter_seed: Some(1),
            ..Default::default()
        };
        let execs = Executors::new()
            .with(
                "claude",
                Arc::new(FakeExecutor {
                    fake: fake.clone(),
                    structured,
                }),
            )
            .with(
                "codex",
                Arc::new(FakeExecutor {
                    fake: fake.clone(),
                    structured,
                }),
            );
        let (h, actor) = orchestra::spawn(port, execs, Hooks::default(), clock.clone(), cfg)
            .await
            .unwrap();
        assert!(cell.set(h.clone()).is_ok());
        assert!(state.orchestra.set(h.clone()).is_ok());
        Self {
            state,
            h,
            fake,
            clock,
            crew,
            abort: actor.abort_handle(),
        }
    }
    async fn start(&self, title: &str, captain: bool) -> String {
        self.start_in(&self.crew, title, captain, "claude").await
    }
    async fn start_in(&self, crew: &str, title: &str, captain: bool, executor: &str) -> String {
        let r = self
            .h
            .start(
                StartSession {
                    codex_options: None,

                    crew_id: crew.into(),
                    captain,
                    executor: executor.into(),
                    brief: "synthetic brief".into(),
                    title: Some(title.into()),
                    model: None,
                    effort: None,
                    isolation: false,
                },
                Origin::Human,
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(2), async {
            while !self.h.live(&r.id).await.unwrap().running {
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        r.id
    }
    async fn rpc(&self, method: &str, params: Value) -> Result<Value, crewd_proto::RpcError> {
        dispatch::handle(
            &self.state,
            crewd_proto::Request {
                id: 1,
                method: method.into(),
                params,
            },
        )
        .await
        .into_result()
    }
    async fn show(&self, id: &str, d: PendingDecision) {
        self.fake.emit(
            id,
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        );
        timeout(Duration::from_secs(2), async {
            loop {
                if self.h.live(id).await.unwrap().pending_decision == Some(d.clone()) {
                    break;
                }
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn posts(&self) -> Vec<(String, String, String)> {
        self.state
            .db
            .call(|s| {
                let mut q = s.conn().prepare(
                    "SELECT channel_id,text,mentions FROM messages WHERE sender_kind='system' ORDER BY seq",
                ).unwrap();
                q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            })
            .await
            .unwrap()
    }
    async fn settled(&self) {
        sleep(Duration::from_millis(30)).await;
        let _ = self.h.decisions().await.unwrap();
    }
}
fn terminal() -> PendingDecision {
    PendingDecision {
        kind: DecisionKind::Terminal,
        prompt: "Trust?".into(),
        options: vec!["Yes".into(), "No".into()],
        numbered: true,
        selected: Some(1),
    }
}
fn runner(id: &str) -> PendingDecision {
    PendingDecision {
        kind: DecisionKind::RunnerRequest {
            request_id: id.into(),
        },
        numbered: false,
        selected: None,
        ..terminal()
    }
}
#[tokio::test]
async fn public_terminal_stale_actual_choice_zero_and_inspect_cap() {
    let r = Rig::new(false).await;
    let id = r.start("worker", false).await;
    let d = terminal();
    r.show(&id, d.clone()).await;
    let err = r
        .rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":"stale","choice":0}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, -32009);
    assert_eq!(err.data.as_ref().unwrap().refusal, "decision.changed");
    assert_eq!(
        err.data.unwrap().decision.unwrap().fingerprint,
        d.fingerprint()
    );
    for p in [
        json!({"sessionId":id}),
        json!({"sessionId":id,"maxLines":999}),
    ] {
        assert!(r.rpc("session.inspect", p).await.is_ok());
    }
    assert_eq!(r.fake.0.lock().unwrap().screens, vec![60, 400]);
    assert_eq!(
        r.rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":0})
        )
        .await
        .unwrap(),
        json!({"cleared":true})
    );
    assert_eq!(
        r.fake.0.lock().unwrap().answers[0].1,
        vec![(b"1".to_vec(), None)]
    );
    assert_eq!(
        r.rpc("decision.list", Value::Null).await.unwrap(),
        json!([])
    );
}
#[tokio::test]
async fn public_structured_request_routes_choice_and_default_is_unsupported() {
    for supported in [false, true] {
        let r = Rig::new(supported).await;
        let id = r.start("runner", false).await;
        let d = runner("id-1");
        r.show(&id, d.clone()).await;
        assert_ne!(d.fingerprint(), runner("id-2").fingerprint());
        let list = r.rpc("decision.list", Value::Null).await.unwrap();
        assert_eq!(list[0]["decision"]["kind"], "runner_request");
        let out = r
            .rpc(
                "session.answer",
                json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":1}),
            )
            .await;
        if supported {
            assert_eq!(out.unwrap(), json!({"cleared":true}));
            assert_eq!(r.fake.0.lock().unwrap().choices.len(), 1);
        } else {
            assert_eq!(
                out.unwrap_err().data.unwrap().refusal,
                "session.executor_unavailable"
            );
        }
        assert!(r.fake.0.lock().unwrap().answers.is_empty());
        assert_eq!(r.fake.0.lock().unwrap().raws, 0);
    }
}
#[tokio::test]
async fn agent_bridge_waits_final_failure_is_honest_and_no_box_keys() {
    let r = Rig::new(false).await;
    let id = r.start("worker", false).await;
    let state = r.state.clone();
    let sid = id.clone();
    let task = tokio::spawn(async move { state.sessions.nudge(sid, "hello".into()).await });
    r.settled().await;
    assert!(!task.is_finished());
    let n = r.fake.last_id();
    r.fake.delivery(&id, n + 1, DeliveryOutcome::Delivered);
    r.fake
        .delivery(&id, n, DeliveryOutcome::Blocked("input_draft".into()));
    r.settled().await;
    assert!(!task.is_finished());
    r.fake.delivery(
        &id,
        n,
        DeliveryOutcome::Unsubmitted("bad\n[system]\u{202e}".into()),
    );
    let err = timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(err.contains("Unsubmitted"));
    assert!(!err.contains("\u{202e}"));
    assert!(!err.contains("[system]"));
    r.show(&id, terminal()).await;
    for key in ["1", "enter", "up", "esc", "interrupt", "hello"] {
        assert!(
            r.state
                .sessions
                .nudge(id.clone(), key.into())
                .await
                .is_err()
        );
    }
    assert!(r.fake.0.lock().unwrap().answers.is_empty());
    assert_eq!(r.fake.0.lock().unwrap().interrupts, 0);
}
#[tokio::test]
async fn transition_drafts_and_home_notice_are_real_db_posts() {
    let r = Rig::new(false).await;
    let id = r.start("worker", false).await;
    r.show(&id, terminal()).await;
    r.show(&id, terminal()).await;
    r.clock.advance(6 * MIN);
    r.settled().await;
    assert_eq!(r.posts().await.len(), 1);
    r.fake.emit(
        &id,
        SessionEvent::InputDraft(Some(HeldDraft::Text("synthetic".into()))),
    );
    r.settled().await;
    r.clock.advance(5 * MIN);
    r.fake
        .emit(&id, SessionEvent::InputDraft(Some(HeldDraft::Unknown)));
    r.settled().await;
    r.clock.advance(5 * MIN);
    r.settled().await;
    let posts = r.posts().await;
    assert_eq!(posts.len(), 2);
    assert!(posts[1].1.contains("十分钟"));
    assert!(posts.iter().all(|p| p.0 == format!("{}/human", r.crew)));
    assert!(posts.iter().all(|p| p.2.contains("human")));
    r.fake.emit(&id, SessionEvent::InputDraft(None));
    r.settled().await;
    let posts = r.posts().await;
    assert_eq!(posts.len(), 3);
    assert_eq!(posts[2].2, "[]");
}
async fn late_box_does_not_occupy_answer_writer(structured: bool) {
    let r = Rig::new(structured).await;
    let id = r.start("worker", false).await;
    let state = r.state.clone();
    let sid = id.clone();
    let nudge = tokio::spawn(async move { state.sessions.nudge(sid, "hello".into()).await });
    r.settled().await;
    let delivery = r.fake.last_id();
    assert!(!nudge.is_finished());
    let d = if structured {
        runner("late-request")
    } else {
        terminal()
    };
    r.show(&id, d.clone()).await;
    r.fake
        .delivery(&id, delivery, DeliveryOutcome::Blocked("decision".into()));
    r.settled().await;
    let stale = r
        .rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":"stale","choice":0}),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.data.unwrap().refusal, "decision.changed");
    let answer = timeout(
        Duration::from_millis(400),
        r.rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":0}),
        ),
    )
    .await;
    assert!(answer.is_ok(), "human answer waits behind nonfinal nudge");
    assert_eq!(answer.unwrap().unwrap(), json!({"cleared":true}));
    assert!(!nudge.is_finished(), "answer is not a Delivered receipt");
    if structured {
        assert_eq!(r.fake.0.lock().unwrap().choices.len(), 1);
        assert!(r.fake.0.lock().unwrap().answers.is_empty());
    } else {
        assert_eq!(r.fake.0.lock().unwrap().answers.len(), 1);
    }
    r.fake
        .delivery(&id, delivery + 1, DeliveryOutcome::Delivered);
    r.settled().await;
    assert!(!nudge.is_finished(), "wrong id never succeeds");
    r.fake.delivery(&id, delivery, DeliveryOutcome::Delivered);
    assert!(
        timeout(Duration::from_secs(2), nudge)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
}
#[tokio::test]
async fn r1_pending_terminal_nudge_permits_public_human_answer_then_matching_delivered() {
    late_box_does_not_occupy_answer_writer(false).await;
}
#[tokio::test]
async fn r1_pending_runner_request_nudge_permits_structured_human_answer_then_matching_delivered() {
    late_box_does_not_occupy_answer_writer(true).await;
}
#[tokio::test]
async fn r1_demoted_quota_captain_working_must_clear_its_existing_machine_episode() {
    let r = Rig::new(false).await;
    let old = r.start("oldcap", true).await;
    let new = r.start("newcap", false).await;
    r.fake.emit(
        &old,
        SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "synthetic"))),
    );
    r.settled().await;
    assert_eq!(r.posts().await.len(), 1);
    let out = r
        .rpc("crew.setCaptain", json!({"crewId":r.crew,"sessionId":new}))
        .await;
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(r.posts().await.len(), 1, "handoff is not quota recovery");
    assert_eq!(
        r.rpc("session.get", json!({"sessionId":old}))
            .await
            .unwrap()["role"],
        "worker"
    );
    assert_eq!(
        r.rpc("session.get", json!({"sessionId":new}))
            .await
            .unwrap()["role"],
        "captain"
    );
    r.fake.emit(&old, SessionEvent::Working(true));
    r.fake.emit(&new, SessionEvent::Working(true));
    r.settled().await;
    assert!(r.h.live(&old).await.unwrap().working);
    assert!(r.h.live(&new).await.unwrap().working);
    let posts = r.posts().await;
    println!(
        "after demoted original blocker and new captain both Working(true): system posts={}",
        posts.len()
    );
    assert_eq!(
        posts.len(),
        2,
        "quota recovery missing after a real captain handoff"
    );
    assert_eq!(posts[1].2, "[]");
    r.fake.emit(
        &new,
        SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "new episode"))),
    );
    r.settled().await;
    assert_eq!(r.posts().await.len(), 3, "new episode can raise again");
}
#[test]
fn real_executor_dialog_block_has_no_automatic_terminal_receipt() {
    use crewd_exec::session::{Action, Core, CoreConfig};
    use crewd_term::ScreenConfig;
    let t = std::time::Instant::now();
    let mut c = Core::new(
        CoreConfig::default(),
        ScreenConfig::default(),
        t,
        vec![],
        None,
    );
    c.send(41, "synthetic nudge", t);
    let dialog=b"\x1b[2J\x1b[HTrust this synthetic folder?\r\n\r\n\xe2\x9d\xaf 1. Yes\r\n  2. No\r\n\r\nEnter to confirm";
    c.on_output(dialog, t);
    let mut actions = c.take_actions();
    for secs in [4, 60, 600, 3600] {
        c.on_tick(t + Duration::from_secs(secs));
        actions.extend(c.take_actions());
    }
    assert!(c.pending_decision().is_some());
    assert!(actions.iter().any(|a| matches!(
        a,
        Action::Event(SessionEvent::Delivery {
            id: 41,
            outcome: DeliveryOutcome::Blocked(_),
            ..
        })
    )));
    assert!(!actions.iter().any(|a|matches!(a,Action::Event(SessionEvent::Delivery{id:41,outcome,..}) if outcome.is_final())));
    println!(
        "actual executor Core: synthetic dialog remains pending at +3600s; no final nudge receipt"
    );
}

// Public-routing controls. A gated adapter performs a synthetic CAS and emits Cleared at
// entry, but holds its result: this independently controls the real actor's WriteDone seam.
#[tokio::test]
async fn r1_final_nudge_receipt_never_releases_active_answer_or_overtakes_queued_body() {
    for collision in [
        None,
        Some(HeldDraft::Unknown),
        Some(HeldDraft::Text("human draft".into())),
    ] {
        let r = Rig::new(false).await;
        let id = r.start("serial-worker", false).await;
        let mut nudge = Box::pin(r.state.sessions.nudge(id.clone(), "first body".into()));
        assert!(
            timeout(Duration::from_millis(30), &mut nudge)
                .await
                .is_err()
        );
        assert_eq!(r.fake.0.lock().unwrap().sends.len(), 1);
        let first_delivery = r.fake.last_id();
        let d1 = terminal();
        r.show(&id, d1.clone()).await;
        r.fake.delivery(
            &id,
            first_delivery,
            DeliveryOutcome::Blocked("decision".into()),
        );
        let release_first = r.fake.hold_answer(&id);
        let mut answer1 = Box::pin(r.rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":d1.fingerprint(),"choice":0}),
        ));
        assert!(
            timeout(Duration::from_millis(30), &mut answer1)
                .await
                .is_err()
        );
        assert_eq!(
            r.fake.0.lock().unwrap().answer_attempts,
            1,
            "first answer entered, not waiting for the nudge"
        );
        r.settled().await;
        assert!(r.h.live(&id).await.unwrap().pending_decision.is_none());
        let mut body = Box::pin(r.state.sessions.nudge(id.clone(), "next body".into()));
        assert!(timeout(Duration::from_millis(30), &mut body).await.is_err());
        let d2 = PendingDecision {
            prompt: "Second real synthetic question?".into(),
            ..terminal()
        };
        r.show(&id, d2.clone()).await;
        let release_second = r.fake.hold_answer(&id);
        let mut answer2 = Box::pin(r.rpc(
            "session.answer",
            json!({"sessionId":id,"fingerprint":d2.fingerprint(),"choice":1}),
        ));
        assert!(
            timeout(Duration::from_millis(30), &mut answer2)
                .await
                .is_err()
        );
        assert_eq!(
            r.fake.0.lock().unwrap().answer_attempts,
            1,
            "second answer must serialize behind the active writer"
        );
        assert_eq!(r.fake.0.lock().unwrap().sends.len(), 1);
        release_first.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), &mut answer1)
                .await
                .unwrap()
                .unwrap(),
            json!({"cleared":true})
        );
        timeout(Duration::from_secs(2), async {
            while r.fake.0.lock().unwrap().answer_attempts != 2 {
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(r.fake.0.lock().unwrap().max_active_answers, 1);
        r.fake
            .delivery(&id, first_delivery, DeliveryOutcome::Delivered);
        assert!(
            timeout(Duration::from_secs(2), &mut nudge)
                .await
                .unwrap()
                .is_ok()
        );
        r.settled().await;
        assert_eq!(
            r.fake.0.lock().unwrap().active_answers,
            1,
            "second answer is held by the actual adapter"
        );
        assert_eq!(
            r.fake.0.lock().unwrap().sends.len(),
            1,
            "a body receipt does not release that answer fence"
        );
        assert!(timeout(Duration::from_millis(30), &mut body).await.is_err());
        if let Some(draft) = &collision {
            r.fake
                .emit(&id, SessionEvent::InputDraft(Some(draft.clone())));
            r.settled().await;
        }
        release_second.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), &mut answer2)
                .await
                .unwrap()
                .unwrap(),
            json!({"cleared":true})
        );
        if collision.is_some() {
            assert!(
                timeout(Duration::from_secs(2), &mut body)
                    .await
                    .unwrap()
                    .is_err(),
                "late human input still blocks the queued body"
            );
            assert_eq!(r.fake.0.lock().unwrap().sends.len(), 1);
        } else {
            timeout(Duration::from_secs(2), async {
                while r.fake.0.lock().unwrap().sends.len() != 2 {
                    sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            let second_delivery = r.fake.last_id();
            r.fake
                .delivery(&id, second_delivery + 1, DeliveryOutcome::Delivered);
            assert!(timeout(Duration::from_millis(30), &mut body).await.is_err());
            r.fake
                .delivery(&id, second_delivery, DeliveryOutcome::Delivered);
            assert!(
                timeout(Duration::from_secs(2), &mut body)
                    .await
                    .unwrap()
                    .is_ok(),
                "positive empty-input control delivers the body after both answers"
            );
        }
        assert_eq!(r.fake.0.lock().unwrap().raws, 0);
    }
}

#[tokio::test]
async fn r1_late_box_executor_conflict_carries_current_decision_and_does_not_complete_nudge() {
    for structured in [false, true] {
        let r = Rig::new(structured).await;
        let id = r.start("cas-worker", false).await;
        let mut nudge = Box::pin(r.state.sessions.nudge(id.clone(), "first".into()));
        assert!(
            timeout(Duration::from_millis(30), &mut nudge)
                .await
                .is_err()
        );
        let delivery = r.fake.last_id();
        let d = if structured {
            runner("original")
        } else {
            terminal()
        };
        r.show(&id, d.clone()).await;
        let changed = if structured {
            runner("changed-request")
        } else {
            PendingDecision {
                prompt: "Changed question?".into(),
                ..terminal()
            }
        };
        // Actor still knows the old box; independently changed executor state is its CAS authority.
        r.fake
            .0
            .lock()
            .unwrap()
            .boxes
            .insert(id.clone(), changed.clone());
        let error = timeout(
            Duration::from_secs(2),
            r.rpc(
                "session.answer",
                json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":0}),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        let data = error.data.unwrap();
        assert_eq!(data.refusal, "decision.changed");
        assert_eq!(data.decision.unwrap().fingerprint, changed.fingerprint());
        assert_eq!(r.fake.0.lock().unwrap().answer_attempts, 1);
        {
            let state = r.fake.0.lock().unwrap();
            assert!(state.answers.is_empty() && state.choices.is_empty());
        }
        assert!(
            timeout(Duration::from_millis(30), &mut nudge)
                .await
                .is_err()
        );
        r.show(&id, changed.clone()).await;
        let stale = r
            .rpc(
                "session.answer",
                json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":0}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            stale.data.unwrap().decision.unwrap().fingerprint,
            changed.fingerprint()
        );
        assert_eq!(
            r.fake.0.lock().unwrap().answer_attempts,
            1,
            "actor stale check does not reach executor"
        );
        assert_eq!(
            r.rpc(
                "session.answer",
                json!({"sessionId":id,"fingerprint":changed.fingerprint(),"choice":0})
            )
            .await
            .unwrap(),
            json!({"cleared":true})
        );
        r.fake.delivery(
            &id,
            delivery,
            DeliveryOutcome::Unsubmitted("controlled final failure after resolving the box".into()),
        );
        let failure = timeout(Duration::from_secs(2), &mut nudge)
            .await
            .unwrap()
            .unwrap_err();
        assert!(failure.to_string().contains("Unsubmitted"));
    }
}

#[tokio::test]
async fn r1_stop_and_exit_cleanup_pending_and_queued_nudge_replies_without_body_writes() {
    use crewd_exec::session::{ExitInfo, ExitStatusKind};
    for stopped in [false, true] {
        let r = Rig::new(false).await;
        let id = r.start("cleanup-worker", false).await;
        let mut first = Box::pin(r.state.sessions.nudge(id.clone(), "first".into()));
        assert!(
            timeout(Duration::from_millis(30), &mut first)
                .await
                .is_err()
        );
        let mut queued = Box::pin(r.state.sessions.nudge(id.clone(), "queued".into()));
        assert!(
            timeout(Duration::from_millis(30), &mut queued)
                .await
                .is_err()
        );
        assert_eq!(r.fake.0.lock().unwrap().sends.len(), 1);
        if stopped {
            r.rpc(
                "session.stop",
                json!({"sessionId":id,"reason":"synthetic cleanup"}),
            )
            .await
            .unwrap();
            assert!(
                timeout(Duration::from_secs(2), &mut first)
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(
                timeout(Duration::from_secs(2), &mut queued)
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        r.fake.emit(
            &id,
            SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: stopped,
                hit_limit: false,
            }),
        );
        timeout(Duration::from_secs(2), async {
            while r.h.live(&id).await.unwrap().running {
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        if !stopped {
            assert!(
                timeout(Duration::from_secs(2), &mut first)
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(
                timeout(Duration::from_secs(2), &mut queued)
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        assert_eq!(r.fake.0.lock().unwrap().sends.len(), 1);
    }
}

#[tokio::test]
async fn r1_quota_historical_members_partial_recovery_and_executor_independence_are_real_posts() {
    let r = Rig::new(false).await;
    let other = r
        .state
        .collab(|tx| {
            crewd_collab::crews::create(
                tx,
                crewd_collab::crews::NewCrew {
                    title: "Second synthetic crew".into(),
                    working_directory: env!("CARGO_MANIFEST_DIR").into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .id;
    let distinct = r
        .state
        .collab(|tx| {
            crewd_collab::crews::create(
                tx,
                crewd_collab::crews::NewCrew {
                    title: "Distinct synthetic executor".into(),
                    working_directory: env!("CARGO_MANIFEST_DIR").into(),
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap()
        .id;
    let cap1 = r.start("cap-one", true).await;
    let successor = r.start("successor", false).await;
    let worker = r.start("unrelated-worker", false).await;
    let cap2 = r.start_in(&other, "cap-two", true, "claude").await;
    // This is a fully controlled adapter under a supported executor key, not real Codex.
    let cap3 = r.start_in(&distinct, "cap-distinct", true, "codex").await;
    for id in [&cap1, &cap2] {
        r.fake.emit(
            id,
            SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "same executor"))),
        );
    }
    r.settled().await;
    assert_eq!(
        r.posts().await.len(),
        1,
        "one machine episode for the shared executor"
    );
    r.fake.emit(
        &cap3,
        SessionEvent::Health(Some(Health::new(
            HealthKind::UsageLimit,
            "distinct executor",
        ))),
    );
    r.settled().await;
    assert_eq!(r.posts().await.len(), 2);
    r.rpc(
        "crew.setCaptain",
        json!({"crewId":r.crew,"sessionId":successor}),
    )
    .await
    .unwrap();
    assert_eq!(
        r.rpc("session.get", json!({"sessionId":cap1}))
            .await
            .unwrap()["role"],
        "worker"
    );
    assert_eq!(
        r.rpc("session.get", json!({"sessionId":successor}))
            .await
            .unwrap()["role"],
        "captain"
    );
    assert_eq!(r.posts().await.len(), 2, "handoff alone is not recovery");
    r.fake.emit(&worker, SessionEvent::Working(true));
    r.settled().await;
    assert_eq!(
        r.posts().await.len(),
        2,
        "unrelated worker cannot clear another session's episode"
    );
    r.fake.emit(&cap1, SessionEvent::Working(true));
    r.settled().await;
    assert!(r.h.live(&cap1).await.unwrap().working);
    assert_eq!(
        r.posts().await.len(),
        2,
        "historical member recovers, but the second remains blocked"
    );
    r.fake.emit(&cap3, SessionEvent::Working(true));
    r.settled().await;
    let posts = r.posts().await;
    assert_eq!(posts.len(), 3);
    assert_eq!(posts[2].0, format!("{distinct}/human"));
    assert_eq!(posts[2].2, "[]");
    r.fake.emit(&cap2, SessionEvent::Working(true));
    r.settled().await;
    let posts = r.posts().await;
    assert_eq!(posts.len(), 4);
    assert_eq!(posts[3].0, format!("{}/human", r.crew));
    assert_eq!(posts[3].2, "[]");
    r.fake.emit(&successor, SessionEvent::Working(true));
    r.settled().await;
    assert_eq!(
        r.posts().await.len(),
        4,
        "unregistered successor cannot add a recovery"
    );
    r.fake.emit(
        &successor,
        SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "new episode"))),
    );
    r.settled().await;
    assert_eq!(
        r.posts().await.len(),
        5,
        "a fresh episode is no longer suppressed"
    );
}

#[tokio::test]
async fn r1_public_message_wake_body_does_not_overtake_pending_nudge_or_its_causal_answer() {
    let r = Rig::new(false).await;
    // This case uses real collab message timestamps; don't age them by the rig's fixed future clock.
    r.clock.set(Ms(crewd_store::now_ms()));
    let forward =
        crewd_server::orchestra::db::forward_collab(r.state.hub.subscribe_collab(), r.h.clone());
    let id = r.start("wake-body-worker", false).await;
    let mut nudge = Box::pin(r.state.sessions.nudge(id.clone(), "first body".into()));
    assert!(
        timeout(Duration::from_millis(30), &mut nudge)
            .await
            .is_err()
    );
    let first_delivery = r.fake.last_id();
    let message = r
        .rpc(
            "message.post",
            json!({
                "channelId":format!("{}/lobby",r.crew), "text":"synthetic queued wake body",
                "mentions":[{"kind":"session","targetId":id}]
            }),
        )
        .await
        .unwrap();
    let message_id = message["id"].as_str().unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            let listed = r.rpc("wake.list", json!({"sessionId":id})).await.unwrap();
            if listed["targets"][0]["writesQueued"]
                .as_u64()
                .is_some_and(|n| n > 0)
            {
                break;
            }
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        r.fake.0.lock().unwrap().sends.len(),
        1,
        "real message bridge queued the body, not sent it"
    );
    let d = terminal();
    r.show(&id, d.clone()).await;
    r.fake.delivery(
        &id,
        first_delivery,
        DeliveryOutcome::Blocked("decision".into()),
    );
    assert_eq!(
        timeout(
            Duration::from_secs(2),
            r.rpc(
                "session.answer",
                json!({"sessionId":id,"fingerprint":d.fingerprint(),"choice":0})
            )
        )
        .await
        .unwrap()
        .unwrap(),
        json!({"cleared":true})
    );
    assert_eq!(r.fake.0.lock().unwrap().answer_attempts, 1);
    assert_eq!(
        r.fake.0.lock().unwrap().sends.len(),
        1,
        "human answer bypassed the held wake body, not vice versa"
    );
    assert!(
        timeout(Duration::from_millis(30), &mut nudge)
            .await
            .is_err()
    );
    r.fake
        .delivery(&id, first_delivery, DeliveryOutcome::Delivered);
    assert!(
        timeout(Duration::from_secs(2), &mut nudge)
            .await
            .unwrap()
            .is_ok()
    );
    timeout(Duration::from_secs(2), async {
        while r.fake.0.lock().unwrap().sends.len() != 2 {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let wake_delivery = r.fake.last_id();
    assert_ne!(wake_delivery, first_delivery);
    let listed = r.rpc("wake.list", json!({"sessionId":id})).await.unwrap();
    assert_eq!(listed["wakes"][0]["messageId"], message_id);
    assert_eq!(listed["wakes"][0]["state"], "inflight");
    r.fake
        .delivery(&id, wake_delivery, DeliveryOutcome::Delivered);
    timeout(Duration::from_secs(2), async {
        loop {
            let listed = r.rpc("wake.list", json!({"sessionId":id})).await.unwrap();
            if listed["wakes"][0]["state"] == "delivered" {
                break;
            }
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    forward.abort();
}
