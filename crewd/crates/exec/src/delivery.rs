//! 把一段正文投进 claude 输入框的状态机（Swift `StartupPromptDelivery` 的移植）。
//!
//! 判据全部作用在**渲染完的那屏**上，不赌时序：
//! 1. 就绪 = 屏上有输入行（`crewd_term::input_row` 非 `None`），且连续 `ready_settle` 没断、
//!    其间没有对话框。录下来的信任框首屏（`tui-claude-trust.bin`）画到一半时，
//!    `❯  No, exit` 那一行单独在屏、页脚还没画出来，那一刻它长得就是输入行——
//!    不等它稳住就会把正文当按键敲进信任框。
//! 2. 落地 = 写完正文后那一行从原样变成非空的别的东西（claude 会把大段输入折成
//!    `[Pasted text #1 +N lines]`，所以不逐字比）；最早 `submit_gap` 之后才作数。
//! 3. 提交 = 发回车后那一行又变了（或提示符被长正文顶出屏幕后重新出现）。
//!
//! 每步有上限，到顶给出终局裁决。等就绪期间的两种坏消息（被对话框挡住、等不到输入框）
//! **只报不终止**：答完对话框 / 输入框终于画出来，照样投出去。
//! 纯值、时间由调用方传入——整台机器可以脱离进程单测。
use std::time::{Duration, Instant};

use crewd_term::STABLE_WINDOW;

/// 这一笔是什么。只影响裁决措辞，不影响任何判据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// 开场任务。
    Brief,
    /// 运行中的一条指令（唤醒 / 续跑 / steer）。
    Steer,
}

impl Subject {
    fn noun(self) -> &'static str {
        match self {
            Self::Brief => "开场任务",
            Self::Steer => "这条指令",
        }
    }

    fn lost_tail(self) -> &'static str {
        match self {
            Self::Brief => "这个 session 一个字都没收到，派给它的活等于没派出去——请改派或重起。",
            Self::Steer => "session 本身是好的、屏幕上也有字，但它没收到这条——需要有人重发。",
        }
    }
}

/// 上限。缺省值的理由见 Swift 版注释；调小只该发生在测试里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// 等输入框就绪多久后报一次 `NotReady`（报完继续等）。
    pub readiness_deadline: Duration,
    /// 一次写入之后最多等多久回读判定。
    pub verify_window: Duration,
    /// 写完正文后最早多久才认「落地」（刚写完那一帧属于写之前；正文与回车也要分开到达）。
    pub submit_gap: Duration,
    /// 写正文 / 发回车各自最多几次。
    pub max_attempts: u32,
    /// 对话框稳定多久才认——与待决策跟踪器同一个常量。
    pub dialog_stable: Duration,
    /// 输入行要连续在屏多久才算就绪（挡住「对话框画到一半像输入行」）。
    pub ready_settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            readiness_deadline: Duration::from_secs(90),
            verify_window: Duration::from_millis(2500),
            submit_gap: Duration::from_millis(500),
            max_attempts: 3,
            dialog_stable: STABLE_WINDOW,
            ready_settle: Duration::from_millis(300),
        }
    }
}

/// 一拍观测量，全部来自权威屏幕。
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    /// 输入行提示符后的内容；`None` = 屏上没有输入行。
    pub input_row: Option<&'a str>,
    /// 正文末尾是否已出现在屏上（长正文会把提示符顶出屏幕）。
    pub body_visible: bool,
    /// 屏上有需要人回答的对话框。
    pub dialog_present: bool,
    pub now: Instant,
}

/// 这一拍该做什么。调用方只执行，不判断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    /// 写正文（不带回车）。`attempt > 1` 时先发 Ctrl-U 清掉输入行。
    WriteBody {
        attempt: u32,
    },
    /// 发回车（经空格 + 退格 + 回车三拍，见 `crate::session`）。
    Submit {
        attempt: u32,
    },
    /// 对话框挡着，没投。只报一次，**不终止**。
    BlockedByDialog(String),
    /// 等到期限也没见到输入框。只报一次，**不终止**。
    NotReady(String),
    /// 写入 / 提交之后，过了本阶段的期限输入框仍不在屏上（整屏重绘卡住、被别的模态占了屏）。
    /// 只报一次，**不终止**、不盲目重试（输入框不在就没法回读，重试只会更乱）；
    /// 输入框回来就按常规继续，一直不回来则 `readiness_deadline` 之后给终局裁决。
    Stalled(String),
    /// 终局：正文反复没进输入行。
    Undelivered(String),
    /// 终局：正文进去了，回车反复没被接受。
    Unsubmitted(String),
    /// 终局：送达并提交。
    Delivered,
}

/// `None` = 没有期限（`Instant` 加法溢出，如 `Duration::MAX`）：永远不到点，不 panic。
type Deadline = Option<Instant>;

fn after(base: Instant, d: Duration) -> Deadline {
    base.checked_add(d)
}

