//! 测试夹具：临时数据目录里的真库（M3a 的 Db + 当前 schema）+ FakeSessionControl。
//! `crewd` 的集成测试经 feature `testkit` 用它；仓库公开，只造合成数据。
//! 不进产品代码（字面量扫描器不扫它）。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_collab::Change;
use crewd_proto::{Mention, Message};
use serde_json::json;

use crate::AppState;
use crate::agent::port::{self, PostReq, PostSender};
use crate::agent::runtime::{AgentConfig, AgentRuntime};
use crate::agent::{Identity, Role};
use crate::control::FakeSessionControl;

/// Test-owned completed hook calls. Not a production log, not a Claude transcript reader.
#[derive(Clone, Debug)]
pub struct HookObservation {
    pub session: String,
    pub event: String,
    pub tool_name: Option<String>,
    pub context_chars: usize,
}

pub(crate) fn record_hook(
    st: &AppState,
    session: &str,
    kind: crewd_proto::agent::HookKind,
    payload: &serde_json::Value,
    result: &crewd_proto::agent::HookResult,
) {
    let parsed = result
        .stdout
        .as_ref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let context_chars = parsed
        .as_ref()
        .and_then(|v| v["hookSpecificOutput"]["additionalContext"].as_str())
        .map_or(0, |s| s.chars().count());
    let row = HookObservation {
        session: session.into(),
        event: payload["hook_event_name"]
            .as_str()
            .unwrap_or(kind.as_str())
            .into(),
        tool_name: payload["tool_name"].as_str().map(str::to_owned),
        context_chars,
    };
    st.agent.hook_log.lock().unwrap().push(row);
}

/// Actual injection handler termination, independently of helper exit0/empty stdout.
/// Stale/Busy are emitted only from actual typed CAS decisions, never from storage errors.
#[cfg(feature = "testkit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionOutcome {
    Committed,
    Empty,
    Stale,
    Busy,
    InternalError,
    Deadline,
}
#[cfg(feature = "testkit")]
#[derive(Clone, Debug)]
pub struct InjectionCompletion {
    pub session: String,
    pub event: String,
    pub outcome: InjectionOutcome,
}
#[cfg(feature = "testkit")]
pub(crate) fn record_injection_completion(
    st: &AppState,
    session: &str,
    payload: &serde_json::Value,
    outcome: InjectionOutcome,
) {
    let event = match payload["hook_event_name"].as_str() {
        Some("PostToolUseFailure") => "PostToolUseFailure",
        _ => "PostToolUse",
    };
    st.agent
        .injection_completions
        .lock()
        .unwrap()
        .push(InjectionCompletion {
            session: session.into(),
            event: event.into(),
            outcome,
        });
}

/// A REAL commit return, before the unchanged retry/termination decision.
/// Empty preparation and deadline-before-commit produce no attempt row.
#[cfg(feature = "testkit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionCommitOutcome {
    Applied,
    Stale,
    Busy,
    InternalError,
}
#[cfg(feature = "testkit")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectionAttempt {
    pub call: u64,
    pub session: String,
    pub event: String,
    /// Zero-based actual bounded handler attempt (0 or 1).
    pub attempt: usize,
    /// Actual prepared cursor targets, without bodies, tokens or user transcripts.
    pub targets: Vec<(String, i64)>,
    pub outcome: InjectionCommitOutcome,
}
#[cfg(feature = "testkit")]
#[derive(Debug, Default)]
pub(crate) struct InjectionAttemptJournal {
    next_call: u64,
    rows: Vec<InjectionAttempt>,
}
#[cfg(feature = "testkit")]
pub(crate) fn begin_injection_observation(st: &AppState) -> Option<u64> {
    let mut slot = st.agent.injection_attempts.lock().unwrap();
    let journal = slot.as_mut()?;
    journal.next_call = journal
        .next_call
        .checked_add(1)
        .expect("fixture call ID exhausted");
    Some(journal.next_call)
}
#[cfg(feature = "testkit")]
pub(crate) fn record_injection_attempt(st: &AppState, call: Option<u64>, row: InjectionAttempt) {
    if let Some(call) = call {
        let mut slot = st.agent.injection_attempts.lock().unwrap();
        let journal = slot
            .as_mut()
            .expect("armed fixture journal cannot disappear");
        assert_eq!(row.call, call);
        journal.rows.push(row);
    }
}

