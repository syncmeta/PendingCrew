//! 一个 codex 会话的全部判断，不碰 IO（与 claude 的 `session::Core` 同一个形状）：
//! 喂一行 stdout / 一次定时拍 / 一条命令 / 账本结果 / 进程退出，吐出按顺序执行的
//! [`CodexAction`]。时间由调用方传入。
//!
//! - **握手**：`initialize` → `initialized` → 已记录 thread 则 `thread/resume`。
//!   生产 `strict_resume` 被拒时直接失败，绝不替换 thread 或重发首正文；独立旧底座 seam
//!   仍可显式测试历史 fallback。拿到 thread id 后先 [`CodexAction::Record`] 耐久落账，
//!   成功后才发 `Spawned`、独立原生标题请求及初始设置；初始设置 ACK 前扣住首正文。
//!   标题失败不重发正文，初始设置失败/不确定则拒绝正文且不自动重试。
//! - 人工请求保留原始身份与表单，等待精确答复；其他请求由 [`proto::answer_server_request`] 回复。
//! - **回合**：一次只开一个；有回合在跑、正在压缩、或上一个 `turn/start` 还没回，正文就排队。
//! - **健康**：只认协议字段（[`proto::health_from`]）；正常完成的回合清掉过时的额度 / 失败 / 版本 / 登录态 health。
//! - **每个请求都有期限**（[`Rpc::expire`]），到期按失败处理，会话不挂。
use std::collections::{BTreeSet, VecDeque};
#[path = "controls.rs"]
mod controls;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::human::{Form, RunnerRequest};
use crewd_term::{DecisionEvent, PendingDecision};
use serde_json::Value;

use crate::codex::proto::{self, Reply, ThreadOptions};
use crate::codex::rpc::{
    Incoming, ParseError, RequestId, Rpc, RpcFailure, error_line, notification_line, parse_line,
    response_line,
};
use crate::codex::turns::{Signals, Transcript, TurnTracker};
use crate::delivery::Subject;
use crate::driver::{Control, ControlOutcome, UntrustedContext};
use crate::health::{Health, HealthKind, LaunchVerdict};
use crate::resume::valid_agent_session_id;
use crate::session::{
    AnswerOutcome, DeliveryOutcome, DerivedState, ExitInfo, ExitStatusKind, Profile, SessionEvent,
    preview,
};

/// 期限与上限。
#[derive(Clone, Copy, Debug)]
pub struct CodexCoreConfig {
    /// `initialize` 的期限。
    pub initialize_timeout: Duration,
    /// `thread/start` / `thread/resume` 的期限（MCP server 起得慢时它会久一点）。
    pub thread_timeout: Duration,
    /// 其余请求（`turn/start`、`turn/interrupt`、设置、压缩）的期限。
    pub request_timeout: Duration,
    /// fast 切换：`thread/settings/update` 回了之后，等 `thread/settings/updated` 回显多久。
    pub settings_echo_timeout: Duration,
    /// 退出前这么久之内撞过额度 → `ExitInfo::hit_limit`。
    pub hit_limit_window: Duration,
    /// 结构化记录保留多少条。
    pub transcript_items: usize,
}

impl Default for CodexCoreConfig {
    fn default() -> Self {
        Self {
            initialize_timeout: Duration::from_secs(30),
            thread_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            settings_echo_timeout: Duration::from_secs(8),
            hit_limit_window: Duration::from_secs(600),
            transcript_items: 200,
        }
    }
}

/// 一条等着开回合的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingTurn {
    pub id: u64,
    pub subject: Subject,
    pub text: String,
    pub context: UntrustedContext,
}

/// 起一个会话要的东西（驱动层给）。
#[derive(Clone, Debug)]
pub struct Opening {
    pub pid: u32,
    pub cwd: PathBuf,
    pub options: ThreadOptions,
    /// 账本里记过的 thread id（有就先试 `thread/resume`）。
    pub recorded: Option<String>,
    /// `initialize.clientInfo.version`。
    pub client_version: String,
    /// 开场任务（就绪后第一个回合）。
    pub brief: Option<PendingTurn>,
    /// 构造时刻的 Unix 秒（`currentTime/read` 按它推算，核心不看墙钟）。
    pub unix_now: i64,
}

/// 启动没成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartFailure {
    /// 握手失败 / 超时 / 进程中途退出。
    Launch(Health),
    /// thread id 落账失败（起了也接不回来）。
    Ledger(String),
}

/// 交给驱动层按顺序执行的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexAction {
    ControlledWrite {
        line: String,
        permit: crate::driver::ManualWritePermit,
    },
    CodexDone {
        ticket: u64,
        outcome: super::control::CodexOutcome,
    },
    /// 往 stdin 写一行（不含换行）。
    Write(String),
    /// 把 thread id 落账，结果用 [`CodexCore::on_recorded`] 交回。
    Record(String),
    /// 一次 [`Control`] 的结果（`ticket` 是驱动层给的编号）。
    AnswerDone {
        ticket: u64,
        outcome: AnswerOutcome,
    },
    ControlDone {
        ticket: u64,
        outcome: ControlOutcome,
    },
    /// 启动失败：驱动层杀掉进程、把它变成 `StartError`。
    Failed(StartFailure),
    Event(SessionEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Initializing,
    Opening,
    Recording,
    Ready,
    Failed,
    Exited,
}

#[derive(Clone, Debug)]
enum Tag {
    Native {
        ticket: Option<u64>,
        op: super::control::CodexOperation,
    },
    PermissionCheck {
        ticket: Option<u64>, patch: crewd_proto::codex::CodexSettingsPatch,
        permit: Option<crate::driver::ManualWritePermit>, cursors: BTreeSet<String>, pages: u8,
    },
    NativeToolsSkills {
        ticket: u64,
        servers: Value,
    },
    Initialize,
    Resume(String),
    Start,
    TurnStart {
        id: u64,
        subject: Subject,
        text: String,
    },
    Interrupt,
    Settings {
        ticket: u64,
        op: Control,
    },
    Compact {
        ticket: Option<u64>,
    },
}

struct EchoWait {
    ticket: u64,
    want_fast: bool,
    deadline: Instant,
}

pub struct CodexCore {
    config: CodexCoreConfig,
    native_echo: Option<controls::NativeEcho>,
    native_settings: Value,
    native_settings_epoch: u64,
    native_name: Option<String>,
    native_history_mode: Option<String>,
    native_input_files:Vec<super::input::Prepared>,
    native_queued_inputs:std::collections::BTreeMap<u64,super::input::Prepared>,
    public_input_ids:
        std::collections::BTreeMap<u64, (String, Option<crate::driver::ManualWritePermit>)>,
    initial_settings_gate: bool,
    initial_settings_failed: bool,
    phase: Phase,
    pid: u32,
    cwd: PathBuf,
    options: ThreadOptions,
    recorded: Option<String>,
    created: Instant,
    unix_at_created: i64,
    rpc: Rpc<Tag>,
    thread_id: Option<String>,
    resumed: bool,
    fallback: Option<(String, String)>,
    opened_profile: Profile,
    tracker: TurnTracker,
    transcript: Transcript,
    queue: VecDeque<PendingTurn>,
    in_flight: bool,
    echo: Option<EchoWait>,
    /// 一次 fast 切换的 `thread/settings/update` 已发、还没回。
    fast_requested: bool,
    health: Option<Health>,
    quota_at: Option<Instant>,
    working: bool,
    stopped_by_user: bool,
    notified: BTreeSet<String>,
    human: VecDeque<(RequestId, Form)>,
    early: Vec<SessionEvent>,
    actions: Vec<CodexAction>,
}

impl CodexCore {
    pub(crate) fn initial_inputs(&mut self,inputs:super::input::Prepared){self.native_queued_inputs.insert(crate::session::BRIEF_DELIVERY_ID,inputs);}
    /// 建核心并发出 `initialize`。
    pub fn new(opening: Opening, config: CodexCoreConfig, now: Instant) -> Self {
        let mut queue = VecDeque::new();
        if let Some(brief) = opening.brief.filter(|b| !b.text.trim().is_empty()) {
            queue.push_back(brief);
        }
        let mut core = Self {
            config,
            native_echo: None,
            native_settings: Value::Null,
            native_settings_epoch: 0,
            native_name: None,
            native_history_mode: None,
            native_input_files:Vec::new(),
            native_queued_inputs:Default::default(),
            public_input_ids: Default::default(),
            initial_settings_gate: false,
            initial_settings_failed: false,
            phase: Phase::Initializing,
            pid: opening.pid,
            cwd: opening.cwd,
            options: opening.options,
            // 账里的号非法（`resume::valid_agent_session_id` 不认）= 当作没记过，开新 thread。
            recorded: opening
                .recorded
                .map(|r| r.trim().to_string())
                .filter(|r| valid_agent_session_id(r)),
            created: now,
            unix_at_created: opening.unix_now,
            rpc: Rpc::default(),
            thread_id: None,
            resumed: false,
            fallback: None,
            opened_profile: Profile::default(),
            tracker: TurnTracker::new(""),
            transcript: Transcript::new(config.transcript_items),
            queue,
            in_flight: false,
            echo: None,
            fast_requested: false,
            health: None,
            quota_at: None,
            working: false,
            stopped_by_user: false,
            notified: BTreeSet::new(),
            human: VecDeque::new(),
            early: Vec::new(),
            actions: Vec::new(),
        };
        let params = proto::initialize_params(&opening.client_version);
        core.request(
            Tag::Initialize,
            "initialize",
            &params,
            now,
            config.initialize_timeout,
        );
        core
    }

    pub fn take_actions(&mut self) -> Vec<CodexAction> {
        std::mem::take(&mut self.actions)
    }

