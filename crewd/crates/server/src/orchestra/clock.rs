//! 时间来源。判定层只吃 `Ms`；IO 层只从这里取「现在」，测试用 `ManualClock`。
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crewd_wake::Ms;

pub trait Clock: Send + Sync {
    fn now(&self) -> Ms;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Ms {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        Ms(ms)
    }
}

pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(at: Ms) -> Self {
        Self(AtomicI64::new(at.0))
    }

    pub fn set(&self, at: Ms) {
        self.0.store(at.0, Ordering::SeqCst);
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Ms {
        Ms(self.0.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_told() {
        let c = ManualClock::new(Ms(5));
        assert_eq!(c.now(), Ms(5));
        c.advance(10);
        assert_eq!(c.now(), Ms(15));
        c.set(Ms(1));
        assert_eq!(c.now(), Ms(1));
        assert!(
            SystemClock.now().0 > 1_700_000_000_000,
            "系统时钟是 Unix 毫秒"
        );
    }
}