/// Single-use pauses at actual caller stages. Absent outside feature=testkit and idle by default.
#[cfg(feature = "testkit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallerStage {
    HookPrepared,
    WakeRendered,
}
#[cfg(feature = "testkit")]
#[derive(Clone, Debug)]
pub struct CallerEvidence {
    pub targets: Vec<(String, i64)>,
    pub text: String,
}
#[cfg(feature = "testkit")]
#[derive(Debug)]
pub(crate) struct CallerPause {
    id: u64,
    entered: tokio::sync::oneshot::Sender<CallerEvidence>,
    release: tokio::sync::oneshot::Receiver<()>,
}
#[cfg(feature = "testkit")]
pub struct CallerGate {
    state: Arc<AppState>,
    stage: CallerStage,
    session: String,
    id: u64,
    entered: tokio::sync::oneshot::Receiver<CallerEvidence>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
}
#[cfg(feature = "testkit")]
impl CallerGate {
    pub async fn entered(&mut self) -> CallerEvidence {
        (&mut self.entered)
            .await
            .expect("actual caller must reach installed stage")
    }
    pub fn release(mut self) {
        let _ = self
            .release
            .take()
            .expect("single-use gate release")
            .send(());
    }
}
#[cfg(feature = "testkit")]
impl Drop for CallerGate {
    fn drop(&mut self) {
        // Cancellation before entry removes only this registration. Cancellation after entry
        // drops its release sender, so the caller proceeds; no mutex/DB thread stays held.
        let key = (self.stage, self.session.clone());
        let mut slots = self.state.agent.caller_gates.lock().unwrap();
        if slots.get(&key).is_some_and(|p| p.id == self.id) {
            slots.remove(&key);
        }
    }
}
#[cfg(feature = "testkit")]
pub(crate) async fn pause_caller(
    state: &AppState,
    stage: CallerStage,
    session: &str,
    targets: Vec<(String, i64)>,
    text: String,
) {
    let pause = {
        let mut slots = state.agent.caller_gates.lock().unwrap();
        slots.remove(&(stage, session.to_string()))
    }; // the ordinary mutex is released before either send or await
    if let Some(p) = pause
        && p.entered.send(CallerEvidence { targets, text }).is_ok()
    {
        let _ = p.release.await;
    }
}

pub struct Fixture {
    pub state: Arc<AppState>,
    pub crew: String,
    pub captain: String,
    pub worker: String,
    dir: tempfile::TempDir,
    control: Arc<FakeSessionControl>,
}

impl Fixture {
    /// Enable the existing production SQLite knowledge runtime in this owned fixture.
    pub fn enable_knowledge(&self) {
        let runtime = std::sync::Arc::new(
            crate::team_data::runtime::Runtime::new(self.data_dir().join("knowledge")).unwrap(),
        );
        assert!(self.state.data_runtime.set(runtime).is_ok());
    }

    /// Arm once before the observed caller. No global storage or public wire selector.
    /// Rows are owned by this AgentRuntime and destroyed with its final Arc.
    #[cfg(feature = "testkit")]
    pub fn observe_injection_attempts(&self) {
        let mut slot = self.state.agent.injection_attempts.lock().unwrap();
        assert!(slot.is_none(), "fixture journal already armed");
        *slot = Some(InjectionAttemptJournal::default());
    }

    #[cfg(feature = "testkit")]
    pub fn injection_attempts(&self, session: &str) -> Vec<InjectionAttempt> {
        self.state
            .agent
            .injection_attempts
            .lock()
            .unwrap()
            .as_ref()
            .map(|journal| {
                journal
                    .rows
                    .iter()
                    .filter(|r| r.session == session)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(feature = "testkit")]
    pub fn caller_gate(&self, session: &str, stage: CallerStage) -> CallerGate {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (entered, receipt) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let key = (stage, session.to_string());
        let mut slots = self.state.agent.caller_gates.lock().unwrap();
        assert!(
            !slots.contains_key(&key),
            "one active gate per session/stage"
        );
        slots.insert(
            key,
            CallerPause {
                id,
                entered,
                release: wait,
            },
        );
        CallerGate {
            state: self.state.clone(),
            stage,
            session: session.into(),
            id,
            entered: receipt,
            release: Some(release),
        }
    }

    #[cfg(feature = "testkit")]
    pub fn injection_completions(&self, session: &str) -> Vec<InjectionCompletion> {
        self.state
            .agent
            .injection_completions
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.session == session)
            .cloned()
            .collect()
    }

    pub fn hook_observations(&self, session: &str) -> Vec<HookObservation> {
        self.state
            .agent
            .hook_log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.session == session)
            .cloned()
            .collect()
    }

