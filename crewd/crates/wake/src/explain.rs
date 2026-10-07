//! 「为什么此刻没叫醒它」（C25 / R9）：**一个**纯函数，编排的投递（`try_deliver`）拿它做判定、排障的 `wake.list`
//! 拿它给人看——两处说法不可能不一样。判定次序就是投递的次序：
//! 停机 → 正在起 → 没到点 → 在跑的此刻能不能投 → 熔断 → 优先级档 → 拉起闸。
//!
//! 副作用（额度唤醒到点时先清额度类 health、错开时刻记进定时器、等重启答复的 session 被人 @ 后移出名单）不在这里：
//! 调用方按同一份事实做。事实（[`TargetFacts`]）由调用方采集；`gate` 由调用方用 `LaunchGate::check`（纯函数）算好。
use crate::model::{Latch, LatchScope, Ms, Priority, QueuedWake};
use crate::policy::Gate;
use crate::queue::is_due;

/// 挡投递的 health（在跑的目标身上）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthBlock {
    /// 额度 / 限流：只有额度重置唤醒到点才放行（先清 health 再投）。
    Quota,
    /// 要人登录。
    Auth,
    /// 起来了但不能用（起不来）。
    LaunchFailed,
}

/// 一个目标 session 此刻的事实（调用方采集；全是值）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetFacts {
    /// 有进程在跑。
    pub running: bool,
    /// 正在拉起（结果还没回来）。
    pub starting: bool,
    /// 有一笔投递还没回执。
    pub inflight: bool,
    pub working: bool,
    /// 卡在待决策框上。
    pub decision: bool,
    /// 正在被停掉：人停、crewd 叫停、休眠到期限按过 Esc。
    pub stopping: bool,
    /// crewd 在停机 / 休眠：不投、不拉起。
    pub shutting_down: bool,
    /// 它的机组已被移除：不会被拉起（下一次尝试把这几行作废）。
    pub crew_removed: bool,
    pub health: Option<HealthBlock>,
    /// 有一条额度重置唤醒已到点。
    pub quota_due: bool,
    /// 有一条额度重置唤醒还排着（不论到没到点）。
    pub quota_pending: bool,
    /// 在等人答重启询问（§22.7 Q1）。
    pub held_for_restart: bool,
    /// 询问开出之后人 @ 了它（C4：只恢复它）。
    pub human_called_after_watermark: bool,
    /// 拉起闸此刻的答复（`LaunchGate::check`，人的唤醒 `bypass_cap`）。只对没在跑的目标有意义。
    pub gate: Gate,
    /// 可选上限（`AtCap` 时报给人看）。
    pub max_live: Option<usize>,
}

impl Default for TargetFacts {
    fn default() -> Self {
        Self {
            running: false,
            starting: false,
            inflight: false,
            working: false,
            decision: false,
            stopping: false,
            shutting_down: false,
            crew_removed: false,
            health: None,
            quota_due: false,
            quota_pending: false,
            held_for_restart: false,
            human_called_after_watermark: false,
            gate: Gate::Go,
            max_live: None,
        }
    }
}

/// 没叫醒的原因。`code()` 是线上的词（`wake.list` 的 `blocker.code`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    ShuttingDown,
    /// 机组已被移除：不拉起，下一次尝试把这几行作废。
    CrewRemoved,
    Starting,
    NotDue {
        until: Ms,
    },
    /// 环路熔断挡着（只有人能解，`loop.recover`）。
    Held {
        scope: LatchScope,
        id: String,
    },
    /// 有更高一档的到期唤醒先走（一次只投最高一档）。
    BehindHigherPriority,
    Inflight,
    Working,
    Decision,
    Stopping,
    /// 额度类 health 在身、额度唤醒还没到点。
    Quota,
    AuthRequired,
    LaunchFailed,
    /// 在等人答重启询问；人在询问之后 @ 它才拉起。
    AwaitingRestartAnswer,
    /// 额度唤醒还没到点：自动唤醒不把它拉起来撞墙（人的唤醒照样拉）。
    QuotaNotReset,
    /// 可选并发上限满了（只挡自动拉起）。
    AtCap {
        max: usize,
    },
    /// 离上一次拉起太近，这一刻之后再试。
    Stagger {
        until: Ms,
    },
}

