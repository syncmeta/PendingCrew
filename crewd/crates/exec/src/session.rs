//! 一个 claude 会话的全部判断，不碰 IO（sans-IO）：喂 PTY 字节、定时拍、命令、退出，
//! 吐出「往 PTY 写什么」和「发生了什么」。时间一律由调用方传入（同 `crewd_term`）。
//!
//! 驱动层（`crate::driver`）只负责把字节搬进搬出、按 [`Core::next_wakeup`] 定时；
//! 所以这里的每条规矩都能用合成时间、合成画面单测。
//!
//! 待决策只看**屏幕**这一路（不喂跟踪器的字节尾窗）：生产 PTY 行尾是 CRLF，尾窗里
//! 行与行之间夹着空行，多行菜单在那一路永远认不出；就算把 CRLF 折掉，尾窗也不知道
//! 屏幕已经翻过去了，答完的菜单会一直挂在 4K 窗里，把「已清除」拖住。见 spec §18。
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crewd_term::{
    AnsiPlainTextTail, CURSOR_MARKERS, DecisionEvent, InputDraft, PendingDecision,
    PendingDecisionTracker, RATE_LIMIT_MENU_PHRASES, Screen, ScreenConfig, Snapshot,
    blocking_dialog, body_tail_visible, input_row, strip,
};

use crate::delivery::{Delivery, Observation, Step, Subject, Timing};
use crate::health::{
    AuthState, FIRST_OUTPUT_DEADLINE, Health, HealthKind, LaunchVerdict, QuotaScanner,
    RATE_LIMIT_COOLDOWN, RateLimitScanner, launch_failure, launch_verdict, quota_recovered,
};

/// 开场任务的投递编号。运行中的指令由 `SessionHandle` 从 1 起编。
pub const BRIEF_DELIVERY_ID: u64 = 0;

/// claude 拒绝续跑时屏上的原话。
pub const NO_CONVERSATION_MARKER: &str = "No conversation found with session ID";

/// 回答待决策框的按键之间隔多久（T10d：从 crewd-wake 的 `NUDGE_KEY_GAP_MS` 挪到这里——按键由会话自己排、
/// 自己在每一下之前回读屏幕，编排层不 sleep）。
pub const ANSWER_KEY_GAP: Duration = Duration::from_millis(150);

/// 最后一下按完多久回读一次，看框答掉没有（S73）。
pub const ANSWER_VERIFY_DELAY: Duration = Duration::from_millis(500);

/// 草稿闸挡住一笔投递时 `DeliveryOutcome::Blocked` 带的原因（是码不是文案：编排层据它显示 `input_draft`）。
pub const INPUT_DRAFT: &str = "input_draft";

/// 外来输入的标记。Typed 自身就是持有 Unknown（旧 Empty 屏不作数）；只有新输出整拍排空后的
/// Text 回显才能进入 Shown，之后新 Empty 才清掉。控制键始终没留下可见字时不会靠时间自动清除。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Foreign {
    None,
    /// 有外来按键，框里还没读到过字（回显可能还在路上）。
    Typed,
    /// 看过人留下的字，且这一笔 raw 是单独的清空 / 退格 / 提交键。新 Empty 可确认该生命周期；旧屏仍持有 Unknown。
    Clearing,
    /// own 正文 / 残留与 raw 相撞：此后 Text 或 Empty 都可能是 own 回显，不能自动清。只认人显式 Ctrl-U 的新回显。
    Conflicted,
    /// 有外来按键之后框里读到过字：再读成空 = 清掉。
    Shown,
}

/// 草稿闸挡着投递时看到的输入框（[`SessionEvent::InputDraft`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldDraft {
    /// 人打的字（正常样式），原样。
    Text(String),
    /// 有人往终端里敲过东西、而此刻分不清框里是草稿还是占位提示（样式认不出 / 混着）：不当成人的草稿转述，
    /// 只说「分不清」（T10d fix R3）。
    Unknown,
}

/// 回答一个待决策框的一下按键（[`Core::answer`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerStep {
    pub bytes: Vec<u8>,
    /// 按这一下之前，框的高亮必须停在第几项（0 起）；`None` = 不核（编号框上按数字）。
    pub selected_before: Option<usize>,
}

/// 一次回答的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerOutcome {
    /// 键都按下去了；最后一下之后 [`ANSWER_VERIFY_DELAY`] 回读：`cleared` = 屏上没有框了、或已换成另一个框
    /// （指纹不同）；`now` = 此刻屏上的框。
    Answered {
        cleared: bool,
        now: Option<PendingDecision>,
    },
    /// 某一下按之前，屏上的框已经不是要答的那个（指纹不同、没有框了、或高亮不在预期的那一项）：这一下和后面的
    /// 都没按。`now` = 此刻屏上的框。
    Conflict { now: Option<PendingDecision> },
    /// 这个执行器不靠按键回答（codex：M3d 改为回复挂起的服务端请求）。
    Unsupported,
    /// A bounded safe write could not be confirmed; the session may still be alive.
    NeedsTerminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreConfig {
    pub delivery: Timing,
    /// 距最后一次输出多久算「不在干活」。
    pub working_quiet: Duration,
    /// 提交三拍（空格 / 退格 / 回车）之间的间隔。
    pub submit_key_gap: Duration,
    /// 限流菜单自动应答「1」与回车之间的间隔。
    pub rate_limit_answer_gap: Duration,
    /// 退出前多久内撞过额度，退出原因记为「撞额度」。
    pub hit_limit_window: Duration,
    /// 续跑被拒只在启动后这段时间里认。
    pub resume_refusal_window: Duration,
    /// 拉起自检的期限。
    pub launch_deadline: Duration,
    /// 回答待决策框的按键间隔（[`ANSWER_KEY_GAP`]）。
    pub answer_key_gap: Duration,
    /// 回答完多久回读（[`ANSWER_VERIFY_DELAY`]）。
    pub answer_verify_delay: Duration,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            delivery: Timing::default(),
            working_quiet: Duration::from_secs(1),
            submit_key_gap: Duration::from_millis(100),
            rate_limit_answer_gap: Duration::from_millis(250),
            hit_limit_window: Duration::from_secs(600),
            resume_refusal_window: Duration::from_secs(5),
            launch_deadline: FIRST_OUTPUT_DEADLINE,
            answer_key_gap: ANSWER_KEY_GAP,
            answer_verify_delay: ANSWER_VERIFY_DELAY,
        }
    }
}

/// 一笔待投的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingText {
    pub id: u64,
    pub subject: Subject,
    pub text: String,
}

/// 一笔投递的结果。`Blocked` / `NotReady` / `Stalled` 只是报告，投递仍在继续；其余是终局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    Blocked(String),
    NotReady(String),
    /// 写了正文 / 发了回车之后输入框一直没回到屏上；先不重试、等它回来（只报一次）。
    Stalled(String),
    Undelivered(String),
    Unsubmitted(String),
    /// 进程退出 / 被停掉，没投出去。
    Abandoned(String),
}

impl DeliveryOutcome {
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            Self::Blocked(_) | Self::NotReady(_) | Self::Stalled(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitStatusKind {
    Code(i32),
    Signal(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    pub status: ExitStatusKind,
    /// 是我们（人 / 机长）停的。
    pub stopped_by_user: bool,
    /// 退出前 `hit_limit_window` 内撞过额度。
    pub hit_limit: bool,
}

/// 驱动层往外发的事件（窄接口的状态面）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// 进程起来了（驱动层发；续跑被拒重起后会再发一次）。`resumed` = 这次是去接一个记过的号
    /// （claude `--resume`、codex `thread/resume`），接没接上看后面有没有 `ResumeRefused`。
    Spawned {
        pid: u32,
        agent_session_id: String,
        resumed: bool,
    },
    /// 干活 / 空闲的边沿。`false` = idle 边沿（「停下即检查」挂在它上面）。
    Working(bool),
    Decision(DecisionEvent),
    Delivery {
        id: u64,
        subject: Subject,
        outcome: DeliveryOutcome,
    },
    /// health 变了（`None` = 恢复正常）。
    Health(Option<Health>),
    /// 续跑被拒：`id` 接不回来了，原话 `said`。后面一定跟一条换了新号的 `Spawned`（换新号
    /// 重起失败时例外：跟的是 LaunchFailed 与 `Exited`）。两个执行器顺序相同：
    /// `Spawned { 旧号, resumed: true }` → `ResumeRefused` → `Spawned { 新号, resumed: false }`。
    /// claude 换一个新进程；codex 在同一个进程里新开一条 thread（pid 不变）。叫停之后不再报。
    /// 当前会话号永远以最近一条 `Spawned` 为准。
    ResumeRefused {
        id: String,
        said: String,
    },
    /// codex：一个完成了的结构化条目（`item/completed` 的 `item` 原样）。claude 不发——它的
    /// 记录就是屏幕。M2d 拿它给 attach 的客户端回放。
    Item {
        turn_id: String,
        item: serde_json::Value,
    },
    /// codex：一个回合正常结束，附本回合最后一条 agent 正文（没有就是空串）。
    /// claude 的对应物是 Stop hook（M3 经 helper 送进来），不走执行器。
    TurnEnded {
        /// 原始 app-server 回合号；协议缺失时为空，不编造身份。
        turn_id: String,
        last_agent_message: String,
    },
    /// 执行器替 agent 做了一个人应该知道的决定（代拒了一个表单、丢了一段原始按键……）。
    Notice(String),
    /// 有正文等着写、而输入框里有草稿（T10d 草稿闸）：[`HeldDraft`]；草稿清掉 / 不再有东西等着写 = `None`。
    /// 只在看到的草稿变了时发。
    InputDraft(Option<HeldDraft>),
    /// codex：服务端确认的实际模型 / 推理强度 / fast 档（握手回包、设置回显）。
    Profile(Profile),
    /// Original provider usage snapshot; not a delta, inferred turn, or progress event.
    Usage(RunnerUsageSnapshot),
    Exited(ExitInfo),
}

/// Canonical JSON over the parsed tokenUsage object. Only explicit provider turn identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerUsageSnapshot {
    pub provider: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub token_usage_json: String,
}
/// Bound the queue and explicit readback; no entire wire notification is retained.
pub const RUNNER_USAGE_MAX_JSON_BYTES: usize = 64 * 1024;
pub const RUNNER_USAGE_MAX_ID_BYTES: usize = 4096;

/// 服务端确认的会话档位。`None` = 服务端没说（不是「关」）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
}

/// 交给驱动层执行的动作，按顺序执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryWritePurpose {
    Body,
    Space { via_body: bool },
    Backspace { via_body: bool },
    Enter { via_body: bool },
    Answer,
    RateLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryWriteToken {
    serial: u64,
    purpose: DeliveryWritePurpose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Write(Vec<u8>),
    /// Terminal protocol reply, distinct from raw human input and guarded product keys.
    TerminalReply(Vec<u8>),
    /// 具有用途和单次写入 token 的正文、提交、回答或限流按键；每次 syscall 前重新核屏幕。
    DeliveryWrite {
        id: u64,
        token: DeliveryWriteToken,
        bytes: Vec<u8>,
    },
    Resize {
        cols: u16,
        lines: u16,
    },
    Event(SessionEvent),
    /// 一次回答（[`Core::answer`] 的 `id`）有了结果；`None` = 会话停了 / 退了，没答成（驱动层回 `Closed`）。
    AnswerDone {
        id: u64,
        outcome: Option<AnswerOutcome>,
    },
}

/// 推导出的会话状态（坏消息优先）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedState {
    LaunchFailed,
    Exited,
    RateLimited,
    Error,
    AwaitingDecision,
    Working,
    Idle,
}

struct InFlight {
    pending: PendingText,
    /// 去毒后的正文：写进去的是它，回读比对的也是它（屏上回显的是去毒后的样子）。
    body: String,
    machine: Delivery,
    /// Core 已排过正文写；驱动可能尚未写或只写了一部分。非 Delivered 终局不能据此声称字节全进了 PTY。
    wrote_body: bool,
    /// 正文落地时输入行的样子（终局不是「已投递」时它就是残留；草稿闸据此认出「是自己的残留」）。
    landed_row: Option<String>,
    /// 最后一个提交回车之后，必须等完整 PTY drain 的新输出才能确认空框。
    submit_needs_output: bool,
    awaiting_write: Option<DeliveryWriteToken>,
    write_started_at: Instant,
    write_progress_at: Instant,
}

/// 排好的按键是干什么的：到点发之前按用途再看一眼屏幕。
#[derive(Clone, Debug, PartialEq, Eq)]
enum KeyPurpose {
    /// 提交三拍里的退格 / 回车。`via_body`：这一轮提交是凭「正文尾巴在屏上」起的
    /// （长正文把提示符滚出了屏），没有输入行可看。
    SubmitBeat {
        via_body: bool,
    },
    /// 限流菜单自动应答「1」之后的回车。
    RateLimitAnswer,
    RateLimitDigit,
    /// 回答待决策框的一下（[`Core::answer`]）：按之前屏上的框指纹必须是 `expect`，`selected` 给了就还要高亮在那一项。
    Answer {
        id: u64,
        expect: String,
        selected: Option<usize>,
    },
}

fn paste_placeholder(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("[Pasted text #") else {
        return false;
    };
    let Some((number, lines)) = rest.split_once(" +") else {
        return false;
    };
    let Some(lines) = lines.strip_suffix(" lines]") else {
        return false;
    };
    !number.is_empty()
        && !lines.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && lines.bytes().all(|b| b.is_ascii_digit())
}

/// 排着、还没开始的一次回答。
struct AnswerJob {
    id: u64,
    expect: String,
    steps: Vec<AnswerStep>,
}

/// 正在进行的一次回答：键排在 `Core::keys` 里；最后一下按完后 `verify_at` 到点回读。
struct Answering {
    started_at: Instant,
    id: u64,
    expect: String,
    /// 还没排进 `keys` 的下几下：上一下真正按下去之后才排下一下（按下时刻 + 间隔，T10d fix M3）。
    remaining: VecDeque<AnswerStep>,
    verify_at: Option<Instant>,
}

const GUARDED_WRITE_IDLE: Duration = Duration::from_millis(2500);
const GUARDED_WRITE_TOTAL: Duration = Duration::from_secs(90);

struct GuardedKeyWrite {
    id: u64,
    token: DeliveryWriteToken,
    purpose: KeyPurpose,
    started_at: Instant,
    progress_at: Instant,
}

struct Key {
    at: Instant,
    bytes: Vec<u8>,
    purpose: KeyPurpose,
}

pub struct Core {
    config: CoreConfig,
    screen: Screen,
    tracker: PendingDecisionTracker,
    quota: QuotaScanner,
    rate_limit: RateLimitScanner,
    /// 上一次自动应答限流菜单的时刻（冷却期内不再答）。
    rate_limit_answered_at: Option<Instant>,
    /// 最近一次 `on_output` / `on_tick` 的 `now`：`next_wakeup` 不给不晚于它的时刻。
    last_now: Instant,
    started_at: Instant,
    running: bool,
    stopped_by_user: bool,
    last_output_at: Option<Instant>,
    working: bool,
    working_since: Option<Instant>,
    health: Option<Health>,
    quota_at: Option<Instant>,
    launch_settled: bool,
    resume_watch: Option<(String, Instant)>,
    /// 续跑观察窗内的字节尾窗：屏幕被改窄时那行原话会折成两行，尾窗里仍是一行。
    resume_tail: AnsiPlainTextTail,
    pending_decision: Option<PendingDecision>,
    in_flight: Option<InFlight>,
    /// 上一笔没投成、但正文写进过输入框：残留可能还在，下一笔第一次写入也先 Ctrl-U。
    input_dirty: bool,
    /// `input_dirty` 时那段残留落地时的输入行（草稿闸认出「是自己的残留」，见 [`Core::draft_held`]）。
    residue: Option<String>,
    /// 最近一次发出去的 `SessionEvent::InputDraft`（只在变了时再发）。
    draft_reported: Option<HeldDraft>,
    /// 输入来源：None = 没有人经 raw 敲过字，claude 自己的占位提示不挡；Typed = 待回显，
    /// 无论旧屏是什么都持有 Unknown；Shown = 新输出里已看到草稿，等后续的新 Empty 才清。
    /// Clearing = 明确清除键的回显未到；Conflicted = own/人混杂，只接受显式 Ctrl-U + 新 Empty。
    /// 我们自己的投递、回答、打断、限流应答、终端回复都不经 raw，不会置它。
    foreign_input: Foreign,
    /// 已喂进新输出、但还没完成整拍排空。没有新输出的定时拍 / 旧屏不能充当 raw 回显或清空证据。
    output_pending: bool,
    /// raw 入队前的输入框读数；同一段旧 Text 的重画不证实这次 raw 已回显。
    foreign_baseline: InputDraft,
    /// 正在进行的回答、排在它后面的回答（一次只答一个）。
    answering: Option<Answering>,
    answers: VecDeque<AnswerJob>,
    queue: VecDeque<PendingText>,
    keys: VecDeque<Key>,
    actions: Vec<Action>,
    next_write: u64,
    guarded_key: Option<GuardedKeyWrite>,
}

impl Core {
    /// `pending`：开机就要投的正文（开场任务排第一）。`resumed_id`：本次用 `--resume` 起的会话号。
    pub fn new(
        config: CoreConfig,
        screen: ScreenConfig,
        now: Instant,
        pending: Vec<PendingText>,
        resumed_id: Option<String>,
    ) -> Self {
        let mut core = Self {
            config,
            screen: Screen::new(screen),
            tracker: PendingDecisionTracker::new(),
            quota: QuotaScanner::default(),
            rate_limit: RateLimitScanner::default(),
            rate_limit_answered_at: None,
            last_now: now,
            started_at: now,
            running: true,
            stopped_by_user: false,
            last_output_at: None,
            working: false,
            working_since: None,
            health: None,
            quota_at: None,
            launch_settled: false,
            resume_watch: resumed_id.map(|id| (id, now + config.resume_refusal_window)),
            resume_tail: AnsiPlainTextTail::new(),
            pending_decision: None,
            in_flight: None,
            input_dirty: false,
            residue: None,
            draft_reported: None,
            foreign_input: Foreign::None,
            output_pending: false,
            foreign_baseline: InputDraft::Empty,
            answering: None,
            answers: VecDeque::new(),
            queue: pending.into_iter().filter(|p| !p.text.is_empty()).collect(),
            keys: VecDeque::new(),
            actions: Vec::new(),
            next_write: 0,
            guarded_key: None,
        };
        core.start_next(now);
        core
    }