    pub async fn new() -> Self {
        Self::with_config(AgentConfig::default()).await
    }

    pub async fn with_config(config: AgentConfig) -> Self {
        Self::with_runtime(AgentRuntime::new(config)).await
    }

    /// A fixture with a prepared runtime (e.g. a post gate on a clock the test moves).
    pub async fn with_runtime(runtime: AgentRuntime) -> Self {
        Self::build(runtime, None).await
    }

    /// Real orchestra adapter; default fixtures retain FakeSessionControl.
    pub async fn with_orchestra(cell: Arc<std::sync::OnceLock<crate::orchestra::Handle>>) -> Self {
        Self::build(AgentRuntime::new(AgentConfig::default()), Some(cell)).await
    }

    /// Real orchestra adapter with an existing explicit post-clock runtime.
    pub async fn with_orchestra_runtime(
        cell: Arc<std::sync::OnceLock<crate::orchestra::Handle>>,
        runtime: AgentRuntime,
    ) -> Self {
        Self::build(runtime, Some(cell)).await
    }

    async fn build(
        runtime: AgentRuntime,
        cell: Option<Arc<std::sync::OnceLock<crate::orchestra::Handle>>>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let control = Arc::new(FakeSessionControl::default());
        let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
        let db = crewd_store::Db::spawn(store).unwrap();
        let sessions: Arc<dyn crate::control::SessionControl> = match cell {
            Some(cell) => Arc::new(crate::orchestra::control::OrchestraControl::new(
                cell,
                db.clone(),
            )),
            None => control.clone(),
        };
        let mut state = AppState::new(db).with_sessions(sessions);
        state.agent = runtime;
        let mut f = Self {
            state: Arc::new(state),
            crew: String::new(),
            captain: "s-cap".into(),
            worker: "s-w1".into(),
            dir,
            control,
        };
        f.crew = f.add_crew("示例机组", None).await;
        let crew = f.crew.clone();
        f.add_session(&crew, "s-cap", Role::Captain, "示例机长")
            .await;
        f.add_session(&crew, "s-w1", Role::Worker, "示例 worker")
            .await;
        f
    }

    /// 经客户端 RPC 建机组（与客户端同一条路，顺带建大厅）。
    pub async fn add_crew(&self, title: &str, parent: Option<&str>) -> String {
        let v = self
            .rpc(
                "crew.create",
                json!({"title": title, "workingDirectory": self.dir.path(), "parentId": parent}),
            )
            .await;
        v["id"].as_str().unwrap().to_string()
    }

    /// 固定 id 的 session。**这样加的 session 没有分机号**（`ext` 空，通讯录里没有它的号，机长除外）；
    /// 测试要用号时改用 `Fixture::register`。与 `sessions::register` 同样的效果（行、机长指针、大厅游标钉在末尾），
    /// 只是 id 由调用方定、没有分机号：测试在登记**之后**发的消息才是它的未读，与生产一致。
    pub async fn add_session(&self, crew: &str, id: &str, role: Role, label: &str) {
        let row = crewd_proto::Session {
            id: id.into(),
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            display_name: Some(label.into()),
            updated_at: "2026-10-06T00:00:00.000Z".into(),
            ..Default::default()
        };
        let (c, i) = (crew.to_string(), id.to_string());
        self.state
            .collab(move |tx| {
                crewd_store::sessions::upsert(tx.conn(), &row, None)?;
                if role == Role::Captain {
                    crewd_collab::crews::set_captain(tx, &c, &i)?;
                }
                let lobby = crewd_store::channels::lobby_id(&c);
                crewd_collab::cursors::pin_to_end(tx, &i, &lobby)?;
                // 大厅的成员是隐式的（= 本机组全体 session）：多了一个人，大厅就变了。
                tx.note(Change::ChannelChanged { channel_id: lobby });
                tx.note(Change::SessionChanged { session_id: i });
                Ok(())
            })
            .await
            .unwrap();
    }

