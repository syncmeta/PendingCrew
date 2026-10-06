//! 给后面的里程碑留的挂钩点（spec §22.4、§22.6）。M3c 交付的都是惰性桩：
//! - `Arbiter`：人发、没有 @ 的消息叫谁（M5b 换成 Jev + 别名层，§20）；`None` / 空集 = 交群主（§22.7 Q2）。
//! - `IdleHook`：idle 边沿、队列空时还要不要叫它一次（M4b 的「停下即检查」）。
//! - `AskAnsweredHook`：问被回答后（M4b 的 BlockerCleared 叫醒）；`cleared` = M3a `Change::AskAnswered`
//!   带来的 (工作项, 负责人)。
//! - `QuotaSource`：额度重置时刻（`Health` 里没有；拿不到就按 45 分钟退避）。
//! - `ContextExtras`：codex 的 `additionalContext` 按 key 组装（M4b 注册 `my_work_items`）。
//!
//! 它们都是 async：Jev 要联网、M4b 要读库。
use std::sync::Arc;

use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::{Ms, NewWake};

use super::exec::{BoxFut, Context};
use super::worktree::{self, Worktrees};

pub struct ArbiterQuestion {
    pub message: MsgFacts,
    pub text: String,
    pub ctx: ChannelCtx,
}

pub trait Arbiter: Send + Sync {
    fn wake_set<'a>(&'a self, q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>>;
}

/// M5b 之前的桩：永远没答案 → 群主。
pub struct NoArbiter;

impl Arbiter for NoArbiter {
    fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
        Box::pin(async { None })
    }
}

pub trait IdleHook: Send + Sync {
    fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>>;
}

pub struct NoIdleHook;

impl IdleHook for NoIdleHook {
    fn on_idle<'a>(&'a self, _session: &'a str, _now: Ms) -> BoxFut<'a, Option<NewWake>> {
        Box::pin(async { None })
    }
}

pub trait AskAnsweredHook: Send + Sync {
    fn on_answered<'a>(
        &'a self,
        crew: &'a str,
        ask: &'a str,
        cleared: &'a [(String, Option<String>)],
        now: Ms,
    ) -> BoxFut<'a, ()>;
}

pub struct NoAskHook;

impl AskAnsweredHook for NoAskHook {
    fn on_answered<'a>(
        &'a self,
        _crew: &'a str,
        _ask: &'a str,
        _cleared: &'a [(String, Option<String>)],
        _now: Ms,
    ) -> BoxFut<'a, ()> {
        Box::pin(async {})
    }
}

pub trait QuotaSource: Send + Sync {
    fn reset_at<'a>(&'a self, executor: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>>;
}

pub struct NoQuotaSource;

impl QuotaSource for NoQuotaSource {
    fn reset_at<'a>(&'a self, _executor: &'a str, _now: Ms) -> BoxFut<'a, Option<Ms>> {
        Box::pin(async { None })
    }
}

pub trait ContextExtras: Send + Sync {
    fn extras<'a>(&'a self, session: &'a str) -> BoxFut<'a, Context>;
}

pub struct NoExtras;

impl ContextExtras for NoExtras {
    fn extras<'a>(&'a self, _session: &'a str) -> BoxFut<'a, Context> {
        Box::pin(async { Context::new() })
    }
}

#[derive(Clone)]
pub struct Hooks {
    pub arbiter: Arc<dyn Arbiter>,
    pub idle: Arc<dyn IdleHook>,
    pub ask: Arc<dyn AskAnsweredHook>,
    pub quota: Arc<dyn QuotaSource>,
    pub extras: Arc<dyn ContextExtras>,
    /// `isolation: true` 时建 git worktree（控制者裁定 C8）；默认真 git，测试换 `StandInWorktrees`。
    pub worktrees: Arc<dyn Worktrees>,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            arbiter: Arc::new(NoArbiter),
            idle: Arc::new(NoIdleHook),
            ask: Arc::new(NoAskHook),
            quota: Arc::new(NoQuotaSource),
            extras: Arc::new(NoExtras),
            worktrees: worktree::real(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_wake::MessageRef;
    use crewd_wake::plan::Sender;

    #[tokio::test]
    async fn default_hooks_are_inert_stubs() {
        let h = Hooks::default();
        let q = ArbiterQuestion {
            message: MsgFacts {
                id: "m-1".into(),
                at: MessageRef {
                    channel: "c1/lobby".into(),
                    seq: 1,
                },
                sender: Sender::Human,
                mentions: vec![],
                created_at: Ms(0),
            },
            text: "合成消息".into(),
            ctx: ChannelCtx::default(),
        };
        assert_eq!(
            h.arbiter.wake_set(&q).await,
            None,
            "M5b 之前没有 Jev → 交群主"
        );
        assert_eq!(h.idle.on_idle("s1", Ms(0)).await, None);
        assert_eq!(h.quota.reset_at("claude", Ms(0)).await, None);
        assert!(h.extras.extras("s1").await.is_empty());
        h.ask.on_answered("c1", "H1", &[], Ms(0)).await;
    }
}
