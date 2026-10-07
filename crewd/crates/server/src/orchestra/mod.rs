//! serve 里的编排：会话管理 + 唤醒（spec §22.4、§22.7）。判定在 crewd-wake，这里只做 IO：
//! 一个 actor（`core::Core`）独占会话句柄与内存状态；数据只经 `port::Port`；执行器只经 `exec::Executor`。
pub mod boot;
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

use std::sync::Arc;
use std::time::Duration;

use crewd_wake::breaker::{LOOP_LIMITS, LoopLimits};
use crewd_wake::{LatchScope, QueuedWake};
use tokio::sync::{mpsc, oneshot};

use self::clock::Clock;
use self::core::{Cmd, CollabFact, ControlError, Core, Input, Origin, Reply, StartSession};
use self::exec::Executors;
use self::hooks::Hooks;
use self::port::{LiveView, Port, SessionRow};
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
    /// 其余挂钩（IdleHook / AskAnsweredHook / QuotaSource / ContextExtras）各自最多等多久（C25：挂住的挂钩不能卡住
    /// actor）。到期 = 当它没答：IdleHook 没有唤醒、QuotaSource 拿不到重置时刻（45 分钟退避）、ContextExtras 空。
    pub hook_timeout: Duration,
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
            hook_timeout: Duration::from_secs(5),
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
            hook_timeout: Duration::from_millis(50),
            boot_id: "boot-1".into(),
            // 测试默认不抖：退避就是 30s / 2min 整（要抖的测试自己给种子）。
            retry_jitter: 0.0,
            jitter_seed: Some(1),
            ..Self::default()
        }
    }
}

/// 编排 actor 的句柄。可克隆；客户端 RPC、`SessionControl`、协作事件转发都经它。
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Input>,
}

/// 建 Core、跑一次启动恢复（`boot`）、起 actor 循环。
pub async fn spawn<P: Port>(
    port: Arc<P>,
    execs: Executors,
    hooks: Hooks,
    clock: Arc<dyn Clock>,
    cfg: OrchestraConfig,
) -> anyhow::Result<Handle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut core = Core::new(port, execs, hooks, clock, cfg, tx.clone()).await?;
    core.boot().await?;
    tokio::spawn(run(core, rx));
    Ok(Handle { tx })
}

/// actor 循环：输入一条条处理；没输入时睡到下一次定时拍（`next_deadline`：最早的未来 not_before、退避 / 错开的
/// 时刻、待决策升级时刻，都没有就心跳）。停机完成就结束（之后句柄的调用都回「编排已停止」）。
async fn run<P: Port>(mut core: Core<P>, mut rx: mpsc::UnboundedReceiver<Input>) {
    loop {
        let wait = core.next_deadline().await.since(core.now()).max(0) as u64;
        tokio::select! {
            input = rx.recv() => match input {
                Some(i) => core.handle(i).await,
                // Core 自己拿着一个发送端（派生任务回报用），所以这里其实收不到 None；收到了也照样收尾。
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(wait)) => core.handle(Input::Tick).await,
        }
        if core.shutdown_done() {
            return;
        }
    }
}

fn stopped() -> ControlError {
    ControlError::Internal("编排已停止".into())
}

