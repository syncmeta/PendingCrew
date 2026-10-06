//! 一个 claude 会话的 tokio 任务，和其余部分唯一的接口 [`SessionHandle`]（spec §13 Q12）。
//!
//! 任务拥有 PTY 主端、子进程、[`Core`]；命令从句柄进，事件从 `mpsc` 出。
//! 节拍：每块 PTY 输出一拍、0.6s 定时一拍、`Core::next_wakeup` 到点一拍。
//! 写 PTY 走单独的写任务：子进程不读输入时写会阻塞，不能卡住读（否则双方互等）。
//!
//! 以后执行器拆成独立进程，只把 `SessionHandle` 的命令与事件换成线协议。
use std::collections::BTreeSet;
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
use crate::registry::{ProbeResult, ProcessRegistry, probe};
use crate::resume::{AgentIdLedger, decide_resume};
use crate::session::{
    Action, BRIEF_DELIVERY_ID, Core, CoreConfig, DerivedState, ExitStatusKind, PendingText,
    SessionEvent,
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
    /// 这个 claude 会话号已经在本进程里跑着：再起一次会删掉它的启动目录、两个进程写同一份对话。
    AlreadyRunning(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ledger(e) => write!(f, "会话号账本读写失败：{e}"),
            Self::Files(e) => write!(f, "启动文件写不出来：{e}"),
            Self::Spawn { health, .. } => write!(f, "{}", health.detail),
            Self::Registry(e) => write!(f, "进程登记表写不进去：{e}"),
            Self::AlreadyRunning(id) => {
                write!(f, "claude 会话 {id} 已经在跑，不能同时再起一个")
            }
        }
    }
}

impl std::error::Error for StartError {}

/// 会话任务已经结束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

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

enum Command {
    Send {
        id: u64,
        text: String,
    },
    Raw(Vec<u8>),
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
        reply: oneshot::Sender<Snapshot>,
    },
    Status {
        reply: oneshot::Sender<SessionStatus>,
    },
}

/// 窄接口：启动之外的一切（输入 / 停止 / 状态 / 屏幕文本 / 快照）。
/// 可克隆；**所有句柄都 drop 了 = 停掉会话**（没人能再控制它）。
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl SessionHandle {
    /// 投一段正文（排队，回读屏幕确认提交）。返回投递编号，结果见 `SessionEvent::Delivery`。
    pub fn send(&self, text: impl Into<String>) -> Result<u64, Closed> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.cmd(Command::Send {
            id,
            text: text.into(),
        })?;
        Ok(id)
    }

    /// 原样写字节（菜单按键、Esc 打断）。
    pub fn raw(&self, bytes: impl Into<Vec<u8>>) -> Result<(), Closed> {
        self.cmd(Command::Raw(bytes.into()))
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

    pub async fn snapshot(&self) -> Result<Snapshot, Closed> {
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
        running,
        cmd_rx,
        ev_tx,
        auth_tx,
        auth_rx,
    }));
    Ok((
        SessionHandle {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
        },
        ev_rx,
    ))
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

/// 本进程里正在跑的 claude 会话号。
static LIVE_AGENT_IDS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// 占着一个会话号；drop 时放开。
struct LiveId(String);