fn reached(deadline: Deadline, now: Instant) -> bool {
    deadline.is_some_and(|d| now >= d)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingReady,
    Writing {
        attempt: u32,
        not_before: Deadline,
        deadline: Deadline,
        baseline: String,
    },
    /// `submitted == None`：提示符已被长正文顶出屏幕，提交成功 = 输入行重新出现。
    Submitting {
        attempt: u32,
        deadline: Deadline,
        submitted: Option<String>,
    },
    /// 上一轮提交拍被调用方撤回（回车没发出去：屏上弹了对话框 / 输入行没了）。这一轮不算
    /// 提交过；对话框不在、输入行回来（或正文尾巴可见）就发第 `attempt` 轮。
    Resubmit {
        attempt: u32,
        deadline: Deadline,
    },
    Finished,
}

#[derive(Clone, Debug)]
pub struct Delivery {
    timing: Timing,
    subject: Subject,
    phase: Phase,
    /// 最近一次 `step` 的 `now`：`next_deadline` 只返回严格晚于它的时刻，
    /// 否则驱动层 `sleep_until` 会对着过去的时刻空转。
    last_now: Instant,
    /// 等就绪的起点；被对话框挡住期间往后推（等人不算「没就绪」）。
    readiness_since: Instant,
    dialog_since: Option<Instant>,
    /// 输入行这一段「文本不变、连续在屏」的起点与那段文本。
    ready_since: Option<Instant>,
    ready_row: String,
    /// 写入 / 提交之后输入行缺席的起点（输入行回来就清掉）。
    stalled_since: Option<Instant>,
    reported_dialog: bool,
    reported_not_ready: bool,
    reported_stall: bool,
}

/// 「正文真的进了输入行」：非空且不同于写之前。
pub fn landed(row: &str, baseline: &str) -> bool {
    !row.is_empty() && row != baseline
}

impl Delivery {
    pub fn new(timing: Timing, subject: Subject, started_at: Instant) -> Self {
        Self {
            timing,
            subject,
            phase: Phase::AwaitingReady,
            last_now: started_at,
            readiness_since: started_at,
            dialog_since: None,
            ready_since: None,
            ready_row: String::new(),
            stalled_since: None,
            reported_dialog: false,
            reported_not_ready: false,
            reported_stall: false,
        }
    }

    pub fn subject(&self) -> Subject {
        self.subject
    }

    /// 已给出终局裁决（成功或放弃）。
    /// 调用方没把这一轮提交拍发完（回车没发出去）。只在提交阶段有效：这一轮不算提交过，
    /// 安全了（没有对话框、输入行或正文尾巴在屏）再发一轮，记一次重试；超过
    /// `max_attempts` 轮就给终局 `Unsubmitted`。
    pub fn submit_aborted(&mut self, now: Instant) {
        self.last_now = self.last_now.max(now);
        if let Phase::Submitting { attempt, .. } = self.phase {
            self.phase = Phase::Resubmit {
                attempt: attempt + 1,
                deadline: after(now, self.timing.verify_window),
            };
        }
    }

    pub fn is_finished(&self) -> bool {
        self.phase == Phase::Finished
    }

    /// 下一个「到点就会有变化」的**将来**时刻（驱动层据此定下一次唤醒）。
    /// 保证 `None` 或严格晚于最近一次 `step` 的 `now`；`None` = 只等画面变化 / 驱动层自己的节拍。
    pub fn next_deadline(&self) -> Option<Instant> {
        let t = &self.timing;
        let stall_cap = self
            .stalled_since
            .and_then(|s| after(s, t.readiness_deadline));
        let candidates: [Deadline; 2] = match &self.phase {
            Phase::AwaitingReady => {
                if let Some(since) = self.dialog_since {
                    [
                        (!self.reported_dialog)
                            .then(|| after(since, t.dialog_stable))
                            .flatten(),
                        None,
                    ]
                } else if let Some(since) = self.ready_since {
                    [after(since, t.ready_settle), None]
                } else {
                    [
                        (!self.reported_not_ready)
                            .then(|| after(self.readiness_since, t.readiness_deadline))
                            .flatten(),
                        None,
                    ]
                }
            }
            Phase::Writing {
                not_before,
                deadline,
                ..
            } => [*not_before, *deadline],
            Phase::Submitting { deadline, .. } | Phase::Resubmit { deadline, .. } => {
                [*deadline, None]
            }
            Phase::Finished => [None, None],
        };
        candidates
            .into_iter()
            .chain([stall_cap])
            .flatten()
            .filter(|d| *d > self.last_now)
            .min()
    }

