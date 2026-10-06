//! 编排核心（spec §22.4）：一个 actor 独占全部会话句柄与内存状态，按输入顺序一条条处理（裁定 6）。
//! 判定在 crewd-wake；这里只调 Port、调执行器、发事件。
//!
//! 外来文字（session 名、claude 的原话、屏幕上抓来的提示 / 选项、错误原因、路径）插进群通知或唤醒附言之前
//! 一律过 `crewd_collab::render::safe_name`（控制者裁定 C12 / C20）：压成一行、去掉双向 / 零宽字符、换掉帧括号。
//!
//! **绑定规则（退出时的 inflight 批）**：驱动层停掉 / 退出时先 `abandon()`——每笔没终局的投递（含开场正文
//! `BRIEF_DELIVERY_ID`）一条 `Delivery{Abandoned}`、在干活的一条 `Working(false)`——最后才是 `Exited`。
//! 退出时 inflight 的那批**只在 `on_exited` 里结算**（人停 → 丢弃；crewd 停 / 撞额度 → 原样退回；自己死了 →
//! 一次失败，C2）。投递路径（Task 8 的 `on_delivery`）收到 `Abandoned` 一律不结算 inflight、留给 `on_exited`：
//! 判据就是 [`delivery_settles_inflight`]，别另写一份。
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_collab::render::safe_name;
use crewd_exec::driver::SendError;
use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, SessionEvent};
use crewd_term::{DecisionEvent, PendingDecision};
use crewd_wake::breaker::SYSTEM;
use crewd_wake::notice::{
    self, DECISION_ESCALATE_MS, DecisionFacts, NUDGE_KEY_GAP_MS, NoticeStage, NudgePlan,
};
use crewd_wake::policy::{self, Gate, LaunchGate};
use crewd_wake::queue::{self, ids};
use crewd_wake::text;
use crewd_wake::{MIN, Ms, NewWake, Priority, QueuedWake, WakeKind, source};
use tokio::sync::{mpsc, oneshot};

use super::OrchestraConfig;
use super::clock::Clock;
use super::exec::{Context, ExecHandle, ExecSendError, Executor, Executors, LaunchRequest};
use super::hooks::Hooks;
use super::port::*;

/// 这次投递回执能不能结算 inflight 的那批：只有终局、且不是 `Abandoned`（那是驱动层停掉 / 退出时的 abandon，
/// 该批由 `on_exited` 按三种情况结算，见模块文档的绑定规则）。
#[allow(dead_code)] // Task 8 的 `on_delivery` 调它。
pub(crate) fn delivery_settles_inflight(outcome: &DeliveryOutcome) -> bool {
    outcome.is_final() && !matches!(outcome, DeliveryOutcome::Abandoned(_))
}

pub enum Input {
    Exec {
        session: String,
        generator: u64,
        event: SessionEvent,
    },
    Launched {
        session: String,
        generator: u64,
        result: Result<Arc<dyn ExecHandle>, String>,
    },
    /// `isolation: true` 的 worktree 在派生任务里建完了（Task 6 评审 (2)：`git worktree add` 可能几十秒，不在 actor 里等）。
    WorktreeReady {
        req: StartSession,
        reply: Reply<SessionRow>,
        result: Result<PathBuf, String>,
    },
    /// 事件流结束了、却没有 `Exited`（驱动任务崩了 / 被丢了）：当作一次没有退出状态的退出（预检 P3）。
    StreamEnded {
        session: String,
        generator: u64,
    },
    Tick,
    Cmd(Cmd),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// 人从客户端（不受可选上限、不错开）。
    Human,
    /// agent 经 `SessionControl`（受可选上限）。
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartSession {
    pub crew_id: String,
    pub captain: bool,
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 控制者裁定 C8：true = 在机组工作目录里新建 git worktree 并在那里起；false = 机组共享目录。
    pub isolation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlError {
    NotFound(String),
    AlreadyRunning(String),
    NotRunning(String),
    AtCap {
        max: usize,
    },
    Unsupported(String),
    NoWorkdir(String),
    /// 建 worktree 失败（C8）：原因给人看；什么都没留下、没起、没退回共享目录。
    Worktree(String),
    /// 要起机长、但机组已有机长（M3a 的 `sessions::register` 对此回 `Conflict`）。换机长走 `crew.setCaptain` / handoff。
    CaptainTaken(String),
    /// 请求本身不成立（空的 nudge、在没编号的选择框上发数字）。
    Invalid(String),
    Internal(String),
}

impl ControlError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }

    /// `Core::launch` 的错误：它自己拒的（`AlreadyRunning`）原样带出来，其余是 Internal。
    pub fn from_launch(e: anyhow::Error) -> Self {
        e.downcast::<Self>().unwrap_or_else(Self::internal)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "没有 session {id}"),
            Self::AlreadyRunning(id) => write!(f, "session {id} 已经在跑或正在起"),
            Self::NotRunning(id) => write!(f, "session {id} 没在跑"),
            Self::AtCap { max } => write!(f, "已达设置里的并发会话上限 {max}"),
            Self::Unsupported(why)
            | Self::Internal(why)
            | Self::Worktree(why)
            | Self::Invalid(why) => {
                write!(f, "{why}")
            }
            Self::NoWorkdir(id) => write!(f, "session {id} 没有可用的工作目录"),
            Self::CaptainTaken(crew) => write!(f, "机组 {crew} 已有机长；换机长用 crew.setCaptain"),
        }
    }
}

impl std::error::Error for ControlError {}

pub type Reply<T> = oneshot::Sender<Result<T, ControlError>>;

pub enum Cmd {
    Start {
        req: StartSession,
        origin: Origin,
        reply: Reply<SessionRow>,
    },
    Resume {
        session: String,
        origin: Origin,
        reply: Reply<()>,
    },
    Stop {
        session: String,
        reply: Reply<()>,
    },
    Live {
        session: String,
        reply: Reply<LiveView>,
    },
    Nudge {
        session: String,
        input: String,
        reply: Reply<()>,
    },
    Handle {
        session: String,
        reply: oneshot::Sender<Option<Arc<dyn ExecHandle>>>,
    },
    SetProfile {
        session: String,
        model: Option<String>,
        effort: Option<String>,
        reply: Reply<()>,
    },
}

/// 一笔还没回执的投递。`delivery` = `ExecHandle::send` 的编号（拉起时带的正文 = `BRIEF_DELIVERY_ID`）。
pub struct Inflight {
    pub delivery: u64,
    /// 可以是空的：只有积压的批（M3a R6）也要投，那时只有 `cursor_targets`。
    pub wakes: Vec<QueuedWake>,
    /// 投递成功后各群游标推到哪（`Rendered::cursor_targets`）；**不是**排队行里最大的 seq。
    pub cursor_targets: Vec<(String, i64)>,
}

pub struct Live {
    /// 这次拉起的代号：旧进程迟到的事件按它丢掉（裁定 6）。
    pub generator: u64,
    pub handle: Arc<dyn ExecHandle>,
    pub crew: String,
    pub captain: bool,
    /// 原样的显示名（`SessionRow::name`）；插进文字前过 `safe_name`。
    pub name: String,
    pub executor: String,
    /// 以最后一条 `Spawned` 为准（裁定 12）。
    pub agent_session_id: String,
    pub working: bool,
    /// 这次拉起之后干过活。
    pub worked: bool,
    /// 这次拉起经历过续跑被拒。
    pub refused: bool,
    pub health: Option<Health>,
    pub decision: Option<PendingDecision>,
    pub inflight: Option<Inflight>,
    /// 人从客户端叫停的（不是停机）。
    pub stopping: bool,
    /// crewd 自己叫停的（停机等，[`Core::halt`]）：不改 desired / was_working，带着的唤醒原样退回。
    pub halting: bool,
    /// 往这个会话里写东西的先后（预检 P4）：nudge 的一串键 / 一段文本拿着它写完才放，两次 nudge 不交错。
    /// **Task 8 的投递（`handle.send`）也要排进来**：actor 里不能等锁——`try_lock` 拿不到 = 正有一串键在写，
    /// 这一刻不投，等那次 nudge 写完（回执之后）再投；拿到了就在持锁时 `send`。
    pub writes: Arc<tokio::sync::Mutex<()>>,
}

pub struct Starting {
    pub generator: u64,
    pub row: SessionRow,
    /// 随开场正文一起送的唤醒（非空时一定有正文）。
    pub wakes: Vec<QueuedWake>,
    /// 开场正文送达后各群游标推到哪（同 `Inflight::cursor_targets`；只有积压时 `wakes` 可以为空而它非空）。
    pub cursor_targets: Vec<(String, i64)>,
    pub stop_requested: bool,
}

pub struct Raised {
    pub at: Ms,
    pub fingerprint: String,
    pub escalated: bool,
}

pub struct Core<P: Port> {
    pub(crate) port: Arc<P>,
    pub(crate) execs: Executors,
    pub(crate) hooks: Hooks,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) cfg: OrchestraConfig,
    pub(crate) inbox: mpsc::UnboundedSender<Input>,
    pub(crate) live: HashMap<String, Live>,
    pub(crate) starting: HashMap<String, Starting>,
    /// 正在派生任务里建 worktree、还没写行的起会话请求数：照样占上限的名额。
    pub(crate) preparing: usize,
    /// 正在建 worktree 的「起机长」请求所在的机组：同一机组的第二个机长此时就该被拒。
    pub(crate) captain_pending: BTreeSet<String>,
    pub(crate) next_generator: u64,
    pub(crate) gate: LaunchGate,
    pub(crate) decisions: HashMap<String, Raised>,
    /// 重启后等人答「恢复」的 session（§22.7 Q1）：唤醒照常入队，但不被自动拉起（人 @ 它例外，C4）。
    pub(crate) held: BTreeSet<String>,
    /// `boot` 结束时队列里最大的唤醒 id：比它新的人类消息唤醒 = 询问开出之后人 @ 的（C4）。
    #[allow(dead_code)] // Task 8 / 9（`may_launch`、`boot`）读它。
    pub(crate) held_watermark: i64,
    /// `boot` 开的那条重启询问（M3a `restart::open` 的 id）：协作事件落后（Lagged）时据它从库里重推「答了没有」。
    #[allow(dead_code)] // Task 9（`on_lagged`）读它。
    pub(crate) restart_prompt: Option<i64>,
}

