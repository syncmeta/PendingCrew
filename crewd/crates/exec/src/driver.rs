//! 一个 claude 会话的 tokio 任务，和其余部分唯一的接口 [`SessionHandle`]（spec §13 Q12）。
//!
//! 任务拥有 PTY 主端、子进程、[`Core`]；命令从句柄进，事件从 `mpsc` 出。
//! 节拍：每块 PTY 输出一拍、0.6s 定时一拍、`Core::next_wakeup` 到点一拍。
//! 写 PTY 走单独的写任务：子进程不读输入时写会阻塞，不能卡住读（否则双方互等）。
//!
//! 以后执行器拆成独立进程，只把 `SessionHandle` 的命令与事件换成线协议。
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crewd_term::{PendingDecision, ScreenConfig, Snapshot};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::delivery::Subject;
use crate::env::build_env_from_process;
use crate::health::{
    AUTH_POLL_INTERVAL, AuthProbe, AuthState, Health, HealthKind, LaunchVerdict, launch_failure,
};
use crate::launch::{
    AgentSessionArg, ClaudeSpec, LaunchFiles, new_agent_session_id, prepare_claude,
};
use crate::pty::{PtyMaster, SpawnSpec, spawn_pty};
use crate::registry::{ProbeResult, ProcessRegistry, group_signal_refused, probe};
use crate::resume::{AgentIdLedger, decide_resume};
use crate::session::{
    Action, AnswerOutcome, AnswerStep, BRIEF_DELIVERY_ID, Core, CoreConfig, DerivedState,
    ExitStatusKind, PendingText, SessionEvent,
};

/// 驱动层参数。
#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub core: CoreConfig,
    pub screen: ScreenConfig,
    /// 定时节拍。
    pub tick: Duration,
    /// SIGTERM 之后多久升级成 `killpg(SIGKILL)`。
    pub stop_grace: Duration,
    /// 启动临时文件放哪（建议 `<数据目录>/run`，数据目录本身是 0700）。
    pub runtime_dir: PathBuf,
    /// claude 的 projects 目录（认证探测用）；`None` = 不探测。
    pub claude_projects_dir: Option<PathBuf>,
    pub auth_poll: Duration,
    /// 孤儿登记表；`None` = 不登记。
    pub registry: Option<Arc<ProcessRegistry>>,
}

impl DriverConfig {
    pub fn new(runtime_dir: PathBuf) -> Self {
        Self {
            core: CoreConfig::default(),
            screen: ScreenConfig::default(),
            tick: Duration::from_millis(600),
            stop_grace: Duration::from_secs(2),
            runtime_dir,
            claude_projects_dir: None,
            auth_poll: AUTH_POLL_INTERVAL,
            registry: None,
        }
    }
}

/// 起一个 claude 会话要的东西。
pub struct ClaudeStart {
    /// crewd 的 session id（登记表的键）。
    pub session_id: String,
    /// claude 可执行文件（`AgentPaths::resolve("claude")`）。
    pub program: PathBuf,
    /// 子进程 PATH（`AgentPaths::child_path`）。
    pub child_path: String,
    pub spec: ClaudeSpec,
    pub ledger: Arc<dyn AgentIdLedger>,
}

#[derive(Debug)]
pub enum StartError {
    /// 账本读写失败：不起进程（起了也接不回来）。
    Ledger(io::Error),
    /// 临时文件写不出来。
    Files(io::Error),
    /// 进程没起来。`health` 是给人看的拉起失败。
    Spawn { health: Health, source: io::Error },
    /// 登记表写不进去（孤儿兜底失效）：已杀掉刚起的进程。
    Registry(io::Error),
    /// 这个会话号（claude 会话号 / codex thread id）已经在本进程里跑着：再起一次会删掉它的
    /// 启动目录、两个进程写同一份对话。
    AlreadyRunning(String),
    /// 进程起来了但握手没完成（codex：`initialize` / `thread/start` 失败、超时或进程中途退出）。
    /// 已杀掉进程。`health` 是给人看的拉起失败。
    Handshake { health: Health },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ledger(e) => write!(f, "会话号账本读写失败：{e}"),
            Self::Files(e) => write!(f, "启动文件写不出来：{e}"),
            Self::Spawn { health, .. } => write!(f, "{}", health.detail),
            Self::Registry(e) => write!(f, "进程登记表写不进去：{e}"),
            Self::AlreadyRunning(id) => {
                write!(f, "会话 {id} 已经在跑，不能同时再起一个")
            }
            Self::Handshake { health } => write!(f, "{}", health.detail),
        }
    }
}

