//! 「有个菜单在等人」的出现/消失跟踪（Swift `PendingDecisionTracker` 的移植）。
//!
//! 两条来源、一个出口：渲染完的屏幕（优先，权威措辞）和去 ANSI 的字节尾窗。
//! 同一指纹稳定 [`STABLE_WINDOW`] 才报 `Appeared`；已在跟踪/已报的那个指纹粘住，
//! 免得两条来源措辞略不同就来回抢；两条都认不出才报 `Cleared`。
//! 时间由调用方传入（`now`），不读时钟——单测用假时间。
use std::time::{Duration, Instant};

use crate::input_box::blocking_dialog;
use crate::menu::{PendingDecision, parse_menu};
use crate::tail::AnsiPlainTextTail;

/// 菜单在屏上稳住多久才算真在等人。全仓唯一的这个阈值（M2b 的投递状态机直接引它）。
pub const STABLE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionEvent {
    Appeared(PendingDecision),
    Cleared,
}

#[derive(Debug)]
pub struct PendingDecisionTracker {
    tail: AnsiPlainTextTail,
    stable: Duration,
    candidate: Option<(PendingDecision, Instant)>,
    reported: Option<PendingDecision>,
}

impl Default for PendingDecisionTracker {
    fn default() -> Self {
        Self::with_stable(STABLE_WINDOW)
    }
}