    /// 走 `sessions::register`（分配分机号）；返回生成的 id。`label` 同时当标题（N1：`register` 要标题或 brief，
    /// 标题 ≤ 18 个码点）。
    pub async fn register(&self, crew: &str, role: Role, label: &str) -> String {
        let n = crewd_collab::sessions::NewSession {
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            title: Some(label.into()),
            display_name: Some(label.into()),
            ..Default::default()
        };
        self.state
            .collab(move |tx| crewd_collab::sessions::register(tx, n))
            .await
            .unwrap()
            .id
    }

    /// 换机长（M3a `crews::set_captain`，经 `AppState::collab` 发 CrewChanged / SessionChanged）。
    /// `Role::Worker` = 把机长还给夹具原来的机长。
    pub async fn set_role(&self, session: &str, role: Role) {
        let crew = self.me(session).await.crew_id;
        let to = if role == Role::Captain {
            session.to_string()
        } else {
            self.captain.clone()
        };
        self.state
            .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &to))
            .await
            .unwrap();
    }

    /// The management crew's id (`crew.create` creates it with the first crew, so it exists in
    /// every fixture).
    pub async fn management_crew(&self) -> String {
        self.state
            .db
            .call(|s| crewd_collab::crews::management(s.conn()))
            .await
            .unwrap()
            .unwrap()
            .expect("crew.create creates the management crew")
    }

    pub fn token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    pub async fn me(&self, session: &str) -> Identity {
        port::identity(&self.state, session).await.unwrap().unwrap()
    }

    /// 进程内调客户端 RPC，出错 panic。
    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let resp = crate::dispatch::handle(
            &self.state,
            crewd_proto::Request {
                id: 1,
                method: method.into(),
                params,
            },
        )
        .await;
        match resp.into_result() {
            Ok(v) => v,
            Err(e) => panic!("{method}: {}", e.message),
        }
    }

    /// 人在某个群里发言（客户端 `message.post`）。
    pub async fn human_post(&self, channel: &str, text: &str, mentions: Vec<Mention>) -> Message {
        let v = self
            .rpc(
                "message.post",
                json!({"channelId": channel, "text": text, "mentions": mentions}),
            )
            .await;
        serde_json::from_value(v).unwrap()
    }

    /// session 以自己身份发言（绕过工具，造场景用）。
    pub async fn session_post(
        &self,
        session: &str,
        channel: &str,
        text: &str,
        mentions: Vec<Mention>,
    ) -> Message {
        port::post(
            &self.state,
            PostReq {
                channel_id: channel.into(),
                sender: PostSender::Session(session.into()),
                text: text.into(),
                mentions,
                in_reply_to: None,
                category: None,
                attachments: vec![],
            },
        )
        .await
        .unwrap()
        .message
    }

    /// 在 `<dir>/crewd.sock` 上起 accept 循环（与 serve 同一个 `conn::run`），返回 socket 路径。
    /// 一个夹具一个 socket：重复调用返回同一个。
    pub async fn spawn_socket(&self) -> PathBuf {
        let sock = self.dir.path().join("crewd.sock");
        if sock.exists() {
            return sock;
        }
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let state = self.state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = state.clone();
                tokio::spawn(async move {
                    let _ = crate::conn::run(st, stream).await;
                });
            }
        });
        sock
    }

    /// Owned socket/connection tasks for integration cases requiring bounded cleanup.
    pub async fn spawn_owned_socket(
        &self,
        mut stop: tokio::sync::oneshot::Receiver<()>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::UnixListener::bind(self.dir.path().join("crewd.sock")).unwrap();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let st = state.clone();
                        connections.spawn(async move { let _ = crate::conn::run(st, stream).await; });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        })
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    /// 本机组 Agent 那本 Todo 加一条（M1 的 store 函数；M3 没有客户端建 Todo 的入口）。
    pub async fn add_agent_todo(&self, number: i64, text: &str) {
        let todo = crewd_proto::Todo {
            requirement_id: None,
            id: format!("todo-{number}"),
            crew_id: self.crew.clone(),
            ledger: crewd_proto::Ledger::Agent,
            number,
            text: text.into(),
            status: "pending".into(),
            created_at: "2026-10-06T00:00:00.000Z".into(),
            updated_at: None,
            deleted_at: None,
            attachments: vec![],
            responses: vec![],
        };
        self.state
            .db
            .call(move |s| crewd_store::todos::insert(s.conn(), &todo, None))
            .await
            .unwrap()
            .unwrap();
    }

    /// 这个 session 的唤醒（kind, not_before 毫秒, note），新的在前（M3a `wakes::list`）。
    pub async fn wakes_of(&self, session: &str) -> Vec<(String, i64, String)> {
        let session = session.to_string();
        let rows = self
            .state
            .db
            .call(move |s| crewd_collab::wakes::list(s.conn(), Some(&session), 100))
            .await
            .unwrap()
            .unwrap();
        rows.into_iter()
            .map(|w| {
                (
                    w.kind.as_str().to_string(),
                    w.not_before_ms,
                    w.note.unwrap_or_default(),
                )
            })
            .collect()
    }

    pub fn set_quota(&self, report: crate::control::QuotaReport) {
        self.control.set_quota(report);
    }

    /// The fake's next call (any method) fails with `e`.
    pub fn fail_next(&self, e: crate::control::ControlError) {
        self.control.fail_next(e);
    }

    /// The fake's next call of `method` fails with `e` (other methods go through).
    pub fn fail_method(&self, method: crate::control::FakeMethod, e: crate::control::ControlError) {
        self.control.fail_method(method, e);
    }

    /// What the fake's `live` / `inspect` report for `l.session_id` from now on.
    pub fn set_live(&self, l: crewd_proto::SessionLive) {
        self.control.set_live(l);
    }

    /// Apply the last `handoff_captain` the fake recorded the way M3c does (collab
    /// `crews::set_captain` through `AppState::collab`), so a test sees the data after a handoff.
    pub async fn apply_last_handoff(&self) {
        let (crew, to) = self
            .control
            .calls()
            .into_iter()
            .rev()
            .find_map(|c| match c {
                crate::control::FakeCall::Handoff(crew, to) => Some((crew, to)),
                _ => None,
            })
            .expect("a recorded handoff");
        self.state
            .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &to))
            .await
            .unwrap();
    }

    /// FakeSessionControl 记下的调用，逐条 `{:?}`（如 `Start(StartRequest { crew_id: "…", role: Worker, … })`）。
    pub fn control_calls(&self) -> Vec<String> {
        self.control
            .calls()
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
    }
}

