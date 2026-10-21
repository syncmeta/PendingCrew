//! 执行器抽象（spec §22.4「执行器抽象」；§13 Q12 的窄接口）。claude = M2b 的 `start_claude` + `SessionHandle`；
//! codex（M2c）/ ACP（M2e）合入时各加一个 `Executor` 实现，`Executors` 里登记一行。
//! 事件流直接用 crewd-exec 的 `SessionEvent`：M2c 已合（main@e66be68），加了 `Item` / `TurnEnded` / `Notice` / `Profile`，
//! `core.rs` 的 match 显式列出它们（claude 不发，接 codex 执行器时逐个决定怎么处理）。
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crewd_exec::driver::{Closed, SendError, SessionHandle, SessionStatus, StartError};
use crewd_exec::resume::AgentIdLedger;
use crewd_exec::session::{AnswerOutcome, AnswerStep, SessionEvent};
use crewd_term::Snapshot;
use tokio::sync::mpsc;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// codex 的 `additionalContext`（按 key 组装、去重；§4.3、§22.6 的 `ContextExtras` 口）。
pub type Context = BTreeMap<String, String>;

pub struct LaunchRequest {
    pub session_id: String,
    pub crew_id: String,
    pub workdir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 世界观（`collab::worldview::render`）。
    pub system_prompt: Option<String>,
    /// 开场正文（只走输入投递，绝不进 argv）。
    pub initial_prompt: Option<String>,
    pub initial_context: Context,
    /// helper 用来 `agent.attach` 的 token（M3b）。
    pub token: String,
    pub ledger: Arc<dyn AgentIdLedger>,
    pub delivery_observer: Option<Arc<dyn crewd_exec::driver::DeliverySuccessObserver>>,
}

#[derive(Debug)]
pub enum LaunchError {
    BinaryReplaced,
    /// 这种执行器还没接进 crewd（codex 在 M2c 合入前）。
    Unsupported(String),
    /// 本 crewd 没配置这个执行器的可执行文件。
    NotConfigured(String),
    Start(StartError),
    Failed(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinaryReplaced => f.write_str("crewd executable was replaced; restart required"),
            Self::Unsupported(kind) => write!(f, "执行器 {kind} 还没接入 crewd"),
            Self::NotConfigured(why) | Self::Failed(why) => write!(f, "{why}"),
            Self::Start(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LaunchError {}

pub type Launched = (Arc<dyn ExecHandle>, mpsc::UnboundedReceiver<SessionEvent>);

pub trait Executor: Send + Sync {
    /// Pure configuration/filesystem metadata. Never initialize a provider here.
    fn profile(&self) -> Option<crewd_proto::executors::ExecutorProfile> {
        None
    }

    /// Capability available before launching the first turn.
    fn accepts_initial_context(&self) -> bool {
        false
    }
    /// Synchronous launch admission; must not enter the actor mailbox.
    fn check_launch(&self) -> Result<(), LaunchError> {
        Ok(())
    }
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>>;

    /// 续跑被拒时多说一句诊断：这个会话号的日志在不在盘上（只用来说明，不决定任何事；
    /// `AgentSessionResume.swift:136-163`）。
    fn locate_transcript(&self, _agent_session_id: &str) -> Option<PathBuf> {
        None
    }
}

/// `ExecHandle::send` 没收下：执行器自己的原因，或调用方给了这个执行器不收的 `context`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecSendError {
    Send(SendError),
    /// `context` 非空、但这个执行器不收 additionalContext（调用方该先看 `accepts_context()`）。
    ContextUnsupported,
}

impl From<SendError> for ExecSendError {
    fn from(e: SendError) -> Self {
        Self::Send(e)
    }
}

impl std::fmt::Display for ExecSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Send(e) => write!(f, "{e}"),
            Self::ContextUnsupported => write!(f, "这个执行器不收 additionalContext"),
        }
    }
}

impl std::error::Error for ExecSendError {}

/// 不收 context 的执行器拿到非空 context → 错误（不 panic：actor 里的 panic 会停掉全部会话）。
pub fn check_context(accepts: bool, context: &Context) -> Result<(), ExecSendError> {
    if accepts || context.is_empty() {
        Ok(())
    } else {
        Err(ExecSendError::ContextUnsupported)
    }
}

/// 回答待决策框的一串键：（字节, 按之前高亮应在第几项；`None` = 不核）。
pub type AnswerSteps = Vec<(Vec<u8>, Option<usize>)>;

/// §13 Q12 的窄接口（启动之外）。
pub trait ExecHandle: Send + Sync {
    /// `context` 只能在 `accepts_context()` 为真时非空，否则回 `ContextUnsupported`（见 [`check_context`]）。
    fn send(&self, text: String, context: &Context) -> Result<u64, ExecSendError>;
    fn send_bound(
        &self,
        _text: String,
        _context: &Context,
        _binding: &dyn crewd_exec::driver::DeliveryBinding,
    ) -> Result<u64, ExecSendError> {
        Err(ExecSendError::Send(
            crewd_exec::driver::SendError::BindingRejected,
        ))
    }
    fn terminal_driver(&self) -> Option<SessionHandle> {
        None
    }
    fn accepts_context(&self) -> bool {
        false
    }
    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed>;
    /// 打断当前回合（claude：一个 Esc；codex：`turn/interrupt`；没有进行中的回合就什么都不做）。休眠到了期限、
    /// 还没做完这一轮的 session 先打断再叫停（C26）。
    fn interrupt(&self) -> Result<(), Closed>;
    /// 回答屏上的待决策框（T10d 安全按键）：`expect` = 框的指纹，`steps` = (字节, 按之前高亮应在第几项)。键之间的间隔、
    /// 每一下之前的回读都在执行器里；编排层只等结果，不 sleep。codex 回 `Unsupported`（M3d）。
    fn answer(
        &self,
        expect: String,
        steps: AnswerSteps,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>>;
    /// Structured runner request: zero-based option index. Never fall back to keys.
    /// M3d must bind this to the pending JSON-RPC request.
    fn answer_choice(
        &self,
        _expect: String,
        _choice: usize,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(async { Ok(AnswerOutcome::Unsupported) })
    }
    fn respond_request(
        &self,
        _expect: String,
        _response: serde_json::Value,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(async { Ok(AnswerOutcome::Unsupported) })
    }
    fn runner_request(
        &self,
    ) -> BoxFut<'_, Result<Option<crewd_exec::codex::human::RunnerRequest>, Closed>> {
        Box::pin(async { Ok(None) })
    }
    fn stop(&self) -> Result<(), Closed>;
    fn clear_quota_health(&self) -> Result<(), Closed>;
    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>>;
    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>>;
    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>>;
}

