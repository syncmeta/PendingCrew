//! serve 里的编排：会话管理 + 唤醒（spec §22.4、§22.7）。判定在 crewd-wake，这里只做 IO：
//! 一个 actor（`core::Core`）独占会话句柄与内存状态；数据只经 `port::Port`；执行器只经 `exec::Executor`。
pub mod clock;
pub mod core;
pub mod deliver;
pub mod exec;
pub mod exec_claude;
pub mod hooks;
pub mod port;
#[cfg(test)]
mod testkit;
pub mod worktree;

use std::time::Duration;

use crewd_wake::breaker::{LOOP_LIMITS, LoopLimits};
use crewd_wake::policy::DEFAULT_STAGGER_MS;
use crewd_wake::queue::{BatchLimits, RETRY_JITTER};
use crewd_wake::text::Lang;

#[derive(Clone, Debug)]
pub struct OrchestraConfig {
    pub batch: BatchLimits,
    pub loops: LoopLimits,
    /// 两次拉起之间至少隔多久（§22.7 Q1）。
    pub stagger_ms: i64,
    pub lang: Lang,
    /// 问 Arbiter（M5b 的 Jev）最多等多久；超时 = 没答案 → 群主。
    pub arbiter_timeout: Duration,
    /// 没别的事时多久醒一次兜底（到点唤醒由「下一个 not_before」精确唤醒，这只是保险）。
    pub heartbeat_ms: i64,
    /// 本次 serve 启动的标识（重启询问的 promptId、restart 唤醒的 source）。
    pub boot_id: String,
    /// 失败重试时刻的抖动幅度（P2：一起失败的不一起重试）；0 = 不抖。
    pub retry_jitter: f64,
    /// 抖动的种子；None = 用启动时刻（`Clock::now`）。测试给定值，结果就是确定的。
    pub jitter_seed: Option<u64>,
}

impl Default for OrchestraConfig {
    fn default() -> Self {
        Self {
            batch: BatchLimits::default(),
            loops: LOOP_LIMITS,
            stagger_ms: DEFAULT_STAGGER_MS,
            lang: Lang::Zh,
            arbiter_timeout: Duration::from_secs(15),
            heartbeat_ms: 30_000,
            boot_id: uuid::Uuid::new_v4().to_string(),
            retry_jitter: RETRY_JITTER,
            jitter_seed: None,
        }
    }
}

impl OrchestraConfig {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            stagger_ms: 0,
            arbiter_timeout: Duration::from_millis(50),
            boot_id: "boot-1".into(),
            // 测试默认不抖：退避就是 30s / 2min 整（要抖的测试自己给种子）。
            retry_jitter: 0.0,
            jitter_seed: Some(1),
            ..Self::default()
        }
    }
}

/// M3a 的占位（`AppState.orchestra` 的类型）；Task 9 换成真 `Handle`。
#[derive(Debug, Clone)]
pub struct Handle;