/// 测试用的 NDJSON 客户端：一条连接，回应按 id 取，事件先攒着。
pub struct RpcClient {
    rd: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    wr: tokio::net::unix::OwnedWriteHalf,
    next: u64,
    events: std::collections::VecDeque<crewd_proto::Event>,
}

impl RpcClient {
    pub async fn connect(sock: &Path) -> Self {
        let (rd, wr) = tokio::net::UnixStream::connect(sock)
            .await
            .unwrap()
            .into_split();
        Self {
            rd: tokio::io::BufReader::new(rd),
            wr,
            next: 1,
            events: Default::default(),
        }
    }

    async fn frame(&mut self) -> crewd_proto::ServerFrame {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.rd.read_line(&mut line).await.unwrap();
        assert!(n > 0, "crewd closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, (i32, String)> {
        use tokio::io::AsyncWriteExt;
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&crewd_proto::Request {
            id,
            method: method.into(),
            params,
        })
        .unwrap();
        line.push(b'\n');
        self.wr.write_all(&line).await.unwrap();
        loop {
            match self.frame().await {
                crewd_proto::ServerFrame::Event(e) => self.events.push_back(e),
                crewd_proto::ServerFrame::Response(r) if r.id == id => {
                    // `"result": null` 是成功、值为 null（M3a 的 `Outcome::Result(Value::Null)`）。
                    return r.into_result().map_err(|e| (e.code, e.message));
                }
                crewd_proto::ServerFrame::Response(_) => {}
            }
        }
    }

    pub async fn next_event(&mut self, within: std::time::Duration) -> Option<crewd_proto::Event> {
        if let Some(e) = self.events.pop_front() {
            return Some(e);
        }
        match tokio::time::timeout(within, self.frame()).await {
            Ok(crewd_proto::ServerFrame::Event(e)) => Some(e),
            _ => None,
        }
    }
}