/// claude（M2b）。上下文由 hook 注入（M3b），不收 additionalContext。
impl ExecHandle for SessionHandle {
    fn terminal_driver(&self) -> Option<SessionHandle> {
        self.manual_pty().then(|| self.clone())
    }
    fn send(&self, text: String, context: &Context) -> Result<u64, ExecSendError> {
        check_context(self.accepts_context(), context)?;
        Ok(SessionHandle::send(self, text)?)
    }

    fn send_bound(
        &self,
        text: String,
        context: &Context,
        binding: &dyn crewd_exec::driver::DeliveryBinding,
    ) -> Result<u64, ExecSendError> {
        check_context(self.accepts_context(), context)?;
        Ok(SessionHandle::send_bound(
            self,
            text,
            context.clone(),
            binding,
        )?)
    }

    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        SessionHandle::raw(self, bytes)
    }

    fn interrupt(&self) -> Result<(), Closed> {
        SessionHandle::interrupt(self)
    }

    fn answer(
        &self,
        expect: String,
        steps: AnswerSteps,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        let steps = steps
            .into_iter()
            .map(|(bytes, selected_before)| AnswerStep {
                bytes,
                selected_before,
            })
            .collect();
        Box::pin(SessionHandle::answer(self, expect, steps))
    }

    fn stop(&self) -> Result<(), Closed> {
        SessionHandle::stop(self)
    }

    fn clear_quota_health(&self) -> Result<(), Closed> {
        SessionHandle::clear_quota_health(self)
    }

    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(SessionHandle::screen_text(self, max_lines))
    }

    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>> {
        // M2c 已合（main@e66be68）：`SessionHandle::snapshot` 自己返回 `Result<Option<Snapshot>, Closed>`。
        Box::pin(SessionHandle::snapshot(self))
    }

    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(SessionHandle::status(self))
    }
}

/// 执行器类型（`sessions.executor`：`claude` / `codex` / `acp:<id>`）→ 实现。
#[derive(Clone, Default)]
pub struct Executors(
    BTreeMap<String, Arc<dyn Executor>>,
    Vec<crewd_proto::executors::ExecutorProfile>,
);

impl Executors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, kind: &str, executor: Arc<dyn Executor>) -> Self {
        self.0.insert(kind.to_string(), executor);
        self
    }

    /// Metadata-only registration for a configured but unqualified adapter.
    /// It is deliberately absent from get()/launch until its actual gate closes.
    pub fn with_unavailable(mut self, profile: crewd_proto::executors::ExecutorProfile) -> Self {
        assert!(!profile.available);
        self.1.push(profile);
        self
    }

    pub fn profiles(&self) -> Vec<crewd_proto::executors::ExecutorProfile> {
        let mut profiles: Vec<_> = self
            .0
            .values()
            .filter_map(|executor| executor.profile())
            .collect();
        profiles.extend(self.1.iter().cloned());
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        profiles
    }

    pub fn get(&self, kind: &str) -> Result<Arc<dyn Executor>, LaunchError> {
        self.0
            .get(kind)
            .cloned()
            .ok_or_else(|| LaunchError::Unsupported(kind.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 6 评审 (4)：带了 context 却发给不收它的执行器 = 调用方的错，回错误、不 panic（actor 里一个 panic 会停掉全部会话）。
    #[test]
    fn non_empty_context_to_a_handle_that_refuses_it_is_an_error_not_a_panic() {
        let mut ctx = Context::new();
        ctx.insert("my_work_items".into(), "合成".into());
        assert_eq!(
            check_context(false, &ctx),
            Err(ExecSendError::ContextUnsupported)
        );
        assert_eq!(check_context(false, &Context::new()), Ok(()));
        assert_eq!(check_context(true, &ctx), Ok(()));
        assert!(
            !ExecSendError::ContextUnsupported.to_string().is_empty(),
            "给人看的原因"
        );
        assert_eq!(
            ExecSendError::from(SendError::EmptyText).to_string(),
            SendError::EmptyText.to_string()
        );
    }

    #[test]
    fn executors_reject_kinds_that_are_not_wired_in() {
        let e = Executors::new();
        for kind in ["codex", "acp:opencode", "claude"] {
            let err = e.get(kind).err().expect("空表什么都不认");
            assert!(
                matches!(&err, LaunchError::Unsupported(k) if k == kind),
                "{err:?}"
            );
        }
    }
}

/// Metadata-only executable readiness; never execute version/login or a shell.
pub(crate) fn executable_metadata(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: owned NUL-terminated path remains live throughout access(2).
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
}
