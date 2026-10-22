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
//!
//! **不变式（inflight 行的主人，C24 / Task 8b 评审 (1)）**：库里每一条 inflight 行恰好被内存里的一批拿着
//! （`Live::inflight` 或 `Starting::batch`），那批的终局只有一次结算（`Port::ack` / `Port::settle`）。所以拿着一批的
//! 路径（`on_exited`、`on_launched`、`send_batch`）里，结算之前的顺带动作（写 session 行、挂额度唤醒、记「起不来」、
//! 发群通知）出错只记日志、不中断——不能因为它们让那批没人结算。**只有重启时的回收（`boot` → `Port::fail_inflight`）
//! 接得走的剩余情形**：① 结算本身出错（`Port::ack` / `Port::settle` 回 `Err` = 库没改，行仍是 inflight、内存里的
//! 那批已经交出去了）；② crewd 被杀 / 崩在 claim 与结算之间（裁定 17，至少一次）；③ actor panic（C25：serve 随之
//! 非零退出，下次启动回收）。运行期没有「没人拿着的 inflight」扫描：①只在库出错时发生，那时扫描也写不进去。
//!
//! S44 exact leases add a bounded retained obligation for a known terminal Failed /
//! stop-exit / launch-error fate whose atomic transaction failed. A later tick or new
//! claim retries that same lease/fate once; persistent errors keep ownership. Confirmed
//! Delivered ack errors retain the prior orphan-to-boot boundary; they are never
//! reclassified as Failed/Dropped merely because the process later exits.
//!
//! **写进会话的先后**：投递、nudge 与回答待决策框都排进 actor 自己的每会话队列 `Live::writes`（[`Write`]），按 actor
//! 收下的先后一个个写（[`Core::pump_writes`]）；回答（一串键）交给执行器的 `ExecHandle::answer`——键之间的间隔与
//! 「每一下之前回读屏幕、框换了就停手」都在执行器里（T10d），编排层**不 sleep**——派生任务只等它的结果，结果以
//! `Input::WriteDone` 回来，队列才往下走。没有派生任务抢锁，先后不靠运行时的调度。
use super::notify::{Key, Kind, Notifier, WINDOW_MS};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_collab::CollabError;
use crewd_collab::refusal::{Code, Refusal};
use crewd_collab::render::safe_name;
use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{
    AnswerOutcome, BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, HeldDraft, SessionEvent,
};
use crewd_store::{StorageKind, StoreError};
use crewd_term::{DecisionEvent, PendingDecision};
use crewd_wake::breaker::{LoopBreaker, SYSTEM};
use crewd_wake::notice::{self, AnswerRefusal, DecisionFacts, NudgePlan};
use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::policy::{self, Gate, LaunchGate};
use crewd_wake::queue;
use crewd_wake::text;
use crewd_wake::{HOUR, Latch, LatchScope, Ms, NewWake, Priority, QueuedWake, WakeKind, source};
use tokio::sync::{mpsc, oneshot, watch};

use super::OrchestraConfig;
use super::clock::Clock;
use super::exec::{
    AnswerSteps, Context, ExecHandle, Executor, Executors, LaunchError, LaunchRequest,
};
use super::hibernate::{HibernateReport, Hibernation};
use super::hooks::Hooks;
use super::port::*;

/// 已结（delivered / dropped）的唤醒行保留多久（排障用 `wake.list` 看得到最近一周；控制者裁定 C6）。
pub(crate) const WAKE_RETENTION_MS: i64 = 7 * 24 * HOUR;
/// 定时拍里一次至多清多少条旧唤醒（S28：积了很多也不一口气删光、把库卡住；清满了下一拍接着清）。
pub(crate) const PRUNE_BATCH: usize = 500;

/// 这次投递回执能不能结算 inflight 的那批：只有终局、且不是 `Abandoned`（那是驱动层停掉 / 退出时的 abandon，
/// 该批由 `on_exited` 按三种情况结算，见模块文档的绑定规则）。
pub(crate) fn delivery_settles_inflight(outcome: &DeliveryOutcome) -> bool {
    outcome.is_final() && !matches!(outcome, DeliveryOutcome::Abandoned(_))
}

/// 停掉一个会话时，写队列里还没轮到的 nudge / 回答当场回「没在跑」（复审 C）；排着的投递留在队列里，
/// 那批随 `inflight` 由 `on_exited` 结算。
pub(crate) fn refuse_queued_nudges(l: &mut Live, session: &str) {
    if let Some((_, reply)) = l.nudge_pending.take() {
        let _ = reply.send(Err(ControlError::NotRunning(session.into())));
    }
    let mut kept = VecDeque::new();
    for job in std::mem::take(&mut l.writes) {
        match job {
            Write::Nudge { reply, .. } => {
                let _ = reply.send(Err(ControlError::NotRunning(session.into())));
            }
            Write::Answer { reply, .. } => {
                reply.send(Err(ControlError::NotRunning(session.into())))
            }
            deliver => kept.push_back(deliver),
        }
    }
    l.writes = kept;
}

/// Nudge command eligibility (not wake readiness). Reused at admission and at the writer head.
fn text_nudge_refusal(session: &str, l: &Live) -> Option<ControlError> {
    if l.decision.is_some() {
        Some(ControlError::Invalid(Refusal::new(
            Code::OrchestraKeysHumanOnly,
        )))
    } else if let Some(draft) = &l.input_draft {
        Some(ControlError::InputDraft {
            session: session.into(),
            text: match draft {
                HeldDraft::Text(t) => Some(t.clone()),
                HeldDraft::Unknown => None,
            },
        })
    } else if l.working {
        Some(ControlError::Invalid(Refusal::new(
            Code::OrchestraNudgeBusy,
        )))
    } else {
        None
    }
}

/// 选屏上框 `d` 的第 `choice` 项（0 起）要按的键（[`notice::answer_plan`] + 键名换字节）。
fn plan_steps(
    session: &str,
    d: &PendingDecision,
    choice: usize,
) -> Result<AnswerSteps, ControlError> {
    let facts = DecisionFacts {
        prompt: d.prompt.clone(),
        options: d.options.clone(),
        numbered: d.numbered,
    };
    match notice::answer_plan(&facts, d.selected, choice) {
        // answer_plan 只说表里有的键名（crewd-wake 的测试钉着）；万一没有就报内部错误，不悄悄少按一下。
        Ok(plan) => plan
            .into_iter()
            .map(|(name, sel)| notice::key_bytes(name).map(|b| (b.to_vec(), sel)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| ControlError::internal("answer_plan named a key with no bytes")),
        Err(AnswerRefusal::OutOfRange) => Err(ControlError::Invalid(
            Refusal::new(Code::DecisionOutOfRange)
                .arg("session", session)
                .arg("options", d.options.len().to_string()),
        )),
        Err(AnswerRefusal::NeedsTerminal) => Err(ControlError::NeedsTerminal(session.into())),
    }
}

/// 在 actor 里等一个挂钩（QuotaSource / AskAnsweredHook / ContextExtras），最多等 `limit`（C25：挂住的挂钩不能
/// 卡住 actor）。到期 = `None`（当它没答），记一行日志。IdleHook 不走这里：它在 actor 之外跑（`Input::Idled`）。
pub(crate) async fn within<T>(
    limit: Duration,
    hook: &str,
    fut: impl std::future::Future<Output = T>,
) -> Option<T> {
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!("crewd: 编排：{hook} 超过 {limit:?} 没答，这次当它没答");
            None
        }
    }
}

/// 下一个「未来的 not_before」的缓存（评审 Fix 3）：actor 每处理一条输入都要算下一拍，不能每次都读库。
/// 改唤醒行的写（入队 / 结算 / 拿走 / 确认 / 回收 / 清理）一律写成 `self.due.touch(&self.port).settle(..)`——
/// 先作废、再写；别人入队的经 `WakeEnqueued` 等协作事件作废（[`Core::handle`]）。缓存的时刻过了也算作废；
/// 缓存满 `max_age`（一个心跳）也算作废（9a 复审：漏了通告的入队、而 actor 一直忙着没有定时拍时，旧缓存最多误事一个心跳）。
#[derive(Default)]
pub(crate) struct DueCache(std::sync::Mutex<Option<(Option<Ms>, Ms)>>);

impl DueCache {
    /// 唤醒行要变了：作废，原样交回 `x`。
    pub(crate) fn touch<'a, T>(&self, x: &'a T) -> &'a T {
        self.clear();
        x
    }

    pub(crate) fn clear(&self) {
        *self.0.lock().expect("缓存锁中毒") = None;
    }

    /// 还有效的缓存（`Some(None)` = 没有未来的行）；没缓存、缓存的时刻已过、或缓存满 `max_age` = `None`（要重读）。
    pub(crate) fn get(&self, now: Ms, max_age: i64) -> Option<Option<Ms>> {
        match *self.0.lock().expect("缓存锁中毒") {
            None => None,
            Some((_, at)) if now.since(at) >= max_age => None,
            Some((Some(t), _)) if t <= now => None,
            Some((v, _)) => Some(v),
        }
    }

    /// `now` = 读库的时刻（缓存从这一刻起算年龄）。
    pub(crate) fn set(&self, v: Option<Ms>, now: Ms) {
        *self.0.lock().expect("缓存锁中毒") = Some((v, now));
    }
}

/// 起会话失败时已建好的 worktree 留着（crewd 永不删 worktree，C8）：路径记进日志，人据此清理。
fn kept_worktree(path: &std::path::Path) {
    eprintln!(
        "crewd: 编排：起会话没成，已建好的 worktree {} 留着、不会被自动删除",
        path.display()
    );
}

/// 会话事件里顺带的投递出错：记日志、不打断这个事件本身的处理（评审 (7)：`session.changed` 照样要发）。
fn log_err(r: anyhow::Result<()>) {
    if let Err(e) = r {
        eprintln!("crewd: 编排：{e:#}");
    }
}

/// 协作层发来的事实（M3a 的 `CollabEvent` = `crewd_collab::Change` 由 `db.rs` 换成这个）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabFact {
    MessageAppended {
        channel: String,
        seq: i64,
    },
    /// 别人（M3a 的问被回答、M3b 的 schedule_wakeup、M4b 的检查点）入了队：试着投。
    WakeEnqueued {
        session: String,
    },
    /// 问被回答（叫醒提问者的唤醒 M3a 已在同一事务里入队）；`cleared` = (工作项, 负责人)，交给 M4b。
    AskAnswered {
        crew: String,
        ask: String,
        cleared: Vec<(String, Option<String>)>,
    },
    /// 这个 session 有了产出（M3a `sessions::note_progress` → `Change::Progress`：工具调用 / 工作项变化；发言不算，C3）。
    Progress {
        session: String,
    },
    /// 订阅落后、漏了事件：按未读重新规划一遍（裁定 16）。
    Lagged,
    /// 人答了重启询问（M3a `restart::answer` 的 `Change::RestartAnswered`）。`resume = false` = 「先不」。
    RestartAnswered {
        prompt: i64,
        resume: bool,
        sessions: Vec<String>,
    },
}

pub enum Input {
    /// Test-only scheduler cut; preserves Core state and queues subsequent inputs.
    /// The release guard must drop even on assertion failure. No public method.
    #[cfg(feature = "testkit")]
    PauseForTest {
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
    Exec {
        session: String,
        generator: u64,
        event: SessionEvent,
    },
    LaunchBinaryReplaced {
        session: String,
        generator: u64,
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
        result: Result<PathBuf, crate::orchestra::worktree::WorktreeError>,
    },
    PublicCodexDone {
        call: crate::public_execution::PublicExecution,
        target: crewd_proto::commands::CommandRuntimeTarget,
        thread: String,
        outcome: crewd_exec::codex::control::CodexOutcome,
    },
    PublicAnswerDone {
        call: crate::public_execution::PublicExecution,
        outcome: Result<AnswerReport, ControlError>,
    },
    PublicWorktreeReady {
        req: StartSession,
        call: crate::public_execution::PublicExecution,
        result: Result<PathBuf, crate::orchestra::worktree::WorktreeError>,
    },
    /// 事件流结束了、却没有 `Exited`（驱动任务崩了 / 被丢了）：当作一次没有退出状态的退出（预检 P3）。
    StreamEnded {
        session: String,
        generator: u64,
    },
    BinaryReplaced {
        crew: String,
    },
    Tick,
    Cmd(Cmd),
    Collab(CollabFact),
    /// Arbiter（Jev）答完了（或超时）。
    Arbitrated {
        msg: MsgFacts,
        ctx: ChannelCtx,
        answer: Option<Vec<String>>,
    },
    /// 这个 session 有了产出（与 `CollabFact::Progress` 同效；测试与 `Handle::note_progress` 用）。
    Progress {
        session: String,
    },
    /// 一串 nudge 键在派生任务里写完了：这个会话的写队列往下走（[`Core::pump_writes`]）。
    WriteDone {
        session: String,
        generator: u64,
    },
    /// 停机（[`Core::begin_shutdown`]）：全部会话都退出之后回执。
    Shutdown(oneshot::Sender<()>),
    /// IdleHook 在派生任务里答完了（或到期，`wake = None`；C25）。`generator` = 问它时那次拉起：会话已经换了 /
    /// 没了 / 正被停掉，答案就作废。
    Idled {
        session: String,
        generator: u64,
        wake: Option<NewWake>,
    },
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
    pub codex_options: Option<crewd_proto::codex::CodexSettingsPatch>,
}

/// collab 规则拒绝的类别（同 `CollabError` 的四个规则变体）：RPC 错误号按它定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusedKind {
    NotFound,
    Invalid,
    Forbidden,
    Conflict,
}

/// 编排拒绝 / 失败的原因。除了 `Storage` / `Internal`，每一个都有稳定的拒绝码（[`ControlError::refusal`]，
/// 文字在 `crewd_collab` 的 `prompts/refusals.{zh,en}.md`）：客户端按码本地化，agent 按设置的语言读文字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlError {
    /// 没有这个 session（id）。
    NotFound(String),
    AlreadyRunning(String),
    NotRunning(String),
    AtCap {
        max: usize,
    },
    /// 这个执行器（`claude` / `codex` / `acp:<id>`）在这台 crewd 上没接上。
    Unsupported(String),
    NoWorkdir(String),
    /// 建 worktree 失败（C8）：原因给人看；什么都没留下、没起、没退回共享目录。
    Worktree(crate::orchestra::worktree::WorktreeError),
    /// `isolation: true` 但机组没有工作目录（C8：不退回共享目录）：`crew.no_workdir`（机组 id）。
    CrewNoWorkdir(String),
    /// 要起机长、但机组已有机长（M3a 的 `sessions::register` 对此回 `Conflict`）。换机长走 `crew.setCaptain` / handoff。
    CaptainTaken(String),
    /// 请求本身不成立（空的 nudge、在没编号的选择框上发数字、设置值不对）。
    Invalid(Refusal),
    /// 要按键回答，但屏上没有待决策框（session id）：`decision.none`。
    NoDecision(String),
    /// 要答的框已经不是屏上那个了（指纹不同、被答掉、高亮被挪了）：`decision.changed`。`now` = 此刻屏上的框。
    DecisionChanged {
        session: String,
        now: Option<PendingDecision>,
    },
    /// 要靠方向键选、但看不出高亮在哪一项（session id）：`decision.needs_terminal`，交给人在终端里选。
    NeedsTerminal(String),
    /// 要打断（`esc` / `interrupt`），但它没在干活、屏上也没有框（session id）：`session.nothing_to_interrupt`。
    NothingToInterrupt(String),
    /// 输入框里有还没发出的草稿：什么都不敲（T10d）。`text` = 人打的字；`None` = 有人在终端里敲过、但分不清框里
    /// 是草稿还是占位提示（fix R3：不把看得见的那行当成人的草稿转述）。
    InputDraft {
        session: String,
        text: Option<String>,
    },
    /// crewd 正在停机 / 休眠：不再起 / 恢复会话。
    BinaryReplaced,
    ShuttingDown,
    /// 机组已被移除（归档，只读；spec §26.5 / §26.7）：不起新 session、不恢复它的 session（机组 id）。
    CrewRemoved(String),
    /// collab 的规则拒绝原样带出来（写 session 行时标题太长、设置值不对……）：码与类别都不变。
    Refused {
        kind: RefusedKind,
        refusal: Refusal,
    },
    /// 存储故障（S26）：给人看的只有类别，原文只进日志。
    Storage {
        kind: StorageKind,
        detail: String,
    },
    /// 别的失败（C14）：原文只进日志，客户端拿到一个编号。
    Internal(String),
}