    /// 取走攒下的动作。
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// 最早什么时候需要再拍一次（排好的按键、投递的期限、拉起自检、空闲边沿）。0.6s 节拍
    /// 之外的精确唤醒。永远晚于上一次 `on_output` / `on_tick` 的 `now`（驱动层不会空转）。
    pub fn next_wakeup(&self) -> Option<Instant> {
        if !self.running {
            return None;
        }
        let key = self.keys.front().map(|k| k.at);
        let verify = self.answering.as_ref().and_then(|a| a.verify_at);
        // 排着按键、或正在回答时投递机是冻住的（`step_delivery` 不推进），它的期限可能早已过去——不算。
        let delivery = if self.keys.is_empty() && self.answering.is_none() {
            self.in_flight
                .as_ref()
                .filter(|f| f.awaiting_write.is_none())
                .and_then(|f| f.machine.next_deadline())
                .or_else(|| self.pending_write_deadline())
        } else {
            None
        };
        let launch = (!self.launch_settled)
            .then(|| self.started_at.checked_add(self.config.launch_deadline))
            .flatten();
        let idle = self
            .working
            .then(|| {
                self.last_output_at
                    .and_then(|at| at.checked_add(self.config.working_quiet))
            })
            .flatten();
        [key, verify, delivery, launch, idle, self.guarded_deadline()]
            .into_iter()
            .flatten()
            .filter(|t| *t > self.last_now)
            .min()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn health(&self) -> Option<&Health> {
        self.health.as_ref()
    }

    pub fn pending_decision(&self) -> Option<&PendingDecision> {
        self.pending_decision.as_ref()
    }

    pub fn is_working(&self) -> bool {
        self.working
    }

    /// 还没投出去的正文（在途的排第一），续跑被拒重起时搬到新的 `Core`。
    pub fn take_pending(&mut self) -> Vec<PendingText> {
        let mut out: Vec<PendingText> = self
            .in_flight
            .take()
            .map(|f| f.pending)
            .into_iter()
            .collect();
        out.extend(self.queue.drain(..));
        out
    }

    pub fn state(&self) -> DerivedState {
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed)
        {
            return DerivedState::LaunchFailed;
        }
        if !self.running {
            return DerivedState::Exited;
        }
        if let Some(h) = &self.health {
            return if h.kind.is_quota() {
                DerivedState::RateLimited
            } else {
                DerivedState::Error
            };
        }
        if self.pending_decision.is_some() {
            return DerivedState::AwaitingDecision;
        }
        if self.working {
            DerivedState::Working
        } else {
            DerivedState::Idle
        }
    }