impl Handle {
    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Input::Cmd(make(tx))).map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())?
    }

    pub async fn start(
        &self,
        req: StartSession,
        origin: Origin,
    ) -> Result<SessionRow, ControlError> {
        self.call(|reply| Cmd::Start { req, origin, reply }).await
    }

    pub async fn resume(&self, session: &str, origin: Origin) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Resume {
            session,
            origin,
            reply,
        })
        .await
    }

    pub async fn stop(&self, session: &str) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Stop { session, reply }).await
    }

    pub async fn live(&self, session: &str) -> Result<LiveView, ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Live { session, reply }).await
    }

    pub async fn nudge(&self, session: &str, input: String) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Nudge {
            session,
            input,
            reply,
        })
        .await
    }

    /// 实时状态 + 屏幕文本（屏幕在 actor 之外读，不卡 actor）。
    pub async fn inspect(
        &self,
        session: &str,
        max_lines: usize,
    ) -> Result<(LiveView, String), ControlError> {
        let (tx, rx) = oneshot::channel();
        let cmd = Cmd::Handle {
            session: session.to_string(),
            reply: tx,
        };
        self.tx.send(Input::Cmd(cmd)).map_err(|_| stopped())?;
        let handle = rx.await.map_err(|_| stopped())?;
        let live = self.live(session).await?;
        let screen = match handle {
            Some(h) => h.screen_text(max_lines).await.unwrap_or_default(),
            None => String::new(),
        };
        Ok((live, screen))
    }

    pub async fn set_profile(
        &self,
        session: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::SetProfile {
            session,
            model,
            effort,
            reply,
        })
        .await
    }

    pub async fn set_captain(&self, crew: &str, session: &str) -> Result<(), ControlError> {
        let (crew, session) = (crew.to_string(), session.to_string());
        self.call(|reply| Cmd::SetCaptain {
            crew,
            session,
            reply,
        })
        .await
    }

    pub async fn wakes(&self, session: Option<String>) -> Result<Vec<QueuedWake>, ControlError> {
        self.call(|reply| Cmd::Wakes { session, reply }).await
    }

    /// 人解熔断（只给客户端 RPC 用，agent 没有入口）。
    pub async fn recover(&self, scope: LatchScope, id: String) -> Result<(), ControlError> {
        self.call(|reply| Cmd::Recover { scope, id, reply }).await
    }

    pub async fn max_live(&self) -> Result<Option<usize>, ControlError> {
        self.call(|reply| Cmd::MaxLive { reply }).await
    }

    pub async fn set_max_live(&self, max: Option<usize>) -> Result<(), ControlError> {
        self.call(|reply| Cmd::SetMaxLive { max, reply }).await
    }

    /// 有了产出（测试用；生产路径是 M3a `sessions::note_progress` → `Change::Progress` → `forward_collab`，
    /// M3b / M4b 不调这里）。
    pub fn note_progress(&self, session: &str) {
        let _ = self.tx.send(Input::Progress {
            session: session.to_string(),
        });
    }

    pub fn collab(&self, f: CollabFact) {
        let _ = self.tx.send(Input::Collab(f));
    }

    /// 停机：叫停全部会话（crewd 叫停，不是人停：desired / was_working 不动，带着的唤醒原样退回），最多等 `limit`。
    /// `limit` 就是期限（T9b 的休眠在这之上加「先等安全点」，C26）。
    pub async fn shutdown(&self, limit: Duration) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Input::Shutdown(tx)).is_ok() {
            let _ = tokio::time::timeout(limit, rx).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::clock::ManualClock;
    use crate::orchestra::testkit::{MemChannel, MemKind, MemPort, StandIn, T0};

    #[tokio::test]
    async fn handle_drives_the_actor_and_shutdown_waits_for_exits() {
        let port = Arc::new(MemPort::new());
        {
            let mut st = port.lock();
            st.now = T0;
            st.channels.insert(
                "c1/lobby".into(),
                MemChannel {
                    crew: "c1".into(),
                    kind: MemKind::Lobby,
                    owner: None,
                    members: vec![],
                },
            );
        }
        let row = port
            .insert_session(port::NewSession {
                crew_id: "c1".into(),
                captain: true,
                executor: "claude".into(),
                brief: String::new(),
                title: Some("cap".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        let exec = StandIn::default();
        let h = spawn(
            port.clone(),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        let mut running = false;
        for _ in 0..200 {
            if h.live(&row.id).await.unwrap().running {
                running = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(running, "拉起结果经事件回到 actor");
        assert_eq!(
            h.resume(&row.id, Origin::Human).await,
            Err(ControlError::AlreadyRunning(row.id.clone()))
        );
        h.shutdown(Duration::from_secs(5)).await;
        assert_eq!(exec.lock().stops, std::slice::from_ref(&row.id));
        assert!(
            h.live(&row.id).await.is_err(),
            "停机完了 actor 就结束了：句柄回「编排已停止」"
        );
    }
}