impl LiveId {
    fn claim(id: &str) -> Result<Self, StartError> {
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

    /// 进程已收尸：停掉附属任务、删启动文件、放开会话号（按这个顺序）。
    fn release(&mut self) {
        self.shutdown_helpers();
        self.files = None;
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
        tokio::task::spawn_blocking(move || ledger.record(&id))
            .await
            .map_err(|e| StartError::Ledger(io::Error::other(e)))?
            .map_err(StartError::Ledger)?;
    }
    let (args, files) = prepare_claude(&start.spec, agent.clone(), &config.runtime_dir)
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
        // 只登记自成进程组的（setsid 后 pgid == pid）：回收时按组杀，组号不对会杀错人。
        let recorded = match probe(proc.pid as i32) {
            ProbeResult::Found(identity) if identity.pgid == proc.pid as i32 => {
                registry.record(&start.session_id, identity)
            }
            ProbeResult::Found(identity) => Err(io::Error::other(format!(
                "刚起的进程 {} 不是自己进程组的组长（pgid {}），不登记",
                proc.pid, identity.pgid
            ))),
            other => Err(io::Error::other(format!("读不到刚起的进程身份：{other:?}"))),
        };
        if let Err(e) = recorded {
            kill_group(proc.pid);
            let _ = child.wait().await;
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
    running: Running,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<SessionEvent>,
    auth_tx: mpsc::UnboundedSender<AuthMsg>,
    auth_rx: mpsc::UnboundedReceiver<AuthMsg>,
}

fn kill_group(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process, kill_process_group};
    if let Some(p) = Pid::from_raw(pid as i32)
        && kill_process_group(p, Signal::KILL).is_err()
    {
        let _ = kill_process(p, Signal::KILL);
    }
}

fn terminate(pid: u32) {
    use rustix::process::{Pid, Signal, kill_process};
    if let Some(p) = Pid::from_raw(pid as i32) {
        let _ = kill_process(p, Signal::TERM);
    }
}

fn decode(status: io::Result<std::process::ExitStatus>) -> ExitStatusKind {
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
                    if handle(&mut t, c) {
                        stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                    }
                }
                None => {
                    commands_open = false;
                    t.core.stop(Instant::now());
                    terminate(t.running.pid);
                    stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                }
            },
            _ = tick.tick() => t.core.on_tick(Instant::now()),
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
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
        if refused && !retried {
            retried = true;
            // 被拒的那个进程：已经自己退了就用它的退出状态，还活着就杀掉收尸。
            let old_status = match exit.take() {
                Some(status) => status,
                None => {
                    kill_group(t.running.pid);
                    decode(t.running.child.wait().await)
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
                    finish(&mut t, old_status);
                    return;
                }
            }
        }
        if let Some(status) = exit {
            finish(&mut t, status);
            return;
        }
    }
}

/// 进程已收尸（退出由 `Child::wait` 决定，不看主端 EOF：主端/从端漏进别的进程时 EOF 可能
/// 永远不来）。先收拾——登记表、启动文件、会话号——再把 `Core` 最后的事件（`Exited` 在末尾）
/// 发出去：调用方见到 `Exited` 就能马上用同一个会话号重起。
fn finish(t: &mut Task, status: ExitStatusKind) {
    t.core.on_exit(status, Instant::now());
    let actions = t.core.take_actions();
    t.running.release();
    if let Some(registry) = &t.config.registry {
        let _ = registry.forget(&t.start.session_id);
    }
    dispatch(t, actions);
}

/// 处理一条命令；返回 true = 开始停止（要起 grace 计时）。
fn handle(t: &mut Task, cmd: Command) -> bool {
    let now = Instant::now();
    match cmd {
        Command::Send { id, text } => t.core.send(id, &text, now),
        Command::Raw(bytes) => t.core.raw(&bytes),
        Command::Resize { cols, lines } => t.core.resize(cols, lines),
        Command::Stop => {
            if t.core.is_running() {
                t.core.stop(now);
                terminate(t.running.pid);
                return true;
            }
        }
        Command::ClearQuotaHealth => t.core.clear_quota_health(now),
        Command::ScreenText { max_lines, reply } => {
            let _ = reply.send(t.core.screen_text(max_lines));
        }
        Command::Snapshot { reply } => {
            let _ = reply.send(t.core.snapshot());
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
        }
    }
    refused
}

/// claude 拒绝了 `--resume`：换新号（先落账）重起一次，没投出去的正文搬过去。
async fn respawn(t: &mut Task) -> Result<(), StartError> {
    let pending = t.core.take_pending();
    let agent = AgentSessionArg::New(new_agent_session_id());
    let running = launch(&t.start, &t.config, agent, &t.auth_tx).await?;
    let mut old = std::mem::replace(&mut t.running, running);
    old.shutdown_helpers();
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
