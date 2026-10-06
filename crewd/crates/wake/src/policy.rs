//! 会话生命周期的几条策略（纯函数）：额度重置唤醒的时刻、重启后的恢复名单顺序、启动闸（可选上限 + 错开）。
use crate::model::{MIN, Ms, QueuedWake, SEC, WakeState};
use crate::queue::is_due;

/// 重置后再缓 1 分钟（额度窗刚翻新，别掐点撞上仍受限的边缘；`AgentQuota.swift:342`）。
pub const QUOTA_RESET_BUFFER_MS: i64 = MIN;
/// 拿不到可信重置时刻时的退避（`AgentQuota.swift:344`）。
pub const QUOTA_FALLBACK_MS: i64 = 45 * MIN;
pub const QUOTA_SOURCE_PREFIX: &str = "quota:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaPlan {
    pub fire_at: Ms,
    /// true = 没拿到可信重置时刻，走的退避（群里要说清，Swift 的 fail-loud）。
    pub fallback: bool,
}

impl QuotaPlan {
    pub fn source(&self) -> String {
        crate::model::source::quota(self.fire_at.0)
    }
}

pub fn quota_plan(reset_at: Option<Ms>, now: Ms) -> QuotaPlan {
    match reset_at {
        Some(r) if r > now => QuotaPlan {
            fire_at: r.add(QUOTA_RESET_BUFFER_MS),
            fallback: false,
        },
        _ => QuotaPlan {
            fire_at: now.add(QUOTA_FALLBACK_MS),
            fallback: true,
        },
    }
}

fn is_quota(w: &QueuedWake) -> bool {
    w.source.starts_with(QUOTA_SOURCE_PREFIX)
}

/// 已挂着一条还没投完的额度唤醒（pending / inflight）。
pub fn quota_pending(pending: &[QueuedWake]) -> bool {
    pending
        .iter()
        .any(|w| is_quota(w) && matches!(w.state, WakeState::Pending | WakeState::Inflight))
}

pub fn quota_due(pending: &[QueuedWake], now: Ms) -> bool {
    pending.iter().any(|w| is_quota(w) && is_due(w, now))
}

/// 重启时「上次在跑」的一行（`desired=running`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartRow {
    pub session: String,
    pub name: String,
    pub crew: String,
    pub captain: bool,
    /// 上次停在干活中途（`was_working=1`）：恢复时补一条「被打断」附言。
    pub was_working: bool,
    pub last_active_at: Option<Ms>,
}

/// 恢复名单的顺序：机长在前，然后最近活跃的在前，最后按 id（§22.7 Q1：人答「恢复」后按这个顺序错开拉起）。
pub fn restart_order(mut rows: Vec<RestartRow>) -> Vec<RestartRow> {
    rows.sort_by(|a, b| {
        b.captain
            .cmp(&a.captain)
            .then(b.last_active_at.cmp(&a.last_active_at))
            .then(a.session.cmp(&b.session))
    });
    rows
}

/// 重启询问的恢复唤醒：`tag` = M3a `restart_prompts` 的 id。
pub fn restart_source(tag: &str) -> String {
    crate::model::source::restart(tag)
}

/// 两次拉起之间至少隔这么久（§22.7 Q1「错开时间逐个」）。
pub const DEFAULT_STAGGER_MS: i64 = 2 * SEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// 可选上限已满（只挡自动拉起）。
    AtCap,
    /// 离上一次拉起太近，这一刻之后再试。
    WaitUntil(Ms),
}

/// 启动闸。**默认没有上限**（§22.7 Q5）；上限只挡自动拉起，人起的（`bypass_cap`）不挡。错开对谁都生效。
#[derive(Clone, Debug)]
pub struct LaunchGate {
    max_live: Option<usize>,
    stagger_ms: i64,
    last: Option<Ms>,
}

impl LaunchGate {
    pub fn new(max_live: Option<usize>, stagger_ms: i64) -> Self {
        Self {
            max_live,
            stagger_ms,
            last: None,
        }
    }

    /// `live` = 正在跑 + 正在起的会话数。
    pub fn check(&self, live: usize, bypass_cap: bool, now: Ms) -> Gate {
        if !bypass_cap && self.max_live.is_some_and(|m| live >= m) {
            return Gate::AtCap;
        }
        match self.last {
            Some(t) if now.since(t) < self.stagger_ms => Gate::WaitUntil(t.add(self.stagger_ms)),
            _ => Gate::Go,
        }
    }