impl std::error::Error for StartError {}

/// 会话任务已经结束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

/// [`SessionHandle::send`] 没收下这段正文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// 会话任务已经结束。
    Closed,
    /// 正文是空的（或只有空白）：投进去什么也不会发生，不占投递编号。
    EmptyText,
}

impl From<Closed> for SendError {
    fn from(_: Closed) -> Self {
        Self::Closed
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "会话已经结束"),
            Self::EmptyText => write!(f, "正文是空的"),
        }
    }
}

impl std::error::Error for SendError {}

/// 某一时刻的会话状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatus {
    pub state: DerivedState,
    pub working: bool,
    pub health: Option<Health>,
    pub pending_decision: Option<PendingDecision>,
    pub pid: u32,
    pub agent_session_id: String,
}

/// 附在一条正文前面的「不可信上下文」：键 = 来源（如 `crew_whiteboard`），值 = 原文。
/// 是数据不是指令——codex 走 `turn/start.additionalContext`（`kind: "untrusted"`），
/// claude 由驱动层包成 `<external_键>…</external_键>` 放在正文前面（[`with_untrusted_context`]）。
pub type UntrustedContext = BTreeMap<String, String>;

/// 只有部分执行器支持的会话控制。不支持的一方回 [`ControlOutcome::Unsupported`]，不假装做了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// 之后的回合换模型。
    Model(String),
    /// 之后的回合换推理强度。
    Effort(String),
    /// fast 档开 / 关。
    Fast(bool),
    /// 压缩上下文（只在空闲时接受）。
    Compact,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlOutcome {
    /// 已生效（附说明）。
    Applied(String),
    /// 请求被接受了，但没等到权威回显——实际状态未知。
    NoConfirmation(String),
    /// 被拒（附原因）。
    Rejected(String),
    /// 这个执行器不支持这项控制。
    Unsupported(String),
}

/// claude 一侧的「不可信上下文」：按键序包成 `<external_键>` 块放在正文前面，与 codex
/// 服务端渲染 `additionalContext` 的形状一致。没有上下文就原样返回正文。
pub fn with_untrusted_context(context: &UntrustedContext, text: &str) -> String {
    let mut out = String::new();
    for (key, value) in context {
        if value.is_empty() {
            continue;
        }
        out.push_str(&format!("<external_{key}>\n{value}\n</external_{key}>\n\n"));
    }
    out.push_str(text);
    out
}

/// 句柄发给会话任务的命令。两个驱动（claude / codex）各自解释同一套命令。
pub(crate) enum Command {
    Send {
        id: u64,
        text: String,
        context: UntrustedContext,
    },
    Raw(Vec<u8>),
    Interrupt,
    Resize {
        cols: u16,
        lines: u16,
    },
    Stop,
    ClearQuotaHealth,
    ScreenText {
        max_lines: usize,
        reply: oneshot::Sender<String>,
    },
    Snapshot {
        reply: oneshot::Sender<Option<Snapshot>>,
    },
    Status {
        reply: oneshot::Sender<SessionStatus>,
    },
    Control {
        op: Control,
        reply: oneshot::Sender<ControlOutcome>,
    },
    /// 回答屏上的待决策框（T10d，[`SessionHandle::answer`]）。
    Answer {
        expect: String,
        steps: Vec<AnswerStep>,
        reply: oneshot::Sender<AnswerOutcome>,
    },
}

