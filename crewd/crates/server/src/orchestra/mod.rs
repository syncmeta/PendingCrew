//! serve 里的编排：会话管理 + 唤醒（spec §22.4、§22.7）。判定在 crewd-wake，这里只做 IO：
//! 一个 actor（`core::Core`）独占会话句柄与内存状态；数据只经 `port::Port`；执行器只经 `exec::Executor`。
pub mod boot;
pub mod clock;
mod codex_control;
mod codex_rpc;
pub mod control;
pub mod core;
pub mod db;
pub mod deliver;
pub mod exec;
pub mod exec_claude;
pub(crate) mod exec_claude_managed;
pub mod exec_codex;
pub mod hibernate;
pub mod hooks;
mod notify;
pub mod port;
#[cfg(test)]
mod port_contract;
mod public_control;
pub mod rpc;
#[cfg(test)]
mod sim;
#[cfg(test)]
mod t10c;
#[cfg(test)]
pub(crate) mod testkit;
pub mod work_context;
pub mod work_idle;
pub mod worktree;

use std::sync::Arc;
use std::time::Duration;

use crewd_wake::LatchScope;
use crewd_wake::breaker::{LOOP_LIMITS, LoopLimits};
use tokio::sync::{mpsc, oneshot, watch};

use self::clock::Clock;
use self::core::{
    AnswerReport, Cmd, CollabFact, ControlError, Core, Input, Origin, Reply, SettingsUpdate,
    SettingsView, StartSession,
};
use self::exec::Executors;
use self::hooks::Hooks;
use self::port::{LiveView, Port, SessionRow};
use crewd_wake::policy::DEFAULT_STAGGER_MS;
use crewd_wake::queue::{BatchLimits, RETRY_JITTER};
use crewd_wake::text::Lang;

#[derive(Clone, Debug)]
pub struct OrchestraConfig {
    /// Production admission is ephemeral and starts empty on every daemon run.
    pub admission: Option<Arc<crate::lifecycle::Admission>>,
    pub previous_clean: bool,
    pub batch: BatchLimits,
    pub loops: LoopLimits,
    /// 两次拉起之间至少隔多久（§22.7 Q1）。
    pub stagger_ms: i64,
    pub lang: Lang,
    /// 问 Arbiter（M5b 的 Jev）最多等多久；超时 = 没答案 → 群主。C25（Jev 的决策预算）：缺省 2 秒——人发的
    /// 没 @ 谁的消息最多晚这么久才叫醒群主；之后才到的答案作废。
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
    /// 休眠（C26）到了期限、叫停之后最多再等多久它们退出；到点还没退的照实回报（`HibernateReport::failed`）。
    pub hibernate_stop_grace: Duration,
}

impl Default for OrchestraConfig {
    fn default() -> Self {
        Self {
            admission: None,
            previous_clean: true,
            batch: BatchLimits::default(),
            loops: LOOP_LIMITS,
            stagger_ms: DEFAULT_STAGGER_MS,
            lang: Lang::Zh,
            arbiter_timeout: Duration::from_secs(2),
            hook_timeout: Duration::from_secs(5),
            heartbeat_ms: 30_000,
            boot_id: uuid::Uuid::new_v4().to_string(),
            retry_jitter: RETRY_JITTER,
            jitter_seed: None,
            hibernate_stop_grace: Duration::from_secs(10),
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
    execs: Executors,
    tx: mpsc::UnboundedSender<Input>,
    exit: watch::Receiver<bool>,
    #[cfg(test)]
    test_panic: mpsc::UnboundedSender<()>,
}

/// 跑着的 actor 任务（C25：不能悄悄留下一个死句柄）。serve 必须等它：[`Actor::finished`] 回 `Err` = actor panic 了，
/// serve 要大声地非零退出（让 app 重启它，下次启动由 `boot` 回收），不能带着一个只会回 `ShuttingDown` 的句柄继续跑。
pub struct Actor {
    task: tokio::task::JoinHandle<()>,
}

/// actor 不是正常收尾（停机完成）而是没了：panic 的原话，或任务被取消。
#[derive(Debug)]
pub struct ActorDied(pub String);

impl std::fmt::Display for ActorDied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "编排 actor 崩了：{}", self.0)
    }
}

impl std::error::Error for ActorDied {}

