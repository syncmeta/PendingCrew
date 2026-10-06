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
    AnsiPlainTextTail, CURSOR_MARKERS, DecisionEvent, PendingDecision, PendingDecisionTracker,
    RATE_LIMIT_MENU_PHRASES, Screen, ScreenConfig, Snapshot, blocking_dialog, body_tail_visible,
    input_row, strip,
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
    /// 进程起来了（驱动层发；续跑被拒重起后会再发一次）。`resumed` = 用了 `--resume`。
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
    /// claude 拒绝 `--resume <id>`，原话 `said`。
    ResumeRefused {
        id: String,
        said: String,
    },
    Exited(ExitInfo),
}

/// 交给驱动层执行的动作，按顺序执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Write(Vec<u8>),
    Resize { cols: u16, lines: u16 },
    Event(SessionEvent),
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
}

/// 排好的按键是干什么的：到点发之前按用途再看一眼屏幕。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyPurpose {
    /// 提交三拍里的退格 / 回车。`via_body`：这一轮提交是凭「正文尾巴在屏上」起的
    /// （长正文把提示符滚出了屏），没有输入行可看。
    SubmitBeat { via_body: bool },
    /// 限流菜单自动应答「1」之后的回车。
    RateLimitAnswer,
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
    queue: VecDeque<PendingText>,
    keys: VecDeque<Key>,
    actions: Vec<Action>,
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
            queue: pending.into(),
            keys: VecDeque::new(),
            actions: Vec::new(),
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
        // 排着按键时投递机是冻住的（`step_delivery` 不推进），它的期限可能早已过去——不算。
        let delivery = if self.keys.is_empty() {
            self.in_flight
                .as_ref()
                .and_then(|f| f.machine.next_deadline())
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
        [key, delivery, launch, idle]
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

    pub fn screen_text(&self, max_lines: usize) -> String {
        self.screen.screen_text(max_lines)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.screen.snapshot()
    }

    /// PTY 来了一块输出。
    pub fn on_output(&mut self, bytes: &[u8], now: Instant) {
        if !self.running {
            return;
        }
        self.last_now = self.last_now.max(now);
        self.screen.feed(bytes);
        let replies = self.screen.take_replies();
        if !replies.is_empty() {
            // 终端回复（DA1 / CPR / OSC 11…）先回给子进程：它可能正卡着等。
            self.actions.push(Action::Write(replies));
        }
        self.last_output_at = Some(now);
        if !self.working {
            self.set_working(true, now);
        }
        if let Some(h) = self.quota.feed(bytes) {
            self.set_health(Some(h), now);
        }
        self.flush_due_keys(now);
        // 扫描器只是附加触发：它触发与否，按键前都要看屏幕；而屏幕每块输出、每拍都看，
        // 所以它的结果不改变判定（扫描器没认出的菜单——光标定位摆的词——照样会答）。
        let _scanner_fired = self.rate_limit.feed(bytes, now);
        self.answer_rate_limit_menu_if_shown(now);
        if self.resume_watch.is_some() {
            self.resume_tail.feed(bytes);
        }
        self.check_resume_refusal(now);
        self.step_delivery(now);
    }

    /// 定时拍（0.6s 节拍，或 `next_wakeup` 到点）。
    pub fn on_tick(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.last_now = self.last_now.max(now);
        self.flush_due_keys(now);
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

    /// 原样写字节（nudge / 人在终端里按键）。
    pub fn raw(&mut self, bytes: &[u8]) {
        if self.running && !bytes.is_empty() {
            self.actions.push(Action::Write(bytes.to_vec()));
        }
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
    pub fn stop(&mut self, now: Instant) {
        if !self.running {
            return;
        }
        self.stopped_by_user = true;
        self.launch_settled = true;
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

    /// health 只有一格：拉起失败 > 额度 / 限流 / 认证 > 投递报告。低档的不盖高档的
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
                .is_some_and(|cur| health_rank(cur.kind) > health_rank(h.kind))
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
    ///   提交拍全丢掉（回车会替人选中对话框的第一项）。投递机还在「提交」阶段，输入行回来、
    ///   期限到了会自己重发一轮。
    /// - 限流应答的回车：菜单不在了就不发（「1」可能已经直接选中关掉了它）。
    fn flush_due_keys(&mut self, now: Instant) {
        while self.keys.front().is_some_and(|k| k.at <= now) {
            let key = self.keys.pop_front().expect("刚看过非空");
            let rows = self.screen.screen_rows();
            let safe = match key.purpose {
                KeyPurpose::SubmitBeat { via_body } => {
                    blocking_dialog(&rows).is_none()
                        && if via_body {
                            self.in_flight
                                .as_ref()
                                .is_some_and(|f| body_tail_visible(&f.body, &rows))
                        } else {
                            input_row(&rows).is_some()
                        }
                }
                KeyPurpose::RateLimitAnswer => rate_limit_menu_on_screen(&rows),
            };
            if safe {
                self.actions.push(Action::Write(key.bytes));
            } else if let KeyPurpose::SubmitBeat { .. } = key.purpose {
                self.keys
                    .retain(|k| !matches!(k.purpose, KeyPurpose::SubmitBeat { .. }));
            }
        }
    }

    fn answer_rate_limit_menu(&mut self, now: Instant) {
        self.actions.push(Action::Write(b"1".to_vec()));
        self.schedule(
            now + self.config.rate_limit_answer_gap,
            b"\r".to_vec(),
            KeyPurpose::RateLimitAnswer,
        );
        if self.health.as_ref().map(|h| h.kind) != Some(HealthKind::RateLimited) {
            self.set_health(
                Some(Health::new(
                    HealthKind::RateLimited,
                    "Claude Code 撞到额度上限（rate-limit 菜单已自动应答「Stop and wait」）——等额度窗口重置后自动继续。",
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
            });
            self.step_delivery(now);
        }
    }

    fn step_delivery(&mut self, now: Instant) {
        // 提交三拍（空格 / 退格 / 回车）没发完之前不回读：中间那一帧（多了个空格）不作数。
        if !self.keys.is_empty() {
            return;
        }
        let Some(flight) = self.in_flight.as_mut() else {
            return;
        };
        let rows = self.screen.screen_rows();
        let row = input_row(&rows);
        let step = flight.machine.step(Observation {
            input_row: row.as_deref(),
            body_visible: body_tail_visible(&flight.body, &rows),
            dialog_present: blocking_dialog(&rows).is_some(),
            now,
        });
        match step {
            Step::Idle => {}
            Step::WriteBody { attempt } => {
                let mut bytes = Vec::new();
                if attempt > 1 {
                    // 重投前清掉输入行：上一笔若落了一半，两笔会叠成乱码。
                    bytes.push(0x15);
                }
                bytes.extend_from_slice(flight.body.as_bytes());
                self.actions.push(Action::Write(bytes));
            }
            Step::Submit { .. } => {
                // 正文是一次灌进去的，claude 会当成粘贴：单发回车可能只结束粘贴而不提交。
                // 先补一个空格再删掉（输入行判据 trim，不影响判定），隔拍再回车。
                self.actions.push(Action::Write(b" ".to_vec()));
                let gap = self.config.submit_key_gap;
                let purpose = KeyPurpose::SubmitBeat {
                    via_body: row.is_none(),
                };
                self.schedule(now + gap, vec![0x7f], purpose);
                self.schedule(now + gap * 2, b"\r".to_vec(), purpose);
            }
            Step::BlockedByDialog(detail) => self.report(DeliveryOutcome::Blocked(detail), now),
            Step::NotReady(detail) => self.report(DeliveryOutcome::NotReady(detail), now),
            Step::Stalled(detail) => self.report(DeliveryOutcome::Stalled(detail), now),
            Step::Undelivered(detail) => self.report(DeliveryOutcome::Undelivered(detail), now),
            Step::Unsubmitted(detail) => self.report(DeliveryOutcome::Unsubmitted(detail), now),
            Step::Delivered => self.report(DeliveryOutcome::Delivered, now),
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
        match &outcome {
            DeliveryOutcome::Delivered => {
                if self.health.as_ref().map(|h| h.kind) == Some(kind) {
                    self.set_health(None, now);
                }
            }
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

/// health 那一格的档位：拉起失败 > 额度 / 限流 / 认证 > 投递报告。
fn health_rank(kind: HealthKind) -> u8 {
    match kind {
        HealthKind::LaunchFailed => 3,
        HealthKind::UsageLimit | HealthKind::RateLimited | HealthKind::AuthRequired => 2,
        HealthKind::BriefUndelivered | HealthKind::PromptUndelivered => 1,
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
            self.core.take_actions()
        }

        fn tick(&mut self, secs: f64) -> Vec<Action> {
            let now = self.at(secs);
            self.core.on_tick(now);
            self.core.take_actions()
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
                Action::Write(b) => Some(b.clone()),
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
        assert_eq!(actions[0], Action::Write(b"\x1b[?6c".to_vec()));
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

    /// 已信任目录：录下来的就绪屏上直接投。
    #[test]
    fn recorded_ready_screen_receives_the_brief() {
        let mut t = T::new(brief("开场任务"));
        let mut all = Vec::new();
        for (k, chunk) in fixture("tui-claude-ready.bin").chunks(64).enumerate() {
            all.extend(t.output(chunk, k as f64 * 0.01));
        }
        all.extend(t.tick(3.0));
        assert!(
            writes(&all)
                .iter()
                .any(|w| w.ends_with("开场任务".as_bytes())),
            "就绪屏上应写正文"
        );
    }
}