/// 窄接口：启动之外的一切（输入 / 打断 / 停止 / 状态 / 屏幕文本 / 快照 / 控制）。
/// claude 与 codex 用同一个句柄类型。可克隆；**所有句柄都 drop 了 = 停掉会话**。
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl SessionHandle {
    /// 驱动层建句柄（投递编号从 `BRIEF_DELIVERY_ID + 1` 起，0 留给开场任务）。
    pub(crate) fn new(tx: mpsc::UnboundedSender<Command>) -> Self {
        Self {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
        }
    }

    /// 投一段正文（排队）。返回投递编号，结果见 `SessionEvent::Delivery`。
    /// 空的 / 只有空白的正文当场拒掉（[`SendError::EmptyText`]，不占编号、不会有结果事件）。
    pub fn send(&self, text: impl Into<String>) -> Result<u64, SendError> {
        self.send_with_context(text, UntrustedContext::new())
    }

    /// 投一段正文，附带不可信上下文（白板未读等）。见 [`UntrustedContext`]。
    /// 上下文不算正文：正文空 / 只有空白照样 [`SendError::EmptyText`]。
    pub fn send_with_context(
        &self,
        text: impl Into<String>,
        context: UntrustedContext,
    ) -> Result<u64, SendError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(SendError::EmptyText);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.cmd(Command::Send { id, text, context })?;
        Ok(id)
    }

    /// 原样写字节（菜单按键）。codex 没有终端：只认单独一个 Esc（= [`Self::interrupt`]），
    /// 其余丢弃并发一条 `Notice`。
    pub fn raw(&self, bytes: impl Into<Vec<u8>>) -> Result<(), Closed> {
        self.cmd(Command::Raw(bytes.into()))
    }

    /// 打断当前回合：claude 写一个 Esc，codex 发 `turn/interrupt`（没有进行中的回合就什么都不做）。
    pub fn interrupt(&self) -> Result<(), Closed> {
        self.cmd(Command::Interrupt)
    }

    /// 回答屏上的待决策框（T10d 安全按键）：`expect` = 要答的那个框的指纹（`PendingDecision::fingerprint`）。
    /// 每一下按之前会话自己回读屏幕，框换了 / 高亮挪了就停手回 `Conflict`；按完回读一次回 `Answered`。
    /// 命令在调用时就进了会话的命令队列（返回的 future 只是等结果），先后与 `send` / `raw` 一致。
    /// codex 回 `Unsupported`。
    pub fn answer(
        &self,
        expect: String,
        steps: Vec<AnswerStep>,
    ) -> impl std::future::Future<Output = Result<AnswerOutcome, Closed>> + Send + 'static {
        let (reply, rx) = oneshot::channel();
        let sent = self.cmd(Command::Answer {
            expect,
            steps,
            reply,
        });
        async move {
            sent?;
            rx.await.map_err(|_| Closed)
        }
    }

    /// 会话控制（换模型 / 推理强度 / fast、压缩）。不支持的执行器回 `Unsupported`。
    pub async fn control(&self, op: Control) -> Result<ControlOutcome, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Control { op, reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub fn resize(&self, cols: u16, lines: u16) -> Result<(), Closed> {
        self.cmd(Command::Resize { cols, lines })
    }

    /// SIGTERM，`stop_grace` 后 `killpg(SIGKILL)`。退出后发 `Exited { stopped_by_user: true }`。
    pub fn stop(&self) -> Result<(), Closed> {
        self.cmd(Command::Stop)
    }

    /// 额度重置唤醒到点后调：清额度类 health、重新武装扫描器。
    pub fn clear_quota_health(&self) -> Result<(), Closed> {
        self.cmd(Command::ClearQuotaHealth)
    }

    pub async fn screen_text(&self, max_lines: usize) -> Result<String, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::ScreenText { max_lines, reply })?;
        rx.await.map_err(|_| Closed)
    }

    /// 终端快照；`None` = 这个执行器没有终端（codex：用 `screen_text` 读结构化记录的文本）。
    pub async fn snapshot(&self) -> Result<Option<Snapshot>, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Snapshot { reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub async fn status(&self) -> Result<SessionStatus, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Status { reply })?;
        rx.await.map_err(|_| Closed)
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn cmd(&self, c: Command) -> Result<(), Closed> {
        self.tx.send(c).map_err(|_| Closed)
    }
}

/// 起一个 claude 会话。账本：记过就 `--resume`，否则新号且**起进程前**落账。
/// 返回句柄和事件流（第一条是 `Spawned`）。
pub async fn start_claude(
    start: ClaudeStart,
    config: DriverConfig,
) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError> {
    let ledger = start.ledger.clone();
    let recorded = tokio::task::spawn_blocking(move || ledger.recorded())
        .await
        .map_err(|e| StartError::Ledger(io::Error::other(e)))?
        .map_err(StartError::Ledger)?;
    let decision = decide_resume(recorded.as_deref());
    if let Some(why) = &decision.ignored {
        eprintln!("crewd: session {}: {why}", start.session_id);
    }
    let (auth_tx, auth_rx) = mpsc::unbounded_channel();
    let running = launch(&start, &config, decision.arg, &auth_tx).await?;
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (tx, cmd_rx) = mpsc::unbounded_channel();
    let pending = start
        .spec
        .initial_prompt
        .iter()
        .filter(|p| !p.is_empty())
        .map(|text| PendingText {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            text: text.clone(),
        })
        .collect();
    let resumed = match &running.agent {
        AgentSessionArg::Resume(id) => Some(id.clone()),
        AgentSessionArg::New(_) => None,
    };
    let core = Core::new(config.core, config.screen, Instant::now(), pending, resumed);
    let _ = ev_tx.send(running.spawned_event());
    tokio::spawn(run(Task {
        start,
        config,
        core,
        answers: BTreeMap::new(),
        next_answer: 0,
        running,
        cmd_rx,
        ev_tx,
        auth_tx,
        auth_rx,
    }));
    Ok((SessionHandle::new(tx), ev_rx))
}