/// Small feature-testkit executor for real helper/DbPort integration. No PTY or provider.
#[derive(Clone, Default)]
pub struct RelayStandIn(Arc<std::sync::Mutex<RelayExecState>>);
#[derive(Clone)]
pub struct RelayLaunch {
    pub initial: Option<String>,
}
#[derive(Default)]
struct RelayExecState {
    sessions: std::collections::BTreeMap<String, RelayExecSession>,
    next_id: u64,
}
struct RelayExecSession {
    tx: tokio::sync::mpsc::UnboundedSender<crewd_exec::session::SessionEvent>,
    launches: Vec<RelayLaunch>,
    sends: Vec<(u64, String)>,
    outstanding: std::collections::BTreeSet<u64>,
    running: bool,
    working: bool,
    hold_stop: bool,
    stop_requests: usize,
    delivery_observer: Option<Arc<dyn crewd_exec::driver::DeliverySuccessObserver>>,
}
impl RelayExecSession {
    fn emit(&mut self, event: crewd_exec::session::SessionEvent) {
        use crewd_exec::session::SessionEvent;
        match &event {
            SessionEvent::Working(on) => self.working = *on,
            SessionEvent::Delivery { id, outcome, .. } if outcome.is_final() => {
                self.outstanding.remove(id);
            }
            SessionEvent::Exited(_) => self.running = false,
            _ => {}
        }
        if let SessionEvent::Delivery {
            id,
            outcome: crewd_exec::session::DeliveryOutcome::Delivered,
            ..
        } = &event
            && let Some(observer) = &self.delivery_observer
        {
            observer.emitted(*id);
        }
        self.tx.send(event).expect("relay actor event stream");
    }
}
impl RelayStandIn {
    /// Opt-in only: a successful Stop request is not a terminal executor receipt.
    pub fn hold_stop(&self, id: &str, hold: bool) {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get_mut(id)
            .expect("launch before stop control")
            .hold_stop = hold;
    }
    pub fn stop_requests(&self, id: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .expect("launch before stop control")
            .stop_requests
    }
    pub fn emit(&self, id: &str, event: crewd_exec::session::SessionEvent) {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get_mut(id)
            .expect("launch before event")
            .emit(event);
    }
    pub fn launches(&self, id: &str) -> Vec<RelayLaunch> {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .map(|s| s.launches.clone())
            .unwrap_or_default()
    }
    pub fn sent(&self, id: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .map(|s| s.sends.iter().map(|(_, t)| t.clone()).collect())
            .unwrap_or_default()
    }
    pub fn last_send_id(&self, id: &str) -> u64 {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .and_then(|s| s.sends.last())
            .expect("send before receipt")
            .0
    }
}
impl crate::orchestra::exec::Executor for RelayStandIn {
    fn launch(
        &self,
        req: crate::orchestra::exec::LaunchRequest,
    ) -> crate::orchestra::exec::BoxFut<
        '_,
        Result<crate::orchestra::exec::Launched, crate::orchestra::exec::LaunchError>,
    > {
        let state = self.0.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let mut locked = state.lock().unwrap();
            let mut launches = locked
                .sessions
                .get(&req.session_id)
                .map(|s| s.launches.clone())
                .unwrap_or_default();
            launches.push(RelayLaunch {
                initial: req.initial_prompt.clone(),
            });
            let mut session = RelayExecSession {
                tx,
                launches,
                sends: vec![],
                outstanding: Default::default(),
                running: true,
                working: false,
                hold_stop: false,
                stop_requests: 0,
                delivery_observer: req.delivery_observer,
            };
            if req.initial_prompt.is_some() {
                session
                    .outstanding
                    .insert(crewd_exec::session::BRIEF_DELIVERY_ID);
            }
            session.emit(crewd_exec::session::SessionEvent::Spawned {
                pid: 4242,
                agent_session_id: format!("synthetic-{}", req.session_id),
                resumed: false,
            });
            locked.sessions.insert(req.session_id.clone(), session);
            drop(locked);
            let h: Arc<dyn crate::orchestra::exec::ExecHandle> = Arc::new(RelayExecHandle {
                id: req.session_id,
                state,
            });
            Ok((h, rx))
        })
    }
}
struct RelayExecHandle {
    id: String,
    state: Arc<std::sync::Mutex<RelayExecState>>,
}
impl RelayExecHandle {
    fn send_inner(
        &self,
        text: String,
        context: &crate::orchestra::exec::Context,
        binding: Option<&dyn crewd_exec::driver::DeliveryBinding>,
    ) -> Result<u64, crate::orchestra::exec::ExecSendError> {
        use crewd_exec::driver::SendError;
        crate::orchestra::exec::check_context(false, context)?;
        if text.trim().is_empty() {
            return Err(SendError::EmptyText.into());
        }
        let mut state = self.state.lock().unwrap();
        if !state.sessions.get(&self.id).is_some_and(|s| s.running) {
            return Err(SendError::Closed.into());
        }
        let id = state
            .next_id
            .checked_add(1)
            .ok_or(SendError::DeliveryIdExhausted)?;
        if let Some(binding) = binding {
            binding.register(id)?;
        }
        state.next_id = id;
        let session = state.sessions.get_mut(&self.id).unwrap();
        session.sends.push((id, text));
        session.outstanding.insert(id);
        Ok(id)
    }
}

