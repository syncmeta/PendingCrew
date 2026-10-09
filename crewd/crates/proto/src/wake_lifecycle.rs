//! 唤醒行的生命周期：**唯一的一张**状态转移表（控制者裁定 C24）。
//!
//! 库里的 `crewd_collab::wakes`（写函数）与编排的测试实现 `MemPort` 都只经 [`next`] 改状态、只经
//! [`attempts_after`] 改次数——假的不可能比库松，两边也不会各写一份起始状态再漂开（C23 的病根）。
//! 放在 crewd-proto：collab 与 server 都已依赖它，crewd-wake 保持零依赖（不引入新的依赖边）。
//!
//! 规则（同 Oban / River）：
//! - 一行**要么**是 pending（谁都可以取）**要么**是 inflight（某一批拿着它），没有「看起来 pending、其实被一批带着」：
//!   随一批走的窗口外行（M3a R6 `deferred`）跟这批一起 `Claim`。
//! - 次数在 `Claim` 时记（这是第几次尝试）；`Fail` 不再加，判放弃读库里的数；`Requeue`（没尝试成：crewd 停 /
//!   撞额度 / 随批成功的窗口外行放回）把这次 `Claim` 记的那一次退回去。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakeState {
    Pending,
    Inflight,
    Delivered,
    Dropped,
}

impl WakeState {
    pub const ALL: [WakeState; 4] = [
        WakeState::Pending,
        WakeState::Inflight,
        WakeState::Delivered,
        WakeState::Dropped,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            WakeState::Pending => "pending",
            WakeState::Inflight => "inflight",
            WakeState::Delivered => "delivered",
            WakeState::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<WakeState> {
        Self::ALL.into_iter().find(|w| w.as_str() == s)
    }
}

impl std::fmt::Display for WakeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 改一行状态的全部理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeEvent {
    /// 一批拿走它（含随批的窗口外行）：pending → inflight，次数 +1。
    Claim,
    /// 送到了（inflight），或游标已越过 / 无附言的恢复在跑就算到（pending）。
    Deliver,
    /// 这一批终局失败、还要再试：inflight → pending；`retry_at` = 新的 `not_before`（None = 不改）。次数不变。
    Fail { retry_at: Option<i64> },
    /// 失败到上限、放弃：inflight → dropped。
    GiveUp,
    /// 人停掉 / 渲染不出来 / 没有这个 session：pending 或 inflight → dropped。
    Drop,
    /// 没尝试成、原样放回（crewd 停、撞额度、随批成功的窗口外行）：inflight → pending，退回 `Claim` 记的那一次。
    Requeue,
}

impl WakeEvent {
    /// 每一种事件各一个（表驱动测试遍历它；`Fail` 的 `retry_at` 不影响状态）。
    pub const ALL: [WakeEvent; 6] = [
        WakeEvent::Claim,
        WakeEvent::Deliver,
        WakeEvent::Fail { retry_at: None },
        WakeEvent::GiveUp,
        WakeEvent::Drop,
        WakeEvent::Requeue,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WakeEvent::Claim => "claim",
            WakeEvent::Deliver => "deliver",
            WakeEvent::Fail { .. } => "fail",
            WakeEvent::GiveUp => "give_up",
            WakeEvent::Drop => "drop",
            WakeEvent::Requeue => "requeue",
        }
    }
}

/// 这一行此刻不认这个事件（库回 `Conflict`，MemPort 回错；整批都不改）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Illegal {
    pub from: WakeState,
    pub event: WakeEvent,
}

impl std::fmt::Display for Illegal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "唤醒是 {}，不认 {}", self.from, self.event.name())
    }
}

impl std::error::Error for Illegal {}

/// 状态转移表。表外的一律 [`Illegal`]。
pub fn next(from: WakeState, event: WakeEvent) -> Result<WakeState, Illegal> {
    use WakeEvent::*;
    use WakeState::*;
    match (from, event) {
        (Pending, Claim) => Ok(Inflight),
        (Pending | Inflight, Deliver) => Ok(Delivered),
        (Inflight, Fail { .. } | Requeue) => Ok(Pending),
        (Inflight, GiveUp) | (Pending | Inflight, Drop) => Ok(Dropped),
        _ => Err(Illegal { from, event }),
    }
}

/// 持久化的人停意图只禁止旧行再尝试，不抹掉真实送达。先核原事件（终态不变、pending 不能 fail/requeue），
/// 再把旧行的 Fail / GiveUp / Requeue 改为 Drop；Claim 拒绝。取消不是失败，不退次数、不走 give-up 告警。
/// `cancelled` 来自库里单调的 stop wake-ID 水位，不是此刻的 desired / updated_at（新工作会改它们）。
pub fn event_after_cancel(
    from: WakeState,
    event: WakeEvent,
    cancelled: bool,
) -> Result<WakeEvent, Illegal> {
    next(from, event)?;
    if !cancelled {
        return Ok(event);
    }
    match event {
        WakeEvent::Claim => Err(Illegal { from, event }),
        WakeEvent::Fail { .. } | WakeEvent::GiveUp | WakeEvent::Requeue => Ok(WakeEvent::Drop),
        WakeEvent::Deliver | WakeEvent::Drop => Ok(event),
    }
}