impl Actor {
    /// 等 actor 结束：停机完成 = `Ok`；被我们自己中止（[`Actor::abort_handle`]，停机到期限之后）也是 `Ok`；
    /// panic = `Err`（tokio 的 `JoinError` 文字里带着 panic 的原话）。
    pub async fn finished(self) -> Result<(), ActorDied> {
        match self.task.await {
            Ok(()) => Ok(()),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(ActorDied(e.to_string())),
        }
    }

    /// 中止 actor 的把手（先拿到它，再把 `Actor` 交给等 `finished` 的那一路）。serve 在 [`Handle::shutdown`]
    /// 回 false（到期限还有会话没退）之后用它中止 actor、再关库（`Db::shutdown`）。中止时还被拿着的唤醒行仍是
    /// inflight：下次启动由 `boot` 按一次失败的尝试回收（C25，至少一次、到上限放弃）。
    pub fn abort_handle(&self) -> tokio::task::AbortHandle {
        self.task.abort_handle()
    }
}

/// 建 Core、跑一次启动恢复（`boot`）、起 actor 循环。返回句柄与 actor 任务（serve 要等它，见 [`Actor`]）。
pub async fn spawn<P: Port>(
    port: Arc<P>,
    execs: Executors,
    hooks: Hooks,
    clock: Arc<dyn Clock>,
    cfg: OrchestraConfig,
) -> anyhow::Result<(Handle, Actor)> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut core = Core::new(port, execs.clone(), hooks, clock, cfg, tx.clone()).await?;
    core.boot().await?;
    let exit = core.exit.subscribe();
    #[cfg(not(test))]
    let task = tokio::spawn(run(core, rx));
    #[cfg(test)]
    let (test_panic, mut panic_rx) = mpsc::unbounded_channel();
    #[cfg(test)]
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = run(core, rx) => {},
            Some(()) = panic_rx.recv() => panic!("synthetic actor panic for serve lifecycle"),
        }
    });
    Ok((
        Handle {
            execs,
            tx,
            exit,
            #[cfg(test)]
            test_panic,
        },
        Actor { task },
    ))
}

/// actor 循环：输入一条条处理；没输入时睡到下一次定时拍（`next_deadline`：最早的未来 not_before、退避 / 错开的
/// 时刻、待决策升级时刻，都没有就心跳）。停机完成就结束（之后句柄的调用都回 `ShuttingDown`）。
/// 心跳另有一个固定节拍的 `interval`（评审 I2）：每条输入都会让「睡到下一拍」重新起算，输入比心跳勤的忙 actor 光靠它
/// 一拍都等不到；`interval` 不随输入重置，所以默认方案到点、清旧唤醒、已过点的唤醒重扫至少每个心跳一次。
async fn run<P: Port>(mut core: Core<P>, mut rx: mpsc::UnboundedReceiver<Input>) {
    let hb = Duration::from_millis(core.cfg.heartbeat_ms.max(1) as u64);
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + hb, hb);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let wait = core.next_deadline().await.since(core.now()).max(0) as u64;
        tokio::select! {
            input = rx.recv() => match input {
                Some(i) => core.handle(i).await,
                // Core 自己拿着一个发送端（派生任务回报用），所以这里其实收不到 None；收到了也照样收尾。
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {
                core.handle(Input::Tick).await;
                // 刚拍过：心跳从这里重新起算（9b 复审），不然两路一前一后各拍一次。
                heartbeat.reset();
            }
            _ = heartbeat.tick() => core.handle(Input::Tick).await,
        }
        if core.shutdown_done() {
            return;
        }
    }
}

/// actor 已经结束（停机 / 休眠收尾，或崩了）之后句柄的每个调用：`ShuttingDown`（-32009 `orchestra.shutting_down`，
/// 修复轮 I1）——客户端看到的是「crewd 正在停」，不是一个编号的内部错误（休眠之后 serve 还在把回执写出去、
/// 收尾的那一小段里，晚到的请求就落在这里）。
fn stopped() -> ControlError {
    ControlError::ShuttingDown
}