/// 一个起好的进程及其附属物。
struct Running {
    child: tokio::process::Child,
    pid: u32,
    master: Arc<PtyMaster>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
    writer_task: JoinHandle<()>,
    auth_task: Option<JoinHandle<()>>,
    agent: AgentSessionArg,
    /// 启动临时文件。字段顺序要紧：先删文件、再放开会话号（[`LiveId`]）——反过来，
    /// 同号的新会话可能刚写好的目录会被这里删掉。
    files: Option<LaunchFiles>,
    live: Option<LiveId>,
}

/// 本进程里正在跑的 agent 会话号（claude 会话号与 codex thread id 共用一张表）。
static LIVE_AGENT_IDS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// 占着一个会话号；drop 时放开。
pub(crate) struct LiveId(String);

impl LiveId {
    pub(crate) fn id(&self) -> &str {
        &self.0
    }

    pub(crate) fn claim(id: &str) -> Result<Self, StartError> {
        let mut live = LIVE_AGENT_IDS.lock().expect("会话号表锁中毒");
        if !live.insert(id.to_string()) {
            return Err(StartError::AlreadyRunning(id.to_string()));
        }
        Ok(Self(id.to_string()))
    }
}

impl Drop for LiveId {
    fn drop(&mut self) {
        if let Ok(mut live) = LIVE_AGENT_IDS.lock() {
            live.remove(&self.0);
        }
    }
}

impl Running {
    fn spawned_event(&self) -> SessionEvent {
        SessionEvent::Spawned {
            pid: self.pid,
            agent_session_id: self.agent.id().to_string(),
            resumed: matches!(self.agent, AgentSessionArg::Resume(_)),
        }
    }

    fn shutdown_helpers(&mut self) {
        if let Some(t) = self.auth_task.take() {
            t.abort();
        }
        self.writer_task.abort();
    }

    /// 进程已收尸：整组补一刀、停掉附属任务、删启动文件、放开会话号（按这个顺序）。
    ///
    /// 补刀：会话首进程（claude）收到 SIGTERM 自己退了，它起的子进程（工具调用、后台命令）
    /// 还留在组里——忽略 HUP 的就成了孤儿。setsid 后 pgid == pid；组里还有成员时这个号
    /// 不会被别的进程拿去当 pid，组空了 killpg 只会得到 ESRCH（无害）。
    async fn release(&mut self) {
        kill_group_only(self.pid);
        self.shutdown_helpers();
        if let Some(files) = self.files.take() {
            let _ = tokio::task::spawn_blocking(move || drop(files)).await;
        }
        self.live = None;
    }
}

type AuthMsg = (String, AuthState);

