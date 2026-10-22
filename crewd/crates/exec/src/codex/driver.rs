//! 一个 codex 会话的 tokio 任务：拥有 app-server 子进程、三根管子和 [`CodexCore`]，
//! 对外只交出与 claude 相同的 [`SessionHandle`] + `SessionEvent` 事件流。
//!
//! `start_codex` 等到握手完成、thread id 落账（第一条事件 `Spawned` 已排进事件流）才返回；
//! 握手失败 / 超时 / 落账失败都在这里变成 `StartError`，进程已杀掉。之后的节拍：每行 stdout、
//! 0.6s 定时、`CodexCore::next_wakeup`（请求期限）到点。
//!
//! 与 claude 驱动（`crate::driver`）同一套进程规矩：
//! - 停止只开始一次（`stop()` 连按、句柄随后全丢都不重发 SIGTERM、不推后 SIGKILL 的期限）。
//! - 进程退出（任何原因）后先把 stdout 剩下的行读完（最多 500ms）、等 stderr 读完（最多 500ms，
//!   握手失败的说明要带上它的尾巴），然后**先收拾再发 `Exited`**：整组补一刀 SIGKILL（只发组信号，
//!   过 pgid 安全闸）、停附属任务、放开 thread id、清登记表——codex 起的工具子命令不留成孤儿，
//!   调用方见到 `Exited` 就能马上用同一个 thread id 重起。
//! - 登记表、账本的文件读写都在阻塞线程池里做；杀掉之后等收尸有上限。
//!
//! 同一个 thread id 在本进程里只能跑一个（与 claude 共用 `driver::LiveId` 那张表）：账里记的号
//! 在起进程之前就占住，新开 thread 的号在 `Spawned` 时占住。
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};

use crate::codex::proto::{self, ThreadOptions};
use crate::codex::session::{
    CodexAction, CodexCore, CodexCoreConfig, Opening, PendingTurn, StartFailure, launch_health,
};
use crate::codex::transport::{
    CodexProcess, CodexProcessSpec, Line, spawn_codex, strip_billing_keys,
};
use crate::delivery::Subject;
use crate::driver::{
    Command, ControlOutcome, LiveId, REAP_LIMIT, SessionHandle, SessionStatus, StartError,
    UntrustedContext, blocking, decode, kill_group, kill_group_only, terminate,
};
use crate::env::build_env_from_process;
use crate::health::LaunchVerdict;
use crate::launch::{CommsSpec, HelperCommands, LaunchFiles, write_token_file};
use crate::registry::{ProbeResult, ProcessRegistry, probe};
use crate::resume::{AgentIdLedger, valid_agent_session_id};
use crate::session::{AnswerOutcome, BRIEF_DELIVERY_ID, ExitStatusKind, SessionEvent};

