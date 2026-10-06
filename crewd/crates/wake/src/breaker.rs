//! 唤醒环路熔断（spec §22.7 Q5，人 2026-10-06 拍板）：**不做任何全机 / 机组总量限流**；只截「短时间内反复叫醒、
//! 却没有新产出」的环路——程序错误让同一 session 被反复叫醒，或两个 session 互相叫醒打转。
//!
//! 两级：
//! - 边（来源 → 目标）：同一来源对同一目标，目标上次产出之后、窗口内已放行 `edge_max` 次，再来就熔断这条边。
//! - 目标：系统 / 目标自己定时这类非 session 来源对同一目标合计已放行 `target_max` 次，再来就熔断目标的这类唤醒。
//!
//! 产出（清零目标的计数）：工具调用、工作项变化（IO 层调 [`LoopBreaker::progress`]）；目标发了一条消息、且它叫醒的
//! 人里不含正在叫它的来源（[`LoopBreaker::on_post`]——回叫不算，否则互相 @ 的环路每轮都「有新消息」，永远截不住）。
//! 人发起的唤醒（来源 `human`）永不计数、永不被挡。熔断只能由人解（[`LoopBreaker::recover`]）。
use std::collections::{BTreeMap, HashMap};

use crate::model::{Latch, LatchScope, MIN, Ms, WakeKind};
use crate::plan::Sender;

/// 判据的唯一一处常量（§22.7 Q5「阈值与判据写成一处常量」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopLimits {
    /// 只看这段时间里的投递。
    pub window_ms: i64,
    /// 同一来源对同一目标、目标没有新产出时最多放行几次。
    pub edge_max: usize,
    /// 非 session 来源（系统、目标自己定时）对同一目标合计最多放行几次。
    pub target_max: usize,
}

pub const LOOP_LIMITS: LoopLimits = LoopLimits {
    window_ms: 10 * MIN,
    edge_max: 8,
    target_max: 8,
};

pub const HUMAN: &str = "human";
pub const SYSTEM: &str = "system";

/// 一条唤醒是谁引起的。消息类 = 发送者；定时 = 目标自己；问被回答 / 重启恢复 = 人（都是人点的头）；其余 = 系统。
pub fn origin_of(kind: WakeKind, sender: Option<&Sender>, target: &str) -> String {
    match kind {
        WakeKind::Message => sender.map_or(SYSTEM, Sender::key).to_string(),
        WakeKind::Timer => target.to_string(),
        WakeKind::AskAnswer | WakeKind::Restart => HUMAN.to_string(),
        WakeKind::System | WakeKind::Checkpoint | WakeKind::StopCheck => SYSTEM.to_string(),
    }
}

/// 边的台账键。与 crewd-collab 的 `wakes::latches::Scope::Edge` 一致：`from→to`（U+2192）；
/// 本 crate 不依赖 collab，所以格式在这里写一遍、由 `edge_id_uses_the_collab_ledger_separator` 钉住。
pub const EDGE_SEP: char = '\u{2192}';

pub fn edge_id(origin: &str, target: &str) -> String {
    format!("{origin}{EDGE_SEP}{target}")
}

fn is_other_session(origin: &str, target: &str) -> bool {
    origin != HUMAN && origin != SYSTEM && origin != target
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// 已熔断：留在 pending，直到人恢复。
    Held(Latch),
    /// 这一次触发了熔断：调用方落库（`wake_latches`）并告诉人与群主；这一次不投。
    Tripped(Latch),
}

#[derive(Debug)]
pub struct LoopBreaker {
    limits: LoopLimits,
    /// 见过的最晚时间（墙钟回跳不让窗口变宽）。
    clock: Ms,
    /// 目标 → 上次产出之后放行的 (时间, 来源)。
    streak: HashMap<String, Vec<(Ms, String)>>,
    latches: BTreeMap<(LatchScope, String), Latch>,
}

impl LoopBreaker {
    pub fn new(limits: LoopLimits, latches: Vec<Latch>) -> Self {
        Self {
            limits,
            clock: Ms(i64::MIN),
            streak: HashMap::new(),
            latches: latches
                .into_iter()
                .map(|l| ((l.scope, l.id.clone()), l))
                .collect(),
        }
    }

    /// 这条来源对这个目标此刻是否被熔断挡着（人永不）。
    pub fn held(&self, target: &str, origin: &str) -> Option<&Latch> {
        if origin == HUMAN {
            return None;
        }
        if let Some(l) = self
            .latches
            .get(&(LatchScope::Edge, edge_id(origin, target)))
        {
            return Some(l);
        }
        if !is_other_session(origin, target) {
            return self.latches.get(&(LatchScope::Session, target.to_string()));
        }
        None
    }