async fn launch(
    start: &ClaudeStart,
    config: &DriverConfig,
    agent: AgentSessionArg,
    auth_tx: &mpsc::UnboundedSender<AuthMsg>,
) -> Result<Running, StartError> {
    // 先占号再动任何东西：落账、清启动目录都不能落到一个正在跑的同号会话头上。
    let live = LiveId::claim(agent.id())?;
    if let AgentSessionArg::New(id) = &agent {
        let (ledger, id) = (start.ledger.clone(), id.clone());
        blocking(move || ledger.record(&id))
            .await
            .map_err(StartError::Ledger)?;
    }
    let (spec, a, dir) = (
        start.spec.clone(),
        agent.clone(),
        config.runtime_dir.clone(),
    );
    let (args, files) = blocking(move || prepare_claude(&spec, a, &dir))
        .await
        .map_err(StartError::Files)?;
    let env = build_env_from_process(&start.child_path, &start.spec.additional_env);
    let since = SystemTime::now();
    let spec = SpawnSpec {
        program: start.program.clone(),
        args: args.argv(),
        env,
        cwd: start.spec.workdir.clone(),
        cols: config.screen.cols as u16,
        lines: config.screen.lines as u16,
    };
    let proc = spawn_pty(&spec).map_err(|source| {
        let underlying = format!("{}: {source}", start.program.display());
        let health = launch_failure(
            LaunchVerdict::SpawnFailed,
            config.core.launch_deadline,
            Some(&underlying),
        )
        .expect("SpawnFailed 一定有说明");
        StartError::Spawn { health, source }
    })?;
    let master = Arc::new(proc.master);
    let mut child = proc.child;
    if let Some(registry) = &config.registry {
        let (registry, session_id, pid) = (registry.clone(), start.session_id.clone(), proc.pid);
        let recorded = blocking(move || {
            // 只登记自成进程组的（setsid 后 pgid == pid）：回收时按组杀，组号不对会杀错人。
            let r = match probe(pid as i32) {
                ProbeResult::Found(identity) if identity.pgid == pid as i32 => {
                    registry.record(&session_id, identity)
                }
                ProbeResult::Found(identity) => Err(io::Error::other(format!(
                    "刚起的进程 {pid} 不是自己进程组的组长（pgid {}），不登记",
                    identity.pgid
                ))),
                other => Err(io::Error::other(format!("读不到刚起的进程身份：{other:?}"))),
            };
            if r.is_err() {
                // `record` 落盘失败时内存里已经记上了：抹掉，免得下次落盘把一个死 pid 带进去。
                let _ = registry.forget(&session_id);
            }
            r
        })
        .await;
        if let Err(e) = recorded {
            kill_group(proc.pid);
            let e = match reap_killed(&mut child, &master, REAP_LIMIT).await {
                Ok(_) => e,
                Err(reap) => io::Error::other(format!("{e}；杀掉刚起的进程后{reap}")),
            };
            return Err(StartError::Registry(e));
        }
    }
    let (writer, mut writes) = mpsc::unbounded_channel::<Vec<u8>>();
    let w = master.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(bytes) = writes.recv().await {
            if w.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let auth_task = config.claude_projects_dir.clone().map(|projects_dir| {
        let probe = AuthProbe {
            session_id: agent.id().to_string(),
            projects_dir,
            since,
        };
        let (tx, every) = (auth_tx.clone(), config.auth_poll);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let p = probe.clone();
                let Ok(state) = tokio::task::spawn_blocking(move || p.read()).await else {
                    return;
                };
                if tx.send((probe.session_id.clone(), state)).is_err() {
                    return;
                }
            }
        })
    });
    Ok(Running {
        child,
        pid: proc.pid,
        master,
        writer,
        writer_task,
        auth_task,
        agent,
        files: Some(files),
        live: Some(live),
    })
}

struct Task {
    start: ClaudeStart,
    config: DriverConfig,
    core: Core,
    /// 等结果的回答（[`Command::Answer`]），按 `Core::answer` 的 id。
    answers: BTreeMap<u64, oneshot::Sender<AnswerOutcome>>,
    next_answer: u64,
    running: Running,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<SessionEvent>,
    auth_tx: mpsc::UnboundedSender<AuthMsg>,
    auth_rx: mpsc::UnboundedReceiver<AuthMsg>,
}

/// 整组 SIGKILL，组发不出去再单杀会话首进程。
pub(crate) fn kill_group(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process};
    if !kill_group_only(pid)
        && let Some(p) = Pid::from_raw(pid as i32)
    {
        let _ = kill_process(p, Signal::KILL);
    }
}

/// 只对进程组 `pid`（setsid 后 pgid == pid）发 SIGKILL；返回是否发出去了（ESRCH 也算：组里
/// 已经没人了）。过登记表同一道闸：pgid <= 1 或就是 crewd 自己那一组，一个信号都不发。
pub(crate) fn kill_group_only(pid: u32) -> bool {
    use rustix::process::{Pid, Signal, getpgrp, kill_process_group};
    let own_pgid = getpgrp().as_raw_nonzero().get();
    let Ok(pgid) = i32::try_from(pid) else {
        return false;
    };
    if group_signal_refused(pgid, own_pgid) {
        return false;
    }
    let Some(group) = Pid::from_raw(pgid) else {
        return false;
    };
    match kill_process_group(group, Signal::KILL) {
        Ok(()) => true,
        Err(e) => e == rustix::io::Errno::SRCH,
    }
}

pub(crate) fn terminate(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process};
    if let Some(p) = Pid::from_raw(pid as i32) {
        let _ = kill_process(p, Signal::TERM);
    }
}