impl crate::orchestra::exec::ExecHandle for RelayExecHandle {
    fn send(
        &self,
        text: String,
        context: &crate::orchestra::exec::Context,
    ) -> Result<u64, crate::orchestra::exec::ExecSendError> {
        self.send_inner(text, context, None)
    }
    fn send_bound(
        &self,
        text: String,
        context: &crate::orchestra::exec::Context,
        binding: &dyn crewd_exec::driver::DeliveryBinding,
    ) -> Result<u64, crate::orchestra::exec::ExecSendError> {
        self.send_inner(text, context, Some(binding))
    }
    fn raw(&self, _bytes: Vec<u8>) -> Result<(), crewd_exec::driver::Closed> {
        Err(crewd_exec::driver::Closed)
    }
    fn interrupt(&self) -> Result<(), crewd_exec::driver::Closed> {
        let mut state = self.state.lock().unwrap();
        let s = state
            .sessions
            .get_mut(&self.id)
            .ok_or(crewd_exec::driver::Closed)?;
        if !s.running {
            return Err(crewd_exec::driver::Closed);
        }
        if s.working {
            s.emit(crewd_exec::session::SessionEvent::Working(false));
        }
        Ok(())
    }
    fn answer(
        &self,
        _expect: String,
        _steps: crate::orchestra::exec::AnswerSteps,
    ) -> crate::orchestra::exec::BoxFut<
        '_,
        Result<crewd_exec::session::AnswerOutcome, crewd_exec::driver::Closed>,
    > {
        Box::pin(async { Ok(crewd_exec::session::AnswerOutcome::Unsupported) })
    }
    fn stop(&self) -> Result<(), crewd_exec::driver::Closed> {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, SessionEvent},
        };
        let mut state = self.state.lock().unwrap();
        let s = state
            .sessions
            .get_mut(&self.id)
            .ok_or(crewd_exec::driver::Closed)?;
        if !s.running {
            return Ok(());
        }
        s.stop_requests += 1;
        if s.hold_stop {
            return Ok(());
        }
        let pending = std::mem::take(&mut s.outstanding);
        for id in pending {
            s.emit(SessionEvent::Delivery {
                id,
                subject: if id == BRIEF_DELIVERY_ID {
                    Subject::Brief
                } else {
                    Subject::Steer
                },
                outcome: DeliveryOutcome::Abandoned("synthetic stop".into()),
            });
        }
        if s.working {
            s.emit(SessionEvent::Working(false));
        }
        s.emit(SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: true,
            hit_limit: false,
        }));
        Ok(())
    }
    fn clear_quota_health(&self) -> Result<(), crewd_exec::driver::Closed> {
        Ok(())
    }
    fn screen_text(
        &self,
        _max: usize,
    ) -> crate::orchestra::exec::BoxFut<'_, Result<String, crewd_exec::driver::Closed>> {
        Box::pin(async { Ok(String::new()) })
    }
    fn snapshot(
        &self,
    ) -> crate::orchestra::exec::BoxFut<
        '_,
        Result<Option<crewd_term::Snapshot>, crewd_exec::driver::Closed>,
    > {
        Box::pin(async { Ok(None) })
    }
    fn status(
        &self,
    ) -> crate::orchestra::exec::BoxFut<
        '_,
        Result<crewd_exec::driver::SessionStatus, crewd_exec::driver::Closed>,
    > {
        let state = self.state.clone();
        let id = self.id.clone();
        Box::pin(async move {
            let state = state.lock().unwrap();
            let s = state.sessions.get(&id).ok_or(crewd_exec::driver::Closed)?;
            if !s.running {
                return Err(crewd_exec::driver::Closed);
            }
            Ok(crewd_exec::driver::SessionStatus {
                state: if s.working {
                    crewd_exec::session::DerivedState::Working
                } else {
                    crewd_exec::session::DerivedState::Idle
                },
                working: s.working,
                health: None,
                pending_decision: None,
                pid: 4242,
                agent_session_id: format!("synthetic-{id}"),
            })
        })
    }
}