impl PendingDecisionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stable(stable: Duration) -> Self {
        Self {
            tail: AnsiPlainTextTail::new(),
            stable,
            candidate: None,
            reported: None,
        }
    }

    /// 喂 PTY 原始字节（进字节尾窗）。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.tail.feed(bytes);
    }

    /// 当前正在等的那个（`None` = 没在等）。
    pub fn pending(&self) -> Option<&PendingDecision> {
        self.reported.as_ref()
    }

    /// 定期调用。`screen` = 渲染完的那一屏（[`crate::Screen::screen_rows`]）；
    /// `None` = 拿不到画面，只看字节尾窗。
    pub fn poll(&mut self, now: Instant, screen: Option<&[String]>) -> Option<DecisionEvent> {
        let seen: Vec<PendingDecision> = [
            screen.and_then(blocking_dialog),
            parse_menu(self.tail.tail()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let sticky = self
            .reported
            .as_ref()
            .or(self.candidate.as_ref().map(|(d, _)| d))
            .map(PendingDecision::fingerprint);
        let chosen = seen
            .iter()
            .find(|d| Some(d.fingerprint()) == sticky)
            .or(seen.first())
            .cloned();

        let Some(decision) = chosen else {
            self.candidate = None;
            return self.reported.take().map(|_| DecisionEvent::Cleared);
        };
        let since = match &self.candidate {
            Some((current, since)) if current.fingerprint() == decision.fingerprint() => *since,
            _ => {
                self.candidate = Some((decision, now));
                return None;
            }
        };
        if now.saturating_duration_since(since) < self.stable {
            return None;
        }
        if self.reported.as_ref().map(PendingDecision::fingerprint) == Some(decision.fingerprint())
        {
            return None;
        }
        self.reported = Some(decision.clone());
        Some(DecisionEvent::Appeared(decision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission_menu() -> String {
        [
            "╭──────────────────────────────────────────────╮",
            "│ Bash command                                 │",
            "│                                              │",
            "│   rm -rf build                               │",
            "│                                              │",
            "│ Do you want to proceed?                      │",
            "│ ❯ 1. Yes                                     │",
            "│   2. No, and tell Claude what to do (esc)    │",
            "╰──────────────────────────────────────────────╯",
        ]
        .join("\n")
    }

    fn unnumbered_trust_rows() -> Vec<String> {
        [
            "Quick safety check: Is this a project you created or one you trust?",
            "",
            "Security guide",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn menu_must_be_stable_before_firing() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        assert_eq!(t.poll(t0, None), None);
        assert_eq!(t.poll(t0 + secs(2.0), None), None);
        match t.poll(t0 + secs(3.1), None) {
            Some(DecisionEvent::Appeared(d)) => assert_eq!(d.prompt, "Do you want to proceed?"),
            other => panic!("稳定超过窗口后应报出待决策，实际 {other:?}"),
        }
        assert_eq!(t.pending().map(|d| d.options.len()), Some(2));
    }

    #[test]
    fn redraw_of_same_menu_does_not_reset_the_clock() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        assert_eq!(t.poll(t0, None), None);
        t.feed(format!("\n{}", permission_menu()).as_bytes());
        assert_eq!(t.poll(t0 + secs(1.0), None), None);
        assert!(matches!(
            t.poll(t0 + secs(3.1), None),
            Some(DecisionEvent::Appeared(_))
        ));
    }

    #[test]
    fn fires_only_once_per_menu() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert!(t.poll(t0 + secs(4.0), None).is_some());
        assert_eq!(t.poll(t0 + secs(5.0), None), None);
        assert_eq!(t.poll(t0 + secs(60.0), None), None);
    }

    #[test]
    fn clears_when_menu_goes_away() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert!(t.poll(t0 + secs(4.0), None).is_some());
        t.feed(
            "\n⏺ Bash(rm -rf build)\n  ⎿  (No content)\n⏺ 删好了，继续。\n继续读文件…\n".as_bytes(),
        );
        assert_eq!(t.poll(t0 + secs(5.0), None), Some(DecisionEvent::Cleared));
        assert_eq!(t.poll(t0 + secs(6.0), None), None, "清过一次就别再清");
        assert_eq!(t.pending(), None);
    }

    #[test]
    fn same_menu_fires_again_after_being_cleared() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(now, None);
        now += secs(4.0);
        assert!(t.poll(now, None).is_some());
        t.feed("\n⏺ 干完了，继续下一步，正在读文件…\n再读一个文件…\n还在读…\n".as_bytes());
        now += secs(1.0);
        assert_eq!(t.poll(now, None), Some(DecisionEvent::Cleared));
        t.feed(format!("\n{}", permission_menu()).as_bytes());
        now += secs(1.0);
        assert_eq!(t.poll(now, None), None);
        now += secs(4.0);
        assert!(matches!(
            t.poll(now, None),
            Some(DecisionEvent::Appeared(_))
        ));
    }

    #[test]
    fn streaming_output_never_fires() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        for i in 0..10 {
            t.feed(format!("⏺ 正在处理第 {i} 个文件…\n").as_bytes());
            now += secs(1.0);
            assert_eq!(t.poll(now, None), None);
        }
    }

    #[test]
    fn screen_source_fires_when_byte_tail_is_blind() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(b"\x1b[2GQuick\x1b[8Gsafety\x1b[15Gcheck");
        let screen = unnumbered_trust_rows();
        assert_eq!(t.poll(t0, Some(&screen)), None);
        match t.poll(t0 + STABLE_WINDOW + secs(0.1), Some(&screen)) {
            Some(DecisionEvent::Appeared(d)) => {
                assert_eq!(d.options, ["No, exit", "Yes, I trust this folder"]);
                assert!(!d.numbered);
            }
            other => panic!("屏幕上稳稳挂着信任框却没报，实际 {other:?}"),
        }
    }

    #[test]
    fn screen_source_clears_when_dialog_leaves_the_screen() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        let screen = unnumbered_trust_rows();
        t.poll(now, Some(&screen));
        now += STABLE_WINDOW + secs(0.1);
        assert!(t.poll(now, Some(&screen)).is_some());
        now += secs(1.0);
        let after = vec![
            "⏺ 好的，开始干活了。".to_string(),
            "正在读文件…".to_string(),
        ];
        assert_eq!(t.poll(now, Some(&after)), Some(DecisionEvent::Cleared));
    }

    #[test]
    fn two_sources_do_not_fight_or_flicker() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        t.feed(permission_menu().as_bytes());
        let screen: Vec<String> = permission_menu().split('\n').map(str::to_string).collect();
        t.poll(now, Some(&screen));
        now += STABLE_WINDOW + secs(0.1);
        assert!(matches!(
            t.poll(now, Some(&screen)),
            Some(DecisionEvent::Appeared(_))
        ));
        for _ in 0..10 {
            now += secs(1.0);
            assert_eq!(t.poll(now, Some(&screen)), None, "同一个菜单不许反复报");
        }
        now += secs(1.0);
        let half = vec!["…重绘中…".to_string()];
        assert_eq!(t.poll(now, Some(&half)), None, "半成品画面不该把状态清掉");
    }

    #[test]
    fn clock_going_backwards_does_not_fire_early() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now() + secs(10.0);
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert_eq!(t.poll(t0 - secs(5.0), None), None);
        assert!(t.poll(t0 + STABLE_WINDOW, None).is_some());
    }

    /// 粘住：字节尾窗先认出措辞 B 的菜单并已报出，之后屏幕源也认出同一个菜单但措辞 A
    /// （指纹不同）。选择必须继续粘在 B 上——不能因为屏幕源排在前面就换成 A、重起一轮计时、
    /// 再报第二次 `Appeared`。
    #[test]
    fn sticky_fingerprint_survives_a_second_source_with_other_wording() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        t.feed(permission_menu().as_bytes());
        let screen_a: Vec<String> = permission_menu()
            .replace(
                "No, and tell Claude what to do (esc)",
                "No, tell Claude (esc)",
            )
            .split('\n')
            .map(str::to_string)
            .collect();
        assert_ne!(
            blocking_dialog(&screen_a).unwrap().fingerprint(),
            parse_menu(&permission_menu()).unwrap().fingerprint(),
            "前提：两条来源的措辞（指纹）确实不同"
        );

        // 第一拍只有字节尾窗；稳住 STABLE_WINDOW 后报出 B。
        let mut appeared = Vec::new();
        let mut events =
            |now: Instant, screen: Option<&[String]>, t: &mut PendingDecisionTracker| {
                if let Some(e) = t.poll(now, screen) {
                    appeared.push(e);
                }
            };
        events(now, None, &mut t);
        now += STABLE_WINDOW + secs(0.1);
        events(now, None, &mut t);
        // 之后屏幕源也来了（措辞 A），两条来源都稳稳挂着，至少 10 拍。
        for _ in 0..12 {
            now += secs(1.0);
            events(now, Some(&screen_a), &mut t);
        }
        assert_eq!(appeared.len(), 1, "只该报一次，实际 {appeared:?}");
        match &appeared[0] {
            DecisionEvent::Appeared(d) => {
                assert_eq!(d.options[1], "No, and tell Claude what to do (esc)");
            }
            other => panic!("应是 Appeared(B)，实际 {other:?}"),
        }
        assert_eq!(
            t.pending().map(|d| d.options[1].as_str()),
            Some("No, and tell Claude what to do (esc)")
        );
    }
}