/// 在阻塞线程池里跑一段文件 IO（账本、启动文件、登记表），不占 tokio 工作线程。
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(io::Error::other)?
}

/// [`reap_killed`] 的上限。
pub(crate) const REAP_LIMIT: Duration = Duration::from_secs(5);

/// 收一个**已经发过 SIGKILL** 的进程，同时把主端的输出读掉扔掉：macOS 上会话首进程退出时
/// 要等 PTY 里的输出被读走（`ttywait`），没人读主端它就永远收不了尸。超过 `limit` 报错，不挂住
/// （丢下的 `Child` 由 tokio 之后收尸）。
async fn reap_killed(
    child: &mut tokio::process::Child,
    master: &PtyMaster,
    limit: Duration,
) -> io::Result<ExitStatusKind> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = vec![0u8; 16 * 1024];
    let mut reading = true;
    loop {
        tokio::select! {
            status = child.wait() => return Ok(decode(status)),
            r = master.read(&mut buf), if reading => {
                if !matches!(r, Ok(n) if n > 0) {
                    reading = false;
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{}ms 内没能收尸", limit.as_millis()),
                ));
            }
        }
    }
}

pub(crate) fn decode(status: io::Result<std::process::ExitStatus>) -> ExitStatusKind {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(s) => match (s.code(), s.signal()) {
            (Some(code), _) => ExitStatusKind::Code(code),
            (None, Some(sig)) => ExitStatusKind::Signal(sig),
            (None, None) => ExitStatusKind::Code(-1),
        },
        Err(_) => ExitStatusKind::Code(-1),
    }
}

/// 定时拍 / 回答按键之前：把 PTY 里此刻已经到了的输出（非阻塞）喂进屏幕，再让 Core 按「当前屏幕」决定按不按
/// （T10d fix M2）。`select!` 不偏向读那一支：一直有输出时会饿死命令；这里只读已经到了的，至多 [`DRAIN_CHUNKS`] 块。
fn drain_ready(t: &mut Task) {
    let mut buf = vec![0u8; 16 * 1024];
    for _ in 0..DRAIN_CHUNKS {
        match t.running.master.try_read(&mut buf) {
            Ok(Some(n)) if n > 0 => t.core.on_output(&buf[..n], Instant::now()),
            _ => return,
        }
    }
}

/// [`drain_ready`] 一次至多读几块（不让一直吐字的子进程把这一拍拖住）。
const DRAIN_CHUNKS: usize = 64;

/// 子进程退出后把 PTY 里剩下的输出读干净（最多 `limit`）：claude 拒绝续跑时是先打印再退出的。
async fn drain(master: &PtyMaster, core: &mut Core, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = vec![0u8; 16 * 1024];
    while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, master.read(&mut buf)).await {
        if n == 0 {
            break;
        }
        core.on_output(&buf[..n], Instant::now());
    }
}