    /// 投递前问一次；`Pass` 即记下这一次。
    pub fn admit(&mut self, target: &str, origin: &str, now: Ms) -> Verdict {
        if origin == HUMAN {
            return Verdict::Pass;
        }
        if let Some(l) = self.held(target, origin) {
            return Verdict::Held(l.clone());
        }
        self.clock = self.clock.max(now);
        let (clock, window) = (self.clock, self.limits.window_ms);
        let (edge_n, target_n) = {
            let streak = self.streak.entry(target.to_string()).or_default();
            streak.retain(|(at, _)| clock.since(*at) < window);
            (
                streak.iter().filter(|(_, o)| o == origin).count(),
                streak
                    .iter()
                    .filter(|(_, o)| !is_other_session(o, target))
                    .count(),
            )
        };
        if edge_n >= self.limits.edge_max {
            return Verdict::Tripped(self.trip(
                LatchScope::Edge,
                edge_id(origin, target),
                edge_n + 1,
            ));
        }
        if !is_other_session(origin, target) && target_n >= self.limits.target_max {
            return Verdict::Tripped(self.trip(
                LatchScope::Session,
                target.to_string(),
                target_n + 1,
            ));
        }
        self.streak
            .get_mut(target)
            .expect("上面 entry 建过")
            .push((clock, origin.to_string()));
        Verdict::Pass
    }

    fn trip(&mut self, scope: LatchScope, id: String, count: usize) -> Latch {
        let latch = Latch {
            scope,
            id: id.clone(),
            reason: format!("no-progress:{count}/{}min", self.limits.window_ms / MIN),
            at: self.clock,
        };
        self.latches.insert((scope, id), latch.clone());
        latch
    }

    /// 目标有了产出（工具调用、工作项变化）。
    pub fn progress(&mut self, target: &str) {
        self.streak.remove(target);
    }

    /// `poster` 发了一条消息、叫醒了 `woke`。spec §22.4 / §22.7：叫醒了「正在叫它的来源以外的人」才算产出
    /// （回话给叫醒你的人不算——否则互相 @ 的环路每轮都「有新消息」，永远截不住）。
    /// 逐字按 spec：`woke` 里只要有一个不是 poster 当前计数里的来源，就算（回叫的同时叫了别人也算）；
    /// `woke` 为空 = 没有「来源以外的人」，不算。
    pub fn on_post(&mut self, poster: &str, woke: &[String]) {
        let reaches_beyond_callers = self
            .streak
            .get(poster)
            .is_none_or(|s| woke.iter().any(|w| !s.iter().any(|(_, o)| o == w)));
        if reaches_beyond_callers {
            self.progress(poster);
        }
    }

    /// 人解开一条熔断；这条环路的计数一并清掉（不然恢复后第一次就又熔断）。
    pub fn recover(&mut self, scope: LatchScope, id: &str) -> bool {
        if self.latches.remove(&(scope, id.to_string())).is_none() {
            return false;
        }
        match scope {
            LatchScope::Session => {
                if let Some(s) = self.streak.get_mut(id) {
                    s.retain(|(_, o)| is_other_session(o, id));
                }
            }
            LatchScope::Edge => {
                if let Some((origin, target)) = id.split_once(EDGE_SEP)
                    && let Some(s) = self.streak.get_mut(target)
                {
                    s.retain(|(_, o)| o != origin);
                }
            }
        }
        true
    }

    pub fn latches(&self) -> Vec<Latch> {
        self.latches.values().cloned().collect()
    }