    /// 下一个要醒来的时刻（最早的请求期限 / fast 回显期限）。
    pub fn next_wakeup(&self) -> Option<Instant> {
        let echo = self.echo.as_ref().map(|e| e.deadline);
        match (self.rpc.next_deadline(), echo) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    pub fn is_running(&self) -> bool {
        !matches!(self.phase, Phase::Exited | Phase::Failed)
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn health(&self) -> Option<&Health> {
        self.health.as_ref()
    }

    /// 在干活：有回合 / 在压缩 / `turn/start` 还没回 / 刚见到条目活动。
    pub fn is_working(&self) -> bool {
        self.working
    }

    pub fn state(&self) -> DerivedState {
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed)
        {
            return DerivedState::LaunchFailed;
        }
        if !self.is_running() {
            return DerivedState::Exited;
        }
        if let Some(h) = &self.health {
            return if h.kind.is_quota() {
                DerivedState::RateLimited
            } else {
                DerivedState::Error
            };
        }
        if !self.human.is_empty() {
            return DerivedState::AwaitingDecision;
        }
        if self.working {
            DerivedState::Working
        } else {
            DerivedState::Idle
        }
    }

    pub fn report_answer(&mut self, ticket: u64, outcome: AnswerOutcome) {
        self.actions
            .push(CodexAction::AnswerDone { ticket, outcome });
    }
    pub fn pending_decision(&self) -> Option<&PendingDecision> {
        self.human.front().map(|(_, f)| &f.decision)
    }

    pub fn runner_request(&self) -> Option<RunnerRequest> {
        self.human.front().map(|(_, f)| f.info())
    }

    /// Answer only the currently displayed request. No key fallback, retries or
    /// implicit grants. A fresh opaque fingerprint prevents reuse across restarts.
    pub fn answer_choice(&mut self, expect: &str, choice: usize, now: Instant) -> AnswerOutcome {
        let response = self
            .human
            .front()
            .and_then(|(_, f)| f.choices.get(choice))
            .cloned();
        let Some(response) = response else {
            return AnswerOutcome::Conflict {
                now: self.pending_decision().cloned(),
            };
        };
        self.respond_request(expect, response, now)
    }

    pub fn respond_request(
        &mut self,
        expect: &str,
        response: Value,
        now: Instant,
    ) -> AnswerOutcome {
        let Some((_, form)) = self.human.front() else {
            return AnswerOutcome::Conflict { now: None };
        };
        if !self.is_running()
            || self.stopped_by_user
            || form.decision.fingerprint() != expect
            || !form.valid_response(&response)
        {
            return AnswerOutcome::Conflict {
                now: self.pending_decision().cloned(),
            };
        }
        let before = self.pending_decision().cloned();
        let (id, _) = self.human.pop_front().expect("checked pending request");
        self.actions
            .push(CodexAction::Write(response_line(&id, &response)));
        self.human_changed(before);
        self.after_input(now);
        AnswerOutcome::Answered {
            cleared: true,
            now: self.pending_decision().cloned(),
        }
    }

    fn hold_human(&mut self, id: &RequestId, method: &str, params: &Value) -> bool {
        let Some(form) = Form::new(method, params) else {
            return false;
        };
        if self.phase != Phase::Ready
            || self.initial_settings_gate
            || self.initial_settings_failed
            || self.stopped_by_user
            || params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref()
            || params
                .get("turnId")
                .and_then(Value::as_str)
                .is_some_and(|turn| Some(turn) != self.tracker.active_turn())
        {
            self.actions.push(CodexAction::Write(error_line(
                id,
                -32602,
                "request no longer belongs to this active thread/turn",
            )));
            return true;
        }
        if let Some((_, old)) = self.human.iter().find(|(request, _)| request == id) {
            if old.method != method || old.params != *params {
                let before = self.pending_decision().cloned();
                self.human.retain(|(request, _)| request != id);
                self.human_changed(before);
                self.actions.push(CodexAction::Write(error_line(
                    id,
                    -32602,
                    "request ID reused with different contents",
                )));
            }
            return true;
        }
        if self.human.len() >= 32 {
            self.actions.push(CodexAction::Write(error_line(
                id,
                -32000,
                "too many pending human requests",
            )));
            self.event(SessionEvent::Notice(
                "Codex 人工待请求过多，新请求未被接受。".into(),
            ));
            return true;
        }
        let before = self.pending_decision().cloned();
        self.human.push_back((id.clone(), form));
        self.human_changed(before);
        true
    }

    fn human_changed(&mut self, before: Option<PendingDecision>) {
        let after = self.pending_decision().cloned();
        if before == after {
            return;
        }
        if before.is_some() {
            self.event(SessionEvent::Decision(DecisionEvent::Cleared));
        }
        if let Some(decision) = after {
            self.event(SessionEvent::Decision(DecisionEvent::Appeared(decision)));
        }
    }

    fn clear_human(&mut self, turn: Option<&str>) {
        let before = self.pending_decision().cloned();
        let mut removed = Vec::new();
        self.human.retain(|(id, f)| {
            let remove = turn
                .is_none_or(|turn| f.params.get("turnId").and_then(Value::as_str) == Some(turn));
            if remove {
                removed.push(id.clone());
            }
            !remove
        });
        for id in removed {
            self.actions.push(CodexAction::Write(error_line(
                &id,
                -32000,
                "human request cancelled",
            )));
        }
        self.human_changed(before);
    }

    /// 结构化记录的文本（`screen_text` 的 codex 版）。
    pub fn transcript_text(&self, max_lines: usize) -> String {
        self.transcript.text(max_lines)
    }

    /// 喂一行 stdout（不含换行）。
    pub fn on_line(&mut self, line: &str, now: Instant) {
        if line.trim().is_empty() {
            return;
        }
        match parse_line(line) {
            Ok(Incoming::Response { id, result }) => {
                if let Some((tag, _method, result)) = self.rpc.resolve(&id, result) {
                    self.on_result(tag, result, now);
                }
            }
            Ok(Incoming::Request { id, method, params }) => {
                if self.hold_human(&id, &method, &params) {
                    self.after_input(now);
                    return;
                }
                let unix = self.unix_at_created
                    + now.saturating_duration_since(self.created).as_secs() as i64;
                let outcome = proto::answer_server_request(&method, &params, unix);
                let reply = match &outcome.reply {
                    Reply::Result(v) => response_line(&id, v),
                    Reply::Error { code, message } => error_line(&id, *code, message),
                };
                self.actions.push(CodexAction::Write(reply));
                if outcome.auth_required {
                    self.set_health(Some(proto::auth_required_health()), now);
                }
                if let Some(notice) = outcome.notice
                    && self.notified.insert(method)
                {
                    self.event(SessionEvent::Notice(notice));
                }
            }
            Ok(Incoming::Notification { method, params }) => {
                self.on_notification(&method, &params, now)
            }
            Err(e) => {
                if self.notified.insert("<malformed>".to_string()) {
                    let what = match e {
                        ParseError::NotJson(l) | ParseError::Unclassifiable(l) => l,
                    };
                    self.event(SessionEvent::Notice(format!(
                        "codex app-server 输出了一行认不出的内容（已跳过，之后同类不再提示）：{what}"
                    )));
                }
            }
        }
        self.after_input(now);
    }

    /// 一行超过上限被整行丢掉了（`transport::MAX_LINE_BYTES`）。每次都说：丢掉的可能正是
    /// 某个请求的回应（那个请求随后按超时处理）。
    pub fn on_dropped_line(&mut self, bytes: usize, now: Instant) {
        self.event(SessionEvent::Notice(format!(
            "codex app-server 的一行输出有 {bytes} 字节，超过上限，已丢弃；如果它是某个请求的回应，那个请求会按超时处理。"
        )));
        self.after_input(now);
    }

    /// 账本结果（[`CodexAction::Record`] 的回应）。
    pub fn on_recorded(&mut self, result: Result<(), String>, now: Instant) {
        if self.phase != Phase::Recording {
            return;
        }
        match result {
            Ok(()) => self.become_ready(now),
            Err(e) => self.fail(StartFailure::Ledger(e)),
        }
    }

    /// 定时拍：请求到期、fast 回显到期。
    pub fn on_tick(&mut self, now: Instant) {
        for (tag, _, failure) in self.rpc.expire(now) {
            self.on_result(tag, Err(failure), now);
        }
        if let Some(e) = &self.echo
            && e.deadline <= now
        {
            let ticket = e.ticket;
            self.echo = None;
            self.actions.push(CodexAction::ControlDone {
                ticket,
                outcome: ControlOutcome::NoConfirmation(
                    "thread/settings/update 已接受，但没等到 thread/settings/updated 回显，实际档位未知"
                        .into(),
                ),
            });
        }
        self.after_input(now);
    }

    /// 投一段正文：排队，空闲时开一个回合。会话已经不在了就当场报没送到。
    /// 空正文由句柄当场拒掉（`SendError::EmptyText`），到不了这里；这里再挡一道。
    pub fn send(&mut self, id: u64, text: &str, context: UntrustedContext, now: Instant) {
        if text.trim().is_empty() {
            return;
        }
        let pending = PendingTurn {
            id,
            subject: Subject::Steer,
            text: text.to_string(),
            context,
        };
        if !self.is_running() {
            let detail = format!(
                "一条指令没送出去：session 已经不在跑了。开头是「{}」。需要重起或改派。",
                preview(text)
            );
            self.delivery(&pending, DeliveryOutcome::Abandoned(detail.clone()));
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
            return;
        }
        if self.stopped_by_user {
            // 已经叫停、还在等退出（SIGTERM 宽限期）：不排队、不开回合——开了也只会被杀，
            // 还可能报出一个假的「已送达」。是人叫停的，不留 health（与 claude 核心同）。
            self.delivery(
                &pending,
                DeliveryOutcome::Abandoned("session 正在被停掉".to_string()),
            );
            return;
        }
        self.queue.push_back(pending);
        self.after_input(now);
    }

    /// 打断进行中的回合（没有就什么都不做）。
    pub fn interrupt(&mut self, now: Instant) {
        if self.phase != Phase::Ready || self.stopped_by_user {
            return;
        }
        let (Some(thread), Some(turn)) = (self.thread_id.clone(), self.tracker.active_turn())
        else {
            return;
        };
        let turn = turn.to_string();
        self.clear_human(Some(&turn));
        let params = proto::turn_interrupt_params(&thread, &turn);
        let timeout = self.config.request_timeout;
        self.request(Tag::Interrupt, "turn/interrupt", &params, now, timeout);
    }

    /// 原始按键：codex 没有终端。单独一个 Esc = 打断，其余丢弃并告知。
    pub fn raw(&mut self, bytes: &[u8], now: Instant) {
        if bytes == [0x1b] {
            self.interrupt(now);
        } else if !bytes.is_empty() {
            self.event(SessionEvent::Notice(format!(
                "codex 会话没有终端，丢弃了 {} 字节原始输入（只认单独一个 Esc = 打断）",
                bytes.len()
            )));
        }
        self.after_input(now);
    }

    /// 会话控制。结果经 [`CodexAction::ControlDone`] 交回（`ticket` 原样）。
    pub fn control_controlled(
        &mut self,
        ticket: u64,
        op: Control,
        permit: crate::driver::ManualWritePermit,
        now: Instant,
    ) {
        let before = self.actions.len();
        self.control(ticket, op, now);
        for action in &mut self.actions[before..] {
            if let CodexAction::Write(line) = action {
                let line = std::mem::take(line);
                *action = CodexAction::ControlledWrite {
                    line,
                    permit: permit.clone(),
                };
            }
        }
    }
    pub fn control(&mut self, ticket: u64, op: Control, now: Instant) {
        let reject = |why: &str| CodexAction::ControlDone {
            ticket,
            outcome: ControlOutcome::Rejected(why.to_string()),
        };
        if self.stopped_by_user {
            self.actions
                .push(reject("session 正在被停掉，不再接受控制"));
            return;
        }
        let Some(thread) = self
            .thread_id
            .clone()
            .filter(|_| self.phase == Phase::Ready)
        else {
            self.actions.push(reject("codex 会话还没就绪或已经结束"));
            return;
        };
        let timeout = self.config.request_timeout;
        match op {
            Control::Compact => {
                if self.tracker.is_busy() || self.in_flight {
                    self.actions
                        .push(reject("Codex 正在执行回合或正在压缩，空闲时再压"));
                    return;
                }
                let params = proto::compact_params(&thread);
                self.request(
                    Tag::Compact {
                        ticket: Some(ticket),
                    },
                    "thread/compact/start",
                    &params,
                    now,
                    timeout,
                );
                self.tracker.begin_compaction();
            }
            op => {
                let fast = matches!(op, Control::Fast(_));
                if fast && (self.fast_requested || self.echo.is_some()) {
                    self.actions
                        .push(reject("上一次 fast 切换还在等服务端回显"));
                    return;
                }
                let params =
                    proto::settings_update_params(&thread, &op).expect("Compact 已在上面处理");
                self.request(
                    Tag::Settings { ticket, op },
                    "thread/settings/update",
                    &params,
                    now,
                    timeout,
                );
                self.fast_requested |= fast;
            }
        }
        self.after_input(now);
    }

    /// 额度重置唤醒到点：清掉额度类 health（codex 的额度判断全来自协议，没有扫描器要重新武装）。
    pub fn clear_quota_health(&mut self, now: Instant) {
        if self.health.as_ref().is_some_and(|h| h.kind.is_quota()) {
            self.set_health(None, now);
        }
    }

    /// 我们要停它了（驱动层随后发信号）。没投出去的不报 health——是人叫停的。
    pub fn stop(&mut self, now: Instant) {
        if !self.is_running() {
            return;
        }
        self.clear_human(None);
        self.stopped_by_user = true;
        self.native_cancel("runtime stopped");
        self.abandon("session 被停掉", false, now);
        self.after_input(now);
    }

    /// 子进程退出。`stderr_tail` 是 stderr 最后一段（握手失败时带进说明）。
    pub fn on_exit(&mut self, status: ExitStatusKind, stderr_tail: &str, now: Instant) {
        if matches!(self.phase, Phase::Exited | Phase::Failed) {
            return;
        }
        self.clear_human(None);
        self.native_cancel("runtime disconnected");
        let ready = self.phase == Phase::Ready;
        for (tag, _method, _) in self.rpc.fail_all() {
            match tag {
                Tag::TurnStart { id, subject, text } => {
                    self.in_flight = false;
                    self.queue.push_front(PendingTurn {
                        id,
                        subject,
                        text,
                        context: UntrustedContext::new(),
                    });
                }
                Tag::Settings { ticket, .. }
                | Tag::Compact {
                    ticket: Some(ticket),
                } => self.actions.push(CodexAction::ControlDone {
                    ticket,
                    outcome: ControlOutcome::Rejected("codex 进程已退出".into()),
                }),
                Tag::Native { ticket, .. } | Tag::PermissionCheck { ticket, .. } => self.native_done(
                    ticket,
                    super::control::CodexOutcome::Unknown {
                        provider_accepted: false,
                        detail: "runtime disconnected".into(),
                    },
                ),
                Tag::NativeToolsSkills { ticket, .. } => self.native_done(
                    Some(ticket),
                    super::control::CodexOutcome::Unknown {
                        provider_accepted: false,
                        detail: "runtime disconnected".into(),
                    },
                ),
                _ => {}
            }
        }
        self.fast_requested = false;
        if let Some(e) = self.echo.take() {
            self.actions.push(CodexAction::ControlDone {
                ticket: e.ticket,
                outcome: ControlOutcome::Rejected("codex 进程已退出".into()),
            });
        }
        if !ready {
            let underlying = format!("{}{}", exit_words(&status), tail_words(stderr_tail));
            self.fail(StartFailure::Launch(launch_health(
                LaunchVerdict::DiedSilently,
                &underlying,
            )));
            return;
        }
        let stopped = self.stopped_by_user;
        self.abandon("进程退出", !stopped, now);
        self.tracker.reset();
        self.sync_working();
        let hit_limit = self
            .quota_at
            .is_some_and(|at| now.saturating_duration_since(at) <= self.config.hit_limit_window);
        self.phase = Phase::Exited;
        self.event(SessionEvent::Exited(ExitInfo {
            status,
            stopped_by_user: stopped,
            hit_limit,
        }));
    }

    // ---- 内部 ----

    fn request(&mut self, tag: Tag, method: &str, params: &Value, now: Instant, timeout: Duration) {
        let line = self.rpc.request(tag, method, params, now, timeout);
        self.actions.push(CodexAction::Write(line));
    }

    fn event(&mut self, e: SessionEvent) {
        if self.phase == Phase::Ready || self.phase == Phase::Exited {
            self.actions.push(CodexAction::Event(e));
        } else {
            self.early.push(e);
        }
    }

    fn fail(&mut self, failure: StartFailure) {
        if let StartFailure::Launch(h) = &failure {
            self.health = Some(h.clone());
        }
        self.phase = Phase::Failed;
        self.early.clear();
        self.actions.push(CodexAction::Failed(failure));
    }

    fn open_thread(&mut self, now: Instant) {
        let timeout = self.config.thread_timeout;
        match self.recorded.clone() {
            Some(id) if self.fallback.is_none() => {
                let params = proto::thread_resume_params(&id, &self.cwd, &self.options);
                self.request(Tag::Resume(id), "thread/resume", &params, now, timeout);
            }
            _ => {
                let params = proto::thread_start_params(&self.cwd, &self.options);
                self.request(Tag::Start, "thread/start", &params, now, timeout);
            }
        }
        self.phase = Phase::Opening;
    }

    fn opened(&mut self, thread: String, resumed: bool, result: &Value, now: Instant) {
        self.opened_profile = proto::profile_of(result);
        self.native_history_mode = result.pointer("/thread/historyMode").and_then(Value::as_str).map(str::to_owned);
        self.native_settings = result.clone();
        if let Some(settings) = result.get("threadSettings") {
            self.native_settings = settings.clone();
        }
        if let Some(sandbox) = self.native_settings.get("sandbox").cloned() {
            self.native_settings["sandboxPolicy"] = sandbox;
        }

        self.tracker = TurnTracker::new(thread.clone());
        self.thread_id = Some(thread.clone());
        self.resumed = resumed;
        self.phase = Phase::Recording;
        if self.recorded.as_deref() == Some(thread.as_str()) {
            // 续上的就是账本里那条：不用再写。
            self.become_ready(now);
        } else {
            self.actions.push(CodexAction::Record(thread));
        }
    }

    fn become_ready(&mut self, now: Instant) {
        self.phase = Phase::Ready;
        if let Some((id, said)) = self.fallback.clone() {
            // 与 claude 同序：先报「去接了旧号」，再报接不回来，最后是新号。
            self.actions.push(CodexAction::Event(SessionEvent::Spawned {
                pid: self.pid,
                agent_session_id: id.clone(),
                resumed: true,
            }));
            self.actions
                .push(CodexAction::Event(SessionEvent::ResumeRefused { id, said }));
        }
        let thread = self.thread_id.clone().unwrap_or_default();
        self.actions.push(CodexAction::Event(SessionEvent::Spawned {
            pid: self.pid,
            agent_session_id: thread,
            resumed: self.resumed,
        }));
        let profile = self.opened_profile.clone();
        if profile != Profile::default() {
            self.actions
                .push(CodexAction::Event(SessionEvent::Profile(profile)));
        }
        let early = std::mem::take(&mut self.early);
        self.actions
            .extend(early.into_iter().map(CodexAction::Event));
        self.native_opening(now);
        self.after_input(now);
    }

    fn on_result(&mut self, tag: Tag, result: Result<Value, RpcFailure>, now: Instant) {
        match tag {
            Tag::Native { ticket, op } => self.native_result(ticket, op, result, now),
            Tag::PermissionCheck{ticket,patch,permit,cursors,pages}=>self.permission_checked(ticket,patch,permit,cursors,pages,result,now),
            Tag::NativeToolsSkills { ticket, servers } => {
                self.native_tools_result(ticket, servers, result)
            }
            Tag::Initialize => match result {
                Ok(_) => {
                    self.actions
                        .push(CodexAction::Write(notification_line("initialized", None)));
                    self.open_thread(now);
                }
                Err(f) => self.handshake_failed("initialize", &f),
            },
            Tag::Resume(id) => match result {
                Ok(v) => {
                    let thread = proto::thread_id_of(&v).unwrap_or(id);
                    self.opened(thread, true, &v, now);
                }
                Err(RpcFailure::Disconnected) => {}
                Err(f) if self.options.strict_resume => self
                    .handshake_failed("thread/resume refused; no replacement thread created", &f),
                Err(f) => {
                    self.fallback = Some((id, f.to_string()));
                    self.open_thread(now);
                }
            },
            Tag::Start => match result {
                Ok(v) => match proto::thread_id_of(&v) {
                    Some(thread) => self.opened(thread, false, &v, now),
                    None => self.fail(StartFailure::Launch(launch_health(
                        LaunchVerdict::SpawnFailed,
                        "thread/start 的回包里没有 thread.id",
                    ))),
                },
                Err(RpcFailure::Disconnected) => {}
                Err(f) => self.handshake_failed("thread/start", &f),
            },
            Tag::TurnStart { id, subject, text } => {
                self.in_flight = false;
                let pending = PendingTurn {
                    id,
                    subject,
                    text,
                    context: UntrustedContext::new(),
                };
                match result {
                    Ok(v) => {
                        if let Some(turn) = v.pointer("/turn/id").and_then(Value::as_str) {
                            self.tracker.on_turn_accepted(turn);
                        }
                        if self.health.as_ref().map(|h| h.kind) == Some(undelivered_kind(subject)) {
                            self.set_health(None, now);
                        }
                        self.delivery(&pending, DeliveryOutcome::Delivered);
                    }
                    Err(f) => {
                        let detail = format!(
                            "{}没送进 codex：turn/start {f}。开头是「{}」。",
                            subject_noun(subject),
                            preview(&pending.text)
                        );
                        self.delivery(&pending, DeliveryOutcome::Undelivered(detail.clone()));
                        self.set_health(Some(Health::new(undelivered_kind(subject), detail)), now);
                    }
                }
            }
            Tag::Interrupt => {
                if let Err(f) = result {
                    self.event(SessionEvent::Notice(format!(
                        "打断没成功：turn/interrupt {f}"
                    )));
                }
            }
            Tag::Settings { ticket, op } => {
                if matches!(op, Control::Fast(_)) {
                    self.fast_requested = false;
                }
                self.on_settings_result(ticket, op, result, now);
            }
            Tag::Compact { ticket } => match result {
                Ok(_) => {
                    if let Some(ticket) = ticket {
                        self.actions.push(CodexAction::ControlDone {
                            ticket,
                            outcome: ControlOutcome::Applied("已开始压缩".into()),
                        });
                    }
                }
                Err(f) => {
                    self.tracker.compaction_failed();
                    let why = format!("Codex 压缩没启动：{f}");
                    if let Some(ticket) = ticket {
                        self.actions.push(CodexAction::ControlDone {
                            ticket,
                            outcome: ControlOutcome::Rejected(why.clone()),
                        });
                    }
                    self.event(SessionEvent::Notice(why));
                }
            },
        }
    }

    fn on_settings_result(
        &mut self,
        ticket: u64,
        op: Control,
        result: Result<Value, RpcFailure>,
        now: Instant,
    ) {
        match result {
            Ok(_) => match op {
                Control::Fast(want_fast) => {
                    self.echo = Some(EchoWait {
                        ticket,
                        want_fast,
                        deadline: now + self.config.settings_echo_timeout,
                    });
                }
                other => self.actions.push(CodexAction::ControlDone {
                    ticket,
                    outcome: ControlOutcome::Applied(format!(
                        "thread/settings/update 已接受：{other:?}"
                    )),
                }),
            },
            Err(f) => self.actions.push(CodexAction::ControlDone {
                ticket,
                outcome: ControlOutcome::Rejected(f.to_string()),
            }),
        }
    }

    fn handshake_failed(&mut self, step: &str, f: &RpcFailure) {
        let verdict = match f {
            RpcFailure::TimedOut(_) => LaunchVerdict::Stalled,
            RpcFailure::Disconnected => return,
            RpcFailure::Server(_) => LaunchVerdict::SpawnFailed,
        };
        self.fail(StartFailure::Launch(launch_health(
            verdict,
            &format!("{step}：{f}"),
        )));
    }

    fn on_notification(&mut self, method: &str, params: &Value, now: Instant) {
        // 一条 stdout 可能夹着别的 thread 的通知（0.160.1 schema 里 turn/started、turn/completed、
        // item/completed、error 都必带 threadId）：带了且不是我们的就整条不看，不喂回合跟踪也不进
        // 健康。没带 threadId（如 account/rateLimits/updated）的是账号级通知，照常处理。
        if let (Some(theirs), Some(ours)) = (
            params.get("threadId").and_then(Value::as_str),
            self.thread_id.as_deref(),
        ) && theirs != ours
        {
            return;
        }
        if let Some(h) = proto::health_from(method, params)
            && self.health.as_ref() != Some(&h)
        {
            self.set_health(Some(h), now);
        }
        if method == "thread/settings/updated" {
            self.native_settings_echo(params);
            self.on_settings_echo(params);
        }
        if method == "thread/name/updated" {
            self.native_name_echo(params);
        }
        if self.thread_id.is_none() {
            return;
        }
        if method == "serverRequest/resolved"
            && let Some(id) = params.get("requestId").and_then(RequestId::from_value)
        {
            let before = self.pending_decision().cloned();
            self.human.retain(|(request, _)| request != &id);
            self.human_changed(before);
        }
        if method == "turn/completed"
            && let Some(turn) = params.pointer("/turn/id").and_then(Value::as_str)
        {
            self.clear_human(Some(turn));
        }
        if method == "thread/tokenUsage/updated" {
            match super::usage::notification(
                params,
                self.thread_id.as_deref().expect("checked active thread"),
            ) {
                Ok(Some(snapshot)) => self.event(SessionEvent::Usage(snapshot)),
                Ok(None) => {}
                Err(reason) => self.event(SessionEvent::Notice(format!(
                    "Usage evidence not recorded: {reason}."
                ))),
            }
        }
        let s: Signals = self.tracker.on_notification(method, params);
        if let Some((turn_id, item)) = s.item {
            if method == "item/completed" {
                self.transcript.push(turn_id.clone(), item.clone());
            }
            self.event(SessionEvent::Item { turn_id, item });
        }
        if let Some((turn_id, failed)) = s.notification_outcome {
            self.event(SessionEvent::NativeTurnOutcome {
                thread_id: self.thread_id.clone().expect("checked active thread"),
                turn_id,
                failed,
                last_agent_message: s.turn_ended.clone().unwrap_or_default(),
            });
        }
        if let Some(last) = s.turn_ended {
            self.event(SessionEvent::TurnEnded {
                turn_id: params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                last_agent_message: last,
            });
        }
        if s.completed_ok
            && self.health.as_ref().is_some_and(|h| {
                h.kind.is_quota()
                    || matches!(
                        h.kind,
                        HealthKind::TurnFailed
                            | HealthKind::VersionIncompatible
                            | HealthKind::AuthRequired
                    )
            })
        {
            // 一个后来正常完成的回合，就是「旧的额度 / 失败 / 登录态标记已过时」的第一手证据
            // （能完成回合 = 认证是好的）。
            self.set_health(None, now);
        }
        if let Some(problem) = s.compaction_problem {
            self.event(SessionEvent::Notice(problem));
        }
        // 读数晚到（回合已经结束）时，排着的下一条可能已经发了 `turn/start`：那就不在它中间压，
        // 等下一个回合边界再判（读数还高，到时照样会压）。
        if s.compact
            && !self.in_flight
            && !self.tracker.is_busy()
            && let Some(thread) = self.thread_id.clone()
        {
            let params = proto::compact_params(&thread);
            let timeout = self.config.request_timeout;
            self.request(
                Tag::Compact { ticket: None },
                "thread/compact/start",
                &params,
                now,
                timeout,
            );
            self.tracker.begin_compaction();
        }
    }

    fn on_settings_echo(&mut self, params: &Value) {
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref() {
            return;
        }
        let Some(settings) = params.get("threadSettings") else {
            return;
        };
        let s = |k: &str| {
            settings
                .get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let tier = s("serviceTier");
        let fast = tier.as_deref().and_then(proto::fast_mode_for_tier);
        self.event(SessionEvent::Profile(Profile {
            model: s("model"),
            effort: s("effort"),
            fast,
        }));
        if let Some(wait) = self.echo.take() {
            let outcome = match (fast, &tier) {
                (Some(f), _) if f == wait.want_fast => {
                    ControlOutcome::Applied(format!("服务端确认档位 {}", tier.unwrap_or_default()))
                }
                (Some(_), Some(t)) => {
                    ControlOutcome::Rejected(format!("Codex 返回的实际档位是 {t}"))
                }
                _ => ControlOutcome::NoConfirmation(format!(
                    "服务端回显了认不出的档位 {:?}，实际状态未知",
                    tier
                )),
            };
            self.actions.push(CodexAction::ControlDone {
                ticket: wait.ticket,
                outcome,
            });
        }
    }

    /// 每次输入之后：能开回合就开，然后对齐干活边沿。
    fn after_input(&mut self, now: Instant) {
        self.start_next(now);
        self.sync_working();
    }

    fn start_next(&mut self, now: Instant) {
        if self.phase != Phase::Ready
            || self.initial_settings_gate
            || self.initial_settings_failed
            || self.stopped_by_user
            || !self.human.is_empty()
            || self.in_flight
            || self.tracker.is_busy()
        {
            return;
        }
        let Some(next) = self.queue.pop_front() else {
            return;
        };
        let thread = self.thread_id.clone().unwrap_or_default();
        let mut params = proto::turn_start_params(&thread, &next.text, &next.context);
        if let Some(inputs)=self.native_queued_inputs.remove(&next.id){
            inputs.append(&mut params["input"]);
            self.native_input_files.push(inputs);
        }
        let public = self.public_input_ids.remove(&next.id);
        if let Some((id, _)) = &public {
            params["clientUserMessageId"] = serde_json::json!(id);
        }
        let timeout = self.config.request_timeout;
        self.request(
            Tag::TurnStart {
                id: next.id,
                subject: next.subject,
                text: next.text,
            },
            "turn/start",
            &params,
            now,
            timeout,
        );
        if let Some((_, Some(permit))) = public {
            if let Some(CodexAction::Write(line)) = self.actions.pop() {
                self.actions
                    .push(CodexAction::ControlledWrite { line, permit });
            }
        }
        self.in_flight = true;
    }

    fn sync_working(&mut self) {
        let working = self.phase == Phase::Ready
            && (self.in_flight || self.tracker.is_busy() || self.tracker.is_working());
        if working != self.working {
            self.working = working;
            self.event(SessionEvent::Working(working));
        }
    }

    /// 换 health。低档不盖高档（[`HealthKind::rank`]，与 claude 核心同一张表）；`None` 总能清。
    fn set_health(&mut self, health: Option<Health>, now: Instant) {
        match &health {
            Some(h) => {
                if h.kind.is_quota() {
                    self.quota_at = Some(now);
                }
                if self
                    .health
                    .as_ref()
                    .is_some_and(|cur| cur.kind.rank() > h.kind.rank())
                {
                    return;
                }
            }
            None => self.quota_at = None,
        }
        if self.health == health {
            return;
        }
        self.health = health.clone();
        self.event(SessionEvent::Health(health));
    }

    fn delivery(&mut self, p: &PendingTurn, outcome: DeliveryOutcome) {
        self.event(SessionEvent::Delivery {
            id: p.id,
            subject: p.subject,
            outcome,
        });
    }

    fn abandon(&mut self, reason: &str, report: bool, now: Instant) {
        let lost: Vec<PendingTurn> = self.queue.drain(..).collect();
        for p in &lost {
            self.delivery(p, DeliveryOutcome::Abandoned(reason.to_string()));
        }
        let steers: Vec<&PendingTurn> = lost
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
    }
}

fn undelivered_kind(subject: Subject) -> HealthKind {
    match subject {
        Subject::Brief => HealthKind::BriefUndelivered,
        Subject::Steer => HealthKind::PromptUndelivered,
    }
}

fn subject_noun(subject: Subject) -> &'static str {
    match subject {
        Subject::Brief => "开场任务",
        Subject::Steer => "这条指令",
    }
}

fn exit_words(status: &ExitStatusKind) -> String {
    match status {
        ExitStatusKind::Code(c) => format!("exit {c}"),
        ExitStatusKind::Signal(s) => format!("signal {s}"),
    }
}

fn tail_words(stderr_tail: &str) -> String {
    let t = stderr_tail.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!("；stderr 最后几行：{t}")
    }
}