async fn run(mut t: Task) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut reading = true;
    let mut commands_open = true;
    let mut stop_deadline: Option<tokio::time::Instant> = None;
    // 已经在停（`stop()`、句柄全丢）：不重起——那是我们要它停的。`Core::stop` 之后本来就不再报
    // `ResumeRefused`；这里再挡一道，免得两边的判断哪天走散。
    let mut stopping = false;
    let mut retried = false;
    let mut tick = tokio::time::interval(t.config.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let wake = t.core.next_wakeup().map(tokio::time::Instant::from_std);
        let mut exit = None;
        tokio::select! {
            r = t.running.master.read(&mut buf), if reading => match r {
                Ok(n) if n > 0 => t.core.on_output(&buf[..n], Instant::now()),
                _ => reading = false,
            },
            status = t.running.child.wait() => exit = Some(decode(status)),
            cmd = t.cmd_rx.recv(), if commands_open => match cmd {
                Some(c) => {
                    if matches!(c, Command::Answer { .. }) {
                        // 第一下当场按（按之前回读）：先把已经到了的输出喂进屏幕，别拿旧屏核框。
                        drain_ready(&mut t);
                    }
                    if handle(&mut t, c) {
                        begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                    }
                }
                None => {
                    commands_open = false;
                    begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                }
            },
            _ = tick.tick() => {
                drain_ready(&mut t);
                t.core.on_tick(Instant::now());
            }
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
                // 到点的多半是排好的按键（回答 / 提交拍）：按之前回读的屏幕要是此刻的（T10d fix M2）。
                drain_ready(&mut t);
                t.core.on_tick(Instant::now());
            }
            _ = tokio::time::sleep_until(stop_deadline.unwrap_or_else(tokio::time::Instant::now)), if stop_deadline.is_some() => {
                stop_deadline = None;
                kill_group(t.running.pid);
            }
            Some((id, state)) = t.auth_rx.recv() => {
                if id == t.running.agent.id() {
                    t.core.on_auth(state, Instant::now());
                }
            }
        }
        if exit.is_some() {
            drain(&t.running.master, &mut t.core, Duration::from_millis(500)).await;
        }
        let actions = t.core.take_actions();
        let refused = dispatch(&mut t, actions);
        if refused && !retried && !stopping {
            retried = true;
            // 被拒的那个进程：已经自己退了就用它的退出状态，还活着就杀掉收尸（边读边等）。
            let old_status = match exit.take() {
                Some(status) => status,
                None => {
                    kill_group(t.running.pid);
                    let r = &mut t.running;
                    match reap_killed(&mut r.child, &r.master, REAP_LIMIT).await {
                        Ok(status) => status,
                        Err(e) => {
                            eprintln!(
                                "crewd: session {}: 续跑被拒的进程 {} {e}，照样换新号重起",
                                t.start.session_id, r.pid
                            );
                            ExitStatusKind::Signal(9)
                        }
                    }
                }
            };
            match respawn(&mut t).await {
                Ok(()) => {
                    reading = true;
                    continue;
                }
                Err(e) => {
                    let health = match &e {
                        StartError::Spawn { health, .. } => health.clone(),
                        other => Health::new(HealthKind::LaunchFailed, other.to_string()),
                    };
                    let _ = t.ev_tx.send(SessionEvent::Health(Some(health)));
                    finish(&mut t, old_status).await;
                    return;
                }
            }
        }
        if let Some(status) = exit {
            finish(&mut t, status).await;
            return;
        }
    }
}

/// 进程已收尸（退出由 `Child::wait` 决定，不看主端 EOF：主端/从端漏进别的进程时 EOF 可能
/// 永远不来）。先收拾——登记表、启动文件、会话号——再把 `Core` 最后的事件（`Exited` 在末尾）
/// 发出去：调用方见到 `Exited` 就能马上用同一个会话号重起。
async fn finish(t: &mut Task, status: ExitStatusKind) {
    t.core.on_exit(status, Instant::now());
    let actions = t.core.take_actions();
    t.running.release().await;
    if let Some(registry) = t.config.registry.clone() {
        let id = t.start.session_id.clone();
        let _ = blocking(move || registry.forget(&id)).await;
    }
    dispatch(t, actions);
}

/// 开始停：只做一次。再叫停（`stop()` 连按、句柄随后全丢）既不重发 SIGTERM，也不把
/// SIGKILL 的期限往后推——否则叫停比 `stop_grace` 还勤时，忽略 SIGTERM 的进程永远杀不掉。
fn begin_stop(t: &mut Task, stopping: &mut bool, stop_deadline: &mut Option<tokio::time::Instant>) {
    if *stopping {
        return;
    }
    *stopping = true;
    t.core.stop(Instant::now());
    terminate(t.running.pid);
    *stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
}

/// 处理一条命令；返回 true = 要停（由 [`begin_stop`] 去重）。
fn handle(t: &mut Task, cmd: Command) -> bool {
    let now = Instant::now();
    match cmd {
        Command::Send { id, text, context } => {
            t.core
                .send(id, &with_untrusted_context(&context, &text), now)
        }
        Command::Raw(bytes) => t.core.raw(&bytes),
        Command::Interrupt => t.core.interrupt(),
        Command::Resize { cols, lines } => t.core.resize(cols, lines),
        Command::Stop => return t.core.is_running(),
        Command::ClearQuotaHealth => t.core.clear_quota_health(now),
        Command::ScreenText { max_lines, reply } => {
            let _ = reply.send(t.core.screen_text(max_lines));
        }
        Command::Snapshot { reply } => {
            let _ = reply.send(Some(t.core.snapshot()));
        }
        Command::Control { op, reply } => {
            let _ = reply.send(ControlOutcome::Unsupported(format!(
                "claude 会话的 {op:?} 由屏幕驱动（M3 的 /model、/compact），执行器不直接支持"
            )));
        }
        Command::Answer {
            expect,
            steps,
            reply,
        } => {
            let id = t.next_answer;
            t.next_answer += 1;
            t.answers.insert(id, reply);
            t.core.answer(id, expect, steps, now);
        }
        Command::Status { reply } => {
            let _ = reply.send(SessionStatus {
                state: t.core.state(),
                working: t.core.is_working(),
                health: t.core.health().cloned(),
                pending_decision: t.core.pending_decision().cloned(),
                pid: t.running.pid,
                agent_session_id: t.running.agent.id().to_string(),
            });
        }
    }
    false
}

