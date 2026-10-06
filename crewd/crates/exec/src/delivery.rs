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
    /// 终局：正文反复没进输入行。
    Undelivered(String),
    /// 终局：正文进去了，回车反复没被接受。
    Unsubmitted(String),
    /// 终局：送达并提交。
    Delivered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingReady,
    Writing {
        attempt: u32,
        not_before: Instant,
        deadline: Instant,
        baseline: String,
    },
    /// `submitted == None`：提示符已被长正文顶出屏幕，提交成功 = 输入行重新出现。
    Submitting {
        attempt: u32,
        deadline: Instant,
        submitted: Option<String>,
    },
    Finished,
}

#[derive(Clone, Debug)]
pub struct Delivery {
    timing: Timing,
    subject: Subject,
    phase: Phase,
    /// 等就绪的起点；被对话框挡住期间往后推（等人不算「没就绪」）。
    readiness_since: Instant,
    dialog_since: Option<Instant>,
    /// 输入行这一段连续在屏的起点。
    ready_since: Option<Instant>,
    reported_dialog: bool,
    reported_not_ready: bool,
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
            readiness_since: started_at,
            dialog_since: None,
            ready_since: None,
            reported_dialog: false,
            reported_not_ready: false,
        }
    }

    pub fn subject(&self) -> Subject {
        self.subject
    }

    /// 已给出终局裁决（成功或放弃）。
    pub fn is_finished(&self) -> bool {
        self.phase == Phase::Finished
    }

    /// 下一个「到点就会有变化」的时刻（驱动层据此定下一次唤醒）。`None` = 只等画面变化。
    pub fn next_deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::AwaitingReady => {
                if let Some(since) = self.dialog_since {
                    (!self.reported_dialog).then_some(since + self.timing.dialog_stable)
                } else if let Some(since) = self.ready_since {
                    Some(since + self.timing.ready_settle)
                } else {
                    (!self.reported_not_ready)
                        .then_some(self.readiness_since + self.timing.readiness_deadline)
                }
            }
            Phase::Writing {
                not_before,
                deadline,
                ..
            } => Some((*not_before).min(*deadline)),
            Phase::Submitting { deadline, .. } => Some(*deadline),
            Phase::Finished => None,
        }
    }

    pub fn step(&mut self, obs: Observation<'_>) -> Step {
        let now = obs.now;
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
                    let since = *self.ready_since.get_or_insert(now);
                    if now.saturating_duration_since(since) < t.ready_settle {
                        return Step::Idle;
                    }
                    self.phase = Phase::Writing {
                        attempt: 1,
                        not_before: now + t.submit_gap,
                        deadline: now + t.verify_window,
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
                    if now < not_before || !obs.body_visible {
                        return Step::Idle;
                    }
                    self.phase = Phase::Submitting {
                        attempt: 1,
                        deadline: now + t.verify_window,
                        submitted: None,
                    };
                    return Step::Submit { attempt: 1 };
                };
                if now >= not_before && landed(row, &baseline) {
                    self.phase = Phase::Submitting {
                        attempt: 1,
                        deadline: now + t.verify_window,
                        submitted: Some(row.to_string()),
                    };
                    return Step::Submit { attempt: 1 };
                }
                if now < deadline {
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
                    not_before: now + t.submit_gap,
                    deadline: now + t.verify_window,
                    baseline,
                };
                Step::WriteBody {
                    attempt: attempt + 1,
                }
            }
            Phase::Submitting {
                attempt,
                deadline,
                submitted,
            } => {
                let Some(row) = obs.input_row else {
                    return Step::Idle;
                };
                if submitted.as_deref() != Some(row) {
                    self.phase = Phase::Finished;
                    return Step::Delivered;
                }
                if now < deadline {
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
                    deadline: now + t.verify_window,
                    submitted,
                };
                Step::Submit {
                    attempt: attempt + 1,
                }
            }
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
        assert_eq!(d.step(obs(None, c.at(3.0))), Step::Idle);
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
}