    pub fn streak_len(&self, target: &str) -> usize {
        self.streak.get(target).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SEC;

    const T0: Ms = Ms(1_000_000_000);

    fn lb() -> LoopBreaker {
        LoopBreaker::new(LOOP_LIMITS, vec![])
    }

    fn round(i: usize) -> Ms {
        T0.add(i as i64 * 30 * SEC)
    }

    #[test]
    fn ping_pong_between_two_sessions_is_cut_on_one_edge() {
        let mut b = lb();
        let mut tripped = Vec::new();
        'rounds: for i in 0..20 {
            // a 叫醒 b，b 回 @a（回叫，不算产出）；b 叫醒 a，a 回 @b ……
            for (target, origin) in [("b", "a"), ("a", "b")] {
                match b.admit(target, origin, round(i)) {
                    Verdict::Pass => b.on_post(target, &[origin.to_string()]),
                    Verdict::Tripped(l) => {
                        tripped.push(l);
                        break 'rounds;
                    }
                    Verdict::Held(l) => panic!("没熔断过却 Held：{l:?}"),
                }
            }
        }
        assert_eq!(tripped.len(), 1);
        assert_eq!(
            (tripped[0].scope, tripped[0].id.as_str()),
            (LatchScope::Edge, "a→b")
        );
        assert_eq!(tripped[0].at, round(LOOP_LIMITS.edge_max));
        assert!(matches!(b.admit("b", "a", round(21)), Verdict::Held(_)));
        assert_eq!(b.admit("b", HUMAN, round(21)), Verdict::Pass, "人照样送到");
        assert_eq!(
            b.admit("b", "c", round(21)),
            Verdict::Pass,
            "别的来源不受影响"
        );
    }