    pub fn step(&mut self, obs: Observation<'_>) -> Step {
        let now = obs.now;
        self.last_now = self.last_now.max(now);
        let t = self.timing;
        match self.phase.clone() {
            Phase::Finished => Step::Idle,
            Phase::AwaitingReady => {
                if obs.dialog_present {
                    self.ready_since = None;
                    let since = *self.dialog_since.get_or_insert(now);
                    self.readiness_since = now;
                    if now.saturating_duration_since(since) < t.dialog_stable
                        || self.reported_dialog
                    {
                        return Step::Idle;
                    }
                    self.reported_dialog = true;
                    return Step::BlockedByDialog(format!(
                        "{}没投出去：屏幕上摆着一个需要人回答的对话框（信任此文件夹 / 命令审批这类），\
                         正文投进去会被当成按键吃掉。看一眼屏幕、代答掉它，答完会自己补投出去。",
                        self.subject.noun()
                    ));
                }
                self.dialog_since = None;
                if let Some(row) = obs.input_row {
                    // 就绪 = 同一段文本连续在屏：文本一变（`""` → `Try "…"` 提示）计时重来，
                    // 这样写入前的基线是个稳定的行，而不是某一帧的过渡。
                    if self.ready_since.is_none() || self.ready_row != row {
                        self.ready_since = Some(now);
                        self.ready_row = row.to_string();
                    }
                    let since = self.ready_since.unwrap_or(now);
                    if now.saturating_duration_since(since) < t.ready_settle {
                        return Step::Idle;
                    }
                    self.phase = Phase::Writing {
                        attempt: 1,
                        not_before: after(now, t.submit_gap),
                        deadline: after(now, t.verify_window),
                        baseline: row.to_string(),
                    };
                    return Step::WriteBody { attempt: 1 };
                }
                self.ready_since = None;
                if now.saturating_duration_since(self.readiness_since) < t.readiness_deadline
                    || self.reported_not_ready
                {
                    return Step::Idle;
                }
                self.reported_not_ready = true;
                Step::NotReady(format!(
                    "{}还没投出去：TUI 在吐输出，但等了 {}s 也没在画面上等到输入框。看一眼它停在哪一屏。",
                    self.subject.noun(),
                    t.readiness_deadline.as_secs()
                ))
            }
            Phase::Writing {
                attempt,
                not_before,
                deadline,
                baseline,
            } => {
                let Some(row) = obs.input_row else {
                    // 重绘到一半输入行暂时不在——等下一拍；除非正文尾巴已经清楚可见
                    // （长正文把提示符顶出了屏幕），那就是落地了。
                    // 对话框在场时不算：空格会被当成按键吃进对话框。
                    if reached(not_before, now) && obs.body_visible && !obs.dialog_present {
                        self.stalled_since = None;
                        self.phase = Phase::Submitting {
                            attempt: 1,
                            deadline: after(now, t.verify_window),
                            submitted: None,
                        };
                        return Step::Submit { attempt: 1 };
                    }
                    return self.input_row_absent(now, deadline, attempt, true);
                };
                self.stalled_since = None;
                if reached(not_before, now) && landed(row, &baseline) {
                    self.phase = Phase::Submitting {
                        attempt: 1,
                        deadline: after(now, t.verify_window),
                        submitted: Some(row.to_string()),
                    };
                    return Step::Submit { attempt: 1 };
                }
                if !reached(deadline, now) {
                    return Step::Idle;
                }
                if attempt >= t.max_attempts {
                    self.phase = Phase::Finished;
                    return Step::Undelivered(format!(
                        "{}没送到：正文写了 {attempt} 次，回读屏幕确认输入行始终没变（重绘把它吞了）。{}",
                        self.subject.noun(),
                        self.subject.lost_tail()
                    ));
                }
                self.phase = Phase::Writing {
                    attempt: attempt + 1,
                    not_before: after(now, t.submit_gap),
                    deadline: after(now, t.verify_window),
                    baseline,
                };
                Step::WriteBody {
                    attempt: attempt + 1,
                }
            }
            Phase::Resubmit { attempt, deadline } => {
                if attempt > t.max_attempts {
                    self.phase = Phase::Finished;
                    return Step::Unsubmitted(format!(
                        "{}卡在输入框里没提交：正文已经进去了，但提交被打断了 {} 次\
                         （每次回车之前屏上都弹出了对话框、或输入框不见了），一直没能安全地按下回车。\
                         看一眼屏幕：代答掉挡路的框，或按 Ctrl-U 清掉再重发。",
                        self.subject.noun(),
                        attempt - 1
                    ));
                }
                let safe = !obs.dialog_present && (obs.input_row.is_some() || obs.body_visible);
                if !safe {
                    return self.input_row_absent(now, deadline, attempt - 1, false);
                }
                self.stalled_since = None;
                self.phase = Phase::Submitting {
                    attempt,
                    deadline: after(now, t.verify_window),
                    submitted: obs.input_row.map(str::to_string),
                };
                Step::Submit { attempt }
            }
            Phase::Submitting {
                attempt,
                deadline,
                submitted,
            } => {
                let Some(row) = obs.input_row else {
                    return self.input_row_absent(now, deadline, attempt, false);
                };
                self.stalled_since = None;
                if submitted.as_deref() != Some(row) {
                    self.phase = Phase::Finished;
                    return Step::Delivered;
                }
                if !reached(deadline, now) {
                    return Step::Idle;
                }
                if attempt >= t.max_attempts {
                    self.phase = Phase::Finished;
                    return Step::Unsubmitted(format!(
                        "{}卡在输入框里没提交：正文已经进去了，但回车发了 {attempt} 次都没被接受\
                         （多半有个模态挡在前面）。输入框里那段没人敲过的字就是它——代答掉挡路的框，\
                         或按 Ctrl-U 清掉再重发。",
                        self.subject.noun()
                    ));
                }
                self.phase = Phase::Submitting {
                    attempt: attempt + 1,
                    deadline: after(now, t.verify_window),
                    submitted,
                };
                Step::Submit {
                    attempt: attempt + 1,
                }
            }
        }
    }