/// 执行 Core 攒下的动作；返回是否见到了 `ResumeRefused`。
fn dispatch(t: &mut Task, actions: Vec<Action>) -> bool {
    let mut refused = false;
    for action in actions {
        match action {
            Action::Write(bytes) => {
                let _ = t.running.writer.send(bytes);
            }
            Action::Resize { cols, lines } => {
                let _ = t.running.master.resize(cols, lines);
            }
            Action::Event(e) => {
                refused |= matches!(e, SessionEvent::ResumeRefused { .. });
                let _ = t.ev_tx.send(e);
            }
            // `None`（会话停了 / 退了）= 丢掉回执，等的一方拿到 `Closed`。
            Action::AnswerDone { id, outcome } => {
                if let (Some(reply), Some(outcome)) = (t.answers.remove(&id), outcome) {
                    let _ = reply.send(outcome);
                }
            }
        }
    }
    refused
}

/// claude 拒绝了 `--resume`：换新号（先落账）重起一次，没投出去的正文搬过去。
async fn respawn(t: &mut Task) -> Result<(), StartError> {
    let pending = t.core.take_pending();
    // 旧 Core 上还没答完的回答随它作废：丢掉回执（等的一方拿到 `Closed`）。
    t.answers.clear();
    let agent = AgentSessionArg::New(new_agent_session_id());
    let running = launch(&t.start, &t.config, agent, &t.auth_tx).await?;
    let mut old = std::mem::replace(&mut t.running, running);
    old.release().await;
    if t.core.is_working() {
        // 被拒的那个进程吐过字：补上它的 idle 边沿，免得外面以为还在干活。
        let _ = t.ev_tx.send(SessionEvent::Working(false));
    }
    t.core = Core::new(
        t.config.core,
        t.config.screen,
        Instant::now(),
        pending,
        None,
    );
    let _ = t.ev_tx.send(t.running.spawned_event());
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// claude 拒绝续跑是「打印一行就退出」：退出被先看到时，PTY 里还躺着那行字。
    /// 只在 Linux 上跑：macOS 关闭 tty 从端时会等输出被读走（`ttywait`），没人读主端时
    /// 子进程根本退不掉——那边「先看到退出、后读输出」不会发生（实测：这条在 macOS 上挂住）。
    #[tokio::test]
    async fn output_left_in_the_pty_after_exit_is_drained() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        let mut p = spawn_pty(&SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo last-words; exit 1".into()],
            env,
            cwd: std::env::temp_dir(),
            cols: 80,
            lines: 25,
        })
        .unwrap();
        p.child.wait().await.unwrap();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            Instant::now(),
            vec![],
            None,
        );
        drain(&p.master, &mut core, Duration::from_millis(500)).await;
        assert!(core.screen_text(25).contains("last-words"));
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn untrusted_context_is_wrapped_in_key_order_before_the_text() {
        let mut ctx = UntrustedContext::new();
        ctx.insert("crew_whiteboard".into(), "A: 我改完了".into());
        ctx.insert("aa_first".into(), "x".into());
        ctx.insert("empty".into(), String::new());
        assert_eq!(
            with_untrusted_context(&ctx, "开工"),
            "<external_aa_first>\nx\n</external_aa_first>\n\n\
             <external_crew_whiteboard>\nA: 我改完了\n</external_crew_whiteboard>\n\n开工"
        );
        assert_eq!(
            with_untrusted_context(&UntrustedContext::new(), "开工"),
            "开工"
        );
    }

    /// 上下文不算正文：只有白板、没有正文的一条照样当场拒掉，不占编号、不进命令通道。
    #[test]
    fn context_without_text_is_refused_like_an_empty_send() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = SessionHandle::new(tx);
        let ctx: UntrustedContext = [("crew_whiteboard".to_string(), "A: 早".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            h.send_with_context(" \n", ctx.clone()),
            Err(SendError::EmptyText)
        );
        assert!(rx.try_recv().is_err(), "拒掉的不进命令通道");
        assert_eq!(h.send_with_context("开工", ctx), Ok(BRIEF_DELIVERY_ID + 1));
    }
}