    pub(crate) fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }

    pub fn screen_text(&self, max_lines: usize) -> String {
        self.screen.screen_text(max_lines)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.screen.snapshot()
    }

    /// PTY 来了一块输出，且调用方已经确认这一块就是这拍里最后一块可读输出。
    ///
    /// 真 PTY 的驱动必须用 [`Self::ingest_output`] 逐块喂完，再调用
    /// [`Self::after_output_drain`] 或 [`Self::on_tick`] 收尾；否则拆在两次 `read` 里的重画
    /// 会让第一块旧屏误放行回答键。
    pub fn on_output(&mut self, bytes: &[u8], now: Instant) {
        self.ingest_output(bytes, now);
        self.after_output_drain(now);
    }

    /// 只把一块 PTY 输出喂进解析器，不根据这块中途屏幕写任何到点按键。
    /// 驱动确认此刻已经可读的所有块排空后，才可调用 [`Self::after_output_drain`] 或 [`Self::on_tick`]。
    pub(crate) fn ingest_output(&mut self, bytes: &[u8], now: Instant) {
        if !self.running {
            return;
        }
        self.last_now = self.last_now.max(now);
        self.screen.feed(bytes);
        self.output_pending |= !bytes.is_empty();
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            // 终端回复（DA1 / CPR / OSC 11…）先回给子进程：它可能正卡着等。
            self.actions.push(Action::TerminalReply(replies));
        }
        self.last_output_at = Some(now);
        if !self.working {
            self.set_working(true, now);
        }
        // 起得慢、报过「拉起卡住」的进程终于吐字了：它是活的，撤掉那条拉起失败。
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.launch_verdict == Some(LaunchVerdict::Stalled))
        {
            self.set_health(None, now);
        }
        if let Some(h) = self.quota.feed(bytes) {
            self.set_health(Some(h), now);
        }
        // 扫描器只是附加触发：真正的屏幕判定在整拍输出排空后的收尾，
        // 不会拿拆帧中途的屏幕按键。
        let _scanner_fired = self.rate_limit.feed(bytes, now);
        if self.resume_watch.is_some() {
            self.resume_tail.feed(bytes);
        }
    }

    /// 一拍里已经可读的 PTY 输出全喂完后，才做依赖完整屏幕的判定和写键。
    pub(crate) fn after_output_drain(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.last_now = self.last_now.max(now);
        self.observe_drained_output();
        self.validate_guarded_key(now);
        self.flush_due_keys(now);
        self.verify_answer(now);
        self.answer_rate_limit_menu_if_shown(now);
        self.check_resume_refusal(now);
        self.step_delivery(now);
    }

    /// 定时拍（0.6s 节拍，或 `next_wakeup` 到点）。
    /// 如有已喂进的 PTY 输出，调用方必须先确认整拍排空；触顶 / 读失败不得调用。
    pub fn on_tick(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.last_now = self.last_now.max(now);
        self.observe_drained_output();
        self.validate_guarded_key(now);
        self.flush_due_keys(now);
        self.verify_answer(now);
        self.answer_rate_limit_menu_if_shown(now);
        let working = self
            .last_output_at
            .is_some_and(|at| now.saturating_duration_since(at) < self.config.working_quiet);
        if working != self.working {
            self.set_working(working, now);
        }
        if quota_recovered(self.health.as_ref(), self.quota_at, self.working_since, now) {
            self.clear_quota_health(now);
        }
        self.check_launch(now);
        self.check_resume_refusal(now);
        let rows = self.screen.screen_rows();
        match self.tracker.poll(now, Some(&rows)) {
            Some(DecisionEvent::Appeared(d)) => {
                self.pending_decision = Some(d.clone());
                self.event(SessionEvent::Decision(DecisionEvent::Appeared(d)));
            }
            Some(DecisionEvent::Cleared) => self.clear_decision(),
            None => {}
        }
        self.step_delivery(now);
    }

    /// 投一段正文（排队，一次只投一笔）。进程已退出则当场报没送到。
    pub fn send(&mut self, id: u64, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let pending = PendingText {
            id,
            subject: Subject::Steer,
            text: text.to_string(),
        };
        if !self.running {
            let detail = format!(
                "一条指令没送出去：session 已经不在跑了。开头是「{}」。需要重起或改派。",
                preview(text)
            );
            self.finish(pending, DeliveryOutcome::Abandoned(detail.clone()), now);
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
            return;
        }
        if self.stopped_by_user {
            // 已经叫停、还在等退出状态：不排队（排了也只会在退出时被丢）。是人叫停的，不留 health。
            self.finish(
                pending,
                DeliveryOutcome::Abandoned("session 正在被停掉".to_string()),
                now,
            );
            return;
        }
        self.queue.push_back(pending);
        self.start_next(now);
    }

    /// 人 / attach 的唯一入口。回显未到时即持有 Unknown；旧 Empty 屏不能放行。
    /// 已排正文之后收到 raw，终结该投递为 Unsubmitted（可能写了一部分，是否提交未知），
    /// 撤掉剩余正文 / 提交拍，不清理可能混有人字的输入框。原始字节本身照常按 FIFO 写入。
    pub fn raw(&mut self, bytes: &[u8]) {
        if self.running && !bytes.is_empty() {
            self.cancel_guarded_key(AnswerOutcome::NeedsTerminal, self.last_now);
            let collision = self.in_flight.as_ref().is_some_and(|f| f.wrote_body)
                || self.input_dirty
                || self.foreign_input == Foreign::Conflicted;
            self.foreign_input = if (self.foreign_input == Foreign::Shown
                && matches!(bytes, b"\x15" | b"\x7f" | b"\r"))
                || (collision && bytes == b"\x15")
            {
                Foreign::Clearing
            } else if collision {
                Foreign::Conflicted
            } else {
                Foreign::Typed
            };
            // raw 前喂进的输出不是它的回显。新打字总要重新看到 Text，不能借上一次 Shown 放行旧 Empty。
            self.output_pending = false;
            self.foreign_baseline = self.screen.input_draft();
            self.input_dirty = false;
            self.residue = None;
            if self.in_flight.as_ref().is_some_and(|f| f.wrote_body) {
                self.keys
                    .retain(|k| !matches!(k.purpose, KeyPurpose::SubmitBeat { .. }));
                self.report(DeliveryOutcome::Unsubmitted(
                    "外来输入与已排正文相撞：已停止剩余写入和提交，不清理输入框；正文可能只写入一部分，是否已提交不能确认。重发前请回读屏幕和对话，避免重复。".into()
                ), self.last_now);
            }
            self.actions.push(Action::Write(bytes.to_vec()));
            self.step_delivery(self.last_now);
        }
    }

    /// 驱动实际写入的最后一道闸；所有重试清空、正文块、空格 / 退格 / 回车都走它。
    pub(crate) fn delivery_write_allowed(&self, id: u64) -> bool {
        self.running
            && !self.stopped_by_user
            && self.draft_held(None).is_none()
            && self.in_flight.as_ref().is_some_and(|f| f.pending.id == id)
    }

    /// 打断当前回合（我们自己的动作：写一个 Esc，不算外来输入）。
    pub fn interrupt(&mut self) {
        if self.running {
            self.actions.push(Action::Write(vec![0x1b]));
        }
    }

    /// 回答屏上那个待决策框（T10d 的安全按键）：`expect` = 要答的框的指纹，`steps` 一下一下按，之间隔
    /// `answer_key_gap`。**每一下按之前都回读屏幕**（[`Core::flush_due_keys`]）：框不是 `expect`、或高亮不在
    /// `selected_before` → 这一下和剩下的都不按，回 `Conflict`。最后一下按完 `answer_verify_delay` 再回读，回
    /// `Answered`。一次只答一个，后来的排队。结果以 [`Action::AnswerDone`] 交出去。
    pub fn answer(&mut self, id: u64, expect: String, steps: Vec<AnswerStep>, now: Instant) {
        self.queue_answer(id, expect, steps, now);
        self.flush_due_keys(now);
    }

    /// 驱动先只把回答排好；等已经可读的 PTY 输出全喂完，再由
    /// [`Self::after_output_drain`] 放行第一下。`Command::Answer` 本身不会绕过整拍排空闸。
    pub(crate) fn queue_answer(
        &mut self,
        id: u64,
        expect: String,
        steps: Vec<AnswerStep>,
        now: Instant,
    ) {
        if !self.running || self.stopped_by_user {
            self.actions.push(Action::AnswerDone { id, outcome: None });
            return;
        }
        self.answers.push_back(AnswerJob { id, expect, steps });
        self.start_next_answer(now);
    }

    pub fn resize(&mut self, cols: u16, lines: u16) {
        if self.screen.resize(cols as usize, lines as usize) {
            self.actions.push(Action::Resize { cols, lines });
        }
    }

    /// 认证探测的结果。
    pub fn on_auth(&mut self, state: AuthState, now: Instant) {
        if !self.running {
            return;
        }
        let next = state.apply(self.health.clone());
        if next != self.health {
            self.set_health(next, now);
        }
    }

    /// 额度重置唤醒到点后由上层调：清掉额度类 health 并重新武装扫描器。
    pub fn clear_quota_health(&mut self, now: Instant) {
        self.quota.rearm();
        if self.health.as_ref().is_some_and(|h| h.kind.is_quota()) {
            self.set_health(None, now);
        }
    }

    /// 我们要停它了（驱动层随后发信号）。没投出去的不报 health——是人叫停的。
    /// 此后也不再认续跑被拒：`ResumeRefused` 的意思是「换新号重起」，停下来的会话不重起。
    pub fn stop(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.stopped_by_user = true;
        self.launch_settled = true;
        self.resume_watch = None;
        self.abandon("session 被停掉", false, now);
    }

    /// 子进程已退出（驱动层收到了退出状态）。
    pub fn on_exit(&mut self, status: ExitStatusKind, now: Instant) {
        if !self.running {
            return;
        }
        if !self.launch_settled && !self.stopped_by_user {
            let verdict = launch_verdict(
                true,
                false,
                true,
                self.last_output_at.is_some(),
                now.saturating_duration_since(self.started_at),
                self.config.launch_deadline,
            );
            let underlying = match &status {
                ExitStatusKind::Code(c) => format!("exit {c}"),
                ExitStatusKind::Signal(s) => format!("signal {s}"),
            };
            if let Some(h) = launch_failure(verdict, self.config.launch_deadline, Some(&underlying))
            {
                self.set_health(Some(h), now);
            }
            self.launch_settled = true;
        }
        let stopped = self.stopped_by_user;
        self.abandon("进程退出", !stopped, now);
        let hit_limit = self
            .quota_at
            .is_some_and(|at| now.saturating_duration_since(at) <= self.config.hit_limit_window);
        self.running = false;
        self.event(SessionEvent::Exited(ExitInfo {
            status,
            stopped_by_user: stopped,
            hit_limit,
        }));
    }

    // ---- 内部 ----

    fn event(&mut self, e: SessionEvent) {
        self.actions.push(Action::Event(e));
    }

    fn set_working(&mut self, working: bool, now: Instant) {
        self.working = working;
        self.working_since = working.then_some(now);
        self.event(SessionEvent::Working(working));
    }

    /// health 只有一格：拉起失败 > 额度 / 限流 > 认证 > 投递报告。低档的不盖高档的
    /// （清掉 = `None` 由各调用方只对自己那一类做）。`quota_at` 与这一格无关：撞过额度
    /// 就记下时刻，health 被盖、被清都不抹它（`hit_limit` 看的是它）。
    fn set_health(&mut self, health: Option<Health>, now: Instant) {
        if let Some(h) = &health {
            if h.kind.is_quota() {
                self.quota_at = Some(now);
                self.working_since = None;
            }
            if self
                .health
                .as_ref()
                .is_some_and(|cur| cur.kind.rank() > h.kind.rank())
            {
                return;
            }
        }
        self.health = health.clone();
        self.event(SessionEvent::Health(health));
    }

    fn clear_decision(&mut self) {
        if self.pending_decision.take().is_some() {
            self.event(SessionEvent::Decision(DecisionEvent::Cleared));
        }
    }

    /// 限流菜单的自动应答只认**当前屏幕**：摆着限流菜单、光标停在「Stop and wait」（第一项）
    /// 上（控制器裁定）。字节尾窗不作数——它会留着旧短语、也会收到对话里回显的短语，
    /// 那时屏上可能是权限菜单，按「1」就替人批了权限。
    ///
    /// 冷却：上一次应答之后 `RATE_LIMIT_COOLDOWN`（30s）内不再答——答完菜单常会再画一两帧，
    /// 连按会把多余的「1」敲进输入框。冷却期里菜单真的又弹出来，要等冷却过后下一块输出 /
    /// 下一拍才答（最多晚 30s；比连按的风险小）。
    fn answer_rate_limit_menu_if_shown(&mut self, now: Instant) {
        if self.guarded_key.is_some() || self.answering.is_some() {
            return;
        }
        if self
            .rate_limit_answered_at
            .is_some_and(|at| now.saturating_duration_since(at) < RATE_LIMIT_COOLDOWN)
        {
            return;
        }
        if rate_limit_menu_on_screen(&self.screen.screen_rows()) {
            self.rate_limit_answered_at = Some(now);
            self.answer_rate_limit_menu(now);
        }
    }

    /// 发到点的排队按键。发之前按用途再看一眼当前屏幕：
    /// - 提交拍：屏上弹了对话框、或输入行没了（凭正文尾巴起的提交：尾巴没了）——这一轮剩下的
    ///   提交拍全丢掉（回车会替人选中对话框的第一项），并经 `Delivery::submit_aborted` 告诉
    ///   投递机这一轮回车没发出去：它转入「等重发」，对话框不在、输入行（或正文尾巴）回来了
    ///   再发一轮（记一次重试）。
    /// - 限流应答的回车：菜单不在了就不发（「1」可能已经直接选中关掉了它）。
    fn flush_due_keys(&mut self, now: Instant) {
        if self.guarded_key.is_some() {
            return;
        }
        while self.keys.front().is_some_and(|k| k.at <= now) {
            if self.guarded_key.is_some() {
                return;
            }
            let key = self.keys.pop_front().expect("刚看过非空");
            let rows = self.screen.screen_rows();
            let safe = match &key.purpose {
                KeyPurpose::SubmitBeat { via_body } => {
                    self.foreign_input == Foreign::None
                        && blocking_dialog(&rows).is_none()
                        && if *via_body {
                            self.in_flight
                                .as_ref()
                                .is_some_and(|f| body_tail_visible(&f.body, &rows))
                        } else {
                            input_row(&rows).is_some()
                        }
                }
                KeyPurpose::RateLimitAnswer | KeyPurpose::RateLimitDigit => {
                    rate_limit_menu_on_screen(&rows)
                }
                KeyPurpose::Answer {
                    id,
                    expect,
                    selected,
                } => {
                    let shown = blocking_dialog(&rows);
                    let same = shown.as_ref().is_some_and(|d| {
                        d.fingerprint() == *expect
                            && selected.is_none_or(|want| d.selected == Some(want))
                    });
                    if same {
                        self.queue_guarded_key(*id, key.bytes, key.purpose.clone(), now);
                    } else {
                        let id = *id;
                        self.keys.retain(|k| answer_id(k) != Some(id));
                        self.finish_answer(AnswerOutcome::Conflict { now: shown }, now);
                    }
                    continue;
                }
            };
            if safe {
                if matches!(key.purpose, KeyPurpose::SubmitBeat { .. }) {
                    let KeyPurpose::SubmitBeat { via_body } = key.purpose else {
                        unreachable!()
                    };
                    let purpose = if key.bytes == b"\r" {
                        DeliveryWritePurpose::Enter { via_body }
                    } else {
                        DeliveryWritePurpose::Backspace { via_body }
                    };
                    self.queue_delivery_write(key.bytes, purpose, now);
                } else {
                    self.queue_guarded_key(0, key.bytes, key.purpose, now);
                }
            } else if let KeyPurpose::SubmitBeat { .. } = key.purpose {
                self.keys
                    .retain(|k| !matches!(k.purpose, KeyPurpose::SubmitBeat { .. }));
                // 回车没发出去：告诉投递机这一轮不算提交过（否则长正文那一路「输入行出现」
                // 会被当成投递成功），等安全了它会再发一轮。
                if let Some(flight) = self.in_flight.as_mut() {
                    flight.machine.submit_aborted(now);
                }
            }
        }
    }

    /// 没有回答在进行时，开始排着的下一个：键全部排进 `keys`（第一下就是「现在」，下一次 flush 前回读）。
    fn start_next_answer(&mut self, now: Instant) {
        if self.answering.is_some() || self.guarded_key.is_some() {
            return;
        }
        let Some(job) = self.answers.pop_front() else {
            return;
        };
        let verify_at = job
            .steps
            .is_empty()
            .then(|| now + self.config.answer_verify_delay);
        self.answering = Some(Answering {
            started_at: now,
            id: job.id,
            expect: job.expect,
            remaining: job.steps.into(),
            verify_at,
        });
        // 第一下「现在」排进去；调用方只在整拍 PTY 输出排空之后 flush。后面的每一下在
        // 上一下按下之后再排。
        self.schedule_next_answer_key(now);
    }

    /// 把正在进行的回答的下一下排在 `at`；没有下一下了 = 到 `at + answer_verify_delay` 回读（S73）。
    fn schedule_next_answer_key(&mut self, at: Instant) {
        let delay = self.config.answer_verify_delay;
        let Some(a) = self.answering.as_mut() else {
            return;
        };
        match a.remaining.pop_front() {
            Some(step) => {
                let purpose = KeyPurpose::Answer {
                    id: a.id,
                    expect: a.expect.clone(),
                    selected: step.selected_before,
                };
                self.schedule(at, step.bytes, purpose);
            }
            None => {
                if a.verify_at.is_none() {
                    a.verify_at = Some(at + delay);
                }
            }
        }
    }

    /// 按下一下之后：还有下一下就排在 `next`；没有了就 `now + answer_verify_delay` 回读。
    fn schedule_next_answer_key_or_verify(&mut self, next: Instant, now: Instant) {
        let has_more = self
            .answering
            .as_ref()
            .is_some_and(|a| !a.remaining.is_empty());
        self.schedule_next_answer_key(if has_more { next } else { now });
    }

    /// 最后一下按完、回读时刻到了：看框还在不在。
    fn verify_answer(&mut self, now: Instant) {
        let Some(a) = self.answering.as_ref() else {
            return;
        };
        if a.verify_at.is_none_or(|at| at > now) {
            return;
        }
        let shown = blocking_dialog(&self.screen.screen_rows());
        let cleared = shown.as_ref().is_none_or(|d| d.fingerprint() != a.expect);
        self.finish_answer(
            AnswerOutcome::Answered {
                cleared,
                now: shown,
            },
            now,
        );
    }

    /// 当前这次回答有了结果：交出去，开始下一个。
    fn finish_answer(&mut self, outcome: AnswerOutcome, now: Instant) {
        self.guarded_key = None;
        if let Some(a) = self.answering.take() {
            self.actions.push(Action::AnswerDone {
                id: a.id,
                outcome: Some(outcome),
            });
        }
        self.start_next_answer(now);
        // `finish_answer` 只会在一轮完整屏幕判定里发生；排着的下一答可在同一轮安全地开始。
        self.flush_due_keys(now);
    }

    fn answer_rate_limit_menu(&mut self, now: Instant) {
        self.queue_guarded_key(0, b"1".to_vec(), KeyPurpose::RateLimitDigit, now);
        if self.health.as_ref().map(|h| h.kind) != Some(HealthKind::RateLimited) {
            self.set_health(
                Some(Health::new(
                    HealthKind::RateLimited,
                    "Claude Code 撞到额度上限（rate-limit 菜单等待安全应答「Stop and wait」）——等额度窗口重置后自动继续。",
                )),
                now,
            );
        }
    }

    fn schedule(&mut self, at: Instant, bytes: Vec<u8>, purpose: KeyPurpose) {
        let pos = self.keys.partition_point(|k| k.at <= at);
        self.keys.insert(pos, Key { at, bytes, purpose });
    }

    fn check_launch(&mut self, now: Instant) {
        if self.launch_settled {
            return;
        }
        let verdict = launch_verdict(
            true,
            true,
            true,
            self.last_output_at.is_some(),
            now.saturating_duration_since(self.started_at),
            self.config.launch_deadline,
        );
        match verdict {
            LaunchVerdict::Alive => self.launch_settled = true,
            v if v.is_terminal() => {
                self.launch_settled = true;
                let h = launch_failure(v, self.config.launch_deadline, None);
                self.set_health(h, now);
            }
            _ => {}
        }
    }

    fn check_resume_refusal(&mut self, now: Instant) {
        let Some((id, deadline)) = &self.resume_watch else {
            return;
        };
        if now > *deadline {
            self.resume_watch = None;
            return;
        }
        let id = id.clone();
        let text = self.screen.screen_text(usize::MAX);
        let said =
            resume_rejection(&text, &id).or_else(|| resume_rejection(self.resume_tail.tail(), &id));
        if let Some(said) = said {
            self.resume_watch = None;
            self.event(SessionEvent::ResumeRefused { id, said });
        }
    }

    fn start_next(&mut self, now: Instant) {
        if self.in_flight.is_some() || !self.running {
            return;
        }
        if let Some(pending) = self.queue.pop_front() {
            let machine = Delivery::new(self.config.delivery, pending.subject, now);
            let body = sanitize_text(&pending.text);
            self.in_flight = Some(InFlight {
                pending,
                body,
                machine,
                wrote_body: false,
                landed_row: None,
                submit_needs_output: false,
                awaiting_write: None,
                write_started_at: now,
                write_progress_at: now,
            });
            self.step_delivery(now);
        }
    }

    fn step_delivery(&mut self, now: Instant) {
        if self.expire_pending_write(now) {
            return;
        }
        // 提交三拍（空格 / 退格 / 回车）没发完之前不回读：中间那一帧（多了个空格）不作数。回答待决策框的键
        // 没按完、或还没回读时也不动（不往一个正在被答的框所在的屏上敲字）。
        if !self.keys.is_empty()
            || self.answering.is_some()
            || self
                .in_flight
                .as_ref()
                .is_some_and(|f| f.awaiting_write.is_some())
        {
            return;
        }
        if self.in_flight.is_none() {
            self.note_draft(None);
            return;
        }
        let rows = self.screen.screen_rows();
        let row = input_row(&rows);
        let held = self.draft_held(row.as_deref());
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        let step = flight.machine.step(Observation {
            input_row: row.as_deref(),
            input_empty: self.screen.input_draft() == InputDraft::Empty
                && !flight.submit_needs_output,
            body_visible: body_tail_visible(&flight.body, &rows),
            dialog_present: blocking_dialog(&rows).is_some(),
            draft_held: held.is_some(),
            now,
        });
        // 第一次写正文之前（还在等就绪）才看草稿：写进去以后输入框里的字是自己的。
        let awaiting = matches!(step, Step::Idle | Step::HeldByDraft) && !flight.wrote_body;
        self.note_draft(held.filter(|_| awaiting));
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        match step {
            Step::Idle => {}
            Step::HeldByDraft => {
                self.report(DeliveryOutcome::Blocked(INPUT_DRAFT.to_string()), now)
            }
            Step::WriteBody { attempt } => {
                let mut bytes = Vec::new();
                if attempt > 1 || self.input_dirty {
                    // 重投前清掉输入行：这一笔（或没投成的上一笔）若落了一半，两段会叠成一条。
                    bytes.push(0x15);
                }
                self.input_dirty = false;
                self.residue = None;
                flight.wrote_body = true;
                bytes.extend_from_slice(flight.body.as_bytes());
                self.queue_delivery_write(bytes, DeliveryWritePurpose::Body, now);
            }
            Step::Submit { .. } => {
                if flight.landed_row.is_none() {
                    flight.landed_row = row.clone();
                }
                // 正文是一次灌进去的，claude 会当成粘贴：单发回车可能只结束粘贴而不提交。
                // 先补一个空格再删掉（输入行判据 trim，不影响判定），隔拍再回车。
                self.queue_delivery_write(
                    b" ".to_vec(),
                    DeliveryWritePurpose::Space {
                        via_body: row.is_none(),
                    },
                    now,
                );
            }
            Step::BlockedByDialog(detail) => self.report(DeliveryOutcome::Blocked(detail), now),
            Step::NotReady(detail) => self.report(DeliveryOutcome::NotReady(detail), now),
            Step::Stalled(detail) => self.report(DeliveryOutcome::Stalled(detail), now),
            Step::Undelivered(detail) => self.report(DeliveryOutcome::Undelivered(detail), now),
            Step::Unsubmitted(detail) => self.report(DeliveryOutcome::Unsubmitted(detail), now),
            Step::Delivered => self.report(DeliveryOutcome::Delivered, now),
        }
    }

    fn queue_delivery_write(
        &mut self,
        bytes: Vec<u8>,
        purpose: DeliveryWritePurpose,
        now: Instant,
    ) {
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        self.next_write += 1;
        let token = DeliveryWriteToken {
            serial: self.next_write,
            purpose,
        };
        flight.awaiting_write = Some(token);
        flight.write_started_at = now;
        flight.write_progress_at = now;
        self.actions.push(Action::DeliveryWrite {
            id: flight.pending.id,
            token,
            bytes,
        });
    }

    fn queue_guarded_key(&mut self, id: u64, bytes: Vec<u8>, purpose: KeyPurpose, now: Instant) {
        if self.guarded_key.is_some() {
            return;
        }
        self.next_write += 1;
        let token = DeliveryWriteToken {
            serial: self.next_write,
            purpose: if matches!(purpose, KeyPurpose::Answer { .. }) {
                DeliveryWritePurpose::Answer
            } else {
                DeliveryWritePurpose::RateLimit
            },
        };
        let started_at = if matches!(purpose, KeyPurpose::Answer { .. }) {
            self.answering
                .as_ref()
                .filter(|a| a.id == id)
                .map_or(now, |a| a.started_at)
        } else {
            self.rate_limit_answered_at.unwrap_or(now)
        };
        self.guarded_key = Some(GuardedKeyWrite {
            id,
            token,
            purpose,
            started_at,
            progress_at: now,
        });
        self.actions
            .push(Action::DeliveryWrite { id, token, bytes });
    }

    fn guarded_deadline(&self) -> Option<Instant> {
        let key = self
            .guarded_key
            .as_ref()
            .map(|k| (k.progress_at + GUARDED_WRITE_IDLE).min(k.started_at + GUARDED_WRITE_TOTAL));
        let answer = self
            .answering
            .as_ref()
            .map(|a| a.started_at + GUARDED_WRITE_TOTAL);
        key.into_iter().chain(answer).min()
    }

    fn cancel_guarded_key(&mut self, outcome: AnswerOutcome, now: Instant) {
        let key = self.guarded_key.take();
        if let Some(k) = key {
            if matches!(k.purpose, KeyPurpose::Answer { .. }) {
                self.keys.retain(|key| answer_id(key) != Some(k.id));
                self.finish_answer(outcome, now);
            } else {
                self.keys.retain(|key| {
                    !matches!(
                        key.purpose,
                        KeyPurpose::RateLimitDigit | KeyPurpose::RateLimitAnswer
                    )
                });
                self.start_next_answer(now);
            }
        }
    }

    /// May run even when an output drain exhausted its budget: only actual positive writes
    /// refresh the inactivity deadline. Noise and deferred screen decisions cannot renew it.
    pub(crate) fn expire_guarded_writes(&mut self, now: Instant) {
        if self
            .guarded_deadline()
            .is_some_and(|deadline| now >= deadline)
        {
            if self.guarded_key.is_some() {
                self.cancel_guarded_key(AnswerOutcome::NeedsTerminal, now);
            } else if let Some(a) = &self.answering {
                let id = a.id;
                self.keys.retain(|k| answer_id(k) != Some(id));
                self.finish_answer(AnswerOutcome::NeedsTerminal, now);
            }
        }
        self.expire_pending_write(now);
    }

    fn validate_guarded_key(&mut self, now: Instant) {
        self.expire_guarded_writes(now);
        let Some(key) = self.guarded_key.as_ref() else {
            return;
        };
        let rows = self.screen.screen_rows();
        let outcome = if self.foreign_input != Foreign::None {
            Some(AnswerOutcome::NeedsTerminal)
        } else {
            match &key.purpose {
                KeyPurpose::Answer {
                    expect, selected, ..
                } => {
                    let shown = blocking_dialog(&rows);
                    (!shown.as_ref().is_some_and(|d| {
                        d.fingerprint() == *expect
                            && selected.is_none_or(|want| d.selected == Some(want))
                    }))
                    .then_some(AnswerOutcome::Conflict { now: shown })
                }
                _ => (!rate_limit_menu_on_screen(&rows)).then_some(AnswerOutcome::NeedsTerminal),
            }
        };
        if let Some(outcome) = outcome {
            self.cancel_guarded_key(outcome, now);
        }
    }

    /// Called after a complete fresh PTY drain, immediately before each bounded syscall.
    /// A token identifies one physical write, not merely a reused public delivery/answer id.
    pub(crate) fn actual_write_guard(
        &mut self,
        id: u64,
        token: DeliveryWriteToken,
        offset: usize,
        now: Instant,
    ) -> bool {
        self.validate_guarded_key(now);
        if !self.delivery_write_pending(id, token) {
            return false;
        }
        if matches!(
            token.purpose,
            DeliveryWritePurpose::Answer | DeliveryWritePurpose::RateLimit
        ) {
            return true;
        }
        let rows = self.screen.screen_rows();
        let row = input_row(&rows);
        let flight = self.in_flight.as_ref().expect("matched delivery lease");
        let authority = match token.purpose {
            DeliveryWritePurpose::Body => {
                let owned = if offset == 0 {
                    self.draft_held(row.as_deref()).is_none()
                } else {
                    match self.screen.input_draft() {
                        InputDraft::Empty => true,
                        InputDraft::Unknown => false,
                        InputDraft::Text(text) => {
                            // A partial syscall is not authority over arbitrary new text. Recognize
                            // only an echoed prefix actually written, or Claude's paste placeholder.
                            (!text.is_empty()
                                && text.len() <= offset
                                && flight.body.trim_start().starts_with(&text))
                                || paste_placeholder(&text)
                        }
                    }
                };
                (row.is_some() || body_tail_visible(&flight.body, &rows)) && owned
            }
            DeliveryWritePurpose::Space { via_body }
            | DeliveryWritePurpose::Backspace { via_body }
            | DeliveryWritePurpose::Enter { via_body } => {
                if via_body {
                    body_tail_visible(&flight.body, &rows)
                } else {
                    row.is_some()
                }
            }
            _ => unreachable!(),
        };
        if self.foreign_input == Foreign::None && blocking_dialog(&rows).is_none() && authority {
            return true;
        }
        self.keys
            .retain(|k| !matches!(k.purpose, KeyPurpose::SubmitBeat { .. }));
        if token.purpose == DeliveryWritePurpose::Body {
            if offset > 0 {
                // The written prefix no longer grants authority over this new/unknown
                // screen. Do not convert it to own residue and later Ctrl-U another job.
                // Existing explicit raw clear + fresh Empty can recover this hold.
                self.foreign_input = Foreign::Conflicted;
                self.foreign_baseline = self.screen.input_draft();
                self.input_dirty = false;
                self.residue = None;
            }
            self.report(
                DeliveryOutcome::Unsubmitted(
                    "正文写入期间输入权威改变；可能写入了部分正文，未确认提交。".into(),
                ),
                now,
            );
        } else if let Some(flight) = self.in_flight.as_mut() {
            flight.awaiting_write = None;
            flight.machine.submit_aborted(now);
        }
        false
    }

    /// Positive syscall progress is not completion and never advances body or submit phases.
    pub fn delivery_write_progress(&mut self, id: u64, token: DeliveryWriteToken, now: Instant) {
        self.expire_guarded_writes(now);
        if let Some(k) = self.guarded_key.as_mut()
            && k.id == id
            && k.token == token
        {
            k.progress_at = now;
            return;
        }
        if self.expire_pending_write(now) || !self.delivery_write_pending(id, token) {
            return;
        }
        self.in_flight
            .as_mut()
            .expect("pending write")
            .write_progress_at = now;
        self.last_now = self.last_now.max(now);
    }

    pub fn delivery_write_pending(&self, id: u64, token: DeliveryWriteToken) -> bool {
        if matches!(
            token.purpose,
            DeliveryWritePurpose::Answer | DeliveryWritePurpose::RateLimit
        ) {
            return self.running
                && !self.stopped_by_user
                && self
                    .guarded_key
                    .as_ref()
                    .is_some_and(|k| k.id == id && k.token == token);
        }
        self.delivery_write_allowed(id)
            && self
                .in_flight
                .as_ref()
                .is_some_and(|f| f.awaiting_write == Some(token))
    }

    fn pending_write_deadline(&self) -> Option<Instant> {
        let flight = self
            .in_flight
            .as_ref()
            .filter(|f| f.awaiting_write.is_some())?;
        [
            flight
                .write_progress_at
                .checked_add(self.config.delivery.verify_window),
            flight
                .write_started_at
                .checked_add(self.config.delivery.readiness_deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn expire_pending_write(&mut self, now: Instant) -> bool {
        if self
            .pending_write_deadline()
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        let purpose = self
            .in_flight
            .as_ref()
            .unwrap()
            .awaiting_write
            .unwrap()
            .purpose;
        let detail = "PTY 写入未能在现有期限内完成；可能只写入了部分字节，未确认提交。".to_string();
        let outcome = if purpose == DeliveryWritePurpose::Body {
            DeliveryOutcome::Undelivered(detail)
        } else {
            DeliveryOutcome::Unsubmitted(detail)
        };
        self.report(outcome, now);
        true
    }

    /// Driver calls this only after the complete write, never for partial, cancelled or blocked writes.
    pub fn delivery_write_completed(&mut self, id: u64, token: DeliveryWriteToken, now: Instant) {
        if matches!(
            token.purpose,
            DeliveryWritePurpose::Answer | DeliveryWritePurpose::RateLimit
        ) {
            self.expire_guarded_writes(now);
            if !self.delivery_write_pending(id, token) {
                return;
            }
            let key = self.guarded_key.take().expect("matched key lease");
            match key.purpose {
                KeyPurpose::Answer { .. } => {
                    self.schedule_next_answer_key_or_verify(now + self.config.answer_key_gap, now)
                }
                KeyPurpose::RateLimitDigit => self.schedule(
                    now + self.config.rate_limit_answer_gap,
                    b"\r".to_vec(),
                    KeyPurpose::RateLimitAnswer,
                ),
                KeyPurpose::RateLimitAnswer => {
                    self.start_next_answer(now);
                }
                _ => unreachable!(),
            }
            return;
        }
        if self.expire_pending_write(now) || !self.delivery_write_pending(id, token) {
            return;
        }
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        if flight.awaiting_write != Some(token) {
            return;
        }
        flight.awaiting_write = None;
        self.last_now = self.last_now.max(now);
        match token.purpose {
            DeliveryWritePurpose::Body => flight.machine.body_written(now),
            DeliveryWritePurpose::Space { via_body } => {
                self.schedule(
                    now + self.config.submit_key_gap,
                    vec![0x7f],
                    KeyPurpose::SubmitBeat { via_body },
                );
            }
            DeliveryWritePurpose::Backspace { via_body } => {
                self.schedule(
                    now + self.config.submit_key_gap,
                    b"\r".to_vec(),
                    KeyPurpose::SubmitBeat { via_body },
                );
            }
            DeliveryWritePurpose::Enter { .. } => {
                flight.machine.enter_written(now);
                flight.submit_needs_output = true;
                // Any bytes ingested before actual Enter completion cannot acknowledge it.
                self.output_pending = false;
            }
            DeliveryWritePurpose::Answer | DeliveryWritePurpose::RateLimit => unreachable!(),
        }
    }

    /// 投递机给了一个报告：非终局只报（翻 health）；终局收掉、清过期 health、取下一笔。
    fn report(&mut self, outcome: DeliveryOutcome, now: Instant) {
        let Some(flight) = self.in_flight.as_ref() else {
            return;
        };
        let kind = match flight.pending.subject {
            Subject::Brief => HealthKind::BriefUndelivered,
            Subject::Steer => HealthKind::PromptUndelivered,
        };
        let pending = flight.pending.clone();
        let wrote_body = flight.wrote_body;
        let landed_row = flight.landed_row.clone();
        match &outcome {
            DeliveryOutcome::Delivered => {
                if self.health.as_ref().map(|h| h.kind) == Some(kind) {
                    self.set_health(None, now);
                }
            }
            // 草稿闸：人在输入框里打字不是会话出了毛病，不留 health（`SessionEvent::InputDraft` 已经说了）。
            DeliveryOutcome::Blocked(d) if d == INPUT_DRAFT => {}
            DeliveryOutcome::Blocked(d)
            | DeliveryOutcome::NotReady(d)
            | DeliveryOutcome::Stalled(d)
            | DeliveryOutcome::Undelivered(d)
            | DeliveryOutcome::Unsubmitted(d)
            | DeliveryOutcome::Abandoned(d) => {
                self.set_health(Some(Health::new(kind, d.clone())), now);
            }
        }
        if outcome.is_final() {
            if outcome != DeliveryOutcome::Delivered
                && wrote_body
                && self.foreign_input == Foreign::None
            {
                self.input_dirty = true;
                self.residue = landed_row;
            }
            self.in_flight = None;
            self.finish(pending, outcome, now);
            self.start_next(now);
        } else {
            self.event(SessionEvent::Delivery {
                id: pending.id,
                subject: pending.subject,
                outcome,
            });
        }
    }

    /// 输入来源优先于样式 / 自己的残留。Typed 的旧屏即使 Empty 或没有输入行，也必须持有 Unknown。
    /// 只读这里不会推进生命周期；只有新输出的整拍排空可以确认回显和清空。
    fn draft_held(&self, row: Option<&str>) -> Option<HeldDraft> {
        match self.foreign_input {
            Foreign::Typed | Foreign::Clearing | Foreign::Conflicted => {
                return Some(HeldDraft::Unknown);
            }
            Foreign::Shown => {
                return Some(match self.screen.input_draft() {
                    InputDraft::Text(t) => HeldDraft::Text(t),
                    InputDraft::Empty | InputDraft::Unknown => HeldDraft::Unknown,
                });
            }
            Foreign::None => {}
        }
        let row = row?;
        if self.input_dirty && self.residue.as_deref() == Some(row) {
            return None;
        }
        None
    }

    /// 两种完整 drain 的收尾共用这一道证据门：普通读 / 回答与 tick / next_wakeup。
    /// 无新输出时不重读旧屏；raw 会撤掉它之前的 pending，所以不能借旧回显放行。
    fn observe_drained_output(&mut self) {
        if std::mem::take(&mut self.output_pending) {
            if let Some(flight) = &mut self.in_flight {
                flight.submit_needs_output = false;
            }
            self.observe_input_box();
        }
    }

    /// 只在 raw 之后的新输出全部排空时调用。Text 才证实回显到了；Unknown 不把未决证据升格为 Shown。
    /// 看过可见草稿再看到新 Empty 才清掉；空框里的控制键 / 回显始终不可辨认时会一直持有 Unknown。
    fn observe_input_box(&mut self) {
        if self.foreign_input == Foreign::None {
            return;
        }
        let rows = self.screen.screen_rows();
        if input_row(&rows).is_none() {
            return;
        }
        let reading = self.screen.input_draft();
        match (self.foreign_input, &reading) {
            (Foreign::Shown | Foreign::Clearing, InputDraft::Empty) => {
                self.foreign_input = Foreign::None
            }
            (Foreign::Typed | Foreign::Clearing, InputDraft::Text(_))
                if reading != self.foreign_baseline =>
            {
                self.foreign_input = Foreign::Shown
            }
            _ => {}
        }
    }

    /// 看到的草稿变了才发 `InputDraft`（清掉 = `None`）。
    fn note_draft(&mut self, draft: Option<HeldDraft>) {
        if draft != self.draft_reported {
            self.draft_reported = draft.clone();
            self.event(SessionEvent::InputDraft(draft));
        }
    }

    fn finish(&mut self, pending: PendingText, outcome: DeliveryOutcome, _now: Instant) {
        self.event(SessionEvent::Delivery {
            id: pending.id,
            subject: pending.subject,
            outcome,
        });
    }

    /// 手上这笔和排队的都投不出去了。`report`：给运行中的指令留一条 health。
    fn abandon(&mut self, reason: &str, report: bool, now: Instant) {
        let lost = self.take_pending();
        self.keys.clear();
        // 回答也答不成了（进程退了 / 被停了）：驱动层回 `Closed`。
        self.guarded_key = None;
        let answers: Vec<u64> = self
            .answering
            .take()
            .map(|a| a.id)
            .into_iter()
            .chain(self.answers.drain(..).map(|j| j.id))
            .collect();
        for id in answers {
            self.actions.push(Action::AnswerDone { id, outcome: None });
        }
        self.note_draft(None);
        for p in &lost {
            self.finish(
                p.clone(),
                DeliveryOutcome::Abandoned(reason.to_string()),
                now,
            );
        }
        let steers: Vec<&PendingText> = lost
            .iter()
            .filter(|p| p.subject == Subject::Steer)
            .collect();
        if report && let Some(first) = steers.first() {
            let detail = format!(
                "{} 条指令没送到就{reason}了，第一条开头是「{}」。这些指令没有被任何人接收，需要重发。",
                steers.len(),
                preview(&first.text)
            );
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
        }
        self.clear_decision();
        if self.working {
            self.set_working(false, now);
        }
    }
}

/// 正文写进 PTY 之前去掉「会被当成按键」的字节：CR（单独或 CRLF）当换行——单独一个 CR
/// 就是「提交」；其余控制字符（ESC 会打断回合、Ctrl-C 会中止、NUL…）换成看得见的
/// Unicode 控制图形（U+2400 起），换行与制表符原样保留。白板里贴进来的终端输出常带这些。
pub fn sanitize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' => '\n',
            '\n' | '\t' => c,
            '\u{7f}' => '\u{2421}',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'),
            c if ('\u{80}'..='\u{9f}').contains(&c) => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// 一眼能认出是哪条的前 40 个字符（换行压成空格）。
pub fn preview(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 40 {
        one_line
    } else {
        one_line.chars().take(40).collect::<String>() + "…"
    }
}

/// 这一下按键属于哪一次回答。
fn answer_id(k: &Key) -> Option<u64> {
    match k.purpose {
        KeyPurpose::Answer { id, .. } => Some(id),
        _ => None,
    }
}

/// 当前屏幕上是不是摆着限流菜单、光标停在「Stop and wait」（第一项）上。
///
/// 两条都要：屏上有限流短语（逐行剥掉框线、去掉空白后拼起来比对——窄屏会把那句折成两行），
/// **且**带光标的那一行就是「Stop and wait」（`❯ 1. Stop and wait…` 或不带编号）。
/// 光标停在别处（权限菜单的 `❯ 1. Yes`）就不是它。
pub fn rate_limit_menu_on_screen(rows: &[String]) -> bool {
    let compact: String = rows
        .iter()
        .map(|r| strip(&r.replace('\0', " ")).to_lowercase())
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let phrase_shown = RATE_LIMIT_MENU_PHRASES.iter().any(|p| {
        let p: String = p.chars().filter(|c| !c.is_whitespace()).collect();
        compact.contains(&p)
    });
    phrase_shown && rows.iter().any(|r| stop_and_wait_selected(r))
}

/// 这一行是不是光标停在「Stop and wait」这一项上。
fn stop_and_wait_selected(row: &str) -> bool {
    let row = row.replace('\0', " ");
    let s = strip(&row);
    let Some(first) = s.chars().next() else {
        return false;
    };
    if !CURSOR_MARKERS.contains(&first) {
        return false;
    }
    let rest = s[first.len_utf8()..].trim_start();
    let rest = rest.strip_prefix("1.").map_or(rest, str::trim_start);
    rest.to_lowercase().starts_with("stop and wait")
}

/// 屏上有没有 claude 拒绝续跑**这个** id 的原话；有则返回那一行。
pub fn resume_rejection(screen_text: &str, resumed_id: &str) -> Option<String> {
    let id = resumed_id.trim();
    if id.is_empty() {
        return None;
    }
    screen_text
        .lines()
        .map(str::trim)
        .find(|l| l.contains(NO_CONVERSATION_MARKER) && l.contains(id))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct T {
        core: Core,
        t0: Instant,
    }

    impl T {
        fn new(pending: Vec<PendingText>) -> Self {
            Self::with(pending, None)
        }

        fn with(pending: Vec<PendingText>, resumed: Option<&str>) -> Self {
            let t0 = Instant::now();
            Self {
                core: Core::new(
                    CoreConfig::default(),
                    ScreenConfig::default(),
                    t0,
                    pending,
                    resumed.map(str::to_string),
                ),
                t0,
            }
        }

        fn at(&self, secs: f64) -> Instant {
            self.t0 + Duration::from_secs_f64(secs)
        }

        fn output(&mut self, bytes: &[u8], secs: f64) -> Vec<Action> {
            let now = self.at(secs);
            self.core.on_output(bytes, now);
            self.completed_actions(now)
        }

        fn tick(&mut self, secs: f64) -> Vec<Action> {
            let now = self.at(secs);
            self.core.on_tick(now);
            self.completed_actions(now)
        }

        fn completed_actions(&mut self, now: Instant) -> Vec<Action> {
            let actions = self.core.take_actions();
            for a in &actions {
                if let Action::DeliveryWrite { id, token, .. } = a {
                    self.core.delivery_write_completed(*id, *token, now);
                }
            }
            actions
        }
    }

    fn brief(text: &str) -> Vec<PendingText> {
        vec![PendingText {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            text: text.to_string(),
        }]
    }

    /// 合成的 claude 输入框画面（整屏重画）。
    fn input_box(input: &str) -> Vec<u8> {
        format!(
            "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n╭──────────────────────╮\r\n│ ❯ {input} \r\n╰──────────────────────╯\r\n  ? for shortcuts"
        )
        .into_bytes()
    }

    fn trust_dialog() -> Vec<u8> {
        "\x1b[2J\x1b[HQuick safety check: Is this a project you created or one you trust?\r\n\r\n❯  No, exit\r\n   Yes, I trust this folder\r\n\r\nEnter to confirm · Esc to cancel"
            .as_bytes()
            .to_vec()
    }

    fn writes(actions: &[Action]) -> Vec<Vec<u8>> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Write(b)
                | Action::TerminalReply(b)
                | Action::DeliveryWrite { bytes: b, .. } => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    fn events(actions: &[Action]) -> Vec<SessionEvent> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    fn delivered(actions: &[Action], id: u64) -> bool {
        events(actions).iter().any(|e| {
            matches!(e, SessionEvent::Delivery { id: i, outcome: DeliveryOutcome::Delivered, .. } if *i == id)
        })
    }

    /// 从「输入框就绪」开始把一笔投完：画框 → 稳住 0.3s 写正文 → 回显 → 三拍提交 → 清空。
    fn drive_one(t: &mut T, text: &str, start: f64) -> Vec<Action> {
        let mut all = t.output(&input_box(""), start);
        all.extend(t.tick(start + 0.3));
        assert!(
            writes(&all).iter().any(|w| w.ends_with(text.as_bytes())),
            "输入框稳住后应写正文：{all:?}"
        );
        all.extend(t.output(&input_box(text), start + 0.9));
        assert_eq!(writes(&all).last().unwrap(), b" ", "落地后先发空格");
        all.extend(t.tick(start + 1.0));
        assert_eq!(writes(&all).last().unwrap(), &[0x7f]);
        all.extend(t.tick(start + 1.1));
        assert_eq!(writes(&all).last().unwrap(), b"\r");
        all.extend(t.output(&input_box(""), start + 1.2));
        all
    }

    #[test]
    fn terminal_replies_are_written_back_first() {
        let mut t = T::new(vec![]);
        let actions = t.output(b"\x1b[c", 0.0);
        assert_eq!(actions[0], Action::TerminalReply(b"\x1b[?6c".to_vec()));
    }

    #[test]
    fn brief_is_written_only_once_the_box_exists_then_submitted() {
        let mut t = T::new(brief("第一条任务"));
        let early = t.output(b"\x1b[2J\x1b[HClaude Code v9.9.9 starting...", 0.0);
        assert!(
            writes(&early).is_empty(),
            "没有输入框一个字节都不写：{early:?}"
        );
        let all = drive_one(&mut t, "第一条任务", 1.0);
        assert!(delivered(&all, BRIEF_DELIVERY_ID), "{all:?}");
    }

    /// 提交三拍发完之前，输入行的变化不算「已提交」：粘贴占位符会把那个空格算成多一行
    /// （`+2 lines` → `+3 lines`），退格之后又变回去——那一帧不能当成回车被接受。
    #[test]
    fn row_change_mid_submit_sequence_is_not_success() {
        let mut t = T::new(brief("一\n二"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        let submit = t.output(&input_box("[Pasted text #1 +2 lines]"), 0.9);
        assert_eq!(writes(&submit).last().unwrap(), b" ");
        let mid = t.output(&input_box("[Pasted text #1 +3 lines]"), 0.95);
        assert!(
            !delivered(&mid, BRIEF_DELIVERY_ID),
            "空格那一帧不算：{mid:?}"
        );
        assert_eq!(writes(&t.tick(1.0)), vec![vec![0x7f]]);
        t.output(&input_box("[Pasted text #1 +2 lines]"), 1.05);
        assert_eq!(writes(&t.tick(1.1)), vec![b"\r".to_vec()]);
        let done = t.output(&input_box(""), 1.2);
        assert!(delivered(&done, BRIEF_DELIVERY_ID), "{done:?}");
    }

    #[test]
    fn submission_late_paste_redraw_is_not_an_acknowledgement() {
        let mut t = T::new(brief("一\n二"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        t.output(&input_box("[Pasted text #1 +2 lines]"), 0.9);
        t.tick(1.0);
        assert_eq!(writes(&t.tick(1.1)), vec![b"\r".to_vec()]);
        let late = t.output(&input_box("[Pasted text #1 +3 lines]"), 1.2);
        assert!(
            !delivered(&late, BRIEF_DELIVERY_ID),
            "still occupied: {late:?}"
        );
        let empty = t.output(&input_box(""), 1.3);
        assert!(delivered(&empty, BRIEF_DELIVERY_ID), "{empty:?}");
        assert!(!delivered(&t.tick(1.4), BRIEF_DELIVERY_ID));
    }

    #[test]
    fn submission_old_empty_screen_is_not_a_fresh_enter_acknowledgement() {
        let mut t = T::new(brief("一\n二"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        t.output(&input_box("[Pasted text #1 +2 lines]"), 0.9);
        assert!(!delivered(
            &t.output(&input_box(""), 0.95),
            BRIEF_DELIVERY_ID
        ));
        t.tick(1.0);
        let enter = t.tick(1.1);
        assert_eq!(writes(&enter), vec![b"\r".to_vec()]);
        assert!(
            !delivered(&enter, BRIEF_DELIVERY_ID),
            "old screen predates Enter: {enter:?}"
        );
        assert!(!delivered(&t.tick(1.15), BRIEF_DELIVERY_ID));
        let empty = t.output(&input_box(""), 1.2);
        assert!(delivered(&empty, BRIEF_DELIVERY_ID), "{empty:?}");
    }

    #[test]
    fn pending_write_empty_initial_text_does_not_hold_the_following_task() {
        let mut pending = brief("");
        pending.push(PendingText {
            id: 8,
            subject: Subject::Steer,
            text: "BOT".into(),
        });
        let mut t = T::new(pending);
        t.output(&input_box(""), 0.0);
        assert_eq!(writes(&t.tick(0.3)), vec![b"BOT".to_vec()]);
    }

    #[test]
    fn pending_write_without_positive_progress_has_a_bounded_terminal_outcome() {
        let mut t = T::new(brief("BOT"));
        t.output(&input_box(""), 0.0);
        t.core.on_tick(t.at(0.3));
        let body = t.core.take_actions();
        assert_eq!(writes(&body), vec![b"BOT".to_vec()]);
        t.core.on_tick(t.at(2.81));
        let expired = t.core.take_actions();
        assert!(
            events(&expired).iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    outcome: DeliveryOutcome::Undelivered(_),
                    ..
                }
            )),
            "a writer making no progress cannot wait forever: {expired:?}"
        );
        assert!(writes(&expired).is_empty());
        t.core.on_output(&input_box(""), t.at(3.0));
        assert!(!delivered(&t.core.take_actions(), BRIEF_DELIVERY_ID));
    }

    #[test]
    fn pending_write_progress_is_not_completion_and_cannot_remove_the_total_bound() {
        let mut t = T::new(brief("BOT"));
        t.core.config.delivery.verify_window = Duration::from_secs(1);
        t.core.config.delivery.readiness_deadline = Duration::from_secs(2);
        t.output(&input_box(""), 0.0);
        t.core.on_tick(t.at(0.3));
        let body = t.core.take_actions();
        let Action::DeliveryWrite { id, token, .. } = body[0] else {
            panic!("body");
        };
        for secs in [0.9, 1.5, 2.1] {
            t.core.delivery_write_progress(id, token, t.at(secs));
            t.core.on_output(&input_box("BOT"), t.at(secs));
            assert!(
                writes(&t.core.take_actions()).is_empty(),
                "progress is not completed body"
            );
        }
        assert!(
            t.core
                .next_wakeup()
                .is_some_and(|deadline| deadline <= t.at(2.3))
        );
        t.core.on_tick(t.at(2.31));
        assert!(events(&t.core.take_actions()).iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Undelivered(_),
                ..
            }
        )));
    }

    #[test]
    fn pending_write_enter_timeout_cannot_be_resurrected_by_late_completion_or_empty() {
        let mut t = T::new(brief("BOT"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        t.output(&input_box("BOT"), 0.9);
        t.tick(1.0);
        t.core.on_tick(t.at(1.1));
        let enter = t.core.take_actions();
        let Action::DeliveryWrite { id, token, .. } = enter[0] else {
            panic!("Enter");
        };
        t.core.on_tick(t.at(3.61));
        let terminal = t.core.take_actions();
        assert!(events(&terminal).iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Unsubmitted(_),
                ..
            }
        )));
        t.core.delivery_write_completed(id, token, t.at(3.7));
        assert!(!delivered(
            &t.output(&input_box(""), 3.8),
            BRIEF_DELIVERY_ID
        ));
        assert!(writes(&t.tick(4.0)).is_empty());
    }

    #[test]
    fn actual_completion_paces_body_and_each_submit_beat() {
        let mut t = T::new(brief("BOT"));
        t.output(&input_box(""), 0.0);
        t.core.on_tick(t.at(0.3));
        let body = t.core.take_actions();
        let Action::DeliveryWrite { id, token, .. } = body[0] else {
            panic!("body");
        };
        t.core.delivery_write_progress(id, token, t.at(2.0));
        t.core.delivery_write_progress(id, token, t.at(4.0));
        t.core.on_output(&input_box("BOT"), t.at(5.0));
        assert!(
            writes(&t.core.take_actions()).is_empty(),
            "pending body cannot submit"
        );
        t.core.delivery_write_completed(id, token, t.at(5.1));
        assert!(
            writes(&t.tick(5.2)).is_empty(),
            "full body gap starts at completion"
        );
        t.core.on_tick(t.at(5.8));
        let space = t.core.take_actions();
        assert_eq!(writes(&space), vec![b" ".to_vec()]);
        assert!(
            writes(&t.tick(8.0)).is_empty(),
            "pending Space cannot enqueue Backspace"
        );
        let Action::DeliveryWrite { id, token, .. } = space[0] else {
            panic!("space");
        };
        t.core.delivery_write_completed(id, token, t.at(8.0));
        assert!(writes(&t.tick(8.05)).is_empty());
        t.core.on_tick(t.at(8.1));
        let backspace = t.core.take_actions();
        assert_eq!(writes(&backspace), vec![vec![0x7f]]);
        assert!(
            writes(&t.tick(10.0)).is_empty(),
            "pending Backspace cannot enqueue Enter"
        );
        let Action::DeliveryWrite { id, token, .. } = backspace[0] else {
            panic!("backspace");
        };
        t.core.delivery_write_completed(id, token, t.at(10.0));
        assert!(writes(&t.tick(10.05)).is_empty());
        assert_eq!(writes(&t.tick(10.1)), vec![b"\r".to_vec()]);
        assert!(delivered(
            &t.output(&input_box(""), 10.2),
            BRIEF_DELIVERY_ID
        ));
    }

    #[test]
    fn pending_enter_cannot_use_late_empty_as_delivery_ack() {
        let mut t = T::new(brief("一\n二"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        t.output(&input_box("[Pasted text #1 +2 lines]"), 0.9);
        t.tick(1.0);
        // Extract the action without actually completing the queued PTY write.
        t.core.on_tick(t.at(1.1));
        let enter = t.core.take_actions();
        assert_eq!(writes(&enter), vec![b"\r".to_vec()]);
        t.core.on_output(&input_box(""), t.at(1.2));
        let late = t.core.take_actions();
        assert!(
            !delivered(&late, BRIEF_DELIVERY_ID),
            "Enter still pending: {late:?}"
        );
        let Action::DeliveryWrite { id, token, .. } = enter[0] else {
            panic!("Enter action");
        };
        t.core.delivery_write_completed(id, token, t.at(1.3));
        assert!(!delivered(&t.tick(1.4), BRIEF_DELIVERY_ID));
        assert!(delivered(&t.output(&input_box(""), 1.5), BRIEF_DELIVERY_ID));
    }

    #[test]
    fn carriage_returns_inside_text_never_submit_early() {
        assert_eq!(sanitize_text("a\r\nb\rc\nd"), "a\nb\nc\nd");
        let mut t = T::new(vec![]);
        t.core.send(7, "一\r二", t.at(0.0));
        let mut actions = t.output(&input_box(""), 0.1);
        actions.extend(t.tick(0.4));
        assert!(
            writes(&actions).contains(&"一\n二".as_bytes().to_vec()),
            "{actions:?}"
        );
    }

    /// Review Focus 1：白板里贴进来的终端输出带 ESC / Ctrl-C——绝不能以按键的形式到达 claude。
    #[test]
    fn control_characters_in_text_are_made_inert() {
        assert_eq!(
            sanitize_text("red \x1b[31mX\x1b[0m\tok\x03\x00\x7f\u{9b}"),
            "red \u{241b}[31mX\u{241b}[0m\tok\u{2403}\u{2400}\u{2421}\u{fffd}"
        );
        let mut t = T::new(vec![]);
        t.core.send(3, "贴进来的 \x1b[2J 和 \x03", t.at(0.0));
        let mut actions = t.output(&input_box(""), 0.1);
        actions.extend(t.tick(0.4));
        let body = writes(&actions).concat();
        assert!(!body.is_empty(), "应当写了正文：{actions:?}");
        assert!(
            !body.iter().any(|&b| b == 0x1b || b == 0x03),
            "正文里不许出现 ESC / Ctrl-C：{body:?}"
        );
    }

    #[test]
    fn retry_clears_the_input_row_with_ctrl_u() {
        let mut t = T::new(brief("x"));
        t.output(&input_box(""), 0.0);
        assert_eq!(writes(&t.tick(0.3)), vec![b"x".to_vec()]);
        let retry = t.tick(2.8);
        assert_eq!(writes(&retry), vec![b"\x15x".to_vec()]);
    }

    #[test]
    fn steers_queue_behind_the_one_in_flight() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        t.core.send(2, "乙", t.at(0.0));
        let first = drive_one(&mut t, "甲", 0.0);
        assert!(delivered(&first, 1));
        assert!(
            !writes(&first).iter().any(|w| w.ends_with("乙".as_bytes())),
            "第二笔要等第一笔收工"
        );
        let second = drive_one(&mut t, "乙", 1.5);
        assert!(delivered(&second, 2), "{second:?}");
    }

    #[test]
    fn dialog_blocks_the_brief_then_it_is_delivered_after_the_answer() {
        let mut t = T::new(brief("开场"));
        let mut all = t.output(&trust_dialog(), 0.0);
        for s in 1..=4 {
            all.extend(t.tick(s as f64));
        }
        assert!(writes(&all).is_empty(), "对话框在场一个字节都不投：{all:?}");
        let ev = events(&all);
        assert!(ev.iter().any(
            |e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(d)) if !d.numbered)
        ));
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Blocked(_),
                ..
            }
        )));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::BriefUndelivered)
        );
        assert_eq!(t.core.state(), DerivedState::Error);

        // 有人代答 → 输入框出现 → 自己补投；health 跟着清掉。
        let after = drive_one(&mut t, "开场", 5.0);
        assert!(delivered(&after, BRIEF_DELIVERY_ID), "{after:?}");
        assert!(events(&after).contains(&SessionEvent::Health(None)));
        assert!(events(&after).contains(&SessionEvent::Decision(DecisionEvent::Cleared)));
    }

    #[test]
    fn working_and_idle_edges() {
        let mut t = T::new(vec![]);
        let a = t.output(b"thinking", 0.0);
        assert_eq!(events(&a), vec![SessionEvent::Working(true)]);
        assert!(events(&t.tick(0.6)).is_empty());
        assert_eq!(events(&t.tick(1.2)), vec![SessionEvent::Working(false)]);
        assert_eq!(t.core.state(), DerivedState::Idle);
    }

    #[test]
    fn rate_limit_menu_is_answered_and_recovers_after_sustained_work() {
        let mut t = T::new(vec![]);
        let a = t.output(b"What do you want to do?\r\n\xe2\x9d\xaf 1. Stop and wait for limit to reset\r\n  2. Upgrade", 0.0);
        assert_eq!(writes(&a), vec![b"1".to_vec()]);
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::RateLimited)
        );
        assert_eq!(t.core.state(), DerivedState::RateLimited);
        assert!(writes(&t.tick(0.1)).is_empty());
        assert_eq!(writes(&t.tick(0.25)), vec![b"\r".to_vec()]);
        assert_eq!(t.core.pending_decision(), None, "限流菜单不报给人");
        // 安静一阵，再连续干活 6s 以上 = 已恢复。
        t.tick(2.0);
        let mut recovered = Vec::new();
        for i in 0..=14 {
            let s = 3.0 + i as f64 * 0.5;
            recovered.extend(t.output(b".", s));
            recovered.extend(t.tick(s));
        }
        assert!(
            events(&recovered).contains(&SessionEvent::Health(None)),
            "{recovered:?}"
        );
    }

    #[test]
    fn usage_limit_marks_the_exit_as_hit_limit() {
        let mut t = T::new(vec![]);
        t.output(b"You've reached your usage limit", 0.0);
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit)
        );
        t.core.on_exit(ExitStatusKind::Code(1), t.at(30.0));
        let ev = events(&t.core.take_actions());
        assert!(
            ev.contains(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(1),
                stopped_by_user: false,
                hit_limit: true,
            })),
            "{ev:?}"
        );
    }

    #[test]
    fn silent_child_is_reported_stalled_at_the_deadline() {
        let mut t = T::new(vec![]);
        assert!(
            t.tick(24.0)
                .iter()
                .all(|a| !matches!(a, Action::Event(SessionEvent::Health(_))))
        );
        t.tick(25.0);
        let h = t.core.health().unwrap();
        assert_eq!(
            (h.kind, h.launch_verdict),
            (HealthKind::LaunchFailed, Some(LaunchVerdict::Stalled))
        );
        assert_eq!(t.core.state(), DerivedState::LaunchFailed);
    }

    #[test]
    fn dying_before_any_output_is_a_launch_failure_unless_we_stopped_it() {
        let mut t = T::new(vec![]);
        t.core.on_exit(ExitStatusKind::Code(1), t.at(0.5));
        let h = t.core.health().unwrap();
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(h.detail.contains("exit 1"), "{}", h.detail);

        let mut t = T::new(vec![]);
        t.core.stop(t.at(0.5));
        t.core.on_exit(ExitStatusKind::Signal(15), t.at(0.6));
        assert_eq!(t.core.health(), None, "我们自己停的不算拉起失败");
        assert!(
            events(&t.core.take_actions()).contains(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Signal(15),
                stopped_by_user: true,
                hit_limit: false,
            }))
        );
    }

    #[test]
    fn exit_reports_lost_steers_but_stop_does_not() {
        let mut t = T::new(vec![]);
        t.output(b"busy", 0.0);
        t.core.send(1, "第一条指令", t.at(0.1));
        t.core.send(2, "第二条", t.at(0.1));
        t.core.on_exit(ExitStatusKind::Code(0), t.at(1.0));
        let ev = events(&t.core.take_actions());
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(
                    e,
                    SessionEvent::Delivery {
                        outcome: DeliveryOutcome::Abandoned(_),
                        ..
                    }
                ))
                .count(),
            2
        );
        let h = t.core.health().unwrap();
        assert_eq!(h.kind, HealthKind::PromptUndelivered);
        assert!(
            h.detail.contains("2 条") && h.detail.contains("第一条指令"),
            "{}",
            h.detail
        );

        let mut t = T::new(vec![]);
        t.output(b"busy", 0.0);
        t.core.send(1, "x", t.at(0.1));
        t.core.stop(t.at(0.2));
        assert_eq!(t.core.health(), None);
    }

    #[test]
    fn sending_to_an_exited_session_is_reported_not_dropped() {
        let mut t = T::new(vec![]);
        t.output(b"x", 0.0);
        t.core.on_exit(ExitStatusKind::Code(0), t.at(1.0));
        t.core.take_actions();
        t.core.send(5, "迟到的指令", t.at(2.0));
        let ev = events(&t.core.take_actions());
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                id: 5,
                outcome: DeliveryOutcome::Abandoned(_),
                ..
            }
        )));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::PromptUndelivered)
        );
    }

    #[test]
    fn resume_refusal_is_recognised_only_for_our_id_and_only_early() {
        let said = format!("{NO_CONVERSATION_MARKER}: abc-123");
        let mut t = T::with(vec![], Some("abc-123"));
        let ev = events(&t.output(said.as_bytes(), 1.0));
        assert!(
            ev.contains(&SessionEvent::ResumeRefused {
                id: "abc-123".into(),
                said: said.clone()
            }),
            "{ev:?}"
        );

        let mut t = T::with(vec![], Some("abc-123"));
        let ev = events(&t.output(format!("{NO_CONVERSATION_MARKER}: other").as_bytes(), 1.0));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. }))
        );

        let mut t = T::with(vec![], Some("abc-123"));
        t.tick(5.1);
        let ev = events(&t.output(said.as_bytes(), 5.2));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
            "窗口外不认"
        );

        let mut t = T::new(vec![]);
        let ev = events(&t.output(said.as_bytes(), 1.0));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
            "没续跑就不看"
        );
    }

    /// 人已经叫停之后才出现的拒绝原话不再报：`ResumeRefused` 的意思是「要换新号重起」，
    /// 而停下来的会话不会重起（每条 `ResumeRefused` 后面都该跟一条 `Spawned`）。
    #[test]
    fn resume_refusal_after_stop_is_not_reported() {
        let said = format!("{NO_CONVERSATION_MARKER}: abc-123");
        let mut t = T::with(vec![], Some("abc-123"));
        t.output(b"starting", 0.1);
        t.core.stop(t.at(0.5));
        let mut ev = events(&t.core.take_actions());
        ev.extend(events(&t.output(said.as_bytes(), 1.0)));
        ev.extend(events(&t.tick(1.2)));
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
            "{ev:?}"
        );
    }

    /// Review Focus 3：attach 的客户端把终端改窄了，那行原话在屏上折成两行——照样认出。
    #[test]
    fn refusal_is_recognised_even_when_wrapped_on_a_narrow_screen() {
        let id = "0b3c5d7e-1111-4222-8333-944455556666";
        let t0 = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig {
                cols: 40,
                ..ScreenConfig::default()
            },
            t0,
            vec![],
            Some(id.to_string()),
        );
        core.on_output(format!("{NO_CONVERSATION_MARKER}: {id}\r\n").as_bytes(), t0);
        assert!(
            resume_rejection(&core.screen_text(25), id).is_none(),
            "前提：屏上确实折成了两行"
        );
        let ev = events(&core.take_actions());
        assert!(
            ev.iter()
                .any(|e| matches!(e, SessionEvent::ResumeRefused { id: i, .. } if i == id)),
            "{ev:?}"
        );
    }

    /// spec §18 的裁定：只看屏幕。菜单的字节里每行只有 LF（字节尾窗认得出），答完后屏幕
    /// 已经清掉——待决策必须随屏幕清除，不能被尾窗里的旧菜单拖住。
    #[test]
    fn answered_menu_clears_with_the_screen_not_the_byte_tail() {
        let mut t = T::new(vec![]);
        let menu =
            "\x1b[10;1HDo you want to proceed?\n\x1b[11;1H\u{276f} 1. Yes\n\x1b[12;1H  2. No\n";
        t.output(menu.as_bytes(), 0.0);
        t.tick(0.6);
        let shown = t.tick(3.7);
        assert!(
            events(&shown)
                .iter()
                .any(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_))))
        );
        t.output("\x1b[2J\x1b[H\u{23fa} ok\n".as_bytes(), 4.0);
        let cleared = t.tick(4.6);
        assert!(
            events(&cleared).contains(&SessionEvent::Decision(DecisionEvent::Cleared)),
            "{cleared:?}"
        );
    }

    #[test]
    fn next_wakeup_covers_scheduled_keys() {
        let mut t = T::new(vec![]);
        t.output(&rate_limit_menu(), 0.0);
        assert_eq!(t.core.next_wakeup(), Some(t.at(0.25)));
    }

    /// 合成的限流菜单（整屏重画，光标停在「Stop and wait」上）。
    fn rate_limit_menu() -> Vec<u8> {
        "\x1b[2J\x1b[H╭──────────────────────────────────────────╮\r\n│ What do you want to do?                  │\r\n│                                          │\r\n│ \u{276f} 1. Stop and wait for limit to reset     │\r\n│   2. Upgrade your plan                   │\r\n╰──────────────────────────────────────────╯\r\n  Enter to confirm · Esc to cancel"
            .as_bytes()
            .to_vec()
    }

    /// 控制器裁定（c）：屏上真摆着限流菜单、光标在「Stop and wait」上 → 答「1」再回车。
    #[test]
    fn rate_limit_menu_on_screen_is_answered() {
        let mut t = T::new(vec![]);
        let a = t.output(&rate_limit_menu(), 0.0);
        assert_eq!(writes(&a), vec![b"1".to_vec()], "{a:?}");
        assert_eq!(writes(&t.tick(0.25)), vec![b"\r".to_vec()]);
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::RateLimited)
        );
    }

    /// 控制器裁定（a）：冷却期里又来过一次短语（没触发、留在尾窗里），屏幕早翻过去了；
    /// 冷却过后随便一块输出让扫描器拿旧尾窗再触发——屏上没有菜单，一个键都不按。
    #[test]
    fn stale_rate_limit_tail_after_cooldown_types_nothing_without_the_menu_on_screen() {
        let feeds: [(&[u8], f64); 3] = [
            (
                "\x1b[2J\x1b[H\u{23fa} it said: Stop and wait for limit to reset\r\n".as_bytes(),
                5.0,
            ),
            ("\x1b[2J\x1b[H\u{23fa} ok\r\n".as_bytes(), 10.0),
            (b".", 40.0),
        ];
        let mut t = T::new(vec![]);
        t.output(&rate_limit_menu(), 0.0);
        t.tick(0.25);

        // 前提：同样的字节、同样的时刻，扫描器在 40s 确实又触发了。
        let mut probe = RateLimitScanner::default();
        assert!(probe.feed(&rate_limit_menu(), t.at(0.0)));
        let fired: Vec<bool> = feeds.iter().map(|(b, s)| probe.feed(b, t.at(*s))).collect();
        assert_eq!(fired, vec![false, false, true], "前提：旧尾窗在冷却后触发");

        let mut after = Vec::new();
        for (bytes, s) in feeds {
            after.extend(t.output(bytes, s));
            after.extend(t.tick(s));
        }
        for s in [40.25, 40.6, 41.2, 45.0] {
            after.extend(t.tick(s));
        }
        assert!(writes(&after).is_empty(), "屏上没菜单不按键：{after:?}");
    }

    /// 控制器裁定（b）：对话里回显了限流短语，屏上摆着的却是权限菜单（光标在「1. Yes」）——
    /// 不能替人按「1」批掉权限；它照常报给人。
    #[test]
    fn rate_limit_phrase_echoed_over_a_permission_menu_types_nothing() {
        let mut t = T::new(vec![]);
        let screen = "\x1b[2J\x1b[H\u{23fa} You ran /rate-limit-options earlier\r\n\r\nDo you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\n";
        let mut all = t.output(screen.as_bytes(), 0.0);
        for s in [0.25, 0.6, 1.2, 3.5] {
            all.extend(t.tick(s));
        }
        assert!(writes(&all).is_empty(), "不许替人批权限：{all:?}");
        assert!(
            events(&all)
                .iter()
                .any(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_)))),
            "{all:?}"
        );
        assert_ne!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::RateLimited)
        );
    }

    /// 扫描器因为一句回显触发了，同一块里屏幕翻成了别的菜单——光标也停在「Stop and wait…」上，
    /// 但屏上没有任何限流短语：不是限流菜单，不按。
    #[test]
    fn lookalike_menu_without_a_rate_limit_phrase_on_screen_types_nothing() {
        let mut t = T::new(vec![]);
        let bytes = "\u{23fa} about /rate-limit-options\r\n\x1b[2J\x1b[HWhat next?\r\n\u{276f} 1. Stop and wait for the build\r\n  2. Keep going\r\n";
        let mut probe = RateLimitScanner::default();
        assert!(probe.feed(bytes.as_bytes(), t.at(0.0)), "前提：扫描器触发");
        let mut all = t.output(bytes.as_bytes(), 0.0);
        for s in [0.25, 0.6, 1.2] {
            all.extend(t.tick(s));
        }
        assert!(writes(&all).is_empty(), "{all:?}");
    }

    /// 短语先到（`/rate-limit-options` 回显）、菜单下一块才画完：扫描器已经在冷却里，
    /// 但这次触发还没兑现——菜单一上屏就答，且只答一次。
    #[test]
    fn rate_limit_menu_drawn_after_the_phrase_is_answered_once_it_is_on_screen() {
        let mut t = T::new(vec![]);
        let first = t.output(b"\x1b[2J\x1b[H> /rate-limit-options\r\n", 0.0);
        assert!(writes(&first).is_empty(), "菜单还没上屏：{first:?}");
        let menu = "What do you want to do?\r\n\u{276f} 1. Stop and wait for limit to reset\r\n  2. Upgrade your plan";
        let second = t.output(menu.as_bytes(), 0.05);
        assert_eq!(writes(&second), vec![b"1".to_vec()], "{second:?}");
        let mut rest = t.output(b" ", 0.1);
        rest.extend(t.tick(0.3));
        rest.extend(t.tick(1.0));
        assert_eq!(writes(&rest), vec![b"\r".to_vec()], "只答一次：{rest:?}");
    }

    /// 写完正文输入框没回来：报一次 `Stalled`（不是终局），输入框回来后照常提交、投递成功。
    #[test]
    fn stalled_delivery_is_reported_and_continues_when_the_box_returns() {
        let mut t = T::new(brief("体"));
        t.output(&input_box(""), 0.0);
        assert_eq!(writes(&t.tick(0.3)), vec!["体".as_bytes().to_vec()]);
        let mut stalled = t.output("\x1b[2J\x1b[H\u{23fa} redrawing".as_bytes(), 0.5);
        stalled.extend(t.tick(2.9));
        stalled.extend(t.tick(3.5));
        let reports: Vec<SessionEvent> = events(&stalled)
            .into_iter()
            .filter(|e| matches!(e, SessionEvent::Delivery { .. }))
            .collect();
        assert_eq!(reports.len(), 1, "只报一次：{stalled:?}");
        assert!(
            matches!(
                &reports[0],
                SessionEvent::Delivery {
                    outcome: DeliveryOutcome::Stalled(_),
                    ..
                }
            ),
            "{stalled:?}"
        );
        assert!(!DeliveryOutcome::Stalled(String::new()).is_final());
        assert!(writes(&stalled).is_empty(), "等它回来，不重投：{stalled:?}");
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::BriefUndelivered)
        );

        let back = t.output(&input_box("体"), 4.0);
        assert_eq!(writes(&back), vec![b" ".to_vec()], "{back:?}");
        assert_eq!(writes(&t.tick(4.1)), vec![vec![0x7f]]);
        assert_eq!(writes(&t.tick(4.2)), vec![b"\r".to_vec()]);
        let done = t.output(&input_box(""), 4.3);
        assert!(delivered(&done, BRIEF_DELIVERY_ID), "{done:?}");
        assert!(events(&done).contains(&SessionEvent::Health(None)));
    }

    #[test]
    fn resize_is_forwarded_once_and_degenerate_sizes_are_ignored() {
        let mut t = T::new(vec![]);
        t.core.resize(120, 40);
        assert_eq!(
            t.core.take_actions(),
            vec![Action::Resize {
                cols: 120,
                lines: 40
            }]
        );
        t.core.resize(1, 40);
        assert!(t.core.take_actions().is_empty());
    }

    #[test]
    fn preview_is_short_and_single_line() {
        assert_eq!(preview("a\nb"), "a b");
        assert_eq!(preview(&"字".repeat(50)).chars().count(), 41);
    }

    fn permission_menu() -> Vec<u8> {
        "\x1b[2J\x1b[HBash(rm -rf build)\r\n\r\nDo you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\n"
            .as_bytes()
            .to_vec()
    }

    /// 修正 1：空格发出后、退格 / 回车到点前，屏上弹出权限框——排好的提交拍不许照发
    /// （回车会替人选中「1. Yes」）。
    #[test]
    fn queued_submit_beats_are_dropped_when_a_dialog_appears() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        t.output(&input_box(""), 0.0);
        assert_eq!(writes(&t.tick(0.3)), vec!["甲".as_bytes().to_vec()]);
        let space = t.output(&input_box("甲"), 0.9);
        assert_eq!(writes(&space), vec![b" ".to_vec()]);
        let mut after = t.output(&permission_menu(), 0.95);
        for s in [1.0, 1.1, 1.2, 1.5] {
            after.extend(t.tick(s));
        }
        let sent = writes(&after).concat();
        assert!(
            !sent.contains(&0x7f) && !sent.contains(&b'\r'),
            "权限框上不许发退格 / 回车：{after:?}"
        );
    }

    /// 修正 1：限流应答的回车到点时菜单已经不在（比如「1」直接选中关掉了它、或换成了权限框）——不发。
    #[test]
    fn rate_limit_carriage_return_is_dropped_when_the_menu_is_gone() {
        let mut t = T::new(vec![]);
        assert_eq!(
            writes(&t.output(&rate_limit_menu(), 0.0)),
            vec![b"1".to_vec()]
        );
        t.output(&permission_menu(), 0.1);
        let mut after = t.tick(0.25);
        after.extend(t.tick(0.6));
        assert!(writes(&after).is_empty(), "{after:?}");
    }

    /// 修正 2：正文去毒后才写进去，屏上回显的是去毒后的样子——回读（长正文、提示符滚出屏）
    /// 必须拿去毒后的那份比对。
    #[test]
    fn read_back_compares_the_sanitised_body() {
        let mut t = T::new(brief("红色的字\x1b[0m"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        // 提示符滚出了屏，屏上只剩正文的尾巴（claude 回显的是写进去的、去毒后的字）。
        let screen = format!("\x1b[2J\x1b[H{}\r\n", sanitize_text("红色的字\x1b[0m"));
        let submit = t.output(screen.as_bytes(), 0.9);
        assert_eq!(writes(&submit), vec![b" ".to_vec()], "{submit:?}");
    }

    /// 修正 3：排着按键时投递机冻住，它的期限可能早已过去；`next_wakeup` 不能给一个
    /// ≤ 上一次 `now` 的时刻（驱动层会空转）。
    #[test]
    fn next_wakeup_is_never_in_the_past_while_keys_are_queued() {
        let mut t = T::new(brief("x"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        // 正文写了（not_before = 0.8），0.85 撞上限流菜单：排了 1.1 的回车，投递冻住。
        assert_eq!(
            writes(&t.output(&rate_limit_menu(), 0.85)),
            vec![b"1".to_vec()]
        );
        let next = t.core.next_wakeup().expect("排着回车");
        assert!(next > t.at(0.85), "不许给过去的时刻");
        assert_eq!(next, t.at(1.1));

        // 投递机的期限（not_before = 0.8）还在将来、早于排着的回车（0.95）：冻住时它到点也
        // 推进不了，不该为它白醒一次——按键排着时只认按键与非投递的定时。
        let mut t = T::new(brief("x"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        assert_eq!(
            writes(&t.output(&rate_limit_menu(), 0.7)),
            vec![b"1".to_vec()]
        );
        assert_eq!(t.core.next_wakeup(), Some(t.at(0.95)));

        // 别的定时同样不给过去的时刻：只来过输出、一拍都没打过，拉起自检（25s）没结算——
        // 30s 时它已过期，下一次唤醒是空闲边沿（31s）。
        let mut t = T::new(vec![]);
        t.output(b"x", 30.0);
        assert_eq!(t.core.next_wakeup(), Some(t.at(31.0)));
    }

    /// 用光标定位摆词画的限流菜单：屏上是整句，字节尾窗里词与词之间没有空格
    /// （`Stopandwaitfor…`），扫描器认不出。
    fn rate_limit_menu_positioned() -> Vec<u8> {
        "\x1b[2J\x1b[1;1HWhat\x1b[1;6Hdo\x1b[1;9Hyou\x1b[1;13Hwant\x1b[1;18Hto\x1b[1;21Hdo?\x1b[2;1H\u{276f}\x1b[2;3H1.\x1b[2;6HStop\x1b[2;11Hand\x1b[2;15Hwait\x1b[2;20Hfor\x1b[2;24Hlimit\x1b[2;30Hto\x1b[2;33Hreset\x1b[3;3H2.\x1b[3;6HUpgrade"
            .as_bytes()
            .to_vec()
    }

    /// 修正 4：限流菜单答完消失、冷却期里又出现——不答（30s 内答过就不再按，防止连按
    /// 打乱它的菜单）；冷却过后再出现——再答，哪怕扫描器这次没触发（屏幕才是判据，
    /// 扫描器只是附加触发，不是前提）。
    #[test]
    fn rate_limit_menu_reshown_is_answered_again_only_after_the_cooldown() {
        let mut probe = RateLimitScanner::default();
        assert!(
            !probe.feed(&rate_limit_menu_positioned(), Instant::now()),
            "前提：扫描器认不出光标定位画的菜单"
        );
        let mut t = T::new(vec![]);
        assert_eq!(
            writes(&t.output(&rate_limit_menu(), 0.0)),
            vec![b"1".to_vec()]
        );
        t.tick(0.25);
        t.output("\x1b[2J\x1b[H\u{23fa} waiting\r\n".as_bytes(), 1.0);
        let within = t.output(&rate_limit_menu_positioned(), 10.0);
        let mut within_all = within.clone();
        within_all.extend(t.tick(10.6));
        assert!(
            writes(&within_all).is_empty(),
            "冷却期内不答：{within_all:?}"
        );
        t.output("\x1b[2J\x1b[H\u{23fa} waiting\r\n".as_bytes(), 11.0);
        let again = t.output(&rate_limit_menu_positioned(), 31.0);
        assert_eq!(writes(&again), vec![b"1".to_vec()], "{again:?}");
    }

    /// 修正 5a：撞额度之后，投递的报告（被对话框挡住）不能把额度 health 盖掉；
    /// 投递成功也不能把「撞过额度」抹掉。
    #[test]
    fn delivery_reports_do_not_overwrite_quota_health_or_its_timestamp() {
        let mut t = T::new(brief("开场"));
        t.output(b"You've reached your usage limit", 0.0);
        let mut all = t.output(&trust_dialog(), 0.1);
        for s in 1..=4 {
            all.extend(t.tick(s as f64));
        }
        assert!(events(&all).iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Blocked(_),
                ..
            }
        )));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit)
        );
        let after = drive_one(&mut t, "开场", 5.0);
        assert!(delivered(&after, BRIEF_DELIVERY_ID), "{after:?}");
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit)
        );
        t.core.on_exit(ExitStatusKind::Code(1), t.at(30.0));
        assert!(events(&t.core.take_actions()).iter().any(|e| matches!(
            e,
            SessionEvent::Exited(ExitInfo {
                hit_limit: true,
                ..
            })
        )));
    }

    /// 修正 5b：额度 health 被清掉（重置唤醒到点）之后 600s 内退出，仍记「撞过额度」。
    #[test]
    fn hit_limit_survives_clearing_the_quota_health() {
        let mut t = T::new(vec![]);
        t.output(b"You've reached your usage limit", 0.0);
        t.core.clear_quota_health(t.at(10.0));
        assert_eq!(t.core.health(), None);
        t.core.on_exit(ExitStatusKind::Code(1), t.at(20.0));
        assert!(events(&t.core.take_actions()).iter().any(|e| matches!(
            e,
            SessionEvent::Exited(ExitInfo {
                hit_limit: true,
                ..
            })
        )),);
    }

    /// 修正 5c：起来即死时手上有指令——health 是拉起失败，不能被「指令没送到」盖掉。
    #[test]
    fn launch_failure_on_exit_is_not_overwritten_by_lost_steers() {
        let mut t = T::new(vec![]);
        t.core.send(1, "第一条", t.at(0.1));
        t.core.on_exit(ExitStatusKind::Code(1), t.at(0.5));
        let ev = events(&t.core.take_actions());
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                id: 1,
                outcome: DeliveryOutcome::Abandoned(_),
                ..
            }
        )));
        let h = t.core.health().unwrap();
        assert_eq!(
            (h.kind, h.launch_verdict),
            (HealthKind::LaunchFailed, Some(LaunchVerdict::DiedSilently))
        );
    }

    /// 修正 6：已经叫停、还没收到退出状态时来的指令——当场报没送到，不排队。
    #[test]
    fn send_after_stop_is_abandoned_at_once() {
        let mut t = T::new(vec![]);
        t.output(b"busy", 0.0);
        t.core.stop(t.at(0.2));
        t.core.take_actions();
        t.core.send(9, "晚到的", t.at(0.3));
        let ev = events(&t.core.take_actions());
        assert!(
            ev.iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    id: 9,
                    outcome: DeliveryOutcome::Abandoned(_),
                    ..
                }
            )),
            "{ev:?}"
        );
        assert_eq!(t.core.health(), None, "是人叫停的，不留 health");
        let mut later = t.output(&input_box(""), 0.4);
        later.extend(t.tick(1.0));
        t.core.on_exit(ExitStatusKind::Signal(15), t.at(1.1));
        later.extend(t.core.take_actions());
        assert!(writes(&later).is_empty(), "{later:?}");
        assert!(
            !events(&later)
                .iter()
                .any(|e| matches!(e, SessionEvent::Delivery { id: 9, .. })),
            "不再报第二次：{later:?}"
        );
    }

    fn final_outcomes(actions: &[Action], id: u64) -> Vec<DeliveryOutcome> {
        events(actions)
            .into_iter()
            .filter_map(|e| match e {
                SessionEvent::Delivery { id: i, outcome, .. } if i == id && outcome.is_final() => {
                    Some(outcome)
                }
                _ => None,
            })
            .collect()
    }

    /// 空格之后弹了权限框、人 90s 多都没答：对话框在屏期间这一笔不终结（终结了正文就留在
    /// 输入框里，下一笔拼在后面成了「甲乙」）。框答掉、输入框带着「甲」回来，补发回车投出去，
    /// 下一笔「乙」单独提交。
    #[test]
    fn an_unanswered_dialog_does_not_end_the_delivery_or_glue_the_next_one_on() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        assert_eq!(
            writes(&t.output(&input_box("甲"), 0.9)),
            vec![b" ".to_vec()]
        );
        let mut waiting = t.output(&permission_menu(), 0.95);
        let mut s = 1.0;
        while s < 200.0 {
            waiting.extend(t.tick(s));
            s += 1.0;
        }
        assert!(
            final_outcomes(&waiting, 1).is_empty(),
            "对话框在屏期间不终结：{:?}",
            events(&waiting)
        );
        assert!(writes(&waiting).is_empty(), "{waiting:?}");
        t.core.send(2, "乙", t.at(199.5));
        let mut back = t.output(&input_box("甲"), 200.0);
        back.extend(t.tick(200.1));
        back.extend(t.tick(200.2));
        back.extend(t.output(&input_box(""), 200.3));
        assert_eq!(
            writes(&back),
            vec![b" ".to_vec(), vec![0x7f], b"\r".to_vec()],
            "甲补发回车：{back:?}"
        );
        assert!(delivered(&back, 1), "{back:?}");
        let second = drive_one(&mut t, "乙", 201.0);
        assert!(delivered(&second, 2), "{second:?}");
        assert!(
            writes(&second).contains(&"乙".as_bytes().to_vec()),
            "乙单独写：{second:?}"
        );
    }

    /// 一笔以非「已投递」的终局收场、而正文已经写进过输入框：残留可能还在。下一笔的第一次
    /// 写入也要先 Ctrl-U，不然两段拼成一条。
    #[test]
    fn residue_of_a_failed_delivery_is_cleared_before_the_next_one() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        assert_eq!(
            writes(&t.output(&input_box("甲"), 0.9)),
            vec![b" ".to_vec()]
        );
        // 输入框没了、也没有对话框：退格 / 回车被撤回，之后一直不回来 → 终局。
        let mut gone = t.output("\x1b[2J\x1b[H\u{23fa} redrawing".as_bytes(), 0.95);
        let mut s = 1.0;
        while s < 100.0 {
            gone.extend(t.tick(s));
            s += 1.0;
        }
        let finals = final_outcomes(&gone, 1);
        assert!(
            matches!(finals.as_slice(), [DeliveryOutcome::Unsubmitted(_)]),
            "前提：第一笔以没提交收场：{:?}",
            events(&gone)
        );
        t.core.send(2, "乙", t.at(100.0));
        let mut next = t.output(&input_box("甲"), 100.0);
        next.extend(t.tick(100.3));
        assert_eq!(
            writes(&next),
            vec!["\u{15}乙".as_bytes().to_vec()],
            "残留先清掉：{next:?}"
        );
    }

    /// 起得慢的 claude：25s 没吐字报了拉起卡住，之后终于有输出——拉起失败要撤掉（它是活的）。
    #[test]
    fn launch_stall_clears_once_output_arrives() {
        let mut t = T::new(vec![]);
        t.tick(25.0);
        assert_eq!(t.core.state(), DerivedState::LaunchFailed);
        let late = t.output(b"Claude Code starting", 26.0);
        assert!(
            events(&late).contains(&SessionEvent::Health(None)),
            "{late:?}"
        );
        assert_eq!(t.core.health(), None);
        assert_ne!(t.core.state(), DerivedState::LaunchFailed);
    }

    /// 长正文把提示符顶出了屏：屏上只剩正文尾巴，没有输入行。
    fn body_only(body: &str) -> Vec<u8> {
        format!("\x1b[2J\x1b[H{body}\r\n").into_bytes()
    }

    /// 正文尾巴还在屏上，下面弹了个权限框。
    fn body_then_dialog(body: &str) -> Vec<u8> {
        format!(
            "\x1b[2J\x1b[H{body}\r\n\r\nDo you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\n"
        )
        .into_bytes()
    }

    /// 修正轮 2 之 3：凭正文尾巴起的提交，退格与回车照样发出去。
    #[test]
    fn long_text_submit_beats_are_sent_when_only_the_body_tail_is_visible() {
        let mut t = T::new(brief("红色的字"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        assert_eq!(
            writes(&t.output(&body_only("红色的字"), 0.9)),
            vec![b" ".to_vec()]
        );
        assert_eq!(writes(&t.tick(1.0)), vec![vec![0x7f]]);
        assert_eq!(writes(&t.tick(1.1)), vec![b"\r".to_vec()]);
        let done = t.output(&input_box(""), 1.2);
        assert!(delivered(&done, BRIEF_DELIVERY_ID), "{done:?}");
    }

    /// 修正轮 2 之 1：长正文那一路，空格之后弹了权限框，退格 / 回车被撤回——这一轮没提交。
    /// 框答掉、输入框带着正文回来：不是「已投递」，重发一轮提交，输入行变了才算投递成功。
    #[test]
    fn dropped_long_text_submit_is_not_delivered_and_is_resent() {
        let mut t = T::new(brief("红色的字"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        assert_eq!(
            writes(&t.output(&body_only("红色的字"), 0.9)),
            vec![b" ".to_vec()]
        );
        let mut dropped = t.output(&body_then_dialog("红色的字"), 0.95);
        dropped.extend(t.tick(1.0));
        dropped.extend(t.tick(1.1));
        assert!(writes(&dropped).is_empty(), "{dropped:?}");
        let back = t.output(&input_box("红色的字"), 2.0);
        assert!(
            !delivered(&back, BRIEF_DELIVERY_ID),
            "回车没发过，不算投递：{back:?}"
        );
        assert_eq!(writes(&back), vec![b" ".to_vec()], "重发一轮提交：{back:?}");
        assert_eq!(writes(&t.tick(2.1)), vec![vec![0x7f]]);
        assert_eq!(writes(&t.tick(2.2)), vec![b"\r".to_vec()]);
        let done = t.output(&input_box(""), 2.3);
        assert!(delivered(&done, BRIEF_DELIVERY_ID), "{done:?}");
    }

    /// 修正轮 2 之 2：正文尾巴在屏上、下面摆着权限框——空格不许写（会被当成按键吃进框里）。
    #[test]
    fn visible_body_over_a_dialog_gets_no_space() {
        let mut t = T::new(brief("红色的字"));
        t.output(&input_box(""), 0.0);
        t.tick(0.3);
        let mut all = t.output(&body_then_dialog("红色的字"), 0.9);
        for s in [1.0, 1.1, 1.5, 2.0] {
            all.extend(t.tick(s));
        }
        assert!(writes(&all).is_empty(), "{all:?}");
    }

    /// 修正轮 2 之 4：额度 health 只由它自己的恢复路径清掉；认证探测的来回不能把它盖掉或清掉。
    #[test]
    fn auth_probe_transitions_do_not_clear_quota_health() {
        let mut t = T::new(vec![]);
        t.output(b"You've reached your usage limit", 0.0);
        t.core.on_auth(AuthState::AuthenticationRequired, t.at(1.0));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit)
        );
        t.core.on_auth(AuthState::Authenticated, t.at(3.0));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::UsageLimit),
            "额度还没恢复"
        );
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/../term/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("缺夹具 {path}: {e}"))
    }

    /// 真 claude 录下来的字节：未信任目录的首屏是信任框 → 不投、报 Blocked 与待决策。
    #[test]
    fn recorded_trust_screen_blocks_the_brief() {
        let mut t = T::new(brief("开场任务"));
        let mut all = Vec::new();
        for (k, chunk) in fixture("tui-claude-trust.bin").chunks(64).enumerate() {
            all.extend(t.output(chunk, k as f64 * 0.01));
        }
        for s in 1..=5 {
            all.extend(t.tick(s as f64));
        }
        assert!(
            !writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "信任框上绝不投正文"
        );
        let ev = events(&all);
        assert!(
            ev.iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    outcome: DeliveryOutcome::Blocked(_),
                    ..
                }
            )),
            "{ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_))))
        );
    }

    /// 这段录像（已信任目录）的末尾是人在输入框里打的一长句草稿：T10d 起正文不往草稿上写，等人清掉。
    /// （T10d 之前这条断言的是「写进去了」——那正是草稿闸要挡的事。）
    #[test]
    fn recorded_ready_screen_with_a_typed_draft_holds_the_brief() {
        let mut t = T::new(brief("开场任务"));
        let raw = fixture("tui-claude-ready.bin");
        let cut = raw
            .windows("这是".len())
            .position(|w| w == "这是".as_bytes())
            .expect("录像里有那句草稿");
        // 草稿开始回显之前，人在终端里敲了字：经 raw 进来（外来输入，T10d fix R1）。前一段每块只隔 10ms，输入行还没
        // 稳住 300ms，开场正文还没写。
        let mut all = Vec::new();
        let (before, after) = raw.split_at(cut);
        let mut k = 0;
        for chunk in before.chunks(64) {
            all.extend(t.output(chunk, k as f64 * 0.01));
            k += 1;
        }
        t.core.raw("这是".as_bytes());
        all.extend(t.core.take_actions());
        for chunk in after.chunks(64) {
            all.extend(t.output(chunk, k as f64 * 0.01));
            k += 1;
        }
        all.extend(t.tick(3.0));
        all.extend(t.tick(200.0));
        assert!(
            !writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "草稿在框里，一个字都不写：{all:?}"
        );
        let ev = events(&all);
        assert!(ev.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery { outcome: DeliveryOutcome::Blocked(d), .. } if d == INPUT_DRAFT
        )));
        assert!(
            ev.iter().any(|e| matches!(
                e,
                // 录像没有样式：分不清是草稿还是占位提示，报「分不清」，不把那行字当成人的草稿转述（fix R3）。
                SessionEvent::InputDraft(Some(HeldDraft::Unknown))
            )),
            "{ev:?}"
        );
        assert!(
            !ev.iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    outcome: DeliveryOutcome::NotReady(_),
                    ..
                }
            )),
            "等人清草稿不算没就绪：{ev:?}"
        );
        assert_eq!(t.core.health(), None, "草稿不是会话的毛病");
    }

    /// 同一段录像截在人开始打字之前、占位提示按 claude 的样子上了暗淡样式：就绪屏上直接投。
    #[test]
    fn recorded_ready_screen_receives_the_brief() {
        let raw = fixture("tui-claude-ready.bin");
        let cut = raw
            .windows("这是".len())
            .position(|w| w == "这是".as_bytes())
            .expect("录像里有那句草稿");
        let styled = String::from_utf8_lossy(&raw[..cut])
            .replace("\u{a0}Try", "\u{a0}\x1b[2mTry")
            .replace("errors\"", "errors\"\x1b[22m");
        assert!(styled.contains("\x1b[2mTry"), "占位提示上了样式");
        let mut t = T::new(brief("开场任务"));
        let mut all = Vec::new();
        for (k, chunk) in styled.as_bytes().chunks(64).enumerate() {
            all.extend(t.output(chunk, k as f64 * 0.01));
        }
        all.extend(t.tick(3.0));
        assert!(
            writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "就绪屏上应写正文：{:?}",
            t.core.screen_text(25)
        );
    }

    // ---- T10d：安全按键（回答待决策框）与草稿闸 ----

    /// 有样式的 claude 输入框：空框 = 暗淡的占位提示；底下一条暗淡的提示条（真 claude 的屏总有样式）。
    fn styled_box(input: &str) -> Vec<u8> {
        let shown = if input.is_empty() {
            "\x1b[2mTry \"write a test\"\x1b[22m".to_string()
        } else {
            input.to_string()
        };
        format!(
            "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {shown}\r\n────────────────────\r\n\x1b[2m  ? for shortcuts\x1b[22m"
        )
        .into_bytes()
    }

    fn numbered_menu(prompt: &str, cursor: usize) -> Vec<u8> {
        let mark = |i: usize| if i == cursor { "❯" } else { " " };
        format!(
            "\x1b[2J\x1b[H{prompt}\r\n{} 1. Yes\r\n{} 2. No\r\n{} 3. Ask later",
            mark(0),
            mark(1),
            mark(2)
        )
        .into_bytes()
    }

    fn cursor_list(prompt: &str, cursor: usize) -> Vec<u8> {
        let mark = |i: usize| if i == cursor { "❯" } else { " " };
        format!(
            "\x1b[2J\x1b[H{prompt}\r\n\r\n{}  No, exit\r\n{}  Yes, I trust this folder\r\n{}  Later\r\n\r\nEnter to confirm · Esc to cancel",
            mark(0),
            mark(1),
            mark(2)
        )
        .into_bytes()
    }

    fn shown(t: &T) -> PendingDecision {
        blocking_dialog(&t.core.screen.screen_rows()).expect("屏上有框")
    }

    fn step(bytes: &[u8], selected_before: Option<usize>) -> AnswerStep {
        AnswerStep {
            bytes: bytes.to_vec(),
            selected_before,
        }
    }

    fn answers(actions: &[Action]) -> Vec<(u64, Option<AnswerOutcome>)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::AnswerDone { id, outcome } => Some((*id, outcome.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn answer_cannot_verify_before_actual_pty_write_completion() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core
            .answer(71, expect, vec![step(b"2", Some(0))], t.at(0.1));
        let pending = t.core.take_actions();
        assert_eq!(
            writes(&pending),
            vec![b"2".to_vec()],
            "key is issued to the writer"
        );
        // Do not use T::tick/completed_actions: no driver syscall has happened.
        t.core.on_tick(t.at(0.7));
        let observed = t.core.take_actions();
        assert!(
            answers(&observed).is_empty(),
            "an issued but unwritten key cannot yield Answered: {observed:?}"
        );
    }

    #[test]
    fn answer_next_key_gap_does_not_start_at_queue_insertion() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(
            72,
            expect,
            vec![step(b"2", Some(0)), step(b"\r", Some(0))],
            t.at(0.1),
        );
        assert_eq!(writes(&t.core.take_actions()), vec![b"2".to_vec()]);
        t.core.on_tick(t.at(0.3));
        let observed = t.core.take_actions();
        assert!(
            writes(&observed).is_empty(),
            "the second key must await the first actual completion plus its gap: {observed:?}"
        );
    }

    fn pending_answer(t: &mut T, id: u64) -> DeliveryWriteToken {
        let expect = shown(t).fingerprint();
        t.core
            .answer(id, expect, vec![step(b"\r", Some(0))], t.at(0.1));
        t.core
            .take_actions()
            .into_iter()
            .find_map(|a| match a {
                Action::DeliveryWrite { token, .. } => Some(token),
                _ => None,
            })
            .expect("real typed Answer action")
    }

    #[test]
    fn answer_positive_progress_before_deadline_renews_but_exact_deadline_does_not() {
        for (delta, renews) in [(2.499, true), (2.5, false)] {
            let mut t = T::new(vec![]);
            t.output(&numbered_menu("Proceed?", 0), 0.0);
            let token = pending_answer(&mut t, 90);
            t.core.delivery_write_progress(90, token, t.at(0.1 + delta));
            assert_eq!(t.core.delivery_write_pending(90, token), renews);
            let events = t.core.take_actions();
            if renews {
                assert!(answers(&events).is_empty());
            } else {
                assert_eq!(
                    answers(&events),
                    vec![(90, Some(AnswerOutcome::NeedsTerminal))]
                );
            }
        }
    }

    #[test]
    fn answer_noise_and_old_same_id_token_cannot_renew_the_new_write_lease() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let old = pending_answer(&mut t, 90);
        // Noise is fresh output but is not physical input progress.
        t.core.on_output(b"\x1b[0m", t.at(2.5));
        t.core.take_actions();
        t.core.on_tick(t.at(2.6));
        assert_eq!(
            answers(&t.core.take_actions()),
            vec![(90, Some(AnswerOutcome::NeedsTerminal))]
        );
        let expect = shown(&t).fingerprint();
        t.core
            .answer(90, expect, vec![step(b"\r", Some(0))], t.at(3.0));
        let fresh = t
            .core
            .take_actions()
            .into_iter()
            .find_map(|a| match a {
                Action::DeliveryWrite { token, .. } => Some(token),
                _ => None,
            })
            .unwrap();
        assert_ne!(
            old, fresh,
            "same id has a different physical write generation"
        );
        assert!(!t.core.actual_write_guard(90, old, 0, t.at(4.0)));
        t.core.delivery_write_progress(90, old, t.at(5.0));
        t.core.delivery_write_completed(90, old, t.at(5.0));
        assert!(t.core.delivery_write_pending(90, fresh));
        t.core.on_tick(t.at(5.5));
        assert_eq!(
            answers(&t.core.take_actions()),
            vec![(90, Some(AnswerOutcome::NeedsTerminal))]
        );
    }

    #[test]
    fn answer_positive_partial_progress_cannot_extend_total_job_past_ninety_seconds() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let token = pending_answer(&mut t, 90);
        for second in (2..90).step_by(2) {
            t.core
                .delivery_write_progress(90, token, t.at(second as f64));
            assert!(t.core.delivery_write_pending(90, token));
        }
        t.core.delivery_write_progress(90, token, t.at(89.9));
        assert!(t.core.delivery_write_pending(90, token));
        t.core.delivery_write_completed(90, token, t.at(90.1));
        assert!(!t.core.delivery_write_pending(90, token));
        assert_eq!(
            answers(&t.core.take_actions()),
            vec![(90, Some(AnswerOutcome::NeedsTerminal))]
        );
    }

    fn pending_body_with_partial_prefix() -> (T, DeliveryWriteToken) {
        let mut t = T::new(brief("BODY"));
        t.output(&styled_box(""), 0.0);
        t.core.on_tick(t.at(0.31));
        let token = t
            .core
            .take_actions()
            .into_iter()
            .find_map(|a| match a {
                Action::DeliveryWrite { token, .. } => Some(token),
                _ => None,
            })
            .unwrap();
        t.core
            .delivery_write_progress(BRIEF_DELIVERY_ID, token, t.at(0.32));
        (t, token)
    }

    #[test]
    fn partial_body_accepts_only_authoritative_empty_owned_prefix_or_exact_paste_placeholder() {
        for (frame, expected) in [
            (styled_box(""), true),
            (styled_box("BO"), true),
            (styled_box("[Pasted text #1 +2 lines]"), true),
            (styled_box("someone else's draft"), false),
            (input_box("BO"), false),
        ] {
            let (mut t, token) = pending_body_with_partial_prefix();
            t.core.on_output(&frame, t.at(0.4));
            t.core.take_actions();
            assert_eq!(
                t.core
                    .actual_write_guard(BRIEF_DELIVERY_ID, token, 2, t.at(0.5)),
                expected
            );
            assert_eq!(
                t.core.delivery_write_pending(BRIEF_DELIVERY_ID, token),
                expected
            );
            if !expected {
                let actions = t.core.take_actions();
                assert!(events(&actions).iter().any(|e| matches!(
                    e,
                    SessionEvent::Delivery {
                        outcome: DeliveryOutcome::Unsubmitted(_),
                        ..
                    }
                )));
            }
        }
    }

    #[test]
    fn partial_body_unknown_or_unrelated_text_cannot_clear_or_replay_into_next_job() {
        for frame in [input_box("BO"), styled_box("someone else's draft")] {
            let (mut t, token) = pending_body_with_partial_prefix();
            t.core.send(2, "NEXT", t.at(0.35));
            t.core.on_output(&frame, t.at(0.4));
            t.core.take_actions();
            assert!(
                !t.core
                    .actual_write_guard(BRIEF_DELIVERY_ID, token, 2, t.at(0.5))
            );
            t.core.take_actions();
            t.core
                .delivery_write_completed(BRIEF_DELIVERY_ID, token, t.at(0.6));
            t.core.on_tick(t.at(2.0));
            let actions = t.core.take_actions();
            assert!(
                writes(&actions).is_empty(),
                "unknown source cannot become Ctrl-U/replay authority: {actions:?}"
            );
            assert!(matches!(t.core.draft_reported, Some(HeldDraft::Unknown)));
            // Existing explicit human clear + fresh full Empty is a recovery path; hold
            // is not a permanent veto on every future job.
            t.core.raw(b"\x15");
            t.core.take_actions();
            t.output(&styled_box(""), 2.1);
            let resumed = t.tick(2.4);
            assert_eq!(
                writes(&resumed),
                vec![b"NEXT".to_vec()],
                "no implicit Ctrl-U or full BODY replay"
            );
        }
    }

    #[test]
    fn stop_and_exit_revoke_guarded_token_before_late_callbacks() {
        for stopped in [false, true] {
            let mut t = T::new(vec![]);
            t.output(&numbered_menu("Proceed?", 0), 0.0);
            let token = pending_answer(&mut t, 96);
            if stopped {
                t.core.stop(t.at(0.2));
            } else {
                t.core.on_exit(ExitStatusKind::Code(0), t.at(0.2));
            }
            let closed = t.core.take_actions();
            assert_eq!(answers(&closed), vec![(96, None)]);
            assert!(!t.core.actual_write_guard(96, token, 0, t.at(0.3)));
            t.core.delivery_write_progress(96, token, t.at(0.3));
            t.core.delivery_write_completed(96, token, t.at(0.3));
            t.core.on_tick(t.at(1.0));
            let late = t.core.take_actions();
            assert!(writes(&late).is_empty());
            assert!(
                answers(&late).is_empty(),
                "no resurrection/Answered: {late:?}"
            );
        }
    }

    #[test]
    fn digits_on_a_numbered_box_are_answered_and_verified() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Do you want to proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(7, expect, vec![step(b"2", None)], t.at(0.1));
        let first = t.completed_actions(t.at(0.1));
        assert_eq!(
            writes(&first),
            vec![b"2".to_vec()],
            "第一下当场按（按之前回读过）"
        );
        assert!(answers(&first).is_empty(), "还没回读");
        t.output(&styled_box(""), 0.2);
        assert!(
            answers(&t.tick(0.5)).is_empty(),
            "最后一下之后 500ms 才回读"
        );
        let done = t.tick(0.6);
        assert_eq!(
            answers(&done),
            vec![(
                7,
                Some(AnswerOutcome::Answered {
                    cleared: true,
                    now: None
                })
            )]
        );
    }

    /// 第一下按完、第二下之前框换了：第二下不按，回 `Conflict` 并带上现在的框。
    #[test]
    fn a_box_replaced_between_two_keys_stops_the_answer() {
        let mut t = T::new(vec![]);
        t.output(&cursor_list("Trust this folder?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(
            1,
            expect,
            vec![step(b"\x1b[B", Some(0)), step(b"\r", Some(1))],
            t.at(0.0),
        );
        let mut log = t.completed_actions(t.at(0.0));
        assert_eq!(writes(&log), vec![b"\x1b[B".to_vec()]);
        // 新框的高亮恰好也停在第二项（下一下预期的位置）：只有指纹能分出它不是原来那个框。
        log.extend(t.output(&numbered_menu("Run rm -rf build?", 1), 0.05));
        log.extend(t.tick(0.15));
        log.extend(t.tick(1.0));
        assert_eq!(
            writes(&log),
            vec![b"\x1b[B".to_vec()],
            "第二下（回车）绝不能按到新框上"
        );
        match answers(&log).as_slice() {
            [(1, Some(AnswerOutcome::Conflict { now: Some(d) }))] => {
                assert_eq!(d.prompt, "Run rm -rf build?")
            }
            other => panic!("{other:?}"),
        }
    }

    /// 高亮被别人挪了（人在终端里按了方向键）：按之前核 `selected`，对不上就停手。
    #[test]
    fn a_highlight_moved_by_someone_else_stops_the_answer() {
        let mut t = T::new(vec![]);
        t.output(&cursor_list("Trust this folder?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(
            2,
            expect,
            vec![step(b"\x1b[B", Some(0)), step(b"\r", Some(1))],
            t.at(0.0),
        );
        let mut log = t.completed_actions(t.at(0.0));
        // 同一个框（指纹相同），高亮却停在第三项。
        log.extend(t.output(&cursor_list("Trust this folder?", 2), 0.05));
        log.extend(t.tick(0.15));
        assert_eq!(writes(&log), vec![b"\x1b[B".to_vec()], "{log:?}");
        assert!(matches!(
            answers(&log).as_slice(),
            [(2, Some(AnswerOutcome::Conflict { now: Some(d) }))] if d.selected == Some(2)
        ));
        // 高亮如期挪到第二项：照常按完。
        let mut t = T::new(vec![]);
        t.output(&cursor_list("Trust this folder?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(
            3,
            expect,
            vec![step(b"\x1b[B", Some(0)), step(b"\r", Some(1))],
            t.at(0.0),
        );
        let mut log = t.completed_actions(t.at(0.0));
        log.extend(t.output(&cursor_list("Trust this folder?", 1), 0.05));
        log.extend(t.tick(0.15));
        assert_eq!(writes(&log), vec![b"\x1b[B".to_vec(), b"\r".to_vec()]);
        log.extend(t.output(&styled_box(""), 0.2));
        log.extend(t.tick(0.7));
        assert!(matches!(
            answers(&log).as_slice(),
            [(3, Some(AnswerOutcome::Answered { cleared: true, .. }))]
        ));
    }

    /// 回答期间不投正文；回答排队一次一个；会话被停掉 = 没答成（`None`）。
    #[test]
    fn answers_queue_and_are_dropped_when_the_session_stops() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core
            .answer(1, expect.clone(), vec![step(b"1", None)], t.at(0.0));
        t.core.answer(2, expect, vec![step(b"2", None)], t.at(0.0));
        let first = t.core.take_actions();
        assert_eq!(writes(&first), vec![b"1".to_vec()], "第二个排在第一个后面");
        t.core.stop(t.at(0.1));
        let stopped = t.core.take_actions();
        assert_eq!(answers(&stopped), vec![(1, None), (2, None)]);
        t.core
            .answer(3, "x".into(), vec![step(b"1", None)], t.at(0.2));
        assert_eq!(answers(&t.core.take_actions()), vec![(3, None)]);
    }

    /// 草稿闸：输入框里有人打的字 → 正文不写（Blocked input_draft、InputDraft 事件）；人清掉之后照常投出去。
    #[test]
    fn a_body_waits_behind_a_typed_draft_and_goes_out_once_it_is_cleared() {
        let mut t = T::new(vec![]);
        t.core.send(1, "唤醒正文", t.at(0.0));
        // 人在终端里打了字（外来输入）：草稿闸才按样式判。
        t.core.raw("人".as_bytes());
        let mut log = t.core.take_actions();
        log.retain(|a| matches!(a, Action::Event(_)));
        log.extend(t.output(&styled_box("人打了一半"), 0.0));
        for s in 1..=5 {
            log.extend(t.tick(s as f64 * 0.4));
        }
        assert!(writes(&log).is_empty(), "一个字节都不写：{log:?}");
        let ev = events(&log);
        assert!(ev.contains(&SessionEvent::InputDraft(Some(HeldDraft::Text(
            "人打了一半".into()
        )))));
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, SessionEvent::Delivery { outcome: DeliveryOutcome::Blocked(d), .. } if d == INPUT_DRAFT))
                .count(),
            1,
            "只报一次：{ev:?}"
        );
        // 草稿改了：再说一次（只在变了时发）。
        let more = t.output(&styled_box("人打了一半又多打几个"), 2.2);
        assert!(
            events(&more).contains(&SessionEvent::InputDraft(Some(HeldDraft::Text(
                "人打了一半又多打几个".into()
            ))))
        );
        // 人清掉了：占位提示回来 → 草稿事件清零、稳住 0.3s 后写正文。
        let cleared = t.output(&styled_box(""), 3.0);
        assert!(events(&cleared).contains(&SessionEvent::InputDraft(None)));
        let wrote = t.tick(3.4);
        assert_eq!(writes(&wrote), vec!["唤醒正文".as_bytes().to_vec()]);
        let mut rest = t.output(&styled_box("唤醒正文"), 4.0);
        rest.extend(t.tick(4.1));
        rest.extend(t.tick(4.2));
        rest.extend(t.output(&styled_box(""), 4.3));
        assert!(delivered(&rest, 1), "{rest:?}");
    }

    /// 占位提示（暗淡）不是草稿：照常投。
    #[test]
    fn a_placeholder_box_is_delivered_normally() {
        let mut t = T::new(vec![]);
        t.core.send(1, "甲", t.at(0.0));
        let mut log = t.output(&styled_box(""), 0.0);
        log.extend(t.tick(0.3));
        assert_eq!(writes(&log), vec!["甲".as_bytes().to_vec()]);
        log.extend(t.output(&styled_box("甲"), 0.9));
        log.extend(t.tick(1.0));
        log.extend(t.tick(1.1));
        log.extend(t.output(&styled_box(""), 1.2));
        assert!(delivered(&log, 1), "{log:?}");
        assert!(
            !events(&log)
                .iter()
                .any(|e| matches!(e, SessionEvent::InputDraft(_))),
            "{log:?}"
        );
    }

    // ---- T10d fix R1：输入来源是草稿闸的主信号 ----

    /// 占位提示的样式认不出来（这里：没样式的 `Try "…"`，读成 Text；以及一个认不出的浅灰 truecolor），但从开起来就
    /// 没有外来输入：没人能往框里打过字，照常投。
    #[test]
    fn a_fresh_session_with_an_unrecognised_placeholder_style_gets_its_brief() {
        for placeholder in [
            "Try \"write a test\"".to_string(),
            "\x1b[38;2;220;220;220mTry \"write a test\"\x1b[39m".to_string(),
        ] {
            let mut t = T::new(brief("开场任务"));
            let screen = |p: &str| {
                format!(
                    "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {p}\r\n────────────────────\r\n\x1b[2m  ? for shortcuts\x1b[22m"
                )
            };
            assert_ne!(
                {
                    let mut sc = Screen::new(ScreenConfig::default());
                    sc.feed(screen(&placeholder).as_bytes());
                    sc.input_draft()
                },
                InputDraft::Empty,
                "前提：这个占位提示读不成空框"
            );
            let mut all = t.output(screen(&placeholder).as_bytes(), 0.0);
            all.extend(t.tick(0.3));
            assert!(
                writes(&all)
                    .iter()
                    .any(|w| w.ends_with("开场任务".as_bytes())),
                "{placeholder:?}: {all:?}"
            );
            assert!(
                !events(&all)
                    .iter()
                    .any(|e| matches!(e, SessionEvent::InputDraft(Some(_)))),
                "{all:?}"
            );
        }
    }

    /// 外来输入之后框里有字 → 挡；框读成空（人清掉 / 发出去）→ 标记清掉、照常投；之后再出现认不出样式的字（没有新的
    /// 外来输入）不再挡。我们自己的打断与回答不算外来输入。
    #[test]
    fn foreign_input_arms_the_guard_until_the_box_reads_blank() {
        let mut t = T::new(vec![]);
        t.core.interrupt();
        let own = t.core.take_actions();
        assert_eq!(writes(&own), vec![vec![0x1b]], "打断写一个 Esc");
        t.core.send(1, "甲", t.at(0.0));
        let mut log = t.output(&styled_box("自己画的字"), 0.0);
        log.extend(t.tick(0.3));
        assert_eq!(
            writes(&log),
            vec!["甲".as_bytes().to_vec()],
            "没有外来输入：照常投"
        );
        log.extend(t.output(&styled_box("甲"), 0.9));
        log.extend(t.tick(1.0));
        log.extend(t.tick(1.1));
        log.extend(t.output(&styled_box(""), 1.2));
        assert!(delivered(&log, 1), "{log:?}");
        // 人在终端里敲了字。
        t.core.raw(b"x");
        t.core.take_actions();
        t.core.send(2, "乙", t.at(2.0));
        let mut held = t.output(&styled_box("x"), 2.0);
        for k in 1..=4 {
            held.extend(t.tick(2.0 + k as f64 * 0.3));
        }
        assert!(writes(&held).is_empty(), "外来输入之后有字：挡：{held:?}");
        assert!(
            events(&held).contains(&SessionEvent::InputDraft(Some(HeldDraft::Text("x".into()))))
        );
        // 框读成空：标记清掉，乙照常投出去。
        let mut back = t.output(&styled_box(""), 4.0);
        back.extend(t.tick(4.3));
        assert_eq!(writes(&back), vec!["乙".as_bytes().to_vec()]);
        back.extend(t.output(&styled_box("乙"), 4.9));
        back.extend(t.tick(5.0));
        back.extend(t.tick(5.1));
        back.extend(t.output(&styled_box(""), 5.2));
        assert!(delivered(&back, 2), "{back:?}");
        // 之后再出现认不出样式的字、没有新的外来输入：不挡。
        t.core.send(3, "丙", t.at(6.0));
        let mut again = t.output(&styled_box("claude 画的"), 6.0);
        again.extend(t.tick(6.3));
        assert_eq!(writes(&again), vec!["丙".as_bytes().to_vec()]);
    }

    // ---- T10d fix R3 ----

    #[test]
    fn raw_pending_echo_holds_an_already_settling_empty_row() {
        let mut t = T::new(vec![]);
        t.output(&styled_box(""), 0.0);
        t.core.send(1, "BOT", t.at(0.0));
        t.core.take_actions();
        t.core.raw(b"x");
        let raw = t.core.take_actions();
        assert_eq!(writes(&raw), vec![b"x".to_vec()]);
        assert!(events(&raw).contains(&SessionEvent::InputDraft(Some(HeldDraft::Unknown))));
        let held = t.tick(200.0);
        assert!(
            writes(&held).is_empty(),
            "no timeout may release old Empty: {held:?}"
        );
        assert_eq!(t.core.foreign_input, Foreign::Typed);
        assert!(!t.core.delivery_write_allowed(1));
    }

    #[test]
    fn empty_preserving_raw_controls_and_unknown_frames_stay_indeterminate() {
        for raw in [b"\x1b".as_slice(), b"\x15", b"\r", b"\x1b[A"] {
            let mut t = T::new(vec![]);
            t.output(&styled_box(""), 0.0);
            t.core.raw(raw);
            t.core.take_actions();
            t.core.send(1, "BOT", t.at(0.0));
            let mut log = t.core.take_actions();
            // A new Empty frame is still not evidence that these controls cleared a visible draft.
            log.extend(t.output(&styled_box(""), 1.0));
            log.extend(t.tick(200.0));
            assert!(writes(&log).is_empty(), "{raw:?}: {log:?}");
            assert_eq!(t.core.foreign_input, Foreign::Typed);
            assert_eq!(t.core.draft_held(None), Some(HeldDraft::Unknown));
        }
        let mut t = T::new(vec![]);
        t.core.raw(b"x");
        t.core.take_actions();
        // Unknown is not proof a human draft was visibly established; a subsequent empty frame stays held.
        t.output(&styled_box("\x1b[2mgrey\x1b[22mplain"), 0.1);
        assert_eq!(t.core.screen.input_draft(), InputDraft::Unknown);
        t.output(&styled_box(""), 0.2);
        assert_eq!(t.core.foreign_input, Foreign::Typed);
    }

    #[test]
    fn raw_after_a_body_or_each_submit_beat_stops_without_own_residue_cleanup() {
        for beat in 0..=3 {
            let mut t = T::new(vec![]);
            t.core.send(1, "BOT", t.at(0.0));
            t.output(&styled_box(""), 0.0);
            assert_eq!(writes(&t.tick(0.3)), vec![b"BOT".to_vec()]);
            if beat > 0 {
                assert_eq!(
                    writes(&t.output(&styled_box("BOT"), 0.9)),
                    vec![b" ".to_vec()]
                );
            }
            if beat > 1 {
                assert_eq!(writes(&t.tick(1.0)), vec![vec![0x7f]]);
            }
            if beat > 2 {
                assert_eq!(writes(&t.tick(1.1)), vec![b"\r".to_vec()]);
            }
            t.core.raw(b"x");
            let mut log = t.core.take_actions();
            t.core.send(2, "AFTER", t.at(1.2));
            log.extend(t.core.take_actions());
            log.extend(t.tick(10.0));
            assert_eq!(
                writes(&log),
                vec![b"x".to_vec()],
                "beat {beat}: no retry, cleanup or remaining submit: {log:?}"
            );
            assert!(events(&log).iter().any(|e| matches!(
                e,
                SessionEvent::Delivery {
                    id: 1,
                    outcome: DeliveryOutcome::Unsubmitted(_),
                    ..
                }
            )));
            assert!(!delivered(&log, 1));
            assert!(!t.core.input_dirty);
            assert!(!t.core.delivery_write_allowed(1));
            assert!(!t.core.delivery_write_allowed(2));
            // Late own echo / a blank redraw cannot clear a mixed-provenance collision.
            assert_eq!(t.core.foreign_input, Foreign::Conflicted);
            t.output(&styled_box("BOT"), 11.0);
            assert!(writes(&t.output(&styled_box(""), 12.0)).is_empty());
            assert_eq!(t.core.foreign_input, Foreign::Conflicted);
            t.core.raw(b"\x15");
            t.core.take_actions();
            t.output(&styled_box(""), 13.0);
            assert_eq!(writes(&t.tick(13.3)), vec![b"AFTER".to_vec()]);
        }
    }

    #[test]
    fn new_typing_cannot_borrow_the_previous_drafts_shown_evidence() {
        let mut t = T::new(vec![]);
        t.core.raw(b"x");
        t.core.take_actions();
        t.output(&styled_box("x"), 0.1);
        assert_eq!(t.core.foreign_input, Foreign::Shown);
        t.core.raw(b"y");
        t.core.take_actions();
        t.output(&styled_box("x"), 0.15);
        assert_eq!(
            t.core.foreign_input,
            Foreign::Typed,
            "unchanged old Text is not this raw's echo"
        );
        t.output(&styled_box(""), 0.2);
        t.core.send(1, "BOT", t.at(0.2));
        t.core.take_actions();
        assert_eq!(t.core.foreign_input, Foreign::Typed);
        assert!(writes(&t.tick(20.0)).is_empty());
        t.output(&styled_box("y"), 21.0);
        t.core.raw(b"\x15");
        t.core.take_actions();
        assert_eq!(t.core.foreign_input, Foreign::Clearing);
        assert!(
            writes(&t.tick(40.0)).is_empty(),
            "old Text cannot acknowledge clear"
        );
        t.output(&styled_box(""), 41.0);
        assert_eq!(writes(&t.tick(41.3)), vec![b"BOT".to_vec()]);
    }

    #[test]
    fn own_answers_interrupts_and_deliveries_do_not_arm_foreign_input() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(1, expect, vec![step(b"1", None)], t.at(0.0));
        t.core.take_actions();
        t.output(&styled_box(""), 0.1);
        t.tick(0.6);
        t.core.interrupt();
        t.core.take_actions();
        t.core.send(1, "BOT", t.at(1.0));
        let log = drive_one(&mut t, "BOT", 1.0);
        assert!(delivered(&log, 1), "{log:?}");
        assert_eq!(t.core.foreign_input, Foreign::None);
    }

    /// T10d fix M3：下一下排在上一下真正按下的时刻 + 间隔（拍子晚到时，后面的间隔不会被压没）。
    #[test]
    fn each_answer_key_waits_a_full_gap_after_the_previous_write() {
        let mut t = T::new(vec![]);
        t.output(&numbered_menu("Proceed?", 0), 0.0);
        let expect = shown(&t).fingerprint();
        t.core.answer(
            1,
            expect,
            vec![step(b"a", None), step(b"b", None), step(b"c", None)],
            t.at(0.0),
        );
        assert_eq!(writes(&t.completed_actions(t.at(0.0))), vec![b"a".to_vec()]);
        // 这一拍晚到了（本该 0.15）：第二下在 0.4 按下。
        assert_eq!(writes(&t.tick(0.4)), vec![b"b".to_vec()]);
        // 第三下在 0.4 + 0.15 = 0.55，不是预排的 0.3：0.45 时还不能按。
        assert!(writes(&t.tick(0.45)).is_empty());
        assert_eq!(writes(&t.tick(0.55)), vec![b"c".to_vec()]);
    }
}