impl ControlError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }

    /// Port（以及 `Core::launch`）的错误 → 编排的错误：沿错误链找，collab 的规则拒绝原样带出（码不丢，T10a 遗留 2）；
    /// 存储故障归成 `Storage`（S26，同 dispatch 的 `collab_err`）；编排自己拒的（`AlreadyRunning` 等）原样；其余 `Internal`。
    pub fn from_port(e: anyhow::Error) -> Self {
        for cause in e.chain() {
            if let Some(c) = cause.downcast_ref::<ControlError>() {
                return c.clone();
            }
            if let Some(c) = cause.downcast_ref::<CollabError>() {
                let kind = match c {
                    CollabError::NotFound(_) => Some(RefusedKind::NotFound),
                    CollabError::Invalid(_) => Some(RefusedKind::Invalid),
                    CollabError::Forbidden(_) => Some(RefusedKind::Forbidden),
                    CollabError::Conflict(_) => Some(RefusedKind::Conflict),
                    CollabError::Work(_) | CollabError::Store(_) => None,
                };
                if let (Some(kind), Some(r)) = (kind, c.refusal()) {
                    return Self::Refused {
                        kind,
                        refusal: r.clone(),
                    };
                }
                if let CollabError::Store(se) = c
                    && let Some(kind) = se.storage_kind()
                {
                    return Self::Storage {
                        kind,
                        detail: format!("{e:#}"),
                    };
                }
            }
            if let Some(se) = cause.downcast_ref::<StoreError>()
                && let Some(kind) = se.storage_kind()
            {
                return Self::Storage {
                    kind,
                    detail: format!("{e:#}"),
                };
            }
        }
        Self::Internal(format!("{e:#}"))
    }

    /// 稳定的拒绝码 + 参数（`Storage` / `Internal` 没有：它们不是规则拒绝）。
    pub fn refusal(&self) -> Option<Refusal> {
        let r = |code| Refusal::new(code);
        Some(match self {
            Self::NotFound(s) => r(Code::SessionNotFound).arg("session", s),
            Self::AlreadyRunning(s) => r(Code::SessionAlreadyRunning).arg("session", s),
            Self::NotRunning(s) => r(Code::SessionNotRunning).arg("session", s),
            Self::AtCap { max } => r(Code::SessionAtCap).arg("max", max.to_string()),
            Self::Unsupported(executor) => {
                r(Code::SessionExecutorUnavailable).arg("executor", executor)
            }
            Self::NoWorkdir(s) => r(Code::SessionNoWorkdir).arg("session", s),
            Self::Worktree(error) => error.refusal(),
            Self::CrewNoWorkdir(crew) => r(Code::CrewNoWorkdir).arg("crew", crew),
            Self::CaptainTaken(crew) => r(Code::SessionCaptainExists).arg("crew", crew),
            Self::Invalid(refusal) | Self::Refused { refusal, .. } => refusal.clone(),
            Self::NoDecision(s) => r(Code::DecisionNone).arg("session", s),
            Self::DecisionChanged { session, .. } => {
                r(Code::DecisionChanged).arg("session", session)
            }
            Self::NeedsTerminal(s) => r(Code::DecisionNeedsTerminal).arg("session", s),
            Self::NothingToInterrupt(s) => r(Code::SessionNothingToInterrupt).arg("session", s),
            // 草稿是屏上抓来的外来文字：过 safe_name（C20）再进拒绝文字。
            Self::InputDraft {
                session,
                text: Some(text),
            } => r(Code::SessionInputDraft)
                .arg("session", session)
                .arg("text", safe_name(text)),
            Self::InputDraft {
                session,
                text: None,
            } => r(Code::SessionInputDraftUnknown).arg("session", session),
            Self::BinaryReplaced => r(Code::OrchestraBinaryReplaced),
            Self::ShuttingDown => r(Code::OrchestraShuttingDown),
            Self::CrewRemoved(crew) => r(Code::CrewRemoved).arg("crew", crew),
            Self::Storage { .. } | Self::Internal(_) => return None,
        })
    }
}

/// 日志与 RPC message 的缺省：拒绝码的中文文字；存储故障只说类别；内部错误是原文（只进日志，RPC 上换成编号）。
impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self, self.refusal()) {
            (_, Some(r)) => f.write_str(&r.text(crewd_collab::Locale::Zh)),
            (Self::Storage { kind, .. }, None) => f.write_str(kind.describe()),
            (Self::Internal(detail), None) => f.write_str(detail),
            (_, None) => unreachable!("只有 Storage / Internal 没有拒绝码"),
        }
    }
}

impl std::error::Error for ControlError {}

pub type Reply<T> = oneshot::Sender<Result<T, ControlError>>;

/// 回答完的回执（[`Cmd::Answer`]）：键都按下去了；`cleared` = 按完回读时那个框已经不在了（或换成了别的框），
/// `now` = 回读时屏上的框。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerReport {
    pub cleared: bool,
    pub now: Option<PendingDecision>,
}

pub enum Cmd {
    NotificationClaim {
        candidate: Box<crewd_store::notifications::Claimed>,
        reply: Reply<Option<crewd_store::notifications::Claimed>>,
    },
    PublicExecution {
        call: Box<crate::public_execution::PublicExecution>,
        reply: oneshot::Sender<Result<serde_json::Value, crewd_proto::RpcError>>,
    },
    PublicTarget {
        session: String,
        reply: Reply<crewd_proto::commands::CommandRuntimeTarget>,
    },
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
    /// 回答屏上的待决策框、选第 `choice` 项（0 起；T10d，T10c 的 `session.answer` 建在它上面）。`fingerprint` = 调用方
    /// 看到的那个框（`PendingDecision::fingerprint`）：和 actor 此刻知道的不一样就当场回 `DecisionChanged`，什么都不排。
    Answer {
        session: String,
        fingerprint: String,
        choice: usize,
        reply: Reply<AnswerReport>,
    },
    Interrupt {
        session: String,
        reply: Reply<()>,
    },
    RespondRequest {
        session: String,
        fingerprint: String,
        response: serde_json::Value,
        reply: Reply<AnswerReport>,
    },
    Decisions {
        reply: Reply<Vec<crewd_proto::orchestra::PendingDecisionView>>,
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
    SetCaptain {
        crew: String,
        session: String,
        reply: Reply<()>,
    },
    /// `wake.list`：唤醒行 + 每条 pending 为什么还没出去 + 每个目标的近况（[`Core::wake_list`]）。
    Wakes {
        session: Option<String>,
        reply: Reply<super::deliver::WakeList>,
    },
    /// 人解熔断（只有客户端能调）。回 false = 本来就没有这条熔断。
    Recover {
        scope: LatchScope,
        id: String,
        reply: Reply<bool>,
    },
    /// Distinct causal route scope; never reinterpret an Edge/Session latch ID.
    CausalHolds {
        target: Option<String>,
        reply: Reply<Vec<crewd_collab::causal::ChainHold>>,
    },
    /// Capability supplied only by the human client RPC path, once per request.
    RecoverChain {
        chain_id: String,
        from: String,
        target: String,
        action: crewd_collab::causal::HumanRecoveryAction,
        reply: Reply<bool>,
    },
    /// 此刻的环路熔断（`loop.list`）。
    Latches {
        reply: Reply<Vec<Latch>>,
    },
    /// 设置（`settings.get`，S49）。
    Settings {
        reply: Reply<SettingsView>,
    },
    /// 改设置（`settings.set`）：落库，再刷新内存里的语言与启动闸。回改完的设置。
    SetSettings {
        update: SettingsUpdate,
        reply: Reply<SettingsView>,
    },
    /// 一条消息叫醒了谁、走到哪了（`message.receipts`，S54）。
    Receipts {
        message_id: String,
        reply: Reply<Vec<super::deliver::ReceiptRow>>,
    },
    /// 休眠（C26，[`Core::begin_hibernate`]）：`deadline_ms` = 在干活的最多等多久（None = 设置里的；0 = 立即）。
    Hibernate {
        deadline_ms: Option<i64>,
        reply: Reply<HibernateReport>,
    },
    /// 测试用：actor 此刻的下一次定时拍（[`Core::next_deadline`]）。
    #[cfg(test)]
    NextDeadline {
        reply: oneshot::Sender<Ms>,
    },
}

/// 一笔还没回执的投递。`delivery` = `ExecHandle::send` 的编号（拉起时带的正文 = `BRIEF_DELIVERY_ID`）；
/// `None` = 已收下、还排在写队列里没写出去（[`Write::Deliver`]）。
pub struct Inflight {
    pub delivery: Option<u64>,
    /// 这笔投递拿着的行（已 claim 成 inflight，含随批的窗口外行）与送到后的游标推进点（[`Batch`]）。`wakes` 可以是空的：
    /// 只有积压的批（M3a R6）也要投，那时只有 `cursor_targets`。它的终局一律一次 [`Port::ack`]（C24）。
    pub batch: Batch,
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
    /// 执行器报的输入框草稿（`SessionEvent::InputDraft`，T10d）：有 = 它正挡着一笔投递，不再往里送（`input_draft`）。
    pub input_draft: Option<HeldDraft>,
    pub input_draft_since_ms: Option<Ms>,
    /// A body delivery awaiting its exact final receipt; blocks later bodies, not human CAS answers.
    pub nudge_pending: Option<(u64, Reply<()>)>,
    pub inflight: Option<Inflight>,
    /// 人从客户端叫停的（不是停机）。
    pub stopping: bool,
    /// crewd 自己叫停的（停机等，[`Core::halt`]）：不改 desired / was_working，带着的唤醒原样退回。
    pub halting: bool,
    /// 最近一批送进去的唤醒（拉起时带的、或最后一次「送到了」的那批）：休眠记录里「停下时在做什么」（C26）。
    pub last_wakes: Vec<QueuedWake>,
    /// 休眠到期限按过 Esc（C26 / 评审 m1）：之后这一轮停下的 `Working(false)` 不落库——它是被打断的，was_working 留着。
    pub hibernate_cut: bool,
    /// 最近一次投递回执的种类（`DeliveryOutcome` 的变体名，`wake.list` 的近况给人看，R9）。
    pub last_outcome: Option<&'static str>,
    /// 往这个会话里写东西的队列（预检 P4 / 控制者裁定）：投递、nudge 与回答按 actor 收下的先后排进来，
    /// 由 [`Core::pump_writes`] 一个个写；一次回答的整串键按完才轮到下一个，两次写不交错。
    /// 文字 nudge 仅等终局时，后续正文保持排队，允许人类 CAS answer 越过这些正文解决挡住投递的框。
    pub writes: VecDeque<Write>,
    /// An actual answer write is in progress; only its WriteDone releases this fence.
    pub writing: bool,
}

pub enum AnswerReply {
    Legacy(Reply<AnswerReport>),
    Public {
        call: Box<crate::public_execution::PublicExecution>,
        inbox: mpsc::UnboundedSender<Input>,
    },
}
impl AnswerReply {
    pub(crate) fn is_public(&self) -> bool {
        matches!(self, Self::Public { .. })
    }
    pub(crate) fn public_call(&self) -> Option<&crate::public_execution::PublicExecution> {
        match self {
            Self::Public { call, .. } => Some(call),
            _ => None,
        }
    }
    pub(crate) fn send(self, outcome: Result<AnswerReport, ControlError>) {
        match self {
            Self::Legacy(reply) => {
                let _ = reply.send(outcome);
            }
            Self::Public { call, inbox } => {
                let _ = inbox.send(Input::PublicAnswerDone {
                    call: *call,
                    outcome,
                });
            }
        }
    }
}

pub enum AnswerAction {
    Terminal(AnswerSteps),
    Choice(usize),
    Response(serde_json::Value),
}

/// 写队列的一项（[`Live::writes`]）。
pub enum Write {
    /// Agent text nudge: send normally, reply only on matching final Delivery.
    Nudge { text: String, reply: Reply<()> },
    /// 回答待决策框的一串键（`ExecHandle::answer`：执行器每一下之前回读屏幕，框不是 `expect` 就停手）。
    Answer {
        expect: String,
        action: AnswerAction,
        reply: AnswerReply,
        public_target: Option<crewd_proto::commands::CommandRuntimeTarget>,
    },
    /// 一批唤醒的正文：轮到它时 `send`，编号记进 `Live::inflight`（收下时已占好位）。
    Deliver { text: String, extras: Context },
}

pub struct Starting {
    pub generator: u64,
    pub row: SessionRow,
    /// 随开场正文一起送的那批（已 claim；有要投的东西时一定有正文），同 `Inflight::batch`。
    pub batch: Batch,
    /// 人在拉起回来之前叫停了（起来了就停、那批丢掉，同人停）。
    pub stop_requested: bool,
    /// crewd 停机时它还在起（[`Core::begin_shutdown`]）：起来了就 [`Core::halt`]、起不来也一样——那批原样退回，
    /// 不丢（不是人停）、不算失败。
    pub halt_requested: bool,
}

pub struct Raised {
    pub at: Ms,
    pub fingerprint: String,
}

#[derive(Clone)]
struct TerminalObligation {
    batch: Batch,
    fate: TerminalFate,
}
#[derive(Clone)]
enum TerminalFate {
    Settle(Settle),
    Failed {
        error: String,
        now: Ms,
        jitter: f64,
    },
    KnownDelivered {
        riders: Riders,
        now: Ms,
        generation: u64,
        delivery: u64,
    },
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
    pub(crate) public_fences: HashMap<String, super::public_control::RuntimeFence>,
    pub(crate) public_physical_controls:
        HashMap<String, Vec<super::public_control::PendingPhysicalControl>>,
    // Exact terminal evidence survives a failed atomic settlement. This is not wake
    // retry/backoff truth; it only retains the original lease/fate until commit or boot.
    terminal_obligations: HashMap<String, TerminalObligation>,
    pub(crate) gate: LaunchGate,
    pub(crate) notifier: Notifier<P>,
    pub(crate) decisions: HashMap<String, Raised>,
    pub(crate) seen_decisions: HashSet<(String, String)>,
    pub(crate) quota_blocked: HashMap<String, HashSet<String>>,
    /// 重启后等人答「恢复」的 session（§22.7 Q1）：唤醒照常入队，但不被自动拉起（人 @ 它例外，C4）。
    pub(crate) held: BTreeSet<String>,
    /// `boot` 结束时队列里最大的唤醒 id：比它新的人类消息唤醒 = 询问开出之后人 @ 的（C4）。
    pub(crate) held_watermark: i64,
    /// `boot` 开的那条重启询问（M3a `restart::open` 的 id）：协作事件落后（Lagged）时据它从库里重推「答了没有」。
    pub(crate) restart_prompt: Option<i64>,
    pub(crate) breaker: LoopBreaker,
    /// 退避 / 错开产生的「这一刻再看一次」。
    pub(crate) timers: BTreeSet<Ms>,
    /// 已经看过的 `last_progress_at`（`Change::Progress` 丢了时的兜底，裁定 3）。
    pub(crate) seen_progress: HashMap<String, Ms>,
    /// 失败重试的抖动（每批没送到时抽一个；种子见 `OrchestraConfig::jitter_seed`）。
    pub(crate) jitter: queue::Jitter,
    /// 停机闸（控制者裁定 C4：跟回执分开——回执发出去之后闸仍关着）：不投、不拉起、不起 / 恢复会话。
    pub(crate) shutting_down: bool,
    pub(crate) binary_replaced: bool,
    /// 停机回执（全部会话退出后发，[`Core::shutdown_done`]）；可能有好几个人在等。
    pub(crate) shutdown: Vec<oneshot::Sender<()>>,
    /// 上一次清旧唤醒的时刻（`Ms(0)` = 启动后第一拍就清）。
    pub(crate) last_prune: Ms,
    /// 上一次清满了一批：下一拍接着清，不等一小时。
    pub(crate) prune_more: bool,
    /// 下一个未来 not_before 的缓存（[`DueCache`]）。
    pub(crate) due: DueCache,
    /// 休眠进行中（C26，`hibernate.rs`）。
    pub(crate) hibernation: Option<Hibernation>,
    pub(crate) public_owned_hibernate: Vec<super::public_control::PendingOwnedHibernate>,
    /// 上次休眠的记录（`boot` 读进来）：恢复时据它给休眠附言（[`Core::resume_note`]），给过就记「已恢复」。
    pub(crate) hibernated: Option<HibernateRecord>,
    /// 休眠回执交给了等它的 oneshot（不论全退没有）= true：serve（T11）据此先排空在途回复、再走停机次序退出
    /// （语义见 [`super::Handle::exit_requested`]）。actor 结束时它随 Core 一起被丢掉：接收端的 `changed()` 回 `Err`。
    pub(crate) exit: watch::Sender<bool>,
}

/// 此刻的设置（`settings.get`，S49）：语言、可选并发上限、休眠期限（秒）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsView {
    pub lang: text::Lang,
    pub max_live: Option<usize>,
    pub hibernate_deadline_secs: Option<u32>,
}

