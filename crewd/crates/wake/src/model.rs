//! 唤醒的数据形状：时间、优先级、种类、状态、队列一行、熔断记录。全是纯值。
//! 词表与 schema v3 的 `wakes` / `wake_latches` 表一一对应（spec §22.2；`edge` 见 §22.7 Q5）。
use std::fmt;

/// Unix 毫秒。时间一律由调用方给（同 crewd-work 的 `Ts`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ms(pub i64);

pub const SEC: i64 = 1_000;
pub const MIN: i64 = 60 * SEC;
pub const HOUR: i64 = 60 * MIN;

impl Ms {
    pub const fn add(self, ms: i64) -> Ms {
        Ms(self.0.saturating_add(ms))
    }

    /// `self - earlier`（毫秒，饱和）。
    pub const fn since(self, earlier: Ms) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// 带表内词的枚举：`as_str` / `parse` / `ALL`，词就是库里存的值。
macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($(#[$vm])* $v),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];

            pub const fn as_str(self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$v),)+ _ => None }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

str_enum!(
    /// 唤醒优先级。声明顺序即 `Ord`：Auto < System < Human（合并投递只取最高一档）。
    Priority { Auto = "auto", System = "system", Human = "human" }
);

str_enum!(
    /// `checkpoint` / `stop_check` 是给 M4b 留的，M3c 里没有写者。
    WakeKind {
        Message = "message",
        Timer = "timer",
        AskAnswer = "ask_answer",
        Restart = "restart",
        System = "system",
        Checkpoint = "checkpoint",
        StopCheck = "stop_check",
    }
);

str_enum!(WakeState {
    Pending = "pending",
    Inflight = "inflight",
    Delivered = "delivered",
    Dropped = "dropped",
});

str_enum!(
    /// 环路熔断的两级（§22.7 Q5）：`Session` = 目标的非 session 来源全部停，`Edge` = 「来源 > 目标」这一条停。
    LatchScope { Session = "session", Edge = "edge" }
);

/// 入队请求（= `wakes` 表一行的可写部分）。`UNIQUE(session, kind, source)` 去重。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWake {
    pub session: String,
    pub kind: WakeKind,
    /// 来源身份（不存渲染好的正文，出队时现渲染：`CrewDeferredWakeQueue.swift:13`）。
    pub source: String,
    pub priority: Priority,
    pub not_before: Ms,
    /// 非消息类唤醒的附言（已渲染好的文字，来自 `text`）。
    pub note: Option<String>,
    /// 是谁引起的（`breaker::origin_of` 的键：`human` / `system` / session id；环路熔断用，§22.7 Q5）。
    pub origin: String,
}

/// 队列里的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedWake {
    pub id: i64,
    pub session: String,
    pub kind: WakeKind,
    pub source: String,
    pub priority: Priority,
    pub not_before: Ms,
    pub note: Option<String>,
    pub state: WakeState,
    pub attempts: u32,
    pub created_at: Ms,
    pub origin: String,
    /// 消息类唤醒指向的那条消息（Port 从 `msg:<id>` 解析出来；消息已删 = None）。
    pub msg: Option<MessageRef>,
}

/// 一条消息在群里的位置（游标比的是它）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub channel: String,
    pub seq: i64,
}

/// `wakes.source` 的拼法。**与 `crewd_collab::wakes::source` 逐字相同**（M3a 定死的格式；
/// `render::build_batch` 靠它找消息 / 问）。`orchestra/db.rs` 有一条测试逐个比对两边。
pub mod source {
    pub fn message(message_id: &str) -> String {
        format!("msg:{message_id}")
    }
    pub fn quota(at_ms: i64) -> String {
        format!("quota:{at_ms}")
    }
    pub fn restart(tag: &str) -> String {
        format!("restart:{tag}")
    }
    pub fn system(tag: &str) -> String {
        format!("system:{tag}")
    }
    pub fn timer(token: &str) -> String {
        format!("timer:{token}")
    }
}

/// 一条熔断（`wake_latches` 一行）。只有人能从客户端解（`admission.recover`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latch {
    pub scope: LatchScope,
    pub id: String,
    /// 诊断短码（如 `no-progress:9/10min`）；给人看的话由 IO 层用 `text` 渲染，不存在这里。
    pub reason: String,
    pub at: Ms,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_their_table_words() {
        for p in Priority::ALL {
            assert_eq!(Priority::parse(p.as_str()), Some(*p));
        }
        for k in WakeKind::ALL {
            assert_eq!(WakeKind::parse(k.as_str()), Some(*k));
        }
        for s in WakeState::ALL {
            assert_eq!(WakeState::parse(s.as_str()), Some(*s));
        }
        for s in LatchScope::ALL {
            assert_eq!(LatchScope::parse(s.as_str()), Some(*s));
        }
        assert_eq!(WakeKind::AskAnswer.as_str(), "ask_answer");
        assert_eq!(WakeKind::StopCheck.as_str(), "stop_check");
        assert_eq!(Priority::parse("urgent"), None);
    }

    #[test]
    fn priority_orders_auto_below_system_below_human() {
        assert!(Priority::Auto < Priority::System);
        assert!(Priority::System < Priority::Human);
    }

    #[test]
    fn source_formats_match_the_m3a_contract() {
        assert_eq!(source::message("m-1"), "msg:m-1");
        assert_eq!(source::quota(5), "quota:5");
        assert_eq!(source::restart("7"), "restart:7");
        assert_eq!(source::system("captain:c1:9"), "system:captain:c1:9");
        assert_eq!(source::timer("t"), "timer:t");
    }

    #[test]
    fn ms_arithmetic_saturates() {
        assert_eq!(Ms(i64::MAX).add(1), Ms(i64::MAX));
        assert_eq!(Ms(10).since(Ms(4)), 6);
        assert_eq!(Ms(i64::MIN).since(Ms(1)), i64::MIN);
    }
}