    pub fn launched(&mut self, now: Ms) {
        self.last = Some(self.last.map_or(now, |t| t.max(now)));
    }

    pub fn set_max_live(&mut self, max: Option<usize>) {
        self.max_live = max;
    }

    pub fn max_live(&self) -> Option<usize> {
        self.max_live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Priority, WakeKind};

    fn quota_wake(source: &str, state: WakeState, not_before: i64) -> QueuedWake {
        QueuedWake {
            id: 1,
            session: "s".into(),
            kind: WakeKind::System,
            source: source.into(),
            priority: Priority::System,
            not_before: Ms(not_before),
            note: None,
            state,
            attempts: 0,
            created_at: Ms(0),
            origin: "system".into(),
            msg: None,
        }
    }

    #[test]
    fn quota_wake_fires_a_minute_after_a_future_reset_else_backs_off() {
        let now = Ms(1_000 * MIN);
        let p = quota_plan(Some(now.add(30 * MIN)), now);
        assert_eq!(
            p,
            QuotaPlan {
                fire_at: now.add(31 * MIN),
                fallback: false
            }
        );
        assert_eq!(p.source(), format!("quota:{}", now.add(31 * MIN).0));
        for stale in [None, Some(now), Some(now.add(-MIN))] {
            assert_eq!(
                quota_plan(stale, now),
                QuotaPlan {
                    fire_at: now.add(45 * MIN),
                    fallback: true
                }
            );
        }
    }

    #[test]
    fn quota_pending_and_due_look_only_at_quota_sources() {
        let p = vec![
            quota_wake("quota:5", WakeState::Pending, 50),
            quota_wake("timer:x", WakeState::Pending, 0),
        ];
        assert!(quota_pending(&p));
        assert!(!quota_due(&p, Ms(49)));
        assert!(quota_due(&p, Ms(50)));
        assert!(!quota_pending(&[quota_wake(
            "quota:5",
            WakeState::Delivered,
            0
        )]));
        assert!(quota_pending(&[quota_wake(
            "quota:5",
            WakeState::Inflight,
            0
        )]));
    }

    fn row(session: &str, captain: bool, was_working: bool, last: Option<i64>) -> RestartRow {
        RestartRow {
            session: session.into(),
            name: session.into(),
            crew: "c".into(),
            captain,
            was_working,
            last_active_at: last.map(Ms),
        }
    }

    #[test]
    fn restart_order_puts_captains_first_then_most_recently_active() {
        let got: Vec<String> = restart_order(vec![
            row("w-old", false, false, Some(1)),
            row("w-never", false, false, None),
            row("cap", true, false, Some(0)),
            row("w-new", false, true, Some(9)),
        ])
        .into_iter()
        .map(|r| r.session)
        .collect();
        assert_eq!(got, ["cap", "w-new", "w-old", "w-never"]);
        assert_eq!(restart_source("b1"), "restart:b1");
    }

    #[test]
    fn launch_gate_has_no_cap_by_default_and_staggers_launches() {
        let mut g = LaunchGate::new(None, DEFAULT_STAGGER_MS);
        assert_eq!(
            g.check(10_000, false, Ms(0)),
            Gate::Go,
            "默认不设上限（§22.7 Q5）"
        );
        g.launched(Ms(0));
        assert_eq!(
            g.check(1, false, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS))
        );
        assert_eq!(
            g.check(1, true, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)),
            "错开对谁都生效"
        );
        assert_eq!(g.check(1, false, Ms(DEFAULT_STAGGER_MS)), Gate::Go);
        g.launched(Ms(-5));
        assert_eq!(
            g.check(1, false, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)),
            "launched 只往后推"
        );
    }

    #[test]
    fn optional_cap_blocks_only_automatic_launches() {
        let mut g = LaunchGate::new(None, 0);
        g.set_max_live(Some(3));
        assert_eq!(g.max_live(), Some(3));
        assert_eq!(g.check(2, false, Ms(0)), Gate::Go);
        assert_eq!(g.check(3, false, Ms(0)), Gate::AtCap);
        assert_eq!(g.check(3, true, Ms(0)), Gate::Go, "人起的不受上限");
        g.set_max_live(None);
        assert_eq!(g.check(3, false, Ms(0)), Gate::Go);
    }
}