/// codex 拉起失败的 health（措辞是 codex 的，`health::launch_failure` 是 claude 的）。
pub fn launch_health(verdict: LaunchVerdict, underlying: &str) -> Health {
    let detail = match verdict {
        LaunchVerdict::SpawnFailed => format!(
            "Codex app-server 没能启动（{underlying}）——这个 session 从头到尾没跑起来，活等于没派出去，请改派或排查后重起。"
        ),
        LaunchVerdict::DiedSilently => format!(
            "Codex app-server 握手完成前就退出了（{underlying}）——常见于未登录 / 额度耗尽 / 版本不兼容 / 工作目录不可用，请核实后重起。"
        ),
        _ => format!(
            "Codex app-server 起来了但握手没完成（{underlying}）——半死状态，已杀掉，请改派或重起。"
        ),
    };
    Health {
        kind: HealthKind::LaunchFailed,
        detail,
        launch_verdict: Some(verdict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_notification_preserves_raw_members_only_for_the_explicit_thread() {
        let mut t = T::ready(None);
        t.take_events();
        let raw = json!({"last":{"inputTokens":42,"cachedInputTokens":3,"outputTokens":9,"reasoningOutputTokens":2,"totalTokens":51},
            "total":{"totalTokens":9007199254740993u64},"future":{"unknown":true},"modelContextWindow":null});
        t.notify(
            "thread/tokenUsage/updated",
            json!({"threadId":"other","turnId":"wrong","tokenUsage":raw}),
        );
        assert!(
            !t.take_events()
                .iter()
                .any(|e| matches!(e, SessionEvent::Usage(_)))
        );
        t.notify(
            "thread/tokenUsage/updated",
            json!({"threadId":"thr-1","tokenUsage":raw}),
        );
        let events = t.take_events();
        let sample = events
            .into_iter()
            .find_map(|e| {
                if let SessionEvent::Usage(s) = e {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(sample.turn_id, None, "must not infer a tracker turn");
        assert_eq!(sample.provider, "codex");
        assert_eq!(
            serde_json::from_str::<Value>(&sample.token_usage_json).unwrap(),
            raw
        );
        assert!(sample.token_usage_json.contains("9007199254740993"));
        assert!(!sample.token_usage_json.contains("cacheWriteInputTokens"));
        let zero = json!({"last":{"cacheWriteInputTokens":0},"future":null});
        t.notify(
            "thread/tokenUsage/updated",
            json!({"threadId":"thr-1","turnId":"provider-turn","tokenUsage":zero}),
        );
        let sample = t
            .take_events()
            .into_iter()
            .find_map(|e| {
                if let SessionEvent::Usage(s) = e {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(sample.turn_id.as_deref(), Some("provider-turn"));
        assert_eq!(
            serde_json::from_str::<Value>(&sample.token_usage_json).unwrap(),
            zero
        );
        for invalid in [
            json!({"tokenUsage":{}}),
            json!({"threadId":"thr-1","tokenUsage":null}),
            json!({"threadId":"thr-1","tokenUsage":{"huge":"x".repeat(crate::session::RUNNER_USAGE_MAX_JSON_BYTES)}}),
        ] {
            t.notify("thread/tokenUsage/updated", invalid);
            let events = t.take_events();
            assert!(!events.iter().any(|e| matches!(e, SessionEvent::Usage(_))));
            assert!(events.iter().any(
                |e| matches!(e,SessionEvent::Notice(n) if n.contains("Usage evidence not recorded"))
            ));
        }
    }

    #[test]
    fn human_approval_waits_for_the_person_instead_of_being_rejected() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1", "turn":{"id":"human-turn"}}),
        );
        let before = t.sent.len();
        t.line(
            json!({"id":"native-approval", "method":"item/commandExecution/requestApproval",
            "params":{"threadId":"thr-1", "turnId":"human-turn", "itemId":"cmd-1",
                "startedAtMs":1,"command":"git status", "cwd":"/repo", "reason":"read state"}}),
            1.0,
        );
        assert_eq!(t.core.state(), DerivedState::AwaitingDecision);
        assert_eq!(
            t.sent.len(),
            before,
            "no automatic answer while waiting for the person"
        );
    }

    fn approval(t: &mut T, id: Value, turn: &str) {
        t.line(json!({"id":id,"method":"item/fileChange/requestApproval","params":{"threadId":"thr-1","turnId":turn,"itemId":"file-1","reason":"edit source"}}),1.0);
    }

    #[test]
    fn human_reply_keeps_wire_id_is_single_use_and_cannot_cross_sessions() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"human"}}),
        );
        approval(&mut t, json!(44), "human");
        approval(&mut t, json!("second"), "human");
        let fingerprint = t.core.pending_decision().unwrap().fingerprint();
        let mut other = T::ready(None);
        other.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"human"}}),
        );
        approval(&mut other, json!(44), "human");
        assert!(matches!(
            other.core.answer_choice(&fingerprint, 0, other.at(2.0)),
            AnswerOutcome::Conflict { .. }
        ));
        assert!(matches!(
            t.core.answer_choice(&fingerprint, 0, t.at(2.0)),
            AnswerOutcome::Answered { .. }
        ));
        t.pump();
        assert!(
            t.sent
                .iter()
                .any(|v| v["id"] == 44 && v["result"]["decision"] == "accept")
        );
        assert!(matches!(
            t.core.answer_choice(&fingerprint, 0, t.at(2.1)),
            AnswerOutcome::Conflict { .. }
        ));
        let second = t.core.pending_decision().unwrap().fingerprint();
        assert!(matches!(
            t.core.answer_choice(&second, 2, t.at(2.2)),
            AnswerOutcome::Answered { .. }
        ));
        t.pump();
        assert!(
            t.sent
                .iter()
                .any(|v| v["id"] == "second" && v["result"]["decision"] == "decline")
        );
        assert_eq!(
            t.sent
                .iter()
                .filter(|v| v.get("result").is_some() && v["id"] == 44)
                .count(),
            1
        );
    }

    #[test]
    fn human_free_text_requires_original_question_ids_and_preserves_form() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"human"}}),
        );
        let params = json!({"threadId":"thr-1","turnId":"human","itemId":"question","isBlocking":true,"questions":[
            {"id":"name","header":"Name","question":"Name?","isSecret":true},
            {"id":"color","header":"Color","question":"Color?","options":[{"label":"Blue","description":"cool"}],"isOther":false}]});
        t.line(
            json!({"id":"ask-name","method":"item/tool/requestUserInput","params":params}),
            1.0,
        );
        let request = t.core.runner_request().unwrap();
        assert_eq!(request.params, params);
        assert!(matches!(
            t.core.respond_request(
                &request.fingerprint,
                json!({"answers":{"wrong":{"answers":["Blue"]}}}),
                t.at(2.0)
            ),
            AnswerOutcome::Conflict { .. }
        ));
        let response = json!({"answers":{"name":{"answers":["SYNTHETIC_SECRET"]},"color":{"answers":["Blue"]}}});
        assert!(matches!(
            t.core
                .respond_request(&request.fingerprint, response.clone(), t.at(2.1)),
            AnswerOutcome::Answered { .. }
        ));
        t.pump();
        assert!(
            t.sent
                .iter()
                .any(|v| v["id"] == "ask-name" && v["result"] == response)
        );
        assert!(!t.core.transcript_text(100).contains("SYNTHETIC_SECRET"));
        assert!(!format!("{:?}", t.events).contains("SYNTHETIC_SECRET"));
    }

    #[test]
    fn human_request_resolved_completed_and_stop_invalidate_old_answers() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"human"}}),
        );
        approval(&mut t, json!(1), "human");
        let first = t.core.pending_decision().unwrap().fingerprint();
        t.notify(
            "serverRequest/resolved",
            json!({"threadId":"foreign","requestId":1}),
        );
        assert!(t.core.pending_decision().is_some());
        t.notify(
            "serverRequest/resolved",
            json!({"threadId":"thr-1","requestId":1}),
        );
        assert!(t.core.pending_decision().is_none());
        assert!(matches!(
            t.core.answer_choice(&first, 0, t.at(2.0)),
            AnswerOutcome::Conflict { .. }
        ));
        approval(&mut t, json!(2), "human");
        t.notify(
            "turn/completed",
            json!({"threadId":"thr-1","turn":{"id":"older","status":"completed"}}),
        );
        assert!(t.core.pending_decision().is_some());
        t.notify(
            "turn/completed",
            json!({"threadId":"thr-1","turn":{"id":"human","status":"interrupted"}}),
        );
        assert!(t.core.pending_decision().is_none());
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"next"}}),
        );
        approval(&mut t, json!(3), "next");
        let last = t.core.pending_decision().unwrap().fingerprint();
        t.core.stop(t.at(4.0));
        assert!(matches!(
            t.core.answer_choice(&last, 0, t.at(4.1)),
            AnswerOutcome::Conflict { .. }
        ));
    }

    #[test]
    fn human_permission_response_cannot_expand_requested_scope() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"human"}}),
        );
        t.line(json!({"id":"permissions","method":"item/permissions/requestApproval","params":{"threadId":"thr-1","turnId":"human","permissions":{"fileSystem":{"write":["/repo/file"]}}}}),1.0);
        let fp = t.core.pending_decision().unwrap().fingerprint();
        assert!(matches!(
            t.core.respond_request(
                &fp,
                json!({"permissions":{"fileSystem":{"write":["/"]}},"scope":"session"}),
                t.at(2.0)
            ),
            AnswerOutcome::Conflict { .. }
        ));
        assert!(matches!(
            t.core.answer_choice(&fp, 0, t.at(2.1)),
            AnswerOutcome::Answered { .. }
        ));
        t.pump();
        assert!(t.sent.iter().any(|v| v["id"] == "permissions"
            && v["result"]["scope"] == "turn"
            && v["result"]["permissions"]["fileSystem"]["write"] == json!(["/repo/file"])));
    }

    #[test]
    fn native_started_tool_is_visible_without_claiming_completed_history() {
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"progress"}}),
        );
        let item = json!({"id":"cmd-live","type":"commandExecution","command":"synthetic long test","status":"inProgress"});
        t.notify(
            "item/started",
            json!({"threadId":"thr-1","turnId":"progress","item":item}),
        );
        assert!(t.events.iter().any(|e|matches!(e,SessionEvent::Item{turn_id,item:got} if turn_id=="progress" && got==&item)));
        assert!(!t.core.transcript_text(100).contains("synthetic long test"));
        assert_eq!(t.core.state(), DerivedState::Working);
        t.notify("item/completed", json!({"threadId":"thr-1","turnId":"progress","item":{"id":"cmd-live","type":"commandExecution","command":"synthetic long test","status":"completed","exitCode":0}}));
        assert!(t.core.transcript_text(100).contains("synthetic long test"));
    }

    const PID: u32 = 4242;

    /// 合成的 app-server：核心发出的每一行都解析回 JSON 记下来，回应按方法名配。
    struct T {
        core: CodexCore,
        t0: Instant,
        sent: Vec<Value>,
        events: Vec<SessionEvent>,
        other: Vec<CodexAction>,
    }

    impl T {
        fn new(recorded: Option<&str>, brief: Option<&str>) -> Self {
            Self::with(recorded, brief, CodexCoreConfig::default())
        }

        fn with(recorded: Option<&str>, brief: Option<&str>, config: CodexCoreConfig) -> Self {
            let t0 = Instant::now();
            let core = CodexCore::new(
                Opening {
                    pid: PID,
                    cwd: PathBuf::from("/repo"),
                    options: ThreadOptions {
                        developer_instructions: Some("世界观".into()),
                        ..ThreadOptions::default()
                    },
                    recorded: recorded.map(str::to_string),
                    client_version: "0.1.0".into(),
                    brief: brief.map(|b| PendingTurn {
                        id: 0,
                        subject: Subject::Brief,
                        text: b.into(),
                        context: [("crew_whiteboard".to_string(), "A: 你好".to_string())]
                            .into_iter()
                            .collect(),
                    }),
                    unix_now: 1_790_000_000,
                },
                config,
                t0,
            );
            let mut t = Self {
                core,
                t0,
                sent: vec![],
                events: vec![],
                other: vec![],
            };
            t.pump();
            t
        }

        fn at(&self, secs: f64) -> Instant {
            self.t0 + Duration::from_secs_f64(secs)
        }

        fn pump(&mut self) {
            for a in self.core.take_actions() {
                match a {
                    CodexAction::Write(l) => self.sent.push(serde_json::from_str(&l).unwrap()),
                    CodexAction::Event(e) => self.events.push(e),
                    other => self.other.push(other),
                }
            }
        }

        fn methods(&self) -> Vec<String> {
            self.sent
                .iter()
                .filter_map(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
                .collect()
        }

        fn last(&self, method: &str) -> Value {
            self.sent
                .iter()
                .rev()
                .find(|v| v["method"] == method)
                .unwrap_or_else(|| panic!("没发过 {method}：{:?}", self.methods()))
                .clone()
        }

        fn count(&self, method: &str) -> usize {
            self.sent.iter().filter(|v| v["method"] == method).count()
        }

        fn line(&mut self, v: Value, secs: f64) {
            let now = self.at(secs);
            self.core.on_line(&v.to_string(), now);
            self.pump();
        }

        fn reply(&mut self, method: &str, result: Value) {
            let id = self.last(method)["id"].clone();
            self.line(json!({ "id": id, "result": result }), 0.1);
        }

        fn reply_err(&mut self, method: &str, message: &str) {
            let id = self.last(method)["id"].clone();
            self.line(
                json!({ "id": id, "error": { "code": -32600, "message": message } }),
                0.1,
            );
        }

        fn notify(&mut self, method: &str, params: Value) {
            self.line(json!({ "method": method, "params": params }), 0.2);
        }

        /// 与 `reply` 同，但指定到达时刻（期限边界要靠它钉住）。
        fn reply_at(&mut self, method: &str, result: Value, secs: f64) {
            let id = self.last(method)["id"].clone();
            self.line(json!({ "id": id, "result": result }), secs);
        }

        fn notify_at(&mut self, method: &str, params: Value, secs: f64) {
            self.line(json!({ "method": method, "params": params }), secs);
        }

        fn tick(&mut self, secs: f64) {
            let now = self.at(secs);
            self.core.on_tick(now);
            self.pump();
        }

        fn take_events(&mut self) -> Vec<SessionEvent> {
            std::mem::take(&mut self.events)
        }

        fn take_other(&mut self) -> Vec<CodexAction> {
            std::mem::take(&mut self.other)
        }

        /// 新开：initialize → thread/start（回 `thr-1`）→ 落账成功。
        fn ready(brief: Option<&str>) -> Self {
            let mut t = Self::new(None, brief);
            t.reply("initialize", json!({ "userAgent": "codex" }));
            t.reply(
                "thread/start",
                json!({ "thread": { "id": "thr-1" }, "model": "gpt-5.5",
                        "reasoningEffort": "high", "serviceTier": "default" }),
            );
            assert_eq!(t.take_other(), vec![CodexAction::Record("thr-1".into())]);
            t.core.on_recorded(Ok(()), t.at(0.2));
            t.pump();
            t
        }

        fn turn(&mut self, id: &str, answer: &str) {
            self.notify(
                "turn/started",
                json!({ "threadId": "thr-1", "turn": { "id": id } }),
            );
            self.notify(
                "item/completed",
                json!({ "threadId": "thr-1", "turnId": id, "completedAtMs": 0,
                        "item": { "id": format!("{id}-a"), "type": "agentMessage", "text": answer } }),
            );
            self.notify(
                "turn/completed",
                json!({ "threadId": "thr-1", "turn": { "id": id, "status": "completed" } }),
            );
        }
    }

    fn deliveries(events: &[SessionEvent]) -> Vec<(u64, DeliveryOutcome)> {
        events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Delivery { id, outcome, .. } => Some((*id, outcome.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn fresh_start_records_the_thread_before_spawned_and_before_the_first_turn() {
        let mut t = T::new(None, Some("开场任务"));
        assert_eq!(t.methods(), vec!["initialize"]);
        assert_eq!(
            t.last("initialize")["params"]["capabilities"]["experimentalApi"],
            true
        );
        t.reply("initialize", json!({}));
        assert_eq!(
            t.methods(),
            vec!["initialize", "initialized", "thread/start"]
        );
        assert!(t.sent[1].get("id").is_none(), "initialized 是通知");
        assert!(
            t.sent[1].get("params").is_none(),
            "initialized 按 schema 不带 params"
        );
        let start = t.last("thread/start");
        assert_eq!(start["params"]["developerInstructions"], "世界观");
        assert_eq!(start["params"]["approvalsReviewer"], "auto_review");
        t.reply(
            "thread/start",
            json!({ "thread": { "id": "thr-1" }, "model": "gpt-5.5", "serviceTier": "fast" }),
        );
        assert_eq!(t.take_other(), vec![CodexAction::Record("thr-1".into())]);
        assert!(t.events.is_empty(), "落账之前不发任何事件");
        assert_eq!(t.count("turn/start"), 0, "落账之前不开回合");
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert_eq!(
            ev[0],
            SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-1".into(),
                resumed: false,
            }
        );
        assert_eq!(
            ev[1],
            SessionEvent::Profile(Profile {
                model: Some("gpt-5.5".into()),
                effort: None,
                fast: Some(true),
            })
        );
        assert_eq!(ev[2], SessionEvent::Working(true));
        let turn = t.last("turn/start");
        assert_eq!(turn["params"]["threadId"], "thr-1");
        assert_eq!(turn["params"]["input"][0]["text"], "开场任务");
        assert_eq!(
            turn["params"]["additionalContext"]["crew_whiteboard"],
            json!({ "value": "A: 你好", "kind": "untrusted" })
        );
        t.reply(
            "turn/start",
            json!({ "turn": { "id": "turn-1", "status": "inProgress" } }),
        );
        assert_eq!(
            deliveries(&t.take_events()),
            vec![(0, DeliveryOutcome::Delivered)]
        );
        assert!(t.core.is_working());
    }

    #[test]
    fn recorded_thread_is_resumed_without_turns_and_not_rerecorded() {
        let mut t = T::new(Some("thr-old"), None);
        t.reply("initialize", json!({}));
        let resume = t.last("thread/resume");
        assert_eq!(resume["params"]["threadId"], "thr-old");
        assert_eq!(resume["params"]["excludeTurns"], true);
        assert_eq!(resume["params"]["developerInstructions"], "世界观");
        t.reply("thread/resume", json!({ "thread": { "id": "thr-old" } }));
        assert!(t.take_other().is_empty(), "续上的就是账本里那条，不再写账");
        assert_eq!(
            t.take_events(),
            vec![SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-old".into(),
                resumed: true,
            }]
        );
        assert_eq!(t.count("thread/start"), 0);
    }

    /// 续跑被拒：新 thread 先落账；事件与 claude 同序（旧号的 Spawned → ResumeRefused → 新号的
    /// Spawned），每条 ResumeRefused 后面都跟一条 Spawned。
    #[test]
    fn refused_resume_falls_back_to_a_fresh_recorded_thread_in_the_same_order_as_claude() {
        let mut t = T::new(Some("thr-gone"), Some("接着干"));
        t.reply("initialize", json!({}));
        t.reply_err("thread/resume", "no rollout found for thread id thr-gone");
        assert_eq!(t.count("thread/start"), 1);
        t.reply("thread/start", json!({ "thread": { "id": "thr-new" } }));
        assert_eq!(t.take_other(), vec![CodexAction::Record("thr-new".into())]);
        assert!(t.events.is_empty(), "落账之前不发任何事件");
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert_eq!(
            ev[0],
            SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-gone".into(),
                resumed: true,
            }
        );
        let SessionEvent::ResumeRefused { id, said } = &ev[1] else {
            panic!("第二条应是 ResumeRefused：{ev:?}");
        };
        assert_eq!(id, "thr-gone");
        assert!(said.contains("no rollout found"), "{said}");
        assert_eq!(
            ev[2],
            SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-new".into(),
                resumed: false,
            }
        );
        assert_eq!(t.core.thread_id(), Some("thr-new"));
        assert_eq!(t.last("turn/start")["params"]["threadId"], "thr-new");
    }

    #[test]
    fn ledger_failure_fails_the_start_and_no_turn_is_opened() {
        let mut t = T::new(None, Some("开场任务"));
        t.reply("initialize", json!({}));
        t.reply("thread/start", json!({ "thread": { "id": "thr-1" } }));
        t.take_other();
        t.core.on_recorded(Err("库坏了".into()), t.at(0.3));
        t.pump();
        assert_eq!(
            t.take_other(),
            vec![CodexAction::Failed(StartFailure::Ledger("库坏了".into()))]
        );
        assert!(t.events.is_empty());
        assert_eq!(t.count("turn/start"), 0);
        assert!(!t.core.is_running());
    }

    #[test]
    fn a_silent_initialize_is_a_stalled_launch_at_its_deadline() {
        let mut t = T::new(None, None);
        t.core.on_tick(t.at(29.9));
        t.pump();
        assert!(t.other.is_empty());
        assert_eq!(t.core.next_wakeup(), Some(t.at(30.0)));
        t.core.on_tick(t.at(30.0));
        t.pump();
        let other = t.take_other();
        let [CodexAction::Failed(StartFailure::Launch(h))] = other.as_slice() else {
            panic!("应当启动失败");
        };
        assert_eq!(h.kind, HealthKind::LaunchFailed);
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::Stalled));
        assert!(h.detail.contains("initialize"), "{}", h.detail);
    }

    #[test]
    fn dying_during_the_handshake_carries_the_stderr_tail() {
        let mut t = T::new(None, None);
        t.reply("initialize", json!({}));
        t.core
            .on_exit(ExitStatusKind::Code(1), "Error: not logged in", t.at(1.0));
        t.pump();
        let other = t.take_other();
        let [CodexAction::Failed(StartFailure::Launch(h))] = other.as_slice() else {
            panic!("应当启动失败");
        };
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(
            h.detail.contains("exit 1") && h.detail.contains("not logged in"),
            "{}",
            h.detail
        );
        assert!(t.events.is_empty(), "没就绪过就不发 Exited");
    }

    #[test]
    fn server_requests_during_the_handshake_are_answered_now_and_reported_after_spawned() {
        let mut t = T::new(None, None);
        t.reply("initialize", json!({}));
        t.line(
            json!({ "id": "srv-a", "method": "account/chatgptAuthTokens/refresh",
                    "params": { "reason": "unauthorized" } }),
            0.2,
        );
        let reply = t.sent.last().unwrap().clone();
        assert_eq!(reply["id"], "srv-a");
        assert_eq!(reply["error"]["code"], -32601);
        assert!(t.events.is_empty());
        t.reply("thread/start", json!({ "thread": { "id": "thr-1" } }));
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert!(matches!(ev[0], SessionEvent::Spawned { .. }), "{ev:?}");
        assert!(
            ev.iter().any(
                |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired)
            ),
            "{ev:?}"
        );
        assert_eq!(t.core.state(), DerivedState::Error);
    }

    #[test]
    fn every_server_request_is_answered_and_each_kind_is_announced_once() {
        let mut t = T::ready(None);
        t.take_events();
        let before = t.sent.len();
        for (i, (id, method, params)) in [
            (json!(7), "item/commandExecution/requestApproval", json!({})),
            (
                json!("e-1"),
                "mcpServer/elicitation/request",
                json!({ "serverName": "crew",
                        "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            ),
            (
                json!("e-2"),
                "mcpServer/elicitation/request",
                json!({ "mode": "form", "message": "你叫什么？" }),
            ),
            (
                json!(8),
                "item/tool/requestUserInput",
                json!({ "prompt": "选一个" }),
            ),
            (
                json!(9),
                "item/tool/requestUserInput",
                json!({ "prompt": "再选一个" }),
            ),
            (
                json!(10),
                "currentTime/read",
                json!({ "threadId": "thr-1" }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            t.line(
                json!({ "id": id, "method": method, "params": params }),
                1.0 + i as f64,
            );
        }
        let replies = &t.sent[before..];
        assert_eq!(replies.len(), 6, "每一条都回");
        assert_eq!(replies[0]["error"]["code"], -32000);
        assert_eq!(
            replies[1],
            json!({ "id": "e-1", "result": { "action": "accept" } })
        );
        assert_eq!(
            replies[2],
            json!({ "id": "e-2", "result": { "action": "decline" } })
        );
        assert_eq!(replies[3]["error"]["code"], -32601);
        assert_eq!(replies[4]["id"], 9);
        // 构造时 unix 1_790_000_000，第 6 条在 6s 后到。
        assert_eq!(
            replies[5],
            json!({ "id": 10, "result": { "currentTimeAt": 1_790_000_006i64 } })
        );
        let notices: Vec<&SessionEvent> = t
            .events
            .iter()
            .filter(|e| matches!(e, SessionEvent::Notice(_)))
            .collect();
        assert_eq!(
            notices.len(),
            2,
            "表单一次、requestUserInput 一次（同种只喊一次）：{notices:?}"
        );
    }

    #[test]
    fn a_tool_approval_from_a_foreign_mcp_server_is_declined_and_announced_once() {
        let mut t = T::ready(None);
        t.take_events();
        let before = t.sent.len();
        for (i, id) in ["f-1", "f-2"].into_iter().enumerate() {
            t.line(
                json!({ "id": id, "method": "mcpServer/elicitation/request",
                        "params": { "serverName": "github", "message": "Allow create_issue?",
                                    "_meta": { "codex_approval_kind": "mcp_tool_call" } } }),
                1.0 + i as f64,
            );
        }
        let replies = &t.sent[before..];
        assert_eq!(
            replies[0],
            json!({ "id": "f-1", "result": { "action": "decline" } })
        );
        assert_eq!(
            replies[1],
            json!({ "id": "f-2", "result": { "action": "decline" } })
        );
        let notices: Vec<&SessionEvent> = t
            .events
            .iter()
            .filter(|e| matches!(e, SessionEvent::Notice(_)))
            .collect();
        assert_eq!(notices.len(), 1, "同种只喊一次：{notices:?}");
        // 我们自己的 server 照旧放行。
        t.line(
            json!({ "id": "c-1", "method": "mcpServer/elicitation/request",
                    "params": { "serverName": "crew",
                                "_meta": { "codex_approval_kind": "mcp_tool_call" } } }),
            3.0,
        );
        assert_eq!(
            t.sent.last().unwrap(),
            &json!({ "id": "c-1", "result": { "action": "accept" } })
        );
    }

    #[test]
    fn a_retryable_unauthorized_error_never_becomes_health() {
        let mut t = T::ready(None);
        t.take_events();
        t.notify(
            "error",
            json!({ "threadId": "thr-1", "turnId": "t1", "willRetry": true,
                    "error": { "message": "token refresh blip", "codexErrorInfo": "unauthorized" } }),
        );
        assert_eq!(t.core.health(), None);
        assert_ne!(t.core.state(), DerivedState::Error);
        assert!(
            !t.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Health(_))),
            "{:?}",
            t.events
        );
    }

    #[test]
    fn auth_required_from_a_final_error_is_cleared_by_a_later_completed_turn() {
        let mut t = T::ready(None);
        t.notify(
            "error",
            json!({ "threadId": "thr-1", "turnId": "t0", "willRetry": false,
                    "error": { "message": "401", "codexErrorInfo": "unauthorized" } }),
        );
        assert_eq!(t.core.health().unwrap().kind, HealthKind::AuthRequired);
        assert_eq!(t.core.state(), DerivedState::Error);
        t.turn("t1", "登录是好的");
        assert_eq!(t.core.health(), None, "一个正常完成的回合证明认证没问题");
        assert_ne!(t.core.state(), DerivedState::Error);
        assert!(
            t.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Health(None))),
            "{:?}",
            t.events
        );
    }

    #[test]
    fn another_threads_notifications_do_not_touch_our_turn_or_health() {
        let mut t = T::ready(Some("干活"));
        t.reply("turn/start", json!({ "turn": { "id": "mine" } }));
        t.notify(
            "turn/started",
            json!({ "threadId": "thr-1", "turn": { "id": "mine" } }),
        );
        t.take_events();
        // 别的 thread 的一整个回合插进我们的回合中间（同一条 stdout 上的多路复用）。
        t.notify(
            "turn/started",
            json!({ "threadId": "other", "turn": { "id": "theirs" } }),
        );
        t.notify(
            "item/completed",
            json!({ "threadId": "other", "turnId": "theirs", "completedAtMs": 0,
                    "item": { "id": "x", "type": "agentMessage", "text": "别人的话" } }),
        );
        t.notify(
            "error",
            json!({ "threadId": "other", "turnId": "theirs", "willRetry": false,
                    "error": { "message": "他们的错" } }),
        );
        t.notify(
            "turn/completed",
            json!({ "threadId": "other", "turn": { "id": "theirs", "status": "failed" } }),
        );
        assert!(
            t.events.is_empty(),
            "别的 thread 不产生任何事件：{:?}",
            t.events
        );
        assert!(t.core.is_working(), "我们的回合还在跑");
        assert_eq!(t.core.health(), None);
        // 我们自己的回合照常结束，且只报我们的话。
        t.notify(
            "item/completed",
            json!({ "threadId": "thr-1", "turnId": "mine", "completedAtMs": 0,
                    "item": { "id": "m", "type": "agentMessage", "text": "我的话" } }),
        );
        t.notify(
            "turn/completed",
            json!({ "threadId": "thr-1", "turn": { "id": "mine", "status": "completed" } }),
        );
        let ended: Vec<_> = t
            .events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::TurnEnded {
                    last_agent_message, ..
                } => Some(last_agent_message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ended, vec!["我的话".to_string()]);
    }

    #[test]
    fn sends_queue_behind_a_running_turn_and_each_gets_its_own_turn() {
        let mut t = T::ready(Some("开场"));
        t.reply("turn/start", json!({ "turn": { "id": "t1" } }));
        t.core.send(1, "第二条", UntrustedContext::new(), t.at(1.0));
        t.core.send(2, "第三条", UntrustedContext::new(), t.at(1.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 1, "回合在跑：排队");
        t.take_events();
        t.turn("t1", "开场做完了");
        let ev = t.take_events();
        assert!(ev.contains(&SessionEvent::TurnEnded {
            turn_id: "t1".into(),
            last_agent_message: "开场做完了".into()
        }));
        assert!(
            !ev.contains(&SessionEvent::Working(false)),
            "后面还有排队的：不报空闲边沿"
        );
        assert_eq!(t.count("turn/start"), 2);
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "第二条");
        assert!(
            t.last("turn/start")["params"]
                .get("additionalContext")
                .is_none()
        );
        t.reply("turn/start", json!({ "turn": { "id": "t2" } }));
        t.turn("t2", "第二条做完了");
        t.reply("turn/start", json!({ "turn": { "id": "t3" } }));
        t.turn("t3", "全做完了");
        let ev = t.take_events();
        assert_eq!(
            deliveries(&ev),
            vec![
                (1, DeliveryOutcome::Delivered),
                (2, DeliveryOutcome::Delivered)
            ]
        );
        assert_eq!(ev.last(), Some(&SessionEvent::Working(false)), "{ev:?}");
        assert!(!t.core.is_working());
        assert_eq!(t.core.state(), DerivedState::Idle);
        assert!(t.core.transcript_text(10).contains("全做完了"));
    }

    #[test]
    fn a_rejected_turn_start_is_undelivered_and_the_queue_moves_on() {
        let mut t = T::ready(Some("开场"));
        t.core.send(1, "第二条", UntrustedContext::new(), t.at(1.0));
        t.pump();
        t.reply_err("turn/start", "thread busy");
        let ev = t.take_events();
        let d = deliveries(&ev);
        assert!(
            matches!(&d[0], (0, DeliveryOutcome::Undelivered(m)) if m.contains("thread busy")),
            "{d:?}"
        );
        assert!(ev.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::BriefUndelivered)
        ));
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "第二条");
        t.reply("turn/start", json!({ "turn": { "id": "t2" } }));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::BriefUndelivered),
            "别的指令送达，不清掉「开场任务没送到」"
        );
    }

    #[test]
    fn a_turn_start_that_never_answers_times_out_and_the_session_stays_usable() {
        let mut t = T::ready(Some("开场"));
        t.take_events();
        t.core.on_tick(t.at(30.3));
        t.pump();
        let ev = t.take_events();
        assert!(
            matches!(&deliveries(&ev)[0], (0, DeliveryOutcome::Undelivered(m)) if m.contains("没有回应")),
            "{ev:?}"
        );
        assert!(ev.contains(&SessionEvent::Working(false)));
        // 迟到的回应被丢弃；之后的正文照常开回合。
        t.reply("turn/start", json!({ "turn": { "id": "late" } }));
        t.core.send(1, "再来", UntrustedContext::new(), t.at(31.0));
        t.pump();
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "再来");
    }

    #[test]
    fn interrupt_targets_the_active_turn_and_a_lone_escape_means_interrupt() {
        let mut t = T::ready(None);
        t.core.interrupt(t.at(1.0));
        t.pump();
        assert_eq!(t.count("turn/interrupt"), 0, "没有进行中的回合");
        t.notify(
            "turn/started",
            json!({ "threadId": "thr-1", "turn": { "id": "t9" } }),
        );
        t.core.raw(&[0x1b], t.at(1.5));
        t.pump();
        assert_eq!(
            t.last("turn/interrupt")["params"],
            json!({ "threadId": "thr-1", "turnId": "t9" })
        );
        t.take_events();
        t.core.raw(b"\x1b[A", t.at(1.6));
        t.pump();
        assert!(matches!(&t.take_events()[..], [SessionEvent::Notice(n)] if n.contains("3 字节")));
        assert_eq!(t.count("turn/interrupt"), 1);
    }

    #[test]
    fn usage_limit_health_is_cleared_by_a_later_completed_turn_and_counts_as_hit_limit() {
        let mut t = T::ready(None);
        t.notify(
            "account/rateLimits/updated",
            json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
        );
        assert_eq!(t.core.state(), DerivedState::RateLimited);
        t.turn("t1", "又能干了");
        assert_eq!(t.core.health(), None);
        t.notify(
            "turn/completed",
            json!({ "threadId": "thr-1", "turn": { "id": "t2", "status": "failed",
                "error": { "message": "Weekly limit reached", "codexErrorInfo": "usageLimitExceeded" } } }),
        );
        assert_eq!(t.core.health().unwrap().kind, HealthKind::UsageLimit);
        t.core.on_exit(ExitStatusKind::Code(0), "", t.at(100.0));
        t.pump();
        assert_eq!(
            t.events.last(),
            Some(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: false,
                hit_limit: true,
            }))
        );
    }

    #[test]
    fn high_context_triggers_one_compaction_and_sends_wait_for_it() {
        let mut t = T::ready(Some("干活"));
        t.reply("turn/start", json!({ "turn": { "id": "work" } }));
        t.turn("work", "干完了");
        t.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": "thr-1", "turnId": "work", "tokenUsage": {
                "last": { "totalTokens": 190_000 }, "total": { "totalTokens": 500_000 },
                "modelContextWindow": 258_400 } }),
        );
        assert_eq!(t.count("thread/compact/start"), 1);
        assert_eq!(
            t.last("thread/compact/start")["params"],
            json!({ "threadId": "thr-1" })
        );
        assert!(t.core.is_working());
        t.core
            .send(1, "压缩时来的", UntrustedContext::new(), t.at(2.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 1, "压缩中排队");
        t.reply("thread/compact/start", json!({}));
        t.notify(
            "turn/started",
            json!({ "threadId": "thr-1", "turn": { "id": "c" } }),
        );
        t.notify(
            "item/completed",
            json!({ "threadId": "thr-1", "turnId": "c", "item": { "id": "ci", "type": "contextCompaction" } }),
        );
        t.take_events();
        t.notify(
            "turn/completed",
            json!({ "threadId": "thr-1", "turn": { "id": "c", "status": "completed" } }),
        );
        let ev = t.take_events();
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::TurnEnded { .. })),
            "压缩完成不是第二次回答：{ev:?}"
        );
        assert_eq!(t.count("turn/start"), 2);
        assert_eq!(
            t.last("turn/start")["params"]["input"][0]["text"],
            "压缩时来的"
        );
        assert_eq!(t.count("thread/compact/start"), 1);
    }

    /// 读数比 `turn/completed` 晚到，而这之间来的一条已经开了回合（`turn/start` 在路上）：
    /// 不在回合中间压缩，等下一个回合边界。
    #[test]
    fn a_late_reading_does_not_compact_under_a_turn_that_is_starting() {
        let mut t = T::ready(Some("干活"));
        t.reply("turn/start", json!({ "turn": { "id": "work" } }));
        t.turn("work", "干完了");
        t.core
            .send(1, "紧接着来的", UntrustedContext::new(), t.at(2.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 2, "前提：空闲，马上开回合");
        t.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": "thr-1", "turnId": "work", "tokenUsage": {
                "last": { "totalTokens": 190_000 }, "total": { "totalTokens": 500_000 },
                "modelContextWindow": 258_400 } }),
        );
        assert_eq!(t.count("thread/compact/start"), 0, "回合在路上，不压");
        // 这一回合照常跑完，排队与回合结束不受影响。
        t.reply("turn/start", json!({ "turn": { "id": "t2" } }));
        t.turn("t2", "也干完了");
        assert!(!t.core.is_working());
    }

    #[test]
    fn a_compaction_that_cannot_start_releases_the_queue_and_says_why() {
        let mut t = T::ready(None);
        t.core.control(5, Control::Compact, t.at(1.0));
        t.pump();
        t.core.send(1, "排着", UntrustedContext::new(), t.at(1.1));
        t.pump();
        assert_eq!(t.count("turn/start"), 0);
        t.reply_err("thread/compact/start", "not supported");
        let other = t.take_other();
        assert!(
            matches!(&other[..], [CodexAction::ControlDone { ticket: 5, outcome: ControlOutcome::Rejected(m) }] if m.contains("not supported")),
            "{other:?}"
        );
        assert!(
            t.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("压缩没启动")))
        );
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "排着");
    }

    #[test]
    fn manual_compaction_is_refused_while_busy_and_accepted_when_idle() {
        let mut t = T::ready(Some("干活"));
        t.core.control(1, Control::Compact, t.at(1.0));
        t.pump();
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::Rejected(_)
            }]
        ));
        t.reply("turn/start", json!({ "turn": { "id": "t1" } }));
        t.turn("t1", "好了");
        t.core.control(2, Control::Compact, t.at(2.0));
        t.pump();
        t.reply("thread/compact/start", json!({}));
        assert_eq!(
            t.take_other(),
            vec![CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::Applied("已开始压缩".into()),
            }]
        );
    }

    #[test]
    fn model_change_is_applied_on_ack_and_fast_waits_for_the_echo() {
        let mut t = T::ready(None);
        t.core.control(1, Control::Model("o5".into()), t.at(1.0));
        t.pump();
        assert_eq!(
            t.last("thread/settings/update")["params"],
            json!({ "threadId": "thr-1", "model": "o5" })
        );
        t.reply("thread/settings/update", json!({}));
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::Applied(_)
            }]
        ));

        t.core.control(2, Control::Fast(true), t.at(2.0));
        t.pump();
        // 回包之前到的回显不算（那是旧状态）。
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "default" } }),
        );
        t.core.control(3, Control::Fast(false), t.at(2.1));
        t.pump();
        assert!(
            matches!(
                &t.take_other()[..],
                [CodexAction::ControlDone {
                    ticket: 3,
                    outcome: ControlOutcome::Rejected(_)
                }]
            ),
            "同时只等一次回显"
        );
        t.reply("thread/settings/update", json!({}));
        assert!(t.other.is_empty(), "空 {{}} 回包只是收到，不是生效");
        // 别的 thread 的回显不算。
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-x", "threadSettings": { "serviceTier": "fast" } }),
        );
        assert!(t.other.is_empty());
        t.take_events();
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "fast", "model": "o5" } }),
        );
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::Applied(_)
            }]
        ));
        assert!(t.events.contains(&SessionEvent::Profile(Profile {
            model: Some("o5".into()),
            effort: None,
            fast: Some(true),
        })));
    }

    #[test]
    fn fast_switch_without_an_echo_or_with_an_unknown_tier_is_not_confirmed() {
        let mut t = T::ready(None);
        t.core.control(1, Control::Fast(true), t.at(1.0));
        t.pump();
        t.reply("thread/settings/update", json!({}));
        t.core.on_tick(t.at(9.2));
        t.pump();
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::NoConfirmation(_)
            }]
        ));
        t.core.control(2, Control::Fast(true), t.at(10.0));
        t.pump();
        t.reply("thread/settings/update", json!({}));
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "turbo-2027" } }),
        );
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::NoConfirmation(_)
            }]
        ));
    }

    #[test]
    fn exit_abandons_what_was_queued_and_reports_it_unless_we_stopped_it() {
        let mut t = T::ready(Some("开场"));
        t.core.send(1, "排着的", UntrustedContext::new(), t.at(1.0));
        t.pump();
        t.take_events();
        t.core.on_exit(ExitStatusKind::Signal(9), "", t.at(2.0));
        t.pump();
        let ev = t.take_events();
        let d = deliveries(&ev);
        assert_eq!(d.len(), 2, "在途的开场 + 排队的一条：{d:?}");
        assert!(
            d.iter()
                .all(|(_, o)| matches!(o, DeliveryOutcome::Abandoned(_)))
        );
        assert!(ev.iter().any(|e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::PromptUndelivered)));
        assert!(ev.contains(&SessionEvent::Working(false)));
        assert!(matches!(
            ev.last(),
            Some(SessionEvent::Exited(ExitInfo {
                stopped_by_user: false,
                ..
            }))
        ));
        // 退出之后再投：当场报没送到。
        t.core.send(2, "晚了", UntrustedContext::new(), t.at(3.0));
        t.pump();
        assert!(matches!(
            &deliveries(&t.take_events())[..],
            [(2, DeliveryOutcome::Abandoned(_))]
        ));

        let mut s = T::ready(None);
        s.core.send(1, "排着的", UntrustedContext::new(), s.at(1.0));
        s.core.stop(s.at(1.1));
        s.core.on_exit(ExitStatusKind::Signal(15), "", s.at(1.2));
        s.pump();
        assert!(
            !s.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Health(Some(_)))),
            "人叫停的不报 health"
        );
        assert!(matches!(
            s.events.last(),
            Some(SessionEvent::Exited(ExitInfo {
                stopped_by_user: true,
                ..
            }))
        ));
    }

    #[test]
    fn a_lower_ranked_health_does_not_hide_a_quota_problem() {
        let mut t = T::ready(None);
        t.notify(
            "account/rateLimits/updated",
            json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
        );
        t.notify(
            "error",
            json!({ "threadId": "thr-1", "turnId": "x", "willRetry": false,
                    "error": { "message": "stream closed" } }),
        );
        assert_eq!(t.core.health().unwrap().kind, HealthKind::UsageLimit);
        assert_eq!(t.core.state(), DerivedState::RateLimited);
    }

    #[test]
    fn an_invalid_recorded_id_is_not_resumed() {
        let mut t = T::new(Some("../../etc"), None);
        t.reply("initialize", json!({}));
        assert_eq!(t.count("thread/resume"), 0);
        assert_eq!(t.count("thread/start"), 1);
    }

    #[test]
    fn every_dropped_oversized_line_is_reported() {
        let mut t = T::ready(None);
        t.take_events();
        t.core.on_dropped_line(70_000_000, t.at(1.0));
        t.core.on_dropped_line(80_000_000, t.at(2.0));
        t.pump();
        let ev = t.take_events();
        assert_eq!(ev.len(), 2, "每次都说：丢掉的可能是某个回应：{ev:?}");
        assert!(matches!(&ev[1], SessionEvent::Notice(n) if n.contains("80000000")));
    }

    #[test]
    fn a_malformed_line_is_announced_once_and_skipped() {
        let mut t = T::ready(None);
        t.take_events();
        let now = t.at(1.0);
        t.core.on_line("this is not json", now);
        t.core.on_line("{\"also\": \"unclassifiable\"}", now);
        t.core.on_line("", now);
        t.pump();
        let ev = t.take_events();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert!(matches!(&ev[0], SessionEvent::Notice(n) if n.contains("this is not json")));
    }

    #[test]
    fn completed_items_become_events_and_transcript_text() {
        let mut t = T::ready(None);
        t.take_events();
        t.notify(
            "item/completed",
            json!({ "threadId": "thr-1", "turnId": "t1", "completedAtMs": 1,
                    "item": { "id": "c1", "type": "commandExecution", "command": "ls", "exitCode": 0 } }),
        );
        let ev = t.take_events();
        assert!(
            !ev.contains(&SessionEvent::Working(true)),
            "没有进行中的回合时，带回合 id 的条目不算在干活（迟到的旧条目）：{ev:?}"
        );
        assert!(ev.iter().any(|e| matches!(e, SessionEvent::Item { turn_id, item } if turn_id == "t1" && item["command"] == "ls")));
        assert_eq!(t.core.transcript_text(5), "$ ls（exit 0）");
    }

    #[test]
    fn send_after_stop_is_abandoned_at_once_and_opens_no_turn() {
        let mut t = T::ready(None);
        t.take_events();
        t.core.stop(t.at(1.0));
        t.core
            .send(1, "叫停之后来的", UntrustedContext::new(), t.at(1.1));
        t.pump();
        assert_eq!(t.count("turn/start"), 0, "SIGTERM 宽限期里不能再开回合");
        let ev = t.take_events();
        assert!(
            matches!(&deliveries(&ev)[..], [(1, DeliveryOutcome::Abandoned(m))] if m.contains("正在被停掉")),
            "{ev:?}"
        );
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::Health(Some(_)))),
            "人叫停的不留 health：{ev:?}"
        );
        assert_eq!(t.core.health(), None);
    }

    #[test]
    fn control_after_stop_is_rejected_and_writes_nothing() {
        let mut t = T::ready(None);
        let before = t.sent.len();
        t.core.stop(t.at(1.0));
        t.core.control(1, Control::Model("o5".into()), t.at(1.1));
        t.core.control(2, Control::Fast(true), t.at(1.2));
        t.core.control(3, Control::Compact, t.at(1.3));
        t.pump();
        assert_eq!(t.sent.len(), before, "叫停之后不写任何请求");
        let outs: Vec<_> = t
            .take_other()
            .into_iter()
            .map(|a| match a {
                CodexAction::ControlDone { ticket, outcome } => (ticket, outcome),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(outs.len(), 3);
        for (i, (ticket, outcome)) in outs.iter().enumerate() {
            assert_eq!(*ticket, i as u64 + 1);
            assert!(
                matches!(outcome, ControlOutcome::Rejected(m) if m.contains("停")),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn a_whitespace_only_brief_opens_no_turn() {
        let mut t = T::ready(Some("  \n\t "));
        assert_eq!(t.count("turn/start"), 0);
        assert!(!t.core.is_working());
        t.take_events();
        t.core
            .send(1, "正常来的", UntrustedContext::new(), t.at(1.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 1);
    }

    #[test]
    fn a_resume_that_times_out_falls_back_to_a_fresh_thread() {
        let mut t = T::new(Some("thr-old"), None);
        t.reply_at("initialize", json!({}), 0.0);
        assert_eq!(t.core.next_wakeup(), Some(t.at(60.0)));
        t.tick(59.9);
        assert_eq!(t.count("thread/start"), 0, "期限没到");
        t.tick(60.0);
        assert_eq!(t.count("thread/start"), 1, "超时也退回新 thread");
        t.reply_at(
            "thread/start",
            json!({ "thread": { "id": "thr-new" } }),
            61.0,
        );
        assert_eq!(t.take_other(), vec![CodexAction::Record("thr-new".into())]);
        t.core.on_recorded(Ok(()), t.at(61.1));
        t.pump();
        let ev = t.take_events();
        assert!(
            matches!(&ev[1], SessionEvent::ResumeRefused { id, said } if id == "thr-old" && said.contains("没有回应")),
            "{ev:?}"
        );
        assert!(
            matches!(&ev[2], SessionEvent::Spawned { agent_session_id, resumed: false, .. } if agent_session_id == "thr-new")
        );
    }

    #[test]
    fn a_thread_start_that_times_out_is_a_stalled_launch() {
        let mut t = T::new(None, None);
        t.reply_at("initialize", json!({}), 0.0);
        t.tick(59.9);
        assert!(t.other.is_empty());
        t.tick(60.0);
        let other = t.take_other();
        let [CodexAction::Failed(StartFailure::Launch(h))] = other.as_slice() else {
            panic!("应当启动失败：{other:?}");
        };
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::Stalled));
        assert!(h.detail.contains("thread/start"), "{}", h.detail);
        assert!(!t.core.is_running());
    }

    #[test]
    fn a_settings_update_that_times_out_is_rejected_and_fast_can_be_retried() {
        let mut t = T::ready(None);
        t.core.control(1, Control::Fast(true), t.at(1.0));
        t.pump();
        t.tick(30.9);
        assert!(t.other.is_empty(), "期限没到");
        t.tick(31.0);
        assert!(
            matches!(
                &t.take_other()[..],
                [CodexAction::ControlDone {
                    ticket: 1,
                    outcome: ControlOutcome::Rejected(m)
                }] if m.contains("没有回应")
            ),
            "超时 = 被拒"
        );
        // 迟到的回包丢弃；fast 的「同时只等一次」标记已清，可以再试。
        t.reply_at("thread/settings/update", json!({}), 31.5);
        assert!(t.other.is_empty());
        t.core.control(2, Control::Fast(true), t.at(32.0));
        t.pump();
        assert_eq!(t.count("thread/settings/update"), 2);
        assert!(t.other.is_empty(), "第二次不该被当场拒掉");
    }

    #[test]
    fn a_compaction_that_times_out_is_rejected_and_releases_the_queue() {
        let mut t = T::ready(Some("干活"));
        t.reply("turn/start", json!({ "turn": { "id": "t1" } }));
        t.turn("t1", "好了");
        t.core.control(2, Control::Compact, t.at(2.0));
        t.core.send(1, "排着", UntrustedContext::new(), t.at(2.1));
        t.pump();
        assert_eq!(t.count("turn/start"), 1, "压缩中排队");
        t.tick(31.9);
        assert_eq!(t.count("turn/start"), 1, "期限没到");
        t.take_other();
        t.take_events();
        t.tick(32.0);
        let other = t.take_other();
        assert!(
            matches!(&other[..], [CodexAction::ControlDone { ticket: 2, outcome: ControlOutcome::Rejected(m) }] if m.contains("没有回应")),
            "{other:?}"
        );
        assert!(
            t.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("压缩没启动")))
        );
        assert_eq!(t.count("turn/start"), 2, "压缩放弃了，排着的放行");
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "排着");
    }

    #[test]
    fn the_fast_echo_window_is_eight_seconds_from_the_ack() {
        // 回显在窗口里到：生效。
        let mut t = T::ready(None);
        t.core.control(1, Control::Fast(true), t.at(1.0));
        t.pump();
        t.reply_at("thread/settings/update", json!({}), 1.0);
        assert_eq!(t.core.next_wakeup(), Some(t.at(9.0)));
        t.tick(8.9);
        assert!(t.other.is_empty(), "窗口没过");
        t.notify_at(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "fast" } }),
            8.95,
        );
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::Applied(_)
            }]
        ));
        assert_eq!(t.core.next_wakeup(), None, "回显到了，期限作废");

        // 窗口里没来：到点才是「无确认」。
        let mut t = T::ready(None);
        t.core.control(1, Control::Fast(true), t.at(1.0));
        t.pump();
        t.reply_at("thread/settings/update", json!({}), 1.0);
        t.tick(8.9);
        assert!(t.other.is_empty());
        t.tick(9.0);
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::NoConfirmation(_)
            }]
        ));
    }
    #[test]
    fn controls_initial_options_wait_for_record_and_ack_without_requiring_echo() {
        use crate::codex::control::{CodexOperation as Op, CodexOutcome as Out};
        let mut t = T::new(
            None,
            Some("Implement the exact task.\n你是 remains original user text."),
        );
        t.core.options.title = Some("Human task title".into());
        t.core.options.initial_settings = Some(crewd_proto::codex::CodexSettingsPatch {
            fast: Some(true),
            ..Default::default()
        });
        t.reply("initialize", json!({}));
        t.reply(
            "thread/start",
            json!({"thread":{"id":"thr-1"},"model":"synthetic-model"}),
        );
        assert_eq!(t.count("thread/name/set"), 0);
        assert_eq!(t.count("thread/settings/update"), 0);
        assert_eq!(t.count("turn/start"), 0);
        t.core.on_recorded(Ok(()), t.at(0.2));
        t.pump();
        assert_eq!(t.count("thread/name/set"), 1);
        assert_eq!(t.count("thread/settings/update"), 1);
        assert_eq!(
            t.count("turn/start"),
            0,
            "initial options must precede body"
        );
        t.reply_err("thread/name/set", "name rejected");
        assert_eq!(t.count("turn/start"), 0);
        t.reply("thread/settings/update", json!({}));
        assert_eq!(
            t.count("turn/start"),
            1,
            "documented settings ACK suffices; no invented echo requirement"
        );
        assert_eq!(
            t.last("turn/start")["params"]["input"][0]["text"],
            "Implement the exact task.\n你是 remains original user text."
        );
        assert!(t.events.iter().any(|e| matches!(
            e,
            SessionEvent::CodexObservation {
                accepted: Some(_),
                ..
            }
        )));
        t.core.codex_control(99, Op::Inspect, t.at(1.0));
        t.pump();
        assert!(t.other.iter().any(|a| matches!(
            a,
            CodexAction::CodexDone {
                ticket: 99,
                outcome: Out::Observed(_)
            }
        )));
    }
    #[test]
    fn controls_initial_rejection_and_unknown_never_deliver_body() {
        for timeout in [false, true] {
            let mut t = T::new(None, Some("Never send before qualified options"));
            t.core.options.initial_settings = Some(crewd_proto::codex::CodexSettingsPatch {
                fast: Some(true),
                ..Default::default()
            });
            t.reply("initialize", json!({}));
            t.reply(
                "thread/start",
                json!({"thread":{"id":"thr-1"},"model":"synthetic-model"}),
            );
            t.core.on_recorded(Ok(()), t.at(0.2));
            t.pump();
            if timeout {
                t.tick(31.0);
            } else {
                t.reply_err("thread/settings/update", "settings rejected");
            }
            assert_eq!(t.count("turn/start"), 0);
            t.tick(60.0);
            assert_eq!(t.count("turn/start"), 0, "no timer retry of body");
        }
    }
    #[test]
    fn controls_ack_is_next_turn_desired_not_effective_and_stop_cuts_late_ack() {
        use crate::codex::control::{CodexOperation as Op, CodexOutcome as Out};
        let mut t = T::ready(None);
        let patch = crewd_proto::codex::CodexSettingsPatch {
            model: Some("new-model".into()),
            ..Default::default()
        };
        t.core
            .codex_control(41, Op::Configure(patch.clone()), t.at(1.0));
        t.pump();
        t.reply("thread/settings/update", json!({}));
        assert!(t.other.iter().any(|a|matches!(a,CodexAction::CodexDone{ticket:41,outcome:Out::Observed(v)} if v["application"]=="followingTurns"&&v["observedSettings"].is_null())));
        t.core.codex_control(42, Op::Configure(patch), t.at(2.0));
        t.pump();
        t.core.stop(t.at(2.1));
        t.pump();
        t.reply_at("thread/settings/update", json!({}), 2.2);
        assert!(!t.other.iter().any(|a| matches!(
            a,
            CodexAction::CodexDone {
                ticket: 42,
                outcome: Out::Observed(_)
            }
        )));
        assert!(t.other.iter().any(|a| matches!(
            a,
            CodexAction::CodexDone {
                ticket: 42,
                outcome: Out::Unknown { .. }
            }
        )));
    }
    #[test]
    fn controls_exact_steer_and_next_turn_queue_are_distinct() {
        use crate::codex::control::{CodexOperation as Op, CodexOutcome as Out};
        let mut t = T::ready(None);
        t.notify(
            "turn/started",
            json!({"threadId":"thr-1","turn":{"id":"active"}}),
        );
        let request = "acebe9bb-b002-42f7-b7f2-0a54c2e0bff6".to_string();
        t.core.codex_control(
            1,
            Op::Steer {
                text: "Wrong turn".into(),
                expected_turn_id: "old".into(),
                request_id: request.clone(),
            },
            t.at(1.0),
        );
        t.pump();
        assert_eq!(t.count("turn/steer"), 0);
        assert_eq!(t.count("turn/start"), 0);
        t.core.codex_control(
            2,
            Op::Steer {
                text: "Exact original text".into(),
                expected_turn_id: "active".into(),
                request_id: request.clone(),
            },
            t.at(1.1),
        );
        t.pump();
        assert_eq!(t.last("turn/steer")["params"]["expectedTurnId"], "active");
        t.reply("turn/steer", json!({"turnId":"different"}));
        assert!(t.other.iter().any(|a| matches!(
            a,
            CodexAction::CodexDone {
                ticket: 2,
                outcome: Out::Unknown { .. }
            }
        )));
        t.core.codex_control(
            3,
            Op::Queue {
                id: 101,
                text: "Only the next turn".into(),
                request_id: request.clone(),
            },
            t.at(1.2),
        );
        t.pump();
        assert_eq!(t.count("turn/start"), 0);
        t.notify(
            "turn/completed",
            json!({"threadId":"thr-1","turn":{"id":"active","status":"completed"}}),
        );
        assert_eq!(t.count("turn/start"), 1);
        assert_eq!(
            t.last("turn/start")["params"]["clientUserMessageId"],
            request
        );
        assert_eq!(
            t.last("turn/start")["params"]["input"][0]["text"],
            "Only the next turn"
        );
    }
    #[test]
    fn controls_review_start_owns_the_next_turn_boundary_until_native_resolution() {
        use crate::codex::control::CodexOperation as Op;
        use crewd_proto::codex::CodexReviewTarget;
        for outcome in ["accepted", "rejected", "unknown"] {
            let mut t = T::ready(None);
            t.core.codex_control(
                70,
                Op::Review(CodexReviewTarget::UncommittedChanges),
                t.at(1.0),
            );
            t.pump();
            t.core.codex_control(
                71,
                Op::Queue {
                    id: 101,
                    text: "queued after review".into(),
                    request_id: "acebe9bb-b002-42f7-b7f2-0a54c2e0bff6".into(),
                },
                t.at(1.1),
            );
            t.pump();
            assert_eq!(
                t.count("turn/start"),
                0,
                "review/start pending must hold the following turn"
            );
            match outcome {
                "accepted" => {
                    t.reply(
                        "review/start",
                        json!({"turn":{"id":"review-turn","status":"inProgress"}}),
                    );
                    assert_eq!(
                        t.count("turn/start"),
                        0,
                        "native review ACK owns its active turn before notification"
                    );
                    t.notify("turn/completed",json!({"threadId":"thr-1","turn":{"id":"review-turn","status":"completed"}}));
                    assert_eq!(t.count("turn/start"), 1);
                }
                "rejected" => {
                    t.reply_err("review/start", "review refused");
                    assert_eq!(t.count("turn/start"), 1);
                }
                _ => {
                    t.tick(31.0);
                    t.tick(60.0);
                    assert_eq!(
                        t.count("turn/start"),
                        0,
                        "unconfirmed review cannot open/retry a following turn"
                    );
                }
            }
        }
    }

    #[test]
    fn controls_strict_resume_never_falls_back_to_new_thread() {
        let mut t = T::new(Some("native-stored"), None);
        t.core.options.strict_resume = true;
        t.reply("initialize", json!({}));
        let p = t.last("thread/resume")["params"].clone();
        for key in [
            "model",
            "serviceTier",
            "approvalPolicy",
            "approvalsReviewer",
            "sandbox",
        ] {
            assert!(p.get(key).is_none(), "resume must not silently reset {key}");
        }
        t.reply_err("thread/resume", "native source unavailable");
        assert_eq!(t.count("thread/start"), 0);
        assert_eq!(t.count("turn/start"), 0);
        assert!(t.other.iter().any(|a| matches!(a, CodexAction::Failed(_))));
    }
    fn completion_prepared(root:&std::path::Path)->crate::codex::input::Prepared {
        use crewd_proto::codex_inputs::CodexInputKind;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root,std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::codex::input::Submission{payloads:vec![crate::codex::input::Payload{kind:CodexInputKind::Text,filename:"notes.txt".into(),mime_type:"text/plain".into(),sha256:"a".repeat(64),body:b"Separate original attachment".to_vec()}],skills:vec![]}.prepare(root).unwrap()
    }
    #[test]
    fn completion_native_input_preserves_task_bytes_and_next_turn_queue_boundary(){
        use crate::codex::control::CodexOperation as Op;
        let root=tempfile::tempdir().unwrap();let mut t=T::ready(None);
        t.notify("turn/started",json!({"threadId":"thr-1","turn":{"id":"busy"}}));
        let text="Task first: exact \n你是用户原文";
        t.core.codex_control(201,Op::PreparedQueue{id:800,text:text.into(),request_id:uuid::Uuid::new_v4().to_string(),inputs:completion_prepared(root.path())},t.at(1.0));t.pump();
        assert_eq!(t.count("turn/start"),0);
        t.notify("turn/completed",json!({"threadId":"thr-1","turn":{"id":"busy","status":"completed"}}));
        assert_eq!(t.count("turn/start"),1);let sent=t.last("turn/start");
        assert_eq!(sent["params"]["input"][0]["text"],text);
        assert!(sent["params"]["input"][1]["text"].as_str().unwrap().contains("Separate original attachment"));
        assert_eq!(sent["params"]["input"].as_array().unwrap().len(),2);
    }
    #[test]
    fn completion_first_input_waits_for_record_and_never_rewrites_prompt(){
        let root=tempfile::tempdir().unwrap();let mut t=T::new(None,Some("Specific first task"));
        t.core.initial_inputs(completion_prepared(root.path()));
        t.reply("initialize",json!({"userAgent":"synthetic"}));
        assert_eq!(t.last("thread/start")["params"]["historyMode"],"paginated");
        t.reply("thread/start",json!({"thread":{"id":"thr-1","historyMode":"paginated"}}));assert_eq!(t.count("turn/start"),0);
        t.core.on_recorded(Ok(()),t.at(0.2));t.pump();assert_eq!(t.count("turn/start"),1);
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"],"Specific first task");assert_eq!(t.last("turn/start")["params"]["input"].as_array().unwrap().len(),2);
    }
    #[test]
    fn completion_prepared_steer_requires_exact_active_turn_and_never_queues(){
        use crate::codex::control::CodexOperation as Op;
        let root=tempfile::tempdir().unwrap();let mut t=T::ready(None);
        let op=|turn:&str|Op::PreparedSteer{text:"Change current task".into(),expected_turn_id:turn.into(),request_id:uuid::Uuid::new_v4().to_string(),inputs:completion_prepared(root.path())};
        t.core.codex_control(1,op("wrong"),t.at(1.0));t.pump();assert_eq!(t.count("turn/steer"),0);assert_eq!(t.count("turn/start"),0);
        t.notify("turn/started",json!({"threadId":"thr-1","turn":{"id":"active"}}));
        t.core.codex_control(2,op("active"),t.at(1.1));t.pump();assert_eq!(t.count("turn/steer"),1);assert_eq!(t.last("turn/steer")["params"]["input"].as_array().unwrap().len(),2);
        t.core.stop(t.at(1.2));t.pump();t.core.codex_control(3,op("active"),t.at(1.3));t.pump();assert_eq!(t.count("turn/steer"),1);
    }
    #[test]
    fn completion_permission_profile_is_current_allowed_catalog_and_stop_fenced(){
        use crate::codex::control::CodexOperation as Op;
        use crewd_proto::codex::CodexSettingsPatch;
        for mode in ["allowed","disabled","missing","stop"] {
            let mut t=T::ready(None);
            t.core.codex_control(1,Op::Configure(CodexSettingsPatch{permissions:Some("workspace-custom".into()),..Default::default()}),t.at(1.0));t.pump();
            assert_eq!(t.count("permissionProfile/list"),1);assert_eq!(t.count("thread/settings/update"),0);
            assert_eq!(t.last("permissionProfile/list")["params"]["cwd"],"/repo");
            if mode=="stop"{t.core.stop(t.at(1.1));t.pump();}
            t.reply("permissionProfile/list",json!({"data":if mode=="missing"{vec![]}else{vec![json!({"id":"workspace-custom","allowed":mode!="disabled","description":"Synthetic"})]}}));
            assert_eq!(t.count("thread/settings/update"),usize::from(mode=="allowed"));
            if mode=="allowed"{assert_eq!(t.last("thread/settings/update")["params"]["permissions"],"workspace-custom");assert!(t.last("thread/settings/update")["params"].get("sandboxPolicy").is_none());}
        }
    }
    #[test]
    fn completion_revert_requires_observed_paginated_idle_and_unknown_holds_queue(){
        use crate::codex::control::CodexOperation as Op;
        use crewd_proto::codex::CodexHistoryAction;
        for mode in ["legacy","paginated","unknown"] {
            let mut t=T::new(None,None);t.reply("initialize",json!({"userAgent":"synthetic"}));
            t.reply("thread/start",json!({"thread":{"id":"thr-1","historyMode":if mode=="legacy"{"legacy"}else{"paginated"}}}));t.core.on_recorded(Ok(()),t.at(0.2));t.pump();
            t.core.codex_control(7,Op::HistoryMutation{action:CodexHistoryAction::Revert,before_turn_id:Some("original-turn".into())},t.at(1.0));t.pump();
            assert_eq!(t.count("thread/revert"),usize::from(mode!="legacy"));
            if mode=="legacy"{continue}
            if mode=="unknown"{t.tick(40.0);}else{t.reply("thread/revert",json!({"thread":{"id":"thr-1"}}));}
            t.core.codex_control(8,Op::Queue{id:80,text:"Next explicit task".into(),request_id:uuid::Uuid::new_v4().to_string()},t.at(41.0));t.pump();
            assert_eq!(t.count("turn/start"),usize::from(mode=="paginated"));
        }
    }

}