    /// 写入 / 提交阶段输入行不在屏上。期限之前当重绘处理；过了期限先报一次 `Stalled`
    /// （不终止、不重试——没有输入行就没有回读，重试只会更乱），
    /// 缺席满 `readiness_deadline` 仍不回来就给终局裁决，免得后面排队的活一直饿着。
    fn input_row_absent(
        &mut self,
        now: Instant,
        deadline: Deadline,
        attempt: u32,
        writing: bool,
    ) -> Step {
        if !reached(deadline, now) {
            return Step::Idle;
        }
        let t = self.timing;
        let since = *self.stalled_since.get_or_insert(now);
        let noun = self.subject.noun();
        if !self.reported_stall {
            self.reported_stall = true;
            let what = if writing {
                "写了正文"
            } else {
                "发了回车"
            };
            return Step::Stalled(format!(
                "{noun}{what}之后，输入框一直没回到屏上（整屏重绘卡住或被别的界面占了）。\
                 先不重试，等它回来；等满 {}s 还不回来就放弃。",
                t.readiness_deadline.as_secs()
            ));
        }
        if now.saturating_duration_since(since) < t.readiness_deadline {
            return Step::Idle;
        }
        self.phase = Phase::Finished;
        let secs = t.readiness_deadline.as_secs();
        if writing {
            Step::Undelivered(format!(
                "{noun}没送到：正文写了 {attempt} 次，之后输入框等了 {secs}s 也没回来，没法确认它进没进去。{}",
                self.subject.lost_tail()
            ))
        } else {
            Step::Unsubmitted(format!(
                "{noun}没提交：回车发了 {attempt} 次，之后输入框等了 {secs}s 也没回来，没法确认它提交没提交。\
                 看一眼屏幕再决定要不要重发。"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> Timing {
        Timing {
            readiness_deadline: Duration::from_secs(10),
            verify_window: Duration::from_secs(2),
            submit_gap: Duration::from_millis(500),
            max_attempts: 3,
            dialog_stable: Duration::from_secs(3),
            ready_settle: Duration::ZERO,
        }
    }

    struct Clock(Instant);

    impl Clock {
        fn at(&self, secs: f64) -> Instant {
            self.0 + Duration::from_secs_f64(secs)
        }
    }

    fn obs(row: Option<&str>, at: Instant) -> Observation<'_> {
        Observation {
            input_row: row,
            body_visible: false,
            dialog_present: false,
            now: at,
        }
    }

    fn setup() -> (Delivery, Clock) {
        let t0 = Instant::now();
        (Delivery::new(timing(), Subject::Brief, t0), Clock(t0))
    }

    #[test]
    fn nothing_is_written_before_the_input_row_exists() {
        let (mut d, c) = setup();
        for s in 0..10 {
            assert_eq!(d.step(obs(None, c.at(s as f64))), Step::Idle, "第 {s}s");
        }
    }

    #[test]
    fn not_ready_is_reported_once_and_delivery_continues() {
        let (mut d, c) = setup();
        assert_eq!(d.step(obs(None, c.at(9.0))), Step::Idle);
        match d.step(obs(None, c.at(10.0))) {
            Step::NotReady(detail) => assert!(detail.contains("开场任务"), "{detail}"),
            other => panic!("过了期限必须报，实际 {other:?}"),
        }
        assert_eq!(
            d.step(obs(None, c.at(20.0))),
            Step::Idle,
            "同一条不许反复报"
        );
        assert!(!d.is_finished(), "报了不等于放弃");
        assert_eq!(
            d.step(obs(Some(""), c.at(30.0))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn happy_path_writes_verifies_then_submits() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.5))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(""), c.at(1.0))), Step::Idle);
        let pasted = "[Pasted text #1 +212 lines]";
        assert_eq!(
            d.step(obs(Some(pasted), c.at(1.5))),
            Step::Submit { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(pasted), c.at(2.0))), Step::Idle);
        assert_eq!(d.step(obs(Some(""), c.at(2.5))), Step::Delivered);
        assert!(d.is_finished());
        assert_eq!(d.step(obs(Some(""), c.at(3.0))), Step::Idle);
    }

    #[test]
    fn a_change_right_after_writing_does_not_count_as_landed() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let hint = "Try \"how do I log an error?\"";
        assert_eq!(d.step(obs(Some(hint), c.at(0.2))), Step::Idle);
        assert_eq!(d.step(obs(Some(hint), c.at(0.49))), Step::Idle);
        assert_eq!(
            d.step(obs(Some("[Pasted text #1 +212 lines]"), c.at(0.5))),
            Step::Submit { attempt: 1 }
        );
    }

    #[test]
    fn landed_means_changed_and_non_empty() {
        assert!(!landed("", ""));
        assert!(!landed("Try \"fix\"", "Try \"fix\""));
        assert!(landed("[Pasted text #1 +212 lines]", ""));
        assert!(landed("修一条 bug", "Try \"fix\""));
    }

    #[test]
    fn body_that_never_lands_is_retried_then_reported() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(Some(""), c.at(1.9))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(""), c.at(2.0))),
            Step::WriteBody { attempt: 2 }
        );
        assert_eq!(
            d.step(obs(Some(""), c.at(4.0))),
            Step::WriteBody { attempt: 3 }
        );
        match d.step(obs(Some(""), c.at(6.0))) {
            Step::Undelivered(detail) => assert!(detail.contains("3 次"), "{detail}"),
            other => panic!("到上限必须报，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn enter_that_is_never_accepted_is_retried_then_reported() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(0.5))),
            Step::Submit { attempt: 1 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(2.5))),
            Step::Submit { attempt: 2 }
        );
        assert_eq!(
            d.step(obs(Some("brief"), c.at(4.5))),
            Step::Submit { attempt: 3 }
        );
        match d.step(obs(Some("brief"), c.at(6.5))) {
            Step::Unsubmitted(detail) => assert!(detail.contains("没提交"), "{detail}"),
            other => panic!("到上限必须报，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn input_row_vanishing_mid_redraw_is_not_a_failure() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        // 期限（2s）之前输入行不在：静默等。
        assert_eq!(d.step(obs(None, c.at(1.5))), Step::Idle);
        // 过了期限还不在：报一次 Stalled（不终止），输入行回来后照常重写。
        assert!(matches!(d.step(obs(None, c.at(3.0))), Step::Stalled(_)));
        assert!(!d.is_finished());
        assert_eq!(
            d.step(obs(Some(""), c.at(3.1))),
            Step::WriteBody { attempt: 2 }
        );
    }

    #[test]
    fn long_body_that_pushed_the_prompt_off_screen_is_submitted() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let mut o = obs(None, c.at(0.5));
        assert_eq!(d.step(o), Step::Idle, "没有正文证据时按重绘处理");
        o.body_visible = true;
        o.now = c.at(0.6);
        assert_eq!(d.step(o), Step::Submit { attempt: 1 });
        // 提示符重新出现 = 提交了。
        assert_eq!(d.step(obs(Some(""), c.at(1.0))), Step::Delivered);
    }

    /// 修正 2：正文尾巴在屏上、可屏上同时摆着对话框——空格会被当成按键吃进对话框，不提交。
    #[test]
    fn visible_body_does_not_submit_into_a_dialog() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let mut o = obs(None, c.at(1.0));
        o.body_visible = true;
        o.dialog_present = true;
        assert_eq!(d.step(o), Step::Idle, "对话框在场不提交");
        o.dialog_present = false;
        o.now = c.at(1.5);
        assert_eq!(d.step(o), Step::Submit { attempt: 1 });
    }

    fn long_path_submitting(d: &mut Delivery, c: &Clock) {
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        let mut o = obs(None, c.at(0.6));
        o.body_visible = true;
        assert_eq!(d.step(o), Step::Submit { attempt: 1 });
    }

    /// 修正 1：提交拍被调用方撤回（回车没发出去）——这一轮不算提交过：输入行回来不是
    /// 「已投递」，等安全了重发一轮（记一次重试）。
    #[test]
    fn aborted_submit_is_resent_when_safe_and_not_taken_as_delivered() {
        let (mut d, c) = setup();
        long_path_submitting(&mut d, &c);
        d.submit_aborted(c.at(0.7));
        let mut dialog = obs(None, c.at(0.8));
        dialog.dialog_present = true;
        dialog.body_visible = true;
        assert_eq!(d.step(dialog), Step::Idle, "对话框还在，不重发");
        assert_eq!(
            d.step(obs(Some("正文"), c.at(1.5))),
            Step::Submit { attempt: 2 },
            "输入行带着正文回来 = 没提交，重发一轮"
        );
        assert_eq!(d.step(obs(Some("正文"), c.at(1.6))), Step::Idle);
        assert_eq!(d.step(obs(Some(""), c.at(1.8))), Step::Delivered);
    }

    #[test]
    fn aborted_submits_count_against_the_attempt_cap() {
        let (mut d, c) = setup();
        long_path_submitting(&mut d, &c);
        d.submit_aborted(c.at(0.7));
        assert_eq!(
            d.step(obs(Some("正文"), c.at(1.0))),
            Step::Submit { attempt: 2 }
        );
        d.submit_aborted(c.at(1.1));
        assert_eq!(
            d.step(obs(Some("正文"), c.at(1.2))),
            Step::Submit { attempt: 3 }
        );
        d.submit_aborted(c.at(1.3));
        match d.step(obs(Some("正文"), c.at(1.4))) {
            Step::Unsubmitted(detail) => assert!(detail.contains("3"), "{detail}"),
            other => panic!("三次都被打断应当放弃，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn dialog_blocks_reports_once_and_does_not_finish() {
        let (mut d, c) = setup();
        let dialog = |at| Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: at,
        };
        assert_eq!(d.step(dialog(c.at(0.0))), Step::Idle);
        assert_eq!(d.step(dialog(c.at(2.9))), Step::Idle);
        match d.step(dialog(c.at(3.0))) {
            Step::BlockedByDialog(detail) => assert!(detail.contains("需要人回答"), "{detail}"),
            other => panic!("对话框稳住后必须报，实际 {other:?}"),
        }
        assert_eq!(d.step(dialog(c.at(5.0))), Step::Idle);
        assert!(!d.is_finished(), "答完之后还要补投");
    }

    #[test]
    fn waiting_on_a_dialog_does_not_count_towards_not_ready() {
        let (mut d, c) = setup();
        for s in 0..=30 {
            let step = d.step(Observation {
                input_row: None,
                body_visible: false,
                dialog_present: true,
                now: c.at(s as f64),
            });
            assert!(
                !matches!(step, Step::NotReady(_)),
                "等人期间不该报没就绪（第 {s}s）"
            );
        }
        assert_eq!(
            d.step(obs(Some(""), c.at(31.0))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn steer_wording_differs_but_rules_do_not() {
        let t0 = Instant::now();
        let mut d = Delivery::new(timing(), Subject::Steer, t0);
        d.step(obs(Some(""), t0));
        for s in [2.0, 4.0] {
            d.step(obs(Some(""), t0 + Duration::from_secs_f64(s)));
        }
        match d.step(obs(Some(""), t0 + Duration::from_secs(6))) {
            Step::Undelivered(detail) => {
                assert!(detail.starts_with("这条指令"), "{detail}");
                assert!(detail.contains("需要有人重发"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// 录下来的信任框首屏画到一半时 `❯  No, exit` 单独在屏、像一行输入框：
    /// 输入行必须连续在屏 `ready_settle` 才算就绪，中途断一下就重新计。
    #[test]
    fn input_row_must_settle_before_anything_is_written() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut d = Delivery::new(
            Timing {
                ready_settle: Duration::from_millis(300),
                ..timing()
            },
            Subject::Brief,
            t0,
        );
        assert_eq!(d.step(obs(Some("No, exit"), at(0.0))), Step::Idle);
        assert_eq!(d.next_deadline(), Some(at(0.3)));
        assert_eq!(d.step(obs(Some("No, exit"), at(0.2))), Step::Idle);
        let dialog = Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: at(0.25),
        };
        assert_eq!(d.step(dialog), Step::Idle, "页脚画出来了：其实是对话框");
        assert_eq!(d.step(obs(Some(""), at(5.0))), Step::Idle, "答完后重新计");
        assert_eq!(d.step(obs(None, at(5.1))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(""), at(5.2))),
            Step::Idle,
            "断过一次再重新计"
        );
        assert_eq!(
            d.step(obs(Some(""), at(5.5))),
            Step::WriteBody { attempt: 1 }
        );
    }

    #[test]
    fn next_deadline_tracks_the_phase() {
        let (mut d, c) = setup();
        assert_eq!(
            d.next_deadline(),
            Some(c.at(10.0)),
            "等就绪：到期报 NotReady"
        );
        d.step(obs(Some(""), c.at(1.0)));
        assert_eq!(d.next_deadline(), Some(c.at(1.5)), "写完：submit_gap 先到");
        d.step(obs(Some("x"), c.at(1.5)));
        assert_eq!(d.next_deadline(), Some(c.at(3.5)), "提交：verify_window");
        d.step(obs(Some(""), c.at(2.0)));
        assert_eq!(d.next_deadline(), None);
    }

    /// 驱动层按 `next_deadline` 睡到点；返回已过去的时刻会让它空转吃满 CPU。
    /// 每一拍之后 `next_deadline` 必须是 `None` 或严格晚于这一拍的 `now`。
    fn step_checked(d: &mut Delivery, o: Observation<'_>, c: &Clock) -> Step {
        let now = o.now;
        let step = d.step(o);
        if let Some(dl) = d.next_deadline() {
            assert!(
                dl > now,
                "next_deadline 落在过去：now={:?}s deadline={:?}s（刚返回 {step:?}）",
                now.saturating_duration_since(c.0),
                dl.saturating_duration_since(c.0)
            );
        }
        step
    }

    #[test]
    fn next_deadline_is_never_in_the_past_while_writing() {
        let (mut d, c) = setup();
        let w = |d: &mut Delivery, s: f64| step_checked(d, obs(Some(""), c.at(s)), &c);
        assert_eq!(w(&mut d, 0.0), Step::WriteBody { attempt: 1 });
        assert_eq!(w(&mut d, 1.0), Step::Idle, "not_before(0.5) 已过");
        assert_eq!(d.next_deadline(), Some(c.at(2.0)), "只剩 verify_window");
        assert_eq!(w(&mut d, 2.0), Step::WriteBody { attempt: 2 });
        assert_eq!(w(&mut d, 3.0), Step::Idle);
        assert_eq!(w(&mut d, 4.0), Step::WriteBody { attempt: 3 });
        assert_eq!(w(&mut d, 5.0), Step::Idle);
        assert!(matches!(w(&mut d, 6.0), Step::Undelivered(_)));
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn next_deadline_is_never_in_the_past_while_submitting() {
        let (mut d, c) = setup();
        let w = |d: &mut Delivery, row: &str, s: f64| step_checked(d, obs(Some(row), c.at(s)), &c);
        assert_eq!(w(&mut d, "", 0.0), Step::WriteBody { attempt: 1 });
        assert_eq!(w(&mut d, "x", 0.5), Step::Submit { attempt: 1 });
        assert_eq!(w(&mut d, "x", 1.0), Step::Idle);
        assert_eq!(d.next_deadline(), Some(c.at(2.5)));
        assert_eq!(w(&mut d, "x", 2.5), Step::Submit { attempt: 2 });
        assert_eq!(w(&mut d, "x", 3.0), Step::Idle);
        assert_eq!(w(&mut d, "x", 4.5), Step::Submit { attempt: 3 });
        assert_eq!(w(&mut d, "x", 5.0), Step::Idle);
        assert!(matches!(w(&mut d, "x", 6.5), Step::Unsubmitted(_)));
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn next_deadline_is_never_in_the_past_while_waiting_or_stalled() {
        let (mut d, c) = setup();
        // 等就绪 → 对话框 → NotReady 已报
        assert_eq!(step_checked(&mut d, obs(None, c.at(1.0)), &c), Step::Idle);
        let dialog = |s: f64| Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: c.at(s),
        };
        assert_eq!(step_checked(&mut d, dialog(2.0), &c), Step::Idle);
        assert!(matches!(
            step_checked(&mut d, dialog(5.0), &c),
            Step::BlockedByDialog(_)
        ));
        assert_eq!(step_checked(&mut d, dialog(6.0), &c), Step::Idle);
        assert_eq!(step_checked(&mut d, obs(None, c.at(7.0)), &c), Step::Idle);
        assert!(matches!(
            step_checked(&mut d, obs(None, c.at(17.0)), &c),
            Step::NotReady(_)
        ));
        assert_eq!(d.next_deadline(), None);
        // 写入后输入行一直不回来：期限过后只剩 stall 的总上限。
        assert_eq!(
            step_checked(&mut d, obs(Some(""), c.at(18.0)), &c),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(step_checked(&mut d, obs(None, c.at(19.0)), &c), Step::Idle);
        assert!(matches!(
            step_checked(&mut d, obs(None, c.at(20.0)), &c),
            Step::Stalled(_)
        ));
        assert_eq!(
            d.next_deadline(),
            Some(c.at(30.0)),
            "stall 起算 + 10s 总上限"
        );
    }

    #[test]
    fn missing_input_row_after_write_is_reported_once_then_gives_up() {
        let (mut d, c) = setup();
        assert_eq!(
            d.step(obs(Some(""), c.at(0.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.step(obs(None, c.at(1.9))), Step::Idle, "期限之前不吵");
        match d.step(obs(None, c.at(2.0))) {
            Step::Stalled(detail) => assert!(detail.contains("输入框"), "{detail}"),
            other => panic!("期限已过输入框不在，必须报一次，实际 {other:?}"),
        }
        assert!(!d.is_finished(), "报了不终止，也不盲目重写");
        assert_eq!(d.step(obs(None, c.at(5.0))), Step::Idle, "同一条不许反复报");
        assert_eq!(d.step(obs(None, c.at(11.9))), Step::Idle);
        match d.step(obs(None, c.at(12.0))) {
            Step::Undelivered(detail) => {
                assert!(detail.contains("输入框"), "{detail}");
                assert!(detail.contains("没回来"), "{detail}");
            }
            other => panic!("输入框一直不回来必须终局，实际 {other:?}"),
        }
        assert!(d.is_finished(), "队列后面的才能往前走");
    }

    #[test]
    fn missing_input_row_after_enter_is_reported_once_then_gives_up() {
        let (mut d, c) = setup();
        d.step(obs(Some(""), c.at(0.0)));
        assert_eq!(
            d.step(obs(Some("brief"), c.at(0.5))),
            Step::Submit { attempt: 1 }
        );
        assert_eq!(step_checked(&mut d, obs(None, c.at(2.4)), &c), Step::Idle);
        match step_checked(&mut d, obs(None, c.at(2.5)), &c) {
            Step::Stalled(detail) => assert!(detail.contains("输入框"), "{detail}"),
            other => panic!("期限已过输入框不在，必须报一次，实际 {other:?}"),
        }
        assert_eq!(d.next_deadline(), Some(c.at(12.5)));
        assert_eq!(step_checked(&mut d, obs(None, c.at(8.0)), &c), Step::Idle);
        match step_checked(&mut d, obs(None, c.at(12.5)), &c) {
            Step::Unsubmitted(detail) => {
                assert!(detail.contains("没回来"), "{detail}");
            }
            other => panic!("输入框一直不回来必须终局，实际 {other:?}"),
        }
        assert!(d.is_finished());
    }

    #[test]
    fn input_row_coming_back_after_a_stall_resets_the_give_up_clock() {
        let (mut d, c) = setup();
        d.step(obs(Some(""), c.at(0.0)));
        assert!(matches!(d.step(obs(None, c.at(2.0))), Step::Stalled(_)));
        // 回来：重写第 2 次；再消失超期，不会拿旧的起点立刻放弃。
        assert_eq!(
            d.step(obs(Some(""), c.at(9.0))),
            Step::WriteBody { attempt: 2 }
        );
        assert_eq!(
            d.step(obs(None, c.at(11.0))),
            Step::Idle,
            "新的一轮已报过，不再吵"
        );
        assert!(!d.is_finished());
        assert!(matches!(
            d.step(obs(None, c.at(21.0))),
            Step::Undelivered(_)
        ));
    }

    /// 基线必须是稳定的输入行：先是 ""、随后冒出 `Try "…"` 提示，
    /// 计时要从提示出现重新算，基线也要取提示——否则提示本身会被当成「正文落地」。
    #[test]
    fn ready_settle_restarts_when_the_input_row_text_changes() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut d = Delivery::new(
            Timing {
                ready_settle: Duration::from_millis(300),
                ..timing()
            },
            Subject::Brief,
            t0,
        );
        let hint = "Try \"fix lint errors\"";
        assert_eq!(d.step(obs(Some(""), at(0.0))), Step::Idle);
        assert_eq!(d.step(obs(Some(""), at(0.1))), Step::Idle);
        assert_eq!(
            d.step(obs(Some(hint), at(0.2))),
            Step::Idle,
            "文本变了重新计"
        );
        assert_eq!(d.next_deadline(), Some(at(0.5)));
        assert_eq!(
            d.step(obs(Some(hint), at(0.4))),
            Step::Idle,
            "按旧起点早该就绪了"
        );
        assert_eq!(
            d.step(obs(Some(hint), at(0.5))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(
            d.step(obs(Some(hint), at(1.0))),
            Step::Idle,
            "基线是提示行：提示本身不算落地"
        );
        assert_eq!(
            d.step(obs(Some("[Pasted text #1 +3 lines]"), at(1.1))),
            Step::Submit { attempt: 1 }
        );
    }

    #[test]
    fn huge_timings_saturate_instead_of_panicking() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let max = Duration::MAX;
        let mut d = Delivery::new(
            Timing {
                readiness_deadline: max,
                verify_window: max,
                submit_gap: max,
                max_attempts: 3,
                dialog_stable: max,
                ready_settle: Duration::ZERO,
            },
            Subject::Brief,
            t0,
        );
        assert_eq!(d.next_deadline(), None, "溢出 = 没有期限");
        assert_eq!(d.step(obs(None, at(1.0))), Step::Idle);
        let dialog = Observation {
            input_row: None,
            body_visible: false,
            dialog_present: true,
            now: at(2.0),
        };
        assert_eq!(d.step(dialog), Step::Idle);
        assert_eq!(d.next_deadline(), None);
        assert_eq!(
            d.step(obs(Some(""), at(3.0))),
            Step::WriteBody { attempt: 1 }
        );
        assert_eq!(d.next_deadline(), None);
        assert_eq!(d.step(obs(Some(""), at(1000.0))), Step::Idle);
        assert_eq!(d.step(obs(None, at(2000.0))), Step::Idle);
        assert_eq!(d.next_deadline(), None);

        let mut d = Delivery::new(
            Timing {
                ready_settle: max,
                ..timing()
            },
            Subject::Steer,
            t0,
        );
        assert_eq!(d.step(obs(Some(""), at(0.0))), Step::Idle);
        assert_eq!(d.next_deadline(), None);
    }
}