#[cfg(feature = "testkit")]
pub struct TestActorPause(Option<oneshot::Sender<()>>);
#[cfg(feature = "testkit")]
impl Drop for TestActorPause {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

impl Handle {
    /// Exact actor mailbox boundary for transaction-fault integration tests.
    /// This grants no caller identity and exposes no wire operation.
    #[cfg(feature = "testkit")]
    pub async fn pause_for_test(&self) -> Result<TestActorPause, ControlError> {
        let (entered, ready) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        self.tx
            .send(Input::PauseForTest {
                entered,
                release: wait,
            })
            .map_err(|_| stopped())?;
        ready.await.map_err(|_| stopped())?;
        Ok(TestActorPause(Some(release)))
    }

    #[cfg(test)]
    pub(crate) fn panic_for_serve_test(&self) {
        self.test_panic.send(()).unwrap();
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Input::Cmd(make(tx))).map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())?
    }

    pub(crate) async fn submit_public(
        &self,
        call: crate::public_execution::PublicExecution,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::PublicExecution {
                call: Box::new(call),
                reply,
            }))
            .map_err(|_| rpc::rpc_err(stopped()))?;
        response.await.map_err(|_| rpc::rpc_err(stopped()))?
    }

    pub(crate) async fn public_target(
        &self,
        session: &str,
    ) -> Result<crewd_proto::commands::CommandRuntimeTarget, ControlError> {
        let session = session.to_owned();
        self.call(|reply| Cmd::PublicTarget { session, reply })
            .await
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

    /// nudge（[`Core`] 的 `nudge`）。文字的 `Ok` = 已排进会话，不是已敲进去：输入框里有人的草稿时执行器挡着它，
    /// 等草稿清掉再写（T10d fix M4）。
    pub async fn nudge(&self, session: &str, input: String) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Nudge {
            session,
            input,
            reply,
        })
        .await
    }

    /// 回答屏上的待决策框、选第 `choice` 项（0 起；T10d，T10c 的 `session.answer` 用它）。
    pub async fn answer(
        &self,
        session: &str,
        fingerprint: &str,
        choice: usize,
    ) -> Result<AnswerReport, ControlError> {
        let (session, fingerprint) = (session.to_string(), fingerprint.to_string());
        self.call(|reply| Cmd::Answer {
            session,
            fingerprint,
            choice,
            reply,
        })
        .await
    }

    pub async fn interrupt(&self, session: &str) -> Result<(), ControlError> {
        self.call(|reply| Cmd::Interrupt {
            session: session.into(),
            reply,
        })
        .await
    }
    pub async fn respond_request(
        &self,
        session: &str,
        fingerprint: &str,
        response: serde_json::Value,
    ) -> Result<AnswerReport, ControlError> {
        let (session, fingerprint) = (session.to_string(), fingerprint.to_string());
        self.call(|reply| Cmd::RespondRequest {
            session,
            fingerprint,
            response,
            reply,
        })
        .await
    }

    pub(crate) async fn terminal_driver(
        &self,
        session: &str,
    ) -> Result<Option<crewd_exec::driver::SessionHandle>, ControlError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::Handle {
                session: session.into(),
                reply,
            }))
            .map_err(|_| stopped())?;
        Ok(rx
            .await
            .map_err(|_| stopped())?
            .and_then(|handle| handle.terminal_driver()))
    }
    pub(crate) async fn codex_handle(
        &self,
        session: &str,
    ) -> Result<Option<Arc<dyn exec::ExecHandle>>, ControlError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::Handle {
                session: session.into(),
                reply,
            }))
            .map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())
    }
    pub(crate) async fn codex_metadata(
        &self,
        req: crewd_exec::codex::metadata::MetadataRequest,
    ) -> crewd_exec::codex::control::CodexOutcome {
        match self.execs.get("codex") {
            Ok(e) => e.codex_metadata(req).await,
            Err(_) => crewd_exec::codex::control::CodexOutcome::unsupported(),
        }
    }
    pub async fn runner_request(
        &self,
        session: &str,
    ) -> Result<Option<crewd_exec::codex::human::RunnerRequest>, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::Handle {
                session: session.into(),
                reply: tx,
            }))
            .map_err(|_| stopped())?;
        match rx.await.map_err(|_| stopped())? {
            Some(h) => h.runner_request().await.map_err(|_| stopped()),
            None => Ok(None),
        }
    }

    /// Pure executor metadata; this does not enter the actor or launch a provider.
    pub fn executor_profiles(&self) -> Vec<crewd_proto::executors::ExecutorProfile> {
        self.execs.profiles()
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

    pub async fn decisions(
        &self,
    ) -> Result<Vec<crewd_proto::orchestra::PendingDecisionView>, ControlError> {
        self.call(|reply| Cmd::Decisions { reply }).await
    }
    /// T11 binary watcher supplies a crew for home-human routing. No watcher is installed here.
    pub fn binary_replaced(&self, crew: &str) {
        let _ = self.tx.send(Input::BinaryReplaced { crew: crew.into() });
    }

    /// `wake.list`：唤醒行（新的在前，至多 `WAKE_LIST_LIMIT` 条）、每条 pending 为什么还没出去、每个目标的近况。
    pub async fn wakes(&self, session: Option<String>) -> Result<deliver::WakeList, ControlError> {
        self.call(|reply| Cmd::Wakes { session, reply }).await
    }

    /// 人解熔断（只给客户端 RPC 用，agent 没有入口）。回 false = 本来就没有这条熔断。
    pub async fn recover(&self, scope: LatchScope, id: String) -> Result<bool, ControlError> {
        self.call(|reply| Cmd::Recover { scope, id, reply }).await
    }

    /// 此刻的环路熔断（`loop.list`）。
    pub async fn latches(&self) -> Result<Vec<crewd_wake::Latch>, ControlError> {
        self.call(|reply| Cmd::Latches { reply }).await
    }

    pub async fn causal_holds(
        &self,
        target: Option<String>,
    ) -> Result<Vec<crewd_collab::causal::ChainHold>, ControlError> {
        self.call(|reply| Cmd::CausalHolds { target, reply }).await
    }

    /// 人的精确链恢复动作在本次请求内固定，重试不生成另一个动作。
    pub async fn recover_chain(
        &self,
        chain_id: String,
        from: String,
        target: String,
        action: crewd_collab::causal::HumanRecoveryAction,
    ) -> Result<bool, ControlError> {
        self.call(|reply| Cmd::RecoverChain {
            chain_id,
            from,
            target,
            action,
            reply,
        })
        .await
    }

    /// 设置（`settings.get`）。
    pub async fn settings(&self) -> Result<SettingsView, ControlError> {
        self.call(|reply| Cmd::Settings { reply }).await
    }

    /// 改设置（`settings.set`）：回改完的设置。
    pub async fn set_settings(&self, update: SettingsUpdate) -> Result<SettingsView, ControlError> {
        self.call(|reply| Cmd::SetSettings { update, reply }).await
    }

    /// 一条消息叫醒了谁、走到哪了（`message.receipts`）。
    pub async fn receipts(
        &self,
        message_id: &str,
    ) -> Result<Vec<deliver::ReceiptRow>, ControlError> {
        let message_id = message_id.to_string();
        self.call(|reply| Cmd::Receipts { message_id, reply }).await
    }

    /// serve 该退出了（T11 消费它；U6「先休眠再更新」）。语义（修复轮 I3）：
    /// - 值变 `true` = 休眠回执已经交给等它的 oneshot（不论全退没有）。**此刻回执还没写到 socket 上**：RPC 那一路
    ///   还要把它序列化、写给客户端。所以 serve 收到它之后必须先让在途的回复写完（排空各连接正在处理的请求），
    ///   再走停机次序退出——否则调 `crewd.hibernate` 的客户端可能收不到回执。
    /// - `changed()` 回 `Err`（发送端没了）= actor 已经结束而没有休眠（普通停机收尾，或崩了）：同样意味着退出；
    ///   是不是崩了看 [`Actor::finished`]。
    /// - 之后句柄的调用都回 `ShuttingDown`（见 `stopped`）。
    pub fn exit_requested(&self) -> watch::Receiver<bool> {
        self.exit.clone()
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

    /// 测试用：actor 此刻的下一次定时拍；actor 没了 = None。
    #[cfg(test)]
    pub async fn next_deadline(&self) -> Option<crewd_wake::Ms> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::NextDeadline { reply: tx }))
            .ok()?;
        rx.await.ok()
    }

    /// 休眠（C26）：不再投 / 拉起；空闲的立刻停，在干活的等到 `deadline`（None = 设置里的，没设 = 120 秒；0 = 立即）
    /// 做完这一轮、到期限还没做完的先打断再停；唤醒一条不丢；每个 session 记一份休眠记录（下次启动的询问与附言据它）。
    /// 回执在全部退出时（`completed = true`，之后 actor 随停机收尾）或到了「期限 + 叫停宽限」时（`completed = false`，
    /// 名单里有 `failed`）发出——后一种 serve 照 [`Handle::shutdown`] 回 false 的办法收尾（[`Actor::abort_handle`] 再关库）。
    /// 已在休眠时再调 = 并进那一次（同一份结果），期限只会提前。普通停机进行中 = `ShuttingDown`。
    /// RPC `crewd.hibernate { deadlineSecs? }` 由 T10 接到这里；回执之后 serve 走自己的停机次序退出。
    pub async fn hibernate(
        &self,
        deadline: Option<Duration>,
    ) -> Result<hibernate::HibernateReport, ControlError> {
        self.enqueue_hibernate(deadline)?
            .await
            .map_err(|_| stopped())?
    }

    /// Enqueue irreversibly before a connection releases its request reader.
    /// Dropping a reply waiter never cancels the already admitted stop.
    pub(crate) fn enqueue_hibernate(
        &self,
        deadline: Option<Duration>,
    ) -> Result<oneshot::Receiver<Result<hibernate::HibernateReport, ControlError>>, ControlError>
    {
        let deadline_ms = deadline.map(|d| d.as_millis().min(i64::MAX as u128) as i64);
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Input::Cmd(Cmd::Hibernate { deadline_ms, reply }))
            .map_err(|_| stopped())?;
        Ok(response)
    }

    /// 停机：叫停全部会话（crewd 叫停，不是人停：desired / was_working 不动，带着的唤醒原样退回），最多等 `limit`。
    /// `limit` 就是期限（T9b 的休眠在这之上加「先等安全点」，C26）。返回 true = 期限内全部退出、actor 收尾了；
    /// false = 到期限还有会话没退（或 actor 已经没了）——这时 serve 用 [`Actor::abort_handle`] 中止 actor 再关库；
    /// 那时还在投的唤醒行留在 inflight，下次启动按一次失败的尝试回收（C25）。
    pub async fn shutdown(&self, limit: Duration) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Input::Shutdown(tx)).is_err() {
            return false;
        }
        matches!(tokio::time::timeout(limit, rx).await, Ok(Ok(())))
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
                codex_options: None,

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
        let (h, actor) = spawn(
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
        assert!(h.shutdown(Duration::from_secs(5)).await, "期限内收尾完了");
        assert_eq!(exec.lock().stops, std::slice::from_ref(&row.id));
        assert!(
            h.live(&row.id).await.is_err(),
            "停机完了 actor 就结束了：句柄回 ShuttingDown"
        );
        assert!(actor.finished().await.is_ok(), "正常收尾");
    }

    /// 评审 Fix 2：停机到期限还有会话没退——`shutdown` 照实回 false，serve 用 `abort_handle` 中止 actor 再关库；
    /// 我们自己中止的不算崩（`finished` 回 Ok），之后句柄回错。
    #[tokio::test]
    async fn a_timed_out_shutdown_says_so_and_the_actor_can_be_aborted() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let row = port
            .insert_session(port::NewSession {
                codex_options: None,

                crew_id: "c1".into(),
                captain: false,
                executor: "claude".into(),
                brief: String::new(),
                title: Some("w1".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        let exec = StandIn::default();
        exec.lock().hang_on_stop.insert(row.id.clone());
        let (h, actor) = spawn(
            port.clone(),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        for _ in 0..200 {
            if h.live(&row.id).await.unwrap().running {
                break;
            }
            tokio::task::yield_now().await;
        }
        let abort = actor.abort_handle();
        assert!(
            !h.shutdown(Duration::from_millis(50)).await,
            "期限到了它还没退：照实说"
        );
        abort.abort();
        let r = tokio::time::timeout(Duration::from_secs(10), actor.finished())
            .await
            .expect("中止之后 actor 要结束");
        assert!(r.is_ok(), "自己中止的不算崩：{r:?}");
        assert!(h.live(&row.id).await.is_err());
    }

    /// C26：经句柄休眠——在干活的当场打断（期限 0）、空闲的停下；结果经句柄回来，之后 actor 正常收尾，句柄回错。
    #[tokio::test]
    async fn hibernate_through_the_handle_reports_and_the_actor_winds_down() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let mut ids = Vec::new();
        for (title, captain) in [("cap", true), ("w1", false)] {
            let row = port
                .insert_session(port::NewSession {
                    codex_options: None,

                    crew_id: "c1".into(),
                    captain,
                    executor: "claude".into(),
                    brief: String::new(),
                    title: Some(title.into()),
                    model: None,
                    effort: None,
                    worktree: None,
                })
                .await
                .unwrap();
            ids.push(row.id);
        }
        let (cap, w1) = (ids[0].clone(), ids[1].clone());
        let exec = StandIn::default();
        let (h, actor) = spawn(
            port.clone(),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        for id in &ids {
            h.resume(id, Origin::Human).await.unwrap();
        }
        for _ in 0..200 {
            if h.live(&cap).await.unwrap().running && h.live(&w1).await.unwrap().running {
                break;
            }
            tokio::task::yield_now().await;
        }
        exec.emit(&w1, crewd_exec::session::SessionEvent::Working(true));
        for _ in 0..200 {
            if h.live(&w1).await.unwrap().working {
                break;
            }
            tokio::task::yield_now().await;
        }
        let exit = h.exit_requested();
        assert!(!*exit.borrow(), "休眠回执之前 serve 不退");
        // 有期限地等：休眠一直不回报（变异 m4：期限永远不到）要红，不能把测试挂住。
        let r = tokio::time::timeout(Duration::from_secs(10), h.hibernate(Some(Duration::ZERO)))
            .await
            .expect("期限 0：应当立刻回报")
            .unwrap();
        assert_eq!(
            r,
            hibernate::HibernateReport {
                clean: vec![cap],
                interrupted: vec![w1.clone()],
                failed: vec![],
                completed: true,
            }
        );
        assert!(*exit.borrow(), "回执发出去了：serve 据此收尾退出（T11）");
        assert_eq!(exec.lock().interrupts, [w1]);
        let done = tokio::time::timeout(Duration::from_secs(10), actor.finished())
            .await
            .expect("休眠完了 actor 收尾");
        assert!(done.is_ok());
        assert!(h.hibernate(None).await.is_err(), "actor 没了：句柄回错");
    }

    /// 评审 I2：心跳不被输入挤掉——actor 每 10 秒收一条输入（比心跳 30 秒勤），定时拍照样按心跳来（默认方案到点、
    /// 清旧唤醒、重扫都挂在它上面）。原先每条输入都把「睡到下一拍」重新起算，忙着的 actor 一拍都等不到。
    #[tokio::test(start_paused = true)]
    async fn a_busy_actor_still_gets_its_heartbeat_ticks() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let row = port
            .insert_session(port::NewSession {
                codex_options: None,

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
        let (h, _actor) = spawn(
            port.clone(),
            Executors::new(),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let before = port.lock().defaults_applied;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            h.live(&row.id).await.unwrap();
        }
        let ticks = port.lock().defaults_applied - before;
        assert!(ticks >= 3, "120 秒里心跳 30 秒一拍，却只拍了 {ticks} 次");
    }

    /// C25：Jev 的决策预算缺省 2 秒（之后按群主兜底）。
    #[test]
    fn default_arbiter_budget_is_two_seconds() {
        assert_eq!(
            OrchestraConfig::default().arbiter_timeout,
            Duration::from_secs(2)
        );
    }

    /// 9b 复审：精确的定时拍（睡到下一个 not_before）拍过之后心跳重新起算——不会在它后面紧跟着再来一拍。
    /// 一条 20 秒后才到点的唤醒（时钟停着，所以它一直在 20 秒后）：95 秒里只有那四拍（20/40/60/80），心跳一次都插不进来。
    #[tokio::test(start_paused = true)]
    async fn an_exact_tick_restarts_the_heartbeat_so_ticks_do_not_double() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let row = port
            .insert_session(port::NewSession {
                codex_options: None,

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
        port.enqueue(crewd_wake::NewWake {
            session: row.id.clone(),
            kind: crewd_wake::WakeKind::Timer,
            source: crewd_wake::source::timer("later"),
            priority: crewd_wake::Priority::Auto,
            not_before: T0.add(20_000),
            note: None,
            origin: row.id.clone(),
        })
        .await
        .unwrap();
        let (_h, _actor) = spawn(
            port.clone(),
            Executors::new(),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let before = port.lock().defaults_applied;
        tokio::time::sleep(Duration::from_secs(95)).await;
        let ticks = port.lock().defaults_applied - before;
        assert_eq!(ticks, 4, "20/40/60/80 秒各一拍，心跳不再紧跟着多拍");
    }

    struct PanickingQuota;
    impl hooks::QuotaSource for PanickingQuota {
        fn reset_at<'a>(
            &'a self,
            _e: &'a str,
            _now: crewd_wake::Ms,
        ) -> exec::BoxFut<'a, Option<crewd_wake::Ms>> {
            Box::pin(async { panic!("合成：挂钩崩了") })
        }
    }

    /// C25：actor 里 panic 不能悄悄留下一个死句柄——`Actor::finished` 报出来（serve 据此非零退出、让 app 重启它），
    /// 句柄的调用回错。
    #[tokio::test]
    async fn a_panic_in_the_actor_is_surfaced() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let row = port
            .insert_session(port::NewSession {
                codex_options: None,

                crew_id: "c1".into(),
                captain: false,
                executor: "claude".into(),
                brief: String::new(),
                title: Some("w1".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        let exec = StandIn::default();
        let (h, actor) = spawn(
            port.clone(),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks {
                quota: Arc::new(PanickingQuota),
                ..Hooks::default()
            },
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        for _ in 0..200 {
            if h.live(&row.id).await.unwrap().running {
                break;
            }
            tokio::task::yield_now().await;
        }
        let quota =
            crewd_exec::health::Health::new(crewd_exec::health::HealthKind::UsageLimit, "x");
        exec.emit(
            &row.id,
            crewd_exec::session::SessionEvent::Health(Some(quota)),
        );
        let died = tokio::time::timeout(Duration::from_secs(10), actor.finished())
            .await
            .expect("actor 应该已经结束")
            .expect_err("panic 要报出来");
        assert!(died.to_string().contains("合成：挂钩崩了"), "{died}");
        assert!(
            h.live(&row.id).await.is_err(),
            "死了的 actor：句柄回错，不挂住"
        );
    }

    /// 修复轮 I1 / I3：休眠收尾、actor 结束之后，句柄的调用（再休眠一次、读设置、wake.list）都回 `ShuttingDown`
    /// （线上 -32009 `orchestra.shutting_down`），不是编号的内部错误；`exit_requested` 读到 true，发送端随 actor 没了
    /// 之后 `changed()` 回 Err（同样意味着退出）。
    #[tokio::test]
    async fn after_the_actor_has_exited_every_call_says_shutting_down() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let (h, actor) = spawn(
            port,
            Executors::new(),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let mut exit = h.exit_requested();
        let r = h.hibernate(None).await.unwrap();
        assert!(r.completed);
        assert!(actor.finished().await.is_ok());
        assert!(*exit.borrow_and_update(), "回执之后读到 true");
        assert!(exit.changed().await.is_err(), "actor 没了：发送端随之没了");
        assert_eq!(
            h.hibernate(Some(Duration::ZERO)).await,
            Err(ControlError::ShuttingDown),
            "第二次休眠"
        );
        assert_eq!(h.settings().await, Err(ControlError::ShuttingDown));
        let e = h.wakes(None).await.unwrap_err();
        assert_eq!(e, ControlError::ShuttingDown);
        let wire = rpc::rpc_err(e);
        assert_eq!(
            (wire.code, wire.data.map(|d| d.refusal)),
            (
                crewd_proto::RpcError::CONFLICT,
                Some("orchestra.shutting_down".into())
            )
        );
    }

    /// 修复轮 I3：没休眠、普通停机收尾的 actor：值一直是 false，`changed()` 回 Err——同样是「退出」。
    #[tokio::test]
    async fn exit_requested_errs_when_the_actor_ends_without_hibernating() {
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let (h, actor) = spawn(
            port,
            Executors::new(),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let mut exit = h.exit_requested();
        assert!(h.shutdown(Duration::from_secs(5)).await);
        assert!(actor.finished().await.is_ok());
        assert!(!*exit.borrow());
        assert!(exit.changed().await.is_err());
    }
}