    #[test]
    fn progress_resets_the_streak() {
        let mut b = lb();
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("b", "a", round(i)), Verdict::Pass);
        }
        b.progress("b");
        assert_eq!(b.streak_len("b"), 0);
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("b", "a", round(10 + i)), Verdict::Pass, "第 {i} 次");
        }
        assert!(matches!(b.admit("b", "a", round(19)), Verdict::Tripped(_)));
    }

    #[test]
    fn posting_to_someone_else_is_progress_replying_back_is_not() {
        // spec §22.4：T 发的消息叫醒了「正在叫它的来源以外的人」才算产出；回话给叫醒你的人不算。
        let mut b = lb();
        for i in 0..3 {
            b.admit("b", "a", round(i));
        }
        b.on_post("b", &["a".to_string()]);
        assert_eq!(b.streak_len("b"), 3, "回给叫它的人不算产出");
        b.on_post("b", &[]);
        assert_eq!(
            b.streak_len("b"),
            3,
            "谁也没叫醒 = 没有「来源以外的人」，不算产出（C13，按 spec 字面）"
        );
        b.on_post("b", &["a".to_string(), "c".to_string()]);
        assert_eq!(
            b.streak_len("b"),
            0,
            "叫醒了来源以外的 c = 算产出，哪怕同时回了 a（spec 字面）"
        );
        for i in 0..3 {
            b.admit("b", "a", round(10 + i));
        }
        b.on_post("b", &["c".to_string()]);
        assert_eq!(b.streak_len("b"), 0, "只叫了别人 = 产出");
    }

    #[test]
    fn reply_back_is_judged_against_every_origin_in_the_streak() {
        // b 被 a 和 x 都叫过：回 x 也是回叫，不算；只有叫到这两个之外的人才算。
        let mut b = lb();
        b.admit("b", "a", round(0));
        b.admit("b", "x", round(1));
        b.on_post("b", &["x".to_string()]);
        assert_eq!(b.streak_len("b"), 2);
        b.on_post("b", &["a".to_string(), "x".to_string()]);
        assert_eq!(b.streak_len("b"), 2);
        b.on_post("b", &["c".to_string()]);
        assert_eq!(b.streak_len("b"), 0);
    }

    #[test]
    fn slow_repeats_outside_the_window_never_trip() {
        let mut b = lb();
        for i in 0..50 {
            assert_eq!(
                b.admit("b", "a", T0.add(i * 2 * MIN)),
                Verdict::Pass,
                "第 {i} 次"
            );
        }
    }

    #[test]
    fn self_timer_loop_trips_its_own_edge() {
        let mut b = lb();
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("t", "t", round(i)), Verdict::Pass);
        }
        let Verdict::Tripped(l) = b.admit("t", "t", round(9)) else {
            panic!("该熔断")
        };
        assert_eq!((l.scope, l.id.as_str()), (LatchScope::Edge, "t→t"));
    }

    #[test]
    fn system_and_self_wakes_share_the_target_budget() {
        let mut b = lb();
        for i in 0..4 {
            assert_eq!(b.admit("t", SYSTEM, round(2 * i)), Verdict::Pass);
            assert_eq!(b.admit("t", "t", round(2 * i + 1)), Verdict::Pass);
        }
        let Verdict::Tripped(l) = b.admit("t", SYSTEM, round(9)) else {
            panic!("该熔断目标")
        };
        assert_eq!((l.scope, l.id.as_str()), (LatchScope::Session, "t"));
        assert!(matches!(b.admit("t", "t", round(10)), Verdict::Held(_)));
        assert_eq!(
            b.admit("t", "x", round(10)),
            Verdict::Pass,
            "别的 session 叫它不受目标级熔断影响"
        );
    }

    #[test]
    fn human_wakes_never_count_and_are_never_held() {
        let latch = Latch {
            scope: LatchScope::Session,
            id: "t".into(),
            reason: "x".into(),
            at: T0,
        };
        let mut b = LoopBreaker::new(LOOP_LIMITS, vec![latch]);
        for i in 0..100 {
            assert_eq!(b.admit("t", HUMAN, T0.add(i)), Verdict::Pass);
        }
        assert_eq!(b.streak_len("t"), 0);
        assert!(b.held("t", HUMAN).is_none());
    }

    #[test]
    fn thirty_sessions_working_normally_are_never_tripped() {
        let workers: Vec<String> = (1..=30).map(|i| format!("w{i}")).collect();
        for tools_reported in [true, false] {
            let mut b = lb();
            // 10 分钟里 3 轮：机长给每个 worker 派一件事，worker 干活、回报 @机长，机长接着派下一件。
            // `tools_reported = false`：M3b 接上工具调用信号之前，只有消息这一种产出信号。
            for r in 0..3 {
                for (i, w) in workers.iter().enumerate() {
                    let now = T0.add((r * 30 + i) as i64 * 6 * SEC);
                    assert_eq!(b.admit(w, "cap", now), Verdict::Pass, "{w} 第 {r} 轮");
                    if tools_reported {
                        b.progress(w);
                    }
                    b.on_post(w, &["cap".to_string()]);
                    assert_eq!(b.admit("cap", w, now), Verdict::Pass, "cap ← {w} 第 {r} 轮");
                    b.on_post("cap", std::slice::from_ref(w));
                }
            }
            assert!(b.latches().is_empty());
        }
    }

    #[test]
    fn loaded_latches_hold_until_recovered() {
        let latch = Latch {
            scope: LatchScope::Edge,
            id: "a→b".into(),
            reason: "x".into(),
            at: T0,
        };
        let mut b = LoopBreaker::new(LOOP_LIMITS, vec![latch]);
        assert!(matches!(b.admit("b", "a", T0), Verdict::Held(_)));
        assert!(b.recover(LatchScope::Edge, "a→b"));
        assert!(!b.recover(LatchScope::Edge, "a→b"), "第二次没东西可解");
        assert_eq!(b.admit("b", "a", T0), Verdict::Pass);
    }

    #[test]
    fn recovering_an_edge_forgets_that_edges_streak() {
        let mut b = lb();
        for i in 0..=LOOP_LIMITS.edge_max {
            b.admit("b", "a", round(i));
        }
        assert!(b.recover(LatchScope::Edge, "a→b"));
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(
                b.admit("b", "a", round(10 + i)),
                Verdict::Pass,
                "恢复后重新计数，第 {i} 次"
            );
        }
    }

    #[test]
    fn origin_of_maps_each_kind() {
        let w1 = Sender::Session {
            id: "w1".into(),
            captain: false,
        };
        assert_eq!(origin_of(WakeKind::Message, Some(&w1), "t"), "w1");
        let cap = Sender::Session {
            id: "cap".into(),
            captain: true,
        };
        assert_eq!(
            origin_of(WakeKind::Message, Some(&cap), "t"),
            "cap",
            "机长也是按 session id 记边"
        );
        assert_eq!(
            origin_of(WakeKind::Message, Some(&Sender::System), "t"),
            SYSTEM
        );
        assert_eq!(
            origin_of(WakeKind::Message, Some(&Sender::Human), "t"),
            HUMAN
        );
        assert_eq!(origin_of(WakeKind::Message, None, "t"), SYSTEM);
        assert_eq!(origin_of(WakeKind::Timer, None, "t"), "t");
        assert_eq!(origin_of(WakeKind::AskAnswer, None, "t"), HUMAN);
        assert_eq!(origin_of(WakeKind::Restart, None, "t"), HUMAN);
        for k in [WakeKind::System, WakeKind::Checkpoint, WakeKind::StopCheck] {
            assert_eq!(origin_of(k, None, "t"), SYSTEM);
        }
    }

    #[test]
    fn edge_id_uses_the_collab_ledger_separator() {
        // 与 crewd-collab 台账（wakes::latches::Scope::Edge）的键一致：from→to（U+2192）。
        // 本 crate 不依赖 collab，格式靠这条钉住。
        assert_eq!(edge_id("w-a1", "w-b1"), "w-a1\u{2192}w-b1");
    }
}