impl Blocker {
    pub fn code(&self) -> &'static str {
        match self {
            Blocker::ShuttingDown => "shutting_down",
            Blocker::CrewRemoved => "crew_removed",
            Blocker::Starting => "starting",
            Blocker::NotDue { .. } => "not_due",
            Blocker::Held { .. } => "held",
            Blocker::BehindHigherPriority => "behind_higher_priority",
            Blocker::Inflight => "inflight",
            Blocker::Working => "working",
            Blocker::Decision => "decision",
            Blocker::Stopping => "stopping",
            Blocker::Quota => "quota",
            Blocker::AuthRequired => "auth_required",
            Blocker::LaunchFailed => "launch_failed",
            Blocker::AwaitingRestartAnswer => "awaiting_restart_answer",
            Blocker::QuotaNotReset => "quota_not_reset",
            Blocker::AtCap { .. } => "at_cap",
            Blocker::Stagger { .. } => "stagger",
        }
    }

    /// 拉起闸那一段的原因（只对没在跑的目标、过了熔断与优先级档之后才出现）。投递按「先选批、再判拉起」的次序，
    /// 选批时这几种算「可以选进批」。
    pub fn is_launch_gate(&self) -> bool {
        matches!(
            self,
            Blocker::AwaitingRestartAnswer
                | Blocker::QuotaNotReset
                | Blocker::AtCap { .. }
                | Blocker::Stagger { .. }
                | Blocker::CrewRemoved
        )
    }

    /// 带时刻的原因的那一刻（`not_due` / `stagger`）。
    pub fn until(&self) -> Option<Ms> {
        match self {
            Blocker::NotDue { until } | Blocker::Stagger { until } => Some(*until),
            _ => None,
        }
    }
}

/// 在跑的目标此刻能不能往里写（readiness 的唯一定义）：没在跑的 = `None`（交给拉起闸那一段）。额度类 health 在身时，
/// 额度唤醒到点就不挡（调用方先清 health 再投）。`why_not_woken` 与 Core 的「先清额度 health」都只用它。
pub fn live_blocker(t: &TargetFacts) -> Option<Blocker> {
    if !t.running {
        return None;
    }
    if t.inflight {
        return Some(Blocker::Inflight);
    }
    if t.working {
        return Some(Blocker::Working);
    }
    if t.decision {
        return Some(Blocker::Decision);
    }
    if t.stopping {
        return Some(Blocker::Stopping);
    }
    match t.health {
        Some(HealthBlock::Quota) if !t.quota_due => Some(Blocker::Quota),
        Some(HealthBlock::Auth) => Some(Blocker::AuthRequired),
        Some(HealthBlock::LaunchFailed) => Some(Blocker::LaunchFailed),
        _ => None,
    }
}