/// 事件之后的次数（只在 [`next`] 认了这个事件时用）。
pub fn attempts_after(event: WakeEvent, attempts: u32) -> u32 {
    match event {
        WakeEvent::Claim => attempts + 1,
        WakeEvent::Requeue => attempts.saturating_sub(1),
        _ => attempts,
    }
}

#[cfg(test)]
mod tests {
    use super::WakeEvent::*;
    use super::WakeState::*;
    use super::*;

    /// 整张表逐格写出来：4 个状态 × 6 个事件，每一格都有答案（None = Illegal）。
    #[test]
    fn every_state_and_event_has_exactly_the_documented_answer() {
        let fail = Fail { retry_at: None };
        let events = [Claim, Deliver, fail, GiveUp, Drop, Requeue];
        assert_eq!(events, WakeEvent::ALL);
        #[rustfmt::skip]
        let table: [(WakeState, [Option<WakeState>; 6]); 4] = [
            //           Claim           Deliver          Fail           GiveUp         Drop           Requeue
            (Pending,   [Some(Inflight), Some(Delivered), None,          None,          Some(Dropped), None]),
            (Inflight,  [None,           Some(Delivered), Some(Pending), Some(Dropped), Some(Dropped), Some(Pending)]),
            (Delivered, [None; 6]),
            (Dropped,   [None; 6]),
        ];
        for (from, row) in table {
            for (event, want) in events.into_iter().zip(row) {
                let got = next(from, event);
                match want {
                    Some(to) => assert_eq!(got, Ok(to), "{from:?} + {event:?}"),
                    None => assert_eq!(got, Err(Illegal { from, event }), "{from:?} + {event:?}"),
                }
            }
        }
    }

    /// C24：pending 的行不认 `Fail` / `GiveUp` / `Requeue`（C23 的「从 pending 记失败」桥拆掉了）——
    /// 随批的窗口外行已被 `Claim` 成 inflight。
    #[test]
    fn a_pending_row_cannot_fail_give_up_or_be_requeued() {
        for e in [Fail { retry_at: Some(5) }, GiveUp, Requeue] {
            assert!(next(Pending, e).is_err(), "{e:?}");
        }
    }

    /// Task 8b 评审 (3)：`Fail` 与 `GiveUp` 各自认哪些起始状态——逐个钉住，调用方（collab `ack_failed`、MemPort）
    /// 按真正要用的那个事件核，不拿其中一个当另一个的替身（两者今天相同，表改了哪个都会在这里红）。
    #[test]
    fn fail_and_give_up_are_accepted_from_inflight_only() {
        for event in [Fail { retry_at: None }, Fail { retry_at: Some(7) }, GiveUp] {
            let accepted: Vec<WakeState> = WakeState::ALL
                .into_iter()
                .filter(|s| next(*s, event).is_ok())
                .collect();
            assert_eq!(accepted, [Inflight], "{event:?}");
        }
        assert_eq!(next(Inflight, Fail { retry_at: Some(7) }), Ok(Pending));
        assert_eq!(next(Inflight, GiveUp), Ok(Dropped));
    }

    /// `Fail` 的 `retry_at` 不影响状态。
    #[test]
    fn fail_goes_back_to_pending_whatever_the_retry_time() {
        assert_eq!(next(Inflight, Fail { retry_at: Some(99) }), Ok(Pending));
        assert_eq!(next(Inflight, Fail { retry_at: None }), Ok(Pending));
    }

    /// 次数：`Claim` +1；`Requeue` 退回那一次（不低于 0）；别的都不动。
    #[test]
    fn attempts_are_counted_at_claim_and_returned_by_requeue() {
        assert_eq!(attempts_after(Claim, 0), 1);
        assert_eq!(attempts_after(Claim, 2), 3);
        assert_eq!(attempts_after(Requeue, 3), 2);
        assert_eq!(attempts_after(Requeue, 0), 0);
        for e in [Deliver, Fail { retry_at: Some(1) }, GiveUp, Drop] {
            assert_eq!(attempts_after(e, 2), 2, "{e:?}");
        }
    }

    #[test]
    fn cancellation_uses_the_same_table_and_never_revives_or_rewrites_terminal_rows() {
        for from in WakeState::ALL {
            for event in WakeEvent::ALL {
                let ordinary = next(from, event);
                assert_eq!(
                    event_after_cancel(from, event, false),
                    ordinary.map(|_| event)
                );
                let got = event_after_cancel(from, event, true);
                if ordinary.is_err() || event == Claim {
                    assert_eq!(got, Err(Illegal { from, event }), "{from:?} {event:?}");
                } else {
                    let want = match event {
                        Fail { .. } | GiveUp | Requeue => Drop,
                        _ => event,
                    };
                    assert_eq!(got, Ok(want), "{from:?} {event:?}");
                    assert_ne!(next(from, want).unwrap(), Pending);
                }
            }
        }
        assert_eq!(event_after_cancel(Inflight, Deliver, true), Ok(Deliver));
        assert_eq!(event_after_cancel(Inflight, Requeue, true), Ok(Drop));
        assert_eq!(
            attempts_after(Drop, 3),
            3,
            "cancellation is not retry or a refund"
        );
    }

    #[test]
    fn state_names_round_trip() {
        for s in WakeState::ALL {
            assert_eq!(WakeState::parse(s.as_str()), Some(s));
            assert_eq!(s.to_string(), s.as_str());
        }
        assert_eq!(WakeState::parse("held"), None);
    }
}