/// 一次 codex 启动要的全部输入（调用方给的）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexSpec {
    pub workdir: PathBuf,
    pub title: Option<String>,
    pub initial_settings: Option<crewd_proto::codex::CodexSettingsPatch>,
    pub strict_resume: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)` → `serviceTier: "fast"`；`None` 不传（用 codex 自己的配置）。
    pub fast_mode: Option<bool>,
    /// 世界观（M3 渲染），走 `thread/start.developerInstructions`。
    pub developer_instructions: Option<String>,
    /// 开场任务（就绪后的第一个回合）。
    pub initial_prompt: Option<String>,
    /// 开场任务附带的不可信上下文（首轮白板）。
    pub initial_context: UntrustedContext,
    /// crew 通讯：挂到 `config.mcp_servers.crew`。codex 没有 hook，`hooks` 不看。
    pub comms: Option<CommsSpec>,
    /// 额外环境变量。过白名单的禁用前缀，再剥掉 [`crate::codex::transport::BILLING_KEYS`]。
    pub additional_env: BTreeMap<String, String>,
}

/// 起一个 codex 会话要的东西。
pub struct CodexStart {
    /// crewd 的 session id（登记表的键）。
    pub session_id: String,
    /// codex 可执行文件（`AgentPaths::resolve("codex")`）。
    pub program: PathBuf,
    /// 子进程 PATH（`AgentPaths::child_path`；codex 的 npm 版要靠它找到 node）。
    pub child_path: String,
    pub spec: CodexSpec,
    pub ledger: Arc<dyn AgentIdLedger>,
}

/// 驱动层参数。
#[derive(Clone)]
pub struct CodexConfig {
    pub core: CodexCoreConfig,
    /// 定时节拍。
    pub tick: Duration,
    /// SIGTERM 之后多久升级成 `killpg(SIGKILL)`。
    pub stop_grace: Duration,
    /// 孤儿登记表；`None` = 不登记。
    pub registry: Option<Arc<ProcessRegistry>>,
    /// `initialize.clientInfo.version`。
    pub client_version: String,
    /// 与 Claude 相同：成功投递在广播事件前登记因果来源。
    pub delivery_observer: Option<Arc<dyn crate::driver::DeliverySuccessObserver>>,
    /// 启动临时文件（token 文件）的运行目录：`<runtime_dir>/codex-<session_id>/`。
    /// 与 claude 的 `DriverConfig.runtime_dir` 同一处（M3c 起由会话给数据目录下的运行目录）。
    pub runtime_dir: PathBuf,
}

impl std::fmt::Debug for CodexConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexConfig")
            .field("core", &self.core)
            .field("tick", &self.tick)
            .field("stop_grace", &self.stop_grace)
            .field("registry", &self.registry)
            .field("client_version", &self.client_version)
            .field("runtime_dir", &self.runtime_dir)
            .field(
                "delivery_observer",
                &self.delivery_observer.as_ref().map(|_| "<opaque>"),
            )
            .finish()
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            core: CodexCoreConfig::default(),
            tick: Duration::from_millis(600),
            stop_grace: Duration::from_secs(2),
            registry: None,
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            delivery_observer: None,
            runtime_dir: std::env::temp_dir().join("crewd-codex"),
        }
    }
}

/// `codex app-server` 的参数（就这一个子命令；首条 prompt、世界观都走 stdin）。
pub fn codex_argv() -> Vec<String> {
    vec!["app-server".to_string()]
}

/// 起一个 codex 会话。返回时握手已完成、thread id 已落账，事件流的第一条是 `Spawned`。
pub async fn start_codex(
    start: CodexStart,
    config: CodexConfig,
) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError> {
    let ledger = start.ledger.clone();
    let recorded = tokio::task::spawn_blocking(move || ledger.recorded())
        .await
        .map_err(|e| StartError::Ledger(io::Error::other(e)))?
        .map_err(StartError::Ledger)?;
    // 要续的那条先占住：同一个 thread 被两个 app-server 同时接着写，对话会乱。
    let live = match recorded
        .as_deref()
        .map(str::trim)
        .filter(|id| valid_agent_session_id(id))
    {
        Some(id) => Some(LiveId::claim(id)?),
        None => None,
    };
    // token 文件：0600、放在这次启动自己的 0700 目录里（目录名带每次启动唯一的后缀：同一个 crewd
    // session id 立刻重起时，新旧目录互不相干）。目录在 `release`（发 `Exited` 之前）里删掉。
    let launch = match &start.spec.comms {
        Some(c) => {
            if !valid_agent_session_id(&start.session_id) {
                return Err(StartError::Files(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid session id {:?}", start.session_id),
                )));
            }
            let name = format!(
                "codex-{}-{}",
                start.session_id,
                uuid::Uuid::new_v4().simple()
            );
            let (runtime_dir, token) = (config.runtime_dir.clone(), c.token.clone());
            let made = blocking(move || {
                let files = LaunchFiles::create(&runtime_dir, &name)?;
                let token_file = write_token_file(files.dir(), &token)?;
                Ok((files, token_file))
            })
            .await
            .map_err(StartError::Files)?;
            Some(made)
        }
        None => None,
    };
    let (files, token_file) = match launch {
        Some((f, t)) => (Some(f), Some(t)),
        None => (None, None),
    };
    let mut env = build_env_from_process(&start.child_path, &start.spec.additional_env);
    strip_billing_keys(&mut env);
    let spec = CodexProcessSpec {
        program: start.program.clone(),
        args: codex_argv(),
        env,
        cwd: start.spec.workdir.clone(),
    };
    let mut proc = spawn_codex(&spec).map_err(|source| StartError::Spawn {
        health: launch_health(
            LaunchVerdict::SpawnFailed,
            &format!("{}: {source}", start.program.display()),
        ),
        source,
    })?;
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
                // Attempt durable removal; if persistence still fails, the registry retains
                // the restrictive original-birth obligation. Never erase it in memory alone.
                let _ = registry.forget(&session_id);
            }
            r
        })
        .await;
        if let Err(e) = recorded {
            kill_group(proc.pid);
            let e = match reap_killed(&mut proc.child).await {
                Ok(_) => e,
                Err(reap) => io::Error::other(format!("{e}；杀掉刚起的进程后{reap}")),
            };
            return Err(StartError::Registry(e));
        }
    }
    let provider_fence = proc.write_fence.clone();
    let unix_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let opening = Opening {
        pid: proc.pid,
        cwd: start.spec.workdir.clone(),
        options: thread_options(
            &start.spec,
            start.spec.comms.as_ref().zip(token_file.as_deref()),
        ),
        recorded,
        client_version: config.client_version.clone(),
        brief: start
            .spec
            .initial_prompt
            .clone()
            .filter(|p| !p.is_empty())
            .map(|text| PendingTurn {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                text,
                context: start.spec.initial_context.clone(),
            }),
        unix_now,
    };
    let core = CodexCore::new(opening, config.core, Instant::now());
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (tx, cmd_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(run(Task {
        start,
        config,
        core,
        proc,
        cmd_rx,
        ev_tx,
        ready: Some(ready_tx),
        controls: HashMap::new(),
        native_controls: HashMap::new(),
        answers: HashMap::new(),
        next_ticket: 0,
        live,
        files,
    }));
    match ready_rx.await {
        Ok(Ok(())) => Ok((
            SessionHandle::new(tx).with_provider_fence(provider_fence),
            ev_rx,
        )),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(StartError::Handshake {
            health: launch_health(LaunchVerdict::DiedSilently, "会话任务提前结束"),
        }),
    }
}

/// `CodexSpec` → `thread/start` 选项（MCP 指向 helper，身份只有数据目录 / session / token 文件）。
/// `mcp` = 通讯配置 + 已写好的 token 文件，两样一起给（没有 token 文件就没有 MCP）。
pub fn thread_options(spec: &CodexSpec, mcp: Option<(&CommsSpec, &Path)>) -> ThreadOptions {
    ThreadOptions {
        title: spec.title.clone(),
        initial_settings: spec.initial_settings.clone(),
        strict_resume: spec.strict_resume,
        model: spec.model.clone(),
        effort: spec.effort.clone(),
        fast: spec.fast_mode,
        developer_instructions: spec.developer_instructions.clone(),
        mcp_servers: mcp.map(|(c, token_file)| {
            proto::mcp_servers(&c.helper, &HelperCommands(c).mcp_args(token_file))
        }),
    }
}

struct Task {
    start: CodexStart,
    config: CodexConfig,
    core: CodexCore,
    proc: CodexProcess,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<SessionEvent>,
    /// 还没告诉 `start_codex` 结果（`Some`）= 还在启动。
    ready: Option<oneshot::Sender<Result<(), StartError>>>,
    controls: HashMap<u64, oneshot::Sender<ControlOutcome>>,
    native_controls: HashMap<u64, oneshot::Sender<super::control::CodexOutcome>>,
    answers: HashMap<u64, oneshot::Sender<AnswerOutcome>>,
    next_ticket: u64,
    /// 占着的 thread id（本进程里同一个 thread 只能跑一个）。
    live: Option<LiveId>,
    /// 启动临时文件（token 文件）：`release` 里（发 `Exited` 之前）在阻塞线程池里删掉。
    files: Option<LaunchFiles>,
}

type RecordMsg = Result<(), String>;

async fn run(mut t: Task) {
    let (record_tx, mut record_rx) = mpsc::unbounded_channel::<RecordMsg>();
    let mut reading = true;
    let mut commands_open = true;
    let mut stop_deadline: Option<tokio::time::Instant> = None;
    // 已经在停：见 [`begin_stop`]。
    let mut stopping = false;
    let mut tick = tokio::time::interval(t.config.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let actions = t.core.take_actions();
        if let Some(err) = execute(&mut t, actions, &record_tx) {
            // 启动失败：杀掉、收尸（有上限）、收拾——**之后**才把错误交给 `start_codex`：
            // 它返回时 thread id 已放开、登记表已清，同一个账本 / session id 立刻能重起。
            kill_group(t.proc.pid);
            if let Err(e) = reap_killed(&mut t.proc.child).await {
                eprintln!(
                    "crewd: session {}: 启动失败后杀掉的 codex 进程 {} {e}",
                    t.start.session_id, t.proc.pid
                );
            }
            release(&mut t).await;
            if let Some(ready) = t.ready.take() {
                let _ = ready.send(Err(err));
            }
            return;
        }
        let wake = t.core.next_wakeup().map(tokio::time::Instant::from_std);
        tokio::select! {
            line = t.proc.stdout.next_line(), if reading => match line {
                Ok(Some(Line::Text(l))) => t.core.on_line(&l, Instant::now()),
                Ok(Some(Line::TooLong { bytes })) => t.core.on_dropped_line(bytes, Instant::now()),
                _ => reading = false,
            },
            status = t.proc.child.wait() => {
                let status = decode(status);
                drain(&mut t, Duration::from_millis(500)).await;
                let tail = t.proc.stderr.text();
                t.core.on_exit(status, &tail, Instant::now());
                let actions = t.core.take_actions();
                release(&mut t).await;
                let failed = execute(&mut t, actions, &record_tx);
                if let Some(ready) = t.ready.take() {
                    let _ = ready.send(Err(failed.unwrap_or_else(|| StartError::Handshake {
                        health: launch_health(LaunchVerdict::DiedSilently, "会话任务提前结束"),
                    })));
                }
                return;
            }
            cmd = t.cmd_rx.recv(), if commands_open => match cmd {
                Some(c) => {
                    if handle(&mut t, c) {
                        begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                    }
                }
                None => {
                    commands_open = false;
                    begin_stop(&mut t, &mut stopping, &mut stop_deadline);
                }
            },
            _ = tick.tick() => t.core.on_tick(Instant::now()),
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
                t.core.on_tick(Instant::now());
            }
            _ = tokio::time::sleep_until(stop_deadline.unwrap_or_else(tokio::time::Instant::now)), if stop_deadline.is_some() => {
                stop_deadline = None;
                kill_group(t.proc.pid);
            }
            Some(result) = record_rx.recv() => t.core.on_recorded(result, Instant::now()),
        }
    }
}

/// 开始停：只做一次。再叫停（`stop()` 连按、句柄随后全丢）既不重发 SIGTERM，也不把
/// SIGKILL 的期限往后推——否则叫停比 `stop_grace` 还勤时，忽略 SIGTERM 的进程永远杀不掉。
fn begin_stop(t: &mut Task, stopping: &mut bool, stop_deadline: &mut Option<tokio::time::Instant>) {
    if *stopping {
        return;
    }
    *stopping = true;
    t.core.stop(Instant::now());
    terminate(t.proc.pid);
    *stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
}

/// 进程已收尸（或启动失败、已杀掉）：整组补一刀、停掉附属任务、删启动文件、放开 thread id、清登记表
/// （按这个顺序，都在发 `Exited` 之前）。
///
/// 补刀：codex 收到 SIGTERM 自己退了（或崩了），它起的工具子命令还留在组里。setsid 后
/// pgid == pid；组里还有成员时这个号不会被别的进程拿去当 pid，组空了 killpg 只会得到 ESRCH。
/// **只发组信号**（`kill_group_only`，过 pgid 安全闸）：首进程已经收了尸，退回去单杀这个 pid
/// 可能杀到复用了它的进程。
async fn release(t: &mut Task) {
    kill_group_only(t.proc.pid);
    t.proc.writer_task.abort();
    t.proc.stderr_task.abort();
    if let Some(files) = t.files.take() {
        let _ = tokio::task::spawn_blocking(move || drop(files)).await;
    }
    t.live = None;
    if let Some(registry) = t.config.registry.clone() {
        let id = t.start.session_id.clone();
        let _ = blocking(move || registry.forget(&id)).await;
    }
}

/// 收一个**已经发过 SIGKILL** 的进程，最多等 [`REAP_LIMIT`]：超了报错、不挂住（丢下的 `Child`
/// 由 tokio 之后收尸）。没有 PTY：stdout / stderr 是管道，进程退出不等它们被读走。
async fn reap_killed(child: &mut Child) -> io::Result<ExitStatusKind> {
    match tokio::time::timeout(REAP_LIMIT, child.wait()).await {
        Ok(status) => Ok(decode(status)),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{}ms 内没能收尸", REAP_LIMIT.as_millis()),
        )),
    }
}

/// 进程退出后把 stdout 剩下的行读完（最多 `limit`），再等 stderr 读到 EOF（同样最多 `limit`：
/// 握手失败的说明要带上 stderr 的尾巴，而它和 `Child::wait` 谁先到不一定）。工具子命令继承了
/// 管道、一直开着时，两步各自到点就走。
async fn drain(t: &mut Task, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    while let Ok(Ok(Some(line))) =
        tokio::time::timeout_at(deadline, t.proc.stdout.next_line()).await
    {
        if let Line::Text(l) = line {
            t.core.on_line(&l, Instant::now());
        }
    }
    let _ = tokio::time::timeout(limit, &mut t.proc.stderr_task).await;
}

/// 执行核心攒下的动作；返回 `Some` = 启动失败（调用方杀进程、收拾完之后再把这个错误交出去）。
fn execute(
    t: &mut Task,
    actions: Vec<CodexAction>,
    record_tx: &mpsc::UnboundedSender<RecordMsg>,
) -> Option<StartError> {
    for action in actions {
        match action {
            CodexAction::ControlledWrite { line, permit } => {
                let _ = t
                    .proc
                    .writer
                    .send(super::transport::WriterMessage::ControlledLine { line, permit });
            }
            CodexAction::Write(line) => {
                let _ = t.proc.writer.send(line.into());
            }
            CodexAction::Record(thread) => {
                let (ledger, tx) = (t.start.ledger.clone(), record_tx.clone());
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || ledger.record(&thread))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()));
                    let _ = tx.send(result);
                });
            }
            CodexAction::AnswerDone { ticket, outcome } => {
                if let Some(reply) = t.answers.remove(&ticket) {
                    if matches!(outcome, AnswerOutcome::Answered { .. }) {
                        let (written, rx) = oneshot::channel();
                        let queued = t
                            .proc
                            .writer
                            .send(super::transport::WriterMessage::Barrier(written))
                            .is_ok();
                        let deadline = t.config.core.request_timeout;
                        tokio::spawn(async move {
                            let result = if queued
                                && matches!(tokio::time::timeout(deadline, rx).await, Ok(Ok(())))
                            {
                                outcome
                            } else {
                                AnswerOutcome::NeedsTerminal
                            };
                            let _ = reply.send(result);
                        });
                    } else {
                        let _ = reply.send(outcome);
                    }
                }
            }
            CodexAction::CodexDone { ticket, outcome } => {
                if let Some(reply) = t.native_controls.remove(&ticket) {
                    let _ = reply.send(outcome);
                }
            }
            CodexAction::ControlDone { ticket, outcome } => {
                if let Some(reply) = t.controls.remove(&ticket) {
                    let _ = reply.send(outcome);
                }
            }
            CodexAction::Failed(failure) => {
                let err = match failure {
                    StartFailure::Launch(health) => StartError::Handshake { health },
                    StartFailure::Ledger(e) => StartError::Ledger(io::Error::other(e)),
                };
                return Some(err);
            }
            CodexAction::Event(e) => {
                if let SessionEvent::Spawned {
                    agent_session_id, ..
                } = &e
                    && t.live.as_ref().map(LiveId::id) != Some(agent_session_id.as_str())
                {
                    // 新开的 thread（含续跑被拒后退回的那条）：占住它的号。新号不会撞，撞了也只是不占。
                    t.live = LiveId::claim(agent_session_id).ok();
                }
                if matches!(e, SessionEvent::Spawned { .. })
                    && let Some(ready) = t.ready.take()
                {
                    crate::driver::emit_session_event(
                        t.config.delivery_observer.as_ref(),
                        &t.ev_tx,
                        e,
                    );
                    let _ = ready.send(Ok(()));
                    continue;
                }
                crate::driver::emit_session_event(t.config.delivery_observer.as_ref(), &t.ev_tx, e);
            }
        }
    }
    None
}

/// 处理一条命令；返回 true = 要停（由 [`begin_stop`] 去重）。
fn handle(t: &mut Task, cmd: Command) -> bool {
    let now = Instant::now();
    match cmd {
        Command::Send { id, text, context } => t.core.send(id, &text, context, now),
        Command::Raw(bytes) => t.core.raw(&bytes, now),
        Command::RawConfirmed { reply, .. } => {
            let _ = reply.send(crate::driver::RawWriteOutcome::Unsupported);
        }
        Command::Interrupt => t.core.interrupt(now),
        // 没有终端，尺寸无意义。
        Command::Resize { .. } => {}
        Command::ResizeConfirmed { reply, .. } => {
            let _ = reply.send(crate::driver::ResizeOutcome::Unsupported);
        }
        Command::Stop => return t.core.is_running(),
        Command::ClearQuotaHealth => t.core.clear_quota_health(now),
        Command::ScreenText { max_lines, reply } => {
            let _ = reply.send(t.core.transcript_text(max_lines));
        }
        Command::ManualHold { reply } => {
            let _ = reply.send(false);
        }
        Command::Snapshot { reply } => {
            let _ = reply.send(None);
        }
        Command::TerminalCut { reply } => {
            let _ = reply.send(None);
        }
        Command::AnswerChoice {
            expect,
            choice,
            reply,
        } => {
            let ticket = t.next_ticket;
            t.next_ticket += 1;
            t.answers.insert(ticket, reply);
            let outcome = t.core.answer_choice(&expect, choice, now);
            t.core.report_answer(ticket, outcome);
        }
        Command::RespondRequest {
            expect,
            response,
            reply,
        } => {
            let ticket = t.next_ticket;
            t.next_ticket += 1;
            t.answers.insert(ticket, reply);
            let outcome = t.core.respond_request(&expect, response, now);
            t.core.report_answer(ticket, outcome);
        }
        Command::RunnerRequest { reply } => {
            let _ = reply.send(t.core.runner_request());
        }
        Command::Status { reply } => {
            let _ = reply.send(SessionStatus {
                state: t.core.state(),
                working: t.core.is_working(),
                health: t.core.health().cloned(),
                pending_decision: t.core.pending_decision().cloned(),
                pid: t.proc.pid,
                agent_session_id: t.core.thread_id().unwrap_or_default().to_string(),
            });
        }
        Command::Codex { op, permit, reply } => {
            let ticket = t.next_ticket;
            t.next_ticket += 1;
            t.native_controls.insert(ticket, reply);
            t.core.codex_controlled(ticket, op, permit, now);
        }
        Command::Control { op, permit, reply } => {
            let ticket = t.next_ticket;
            t.next_ticket += 1;
            t.controls.insert(ticket, reply);
            match permit {
                Some(p) => t.core.control_controlled(ticket, op, p, now),
                None => t.core.control(ticket, op, now),
            }
        }
        // codex 没有终端可按：M3d 改为回复挂起的服务端请求（T10c §2.6）。
        Command::Answer { reply, .. } => {
            let _ = reply.send(AnswerOutcome::Unsupported);
        }
    }
    false
}