impl<P: Port> Core<P> {
    pub async fn new(
        port: Arc<P>,
        execs: Executors,
        hooks: Hooks,
        clock: Arc<dyn Clock>,
        cfg: OrchestraConfig,
        inbox: mpsc::UnboundedSender<Input>,
    ) -> anyhow::Result<Self> {
        let mut cfg = cfg;
        cfg.lang = port.locale().await?;
        // 可选并发上限（§22.7 Q5：缺省不设），存在 M3a 的 `collab::settings`。
        let gate = LaunchGate::new(port.max_live().await?, cfg.stagger_ms);
        Ok(Self {
            port,
            execs,
            hooks,
            clock,
            cfg,
            inbox,
            live: HashMap::new(),
            starting: HashMap::new(),
            preparing: 0,
            captain_pending: BTreeSet::new(),
            next_generator: 0,
            gate,
            decisions: HashMap::new(),
            held: BTreeSet::new(),
            held_watermark: 0,
            restart_prompt: None,
        })
    }

    pub async fn handle(&mut self, input: Input) {
        let r = match input {
            Input::Exec {
                session,
                generator,
                event,
            } => self.on_exec(&session, generator, event).await,
            Input::Launched {
                session,
                generator,
                result,
            } => self.on_launched(&session, generator, result).await,
            Input::WorktreeReady { req, reply, result } => {
                self.on_worktree_ready(req, reply, result).await;
                Ok(())
            }
            Input::StreamEnded { session, generator } => match self.live.get(&session) {
                Some(l) if l.generator == generator => self.on_exited(&session, None).await,
                _ => Ok(()),
            },
            Input::Tick => self.on_tick().await,
            Input::Cmd(c) => {
                self.on_cmd(c).await;
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("crewd: 编排：{e:#}");
        }
    }

    pub(crate) fn now(&self) -> Ms {
        self.clock.now()
    }

    pub(crate) fn lang(&self) -> text::Lang {
        self.cfg.lang
    }

    // —— 命令

    async fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Start { req, origin, reply } => self.start_new(req, origin, reply).await,
            Cmd::Resume {
                session,
                origin,
                reply,
            } => {
                let r = self.resume(&session, origin).await;
                let _ = reply.send(r);
            }
            Cmd::Stop { session, reply } => {
                let r = self.stop(&session).await;
                let _ = reply.send(r);
            }
            Cmd::Live { session, reply } => {
                let r = self.live_view(&session).await;
                let _ = reply.send(r);
            }
            Cmd::Nudge {
                session,
                input,
                reply,
            } => self.nudge(&session, &input, reply),
            Cmd::Handle { session, reply } => {
                let _ = reply.send(self.live.get(&session).map(|l| l.handle.clone()));
            }
            Cmd::SetProfile {
                session,
                model,
                effort,
                reply,
            } => {
                // 没给的 = 不改（M3a `set_profile` 是 COALESCE，清不掉）。
                let patch = SessionPatch {
                    model,
                    effort,
                    ..Default::default()
                };
                let r = self
                    .port
                    .patch_session(&session, patch)
                    .await
                    .map_err(ControlError::internal);
                let _ = reply.send(r);
            }
        }
    }

    /// 显式起会话的上限检查：人起的不受上限；显式起的不错开（裁定 5）。正在建 worktree 的也占名额。
    pub(crate) fn check_cap(&self, origin: Origin) -> Result<(), ControlError> {
        let live = self.live.len() + self.starting.len() + self.preparing;
        match self.gate.check(live, origin == Origin::Human, self.now()) {
            Gate::AtCap => Err(ControlError::AtCap {
                max: self.gate.max_live().unwrap_or(live),
            }),
            Gate::Go | Gate::WaitUntil(_) => Ok(()),
        }
    }

    /// 起一个新会话。`isolation: true` 时 worktree 在派生任务里建（不堵 actor），建完以 `Input::WorktreeReady`
    /// 回来再写行、拉起；回执在那时才发。
    async fn start_new(&mut self, req: StartSession, origin: Origin, reply: Reply<SessionRow>) {
        let base = match self.check_start(&req, origin).await {
            Ok(base) => base,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let Some(base) = base else {
            let r = self.finish_start(req, None).await;
            let _ = reply.send(r);
            return;
        };
        self.preparing += 1;
        if req.captain {
            self.captain_pending.insert(req.crew_id.clone());
        }
        let (worktrees, tx) = (self.hooks.worktrees.clone(), self.inbox.clone());
        tokio::spawn(async move {
            // hint = 开场 brief（Swift `SessionWorkspace.resolve(hint: brief)`）。
            let result = worktrees.create(&base, &req.brief).await;
            let _ = tx.send(Input::WorktreeReady { req, reply, result });
        });
    }

    /// 起之前的检查；`isolation: true` 时返回建 worktree 的起点（机组工作目录）。
    async fn check_start(
        &self,
        req: &StartSession,
        origin: Origin,
    ) -> Result<Option<PathBuf>, ControlError> {
        self.execs
            .get(&req.executor)
            .map_err(|e| ControlError::Unsupported(e.to_string()))?;
        self.check_cap(origin)?;
        // M3a 的 `sessions::register` 对「机组已有机长、又要起一个机长」回 `Conflict`：先查，给一个说得清的错
        // （不然它会在 worktree 建好之后才冒出来，还被包成 Internal）。不建 worktree、不写行、不起进程。
        if req.captain
            && (self.captain_pending.contains(&req.crew_id)
                || self
                    .port
                    .captain(&req.crew_id)
                    .await
                    .map_err(ControlError::internal)?
                    .is_some())
        {
            return Err(ControlError::CaptainTaken(req.crew_id.clone()));
        }
        if !req.isolation {
            return Ok(None);
        }
        // 控制者裁定 C8：isolation=true → 先建 worktree（写行之前；失败 = 什么都不留、不起、不退回共享目录）。
        let base = self
            .port
            .crew_workdir(&req.crew_id)
            .await
            .map_err(ControlError::internal)?
            .ok_or_else(|| {
                ControlError::Worktree(format!(
                    "机组 {} 没有工作目录，无法为 session 新建 worktree",
                    req.crew_id
                ))
            })?;
        Ok(Some(base))
    }

    async fn on_worktree_ready(
        &mut self,
        req: StartSession,
        reply: Reply<SessionRow>,
        result: Result<PathBuf, String>,
    ) {
        self.preparing = self.preparing.saturating_sub(1);
        if req.captain {
            self.captain_pending.remove(&req.crew_id);
        }
        let r = match result {
            Ok(path) => self.finish_start(req, Some(path)).await,
            Err(why) => Err(ControlError::Worktree(why)),
        };
        let _ = reply.send(r);
    }

    /// 写行、拉起。`worktree` = 已建好的 worktree（之后出错它也留着：crewd 不删 worktree，把路径告诉人）。
    async fn finish_start(
        &mut self,
        req: StartSession,
        worktree: Option<PathBuf>,
    ) -> Result<SessionRow, ControlError> {
        let kept = |e: String| match &worktree {
            Some(p) => ControlError::Internal(format!(
                "{e}（已建好的 worktree {} 不会被自动删除）",
                p.display()
            )),
            None => ControlError::Internal(e),
        };
        let exec = self
            .execs
            .get(&req.executor)
            .map_err(|e| ControlError::Unsupported(e.to_string()))?;
        // 建 worktree 期间机长指针可能变了（换机长）：再查一次。
        if worktree.is_some()
            && req.captain
            && self
                .port
                .captain(&req.crew_id)
                .await
                .map_err(|e| kept(e.to_string()))?
                .is_some()
        {
            return Err(kept(
                ControlError::CaptainTaken(req.crew_id.clone()).to_string(),
            ));
        }
        let brief = Some(req.brief.clone()).filter(|b| !b.trim().is_empty());
        let row = self
            .port
            .insert_session(NewSession {
                crew_id: req.crew_id,
                captain: req.captain,
                executor: req.executor,
                brief: req.brief,
                title: req.title,
                model: req.model,
                effort: req.effort,
                worktree: worktree.clone(),
            })
            .await
            .map_err(|e| kept(e.to_string()))?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        self.launch(row.clone(), exec, brief, vec![], vec![])
            .await
            .map_err(ControlError::from_launch)?;
        Ok(row)
    }

    pub(crate) async fn resume(
        &mut self,
        session: &str,
        origin: Origin,
    ) -> Result<(), ControlError> {
        if self.live.contains_key(session) || self.starting.contains_key(session) {
            return Err(ControlError::AlreadyRunning(session.into()));
        }
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        let exec = self
            .execs
            .get(&row.executor)
            .map_err(|e| ControlError::Unsupported(e.to_string()))?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        self.check_cap(origin)?;
        // 单个恢复一个等人答复的 session（§22.7 Q1）：上次停在干活中途的，开场补一句「被打断」。
        let interrupted = self.held.remove(session) && row.was_working;
        let text = interrupted.then(|| text::get(self.lang(), "restart.interrupted").to_string());
        self.launch(row, exec, text, vec![], vec![])
            .await
            .map_err(ControlError::from_launch)
    }

    /// 人停掉（裁定 10）：desired=stopped，丢掉此刻 pending 的唤醒，叫停进程（已带进进程、还没投进去的那些
    /// 在它退出时一并丢掉，见 `on_exited`）。
    async fn stop(&mut self, session: &str) -> Result<(), ControlError> {
        if self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .is_none()
        {
            return Err(ControlError::NotFound(session.into()));
        }
        let now = self.now();
        let pending = self
            .port
            .pending(session)
            .await
            .map_err(ControlError::internal)?;
        if !pending.is_empty() {
            self.port
                .settle(
                    &ids(&pending),
                    Settle::Dropped {
                        error: "stopped".into(),
                    },
                    now,
                )
                .await
                .map_err(ControlError::internal)?;
        }
        let patch = SessionPatch {
            desired: Some(Desired::Stopped),
            was_working: Some(false),
            ..Default::default()
        };
        self.port
            .patch_session(session, patch)
            .await
            .map_err(ControlError::internal)?;
        self.held.remove(session);
        if let Some(l) = self.live.get_mut(session) {
            l.stopping = true;
            let _ = l.handle.stop();
        } else if let Some(st) = self.starting.get_mut(session) {
            st.stop_requested = true;
        }
        Ok(())
    }

    /// crewd 自己叫停一个在跑的 session（停机用，Task 9）：不改 desired，也不让驱动层 abandon 发来的
    /// `Working(false)` 抹掉 `was_working`——下次启动问人时要知道它是被打断的。带着的唤醒退出时原样退回 pending。
    #[allow(dead_code)] // Task 9 的停机调它。
    pub(crate) fn halt(&mut self, session: &str) {
        if let Some(l) = self.live.get_mut(session) {
            l.halting = true;
            let _ = l.handle.stop();
        }
    }

    /// nudge（`crewd_wake::notice::nudge_plan` 的契约）：空的拒绝（绝不变成一个裸回车）；键 / 数字逐个单独 `raw`、
    /// 之间隔 `NUDGE_KEY_GAP_MS`；文本走 `send`（三拍提交）。都在派生任务里、拿着 `Live::writes` 写（不堵 actor、
    /// 不交错；写完才回执）。
    /// 待决策框的选项没有编号时拒绝数字：发了也只是把回车落在当前高亮项上。
    fn nudge(&self, session: &str, input: &str, reply: Reply<()>) {
        let not_running = || ControlError::NotRunning(session.into());
        let Some(l) = self.live.get(session) else {
            let _ = reply.send(Err(not_running()));
            return;
        };
        let Some(plan) = notice::nudge_plan(input) else {
            let why = "nudge 的输入是空的：不会替它按一个裸回车（那会选中当前高亮的那一项）";
            let _ = reply.send(Err(ControlError::Invalid(why.into())));
            return;
        };
        let digits = input.trim().bytes().all(|b| b.is_ascii_digit());
        if digits && l.decision.as_ref().is_some_and(|d| !d.numbered) {
            let why =
                "这一屏的选项没有编号，发数字无效；高亮在要选的那一项上就发 enter，否则交给人";
            let _ = reply.send(Err(ControlError::Invalid(why.into())));
            return;
        }
        // 写会话的先后由 `Live::writes` 排队：拿到锁才写，一次 nudge 的整串写完才放（锁是 FIFO 的，按到达顺序）。
        let (handle, writes, session) = (l.handle.clone(), l.writes.clone(), session.to_string());
        tokio::spawn(async move {
            let _turn = writes.lock().await;
            let r =
                match plan {
                    NudgePlan::Text(text) => handle
                        .send(text, &Context::new())
                        .map(|_| ())
                        .map_err(|e| match e {
                            ExecSendError::Send(SendError::Closed) => {
                                ControlError::NotRunning(session)
                            }
                            other => ControlError::Invalid(other.to_string()),
                        }),
                    NudgePlan::Keys(keys) => {
                        let mut r = Ok(());
                        for (i, key) in keys.into_iter().enumerate() {
                            if i > 0 {
                                tokio::time::sleep(Duration::from_millis(NUDGE_KEY_GAP_MS)).await;
                            }
                            if handle.raw(key).is_err() {
                                r = Err(ControlError::NotRunning(session));
                                break;
                            }
                        }
                        r
                    }
                };
            let _ = reply.send(r);
        });
    }

    // —— 拉起

    /// 拉起一个 session：`text` = 开场正文（新会话的 brief，或唤醒批的正文），`wakes` = 随正文送的唤醒（记 inflight；
    /// 非空时 `text` 必须有）。
    /// 真正的启动在派生任务里做，结果以 `Input::Launched` 回来（裁定 6）。`desired=running` 等 `Spawned` 再写（C18）。
    pub(crate) async fn launch(
        &mut self,
        row: SessionRow,
        exec: Arc<dyn Executor>,
        text: Option<String>,
        wakes: Vec<QueuedWake>,
        cursor_targets: Vec<(String, i64)>,
    ) -> anyhow::Result<()> {
        // 一个 session 一个进程（裁定 6）：所有拉起路径都经过这里，所以在这里挡，不只靠调用方先查。
        if self.live.contains_key(&row.id) || self.starting.contains_key(&row.id) {
            return Err(ControlError::AlreadyRunning(row.id).into());
        }
        let now = self.now();
        let workdir = row
            .workdir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("session {} 没有工作目录", row.id))?;
        let system_prompt = Some(self.port.worldview(&row.id).await?);
        let token = self.port.mint_token(&row.id);
        if !wakes.is_empty() {
            self.port
                .settle(&ids(&wakes), Settle::Inflight, now)
                .await?;
        }
        self.gate.launched(now);
        self.next_generator += 1;
        let generator = self.next_generator;
        let session = row.id.clone();
        let req = LaunchRequest {
            session_id: row.id.clone(),
            crew_id: row.crew_id.clone(),
            workdir,
            model: row.model.clone(),
            effort: row.effort.clone(),
            system_prompt,
            initial_prompt: text.clone(),
            token,
            ledger: self.port.ledger(&row.id),
        };
        self.starting.insert(
            session.clone(),
            Starting {
                generator,
                row,
                wakes,
                cursor_targets,
                stop_requested: false,
            },
        );
        let tx = self.inbox.clone();
        let task_session = session.clone();
        tokio::spawn(async move {
            match exec.launch(req).await {
                Ok((handle, mut events)) => {
                    // 先报 Launched、再转事件：同一个发送端按序到达，actor 先建好 Live 再收 Spawned。
                    let launched = Input::Launched {
                        session: task_session.clone(),
                        generator,
                        result: Ok(handle),
                    };
                    if tx.send(launched).is_err() {
                        return;
                    }
                    let mut exited = false;
                    while let Some(event) = events.recv().await {
                        exited |= matches!(event, SessionEvent::Exited(_));
                        let input = Input::Exec {
                            session: task_session.clone(),
                            generator,
                            event,
                        };
                        if tx.send(input).is_err() {
                            return;
                        }
                    }
                    // 流断了却没见过 `Exited`（驱动任务没了）：照样当进程没了处理，不留僵尸 `Live`。
                    if !exited {
                        let _ = tx.send(Input::StreamEnded {
                            session: task_session,
                            generator,
                        });
                    }
                }
                Err(e) => {
                    let _ = tx.send(Input::Launched {
                        session: task_session,
                        generator,
                        result: Err(e.to_string()),
                    });
                }
            }
        });
        self.publish_live(&session).await;
        Ok(())
    }

    async fn on_launched(
        &mut self,
        session: &str,
        generator: u64,
        result: Result<Arc<dyn ExecHandle>, String>,
    ) -> anyhow::Result<()> {
        if self
            .starting
            .get(session)
            .is_none_or(|s| s.generator != generator)
        {
            // 没人在等这次拉起：起来了的进程不留着就叫停，不能成为没人管的孤儿。
            if let Ok(handle) = result {
                let _ = handle.stop();
            }
            return Ok(());
        }
        let st = self.starting.remove(session).expect("上面查过");
        match result {
            Ok(handle) => {
                if st.stop_requested {
                    let _ = handle.stop();
                }
                let inflight =
                    (!st.wakes.is_empty() || !st.cursor_targets.is_empty()).then(|| Inflight {
                        delivery: BRIEF_DELIVERY_ID,
                        wakes: st.wakes.clone(),
                        cursor_targets: st.cursor_targets.clone(),
                    });
                self.live.insert(
                    session.to_string(),
                    Live {
                        generator,
                        handle,
                        crew: st.row.crew_id.clone(),
                        captain: st.row.captain,
                        name: st.row.name(),
                        executor: st.row.executor.clone(),
                        agent_session_id: String::new(),
                        working: false,
                        worked: false,
                        refused: false,
                        health: None,
                        decision: None,
                        inflight,
                        stopping: st.stop_requested,
                        halting: false,
                        writes: Arc::new(tokio::sync::Mutex::new(())),
                    },
                );
            }
            Err(_) if st.stop_requested => {
                // 人在拉起回来之前就停掉了：带着的唤醒跟人停一样丢掉（裁定 10）——不重试、不告警、不记「起不来」。
                if !st.wakes.is_empty() {
                    let s = Settle::Dropped {
                        error: "stopped".into(),
                    };
                    self.port.settle(&ids(&st.wakes), s, self.now()).await?;
                }
            }
            Err(e) => {
                // 先记恢复失败（告警一次），再按失败处理带着的唤醒：放弃时就不会再告第二次（裁定 9 / C2）。
                self.launch_failed(&st.row, &e).await?;
                self.fail_batch(session, st.wakes, &e).await?;
            }
        }
        self.publish_live(session).await;
        Ok(())
    }

    /// 起不来（裁定 11）：写 `resume_failed_at`；第一次才告警。
    pub(crate) async fn launch_failed(
        &mut self,
        row: &SessionRow,
        detail: &str,
    ) -> anyhow::Result<()> {
        let already = self
            .port
            .session(&row.id)
            .await?
            .and_then(|r| r.resume_failed_at)
            .is_some();
        let patch = SessionPatch {
            resume_failed_at: Some(Some(self.now())),
            ..Default::default()
        };
        self.port.patch_session(&row.id, patch).await?;
        if already {
            return Ok(());
        }
        let text = text::render(
            self.lang(),
            "resume.failed.group",
            &[
                ("name", &safe_name(&row.name())),
                ("detail", &safe_name(detail)),
            ],
        );
        self.port
            .post_system(self.alert_audience(&row.crew_id, row.captain), text)
            .await
    }

    /// 告警发给谁：worker 的事 → 本机组大厅 @群主；机长自己的事 → 根机组人类群 @人（绝不 @ 自己）。
    pub(crate) fn alert_audience(&self, crew: &str, is_captain: bool) -> Audience {
        if is_captain {
            Audience::HumanChannel { crew: crew.into() }
        } else {
            Audience::Lobby {
                crew: crew.into(),
                mention_owner: true,
            }
        }
    }

    /// 一批唤醒没送进去（终局失败）：30s / 2min 退避回 pending、次数 +1，第 `MAX_ATTEMPTS` 次放弃记 dropped（裁定 9）。
    /// 放弃时告警一次（任何优先级，C19）；已记过「起不来」的 session 不再告（那条已经说了）。
    /// 控制者裁定 C2 让它在这里先落地（拉起后、投进去之前进程就没了 = 一次失败）；Task 8 的投递失败也走它。
    pub(crate) async fn fail_batch(
        &mut self,
        session: &str,
        wakes: Vec<QueuedWake>,
        error: &str,
    ) -> anyhow::Result<()> {
        if wakes.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let mut gave_up = Vec::new();
        for w in wakes {
            let attempts = w.attempts + 1;
            let settle = match queue::after_failure(attempts, now) {
                Some(retry_at) => Settle::Retry {
                    attempts,
                    error: error.into(),
                    retry_at,
                },
                None => {
                    gave_up.push(w.clone());
                    Settle::Dropped {
                        error: error.into(),
                    }
                }
            };
            self.port.settle(&[w.id], settle, now).await?;
        }
        if queue::should_alert(&gave_up)
            && let Some(row) = self.port.session(session).await?
            && row.resume_failed_at.is_none()
        {
            let attempts = queue::MAX_ATTEMPTS.to_string();
            let msg = text::render(
                self.lang(),
                "delivery.failed.group",
                &[
                    ("name", &safe_name(&row.name())),
                    ("attempts", &attempts),
                    ("error", &safe_name(error)),
                ],
            );
            self.port
                .post_system(self.alert_audience(&row.crew_id, row.captain), msg)
                .await?;
        }
        self.port.publish(OrchestraEvent::WakeChanged {
            session: session.into(),
        });
        Ok(())
    }

    // —— 会话事件

    async fn on_exec(
        &mut self,
        session: &str,
        generator: u64,
        event: SessionEvent,
    ) -> anyhow::Result<()> {
        let now = self.clock.now();
        let Some(l) = self.live.get_mut(session) else {
            return Ok(());
        };
        if l.generator != generator {
            return Ok(());
        }
        match event {
            SessionEvent::Spawned {
                agent_session_id, ..
            } => {
                // 续跑被拒后驱动层换新号再起，会再来一条 Spawned：以最后一条为准（裁定 12 / M2c）。
                l.agent_session_id = agent_session_id;
                // 控制者裁定 C18：进程真的起来了才算「要它在跑」；人已经叫停的不写。
                if !l.stopping {
                    let patch = SessionPatch {
                        desired: Some(Desired::Running),
                        ..Default::default()
                    };
                    self.port.patch_session(session, patch).await?;
                }
            }
            SessionEvent::Working(on) => {
                l.working = on;
                let mut patch = SessionPatch {
                    // crewd 叫停时驱动层 abandon 发的 Working(false) 不落库：was_working 留着「被打断」。
                    was_working: (on || !l.halting).then_some(on),
                    last_active_at: Some(now),
                    ..Default::default()
                };
                if on {
                    l.worked = true;
                    if !l.refused {
                        patch.resume_failed_at = Some(None);
                    }
                }
                self.port.patch_session(session, patch).await?;
            }
            SessionEvent::Decision(DecisionEvent::Appeared(d)) => {
                l.decision = Some(d.clone());
                self.raise_decision(session, &d, now).await?;
            }
            SessionEvent::Decision(DecisionEvent::Cleared) => {
                l.decision = None;
                self.decisions.remove(session);
            }
            SessionEvent::Delivery { .. } => {}
            // M2c 合进 main 后 `SessionEvent` 多了这四个（main@e66be68 起；claude 执行器不发它们，codex 发）。M3c 只接了
            // claude，先不处理；接 codex 执行器时：`TurnEnded` → `crewd_collab::turn_trace::on_turn_end`（M3b）、
            // `Notice` → 群里一条（M2c 的代拒去重是**按方法**的：同一个 MCP server 的多种拒绝要按 server 汇总再报）、
            // `Profile` → 落库、`Item` → M2d 回放。
            SessionEvent::Item { .. }
            | SessionEvent::TurnEnded { .. }
            | SessionEvent::Notice(_)
            | SessionEvent::Profile(_) => {}
            SessionEvent::Health(h) => {
                let quota = h.as_ref().is_some_and(|h| h.kind.is_quota());
                l.health = h;
                if quota {
                    self.schedule_quota(session).await?;
                }
            }
            SessionEvent::ResumeRefused { id, said } => {
                l.refused = true;
                self.on_resume_refused(session, &id, &said).await?;
            }
            SessionEvent::Exited(info) => return self.on_exited(session, Some(info)).await,
        }
        self.publish_live(session).await;
        Ok(())
    }

    /// `info` = None：事件流断了、没有退出状态（`Input::StreamEnded`），按「自己没了」处理。
    async fn on_exited(&mut self, session: &str, info: Option<ExitInfo>) -> anyhow::Result<()> {
        let Some(l) = self.live.remove(session) else {
            return Ok(());
        };
        // 进程没了，它的 helper 令牌作废（M3a `TokenTable::revoke`；下次拉起重新 mint）。
        self.port.revoke_token(session);
        self.decisions.remove(session);
        let now = self.now();
        let mut patch = SessionPatch {
            last_active_at: Some(now),
            ..Default::default()
        };
        if l.stopping {
            patch.was_working = Some(false);
        }
        self.port.patch_session(session, patch).await?;
        let hit_limit = info.as_ref().is_some_and(|i| i.hit_limit);
        let stopped_by_user = info.as_ref().is_some_and(|i| i.stopped_by_user);
        if hit_limit {
            self.schedule_quota_for(session, &l.executor, &l.name, &l.crew)
                .await?;
        }
        // 先记「起不来」（告警一次），再处理带着的唤醒：放弃时就不会再告第二次。
        let launch_failed = l
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed);
        if launch_failed
            && !l.worked
            && let Some(row) = self.port.session(session).await?
        {
            let detail = l
                .health
                .as_ref()
                .map(|h| h.detail.clone())
                .unwrap_or_default();
            self.launch_failed(&row, &detail).await?;
        }
        // 还没投进去的唤醒（绑定规则：只在这里结算，见模块文档）：人停的 → 跟 pending 的一样丢掉（裁定 10）；
        // crewd 自己叫停的（停机）或撞额度退出 → 原样退回 pending（裁定 17 至少一次；额度唤醒另挂）；
        // 进程自己没了 → 一次失败的投递，退避、有限次后放弃（控制者裁定 C2）。
        if let Some(inf) = l.inflight
            && !inf.wakes.is_empty()
        {
            if l.stopping {
                let s = Settle::Dropped {
                    error: "stopped".into(),
                };
                self.port.settle(&ids(&inf.wakes), s, now).await?;
            } else if l.halting || stopped_by_user || hit_limit {
                self.port
                    .settle(&ids(&inf.wakes), Settle::Requeue, now)
                    .await?;
            } else {
                let why = match &info {
                    Some(i) => format!("{:?}", i.status),
                    None => "event stream ended without an exit status".to_string(),
                };
                self.fail_batch(session, inf.wakes, &why).await?;
            }
        }
        self.publish_live(session).await;
        Ok(())
    }

    /// 续跑被拒（裁定 12）：群里一条（claude 原话 + 日志在不在盘上），给 agent 一条 system 唤醒，记 `resume_failed_at`。
    /// 控制者裁定 C13：群里那条 @机长（机长自己被拒 → 改 @人，绝不 @ 自己）。
    async fn on_resume_refused(
        &mut self,
        session: &str,
        id: &str,
        said: &str,
    ) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else {
            return Ok(());
        };
        let (crew, captain, name, executor) = (
            l.crew.clone(),
            l.captain,
            l.name.clone(),
            l.executor.clone(),
        );
        let lang = self.lang();
        let (id_s, said_s) = (safe_name(id), safe_name(said));
        let diag = match self
            .execs
            .get(&executor)
            .ok()
            .and_then(|e| e.locate_transcript(id))
        {
            Some(p) => text::render(
                lang,
                "resume.refused.diag.found",
                &[("path", &safe_name(&p.display().to_string()))],
            ),
            None => text::get(lang, "resume.refused.diag.missing").to_string(),
        };
        let group = text::render(
            lang,
            "resume.refused.group",
            &[("name", &safe_name(&name)), ("said", &said_s)],
        );
        self.port
            .post_system(
                self.alert_audience(&crew, captain),
                format!("{group}\n{diag}"),
            )
            .await?;
        let now = self.now();
        let patch = SessionPatch {
            resume_failed_at: Some(Some(now)),
            ..Default::default()
        };
        self.port.patch_session(session, patch).await?;
        self.port
            .enqueue(NewWake {
                session: session.into(),
                kind: WakeKind::System,
                source: source::system(&format!("resume-refused:{id}")),
                priority: Priority::System,
                not_before: now,
                note: Some(text::render(
                    lang,
                    "resume.refused.agent",
                    &[("id", &id_s), ("said", &said_s)],
                )),
                origin: SYSTEM.into(),
            })
            .await?;
        Ok(())
    }

    // —— 额度（裁定 13）

    async fn schedule_quota(&mut self, session: &str) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else {
            return Ok(());
        };
        let (executor, name, crew) = (l.executor.clone(), l.name.clone(), l.crew.clone());
        self.schedule_quota_for(session, &executor, &name, &crew)
            .await
    }

    pub(crate) async fn schedule_quota_for(
        &mut self,
        session: &str,
        executor: &str,
        name: &str,
        crew: &str,
    ) -> anyhow::Result<()> {
        if policy::quota_pending(&self.port.pending(session).await?) {
            return Ok(());
        }
        let now = self.now();
        let reset = self.hooks.quota.reset_at(executor, now).await;
        let plan = policy::quota_plan(reset, now);
        let lang = self.lang();
        self.port
            .enqueue(NewWake {
                session: session.into(),
                kind: WakeKind::System,
                source: plan.source(),
                priority: Priority::System,
                not_before: plan.fire_at,
                note: Some(text::get(lang, "quota.wake").to_string()),
                origin: SYSTEM.into(),
            })
            .await?;
        let minutes = (plan.fire_at.since(now) + MIN - 1) / MIN;
        let key = if plan.fallback {
            "quota.fallback"
        } else {
            "quota.scheduled"
        };
        let msg = text::render(
            lang,
            key,
            &[
                ("name", &safe_name(name)),
                ("minutes", &minutes.to_string()),
            ],
        );
        self.port
            .post_system(
                Audience::Lobby {
                    crew: crew.into(),
                    mention_owner: false,
                },
                msg,
            )
            .await
    }

    // —— 待决策（裁定 14）

    async fn raise_decision(
        &mut self,
        session: &str,
        d: &PendingDecision,
        now: Ms,
    ) -> anyhow::Result<()> {
        let fingerprint = d.fingerprint();
        if self
            .decisions
            .get(session)
            .is_some_and(|r| r.fingerprint == fingerprint)
        {
            return Ok(());
        }
        self.decisions.insert(
            session.into(),
            Raised {
                at: now,
                fingerprint,
                escalated: false,
            },
        );
        self.post_decision(session, d, NoticeStage::First, 0).await
    }

    /// 名字、session、屏幕上抓来的提示与选项都是外来文字：先过 `safe_name`（`crewd_wake::notice` 模块文档的约定）。
    async fn post_decision(
        &self,
        session: &str,
        d: &PendingDecision,
        stage: NoticeStage,
        waited_min: i64,
    ) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else {
            return Ok(());
        };
        let facts = DecisionFacts {
            prompt: safe_name(&d.prompt),
            options: d.options.iter().map(|o| safe_name(o)).collect(),
            numbered: d.numbered,
        };
        let n = notice::decision_notice(
            self.lang(),
            stage,
            &safe_name(&l.name),
            &safe_name(session),
            l.captain,
            &facts,
            waited_min,
        );
        let to = if n.to_human {
            Audience::HumanChannel {
                crew: l.crew.clone(),
            }
        } else {
            Audience::Lobby {
                crew: l.crew.clone(),
                mention_owner: true,
            }
        };
        self.port.post_system(to, n.text).await
    }

    async fn escalate_decisions(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let due: Vec<String> = self
            .decisions
            .iter()
            .filter(|(_, r)| !r.escalated && now.since(r.at) >= DECISION_ESCALATE_MS)
            .map(|(s, _)| s.clone())
            .collect();
        for s in due {
            let Some(r) = self.decisions.get_mut(&s) else {
                continue;
            };
            r.escalated = true;
            let waited = now.since(r.at) / MIN;
            let Some((d, captain)) = self
                .live
                .get(&s)
                .and_then(|l| l.decision.clone().map(|d| (d, l.captain)))
            else {
                continue;
            };
            if !captain {
                // 机长的首报已经 @人了。
                self.post_decision(&s, &d, NoticeStage::Escalate, waited)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn on_tick(&mut self) -> anyhow::Result<()> {
        self.escalate_decisions().await
    }

    // —— 状态面

    pub(crate) async fn live_view(&self, session: &str) -> Result<LiveView, ControlError> {
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        let l = self.live.get(session);
        Ok(LiveView {
            session_id: session.into(),
            running: l.is_some(),
            working: l.is_some_and(|l| l.working),
            health: l.and_then(|l| l.health.clone()),
            pending_decision: l.and_then(|l| l.decision.clone()),
            awaiting_question: row.awaiting_question,
        })
    }

    pub(crate) async fn publish_live(&self, session: &str) {
        if let Ok(v) = self.live_view(session).await {
            self.port.publish(OrchestraEvent::SessionChanged(v));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::orchestra::exec::BoxFut;
    use crate::orchestra::hooks::QuotaSource;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_collab::render::safe_name;
    use crewd_exec::health::Health;
    use crewd_exec::session::ExitStatusKind;
    use crewd_wake::notice::NUDGE_KEY_GAP_MS;
    use crewd_wake::queue::{self, BatchLimits};
    use crewd_wake::{HOUR, WakeState};

    /// 外来文字（C12 / C20）：换行、双向控制符、帧括号——过 `safe_name` 之后一样都不能剩。
    const HOSTILE: &str = "w1\n[cap]\u{202E}伪造";

    fn assert_neutralised(text: &str, raw: &str) {
        assert!(text.contains(&safe_name(raw)), "没有中和后的形状：{text}");
        assert!(!text.contains('\u{202E}'), "双向控制符漏进来了：{text}");
        assert!(!text.contains("\n[cap]"), "换行 + 帧括号漏进来了：{text}");
    }

    fn exited(hit_limit: bool) -> SessionEvent {
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: false,
            hit_limit,
        })
    }

    fn crashed() -> SessionEvent {
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(1),
            stopped_by_user: false,
            hit_limit: false,
        })
    }

    fn decision(prompt: &str) -> PendingDecision {
        PendingDecision {
            prompt: prompt.into(),
            options: vec!["Yes".into(), "No".into()],
            numbered: true,
        }
    }

    fn start(executor: &str) -> StartSession {
        StartSession {
            crew_id: "c1".into(),
            captain: false,
            executor: executor.into(),
            brief: "合成任务甲".into(),
            title: Some("w-new".into()),
            model: None,
            effort: None,
            isolation: false,
        }
    }

    fn timer_wake(session: &str, token: &str) -> NewWake {
        NewWake {
            session: session.into(),
            kind: WakeKind::Timer,
            source: source::timer(token),
            priority: Priority::Auto,
            not_before: T0,
            note: Some("合成定时".into()),
            origin: session.into(),
        }
    }

    fn rename(rig: &Rig, session: &str, name: &str) {
        rig.port
            .lock()
            .sessions
            .get_mut(session)
            .expect("先建 session")
            .display_name = Some(name.into());
    }

    #[tokio::test]
    async fn concurrent_resume_of_one_session_starts_exactly_one_process() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let (a, ra) = oneshot::channel();
        let (b, rb) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply: a,
            }))
            .await;
        rig.core
            .handle(Input::Cmd(Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply: b,
            }))
            .await;
        rig.settle().await;
        assert_eq!(ra.await.unwrap(), Ok(()));
        assert_eq!(
            rb.await.unwrap(),
            Err(ControlError::AlreadyRunning("cap".into()))
        );
        assert_eq!(rig.exec.launches("cap").len(), 1);
        let l = &rig.exec.launches("cap")[0];
        assert_eq!(
            (l.system_prompt.as_deref(), l.token.as_str()),
            (Some("WORLD cap"), "tok-cap")
        );
        assert_eq!(
            rig.live("cap").map(|l| l.agent_session_id.as_str()),
            Some("agent-cap")
        );
        assert_eq!(rig.port.row("cap").desired, Desired::Running);
        let again = rig
            .cmd(|reply| Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply,
            })
            .await;
        assert_eq!(again, Err(ControlError::AlreadyRunning("cap".into())));
    }

    /// 控制者裁定 C18：`desired=running` 只在 `Spawned` 之后写；起不来的不改它（重启时不会被当成「在跑」去问人）。
    #[tokio::test]
    async fn desired_running_is_written_only_after_spawned_and_a_failed_launch_leaves_it() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        let (tx, rx) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply: tx,
            }))
            .await;
        assert_eq!(
            rig.port.row("cap").desired,
            Desired::Stopped,
            "拉起还在派生任务里：进程还不存在"
        );
        rig.settle().await;
        rx.await.unwrap().unwrap();
        assert_eq!(rig.port.row("cap").desired, Desired::Running);
        rig.exec.lock().fail_next = vec!["合成：claude 不在".into()];
        rig.run("w1").await;
        assert_eq!(rig.port.row("w1").desired, Desired::Stopped);
    }

    /// C18：人在进程起来之前就停掉了——之后到的 `Spawned` 不能把 `desired` 改回 running。
    #[tokio::test]
    async fn a_stop_while_starting_keeps_desired_stopped_after_spawned() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let (tx, rx) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply: tx,
            }))
            .await;
        let (tx, stopped) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "cap".into(),
                reply: tx,
            }))
            .await;
        rig.settle().await;
        rx.await.unwrap().unwrap();
        stopped.await.unwrap().unwrap();
        assert_eq!(rig.exec.lock().stops, ["cap"], "起来就叫停");
        assert_eq!(rig.port.row("cap").desired, Desired::Stopped);
        assert!(rig.live("cap").is_none());
    }

    #[tokio::test]
    async fn start_creates_a_row_and_opens_with_the_brief() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let row = rig
            .cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(
            rig.exec.launches(&row.id)[0].initial.as_deref(),
            Some("合成任务甲")
        );
        assert!(rig.live(&row.id).is_some());
    }

    #[tokio::test]
    async fn isolation_false_uses_the_shared_crew_directory_and_never_touches_git() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let row = rig
            .cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap();
        assert!(rig.worktrees.calls().is_empty());
        assert_eq!(
            rig.exec.launches(&row.id)[0].workdir,
            PathBuf::from("/nonexistent/wd"),
            "机组共享目录"
        );
    }

    #[tokio::test]
    async fn isolation_true_creates_the_worktree_before_the_row_and_launches_there() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let req = StartSession {
            isolation: true,
            ..start("claude")
        };
        let row = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Agent,
                reply,
            })
            .await
            .unwrap();
        // 在机组工作目录里建，hint = 开场 brief（Swift `SessionWorkspace.resolve(hint: brief)`）。
        assert_eq!(
            rig.worktrees.calls(),
            vec![(PathBuf::from("/nonexistent/wd"), "合成任务甲".to_string())]
        );
        assert_eq!(
            row.workdir,
            Some(PathBuf::from("/nonexistent/worktrees/w-1"))
        );
        assert_eq!(
            rig.exec.launches(&row.id)[0].workdir,
            PathBuf::from("/nonexistent/worktrees/w-1"),
            "进程在 worktree 里起"
        );
    }

    /// Task 6 评审 (2)：建 worktree（大仓库要检出几十秒）不在 actor 里等——期间别的命令照常处理；
    /// 等着的那一个照样占上限的名额，另一个机长也起不来。
    #[tokio::test]
    async fn a_slow_worktree_does_not_stall_the_actor_and_still_counts() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port.lock().sessions.clear(); // 机组暂时没有机长
        rig.core.gate.set_max_live(Some(1));
        let release = rig.worktrees.hold_next();
        let (tx, mut slow) = oneshot::channel();
        let req = StartSession {
            captain: true,
            isolation: true,
            ..start("claude")
        };
        rig.core
            .handle(Input::Cmd(Cmd::Start {
                req,
                origin: Origin::Agent,
                reply: tx,
            }))
            .await;
        rig.settle().await;
        assert!(slow.try_recv().is_err(), "worktree 还没建好");
        let err = rig
            .cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Agent,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(err, ControlError::AtCap { max: 1 }, "建着的那个也占名额");
        let req = StartSession {
            captain: true,
            ..start("claude")
        };
        let err = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(err, ControlError::CaptainTaken("c1".into()));
        release.send(()).unwrap();
        rig.settle().await;
        let row = slow.await.unwrap().unwrap();
        assert!(row.captain && rig.live(&row.id).is_some());
        assert_eq!(rig.port.lock().sessions.len(), 1, "只建了一行");
    }

    #[tokio::test]
    async fn worktree_failure_fails_the_start_with_the_reason_and_creates_nothing() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.core.gate.set_max_live(Some(1));
        rig.worktrees
            .fail_next("无法为 session 新建 worktree：/x 当前是 detached HEAD");
        let req = StartSession {
            isolation: true,
            ..start("claude")
        };
        let err = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Agent,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ControlError::Worktree("无法为 session 新建 worktree：/x 当前是 detached HEAD".into())
        );
        assert_eq!(
            rig.port.lock().sessions.len(),
            1,
            "没写 session 行（只有 cap）"
        );
        assert!(
            rig.exec.lock().launches.is_empty(),
            "什么都没起，也没退回共享目录"
        );
        assert_eq!(
            rig.core.check_cap(Origin::Agent),
            Ok(()),
            "失败的那次不再占名额"
        );
    }

    #[tokio::test]
    async fn isolation_without_a_crew_directory_is_a_worktree_error_not_a_silent_fallback() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port.lock().crew_dirs.remove("c1");
        let req = StartSession {
            isolation: true,
            ..start("claude")
        };
        let err = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ControlError::Worktree(_)), "{err}");
        assert!(rig.worktrees.calls().is_empty());
        assert_eq!(rig.port.lock().sessions.len(), 1);
    }

    #[tokio::test]
    async fn codex_sessions_are_unsupported_until_m2c_and_nothing_is_created() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let err = rig
            .cmd(|reply| Cmd::Start {
                req: start("codex"),
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ControlError::Unsupported(_)), "{err}");
        assert_eq!(rig.port.lock().sessions.len(), 1, "没建行");
    }

    #[tokio::test]
    async fn starting_a_second_captain_is_refused_clearly_and_creates_nothing() {
        // M3a 的 register 对此回 Conflict；Core 先查机长指针，回 CaptainTaken（map_error → Invalid），不是 Internal。
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let req = StartSession {
            captain: true,
            isolation: true,
            ..start("claude")
        };
        let err = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(err, ControlError::CaptainTaken("c1".into()));
        assert!(rig.worktrees.calls().is_empty(), "没建 worktree");
        assert_eq!(rig.port.lock().sessions.len(), 1, "没建行");
    }

    #[tokio::test]
    async fn optional_cap_refuses_agent_starts_but_never_human_ones() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.core.gate.set_max_live(Some(1));
        let err = rig
            .cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Agent,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(err, ControlError::AtCap { max: 1 });
        assert!(
            rig.cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Human,
                reply
            })
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn working_edges_persist_was_working_and_last_active() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let row = rig.port.row("cap");
        assert!(row.was_working);
        assert_eq!(row.last_active_at, Some(T0));
        rig.emit("cap", SessionEvent::Working(false)).await;
        assert!(!rig.port.row("cap").was_working);
    }

    #[tokio::test]
    async fn exit_clears_the_live_entry_and_publishes_not_running() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", exited(false)).await;
        assert!(rig.live("cap").is_none());
        assert_eq!(rig.port.lock().revoked, ["cap"], "进程没了，令牌作废");
        let last = rig.port.lock().events.last().cloned();
        assert!(
            matches!(last, Some(OrchestraEvent::SessionChanged(v)) if !v.running && v.session_id == "cap")
        );
    }

    #[tokio::test]
    async fn stop_sets_desired_stopped_drops_pending_wakes_and_stops_the_process() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.port
            .enqueue(NewWake {
                session: "cap".into(),
                kind: WakeKind::Timer,
                source: source::timer("t1"),
                priority: Priority::Auto,
                not_before: T0.add(HOUR),
                note: Some("x".into()),
                origin: "cap".into(),
            })
            .await
            .unwrap();
        rig.cmd(|reply| Cmd::Stop {
            session: "cap".into(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(rig.port.row("cap").desired, Desired::Stopped);
        assert_eq!(rig.port.wakes_of("cap")[0].state, WakeState::Dropped);
        assert_eq!(rig.exec.lock().stops, ["cap"]);
    }

    /// 带着一条定时唤醒把 `session` 拉起（唤醒记 inflight，开场正文 = 它），并让它开始干活。
    async fn launch_with_a_wake(rig: &mut Rig, session: &str) {
        rig.port.enqueue(timer_wake(session, "t1")).await.unwrap();
        let wakes = rig.port.pending(session).await.unwrap();
        let exec = rig.core.execs.get("claude").unwrap();
        let row = rig.port.row(session);
        rig.core
            .launch(row, exec, Some("合成定时".into()), wakes, vec![])
            .await
            .unwrap();
        rig.settle().await;
        assert_eq!(rig.port.wakes_of(session)[0].state, WakeState::Inflight);
        rig.emit(session, SessionEvent::Working(true)).await;
        assert!(rig.port.row(session).was_working);
    }

    /// 起会话时带着唤醒（inflight），人在它投进去之前就停掉了：那些唤醒跟 pending 的一样丢掉（裁定 10），
    /// 不能退回 pending——否则「腾出位置」那一拍就把它重新拉起来。驱动层的次序是 Abandoned → Working(false) → Exited。
    #[tokio::test]
    async fn stopping_before_delivery_drops_the_inflight_wakes_too() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        launch_with_a_wake(&mut rig, "w1").await;
        rig.cmd(|reply| Cmd::Stop {
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        assert!(rig.live("w1").is_none());
        let w = &rig.port.wakes_of("w1")[0];
        assert_eq!((w.state, w.attempts), (WakeState::Dropped, 0));
        assert!(!rig.port.row("w1").was_working, "人停的：不算被打断");
        assert!(rig.port.posts().is_empty());
    }

    /// crewd 自己叫停的（停机等，不是人停的）：没投进去的唤醒退回 pending、次数不变（裁定 17：至少一次）；
    /// 驱动层 abandon 发的 `Working(false)` 不能抹掉 `was_working`——下次启动问人时要知道它是被打断的。
    #[tokio::test]
    async fn a_stop_crewd_asked_for_requeues_the_inflight_wakes_and_keeps_was_working() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        launch_with_a_wake(&mut rig, "w1").await;
        rig.core.halt("w1");
        rig.settle().await;
        assert!(rig.live("w1").is_none());
        assert_eq!(rig.exec.lock().stops, ["w1"]);
        let w = &rig.port.wakes_of("w1")[0];
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 0));
        let row = rig.port.row("w1");
        assert!(row.was_working, "被 crewd 打断的仍记着在干活");
        assert_eq!(row.desired, Desired::Running, "crewd 停的不改 desired");
        assert!(rig.port.posts().is_empty());
    }

    /// 撞额度退出不是「投递失败」：带着的唤醒原样退回 pending（额度唤醒另挂），不吃重试次数。
    #[tokio::test]
    async fn a_death_by_usage_limit_requeues_the_inflight_wakes() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        launch_with_a_wake(&mut rig, "w1").await;
        rig.emit("w1", exited(true)).await;
        let w = rig.port.wakes_of("w1");
        assert_eq!((w[0].state, w[0].attempts), (WakeState::Pending, 0));
        assert!(
            w[1].source.starts_with(policy::QUOTA_SOURCE_PREFIX),
            "{w:?}"
        );
    }

    /// 绑定规则：退出时 inflight 的批只在 `on_exited` 里结算；驱动层 abandon 发来的 Abandoned 投递不结算它。
    #[test]
    fn abandoned_deliveries_never_settle_an_inflight_batch() {
        use crewd_exec::session::DeliveryOutcome as O;
        assert!(delivery_settles_inflight(&O::Delivered));
        assert!(delivery_settles_inflight(&O::Undelivered("x".into())));
        assert!(delivery_settles_inflight(&O::Unsubmitted("x".into())));
        assert!(!delivery_settles_inflight(&O::Abandoned("进程退出".into())));
        assert!(
            !delivery_settles_inflight(&O::Blocked("x".into())),
            "非终局"
        );
    }

    /// 预检 P1：人在拉起还没回来时停掉、拉起又失败了——带着的唤醒跟人停一样丢掉：不重试、不告警、不记「起不来」。
    #[tokio::test]
    async fn a_launch_error_after_a_stop_drops_its_wakes_without_retry_or_alert() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        rig.exec.lock().fail_next = vec!["合成：claude 不在".into()];
        let wakes = rig.port.pending("w1").await.unwrap();
        let exec = rig.core.execs.get("claude").unwrap();
        let row = rig.port.row("w1");
        rig.core
            .launch(row, exec, Some("合成定时".into()), wakes, vec![])
            .await
            .unwrap();
        let (tx, stopped) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "w1".into(),
                reply: tx,
            }))
            .await;
        rig.settle().await;
        stopped.await.unwrap().unwrap();
        let w = &rig.port.wakes_of("w1")[0];
        assert_eq!((w.state, w.attempts), (WakeState::Dropped, 0));
        assert!(rig.port.posts().is_empty(), "{:?}", rig.port.posts());
        assert_eq!(rig.port.row("w1").resume_failed_at, None);
    }

    /// 「一个 session 一个进程」不只靠 `resume` 先查：`launch` 自己也拒（Task 8 的唤醒拉起直接调它）。
    #[tokio::test]
    async fn launch_itself_refuses_a_session_that_is_starting_or_live() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let exec = rig.core.execs.get("claude").unwrap();
        let row = rig.port.row("cap");
        rig.core
            .launch(row.clone(), exec.clone(), None, vec![], vec![])
            .await
            .unwrap();
        let err = rig
            .core
            .launch(row.clone(), exec.clone(), None, vec![], vec![])
            .await
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<ControlError>(),
            Some(&ControlError::AlreadyRunning("cap".into())),
            "正在起"
        );
        rig.settle().await;
        assert!(rig.live("cap").is_some());
        assert!(
            rig.core
                .launch(row, exec, None, vec![], vec![])
                .await
                .is_err(),
            "已经在跑"
        );
        rig.settle().await;
        assert_eq!(rig.exec.launches("cap").len(), 1);
    }

    /// 迟到 / 对不上号的 `Launched`（没人在等它）：进程已经起来了，不留着就叫停，不能变成没人管的孤儿。
    #[tokio::test]
    async fn a_launched_handle_nobody_waits_for_is_stopped() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let exec = rig.core.execs.get("claude").unwrap();
        let req = LaunchRequest {
            session_id: "cap".into(),
            crew_id: "c1".into(),
            workdir: PathBuf::from("/nonexistent/wd"),
            model: None,
            effort: None,
            system_prompt: None,
            initial_prompt: None,
            token: "tok".into(),
            ledger: rig.port.ledger("cap"),
        };
        let (handle, _events) = exec.launch(req).await.unwrap();
        rig.core
            .handle(Input::Launched {
                session: "cap".into(),
                generator: 99,
                result: Ok(handle),
            })
            .await;
        assert_eq!(rig.exec.lock().stops, ["cap"]);
        assert!(rig.live("cap").is_none());
    }

    /// 预检 P3：事件流断了却没有 `Exited`（驱动任务没了）——按进程没了处理：不留僵尸 `Live`、不占上限名额，
    /// 带着的唤醒算一次失败（C2）。
    #[tokio::test]
    async fn an_event_stream_that_ends_without_exited_is_an_exit() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        launch_with_a_wake(&mut rig, "w1").await;
        rig.core.gate.set_max_live(Some(1));
        rig.exec.end_stream("w1");
        rig.settle().await;
        assert!(rig.live("w1").is_none(), "没有僵尸 Live");
        assert_eq!(rig.port.lock().revoked, ["w1"]);
        let w = &rig.port.wakes_of("w1")[0];
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 1));
        assert!(
            rig.cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Agent,
                reply
            })
            .await
            .is_ok(),
            "名额腾出来了"
        );
    }

    /// 把 `session` 到期的唤醒带着拉起、让它按 `die` 死掉，一直重复到没有到期的唤醒为止（每轮先等过最长的退避）。
    /// 模拟 Task 8 的「到期就拉起」；返回拉起了几次。
    async fn relaunch_until_nothing_is_due(
        rig: &mut Rig,
        session: &str,
        die: &[SessionEvent],
    ) -> usize {
        let exec = rig.core.execs.get("claude").unwrap();
        for _ in 0..10 {
            rig.tick(3 * MIN).await;
            let due = queue::select_batch(
                &rig.port.pending(session).await.unwrap(),
                rig.clock.now(),
                &BatchLimits::default(),
            );
            if due.is_empty() {
                break;
            }
            let row = rig.port.row(session);
            rig.core
                .launch(row, exec.clone(), Some("合成".into()), due, vec![])
                .await
                .unwrap();
            rig.settle().await;
            for e in die {
                rig.emit(session, e.clone()).await;
            }
        }
        rig.exec.launches(session).len()
    }

    /// 控制者裁定 C2：拉起了、正文还没投进去进程就没了 = 一次失败的投递（次数 +1、按退避回 pending），
    /// 所以重新拉起是有限的；放弃时告警一次。起来即死（`LaunchFailed`）的那次已经告过「起不来」，放弃时不再告。
    #[tokio::test]
    async fn a_session_that_dies_on_every_start_is_launched_a_bounded_number_of_times() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        let die = [
            SessionEvent::Health(Some(Health::new(
                HealthKind::LaunchFailed,
                "合成：起来即死",
            ))),
            crashed(),
        ];
        let n = relaunch_until_nothing_is_due(&mut rig, "w1", &die).await;
        assert_eq!(n, queue::MAX_ATTEMPTS as usize);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "只告一次：{posts:?}");
        assert!(posts[0].1.contains("合成：起来即死"), "{}", posts[0].1);
        assert!(rig.port.row("w1").resume_failed_at.is_some());
    }

    #[tokio::test]
    async fn a_session_that_crashes_on_every_start_gives_up_with_one_delivery_alert() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rename(&rig, "w1", HOSTILE);
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        let n = relaunch_until_nothing_is_due(&mut rig, "w1", &[crashed()]).await;
        assert_eq!(n, queue::MAX_ATTEMPTS as usize);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
        let attempts = queue::MAX_ATTEMPTS.to_string();
        let name = safe_name(HOSTILE);
        let template = text::render(
            text::Lang::Zh,
            "delivery.failed.group",
            &[("name", &name), ("attempts", &attempts), ("error", "\u{0}")],
        );
        let prefix = template.split('\u{0}').next().unwrap();
        assert!(posts[0].1.starts_with(prefix), "{}", posts[0].1);
        assert_neutralised(&posts[0].1, HOSTILE);
    }

    /// 拉起本身报错时带着的唤醒：同样是一次失败（退避回 pending），不能卡在 inflight 里永远不再投。
    #[tokio::test]
    async fn a_launch_error_backs_off_the_wakes_it_carried() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        rig.exec.lock().fail_next = vec!["合成：claude 不在".into()];
        let wakes = rig.port.pending("w1").await.unwrap();
        let exec = rig.core.execs.get("claude").unwrap();
        let row = rig.port.row("w1");
        rig.core
            .launch(row, exec, Some("合成定时".into()), wakes, vec![])
            .await
            .unwrap();
        rig.settle().await;
        let w = &rig.port.wakes_of("w1")[0];
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 1));
        assert_eq!(w.not_before, T0.add(queue::RETRY_DELAYS_MS[0]));
        assert_eq!(rig.port.posts().len(), 1, "只有「起不来」那一条");
    }

    #[tokio::test]
    async fn refused_resume_notifies_queues_a_note_and_counts_as_resume_failure() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.exec
            .lock()
            .transcripts
            .insert("old-1".into(), "/synthetic/p/old-1.jsonl".into());
        rig.run("cap").await;
        let said = "No conversation found with session ID: old-1";
        rig.emit(
            "cap",
            SessionEvent::Spawned {
                pid: 1,
                agent_session_id: "old-1".into(),
                resumed: true,
            },
        )
        .await;
        rig.emit(
            "cap",
            SessionEvent::ResumeRefused {
                id: "old-1".into(),
                said: said.into(),
            },
        )
        .await;
        rig.emit(
            "cap",
            SessionEvent::Spawned {
                pid: 2,
                agent_session_id: "new-1".into(),
                resumed: false,
            },
        )
        .await;
        assert_eq!(
            rig.live("cap").unwrap().agent_session_id,
            "new-1",
            "以最后一条 Spawned 为准"
        );
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1);
        // 控制者裁定 C13：续跑被拒 @机长；这里被拒的就是机长自己 → 绝不 @ 自己，改 @人（同 alert_audience）。
        assert_eq!(posts[0].0, Audience::HumanChannel { crew: "c1".into() });
        assert!(
            posts[0].1.contains(said) && posts[0].1.contains("/synthetic/p/old-1.jsonl"),
            "{}",
            posts[0].1
        );
        let w = rig.port.wakes_of("cap");
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].kind, w[0].source.as_str(), w[0].priority),
            (
                WakeKind::System,
                "system:resume-refused:old-1",
                Priority::System
            )
        );
        assert!(w[0].note.as_deref().unwrap().contains("old-1"));
        assert!(rig.port.row("cap").resume_failed_at.is_some());
        rig.emit("cap", SessionEvent::Working(true)).await;
        assert!(
            rig.port.row("cap").resume_failed_at.is_some(),
            "这次拉起经历过续跑被拒：原对话仍算丢了"
        );
    }

    /// 控制者裁定 C13 + C20：worker 的续跑被拒在大厅 @机长；claude 的原话、名字、路径都过 `safe_name`。
    #[tokio::test]
    async fn a_workers_refused_resume_mentions_the_captain_and_neutralises_external_text() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rename(&rig, "w1", HOSTILE);
        rig.exec
            .lock()
            .transcripts
            .insert("old-1".into(), "/synthetic/p\n[cap]/old-1.jsonl".into());
        rig.run("w1").await;
        rig.emit(
            "w1",
            SessionEvent::ResumeRefused {
                id: "old-1".into(),
                said: HOSTILE.into(),
            },
        )
        .await;
        let posts = rig.port.posts();
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
        assert_neutralised(&posts[0].1, HOSTILE);
        assert!(
            posts[0]
                .1
                .contains(&safe_name("/synthetic/p\n[cap]/old-1.jsonl"))
        );
        let note = rig.port.wakes_of("w1")[0].note.clone().unwrap();
        assert_neutralised(&note, HOSTILE);
    }

    #[tokio::test]
    async fn launch_error_marks_resume_failed_and_alerts_the_owner_once() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.exec.lock().fail_next = vec!["合成：claude 不在".into(), "合成：claude 不在".into()];
        rig.run("w1").await;
        rig.run("w1").await;
        assert!(rig.live("w1").is_none());
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "已经记过就不再告警：{posts:?}");
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
        assert!(posts[0].1.contains("合成：claude 不在"));
    }

    #[tokio::test]
    async fn launch_failure_alert_neutralises_the_name_and_the_detail() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rename(&rig, "w1", HOSTILE);
        rig.exec.lock().fail_next = vec![HOSTILE.into()];
        rig.run("w1").await;
        let text = rig.port.posts()[0].1.clone();
        assert_neutralised(&text, HOSTILE);
        let want = text::render(
            text::Lang::Zh,
            "resume.failed.group",
            &[
                ("name", &safe_name(HOSTILE)),
                ("detail", &safe_name(HOSTILE)),
            ],
        );
        assert_eq!(text, want);
    }

    #[tokio::test]
    async fn captain_launch_failure_goes_to_the_human_channel_never_to_itself() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.exec.lock().fail_next = vec!["合成：起不来".into()];
        rig.run("cap").await;
        assert_eq!(
            rig.port.posts()[0].0,
            Audience::HumanChannel { crew: "c1".into() }
        );
    }

    #[tokio::test]
    async fn dying_with_launch_failed_before_any_work_is_a_resume_failure_and_later_work_clears_it()
    {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit(
            "w1",
            SessionEvent::Health(Some(Health::new(
                HealthKind::LaunchFailed,
                "合成：起来即死",
            ))),
        )
        .await;
        rig.emit("w1", exited(false)).await;
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        assert_eq!(rig.port.row("w1").resume_failed_at, None);
    }

    #[tokio::test]
    async fn quota_without_a_reset_time_schedules_one_fallback_wake_and_says_so() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        for _ in 0..2 {
            rig.emit(
                "w1",
                SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "x"))),
            )
            .await;
        }
        let w = rig.port.wakes_of("w1");
        assert_eq!(w.len(), 1, "已有一条没投的额度唤醒就不再挂");
        assert_eq!(w[0].not_before, T0.add(45 * MIN));
        assert_eq!(
            w[0].note.as_deref(),
            Some(text::get(text::Lang::Zh, "quota.wake"))
        );
        let want = text::render(
            text::Lang::Zh,
            "quota.fallback",
            &[("name", "w1"), ("minutes", "45")],
        );
        assert!(rig.port.posts().iter().any(|(_, t)| *t == want));
    }

    struct ResetIn30;
    impl QuotaSource for ResetIn30 {
        fn reset_at<'a>(&'a self, _e: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>> {
            Box::pin(async move { Some(now.add(30 * MIN)) })
        }
    }

    #[tokio::test]
    async fn quota_with_a_reset_time_fires_a_minute_after_it_and_exit_by_limit_schedules_too() {
        let mut rig = Rig::with(Hooks {
            quota: Arc::new(ResetIn30),
            ..Hooks::default()
        })
        .await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rename(&rig, "w1", HOSTILE);
        rig.run("w1").await;
        rig.emit("w1", exited(true)).await;
        let w = rig.port.wakes_of("w1");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].not_before, T0.add(31 * MIN));
        let name = safe_name(HOSTILE);
        let want = text::render(
            text::Lang::Zh,
            "quota.scheduled",
            &[("name", &name), ("minutes", "31")],
        );
        assert!(
            rig.port.posts().iter().any(|(_, t)| *t == want),
            "{:?}",
            rig.port.posts()
        );
    }

    #[tokio::test]
    async fn worker_decision_goes_to_the_owner_once_then_escalates_to_the_human() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        for _ in 0..2 {
            rig.emit(
                "w1",
                SessionEvent::Decision(DecisionEvent::Appeared(decision("Trust?"))),
            )
            .await;
        }
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "同一指纹不重发");
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
        rig.tick(4 * MIN).await;
        assert_eq!(rig.port.posts().len(), 1);
        rig.tick(MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[1].0, Audience::HumanChannel { crew: "c1".into() });
        rig.tick(10 * MIN).await;
        assert_eq!(rig.port.posts().len(), 2, "只升级一次");
    }

    /// C20：待决策通知里的名字、session、屏幕上抓来的提示与选项都是外来文字，逐个过 `safe_name`。
    #[tokio::test]
    async fn decision_notices_neutralise_the_name_prompt_and_options() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rename(&rig, "w1", HOSTILE);
        rig.run("w1").await;
        let d = PendingDecision {
            prompt: HOSTILE.into(),
            options: vec![HOSTILE.into(), "No".into()],
            numbered: true,
        };
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Appeared(d)))
            .await;
        rig.tick(5 * MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 2);
        for (_, text) in &posts {
            assert_neutralised(text, HOSTILE);
            assert!(
                text.contains(&format!("  1. {}", safe_name(HOSTILE))),
                "{text}"
            );
        }
    }

    #[tokio::test]
    async fn captain_decision_goes_straight_to_the_human_and_is_not_escalated_again() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(decision("Trust?"))),
        )
        .await;
        rig.tick(6 * MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, Audience::HumanChannel { crew: "c1".into() });
    }

    async fn nudge(rig: &mut Rig, session: &str, input: &str) -> Result<(), ControlError> {
        let (s, i) = (session.to_string(), input.to_string());
        rig.cmd(|reply| Cmd::Nudge {
            session: s,
            input: i,
            reply,
        })
        .await
    }

    /// `crewd_wake::notice::nudge_plan` 的契约：键 / 数字逐个单独写（之间隔 `NUDGE_KEY_GAP_MS`），数字后单独一个回车。
    #[tokio::test]
    async fn nudge_writes_keys_one_by_one_with_a_gap_and_needs_a_running_session() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let err = nudge(&mut rig, "cap", "enter").await;
        assert_eq!(err, Err(ControlError::NotRunning("cap".into())));
        rig.run("cap").await;
        nudge(&mut rig, "cap", "enter").await.unwrap();
        nudge(&mut rig, "cap", "2").await.unwrap();
        let st = rig.exec.lock();
        assert_eq!(
            st.raws["cap"],
            [b"\r".to_vec(), b"2".to_vec(), b"\r".to_vec()]
        );
        let at = &st.raw_at["cap"];
        let gap = at[2].duration_since(at[1]);
        assert!(
            gap >= Duration::from_millis(NUDGE_KEY_GAP_MS),
            "数字和回车分两次写、至少隔 {NUDGE_KEY_GAP_MS}ms：{gap:?}"
        );
        assert!(st.sends.is_empty(), "键不走 send");
    }

    /// 预检 P4：同一个 session 的两次 nudge 同时来——各自的键序列不能交错（「1」「回车」「2」「回车」会答错题）。
    #[tokio::test]
    async fn concurrent_nudges_to_one_session_do_not_interleave() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let mut replies = Vec::new();
        for input in ["12", "enter", "继续"] {
            let (tx, rx) = oneshot::channel();
            rig.core
                .handle(Input::Cmd(Cmd::Nudge {
                    session: "cap".into(),
                    input: input.into(),
                    reply: tx,
                }))
                .await;
            replies.push(rx);
        }
        rig.settle().await;
        for rx in replies {
            rx.await.unwrap().unwrap();
        }
        assert_eq!(
            rig.exec.lock().writes["cap"],
            ["raw:1", "raw:2", "raw:\\r", "raw:\\r", "send:继续"],
            "按到达顺序、一次 nudge 写完再下一次"
        );
    }

    #[tokio::test]
    async fn nudge_text_goes_through_send_and_an_empty_one_is_refused() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let err = nudge(&mut rig, "cap", "  ").await.unwrap_err();
        assert!(matches!(err, ControlError::Invalid(_)), "{err}");
        nudge(&mut rig, "cap", "继续\n干").await.unwrap();
        assert_eq!(rig.exec.sent("cap"), ["继续 干"], "去毒后经 send 投递");
        assert!(!rig.exec.lock().raws.contains_key("cap"), "文本不走 raw");
    }

    /// 屏上的选项没有编号时发数字没用（它会被当成文本、回车落在高亮项上）：拒绝，别替人选。
    #[tokio::test]
    async fn digit_nudges_are_refused_when_the_pending_decision_is_not_numbered() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let d = PendingDecision {
            numbered: false,
            ..decision("Trust?")
        };
        rig.emit("cap", SessionEvent::Decision(DecisionEvent::Appeared(d)))
            .await;
        let err = nudge(&mut rig, "cap", "1").await.unwrap_err();
        assert!(matches!(err, ControlError::Invalid(_)), "{err}");
        assert!(!rig.exec.lock().raws.contains_key("cap"));
        nudge(&mut rig, "cap", "enter").await.unwrap();
        assert_eq!(rig.exec.lock().raws["cap"], [b"\r".to_vec()]);
    }

    #[tokio::test]
    async fn set_profile_only_persists_for_the_next_start() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.cmd(|reply| Cmd::SetProfile {
            session: "cap".into(),
            model: Some("opus".into()),
            effort: None,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(rig.port.row("cap").model.as_deref(), Some("opus"));
    }
}