/// 这一条唤醒此刻为什么没被叫醒；`None` = 它此刻就会出去（或被当作已注入直接记 delivered）。
/// `top_due` = 这个目标到期、没被熔断挡着的唤醒里最高的一档；`held` = 熔断里挡着它的那一条（调用方查 `LoopBreaker::held`）。
pub fn why_not_woken(
    w: &QueuedWake,
    top_due: Option<Priority>,
    t: &TargetFacts,
    held: Option<&Latch>,
    now: Ms,
) -> Option<Blocker> {
    if t.shutting_down {
        return Some(Blocker::ShuttingDown);
    }
    if t.starting {
        return Some(Blocker::Starting);
    }
    if !is_due(w, now) {
        return Some(Blocker::NotDue {
            until: w.not_before,
        });
    }
    if let Some(b) = live_blocker(t) {
        return Some(b);
    }
    if let Some(l) = held {
        return Some(Blocker::Held {
            scope: l.scope,
            id: l.id.clone(),
        });
    }
    if top_due.is_some_and(|top| w.priority < top) {
        return Some(Blocker::BehindHigherPriority);
    }
    if !t.running {
        let human = w.priority == Priority::Human;
        if t.held_for_restart && !t.human_called_after_watermark {
            return Some(Blocker::AwaitingRestartAnswer);
        }
        if !human && t.quota_pending && !t.quota_due {
            return Some(Blocker::QuotaNotReset);
        }
        match t.gate {
            Gate::AtCap => {
                return Some(Blocker::AtCap {
                    max: t.max_live.unwrap_or(0),
                });
            }
            Gate::WaitUntil(until) => return Some(Blocker::Stagger { until }),
            Gate::Go => {}
        }
        if t.crew_removed {
            return Some(Blocker::CrewRemoved);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{WakeKind, WakeState};

    const NOW: Ms = Ms(1_000_000);

    fn wake(priority: Priority, not_before: Ms) -> QueuedWake {
        QueuedWake {
            id: 1,
            session: "w1".into(),
            kind: WakeKind::Message,
            source: "msg:m1".into(),
            priority,
            not_before,
            note: None,
            state: WakeState::Pending,
            attempts: 0,
            created_at: Ms(0),
            origin: "cap".into(),
            msg: None,
            last_error: None,
        }
    }

    fn due(p: Priority) -> QueuedWake {
        wake(p, NOW)
    }

    fn running() -> TargetFacts {
        TargetFacts {
            running: true,
            ..TargetFacts::default()
        }
    }

    fn stopped() -> TargetFacts {
        TargetFacts::default()
    }

    fn latch() -> Latch {
        Latch {
            scope: LatchScope::Edge,
            id: "cap→w1".into(),
            reason: "no-progress".into(),
            at: Ms(0),
        }
    }

    fn why(w: &QueuedWake, t: &TargetFacts) -> Option<Blocker> {
        why_not_woken(w, Some(w.priority), t, None, NOW)
    }

    #[test]
    fn a_due_wake_to_an_idle_running_target_goes_out() {
        assert_eq!(why(&due(Priority::Auto), &running()), None);
        assert_eq!(why(&due(Priority::Auto), &stopped()), None);
    }

    #[test]
    fn shutting_down_blocks_everything_first() {
        let t = TargetFacts {
            shutting_down: true,
            starting: true,
            ..running()
        };
        assert_eq!(
            why(&wake(Priority::Human, Ms(NOW.0 + 1)), &t),
            Some(Blocker::ShuttingDown)
        );
    }

    #[test]
    fn starting_comes_before_due() {
        let t = TargetFacts {
            starting: true,
            ..stopped()
        };
        assert_eq!(
            why(&wake(Priority::Auto, Ms(NOW.0 + 1)), &t),
            Some(Blocker::Starting)
        );
    }

    #[test]
    fn a_future_wake_is_not_due_until_its_time_even_when_the_target_is_busy() {
        let later = Ms(NOW.0 + 5_000);
        let t = TargetFacts {
            working: true,
            ..running()
        };
        assert_eq!(
            why(&wake(Priority::Auto, later), &t),
            Some(Blocker::NotDue { until: later })
        );
        assert_eq!(Blocker::NotDue { until: later }.until(), Some(later));
    }

    #[test]
    fn inflight_working_decision_and_stopping_hold_a_running_target_in_that_order() {
        let all = TargetFacts {
            inflight: true,
            working: true,
            decision: true,
            stopping: true,
            ..running()
        };
        let w = due(Priority::Human);
        assert_eq!(why(&w, &all), Some(Blocker::Inflight));
        let t = TargetFacts {
            inflight: false,
            ..all.clone()
        };
        assert_eq!(why(&w, &t), Some(Blocker::Working));
        let t = TargetFacts {
            inflight: false,
            working: false,
            ..all.clone()
        };
        assert_eq!(why(&w, &t), Some(Blocker::Decision));
        let t = TargetFacts {
            stopping: true,
            ..running()
        };
        assert_eq!(why(&w, &t), Some(Blocker::Stopping));
    }

    #[test]
    fn quota_health_holds_until_the_quota_wake_is_due() {
        let t = TargetFacts {
            health: Some(HealthBlock::Quota),
            quota_pending: true,
            ..running()
        };
        assert_eq!(why(&due(Priority::Human), &t), Some(Blocker::Quota));
        let t = TargetFacts {
            quota_due: true,
            ..t
        };
        assert_eq!(
            why(&due(Priority::Human), &t),
            None,
            "到点了：先清 health 再投"
        );
    }

    #[test]
    fn auth_and_launch_failed_health_hold_a_running_target() {
        let t = TargetFacts {
            health: Some(HealthBlock::Auth),
            ..running()
        };
        assert_eq!(why(&due(Priority::Human), &t), Some(Blocker::AuthRequired));
        let t = TargetFacts {
            health: Some(HealthBlock::LaunchFailed),
            ..running()
        };
        assert_eq!(why(&due(Priority::Human), &t), Some(Blocker::LaunchFailed));
    }

    #[test]
    fn live_blocker_is_the_readiness_rule_on_its_own() {
        assert_eq!(live_blocker(&running()), None);
        assert_eq!(live_blocker(&stopped()), None, "没在跑的不归它管");
        let quota = TargetFacts {
            health: Some(HealthBlock::Quota),
            ..running()
        };
        assert_eq!(live_blocker(&quota), Some(Blocker::Quota));
        let due = TargetFacts {
            quota_due: true,
            ..quota.clone()
        };
        assert_eq!(live_blocker(&due), None, "额度唤醒到点：先清 health 再投");
        let busy = TargetFacts {
            working: true,
            ..due
        };
        assert_eq!(live_blocker(&busy), Some(Blocker::Working));
    }

    #[test]
    fn live_flags_do_not_apply_to_a_target_that_is_not_running() {
        let t = TargetFacts {
            working: true,
            health: Some(HealthBlock::Auth),
            ..stopped()
        };
        assert_eq!(why(&due(Priority::Auto), &t), None);
    }

    #[test]
    fn a_latch_holds_after_live_readiness() {
        let l = latch();
        let w = due(Priority::Auto);
        assert_eq!(
            why_not_woken(&w, Some(Priority::Auto), &running(), Some(&l), NOW),
            Some(Blocker::Held {
                scope: LatchScope::Edge,
                id: "cap→w1".into()
            })
        );
        let busy = TargetFacts {
            working: true,
            ..running()
        };
        assert_eq!(
            why_not_woken(&w, Some(Priority::Auto), &busy, Some(&l), NOW),
            Some(Blocker::Working),
            "在跑的此刻能不能投，先于熔断"
        );
    }

    #[test]
    fn a_lower_tier_waits_behind_the_top_due_tier() {
        let w = due(Priority::Auto);
        assert_eq!(
            why_not_woken(&w, Some(Priority::Human), &running(), None, NOW),
            Some(Blocker::BehindHigherPriority)
        );
        assert_eq!(why_not_woken(&w, None, &running(), None, NOW), None);
    }

    #[test]
    fn a_held_session_waits_for_the_restart_answer_unless_a_human_called_it_since() {
        let t = TargetFacts {
            held_for_restart: true,
            ..stopped()
        };
        assert_eq!(
            why(&due(Priority::Human), &t),
            Some(Blocker::AwaitingRestartAnswer)
        );
        let t = TargetFacts {
            human_called_after_watermark: true,
            ..t
        };
        assert_eq!(why(&due(Priority::Human), &t), None);
    }

    #[test]
    fn automatic_wakes_do_not_relaunch_into_an_unreset_quota_but_human_ones_do() {
        let t = TargetFacts {
            quota_pending: true,
            ..stopped()
        };
        assert_eq!(why(&due(Priority::Auto), &t), Some(Blocker::QuotaNotReset));
        assert_eq!(why(&due(Priority::Human), &t), None);
        let t = TargetFacts {
            quota_due: true,
            ..t
        };
        assert_eq!(why(&due(Priority::Auto), &t), None);
    }

    #[test]
    fn the_launch_gate_reports_the_cap_and_the_stagger() {
        let t = TargetFacts {
            gate: Gate::AtCap,
            max_live: Some(3),
            ..stopped()
        };
        assert_eq!(
            why(&due(Priority::Auto), &t),
            Some(Blocker::AtCap { max: 3 })
        );
        let until = Ms(NOW.0 + 2_000);
        let t = TargetFacts {
            gate: Gate::WaitUntil(until),
            ..stopped()
        };
        let b = why(&due(Priority::Auto), &t);
        assert_eq!(b, Some(Blocker::Stagger { until }));
        assert_eq!(b.unwrap().until(), Some(until));
        let t = TargetFacts {
            gate: Gate::AtCap,
            ..running()
        };
        assert_eq!(why(&due(Priority::Auto), &t), None, "闸只管拉起");
    }

    #[test]
    fn a_removed_crew_is_reported_last_and_only_for_a_target_that_would_be_launched() {
        let t = TargetFacts {
            crew_removed: true,
            ..stopped()
        };
        assert_eq!(why(&due(Priority::Human), &t), Some(Blocker::CrewRemoved));
        let t = TargetFacts {
            crew_removed: true,
            gate: Gate::AtCap,
            max_live: Some(1),
            ..stopped()
        };
        assert_eq!(
            why(&due(Priority::Auto), &t),
            Some(Blocker::AtCap { max: 1 })
        );
    }

    #[test]
    fn launch_gate_blockers_are_exactly_the_last_stage() {
        let gates = [
            Blocker::AwaitingRestartAnswer,
            Blocker::QuotaNotReset,
            Blocker::AtCap { max: 1 },
            Blocker::Stagger { until: NOW },
            Blocker::CrewRemoved,
        ];
        for b in &gates {
            assert!(b.is_launch_gate(), "{b:?}");
        }
        for b in [
            Blocker::ShuttingDown,
            Blocker::Starting,
            Blocker::NotDue { until: NOW },
            Blocker::Held {
                scope: LatchScope::Session,
                id: "w1".into(),
            },
            Blocker::BehindHigherPriority,
            Blocker::Inflight,
            Blocker::Working,
            Blocker::Decision,
            Blocker::Stopping,
            Blocker::Quota,
            Blocker::AuthRequired,
            Blocker::LaunchFailed,
        ] {
            assert!(!b.is_launch_gate(), "{b:?}");
        }
    }

    #[test]
    fn codes_are_unique_snake_case_words() {
        let all = [
            Blocker::ShuttingDown,
            Blocker::CrewRemoved,
            Blocker::Starting,
            Blocker::NotDue { until: NOW },
            Blocker::Held {
                scope: LatchScope::Session,
                id: "w1".into(),
            },
            Blocker::BehindHigherPriority,
            Blocker::Inflight,
            Blocker::Working,
            Blocker::Decision,
            Blocker::Stopping,
            Blocker::Quota,
            Blocker::AuthRequired,
            Blocker::LaunchFailed,
            Blocker::AwaitingRestartAnswer,
            Blocker::QuotaNotReset,
            Blocker::AtCap { max: 1 },
            Blocker::Stagger { until: NOW },
        ];
        let mut codes: Vec<&str> = all.iter().map(Blocker::code).collect();
        assert!(
            codes
                .iter()
                .all(|c| c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'))
        );
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
