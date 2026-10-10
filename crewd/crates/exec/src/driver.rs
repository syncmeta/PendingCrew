//! 一个 claude 会话的 tokio 任务，和其余部分唯一的接口 [`SessionHandle`]（spec §13 Q12）。
//!
//! 任务拥有 PTY 主端、子进程、[`Core`]；命令从句柄进，事件从 `mpsc` 出。
//! 节拍：PTY 输出先有界排空再一拍、0.6s 定时一拍、`Core::next_wakeup` 到点一拍。
//! 写 PTY 是本任务的有界非阻塞队列：每个 syscall 前重新核 raw 闸；子进程不读时仍能读输出、处理命令。
//!
//! 以后执行器拆成独立进程，只把 `SessionHandle` 的命令与事件换成线协议。
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    Action, AnswerOutcome, AnswerStep, BRIEF_DELIVERY_ID, Core, CoreConfig, DeliveryWriteToken,
    DerivedState, ExitStatusKind, PendingText, SessionEvent,
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
    /// claude 的 raw 入队与实际 PTY syscall 共用这道锁：raw 一入队就撤销旧正文的写权限。
    /// codex 无 PTY，不装这道闸。
    raw_fence: Option<Arc<Mutex<u64>>>,
}

impl SessionHandle {
    /// 驱动层建句柄（投递编号从 `BRIEF_DELIVERY_ID + 1` 起，0 留给开场任务）。
    pub(crate) fn new(tx: mpsc::UnboundedSender<Command>) -> Self {
        Self {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
            raw_fence: None,
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
        let bytes = bytes.into();
        if let Some(fence) = &self.raw_fence
            && !bytes.is_empty()
        {
            let mut epoch = fence.lock().expect("raw fence poisoned");
            *epoch += 1;
            // Send under the same lock: concurrent handle clones preserve epoch / command order.
            self.cmd(Command::Raw(bytes))
        } else {
            self.cmd(Command::Raw(bytes))
        }
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
    let raw_fence = Arc::new(Mutex::new(0));
    let mut handle = SessionHandle::new(tx);
    handle.raw_fence = Some(raw_fence.clone());
    tokio::spawn(run(Task {
        start,
        config,
        core,
        raw_fence,
        input_epoch: 0,
        writes: VecDeque::new(),
        answers: BTreeMap::new(),
        next_answer: 0,
        running,
        cmd_rx,
        ev_tx,
        auth_tx,
        auth_rx,
    }));
    Ok((handle, ev_rx))
}

/// 一个起好的进程及其附属物。
struct Running {
    child: tokio::process::Child,
    pid: u32,
    master: Arc<PtyMaster>,
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
        auth_task,
        agent,
        files: Some(files),
        live: Some(live),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WriteProgress {
    id: u64,
    token: DeliveryWriteToken,
    complete: bool,
}

struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
    delivery: Option<(u64, DeliveryWriteToken)>,
}

struct Task {
    raw_fence: Arc<Mutex<u64>>,
    /// 已处理的 raw 命令数；锁里的数更大 = raw 已入队但还没处理，不能写正文。
    input_epoch: u64,
    writes: VecDeque<PendingWrite>,
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

/// 把这一拍里已经可读的输出逐块喂进解析器，**不逐块 flush**（T10d fix round 2）。
/// `first` 是普通 read 分支已经读走的第一块；它也占一块预算。
///
/// 只有确认读到 WouldBlock / EOF 才回 true，调用方才可以做依赖完整屏幕的 tick / flush。
/// 预算用尽或读失败回 false：这拍不按回答键，下一次 select / drain 再继续。
/// `select!` 仍不偏向读分支，加上预算，一直吐字的子进程不会饿死命令。
fn drain_ready(master: &PtyMaster, core: &mut Core, first: Option<&[u8]>, budget: usize) -> bool {
    let mut read_chunks = 0;
    if let Some(bytes) = first {
        core.ingest_output(bytes, Instant::now());
        read_chunks += 1;
    }
    let mut buf = [0u8; PTY_READ_SIZE];
    while read_chunks < budget {
        match master.try_read(&mut buf) {
            Ok(Some(n)) if n > 0 => {
                core.ingest_output(&buf[..n], Instant::now());
                read_chunks += 1;
            }
            Ok(None | Some(0)) => return true,
            Err(_) => return false,
            _ => unreachable!("有字节或已经排空"),
        }
    }
    false
}

const PTY_READ_SIZE: usize = 16 * 1024;
/// 一拍至多读几块（不让一直吐字的子进程把这一拍拖住）；触顶不等于已经排空。
const DRAIN_CHUNKS: usize = 64;

/// 子进程退出后把 PTY 里剩下的输出读干净（最多 `limit`）：claude 拒绝续跑时是先打印再退出的。
async fn drain(master: &PtyMaster, core: &mut Core, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = [0u8; PTY_READ_SIZE];
    while let Ok(Ok(n)) = tokio::time::timeout_at(deadline, master.read(&mut buf)).await {
        if n == 0 {
            core.after_output_drain(Instant::now());
            return;
        }
        core.ingest_output(&buf[..n], Instant::now());
    }
    // 子进程虽退了，继承从端的后代可能还活着：超时不是排空。非阻塞确认之后才 flush。
    if drain_ready(master, core, None, DRAIN_CHUNKS) {
        core.after_output_drain(Instant::now());
    }
}

async fn run(mut t: Task) {
    let mut buf = [0u8; PTY_READ_SIZE];
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
        let master = t.running.master.clone();
        tokio::select! {
            ready = master.writable(), if !t.writes.is_empty() => {
                // Even if select chose writable over a queued Raw, process commands before any syscall.
                // Bounded like the output drain; raw_fence blocks writes if a Raw remains beyond the cap.
                let mut answer = false;
                for _ in 0..DRAIN_CHUNKS {
                    match t.cmd_rx.try_recv() {
                        Ok(c) => {
                            answer |= matches!(c, Command::Answer { .. });
                            if handle(&mut t, c) {
                                begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            commands_open = false;
                            begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                            break;
                        }
                    }
                }
                if answer && drain_ready(&master, &mut t.core, None, DRAIN_CHUNKS) {
                    t.core.after_output_drain(Instant::now());
                }
                match ready {
                    Ok(mut guard) => match write_one(&mut t) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => guard.clear_ready(),
                        Err(e) => {
                            let _ = t.ev_tx.send(SessionEvent::Notice(format!("PTY write failed: {e}")));
                            begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                        }
                        Ok(()) => {}
                    },
                    Err(_) => begin_stop(&mut t, &mut stopping, &mut stop_deadline),
                }
            }
            r = t.running.master.read(&mut buf), if reading => match r {
                Ok(n) if n > 0 => {
                    if drain_ready(&t.running.master, &mut t.core, Some(&buf[..n]), DRAIN_CHUNKS) {
                        t.core.after_output_drain(Instant::now());
                    }
                }
                _ => reading = false,
            },
            status = t.running.child.wait() => exit = Some(decode(status)),
            cmd = t.cmd_rx.recv(), if commands_open => match cmd {
                Some(c) => {
                    let answer = matches!(c, Command::Answer { .. });
                    // handle 只排回答，不当场 flush；预算触顶时第一下也必须等下一轮排空。
                    if handle(&mut t, c) {
                        begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                    }
                    if answer && drain_ready(&t.running.master, &mut t.core, None, DRAIN_CHUNKS) {
                        t.core.after_output_drain(Instant::now());
                    }
                }
                None => {
                    commands_open = false;
                    begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                }
            },
            _ = tick.tick() => {
                if drain_ready(&t.running.master, &mut t.core, None, DRAIN_CHUNKS) {
                    t.core.on_tick(Instant::now());
                }
            }
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
                // 到点的多半是排好的按键：只有已经可读的输出全喂完，屏幕才可用来核框。
                if drain_ready(&t.running.master, &mut t.core, None, DRAIN_CHUNKS) {
                    t.core.on_tick(Instant::now());
                }
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
    t.writes.clear();
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
        Command::Raw(bytes) => {
            if !bytes.is_empty() {
                t.input_epoch += 1;
            }
            t.core.raw(&bytes);
        }
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
            t.core.queue_answer(id, expect, steps, now);
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

/// 单个有界 syscall，与 SessionHandle::raw 入队线性化。锁绝不跨 await / 阻塞 IO。
/// 不再有旁路 write_all 任务：剩余正文、重试的 Ctrl-U、每一拍提交都能在这里被撤销。
fn write_one(t: &mut Task) -> io::Result<()> {
    let completed = write_ready(
        &t.running.master,
        &t.core,
        &t.raw_fence,
        t.input_epoch,
        &mut t.writes,
    )?;
    if let Some(progress) = completed {
        let now = Instant::now();
        t.core
            .delivery_write_progress(progress.id, progress.token, now);
        if progress.complete {
            t.core
                .delivery_write_completed(progress.id, progress.token, now);
        }
    }
    Ok(())
}

fn write_ready(
    master: &PtyMaster,
    core: &Core,
    raw_fence: &Mutex<u64>,
    input_epoch: u64,
    writes: &mut VecDeque<PendingWrite>,
) -> io::Result<Option<WriteProgress>> {
    let epoch = raw_fence.lock().expect("raw fence poisoned");
    if *epoch != input_epoch {
        return Ok(None);
    }
    while let Some(write) = writes.front_mut() {
        if write
            .delivery
            .is_some_and(|(id, token)| !core.delivery_write_pending(id, token))
            || write.offset == write.bytes.len()
        {
            writes.pop_front();
            continue;
        }
        let end = (write.offset + 4096).min(write.bytes.len());
        let n = master.try_write(&write.bytes[write.offset..end])?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        write.offset += n;
        let progress = write.delivery.map(|(id, token)| WriteProgress {
            id,
            token,
            complete: write.offset == write.bytes.len(),
        });
        if write.offset == write.bytes.len() {
            writes.pop_front();
        }
        return Ok(progress);
    }
    Ok(None)
}

/// 执行 Core 攒下的动作；返回是否见到了 `ResumeRefused`。
fn dispatch(t: &mut Task, actions: Vec<Action>) -> bool {
    let mut refused = false;
    for action in actions {
        match action {
            Action::Write(bytes) => t.writes.push_back(PendingWrite {
                bytes,
                offset: 0,
                delivery: None,
            }),
            Action::DeliveryWrite { id, token, bytes } => t.writes.push_back(PendingWrite {
                bytes,
                offset: 0,
                delivery: Some((id, token)),
            }),
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
    // Old writer work belongs to the retired PTY. The new Core starts each pending body afresh.
    t.writes.clear();
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

#[cfg(test)]
mod output_drain_tests {
    use super::*;
    use crate::pty::PtyProcess;
    use crewd_term::blocking_dialog;

    fn menu(prompt: &str, selected: usize) -> Vec<u8> {
        let mark = |i| if i == selected { "❯" } else { " " };
        format!(
            "\x1b[2J\x1b[H{prompt}\r\n{} 1. Yes\r\n{} 2. No\r\n{} 3. Ask later",
            mark(0),
            mark(1),
            mark(2)
        )
        .into_bytes()
    }

    fn shown(core: &Core) -> PendingDecision {
        let rows = core
            .screen_text(25)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        blocking_dialog(&rows).expect("前提：屏上有待决策框")
    }

    fn due_answer() -> (Core, PendingDecision) {
        let now = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            now,
            vec![],
            None,
        );
        core.on_output(&menu("Proceed?", 0), now);
        core.take_actions();
        let d = shown(&core);
        // 与 Command::Answer 一样只排第一下；在第一块 PTY 输出之前已经到点。
        core.queue_answer(
            1,
            d.fingerprint(),
            vec![AnswerStep {
                bytes: b"\r".to_vec(),
                selected_before: Some(0),
            }],
            now,
        );
        assert!(core.take_actions().is_empty(), "排队本身不能绕过排空闸");
        (core, d)
    }

    fn writes(actions: &[Action]) -> Vec<&[u8]> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Write(bytes) | Action::DeliveryWrite { bytes, .. } => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .collect()
    }

    /// 真 PTY，子进程只把合成字节写进去，然后落盘回执。读之前等回执，保证第二块早已可读，
    /// 不是「我们试图排空之后它才到」的另一种竞态；不启动真 claude / codex。
    async fn readable_output(bytes: &[u8]) -> (tempfile::TempDir, PtyProcess) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("screen.bin");
        let ready = dir.path().join("ready");
        std::fs::write(&source, bytes).unwrap();
        let p = spawn_pty(&SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "cat '{}' && printf ready > '{}' && sleep 30",
                    source.display(),
                    ready.display()
                ),
            ],
            env: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            cwd: dir.path().to_path_buf(),
            cols: 80,
            lines: 25,
        })
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("合成输出 5s 内必须已经全写进 PTY");
        (dir, p)
    }

    async fn stop(mut p: PtyProcess) {
        kill_group(p.pid);
        reap_killed(&mut p.child, &p.master, REAP_LIMIT)
            .await
            .unwrap();
    }

    /// R3: Raw is enqueued after Core issued a body or submit action, before the driver syscall.
    /// This deliberately leaves the command unhandled, so only SessionHandle's synchronous fence can stop it.
    #[tokio::test]
    async fn a_raw_enqueued_after_readiness_revokes_every_queued_delivery_write() {
        for beat in 0..=3 {
            let (_dir, p) = readable_output(b"").await;
            let now = Instant::now();
            let mut core = Core::new(
                CoreConfig::default(),
                ScreenConfig::default(),
                now,
                vec![],
                None,
            );
            let screen = |text| {
                format!(
                    "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {text}\r\n────────────────────\r\n\x1b[2m ? for shortcuts\x1b[22m"
                )
            };
            core.on_output(screen("").as_bytes(), now);
            core.send(1, "BOT", now);
            core.take_actions();
            core.on_tick(now + Duration::from_millis(300));
            let mut actions = core.take_actions();
            complete_actions(&mut core, &actions, now + Duration::from_millis(300));
            if beat > 0 {
                core.on_output(screen("BOT").as_bytes(), now + Duration::from_millis(900));
                actions = core.take_actions();
                complete_actions(&mut core, &actions, now + Duration::from_millis(900));
            }
            if beat > 1 {
                core.on_tick(now + Duration::from_millis(1000));
                actions = core.take_actions();
                complete_actions(&mut core, &actions, now + Duration::from_millis(1000));
            }
            if beat > 2 {
                core.on_tick(now + Duration::from_millis(1100));
                actions = core.take_actions();
            }
            let (bytes, token) = actions
                .into_iter()
                .find_map(|a| match a {
                    Action::DeliveryWrite {
                        id: 1,
                        token,
                        bytes,
                    } => Some((bytes, token)),
                    _ => None,
                })
                .expect("premise: Core already issued a delivery write");
            assert_eq!(
                bytes,
                match beat {
                    0 => b"BOT".to_vec(),
                    1 => b" ".to_vec(),
                    2 => vec![0x7f],
                    _ => b"\r".to_vec(),
                }
            );
            let mut writes = VecDeque::from([PendingWrite {
                bytes,
                offset: 0,
                delivery: Some((1, token)),
            }]);
            let fence = Arc::new(Mutex::new(0));
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut h = SessionHandle::new(tx);
            h.raw_fence = Some(fence.clone());
            h.raw(b"x".to_vec()).unwrap();
            // Core still thinks the original body may be written. raw_fence must independently stop it.
            assert!(core.delivery_write_allowed(1));
            write_ready(&p.master, &core, &fence, 0, &mut writes).unwrap();
            assert_eq!(
                writes.len(),
                1,
                "beat {beat}: must not complete the queued syscall"
            );
            assert_eq!(
                writes.front().unwrap().offset,
                0,
                "beat {beat}: no bytes written"
            );
            let Command::Raw(raw) = rx.try_recv().unwrap() else {
                panic!("raw must be enqueued")
            };
            core.raw(&raw);
            write_ready(&p.master, &core, &fence, 1, &mut writes).unwrap();
            assert!(
                writes.is_empty(),
                "the cancelled delivery write is discarded, not retried"
            );
            // A known-normal counterpart proves this PTY can accept writes through the same function.
            writes.push_back(PendingWrite {
                bytes: b"ok".to_vec(),
                offset: 0,
                delivery: None,
            });
            write_ready(&p.master, &core, &fence, 1, &mut writes).unwrap();
            assert!(writes.is_empty(), "ordinary writes remain available");
            stop(p).await;
        }
    }

    #[tokio::test]
    async fn delivery_completion_requires_the_last_actual_byte_and_survives_no_cancellation() {
        let (_dir, p) = readable_output(b"").await;
        let now = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            now,
            vec![PendingText {
                id: 1,
                subject: Subject::Brief,
                text: "x".repeat(5000),
            }],
            None,
        );
        core.on_output(&input_box(""), now);
        core.take_actions();
        core.on_tick(now + Duration::from_millis(300));
        let actions = core.take_actions();
        let Action::DeliveryWrite {
            id,
            token,
            ref bytes,
        } = actions[0]
        else {
            panic!("body");
        };
        let mut writes = VecDeque::from([PendingWrite {
            bytes: bytes.clone(),
            offset: 0,
            delivery: Some((id, token)),
        }]);
        let fence = Mutex::new(0);
        let completed = write_ready(&p.master, &core, &fence, 0, &mut writes).unwrap();
        assert_eq!(
            completed,
            Some(WriteProgress {
                id,
                token,
                complete: false
            }),
            "one bounded syscall is progress, not completion for 5000 bytes"
        );
        assert!(writes[0].offset > 0 && writes[0].offset < bytes.len());
        core.on_output(&input_box("x"), now + Duration::from_secs(1));
        assert!(
            self::writes(&core.take_actions()).is_empty(),
            "partial body cannot enqueue submit"
        );
        core.raw(b"z");
        assert_eq!(
            write_ready(&p.master, &core, &fence, 0, &mut writes).unwrap(),
            None
        );
        assert!(
            writes.is_empty(),
            "cancelled partial write has no completion"
        );
        // Full positive writes return the exact token once, leaving the caller to notify Core.
        let mut fresh = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            now,
            vec![PendingText {
                id: 2,
                subject: Subject::Brief,
                text: "ok".into(),
            }],
            None,
        );
        fresh.on_output(&input_box(""), now);
        fresh.take_actions();
        fresh.on_tick(now + Duration::from_millis(300));
        let actions = fresh.take_actions();
        let Action::DeliveryWrite {
            id,
            token,
            ref bytes,
        } = actions[0]
        else {
            panic!("body");
        };
        let mut writes = VecDeque::from([PendingWrite {
            bytes: bytes.clone(),
            offset: 0,
            delivery: Some((id, token)),
        }]);
        assert_eq!(
            write_ready(&p.master, &fresh, &fence, 0, &mut writes).unwrap(),
            Some(WriteProgress {
                id,
                token,
                complete: true
            })
        );
        assert_eq!(
            write_ready(&p.master, &fresh, &fence, 0, &mut writes).unwrap(),
            None
        );
        stop(p).await;
    }