/// 改设置（`settings.set`）的数据形状在 Port 那边（一个事务写完，[`Port::set_settings`]）。
pub use super::port::SettingsUpdate;

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
        // 给 agent / 群看的文字的语言 = 设置里的（M3a `settings::locale`）；读不出来才退回配置里的，并记一行。
        match port.locale().await {
            Ok(l) => cfg.lang = l,
            Err(e) => eprintln!("crewd: 编排：读语言设置失败，先用 {:?}：{e:#}", cfg.lang),
        }
        // 可选并发上限（§22.7 Q5：缺省不设），存在 M3a 的 `collab::settings`。
        let gate = LaunchGate::new(port.max_live().await?, cfg.stagger_ms);
        // M3a 的 `latches::list` 遇到坏 id / 坏行整表回 `Invalid`——fail-loud，但不能让它把编排起不来；也不能悄悄
        // 当没有：打日志、本次不带旧熔断，坏行要人修（写进交付记录）。
        let latches = match port.latches().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "crewd: 编排：熔断台账读不出来（{e:#}）；本次启动不带旧熔断，wake_latches 里的坏行要人修"
                );
                Vec::new()
            }
        };
        let loops = cfg.loops;
        let seed = cfg.jitter_seed.unwrap_or(clock.now().0 as u64);
        let jitter = queue::Jitter::new(seed, cfg.retry_jitter);
        Ok(Self {
            notifier: Notifier::new(port.clone()),
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
            public_fences: HashMap::new(),
            public_physical_controls: HashMap::new(),
            terminal_obligations: HashMap::new(),
            gate,
            decisions: HashMap::new(),
            seen_decisions: HashSet::new(),
            quota_blocked: HashMap::new(),
            held: BTreeSet::new(),
            held_watermark: 0,
            restart_prompt: None,
            breaker: LoopBreaker::new(loops, latches),
            timers: BTreeSet::new(),
            seen_progress: HashMap::new(),
            jitter,
            shutting_down: false,
            binary_replaced: false,
            shutdown: Vec::new(),
            last_prune: Ms(0),
            prune_more: false,
            due: DueCache::default(),
            hibernation: None,
            public_owned_hibernate: Vec::new(),
            hibernated: None,
            exit: watch::channel(false).0,
        })
    }

    pub async fn handle(&mut self, input: Input) {
        // 别人改了队列（M3a / M3b / M4b 入队、订阅落后）或定时拍：下一拍的缓存作废（[`DueCache`]）。
        if matches!(
            input,
            Input::Tick
                | Input::Collab(
                    CollabFact::WakeEnqueued { .. }
                        | CollabFact::Lagged
                        | CollabFact::AskAnswered { .. }
                        | CollabFact::RestartAnswered { .. }
                )
        ) {
            self.due.clear();
        }
        let r = match input {
            #[cfg(feature = "testkit")]
            Input::PauseForTest { entered, release } => {
                let _ = entered.send(());
                let _ = release.await;
                Ok(())
            }
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
            Input::PublicCodexDone {
                call,
                target,
                thread,
                outcome,
            } => {
                self.finish_public_codex(call, target, thread, outcome)
                    .await;
                Ok(())
            }
            Input::PublicAnswerDone { call, outcome } => {
                self.settle_public_answer(call, outcome).await;
                Ok(())
            }
            Input::PublicWorktreeReady { req, call, result } => {
                self.preparing = self.preparing.saturating_sub(1);
                if req.captain {
                    self.captain_pending.remove(&req.crew_id);
                }
                let outcome = match result {
                    Ok(path) if self.shutting_down => {
                        kept_worktree(&path);
                        Err(ControlError::ShuttingDown)
                    }
                    Ok(path) => {
                        self.finish_public_start(req, Some(path), call.clone())
                            .await
                    }
                    Err(error) => {
                        self.public_worktree_rejected(&call, error).await;
                        return;
                    }
                };
                if let Err(error) = outcome {
                    self.public_start_failed(&call, error).await;
                }
                Ok(())
            }
            Input::StreamEnded { session, generator } => match self.live.get(&session) {
                Some(l) if l.generator == generator => self.on_exited(&session, None).await,
                _ => Ok(()),
            },
            Input::LaunchBinaryReplaced { session, generator } => {
                self.on_binary_launch_rejected(&session, generator).await
            }
            Input::BinaryReplaced { crew } => {
                self.binary_replaced = true;
                self.notifier
                    .raise(
                        Key::new(Kind::BinaryReplaced, "machine"),
                        &crew,
                        text::get(self.lang(), "notice.binary_replaced").into(),
                    )
                    .await
            }
            Input::Tick => self.on_tick().await,
            Input::Cmd(c) => {
                self.on_cmd(c).await;
                Ok(())
            }
            Input::Collab(f) => self.on_collab(f).await,
            Input::Arbitrated { msg, ctx, answer } => {
                self.enqueue_targets(&msg, &ctx, answer.as_deref()).await
            }
            Input::Progress { session } => {
                self.breaker.progress(&session);
                Ok(())
            }
            Input::WriteDone { session, generator } => {
                self.on_write_done(&session, generator).await
            }
            Input::Shutdown(reply) => {
                self.begin_shutdown(reply);
                Ok(())
            }
            Input::Idled {
                session,
                generator,
                wake,
            } => self.on_idled(&session, generator, wake).await,
        };
        if let Err(e) = r {
            eprintln!("crewd: 编排：{e:#}");
        }
        // 休眠中（C26）：每条输入之后看一眼——谁到了安全点、期限到没到、是不是全退了。
        self.hibernate_step().await;
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
            Cmd::PublicExecution { call, reply } => {
                let call = *call;
                if matches!(
                    &call.request.command,
                    crewd_proto::commands::PublicCommand::Hibernate(_)
                ) {
                    self.execute_public_hibernate(call, reply).await;
                } else if super::codex_control::is_command(&call.request.command) {
                    let result = self.execute_public_codex(call).await;
                    let _ = reply.send(result);
                } else {
                    let result = self.execute_public_control(call).await;
                    let _ = reply.send(result);
                }
            }
            Cmd::NotificationClaim{candidate,reply}=>{
                let result=self.claim_notification(*candidate).await;
                let _=reply.send(result);
            }
            Cmd::PublicTarget { session, reply } => {
                let result = self.public_target(&session).await;
                let _ = reply.send(result);
            }
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
                self.advance_public_control(&session);
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
            } => {
                self.advance_public_control(&session);
                if let Err(e) = self.nudge(&session, &input, reply).await {
                    eprintln!("crewd: 编排：{e:#}");
                }
            }
            Cmd::Answer {
                session,
                fingerprint,
                choice,
                reply,
            } => {
                self.advance_public_control(&session);
                if let Err(e) = self.answer(&session, &fingerprint, choice, reply).await {
                    eprintln!("crewd: 编排：{e:#}");
                }
            }
            Cmd::Interrupt { session, reply } => {
                self.advance_public_control(&session);
                let result = match self.writable(&session) {
                    Some(l) if l.working || l.decision.is_some() => l
                        .handle
                        .interrupt()
                        .map_err(|_| ControlError::NotRunning(session.clone())),
                    Some(_) => Err(ControlError::NothingToInterrupt(session.clone())),
                    None => Err(ControlError::NotRunning(session.clone())),
                };
                let _ = reply.send(result);
            }
            Cmd::RespondRequest {
                session,
                fingerprint,
                response,
                reply,
            } => {
                self.advance_public_control(&session);
                if let Err(e) = self
                    .respond_request(&session, &fingerprint, response, reply)
                    .await
                {
                    eprintln!("crewd: 编排：{e:#}");
                }
            }
            Cmd::Decisions { reply } => {
                let mut rows: Vec<_> = self
                    .live
                    .iter()
                    .filter_map(|(session, l)| {
                        l.decision
                            .as_ref()
                            .map(|d| crewd_proto::orchestra::PendingDecisionView {
                                session_id: session.clone(),
                                crew_id: l.crew.clone(),
                                title: l.name.clone(),
                                decision: super::control::decision_view(d),
                                since_ms: self
                                    .decisions
                                    .get(session)
                                    .map_or(self.now().0, |r| r.at.0),
                            })
                    })
                    .collect();
                rows.sort_by(|a, b| a.session_id.cmp(&b.session_id));
                let _ = reply.send(Ok(rows));
            }
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
                    .map_err(ControlError::from_port);
                let _ = reply.send(r);
            }
            Cmd::SetCaptain {
                crew,
                session,
                reply,
            } => {
                let r = self.set_captain(&crew, &session).await;
                let _ = reply.send(r);
            }
            Cmd::Wakes { session, reply } => {
                let r = self
                    .wake_list(session.as_deref())
                    .await
                    .map_err(ControlError::from_port);
                let _ = reply.send(r);
            }
            Cmd::Recover { scope, id, reply } => {
                let r = self.recover(scope, &id).await;
                let _ = reply.send(r);
            }
            Cmd::CausalHolds { target, reply } => {
                let result = self
                    .port
                    .causal_holds(target.as_deref())
                    .await
                    .map_err(ControlError::from_port);
                let _ = reply.send(result);
            }
            Cmd::RecoverChain {
                chain_id,
                from,
                target,
                action,
                reply,
            } => {
                let result = self
                    .port
                    .recover_chain(&chain_id, &from, &target, &action)
                    .await
                    .map_err(ControlError::from_port);
                let _ = reply.send(result);
            }
            Cmd::Latches { reply } => {
                let mut v = self.breaker.latches();
                v.sort_by(|a, b| (a.scope, &a.id).cmp(&(b.scope, &b.id)));
                let _ = reply.send(Ok(v));
            }
            Cmd::Settings { reply } => {
                let r = self.settings().await;
                let _ = reply.send(r);
            }
            Cmd::SetSettings { update, reply } => {
                let r = self.set_settings(update).await;
                let _ = reply.send(r);
            }
            Cmd::Receipts { message_id, reply } => {
                let r = self
                    .receipts(&message_id)
                    .await
                    .map_err(ControlError::from_port);
                let _ = reply.send(r);
            }
            Cmd::Hibernate { deadline_ms, reply } => self.begin_hibernate(deadline_ms, reply).await,
            #[cfg(test)]
            Cmd::NextDeadline { reply } => {
                let _ = reply.send(self.next_deadline().await);
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
        if self.shutting_down {
            let _ = reply.send(Err(ControlError::ShuttingDown));
            return;
        }
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
    pub(crate) async fn check_start(
        &self,
        req: &StartSession,
        origin: Origin,
    ) -> Result<Option<PathBuf>, ControlError> {
        if origin == Origin::Agent && self.cfg.admission.as_ref().is_some_and(|a| !a.any()) {
            return Err(ControlError::ShuttingDown);
        }
        if let Some(patch) = &req.codex_options {
            if req.executor != "codex" {
                return Err(ControlError::Invalid(
                    Refusal::new(Code::ResourceInvalid).arg("field", "codexOptions.executor"),
                ));
            }
            patch.validate().map_err(|e| {
                ControlError::Invalid(Refusal::new(Code::ResourceInvalid).arg("field", e))
            })?;
            if (req.model.is_some() && patch.model.is_some())
                || (req.effort.is_some() && patch.effort.is_some())
            {
                return Err(ControlError::Invalid(
                    Refusal::new(Code::ResourceInvalid)
                        .arg("field", "codexOptions.duplicateProfile"),
                ));
            }
        }
        let exec = self
            .execs
            .get(&req.executor)
            .map_err(|_| ControlError::Unsupported(req.executor.clone()))?;
        self.check_launch(&exec, &req.crew_id, &req.executor)?;
        // 已移除的机组只读（§26.7）：什么都不建、不写、不起。
        if self
            .port
            .crew_removed(&req.crew_id)
            .await
            .map_err(ControlError::from_port)?
        {
            return Err(ControlError::CrewRemoved(req.crew_id.clone()));
        }
        self.check_cap(origin)?;
        // M3a 的 `sessions::register` 对「机组已有机长、又要起一个机长」回 `Conflict`：先查，给一个说得清的错
        // （不然它会在 worktree 建好之后才冒出来，还被包成 Internal）。不建 worktree、不写行、不起进程。
        if req.captain
            && (self.captain_pending.contains(&req.crew_id)
                || self
                    .port
                    .captain(&req.crew_id)
                    .await
                    .map_err(ControlError::from_port)?
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
            .map_err(ControlError::from_port)?
            .ok_or_else(|| ControlError::CrewNoWorkdir(req.crew_id.clone()))?;
        Ok(Some(base))
    }

    async fn on_worktree_ready(
        &mut self,
        req: StartSession,
        reply: Reply<SessionRow>,
        result: Result<PathBuf, crate::orchestra::worktree::WorktreeError>,
    ) {
        self.preparing = self.preparing.saturating_sub(1);
        if req.captain {
            self.captain_pending.remove(&req.crew_id);
        }
        let r = match result {
            // 建 worktree 期间开始停机了：不写行、不起（worktree 留着：crewd 不删，路径记进日志）。
            Ok(path) if self.shutting_down => {
                kept_worktree(&path);
                Err(ControlError::ShuttingDown)
            }
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
        // 之后出错：已建好的 worktree 留着（crewd 不删），路径记进日志；错误本身照常带码上抛。
        let kept = |e: ControlError| {
            if let Some(p) = &worktree {
                kept_worktree(p);
            }
            e
        };
        let exec = self
            .execs
            .get(&req.executor)
            .map_err(|_| kept(ControlError::Unsupported(req.executor.clone())))?;
        self.check_launch(&exec, &req.crew_id, &req.executor)
            .map_err(kept)?;
        // 建 worktree 期间机长指针可能变了（换机长）：再查一次。
        if worktree.is_some()
            && req.captain
            && self
                .port
                .captain(&req.crew_id)
                .await
                .map_err(|e| kept(ControlError::from_port(e)))?
                .is_some()
        {
            return Err(kept(ControlError::CaptainTaken(req.crew_id.clone())));
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
                codex_options: req.codex_options,
                model: req.model,
                effort: req.effort,
                worktree: worktree.clone(),
            })
            .await
            .map_err(|e| kept(ControlError::from_port(e)))?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        if let Some(a) = &self.cfg.admission {
            a.allow(&row.id);
        }
        self.launch(row.clone(), exec, brief, vec![], vec![])
            .await
            .map_err(ControlError::from_port)?;
        Ok(row)
    }

    pub(crate) async fn resume(
        &mut self,
        session: &str,
        origin: Origin,
    ) -> Result<(), ControlError> {
        if self.shutting_down {
            return Err(ControlError::ShuttingDown);
        }
        if origin == Origin::Agent && self.cfg.admission.as_ref().is_some_and(|a| !a.any()) {
            return Err(ControlError::ShuttingDown);
        }
        if self.live.contains_key(session) || self.starting.contains_key(session) {
            return Err(ControlError::AlreadyRunning(session.into()));
        }
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::from_port)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        if row.crew_removed {
            return Err(ControlError::CrewRemoved(row.crew_id));
        }
        if row.executor == "codex"
            && !self
                .port
                .codex_resume_ready(session)
                .await
                .map_err(ControlError::from_port)?
        {
            return Err(ControlError::Invalid(
                Refusal::new(Code::ResourceInvalid).arg("field", "codex.forkUnconfirmed"),
            ));
        }
        let exec = self
            .execs
            .get(&row.executor)
            .map_err(|_| ControlError::Unsupported(row.executor.clone()))?;
        if !exec.accepts_resume() {
            return Err(ControlError::Unsupported(format!(
                "{} managed history/resume unavailable",
                row.executor
            )));
        }
        self.check_launch(&exec, &row.crew_id, &row.executor)?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        self.check_cap(origin)?;
        // 单个恢复一个等人答复的 session（§22.7 Q1）：上次停在干活中途的，开场补一句「被打断」；被休眠停下的换成
        // 休眠附言（C26，只给一次）。
        let text = if self.held.remove(session) {
            self.resume_note(session, row.was_working).await
        } else {
            None
        };
        if let Some(a) = &self.cfg.admission {
            a.allow(session);
        }
        self.launch(row, exec, text, vec![], vec![])
            .await
            .map_err(ControlError::from_port)
    }

    /// 人停掉（裁定 10）：一个事务写停止状态 + 单调 wake-ID 取消水位 + pending 作废，提交后才叫停进程。
    /// inflight 留到真实终局（迟到 Delivered 仍承认）；结算失败再 crash 也按持久取消意图作废，不重放旧工作。
    pub(crate) async fn stop(&mut self, session: &str) -> Result<(), ControlError> {
        if self
            .port
            .session(session)
            .await
            .map_err(ControlError::from_port)?
            .is_none()
        {
            return Err(ControlError::NotFound(session.into()));
        }
        let pending = self
            .due
            .touch(&self.port)
            .human_stop(session)
            .await
            .map_err(ControlError::from_port)?;
        self.revoke_codex_metadata(session);
        if !pending.is_empty() {
            self.port.publish(wake_changed(session, &pending));
        }
        if let Some(a) = &self.cfg.admission {
            a.revoke(session);
        }
        if let Some(l) = self.live.get(session) {
            l.handle.fence_inputs();
        }
        self.held.remove(session);
        if let Some(l) = self.live.get_mut(session) {
            l.stopping = true;
            refuse_queued_nudges(l, session);
            let _ = l.handle.stop();
        } else if let Some(st) = self.starting.get_mut(session) {
            st.stop_requested = true;
        }
        Ok(())
    }

    /// crewd 自己叫停一个在跑的 session（停机，[`Core::begin_shutdown`]）：不改 desired，也不让驱动层 abandon 发来的
    /// `Working(false)` 抹掉 `was_working`——下次启动问人时要知道它是被打断的。带着的唤醒退出时原样退回 pending。
    pub(crate) fn halt(&mut self, session: &str) {
        if let Some(l) = self.live.get_mut(session) {
            l.halting = true;
            refuse_queued_nudges(l, session);
            let _ = l.handle.stop();
        }
    }

    /// 能往里写的那个会话（正在被停掉的——含休眠到期限、按过 Esc 等它停下的——不算：当场回「没在跑」，不排队等
    /// `on_exited`，它可能永远不来）。
    fn writable(&mut self, session: &str) -> Option<&mut Live> {
        self.live
            .get_mut(session)
            .filter(|l| !l.stopping && !l.halting && !l.hibernate_cut)
    }

    /// Agent command policy only; wake readiness remains explain::live_blocker.
    /// Late drafts are still fenced by the executor immediately before writing/submitting.
    async fn nudge(&mut self, session: &str, input: &str, reply: Reply<()>) -> anyhow::Result<()> {
        let Some(l) = self.writable(session) else {
            let _ = reply.send(Err(ControlError::NotRunning(session.into())));
            return Ok(());
        };
        let Some(plan) = notice::nudge_plan(input) else {
            let _ = reply.send(Err(ControlError::Invalid(Refusal::new(
                Code::OrchestraNudgeEmpty,
            ))));
            return Ok(());
        };
        if plan == NudgePlan::HumanOnly || (plan == NudgePlan::Interrupt && l.decision.is_some()) {
            let _ = reply.send(Err(ControlError::Invalid(Refusal::new(
                Code::OrchestraKeysHumanOnly,
            ))));
            return Ok(());
        }
        match plan {
            NudgePlan::Interrupt => {
                let r = if l.working {
                    l.handle
                        .interrupt()
                        .map_err(|_| ControlError::NotRunning(session.into()))
                } else {
                    Err(ControlError::NothingToInterrupt(session.into()))
                };
                let _ = reply.send(r);
            }
            NudgePlan::Text(text) => {
                if let Some(error) = text_nudge_refusal(session, l) {
                    let _ = reply.send(Err(error));
                } else {
                    l.writes.push_back(Write::Nudge { text, reply });
                    return self.pump_writes(session).await;
                }
            }
            NudgePlan::HumanOnly => unreachable!(),
        }
        Ok(())
    }

    /// 回答屏上的待决策框（[`Cmd::Answer`]，T10d）：在跑、框还是调用方看到的那个（否则当场 `DecisionChanged`，什么都不排）、
    /// 按法由 [`notice::answer_plan`] 算；排进写队列，由执行器按（它每一下之前回读屏幕）。
    async fn answer(
        &mut self,
        session: &str,
        fingerprint: &str,
        choice: usize,
        reply: Reply<AnswerReport>,
    ) -> anyhow::Result<()> {
        let Some(l) = self.writable(session) else {
            let _ = reply.send(Err(ControlError::NotRunning(session.into())));
            return Ok(());
        };
        let Some(d) = l.decision.clone() else {
            let _ = reply.send(Err(ControlError::NoDecision(session.into())));
            return Ok(());
        };
        if d.fingerprint() != fingerprint {
            let _ = reply.send(Err(ControlError::DecisionChanged {
                session: session.into(),
                now: Some(d),
            }));
            return Ok(());
        }
        if choice >= d.options.len() {
            let _ = reply.send(Err(ControlError::Invalid(
                Refusal::new(Code::DecisionOutOfRange)
                    .arg("session", session)
                    .arg("options", d.options.len().to_string()),
            )));
            return Ok(());
        }
        let action = match d.kind {
            crewd_term::DecisionKind::RunnerRequest { .. } => AnswerAction::Choice(choice),
            crewd_term::DecisionKind::Terminal => match plan_steps(session, &d, choice) {
                Ok(steps) => AnswerAction::Terminal(steps),
                Err(e) => {
                    let _ = reply.send(Err(e));
                    return Ok(());
                }
            },
        };
        // 执行器核的是调用方看到的那个框（不是 actor 此刻记的）：两道检查各自独立成立。
        l.writes.push_back(Write::Answer {
            expect: fingerprint.to_string(),
            action,
            reply: AnswerReply::Legacy(reply),
            public_target: None,
        });
        self.pump_writes(session).await
    }

    async fn respond_request(
        &mut self,
        session: &str,
        fingerprint: &str,
        response: serde_json::Value,
        reply: Reply<AnswerReport>,
    ) -> anyhow::Result<()> {
        let Some(l) = self.writable(session) else {
            let _ = reply.send(Err(ControlError::NotRunning(session.into())));
            return Ok(());
        };
        let Some(d) = l.decision.clone() else {
            let _ = reply.send(Err(ControlError::NoDecision(session.into())));
            return Ok(());
        };
        if d.fingerprint() != fingerprint {
            let _ = reply.send(Err(ControlError::DecisionChanged {
                session: session.into(),
                now: Some(d),
            }));
            return Ok(());
        }
        if !matches!(d.kind, crewd_term::DecisionKind::RunnerRequest { .. }) {
            let _ = reply.send(Err(ControlError::Unsupported(l.executor.clone())));
            return Ok(());
        }
        l.writes.push_back(Write::Answer {
            expect: fingerprint.into(),
            action: AnswerAction::Response(response),
            reply: AnswerReply::Legacy(reply),
            public_target: None,
        });
        self.pump_writes(session).await
    }

    /// 按收下的先后写这个会话的写队列，直到队列空、或一串键交给了派生任务（它写完以 `Input::WriteDone` 回来）。
    /// 投递（`Write::Deliver`）在 actor 里直接 `send`（不阻塞：驱动层只是收下），编号当场记进 `inflight`，
    /// 所以它的回执不可能先于编号到达。
    pub(crate) async fn pump_writes(&mut self, session: &str) -> anyhow::Result<()> {
        loop {
            let current_public_target = self.current_public_target(session);
            if self
                .public_fences
                .get(session)
                .is_some_and(|fence| fence.manual)
            {
                return Ok(());
            }
            let Some(l) = self.live.get_mut(session) else {
                return Ok(());
            };
            // 正在被停掉的不再往里写（评审 (6)）：排着的投递 / nudge 由退出时的 `on_exited` 统一结算。
            if l.writing || l.stopping || l.halting {
                return Ok(());
            }
            // Waiting for a text receipt is not an active answer writer. Hold all later bodies,
            // but let the first human CAS answer resolve the dialog blocking that receipt.
            // Answers retain their own FIFO order and serialize on `writing` / WriteDone.
            let job = if l.nudge_pending.is_some() {
                l.writes
                    .iter()
                    .position(|job| matches!(job, Write::Answer { .. }))
                    .and_then(|at| l.writes.remove(at))
            } else {
                l.writes.pop_front()
            };
            let Some(job) = job else {
                return Ok(());
            };
            match job {
                Write::Nudge { text, reply } => {
                    if let Some(error) = text_nudge_refusal(session, l) {
                        let _ = reply.send(Err(error));
                        continue;
                    }
                    match l.handle.send(text, &Context::new()) {
                        Ok(id) => {
                            l.nudge_pending = Some((id, reply));
                            // Receipt/id/reply live separately; a late human Answer can now run.
                            continue;
                        }
                        Err(e) => {
                            let error = match e {
                                super::exec::ExecSendError::Send(
                                    crewd_exec::driver::SendError::Closed,
                                ) => ControlError::NotRunning(session.into()),
                                other => ControlError::internal(other),
                            };
                            let _ = reply.send(Err(error));
                        }
                    }
                }
                Write::Answer {
                    expect,
                    action,
                    reply,
                    public_target,
                } => {
                    if let Some(call) = reply.public_call()
                        && (public_target.as_ref() != Some(&current_public_target)
                            || call.recheck_dispatched().await.is_err())
                    {
                        reply.send(Err(ControlError::NotRunning(session.into())));
                        continue;
                    }
                    l.writing = true;
                    let (handle, tx, generator) =
                        (l.handle.clone(), self.inbox.clone(), l.generator);
                    let executor = l.executor.clone();
                    let session = session.to_string();
                    // 不 sleep：键之间的间隔与每一下之前的回读都在执行器里（T10d），这里只等结果。
                    tokio::spawn(async move {
                        let outcome = match action {
                            AnswerAction::Terminal(steps) => handle.answer(expect, steps).await,
                            AnswerAction::Choice(choice) => {
                                handle.answer_choice(expect, choice).await
                            }
                            AnswerAction::Response(response) => {
                                handle.respond_request(expect, response).await
                            }
                        };
                        let r = match outcome {
                            Ok(AnswerOutcome::Answered { cleared, now }) => {
                                Ok(AnswerReport { cleared, now })
                            }
                            Ok(AnswerOutcome::Conflict { now }) => {
                                Err(ControlError::DecisionChanged {
                                    session: session.clone(),
                                    now,
                                })
                            }
                            Ok(AnswerOutcome::NeedsTerminal) if executor == "codex" => {
                                Err(ControlError::internal(
                                    "Codex human response write could not be confirmed; check session state",
                                ))
                            }
                            Ok(AnswerOutcome::NeedsTerminal) => {
                                Err(ControlError::NeedsTerminal(session.clone()))
                            }
                            Ok(AnswerOutcome::Unsupported) => {
                                Err(ControlError::Unsupported(executor))
                            }
                            Err(_) => Err(ControlError::NotRunning(session.clone())),
                        };
                        if reply.is_public() {
                            // Receipt settlement enters Core before releasing the
                            // writer/hibernation exit; no detached DB job races close.
                            reply.send(r);
                            let _ = tx.send(Input::WriteDone { session, generator });
                        } else {
                            let _ = tx.send(Input::WriteDone { session, generator });
                            reply.send(r);
                        }
                    });
                    return Ok(());
                }
                Write::Deliver { text, extras } => {
                    let binding = match l
                        .inflight
                        .as_ref()
                        .and_then(|f| f.batch.lease.as_ref())
                        .and_then(BatchLease::causal_claimed)
                    {
                        Some(claimed) => {
                            let fence = self
                                .port
                                .causal_fence()
                                .ok_or_else(|| anyhow::anyhow!("missing causal fence"))?;
                            Some(fence.binder(&claimed.pending, l.generator)?)
                        }
                        _ => None,
                    };
                    let sent = if let Some(binding) = binding {
                        l.handle.send_bound(text, &extras, binding.as_ref())
                    } else {
                        l.handle.send(text, &extras)
                    };
                    match sent {
                        Ok(id) => {
                            if let Some(f) = l.inflight.as_mut() {
                                f.delivery = Some(id);
                            }
                        }
                        Err(e) => {
                            if let Some(inf) = l.inflight.take() {
                                self.fail_batch(inf.batch, &e.to_string()).await?;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn on_write_done(&mut self, session: &str, generator: u64) -> anyhow::Result<()> {
        let Some(l) = self.live.get_mut(session) else {
            return Ok(());
        };
        if l.generator != generator {
            return Ok(());
        }
        l.writing = false;
        self.pump_writes(session).await
    }

    // —— 拉起

    pub(crate) fn check_launch(
        &self,
        exec: &Arc<dyn Executor>,
        crew: &str,
        executor: &str,
    ) -> Result<(), ControlError> {
        if self.binary_replaced {
            return Err(ControlError::BinaryReplaced);
        }
        exec.check_launch().map_err(|e| match e {
            LaunchError::BinaryReplaced => {
                let _ = self.inbox.send(Input::BinaryReplaced { crew: crew.into() });
                ControlError::BinaryReplaced
            }
            LaunchError::NotConfigured(_) => ControlError::Unsupported(executor.into()),
            other => ControlError::internal(other),
        })
    }

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
        let batch = Batch {
            session: row.id.clone(),
            wakes,
            riders: vec![],
            cursor_targets,
            ..Default::default()
        };
        self.launch_carrying(row, exec, text, batch)
            .await
            .map(|_| ())
    }

    /// 同 [`launch`](Self::launch)，带一整批（含随批的窗口外行 `Batch::riders`）。这批在拉起之前 claim。
    pub(crate) async fn launch_carrying(
        &mut self,
        row: SessionRow,
        exec: Arc<dyn Executor>,
        text: Option<String>,
        mut batch: Batch,
    ) -> anyhow::Result<bool> {
        if self
            .cfg
            .admission
            .as_ref()
            .is_some_and(|a| !a.contains(&row.id))
        {
            return Err(anyhow::anyhow!(
                "session awaits explicit current-run Human admission"
            ));
        }
        // 一个 session 一个进程（裁定 6）：所有拉起路径都经过这里，所以在这里挡，不只靠调用方先查。
        if self.live.contains_key(&row.id) || self.starting.contains_key(&row.id) {
            return Err(ControlError::AlreadyRunning(row.id).into());
        }
        self.retry_terminal(&row.id).await?;
        self.check_launch(&exec, &row.crew_id, &row.executor)?;
        let now = self.now();
        let workdir = row
            .workdir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("session {} 没有工作目录", row.id))?;
        let system_prompt = Some(self.port.worldview(&row.id).await?);
        let initial_context = if text.is_some() && exec.accepts_initial_context() {
            within(
                self.cfg.hook_timeout,
                "ContextExtras",
                self.hooks.extras.extras(&row.id),
            )
            .await
            .unwrap_or_default()
        } else {
            Default::default()
        };
        let generator = self
            .next_generator
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("executor generation exhausted"))?;
        if batch.has_work() {
            batch.lease = match self
                .due
                .touch(&self.port)
                .claim_reserved(&batch, generator)
                .await?
            {
                super::port::Decision::Applied(lease) => Some(lease),
                super::port::Decision::Stale | super::port::Decision::Busy => return Ok(false),
            };
        }
        self.gate.launched(now);
        self.next_generator = generator;
        let token = self.port.mint_token(&row.id);
        let session = row.id.clone();
        let delivery_observer = self
            .port
            .causal_fence()
            .map(|fence| fence.observer(&row.id, generator));
        let registration =
            if let Some(claimed) = batch.lease.as_ref().and_then(BatchLease::causal_claimed) {
                (|| -> anyhow::Result<()> {
                    let fence = self
                        .port
                        .causal_fence()
                        .ok_or_else(|| anyhow::anyhow!("missing causal fence"))?;
                    fence
                        .binder(&claimed.pending, generator)?
                        .register(BRIEF_DELIVERY_ID)?;
                    Ok(())
                })()
            } else {
                Ok(())
            };
        if let Err(error) = registration {
            self.port.revoke_token(&row.id);
            // No Task started: preserve the original claim's requeue obligation on SQL Err.
            self.settle_terminal(batch, Settle::Requeue).await?;
            return Err(error);
        }
        let (title, codex_options) = self.port.codex_launch_options(&row.id).await?;
        let req = LaunchRequest {
            title,
            codex_options,
            session_id: row.id.clone(),
            crew_id: row.crew_id.clone(),
            workdir,
            model: row.model.clone(),
            effort: row.effort.clone(),
            system_prompt,
            initial_prompt: text.clone(),
            initial_context,
            token,
            ledger: self.port.ledger(&row.id),
            delivery_observer,
        };
        self.starting.insert(
            session.clone(),
            Starting {
                generator,
                row,
                batch,
                stop_requested: false,
                halt_requested: false,
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
                Err(LaunchError::BinaryReplaced) => {
                    let _ = tx.send(Input::LaunchBinaryReplaced {
                        session: task_session,
                        generator,
                    });
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
        Ok(true)
    }

    async fn on_binary_launch_rejected(
        &mut self,
        session: &str,
        generator: u64,
    ) -> anyhow::Result<()> {
        if self
            .starting
            .get(session)
            .is_some_and(|s| s.generator == generator)
        {
            let st = self.starting.remove(session).expect("checked");
            self.binary_replaced = true;
            self.port.revoke_token(session);
            let ids = st.batch.ids();
            if !ids.is_empty() || st.batch.lease.is_some() {
                let fate = if st.stop_requested {
                    Settle::Dropped {
                        error: "stopped".into(),
                    }
                } else {
                    Settle::Requeue
                };
                self.settle_terminal(st.batch.clone(), fate).await?;
            }
            self.notifier
                .raise(
                    Key::new(Kind::BinaryReplaced, "machine"),
                    &st.row.crew_id,
                    text::get(self.lang(), "notice.binary_replaced").into(),
                )
                .await?;
            self.publish_live(session).await;
        }
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
        if result.is_err() {
            self.public_physical_lost(session, generator, "launch_failed_without_confirmed_birth")
                .await;
        }
        match result {
            Ok(handle) => {
                self.observe_public_runtime(session, generator);
                if st.stop_requested || st.halt_requested {
                    let _ = handle.stop();
                }
                let inflight = st.batch.has_work().then(|| Inflight {
                    delivery: Some(BRIEF_DELIVERY_ID),
                    batch: st.batch.clone(),
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
                        input_draft: None,
                        input_draft_since_ms: None,
                        nudge_pending: None,
                        inflight,
                        stopping: st.stop_requested,
                        halting: st.halt_requested && !st.stop_requested,
                        last_wakes: st.batch.wakes.clone(),
                        hibernate_cut: false,
                        last_outcome: None,
                        writes: VecDeque::new(),
                        writing: false,
                    },
                );
            }
            Err(_) if st.stop_requested => {
                // 人在拉起回来之前就停掉了：带着的唤醒（含随批行）跟人停一样丢掉（裁定 10）——不重试、不告警、不记「起不来」。
                let held = st.batch.ids();
                if !held.is_empty() || st.batch.lease.is_some() {
                    let s = Settle::Dropped {
                        error: "stopped".into(),
                    };
                    self.settle_terminal(st.batch.clone(), s).await?;
                }
            }
            Err(_) if st.halt_requested => {
                // crewd 停机时它还在起、结果起不来：那批原样退回（停机不是人停、也不算一次失败；起不来的原因多半就是停机）。
                let held = st.batch.ids();
                if !held.is_empty() || st.batch.lease.is_some() {
                    self.settle_terminal(st.batch.clone(), Settle::Requeue)
                        .await?;
                }
            }
            Err(e) => {
                // 先记恢复失败（告警一次），再按失败处理带着的那批：放弃时就不会再告第二次（裁定 9 / C2）。
                // 记不进去只记日志：那批照样结算（模块文档的不变式）。
                log_err(self.launch_failed(&st.row, &e).await);
                self.fail_batch(st.batch, &e).await?;
            }
        }
        self.publish_live(session).await;
        // A wake may arrive while this generation is still starting. An idle
        // resumed thread emits no working-to-idle edge to retry that wake, so
        // drain it now that the handle is ready. Normal delivery gates still
        // enforce inflight ownership and any stop requested during launch.
        log_err(self.try_deliver(session).await.map(|_| ()));
        Ok(())
    }

    /// 起不来（裁定 11）：写 `resume_failed_at`；第一次才告警。
    pub(crate) async fn launch_failed(
        &mut self,
        row: &SessionRow,
        detail: &str,
    ) -> anyhow::Result<()> {
        self.port
            .patch_session(
                &row.id,
                SessionPatch {
                    resume_failed_at: Some(Some(self.now())),
                    ..Default::default()
                },
            )
            .await?;
        let text = text::render(
            self.lang(),
            "resume.failed.group",
            &[
                ("name", &safe_name(&row.name())),
                ("detail", &safe_name(detail)),
            ],
        );
        self.notifier
            .raise(Key::new(Kind::LaunchFailed, &row.id), &row.crew_id, text)
            .await
    }

    /// 一批没送进去（终局失败）：**一次** [`Port::ack`]（C24，全有或全无）——每一行按库里的次数（claim 时记的，
    /// 不是这里手里那份）退避 30s / 2min 回 pending，第 `MAX_ATTEMPTS` 次放弃记 dropped（裁定 9）。
    /// Give-ups of every priority join the crew notification window (T10c).
    /// 控制者裁定 C2 让它在这里先落地（拉起后、投进去之前进程就没了 = 一次失败）；Task 8 的投递失败也走它。
    pub(crate) async fn fail_batch(&mut self, batch: Batch, error: &str) -> anyhow::Result<()> {
        if batch.ids().is_empty() && batch.lease.is_none() {
            return Ok(());
        }
        let session = batch.session.clone();
        if let Some(held) = self.terminal_obligations.get(&session) {
            anyhow::ensure!(
                held.batch.lease == batch.lease && held.batch.ids() == batch.ids(),
                "different terminal batch already retained"
            );
            return self.retry_terminal(&session).await;
        }
        let fate = TerminalFate::Failed {
            error: error.into(),
            now: self.now(),
            jitter: self.jitter.draw(),
        };
        self.remember_terminal(batch, fate)?;
        self.retry_terminal(&session).await
    }

    fn remember_terminal(&mut self, batch: Batch, fate: TerminalFate) -> anyhow::Result<()> {
        if let Some(existing) = self.terminal_obligations.get(&batch.session) {
            anyhow::ensure!(
                existing.batch.lease == batch.lease && existing.batch.ids() == batch.ids(),
                "another exact terminal obligation owns this session"
            );
            return Ok(());
        }
        self.terminal_obligations
            .insert(batch.session.clone(), TerminalObligation { batch, fate });
        Ok(())
    }

    async fn settle_terminal(&mut self, batch: Batch, fate: Settle) -> anyhow::Result<()> {
        let session = batch.session.clone();
        self.remember_terminal(batch, TerminalFate::Settle(fate))?;
        self.retry_terminal(&session).await
    }

    pub(crate) async fn remember_delivered(
        &mut self,
        batch: Batch,
        riders: Riders,
        generation: u64,
        delivery: u64,
    ) -> anyhow::Result<()> {
        let session = batch.session.clone();
        let now = self.now();
        // The physical fate is irreversible even if the trusted ticket is unavailable.
        self.remember_terminal(
            batch,
            TerminalFate::KnownDelivered {
                riders,
                now,
                generation,
                delivery,
            },
        )?;
        self.retry_terminal(&session).await
    }

    fn retire_causal_binding(&self, batch: &Batch) -> anyhow::Result<()> {
        if let Some(claimed) = batch.lease.as_ref().and_then(BatchLease::causal_claimed) {
            self.port
                .causal_fence()
                .ok_or_else(|| anyhow::anyhow!("missing causal fence"))?
                .retire(&claimed.pending)?;
        }
        Ok(())
    }

    pub(crate) fn terminal_sessions(&self) -> Vec<String> {
        self.terminal_obligations.keys().cloned().collect()
    }

    // Exactly one attempt per caller/tick. On Err the original lease and policy remain;
    // never infer a terminal from desired/phase and never clear metadata separately.
    pub(crate) async fn retry_terminal(&mut self, session: &str) -> anyhow::Result<()> {
        let Some(mut held) = self.terminal_obligations.get(session).cloned() else {
            return Ok(());
        };
        // Look up only the already-emitted original ticket. Errors leave KnownDelivered
        // locked; Exit/StreamEnded must not turn actual success into a failed/replayable fate.
        if let TerminalFate::KnownDelivered {
            generation,
            delivery,
            ..
        } = &held.fate
            && let Some((claimed, observed)) = held
                .batch
                .lease
                .as_mut()
                .and_then(BatchLease::causal_witness_mut)
            && observed.is_none()
        {
            let fence = self.port.causal_fence().ok_or_else(|| {
                anyhow::anyhow!("missing causal fence for known physical success")
            })?;
            *observed = Some(Box::new(fence.success(
                &claimed.pending,
                *generation,
                *delivery,
            )?));
            // Keep the immutable witness with the exact debt before entering SQL.
            self.terminal_obligations
                .insert(session.to_string(), held.clone());
        }
        let acked = match &held.fate {
            TerminalFate::Settle(fate) => {
                match self
                    .due
                    .touch(&self.port)
                    .settle_batch(&held.batch, fate.clone(), self.now())
                    .await?
                {
                    super::port::Decision::Applied(()) => {}
                    super::port::Decision::Stale => {
                        self.retire_causal_binding(&held.batch)?;
                        self.terminal_obligations.remove(session);
                        return Ok(()); // retire only old receipt; no row-change success event
                    }
                    super::port::Decision::Busy => {
                        anyhow::bail!("exact terminal settlement is busy; ownership retained")
                    }
                }
                None
            }
            TerminalFate::KnownDelivered { riders, .. } => Some(
                self.due
                    .touch(&self.port)
                    .ack(
                        &held.batch,
                        Ack::Delivered {
                            riders: riders.clone(),
                        },
                    )
                    .await?,
            ),
            TerminalFate::Failed { error, now, jitter } => Some(
                self.due
                    .touch(&self.port)
                    .ack(
                        &held.batch,
                        Ack::Failed {
                            error: error.clone(),
                            now: *now,
                            jitter: *jitter,
                        },
                    )
                    .await?,
            ),
        };
        if acked.as_ref().is_some_and(|a| a.stale) {
            self.retire_causal_binding(&held.batch)?;
            self.terminal_obligations.remove(session);
            return Ok(());
        }
        self.retire_causal_binding(&held.batch)?;
        self.terminal_obligations.remove(session);
        if let Some(acked) = acked {
            if acked.stale {
                return Ok(());
            }
            for (_, retry_at) in &acked.retried {
                self.timers.insert(*retry_at);
            }
            let gave_up: Vec<_> = held
                .batch
                .wakes
                .iter()
                .chain(&held.batch.riders)
                .filter(|w| acked.gave_up.contains(&w.id))
                .cloned()
                .collect();
            if let TerminalFate::Failed { error, .. } = &held.fate {
                self.alert_gave_up(session, &gave_up, error).await?;
            }
            if let TerminalFate::KnownDelivered {
                now, generation, ..
            } = &held.fate
            {
                for origin in &acked.origins {
                    self.breaker.record(session, origin, *now);
                }
                if let Some(l) = self
                    .live
                    .get_mut(session)
                    .filter(|l| l.generator == *generation)
                {
                    l.worked = true;
                    if !held.batch.wakes.is_empty() {
                        l.last_wakes = held.batch.wakes.clone();
                    }
                }
            }
        }
        self.port.publish(wake_changed(
            session,
            held.batch.wakes.iter().chain(&held.batch.riders),
        ));
        Ok(())
    }

    /// 编排自己入队（规划、恢复、换机长、额度、IdleHook……）：入了就发一条 `wake.changed`（S54：客户端看得到「排上了」）。
    /// 返回入没入（撞了去重 / 机组已移除 = false）。
    pub(crate) async fn enqueue(&self, w: NewWake) -> anyhow::Result<bool> {
        let session = w.session.clone();
        let Some(id) = self.due.touch(&self.port).enqueue(w.clone()).await? else {
            return Ok(false);
        };
        let row = QueuedWake {
            id,
            session: w.session,
            kind: w.kind,
            source: w.source,
            priority: w.priority,
            not_before: w.not_before,
            note: w.note,
            state: crewd_wake::WakeState::Pending,
            attempts: 0,
            created_at: self.now(),
            origin: w.origin,
            msg: None,
            last_error: None,
        };
        self.port.publish(wake_changed(&session, [&row]));
        Ok(true)
    }

    /// Give-ups of every priority join a ten-minute crew window; count each given-up wake once.
    pub(crate) async fn alert_gave_up(
        &mut self,
        session: &str,
        gave_up: &[QueuedWake],
        error: &str,
    ) -> anyhow::Result<()> {
        if !queue::should_alert(gave_up) {
            return Ok(());
        }
        let Some(row) = self.port.session(session).await? else {
            return Ok(());
        };
        self.notifier.give_up(
            &row.crew_id,
            session,
            &row.name(),
            gave_up.len(),
            error,
            self.now(),
        );
        Ok(())
    }

    // —— 会话事件

    /// This actor cannot process resume/decision/generation events between the
    /// final in-memory eligibility read and the DB actor's durable claim ACK.
    async fn claim_notification(&self,candidate:crewd_store::notifications::Claimed)->Result<Option<crewd_store::notifications::Claimed>,ControlError>{
        use crewd_proto::notifications::{NotificationSource,NotificationKind};
        let (session,expected,decision)=match &candidate.context.source {
            NotificationSource::CodexTurn{session_id,..}=>(session_id,candidate.session_generation.as_deref(),None),
            NotificationSource::ManagedResponse{session_id,..}=>(session_id,if candidate.context.kind==NotificationKind::ResponseFailed{candidate.session_generation.as_deref()}else{None},None),
            NotificationSource::Decision{session_id,generation,decision_id}=>(session_id,Some(generation.as_str()),Some(decision_id)),
            _=>return Ok(None),
        };
        let Some(live)=self.live.get(session)else{return Ok(None)};
        if live.halting||live.hibernate_cut||expected.is_some_and(|g|g!=self.current_public_target(session).generation){return Ok(None)}
        if let Some(id)=decision {
            if !live.decision.as_ref().is_some_and(|d|matches!(&d.kind,crewd_term::DecisionKind::RunnerRequest{request_id} if request_id.as_str()==id.as_str())){return Ok(None)}
        }else if live.working{return Ok(None)}
        self.port.claim_notification(candidate.delivery_id,candidate.config_revision,crewd_store::now_ms()).await.map_err(ControlError::from_port)
    }

    async fn on_exec(
        &mut self,
        session: &str,
        generator: u64,
        event: SessionEvent,
    ) -> anyhow::Result<()> {
        let now = self.clock.now();
        if self
            .live
            .get(session)
            .is_none_or(|l| l.generator != generator)
        {
            return Ok(());
        }
        self.observe_public_event(session, &event);
        let notification_generation=self.current_public_target(session).generation;
        self.settle_public_physical_event(session, generator, &event)
            .await;
        let l = self
            .live
            .get_mut(session)
            .expect("validated current generation");
        match event {
            SessionEvent::CodexObservation {
                settings,
                title,
                accepted,
            } => {
                self.port
                    .codex_observe(session, settings, title, accepted)
                    .await?;
            }
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
                if on {self.port.notification_change(session,NotificationChange::Clear).await?;}
                let became_idle = l.working && !on;
                l.working = on;
                let mut patch = SessionPatch {
                    // crewd 叫停时驱动层 abandon 发的 Working(false) 不落库：was_working 留着「被打断」。
                    // 休眠按过 Esc 之后的 Working(false) 同理（它是被打断的，C26）。
                    was_working: (on || !(l.halting || l.hibernate_cut)).then_some(on),
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
                if on {
                    let l = &self.live[session];
                    let msg = text::render(
                        self.lang(),
                        "notice.recovered",
                        &[("name", &safe_name(&l.name))],
                    );
                    let executor = l.executor.clone();
                    self.notifier
                        .clear(&Key::new(Kind::LaunchFailed, session), msg.clone())
                        .await?;
                    self.notifier
                        .clear(&Key::new(Kind::BriefUndelivered, session), msg.clone())
                        .await?;
                    // Membership belongs to the episode that registered this session, not its
                    // current role. A demoted captain can recover; an unrelated worker cannot.
                    let recovered_last = self
                        .quota_blocked
                        .get_mut(&executor)
                        .is_some_and(|blocked| blocked.remove(session) && blocked.is_empty());
                    if recovered_last {
                        self.notifier
                            .clear(&Key::new(Kind::Quota, &executor), msg)
                            .await?;
                    }
                }
                if became_idle {
                    log_err(self.on_idle(session).await);
                }
            }
            SessionEvent::Decision(DecisionEvent::Appeared(d)) => {
                l.decision = Some(d.clone());
                if let crewd_term::DecisionKind::RunnerRequest{request_id}=&d.kind {
                    self.port.notification_change(session,NotificationChange::Approval{generation:notification_generation,decision_id:request_id.clone()}).await?;
                }
                self.raise_decision(session, &d, now).await?;
            }
            SessionEvent::Decision(DecisionEvent::Cleared) => {
                l.decision = None;
                self.port.notification_change(session,NotificationChange::ClearApproval).await?;
                self.decisions.remove(session);
                log_err(self.try_deliver(session).await.map(|_| ()));
            }
            SessionEvent::Delivery { id, outcome, .. } => {
                log_err(self.on_delivery(session, id, outcome).await)
            }
            SessionEvent::Item { turn_id, item } => {
                self.port.publish(OrchestraEvent::RunnerItem {
                    session: session.into(),
                    turn_id,
                    item,
                });
            }
            SessionEvent::ManagedResponseFailed{runtime_id,request_id,prompt_id,receipt_revision}=>{
                l.working=false;
                self.port.patch_session(session,SessionPatch{was_working:Some(false),..Default::default()}).await?;
                self.port.notification_change(session,NotificationChange::ManagedFailure{generation:notification_generation,runtime_id,request_id,prompt_id,receipt_revision}).await?;
            }
            SessionEvent::NativeTurnOutcome{thread_id,turn_id,failed,last_agent_message:_}=>{
                self.port.notification_change(session,NotificationChange::CodexOutcome{generation:notification_generation,thread_id,turn_id,failed}).await?;
            }
            SessionEvent::TurnEnded {
                turn_id,
                last_agent_message,
            } => {
                self.port
                    .runner_turn_end(session, &turn_id, &last_agent_message)
                    .await?;
            }
            SessionEvent::Usage(snapshot) => {
                if snapshot.thread_id != l.agent_session_id {
                    return Ok(());
                }
                // Telemetry is persisted only: no live refresh, model wake, or progress.
                return self.port.record_usage(session, snapshot, now).await;
            }
            SessionEvent::Profile(profile) => {
                self.port
                    .patch_session(
                        session,
                        SessionPatch {
                            model: profile.model.clone(),
                            effort: profile.effort.clone(),
                            ..Default::default()
                        },
                    )
                    .await?;
                self.port.publish(OrchestraEvent::RunnerProfile {
                    session: session.into(),
                    profile,
                });
            }
            SessionEvent::Notice(detail) => {
                let row = self.port.session(session).await?;
                if let Some(row) = row {
                    let notice = text::render(
                        self.lang(),
                        "notice.runner",
                        &[("name", &safe_name(&row.name())), ("detail", &detail)],
                    );
                    self.port
                        .post_system(
                            Audience::HumanChannel {
                                crew: row.crew_id,
                                mention_human: false,
                            },
                            notice,
                        )
                        .await?;
                }
            }
            // 执行器挡着一笔投递、输入框里有草稿（T10d）：记下（`input_draft` 挡后面的投递、`session.changed` 带给客户端）；
            // 清掉了就再试一次投递。
            SessionEvent::InputDraft(draft) => {
                let cleared = draft.is_none() && l.input_draft.is_some();
                if l.input_draft.is_none() && draft.is_some() {
                    l.input_draft_since_ms = Some(now);
                }
                if draft.is_none() {
                    l.input_draft_since_ms = None;
                }
                l.input_draft = draft;
                if cleared {
                    let msg = text::render(
                        self.lang(),
                        "notice.recovered",
                        &[("name", &safe_name(&self.live[session].name))],
                    );
                    self.notifier
                        .clear(&Key::new(Kind::InputDraft, session), msg)
                        .await?;
                    log_err(self.try_deliver(session).await.map(|_| ()));
                }
            }
            SessionEvent::Health(h) => {
                let quota = h.as_ref().is_some_and(|h| h.kind.is_quota());
                let cleared = h.is_none();
                let brief = h
                    .as_ref()
                    .is_some_and(|h| h.kind == HealthKind::BriefUndelivered)
                    && !l
                        .health
                        .as_ref()
                        .is_some_and(|h| h.kind == HealthKind::BriefUndelivered);
                l.health = h;
                if brief {
                    self.brief_undelivered(session).await?;
                }
                if quota {
                    self.schedule_quota(session).await?;
                }
                if cleared {
                    log_err(self.try_deliver(session).await.map(|_| ()));
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
        log_err(self.port.notification_change(session,NotificationChange::Clear).await);
        let Some(mut l) = self.live.remove(session) else {
            return Ok(());
        };
        if info.is_none() {
            self.public_physical_lost(
                session,
                l.generator,
                "driver_stream_lost_without_exit_confirmation",
            )
            .await;
        }
        // 还排在写队列里的 nudge / 回答：进程没了，回执「没在跑」（投递的那批随 `inflight` 在下面结算）。
        if l.input_draft.is_some() {
            let msg = text::render(
                self.lang(),
                "notice.recovered",
                &[("name", &safe_name(&l.name))],
            );
            log_err(
                self.notifier
                    .clear(&Key::new(Kind::InputDraft, session), msg)
                    .await,
            );
        }
        if let Some((_, reply)) = l.nudge_pending.take() {
            let _ = reply.send(Err(ControlError::NotRunning(session.into())));
        }
        for job in std::mem::take(&mut l.writes) {
            match job {
                Write::Nudge { reply, .. } => {
                    let _ = reply.send(Err(ControlError::NotRunning(session.into())));
                }
                Write::Answer { reply, .. } => {
                    reply.send(Err(ControlError::NotRunning(session.into())))
                }
                Write::Deliver { .. } => {}
            }
        }
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
        // 顺带动作出错只记日志：下面那批照样要结算（见模块文档的不变式）。
        log_err(self.port.patch_session(session, patch).await);
        let hit_limit = info.as_ref().is_some_and(|i| i.hit_limit);
        let stopped_by_user = info.as_ref().is_some_and(|i| i.stopped_by_user);
        if hit_limit {
            log_err(
                self.schedule_quota_for(session, &l.executor, &l.name, &l.crew)
                    .await,
            );
        }
        // 先记「起不来」（告警一次），再处理带着的唤醒：放弃时就不会再告第二次。
        let launch_failed = l
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed);
        if launch_failed && !l.worked {
            let detail = l
                .health
                .as_ref()
                .map(|h| h.detail.clone())
                .unwrap_or_default();
            match self.port.session(session).await {
                Ok(Some(row)) => log_err(self.launch_failed(&row, &detail).await),
                Ok(None) => {}
                Err(e) => log_err(Err(e)),
            }
        }
        // 还没投进去的那批（绑定规则：只在这里结算，见模块文档），整批一次（含随批的窗口外行——它们跟这批一起 claim 了，
        // C24）：人停的 → 跟 pending 的一样丢掉（裁定 10）；crewd 自己叫停的（停机）或撞额度退出 → 原样退回 pending
        // （裁定 17 至少一次；额度唤醒另挂；退回 claim 那一次）；进程自己没了 → 一次失败的投递，退避、有限次后放弃（C2）。
        if let Some(inf) = l.inflight {
            let held = inf.batch.ids();
            if held.is_empty() && inf.batch.lease.is_none() {
                // 普通启动没有 reservation。
            } else if l.stopping {
                let s = Settle::Dropped {
                    error: "stopped".into(),
                };
                self.settle_terminal(inf.batch.clone(), s).await?;
            } else if l.halting || stopped_by_user || hit_limit {
                self.settle_terminal(inf.batch.clone(), Settle::Requeue)
                    .await?;
            } else {
                let why = match &info {
                    Some(i) => format!("{:?}", i.status),
                    None => "event stream ended without an exit status".to_string(),
                };
                self.fail_batch(inf.batch, &why).await?;
            }
        }
        self.publish_live(session).await;
        // 腾出了位置 / 错开到点：别的 session 排着的唤醒此刻可能投得出去了。
        self.rescan().await
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
        let (crew, name, executor) = (l.crew.clone(), l.name.clone(), l.executor.clone());
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
                Audience::HumanChannel {
                    crew: crew.clone(),
                    mention_human: false,
                },
                format!("{group}\n{diag}"),
            )
            .await?;
        let now = self.now();
        let patch = SessionPatch {
            resume_failed_at: Some(Some(now)),
            ..Default::default()
        };
        self.port.patch_session(session, patch).await?;
        self.enqueue(NewWake {
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
        let captain = self.port.session(session).await?.is_some_and(|r| r.captain);
        if captain {
            self.quota_blocked
                .entry(executor.into())
                .or_default()
                .insert(session.into());
            let msg = text::render(
                self.lang(),
                "notice.quota.captain",
                &[
                    ("name", &safe_name(name)),
                    ("executor", &safe_name(executor)),
                ],
            );
            self.notifier
                .raise(Key::new(Kind::Quota, executor), crew, msg)
                .await?;
        }
        if policy::quota_pending(&self.port.pending(session).await?) {
            return Ok(());
        }
        let now = self.now();
        // 拿不到（含挂钩到期）= None → 45 分钟退避；worker 不发群消息。
        let reset = within(
            self.cfg.hook_timeout,
            "QuotaSource",
            self.hooks.quota.reset_at(executor, now),
        )
        .await
        .flatten();
        let plan = policy::quota_plan(reset, now);
        let lang = self.lang();
        self.enqueue(NewWake {
            session: session.into(),
            kind: WakeKind::System,
            source: plan.source(),
            priority: Priority::System,
            not_before: plan.fire_at,
            note: Some(text::get(lang, "quota.wake").to_string()),
            origin: SYSTEM.into(),
        })
        .await?;
        Ok(())
    }

    async fn brief_undelivered(&mut self, session: &str) -> anyhow::Result<()> {
        let l = &self.live[session];
        let (crew, name, captain) = (l.crew.clone(), l.name.clone(), l.captain);
        let msg = text::render(
            self.lang(),
            "brief.undelivered",
            &[("name", &safe_name(&name))],
        );
        if captain {
            self.notifier
                .raise(Key::new(Kind::BriefUndelivered, session), &crew, msg)
                .await?;
        } else if let Some(captain) = self.port.captain(&crew).await? {
            self.enqueue(NewWake {
                session: captain,
                kind: WakeKind::System,
                source: source::system(&format!("brief-undelivered:{session}:{}", self.now().0)),
                priority: Priority::System,
                not_before: self.now(),
                note: Some(msg),
                origin: SYSTEM.into(),
            })
            .await?;
        }
        Ok(())
    }

    // —— 待决策（裁定 14）

    async fn raise_decision(
        &mut self,
        session: &str,
        d: &PendingDecision,
        now: Ms,
    ) -> anyhow::Result<()> {
        let fingerprint = d.fingerprint();
        let same = self
            .decisions
            .get(session)
            .is_some_and(|r| r.fingerprint == fingerprint);
        if !same {
            self.decisions.insert(
                session.into(),
                Raised {
                    at: now,
                    fingerprint: fingerprint.clone(),
                },
            );
        }
        if self
            .seen_decisions
            .contains(&(session.into(), fingerprint.clone()))
        {
            return Ok(());
        }
        let Some(l) = self.live.get(session) else {
            return Ok(());
        };
        let facts = DecisionFacts {
            prompt: safe_name(&d.prompt),
            options: d.options.iter().map(|o| safe_name(o)).collect(),
            numbered: d.numbered,
        };
        let msg = notice::decision_notice(
            self.lang(),
            &safe_name(&l.name),
            &safe_name(session),
            &facts,
        );
        self.port
            .post_system(
                Audience::HumanChannel {
                    crew: l.crew.clone(),
                    mention_human: true,
                },
                msg,
            )
            .await?;
        self.seen_decisions.insert((session.into(), fingerprint));
        Ok(())
    }

    async fn notify_drafts(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let due: Vec<_> = self
            .live
            .iter()
            .filter(|(_, l)| {
                l.input_draft_since_ms
                    .is_some_and(|t| now.since(t) >= WINDOW_MS)
            })
            .map(|(s, l)| {
                (
                    s.clone(),
                    l.crew.clone(),
                    l.name.clone(),
                    l.input_draft.clone(),
                )
            })
            .collect();
        for (session, crew, name, draft) in due {
            let key = if draft == Some(HeldDraft::Unknown) {
                "session.input_draft_unknown"
            } else {
                "notice.input_draft"
            };
            let msg = text::render(self.lang(), key, &[("name", &safe_name(&name))]);
            self.notifier
                .raise(Key::new(Kind::InputDraft, &session), &crew, msg)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn on_tick(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        self.timers.retain(|t| *t > now);
        self.notify_drafts().await?;
        self.notifier.flush(now, self.lang()).await?;
        // 默认方案到点（控制者裁定 C7）与清旧唤醒（C6 / S28）出错只记日志：不能挡住下面的重扫。
        if self.cfg.admission.as_ref().is_some_and(|a| !a.any()) {
            return Ok(());
        }
        log_err(self.port.apply_due_defaults().await);
        log_err(self.port.enqueue_work_checkpoints().await);
        log_err(self.prune(now).await);
        self.rescan().await
    }

    /// 清掉保留期之前就已结的唤醒：最多一小时一次（启动后第一拍就清）；一次至多 [`PRUNE_BATCH`] 条，清满了下一拍接着清。
    async fn prune(&mut self, now: Ms) -> anyhow::Result<()> {
        if !self.prune_more && now.since(self.last_prune) < HOUR {
            return Ok(());
        }
        self.last_prune = now;
        let n = self
            .due
            .touch(&self.port)
            .prune_wakes(Ms(now.0 - WAKE_RETENTION_MS), PRUNE_BATCH)
            .await?;
        self.prune_more = n >= PRUNE_BATCH;
        if n > 0 {
            eprintln!("crewd: 编排：清掉 {n} 条已结的旧唤醒");
        }
        Ok(())
    }

    // —— 状态面

    pub(crate) async fn live_view(&self, session: &str) -> Result<LiveView, ControlError> {
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::from_port)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        let l = self.live.get(session);
        Ok(LiveView {
            session_id: session.into(),
            running: l.is_some(),
            working: l.is_some_and(|l| l.working),
            health: l.and_then(|l| l.health.clone()),
            pending_decision: l.and_then(|l| l.decision.clone()),
            awaiting_question: row.awaiting_question,
            input_draft: l.and_then(|l| l.input_draft.clone()),
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

    use super::*;
    use crate::orchestra::exec::BoxFut;
    use crate::orchestra::hooks::QuotaSource;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_collab::render::safe_name;
    use crewd_exec::health::Health;
    use crewd_exec::session::ExitStatusKind;
    use crewd_wake::queue;
    use crewd_wake::{HOUR, MIN, SEC, WakeState};

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
            kind: crewd_term::DecisionKind::Terminal,
            prompt: prompt.into(),
            options: vec!["Yes".into(), "No".into()],
            numbered: true,
            selected: Some(0),
        }
    }

    #[tokio::test]
    async fn usage_evidence_rejects_old_runner_generation_and_wrong_thread_without_progress() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let live = rig.live("cap").unwrap();
        let generation = live.generator;
        let thread = live.agent_session_id.clone();
        let sample = crewd_exec::session::RunnerUsageSnapshot {
            provider: "codex".into(),
            thread_id: thread,
            turn_id: None,
            token_usage_json: "{}".into(),
        };
        let before = rig.port.row("cap").last_progress_at;
        rig.core
            .on_exec("cap", generation + 1, SessionEvent::Usage(sample.clone()))
            .await
            .unwrap();
        let mut wrong = sample.clone();
        wrong.thread_id = "old-provider-thread".into();
        rig.core
            .on_exec("cap", generation, SessionEvent::Usage(wrong))
            .await
            .unwrap();
        assert!(rig.port.lock().runner_usage.is_empty());
        rig.core
            .on_exec("cap", generation, SessionEvent::Usage(sample.clone()))
            .await
            .unwrap();
        assert_eq!(rig.port.lock().runner_usage.len(), 1);
        assert_eq!(rig.port.lock().runner_usage[0].1, sample);
        assert_eq!(rig.port.row("cap").last_progress_at, before);
    }

    #[tokio::test]
    async fn codex_confirmed_profile_and_runner_notice_are_consumed() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit(
            "cap",
            SessionEvent::Profile(crewd_exec::session::Profile {
                model: Some("actual-model".into()),
                effort: Some("high".into()),
                fast: Some(true),
            }),
        )
        .await;
        assert_eq!(rig.port.row("cap").model.as_deref(), Some("actual-model"));
        assert_eq!(rig.port.row("cap").effort.as_deref(), Some("high"));
        rig.emit("cap", SessionEvent::Profile(Default::default()))
            .await;
        assert_eq!(
            rig.port.row("cap").model.as_deref(),
            Some("actual-model"),
            "unknown is not clearing the confirmed model"
        );
        rig.emit("cap", SessionEvent::Notice("Runner refused a form".into()))
            .await;
        assert!(
            rig.port
                .lock()
                .posts
                .iter()
                .any(|(_, text)| text.contains("Runner refused a form")),
            "runner notice must be visible"
        );
        let before = rig.port.lock().posts.len();
        let old_generation = rig.live("cap").unwrap().generator + 1;
        rig.core
            .on_exec(
                "cap",
                old_generation,
                SessionEvent::Notice("stale notice".into()),
            )
            .await
            .unwrap();
        rig.core
            .on_exec(
                "cap",
                old_generation,
                SessionEvent::Profile(crewd_exec::session::Profile {
                    model: Some("stale".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(rig.port.lock().posts.len(), before);
        assert_eq!(rig.port.row("cap").model.as_deref(), Some("actual-model"));
    }

    #[tokio::test]
    async fn codex_items_and_turn_end_keep_identity_and_reject_old_generation() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let item = serde_json::json!({"id":"item-original", "type":"agentMessage", "text":"Need your choice?"});
        rig.emit(
            "cap",
            SessionEvent::Item {
                turn_id: "turn-original".into(),
                item: item.clone(),
            },
        )
        .await;
        assert!(rig.port.lock().events.iter().any(|event| matches!(event,
            OrchestraEvent::RunnerItem { session, turn_id, item: body } if session == "cap" && turn_id == "turn-original" && body == &item)));
        rig.emit(
            "cap",
            SessionEvent::TurnEnded {
                turn_id: "turn-original".into(),
                last_agent_message: "Need your choice?".into(),
            },
        )
        .await;
        assert_eq!(
            rig.port.lock().runner_ends,
            vec![(
                "cap".into(),
                "turn-original".into(),
                "Need your choice?".into()
            )]
        );
        assert_eq!(
            rig.port.row("cap").awaiting_question.as_deref(),
            Some("Need your choice?")
        );
        let generation = rig.live("cap").unwrap().generator + 1;
        let count = rig.port.lock().events.len();
        rig.core
            .on_exec(
                "cap",
                generation,
                SessionEvent::Item {
                    turn_id: "old".into(),
                    item,
                },
            )
            .await
            .unwrap();
        rig.core
            .on_exec(
                "cap",
                generation,
                SessionEvent::TurnEnded {
                    turn_id: "old".into(),
                    last_agent_message: "Old question?".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(rig.port.lock().events.len(), count);
        assert_eq!(rig.port.lock().runner_ends.len(), 1);
    }

    fn start(executor: &str) -> StartSession {
        StartSession {
            codex_options: None,

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

    /// Task 8b 评审 (1)：进程自己没了时，写 session 行之类的顺带动作出错，带着的那批照样结算（一次失败、退避），
    /// 不留下没人拿着的 inflight 行（那种行只有重启时的回收接得走）。
    #[tokio::test]
    async fn a_port_error_at_exit_does_not_orphan_the_inflight_batch() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        rig.tick(SEC).await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Inflight);
        rig.port.lock().patch_error = Some("合成：session 行写不进去".into());
        rig.emit("w1", crashed()).await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Pending, 1),
            "按一次失败结算了"
        );
        assert_eq!(w.not_before, T0.add(SEC + 30 * SEC));
    }

    /// 同上，拉起报错时：记「起不来」出错，带着的那批照样按一次失败结算。
    #[tokio::test]
    async fn a_port_error_while_recording_a_launch_failure_does_not_orphan_the_batch() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.exec.lock().fail_next.push("合成：起不来".into());
        rig.port.lock().patch_error = Some("合成：session 行写不进去".into());
        rig.port.enqueue(timer_wake("w1", "t1")).await.unwrap();
        rig.tick(SEC).await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Pending, 1),
            "按一次失败结算了"
        );
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

    struct ReplacedExecutor;
    impl Executor for ReplacedExecutor {
        fn check_launch(&self) -> Result<(), LaunchError> {
            Err(LaunchError::BinaryReplaced)
        }
        fn launch(
            &self,
            _: LaunchRequest,
        ) -> super::super::exec::BoxFut<'_, Result<super::super::exec::Launched, LaunchError>>
        {
            panic!("identity refusal must never launch")
        }
    }

    #[tokio::test]
    async fn t11_identity_admission_refusal_raises_the_machine_notice() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.core.execs = Executors::new().with("claude", Arc::new(ReplacedExecutor));
        assert_eq!(
            rig.core.resume("cap", Origin::Human).await,
            Err(ControlError::BinaryReplaced)
        );
        rig.settle().await;
        assert!(
            rig.core.binary_replaced,
            "admission must schedule the machine notice and close future launches"
        );
        assert_eq!(rig.port.row("cap").resume_failed_at, None);
    }

    #[tokio::test]
    async fn t11_binary_replacement_refuses_resume_before_claim_or_failure() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.core
            .handle(Input::BinaryReplaced { crew: "c1".into() })
            .await;
        assert_eq!(
            rig.core.resume("cap", Origin::Human).await,
            Err(ControlError::BinaryReplaced)
        );
        assert!(rig.exec.launches("cap").is_empty());
        assert_eq!(rig.port.row("cap").resume_failed_at, None);
        assert_eq!(
            super::super::rpc::rpc_err(ControlError::BinaryReplaced)
                .data
                .map(|d| d.refusal),
            Some("orchestra.binary_replaced".into())
        );
    }

    #[tokio::test]
    async fn t11_replacement_during_launch_requeues_without_additional_failure() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port
            .enqueue(NewWake {
                session: "cap".into(),
                kind: WakeKind::Timer,
                source: source::timer("replacement"),
                priority: Priority::Auto,
                not_before: T0,
                note: Some("synthetic".into()),
                origin: "cap".into(),
            })
            .await
            .unwrap();
        let wakes = rig.port.pending("cap").await.unwrap();
        let row = rig.port.row("cap");
        let exec = rig.core.execs.get("claude").unwrap();
        launch_rendered_wakes(&mut rig, row, exec, wakes).await;
        let generator = rig.core.starting["cap"].generator;
        assert_eq!(
            rig.port.wakes_of("cap")[0].attempts,
            1,
            "the batch is claimed"
        );
        rig.core
            .handle(Input::LaunchBinaryReplaced {
                session: "cap".into(),
                generator,
            })
            .await;
        let after = rig.port.wakes_of("cap")[0].clone();
        assert_eq!(after.state, crewd_wake::WakeState::Pending);
        assert_eq!(
            after.attempts, 0,
            "no process started: shared Requeue refunds claim; no failure is counted"
        );
        assert_eq!(rig.port.row("cap").resume_failed_at, None);
        assert!(rig.core.starting.is_empty());
        assert!(rig.core.binary_replaced);
        rig.settle().await;
    }

    #[tokio::test]
    async fn t11_replacement_race_preserves_human_stop_cancellation() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port
            .enqueue(NewWake {
                session: "cap".into(),
                kind: WakeKind::Timer,
                source: source::timer("replacement"),
                priority: Priority::Auto,
                not_before: T0,
                note: Some("synthetic".into()),
                origin: "cap".into(),
            })
            .await
            .unwrap();
        let wakes = rig.port.pending("cap").await.unwrap();
        let row = rig.port.row("cap");
        let exec = rig.core.execs.get("claude").unwrap();
        launch_rendered_wakes(&mut rig, row, exec, wakes).await;
        let generator = rig.core.starting["cap"].generator;
        assert_eq!(
            rig.port.wakes_of("cap")[0].attempts,
            1,
            "the batch is claimed"
        );
        let (reply, response) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "cap".into(),
                reply,
            }))
            .await;
        assert_eq!(response.await.unwrap(), Ok(()));
        rig.core
            .handle(Input::LaunchBinaryReplaced {
                session: "cap".into(),
                generator,
            })
            .await;
        let after = rig.port.wakes_of("cap")[0].clone();
        assert_eq!(after.state, crewd_wake::WakeState::Dropped);
        assert_eq!(
            after.attempts, 1,
            "human cancellation is neither retry nor refund"
        );
        assert_eq!(rig.port.row("cap").resume_failed_at, None);
        assert!(rig.core.starting.is_empty());
        assert!(rig.core.binary_replaced);
        rig.settle().await;
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
            codex_options: None,

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
            codex_options: None,

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
            .fail_next(crate::orchestra::worktree::WorktreeError::new(
                crate::orchestra::worktree::WorktreeReason::Detached,
                "/x",
                "",
            ));
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
            ControlError::Worktree(crate::orchestra::worktree::WorktreeError::new(
                crate::orchestra::worktree::WorktreeReason::Detached,
                "/x",
                ""
            ))
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
        // 修复轮 M3：带码（`crew.no_workdir`），不再是一段中文原因的 Worktree。
        assert_eq!(err, ControlError::CrewNoWorkdir("c1".into()), "{err}");
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
            codex_options: None,

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
    /// 唤醒一小时后才到点：退回 pending 之后，Task 8 的「退出即重扫」不会当场把它重新拉起来（这几条测的是退出时
    /// 怎么结算；停机时不再拉起是 Task 9 停机闸的事）。
    async fn launch_rendered_wakes(
        rig: &mut Rig,
        row: SessionRow,
        exec: Arc<dyn Executor>,
        wakes: Vec<QueuedWake>,
    ) {
        let rendered = rig.port.render_wake(&row.id, &wakes).await.unwrap();
        let batch = Batch {
            session: row.id.clone(),
            wakes,
            riders: vec![],
            cursor_targets: rendered.cursor_targets,
            prepared: rendered.prepared,
            lease: None,
        };
        assert!(
            rig.core
                .launch_carrying(row, exec, Some(rendered.text), batch)
                .await
                .unwrap()
        );
    }

    // R4 exercises the actual terminal caller, not retry_terminal's return value.
    // The message and its unread window exist in MemPort before real render/claim.
    async fn terminal_receipt_event_observation(
        stale: bool,
    ) -> (Vec<OrchestraEvent>, OrchestraEvent) {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let message_id = "c1/lobby#1".to_string();
        let at = crewd_wake::MessageRef {
            channel: "c1/lobby".into(),
            seq: 1,
        };
        rig.port.lock().messages.insert(
            (at.channel.clone(), at.seq),
            (
                MsgFacts {
                    provenance: crewd_wake::plan::MessageProvenance::Ordinary,
                    id: message_id.clone(),
                    at,
                    sender: crewd_wake::plan::Sender::Human,
                    mentions: vec![],
                    created_at: T0,
                },
                "synthetic terminal receipt message".into(),
            ),
        );
        rig.port
            .enqueue(NewWake {
                session: "cap".into(),
                kind: WakeKind::Message,
                source: source::message(&message_id),
                priority: Priority::Human,
                not_before: T0,
                note: None,
                origin: "human".into(),
            })
            .await
            .unwrap();
        let wakes = rig.port.pending("cap").await.unwrap();
        let row = rig.port.row("cap");
        let exec = rig.core.execs.get("claude").unwrap();
        launch_rendered_wakes(&mut rig, row, exec, wakes).await;
        let starting = &rig.core.starting["cap"];
        let generator = starting.generator;
        let batch = starting.batch.clone();
        assert!(batch.lease.is_some());
        assert_eq!(batch.cursor_targets, [("c1/lobby".into(), 1)]);
        let expected = wake_changed("cap", batch.wakes.iter().chain(&batch.riders));
        assert!(
            matches!(&expected, OrchestraEvent::WakeChanged {wake_ids, message_ids, ..}
            if !wake_ids.is_empty() && message_ids == &[message_id])
        );
        if stale {
            // A real, exact successful settlement retires this lease; the actor's
            // subsequently arriving terminal receipt must now be genuinely Stale.
            assert_eq!(
                rig.port
                    .settle_batch(&batch, Settle::Requeue, T0)
                    .await
                    .unwrap(),
                super::super::port::Decision::Applied(())
            );
            assert_eq!(rig.port.wakes_of("cap")[0].state, WakeState::Pending);
        }
        rig.port.lock().events.clear(); // exclude prepare/claim/admission observations
        rig.core
            .handle(Input::LaunchBinaryReplaced {
                session: "cap".into(),
                generator,
            })
            .await;
        let changes: Vec<_> = rig
            .port
            .lock()
            .events
            .iter()
            .filter(|e| matches!(e, OrchestraEvent::WakeChanged { .. }))
            .cloned()
            .collect();
        assert_eq!(rig.port.wakes_of("cap")[0].state, WakeState::Pending);
        assert_eq!(rig.port.wakes_of("cap")[0].attempts, 0);
        assert!(rig.core.terminal_obligations.is_empty());
        // Reap the delayed StandIn launch via its stale-generation handler,
        // then finish the normal actor shutdown before the behavioral assertion.
        rig.settle().await;
        let (done, complete) = oneshot::channel();
        rig.core.handle(Input::Shutdown(done)).await;
        rig.settle().await;
        assert!(rig.core.shutdown_done());
        complete.await.unwrap();
        assert!(rig.core.starting.is_empty());
        assert!(rig.core.live.is_empty());
        (changes, expected)
    }

    #[tokio::test]
    async fn s44_applied_terminal_receipt_publishes_one_actual_wake_change() {
        let (changes, expected) = terminal_receipt_event_observation(false).await;
        assert_eq!(changes, [expected], "Applied must publish exactly once");
    }

    #[tokio::test]
    async fn s44_stale_terminal_receipt_publishes_no_actual_wake_change() {
        let (changes, _) = terminal_receipt_event_observation(true).await;
        assert!(changes.is_empty(), "Stale changed no rows: {changes:?}");
    }

    async fn launch_with_a_wake(rig: &mut Rig, session: &str) {
        let wake = NewWake {
            not_before: T0.add(HOUR),
            ..timer_wake(session, "t1")
        };
        rig.port.enqueue(wake).await.unwrap();
        let wakes = rig.port.pending(session).await.unwrap();
        let exec = rig.core.execs.get("claude").unwrap();
        let row = rig.port.row(session);
        launch_rendered_wakes(rig, row, exec, wakes).await;
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
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Dropped, 1),
            "claim 记的那次留着（C24）"
        );
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
        launch_rendered_wakes(&mut rig, row, exec, wakes).await;
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
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Dropped, 1),
            "claim 记的那次留着（C24）"
        );
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
            codex_options: None,
            title: None,

            delivery_observer: None,
            session_id: "cap".into(),
            crew_id: "c1".into(),
            workdir: PathBuf::from("/nonexistent/wd"),
            model: None,
            effort: None,
            system_prompt: None,
            initial_prompt: None,
            initial_context: Default::default(),
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

    /// 等过最长的退避（定时拍 → Task 8 的重扫把到期的唤醒带着拉起），让它按 `die` 死掉，一直重复到不再被拉起为止。
    /// 返回拉起了几次。
    async fn relaunch_until_nothing_is_due(
        rig: &mut Rig,
        session: &str,
        die: &[SessionEvent],
    ) -> usize {
        for _ in 0..10 {
            rig.tick(3 * MIN).await;
            if rig.live(session).is_none() {
                break;
            }
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
        rig.tick(10 * MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(
            posts[0].0,
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: true
            }
        );
        assert!(posts[0].1.contains(&safe_name(HOSTILE)));
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
        launch_rendered_wakes(&mut rig, row, exec, wakes).await;
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
        assert_eq!(
            posts[0].0,
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: false
            }
        );
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
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: false
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

    /// 日志在盘上找不到时，群里那条诊断说「找不到」（`resume.refused.diag.missing`）。
    #[tokio::test]
    async fn a_refused_resume_without_a_transcript_says_it_is_missing() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit(
            "w1",
            SessionEvent::ResumeRefused {
                id: "old-2".into(),
                said: "No conversation found".into(),
            },
        )
        .await;
        let text = rig.port.posts()[0].1.clone();
        assert!(
            text.ends_with(&format!(
                "\n{}",
                text::get(text::Lang::Zh, "resume.refused.diag.missing")
            )),
            "{text}"
        );
    }

    /// 单个恢复一个等重启答复的 session（§22.7 Q1）：上次停在干活中途的开场补「被打断」；没在干活的不补。
    #[tokio::test]
    async fn resuming_a_held_session_that_was_working_opens_with_the_interrupted_note() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.port.lock().sessions.get_mut("w1").unwrap().was_working = true;
        rig.core.held.insert("w1".into());
        rig.core.held.insert("cap".into());
        rig.run("w1").await;
        rig.run("cap").await;
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some(text::get(text::Lang::Zh, "restart.interrupted"))
        );
        assert_eq!(rig.exec.launches("cap")[0].initial, None, "没在干活：不补");
        assert!(rig.core.held.is_empty());
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
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: true
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
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: true
            }
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
        assert!(rig.port.posts().is_empty(), "worker quota is health only");
    }

    struct ResetIn30;
    impl QuotaSource for ResetIn30 {
        fn reset_at<'a>(&'a self, _e: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>> {
            Box::pin(async move { Some(now.add(30 * MIN)) })
        }
    }

    #[tokio::test]
    async fn s46_worldview_polled_for_start_and_resume_launch() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let expected_id = format!("new-{}", rig.port.lock().next_session + 1);
        let first = "S46 distinct start worldview";
        rig.port
            .lock()
            .worldview_responses
            .insert(expected_id.clone(), first.into());
        let row = rig
            .cmd(|reply| Cmd::Start {
                req: start("claude"),
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap();
        let first_calls = rig.port.lock().worldview_calls.clone();
        let first_prompts: Vec<_> = rig
            .exec
            .launches(&row.id)
            .iter()
            .map(|r| r.system_prompt.clone())
            .collect();
        rig.cmd(|reply| Cmd::Stop {
            session: row.id.clone(),
            reply,
        })
        .await
        .unwrap();
        let fully_stopped = rig.live(&row.id).is_none() && !rig.core.starting.contains_key(&row.id);
        let second = "S46 distinct resumed worldview";
        rig.port
            .lock()
            .worldview_responses
            .insert(row.id.clone(), second.into());
        rig.run(&row.id).await;
        let resumed_calls = rig.port.lock().worldview_calls.clone();
        let resumed_prompts: Vec<_> = rig
            .exec
            .launches(&row.id)
            .iter()
            .map(|r| r.system_prompt.clone())
            .collect();
        let again = rig
            .cmd(|reply| Cmd::Resume {
                session: row.id.clone(),
                origin: Origin::Human,
                reply,
            })
            .await;
        let final_calls = rig.port.lock().worldview_calls.clone();
        let final_launches = rig.exec.launches(&row.id).len();
        rig.cmd(|reply| Cmd::Stop {
            session: row.id.clone(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(row.id, expected_id);
        assert!(fully_stopped, "Resume follows completed actual Stop");
        assert_eq!(
            first_calls.as_slice(),
            std::slice::from_ref(&row.id),
            "S46 worldview polled call"
        );
        assert_eq!(first_prompts, [Some(first.into())]);
        assert_eq!(resumed_calls, [row.id.clone(), row.id.clone()]);
        assert_eq!(resumed_prompts, [Some(first.into()), Some(second.into())]);
        assert_eq!(again, Err(ControlError::AlreadyRunning(row.id.clone())));
        assert_eq!(final_calls, resumed_calls);
        assert_eq!(final_launches, 2);
    }

    #[derive(Default)]
    struct S46Quota(std::sync::Mutex<Vec<(String, Ms)>>);
    impl QuotaSource for S46Quota {
        fn reset_at<'a>(&'a self, executor: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>> {
            Box::pin(async move {
                self.0.lock().unwrap().push((executor.into(), now));
                Some(now.add(17 * MIN))
            })
        }
    }

    #[tokio::test]
    async fn s46_quota_polled_reset_controls_actual_pending_wake() {
        let quota = Arc::new(S46Quota::default());
        let mut rig = Rig::with(Hooks {
            quota: quota.clone(),
            ..Hooks::default()
        })
        .await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        for _ in 0..2 {
            rig.emit(
                "w1",
                SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "S46"))),
            )
            .await;
        }
        let calls = quota.0.lock().unwrap().clone();
        let wakes = rig.port.wakes_of("w1");
        // Capture the pending episode before human Stop legitimately drops it.
        rig.cmd(|reply| Cmd::Stop {
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            calls,
            [("claude".into(), T0)],
            "S46 QuotaSource polled call"
        );
        assert_eq!(wakes.len(), 1);
        let w = &wakes[0];
        assert_eq!(w.session, "w1");
        assert_eq!(w.kind, WakeKind::System);
        assert_eq!(w.priority, Priority::System);
        assert_eq!(w.state, WakeState::Pending);
        assert_eq!(w.not_before, T0.add(18 * MIN));
        assert_eq!(w.source, format!("quota:{}", T0.add(18 * MIN).0));
        assert_eq!(w.origin, SYSTEM);
        assert_eq!(
            w.note.as_deref(),
            Some(text::get(text::Lang::Zh, "quota.wake"))
        );
        assert!(
            rig.exec.sent("w1").is_empty(),
            "future wake has not been sent early"
        );
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
        assert!(rig.port.posts().is_empty(), "worker quota is health only");
    }

    #[tokio::test]
    async fn t10c_worker_box_is_human_only_once_without_escalation() {
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
        rig.tick(6 * MIN).await;
        assert_eq!(rig.port.posts().len(), 1, "no escalation");
        assert_eq!(rig.port.lock().posted[0].0, "c1/human");
        assert_eq!(rig.port.lock().posted_mentions, [vec!["human".to_string()]]);
    }

    #[tokio::test]
    async fn t10c_agent_never_answers_a_box_or_sends_keys() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(decision("Trust?"))),
        )
        .await;
        for input in ["1", "enter", "up", "down", "esc", "interrupt", "free text"] {
            let r = nudge(&mut rig, "cap", input).await;
            assert!(matches!(r, Err(ControlError::Invalid(_))), "{input}: {r:?}");
        }
        assert!(!rig.exec.lock().answers.contains_key("cap"));
        assert!(rig.exec.sent("cap").is_empty());
    }

    #[tokio::test]
    async fn t10c_text_nudge_waits_for_matching_final_delivery() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let (tx, mut rx) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "cap".into(),
                input: "hello".into(),
                reply: tx,
            }))
            .await;
        assert!(
            matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "queued is not delivered"
        );
        let id = rig.exec.last_send_id("cap");
        rig.emit(
            "cap",
            SessionEvent::Delivery {
                id: id + 1,
                outcome: DeliveryOutcome::Delivered,
                subject: crewd_exec::delivery::Subject::Steer,
            },
        )
        .await;
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        rig.emit(
            "cap",
            SessionEvent::Delivery {
                id,
                outcome: DeliveryOutcome::Delivered,
                subject: crewd_exec::delivery::Subject::Steer,
            },
        )
        .await;
        assert_eq!(rx.await.unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn worker_decision_goes_to_the_human_once_without_escalation() {
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
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: true
            }
        );
        rig.tick(4 * MIN).await;
        assert_eq!(rig.port.posts().len(), 1);
        rig.tick(11 * MIN).await;
        assert_eq!(rig.port.posts().len(), 1, "不升级");
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
            kind: crewd_term::DecisionKind::Terminal,
            prompt: HOSTILE.into(),
            options: vec![HOSTILE.into(), "No".into()],
            numbered: true,
            selected: Some(0),
        };
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Appeared(d)))
            .await;
        rig.tick(5 * MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1);
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
        assert_eq!(
            posts[0].0,
            Audience::HumanChannel {
                crew: "c1".into(),
                mention_human: true
            }
        );
    }

    async fn nudge(rig: &mut Rig, session: &str, input: &str) -> Result<(), ControlError> {
        let (cmd, rx) = nudge_cmd(session, input);
        rig.core.handle(cmd).await;
        if rig.live(session).is_some_and(|l| l.nudge_pending.is_some()) {
            rig.ack(session).await;
        }
        rig.settle_until(rx).await
    }

    /// T10d：按键只用来答屏上的待决策框，经执行器的安全按键（`expect` = 收下时的框）。数字 = 选第 N 项（编号框上只按那个
    /// 数字，不补回车）；键名原样按一下；屏上没有框 → `decision.none`；没有框、在干活时的 esc = 打断这一轮。
    #[tokio::test]
    async fn key_nudges_answer_the_box_on_screen_through_the_executor() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        assert_eq!(
            nudge(&mut rig, "cap", "enter").await,
            Err(ControlError::NotRunning("cap".into()))
        );
        rig.run("cap").await;
        assert_eq!(
            nudge(&mut rig, "cap", "enter")
                .await
                .unwrap_err()
                .refusal()
                .unwrap()
                .code,
            Code::OrchestraKeysHumanOnly
        );
        assert_eq!(
            nudge(&mut rig, "cap", "esc").await,
            Err(ControlError::NothingToInterrupt("cap".into()))
        );
        rig.emit("cap", SessionEvent::Working(true)).await;
        nudge(&mut rig, "cap", "esc").await.unwrap();
        assert_eq!(rig.exec.lock().interrupts, ["cap"]);
    }

    fn nudge_cmd(
        session: &str,
        input: &str,
    ) -> (Input, oneshot::Receiver<Result<(), ControlError>>) {
        let (tx, rx) = oneshot::channel();
        let cmd = Cmd::Nudge {
            session: session.into(),
            input: input.into(),
            reply: tx,
        };
        (Input::Cmd(cmd), rx)
    }

    /// 预检 P4 + 控制者裁定（Task 8）：写进同一个会话的东西按 actor 收下的先后写——一次回答的整串键按完才轮到下一个，
    /// 投递排在它收下时的位置（T10d：回答也在这条队里，`an_answer_waits_behind_a_delivery_in_the_fifo`）。多线程运行时：
    /// 先后不靠调度运气。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_answer_waits_behind_a_delivery_in_the_fifo() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        // 多线程下拉起的派生任务可能还没把 `Launched` 发回来：等到它真的在跑。
        while rig.live("w1").is_none() {
            let input = rig.rx.recv().await.expect("拉起要回来");
            rig.core.handle(input).await;
        }
        let first_box = decision("Trust?");
        rig.exec.emit(
            "w1",
            SessionEvent::Decision(DecisionEvent::Appeared(first_box.clone())),
        );
        while rig.live("w1").is_some_and(|l| l.decision.is_none()) {
            let input = rig.rx.recv().await.expect("框要到");
            rig.core.handle(input).await;
        }
        // 回答 1 在执行器里按着（挂住）：框答掉之后来的 @ 排在它后面。
        let (release, held) = oneshot::channel();
        rig.exec.lock().answer_hold.insert("w1".into(), held);
        let (tx, r1) = oneshot::channel();
        let first = Input::Cmd(Cmd::Answer {
            session: "w1".into(),
            fingerprint: first_box.fingerprint(),
            choice: 1,
            reply: tx,
        });
        rig.core.handle(first).await;
        // 等执行器真的接到回答 1（它当下就按，结果挂着）：之后框才被答掉。
        while !rig.exec.lock().answers.contains_key("w1") {
            tokio::task::yield_now().await;
        }
        rig.exec
            .emit("w1", SessionEvent::Decision(DecisionEvent::Cleared));
        while rig.live("w1").is_some_and(|l| l.decision.is_some()) {
            let input = rig.rx.recv().await.expect("清框要到");
            rig.core.handle(input).await;
        }
        let at = vec![crewd_wake::plan::Mention::Session("w1".into())];
        rig.post("c1/lobby", Rig::sender("cap"), at, "合成派活甲")
            .await;
        assert!(rig.exec.sent("w1").is_empty(), "投递排在回答 1 后面");
        // 第二个框来了：回答 2 排在投递后面，文字 nudge 排在最后。
        let second_box = decision("Run it?");
        rig.exec.emit(
            "w1",
            SessionEvent::Decision(DecisionEvent::Appeared(second_box.clone())),
        );
        while rig.live("w1").is_some_and(|l| l.decision.is_none()) {
            let input = rig.rx.recv().await.expect("框要到");
            rig.core.handle(input).await;
        }
        let (tx, r2) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Answer {
                session: "w1".into(),
                fingerprint: second_box.fingerprint(),
                choice: 0,
                reply: tx,
            }))
            .await;
        let (third, r3) = nudge_cmd("w1", "继续");
        rig.core.handle(third).await;
        assert!(rig.exec.sent("w1").is_empty(), "回答 1 还没按完，谁都不动");
        let _ = release.send(());
        rig.settle_until(r1).await.unwrap();
        let report = rig.settle_until(r2).await.unwrap();
        assert_eq!(report.now.map(|d| d.prompt), Some("Run it?".into()));
        assert_eq!(
            rig.settle_until(r3)
                .await
                .unwrap_err()
                .refusal()
                .unwrap()
                .code,
            Code::OrchestraKeysHumanOnly
        );
        assert_eq!(
            rig.exec.lock().writes["w1"],
            ["answer:2", "send:合成派活甲", "answer:1"],
            "按收下的先后、一次回答按完再下一个"
        );
    }

    // This exercises the upstream executor-future contract only. StandIn does not model a
    // kernel PTY write: the public SessionHandle/driver tests own actual WouldBlock coverage.
    async fn held_answer_result_preserves_the_body_fence(
        outcome: AnswerOutcome,
    ) -> Result<AnswerReport, ControlError> {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let (old_body, old_reply) = nudge_cmd("cap", "合成旧正文");
        rig.core.handle(old_body).await;
        let old_id = rig.exec.last_send_id("cap");
        let d = decision("Synthetic pending answer?");
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        )
        .await;
        let (release, held) = oneshot::channel();
        {
            let mut st = rig.exec.lock();
            st.answer_hold.insert("cap".into(), held);
            st.answer_outcome.insert("cap".into(), outcome);
        }
        let (reply, mut answer_reply) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Answer {
                session: "cap".into(),
                fingerprint: d.fingerprint(),
                choice: 0,
                reply,
            }))
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !rig.exec.lock().answers.contains_key("cap") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the executor must enter the held answer future");
        assert!(matches!(
            answer_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(rig.live("cap").unwrap().writing);
        // A cleared screen and the old body's final receipt are independent of this answer.
        rig.emit("cap", SessionEvent::Decision(DecisionEvent::Cleared))
            .await;
        let (body, body_reply) = nudge_cmd("cap", "合成后续正文");
        rig.core.handle(body).await;
        rig.emit(
            "cap",
            SessionEvent::Delivery {
                id: old_id,
                subject: crewd_exec::delivery::Subject::Steer,
                outcome: DeliveryOutcome::Delivered,
            },
        )
        .await;
        rig.settle_until(old_reply).await.unwrap();
        assert!(
            rig.live("cap").unwrap().writing,
            "old receipt is not WriteDone"
        );
        let generator = rig.live("cap").unwrap().generator;
        rig.core
            .handle(Input::WriteDone {
                session: "cap".into(),
                generator: generator.wrapping_sub(1),
            })
            .await;
        assert!(
            rig.live("cap").unwrap().writing,
            "another generation cannot release"
        );
        assert_eq!(rig.exec.sent("cap"), ["合成旧正文"]);
        assert!(
            !rig.exec.lock().raws.contains_key("cap"),
            "injected result presses no keys"
        );
        release.send(()).expect("held executor is still waiting");
        // Receive and apply the actual derived WriteDone, rather than synthesizing a release.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let input = rig.rx.recv().await.expect("answer task sends WriteDone");
                let done = matches!(
                    &input,
                    Input::WriteDone { session, generator: g } if session == "cap" && *g == generator
                );
                rig.core.handle(input).await;
                if done {
                    break;
                }
            }
        })
        .await
        .expect("bounded executor completion must reach the actor");
        let result = answer_reply
            .await
            .expect("answer returns its typed outcome");
        assert!(!rig.live("cap").unwrap().writing);
        assert_eq!(rig.exec.sent("cap"), ["合成旧正文", "合成后续正文"]);
        rig.ack("cap").await;
        rig.settle_until(body_reply).await.unwrap();
        rig.settle().await;
        assert_eq!(rig.exec.sent("cap"), ["合成旧正文", "合成后续正文"]);
        result
    }

    #[tokio::test]
    async fn needs_terminal_answer_waits_for_write_done_and_returns_the_stable_rpc_refusal() {
        let err = held_answer_result_preserves_the_body_fence(AnswerOutcome::NeedsTerminal)
            .await
            .unwrap_err();
        assert_eq!(err, ControlError::NeedsTerminal("cap".into()));
        assert_eq!(err.refusal().unwrap().code, Code::DecisionNeedsTerminal);
        let wire = super::super::rpc::rpc_err(err);
        assert_eq!(wire.code, crewd_proto::RpcError::CONFLICT);
        assert_eq!(wire.data.unwrap().refusal, "decision.needs_terminal");
    }

    #[tokio::test]
    async fn answered_executor_result_preserves_the_same_answer_fence() {
        let report = held_answer_result_preserves_the_body_fence(AnswerOutcome::Answered {
            cleared: true,
            now: None,
        })
        .await
        .unwrap();
        assert!(report.cleared);
        assert_eq!(report.now, None);
    }

    #[tokio::test]
    async fn conflict_executor_result_preserves_the_same_answer_fence_and_current_box() {
        let now = decision("Synthetic replacement box?");
        let err = held_answer_result_preserves_the_body_fence(AnswerOutcome::Conflict {
            now: Some(now.clone()),
        })
        .await
        .unwrap_err();
        assert_eq!(
            err,
            ControlError::DecisionChanged {
                session: "cap".into(),
                now: Some(now),
            }
        );
    }

    #[tokio::test]
    async fn nudge_text_goes_through_send_and_an_empty_one_is_refused() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let err = nudge(&mut rig, "cap", "  ").await.unwrap_err();
        assert_eq!(
            err,
            ControlError::Invalid(Refusal::new(Code::OrchestraNudgeEmpty))
        );
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
            kind: crewd_term::DecisionKind::Terminal,
            numbered: false,
            ..decision("Trust?")
        };
        rig.emit("cap", SessionEvent::Decision(DecisionEvent::Appeared(d)))
            .await;
        let err = nudge(&mut rig, "cap", "1").await.unwrap_err();
        assert_eq!(
            err,
            ControlError::Invalid(Refusal::new(Code::OrchestraKeysHumanOnly))
        );
        assert!(!rig.exec.lock().raws.contains_key("cap"));
        assert_eq!(
            nudge(&mut rig, "cap", "enter")
                .await
                .unwrap_err()
                .refusal()
                .unwrap()
                .code,
            Code::OrchestraKeysHumanOnly
        );
        assert!(!rig.exec.lock().raws.contains_key("cap"));
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

    // —— T10d：安全按键与草稿闸

    async fn answer_cmd(
        rig: &mut Rig,
        session: &str,
        fingerprint: &str,
        choice: usize,
    ) -> Result<AnswerReport, ControlError> {
        let (s, f) = (session.to_string(), fingerprint.to_string());
        rig.cmd(|reply| Cmd::Answer {
            session: s,
            fingerprint: f,
            choice,
            reply,
        })
        .await
    }

    /// 调用方看到的框已经不是 actor 此刻知道的那个：当场 `DecisionChanged`（带上现在的框），什么都不排、执行器一下都没按。
    #[tokio::test]
    async fn an_answer_whose_box_changed_before_it_was_queued_is_refused_with_the_new_box() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        assert_eq!(
            answer_cmd(&mut rig, "cap", "x", 0).await,
            Err(ControlError::NoDecision("cap".into()))
        );
        let old = decision("Trust?");
        let new = decision("Run rm -rf build?");
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(new.clone())),
        )
        .await;
        let err = answer_cmd(&mut rig, "cap", &old.fingerprint(), 0)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ControlError::DecisionChanged {
                session: "cap".into(),
                now: Some(new.clone()),
            }
        );
        {
            let st = rig.exec.lock();
            assert!(!st.answers.contains_key("cap"), "没交给执行器");
            assert!(!st.raws.contains_key("cap"));
        }
        assert!(rig.live("cap").unwrap().writes.is_empty(), "什么都没排");
        // 线上：-32009 decision.changed，data.decision = 现在的框。
        let wire = super::super::rpc::rpc_err(err);
        assert_eq!(wire.code, crewd_proto::RpcError::CONFLICT);
        let data = wire.data.unwrap();
        assert_eq!(data.refusal, "decision.changed");
        assert_eq!(data.decision.map(|d| d.summary), Some(new.prompt.clone()));
        // 对的指纹：照常按（编号框选第二项 = 只按「2」）。
        let r = answer_cmd(&mut rig, "cap", &new.fingerprint(), 1).await;
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(rig.exec.lock().raws["cap"], [b"2".to_vec()]);
    }

    /// 编排层不 sleep（T10d）：三下按键的回答从收下到回执，tokio 的虚拟时钟一点没走（键间隔在执行器里）；
    /// 执行器回 `Conflict` → `DecisionChanged`，回 `Unsupported` 不会出现在 claude 上。
    #[tokio::test(start_paused = true)]
    async fn no_sleep_in_orchestra() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let at = |sel: usize| PendingDecision {
            kind: crewd_term::DecisionKind::Terminal,
            prompt: "Pick".into(),
            options: vec!["a".into(), "b".into(), "c".into()],
            numbered: false,
            selected: Some(sel),
        };
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(at(0))),
        )
        .await;
        rig.exec
            .lock()
            .box_script
            .insert("cap".into(), [Some(at(1)), Some(at(2)), None].into());
        let (t0, m0) = (tokio::time::Instant::now(), rig.clock.now());
        let r = answer_cmd(&mut rig, "cap", &at(0).fingerprint(), 2).await;
        assert_eq!(
            r,
            Ok(AnswerReport {
                cleared: true,
                now: None
            })
        );
        assert_eq!(tokio::time::Instant::now(), t0, "编排层没有 sleep");
        assert_eq!(rig.clock.now(), m0);
        assert_eq!(
            rig.exec.lock().writes["cap"],
            ["answer:\\u{1b}[B", "answer:\\u{1b}[B", "answer:\\r"]
        );
        // 高亮被人挪了：执行器停手 → DecisionChanged（带它看到的框）。
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(at(0))),
        )
        .await;
        rig.exec
            .lock()
            .box_script
            .insert("cap".into(), [Some(at(2))].into());
        let err = answer_cmd(&mut rig, "cap", &at(0).fingerprint(), 2)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ControlError::DecisionChanged {
                session: "cap".into(),
                now: Some(at(2))
            }
        );
        // 看不出高亮（没编号）：交给人。
        let blind = PendingDecision {
            kind: crewd_term::DecisionKind::Terminal,
            selected: None,
            ..at(0)
        };
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(blind.clone())),
        )
        .await;
        assert_eq!(
            answer_cmd(&mut rig, "cap", &blind.fingerprint(), 1).await,
            Err(ControlError::NeedsTerminal("cap".into()))
        );
    }

    /// T10d：输入框里有草稿时文字 nudge 当场拒绝（`session.input_draft`，文字里带着草稿），什么都不 send。
    #[tokio::test]
    async fn a_text_nudge_with_a_draft_is_refused_and_says_the_draft() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text("人打的半句".into()))),
        )
        .await;
        let err = nudge(&mut rig, "cap", "继续").await.unwrap_err();
        assert_eq!(
            err,
            ControlError::InputDraft {
                session: "cap".into(),
                text: Some("人打的半句".into())
            }
        );
        let r = err.refusal().unwrap();
        assert_eq!(r.code, Code::SessionInputDraft);
        for l in [crewd_collab::Locale::Zh, crewd_collab::Locale::En] {
            assert!(r.text(l).contains("人打的半句"), "{}", r.text(l));
        }
        assert!(rig.exec.sent("cap").is_empty());
        // 分不清（fix R3）：照样拒，但用另一个码、不转述看得见的那行。
        rig.emit("cap", SessionEvent::InputDraft(Some(HeldDraft::Unknown)))
            .await;
        let err = nudge(&mut rig, "cap", "继续").await.unwrap_err();
        assert_eq!(
            err,
            ControlError::InputDraft {
                session: "cap".into(),
                text: None
            }
        );
        assert_eq!(err.refusal().unwrap().code, Code::SessionInputDraftUnknown);
        // 草稿清掉了：照常 send。
        rig.emit("cap", SessionEvent::InputDraft(None)).await;
        nudge(&mut rig, "cap", "继续").await.unwrap();
        assert_eq!(rig.exec.sent("cap"), ["继续"]);
    }

    /// 草稿是屏上抓来的外来文字：进拒绝文字之前过 safe_name（C20）。
    #[tokio::test]
    async fn the_draft_in_a_refusal_goes_through_safe_name() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text(HOSTILE.into()))),
        )
        .await;
        let err = nudge(&mut rig, "cap", "继续").await.unwrap_err();
        let text = err.refusal().unwrap().text(crewd_collab::Locale::En);
        assert!(text.contains(&safe_name(HOSTILE)), "{text}");
        assert!(!text.contains('\u{202E}'), "{text}");
    }

    /// T10d fix M7：`interrupt`（M3b 的 `nudge_session` 发它）= 打断这一轮；没在干活 = `session.nothing_to_interrupt`；
    /// 屏上有框时两者都拒绝；只有人类 answer 路径能回答框。
    #[tokio::test]
    async fn the_interrupt_token_interrupts_the_turn() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        assert_eq!(
            nudge(&mut rig, "cap", "interrupt").await,
            Err(ControlError::NothingToInterrupt("cap".into()))
        );
        rig.emit("cap", SessionEvent::Working(true)).await;
        nudge(&mut rig, "cap", " Interrupt ").await.unwrap();
        assert_eq!(rig.exec.lock().interrupts, ["cap"]);
        assert!(rig.exec.sent("cap").is_empty(), "不是文字");
        let d = decision("Trust?");
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        )
        .await;
        assert_eq!(
            nudge(&mut rig, "cap", "interrupt")
                .await
                .unwrap_err()
                .refusal()
                .unwrap()
                .code,
            Code::OrchestraKeysHumanOnly
        );
        let st = rig.exec.lock();
        assert_eq!(st.interrupts, ["cap"], "有框：不是盲按的打断");
        assert!(!st.raws.contains_key("cap"));
        assert!(!st.answers.contains_key("cap"));
    }

    /// T10d fix M1 via the human answer path: the first key checks the observed highlight——高亮被人挪了（指纹不变）就停手、回 `DecisionChanged`。
    #[tokio::test]
    async fn a_named_key_nudge_stops_when_the_highlight_moved() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        let d = PendingDecision {
            kind: crewd_term::DecisionKind::Terminal,
            numbered: false,
            ..decision("Trust?")
        };
        rig.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        )
        .await;
        let moved = PendingDecision {
            kind: crewd_term::DecisionKind::Terminal,
            selected: Some(1),
            ..d.clone()
        };
        rig.exec.lock().boxes.insert("cap".into(), moved.clone());
        let err = answer_cmd(&mut rig, "cap", &d.fingerprint(), 0)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ControlError::DecisionChanged {
                session: "cap".into(),
                now: Some(moved)
            }
        );
        assert!(!rig.exec.lock().raws.contains_key("cap"), "回车没按");
    }
}