    #[tokio::test]
    async fn a_real_would_block_write_expires_without_completion_or_submit() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let p = spawn_pty(&SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "stty raw -echo && printf ready > '{}' && sleep 30",
                    ready.display()
                ),
            ],
            env: [("PATH".into(), "/usr/bin:/bin".into())].into(),
            cwd: dir.path().to_path_buf(),
            cols: 80,
            lines: 25,
        })
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("synthetic slave must be raw before testing backpressure");
        let now = Instant::now();
        let mut config = CoreConfig::default();
        config.delivery.verify_window = Duration::from_millis(50);
        let mut core = Core::new(
            config,
            ScreenConfig::default(),
            now,
            vec![PendingText {
                id: 1,
                subject: Subject::Brief,
                text: "x".repeat(1_000_000),
            }],
            None,
        );
        core.on_output(&input_box(""), now);
        core.take_actions();
        let written_at = now + Duration::from_millis(300);
        core.on_tick(written_at);
        let actions = core.take_actions();
        let Action::DeliveryWrite {
            id,
            token,
            ref bytes,
        } = actions[0]
        else {
            panic!("body");
        };
        let mut writes = VecDeque::from([PendingWrite {
            bytes: bytes.clone(),
            offset: 0,
            delivery: Some((id, token)),
        }]);
        let fence = Mutex::new(0);
        let mut blocked = false;
        for _ in 0..256 {
            match write_ready(&p.master, &core, &fence, 0, &mut writes) {
                Ok(Some(progress)) => {
                    assert!(!progress.complete, "noread slave cannot complete this body");
                    core.delivery_write_progress(progress.id, progress.token, written_at);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    blocked = true;
                    break;
                }
                other => panic!("unexpected writer result: {other:?}"),
            }
        }
        assert!(blocked, "bounded test must actually encounter WouldBlock");
        let offset = writes[0].offset;
        assert!(offset > 0 && offset < bytes.len());
        core.on_tick(written_at + Duration::from_millis(51));
        let terminal = core.take_actions();
        assert!(terminal.iter().any(|a| matches!(
            a,
            Action::Event(SessionEvent::Delivery {
                outcome: crate::session::DeliveryOutcome::Undelivered(_),
                ..
            })
        )));
        assert!(
            self::writes(&terminal).is_empty(),
            "timeout cannot enqueue submit"
        );
        assert_eq!(
            write_ready(&p.master, &core, &fence, 0, &mut writes).unwrap(),
            None
        );
        assert!(
            writes.is_empty(),
            "timed out partial body is dropped, never completed"
        );
        stop(p).await;
    }

    fn complete_actions(core: &mut Core, actions: &[Action], now: Instant) {
        for action in actions {
            if let Action::DeliveryWrite { id, token, .. } = action {
                core.delivery_write_completed(*id, *token, now);
            }
        }
    }

    fn input_box(text: &str) -> Vec<u8> {
        format!(
            "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {text}\r\n────────────────────\r\n\x1b[2m ? for shortcuts\x1b[22m"
        )
        .into_bytes()
    }

    fn held_delivery() -> (Core, Instant) {
        let now = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            now,
            vec![],
            None,
        );
        core.on_output(&input_box(""), now);
        core.raw(b"x");
        core.send(1, "BOT", now);
        core.take_actions();
        (core, now)
    }

    fn assert_resumed_once(core: &mut Core, now: Instant, mut log: Vec<Action>) {
        core.on_tick(now + Duration::from_secs(1));
        log.extend(core.take_actions());
        core.on_tick(now + Duration::from_millis(1100));
        log.extend(core.take_actions());
        assert!(
            log.contains(&Action::Event(SessionEvent::InputDraft(None))),
            "the fresh clear must release the draft hold: {log:?}"
        );
        assert_eq!(
            writes(&log),
            vec![b"BOT".as_slice()],
            "one body, no cleanup or extra repaint required: {log:?}"
        );
        assert!(core.delivery_write_allowed(1));
    }

    /// Same successful drain_ready + on_tick call path as BOTH production timer branches.
    /// Actual PTY bytes, not on_output or an assumed Foreign::Shown test premise.
    #[tokio::test]
    async fn timer_drained_text_establishes_shown_before_an_ordinary_clear() {
        use crate::session::HeldDraft;
        let (mut core, now) = held_delivery();
        let (_dir, p) = readable_output(&input_box("x")).await;
        assert!(drain_ready(&p.master, &mut core, None, DRAIN_CHUNKS));
        assert!(
            core.take_actions().is_empty(),
            "ingestion alone stays inert"
        );
        core.on_tick(now + Duration::from_millis(100));
        let shown = core.take_actions();
        assert!(
            shown.contains(&Action::Event(SessionEvent::InputDraft(Some(
                HeldDraft::Text("x".into())
            )))),
            "timer-drained Text must establish the human draft: {shown:?}"
        );
        assert!(writes(&shown).is_empty());
        core.raw(b"\x15");
        core.take_actions();
        core.on_output(&input_box(""), now + Duration::from_millis(200));
        let log = core.take_actions();
        assert_resumed_once(&mut core, now, log);
        stop(p).await;
    }

    #[tokio::test]
    async fn timer_drained_fresh_empty_after_explicit_clear_resumes_without_repaint() {
        let (mut core, now) = held_delivery();
        core.on_output(&input_box("x"), now + Duration::from_millis(100));
        core.take_actions();
        core.raw(b"\x15");
        core.take_actions();
        let (_dir, p) = readable_output(&input_box("")).await;
        assert!(drain_ready(&p.master, &mut core, None, DRAIN_CHUNKS));
        assert!(
            core.take_actions().is_empty(),
            "ingestion alone stays inert"
        );
        core.on_tick(now + Duration::from_millis(200));
        let log = core.take_actions();
        assert_resumed_once(&mut core, now, log);
        stop(p).await;
    }

    /// Known-normal counterpart: identical lifecycle, finalized by the ordinary read path.
    #[tokio::test]
    async fn ordinary_drained_fresh_empty_positive_control_resumes_once() {
        let (mut core, now) = held_delivery();
        core.on_output(&input_box("x"), now + Duration::from_millis(100));
        core.take_actions();
        core.raw(b"\x15");
        core.take_actions();
        let (_dir, p) = readable_output(&input_box("")).await;
        let mut first = [0u8; PTY_READ_SIZE];
        let n = p.master.read(&mut first).await.unwrap();
        assert!(drain_ready(
            &p.master,
            &mut core,
            Some(&first[..n]),
            DRAIN_CHUNKS
        ));
        core.after_output_drain(now + Duration::from_millis(200));
        let log = core.take_actions();
        assert_resumed_once(&mut core, now, log);
        stop(p).await;
    }

    /// A cap is not completion, even when that chunk already parses as fresh Empty.
    #[tokio::test]
    async fn capped_timer_drain_keeps_explicit_clear_held_until_completion() {
        let (mut core, now) = held_delivery();
        core.on_output(&input_box("x"), now + Duration::from_millis(100));
        core.take_actions();
        core.raw(b"\x15");
        core.take_actions();
        let (_dir, p) = readable_output(&input_box("")).await;
        let complete = drain_ready(&p.master, &mut core, None, 1);
        if complete {
            core.on_tick(now + Duration::from_millis(200));
        }
        assert!(!complete, "the one-chunk budget cannot confirm WouldBlock");
        assert!(core.screen_text(25).contains("❯\n"), "Empty already parsed");
        assert!(core.take_actions().is_empty());
        assert!(
            !core.delivery_write_allowed(1),
            "cap must not release Unknown"
        );
        assert!(drain_ready(&p.master, &mut core, None, DRAIN_CHUNKS));
        core.on_tick(now + Duration::from_millis(200));
        let log = core.take_actions();
        assert_resumed_once(&mut core, now, log);
        stop(p).await;
    }

    /// R2 必须钉住的边界：chunk 1 保持旧框；chunk 2 才露出新框。键在 chunk 1 之前已经到点。
    /// 第一次只给一块预算，直接检查 chunk 1 后无写键；再排完第二块，回 Conflict 且仍一键未写。
    #[tokio::test]
    async fn a_due_answer_waits_for_the_second_pty_read() {
        let (mut core, before) = due_answer();
        let prefix = b"\x1b[1;1H";
        let mut output = prefix.to_vec();
        output.extend(menu("Replace the old decision?", 0));
        let (_dir, p) = readable_output(&output).await;
        let mut first = [0u8; 6];
        let n = p.master.read(&mut first).await.unwrap();
        assert_eq!(&first[..n], prefix, "第一块只改光标，旧框还没换");
        assert!(!drain_ready(&p.master, &mut core, Some(&first[..n]), 1));
        let mut log = core.take_actions();
        assert!(writes(&log).is_empty(), "chunk 1 后不按到点键：{log:?}");
        assert_eq!(shown(&core), before, "第一块仍是旧框和旧高亮");
        assert!(drain_ready(&p.master, &mut core, None, DRAIN_CHUNKS));
        core.after_output_drain(Instant::now());
        log.extend(core.take_actions());
        assert!(writes(&log).is_empty(), "排完第二块也不能按旧答：{log:?}");
        assert!(
            matches!(
                log.iter().find(|a| matches!(a, Action::AnswerDone { .. })),
                Some(Action::AnswerDone {
                    id: 1,
                    outcome: Some(AnswerOutcome::Conflict { now: Some(d) })
                }) if d.prompt == "Replace the old decision?" && d.selected == Some(0)
            ),
            "{log:?}"
        );
        stop(p).await;
    }

    /// 普通 read 分支的 first + 后续 try_read 走同一个排空闸；指纹相同、只有第二块挪了高亮。
    #[tokio::test]
    async fn an_ordinary_read_drains_the_changed_highlight_before_a_due_answer() {
        let (mut core, before) = due_answer();
        let prefix = b"\x1b[1;1H";
        let mut output = prefix.to_vec();
        output.extend(menu("Proceed?", 2));
        let (_dir, p) = readable_output(&output).await;
        let mut first = [0u8; 6];
        let n = p.master.read(&mut first).await.unwrap();
        assert_eq!(&first[..n], prefix);
        // 与 run() 的普通 read 分支完全相同，第二次 PTY read 才带来变动。
        assert!(drain_ready(
            &p.master,
            &mut core,
            Some(&first[..n]),
            DRAIN_CHUNKS
        ));
        core.after_output_drain(Instant::now());
        let log = core.take_actions();
        assert!(writes(&log).is_empty(), "不能在第一块之后按 Enter：{log:?}");
        assert!(
            matches!(
                log.iter().find(|a| matches!(a, Action::AnswerDone { .. })),
                Some(Action::AnswerDone {
                    id: 1,
                    outcome: Some(AnswerOutcome::Conflict { now: Some(d) })
                }) if d.fingerprint() == before.fingerprint() && d.selected == Some(2)
            ),
            "{log:?}"
        );
        stop(p).await;
    }

    /// tick / Command::Answer 的无 first 路也不能在预算触顶后 flush，即使刚读完的框看起来完全对。
    /// 下一轮真正读到 WouldBlock 才放行，避免用「恰好读完」的猜测代替排空事实。
    #[tokio::test]
    async fn a_capped_drain_defers_a_due_answer_until_would_block() {
        let (mut core, before) = due_answer();
        let (_dir, p) = readable_output(b"\x1b[1;1H").await;
        assert!(!drain_ready(&p.master, &mut core, None, 1));
        let mut log = core.take_actions();
        assert!(writes(&log).is_empty(), "预算触顶不是排空：{log:?}");
        assert_eq!(shown(&core), before);
        assert!(drain_ready(&p.master, &mut core, None, DRAIN_CHUNKS));
        core.after_output_drain(Instant::now());
        log.extend(core.take_actions());
        assert_eq!(
            writes(&log),
            vec![b"\r".as_slice()],
            "确认排空后才按：{log:?}"
        );
        stop(p).await;
    }
}
