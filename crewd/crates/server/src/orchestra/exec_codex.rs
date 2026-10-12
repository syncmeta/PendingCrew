//! Codex 编排适配器。生产登记等待待请求、事件消费和端到端验收，不提前宣告可用。
use super::exec::{
    AnswerSteps, BoxFut, Context, ExecHandle, ExecSendError, Executor, LaunchError, LaunchRequest,
    Launched,
};
use crewd_exec::{
    codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex},
    driver::{Closed, SessionHandle, SessionStatus},
    env::AgentPaths,
    launch::CommsSpec,
    registry::ProcessRegistry,
    session::AnswerOutcome,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CodexProgram {
    #[default]
    Disabled,
    Discover,
    Path(PathBuf),
}

pub struct CodexExecutorConfig {
    pub program: CodexProgram,
    pub data_dir: PathBuf,
    pub helper: PathBuf,
    pub registry: Option<Arc<ProcessRegistry>>,
    pub extra_env: BTreeMap<String, String>,
    pub runtime_dir: PathBuf,
    pub child_path: Option<String>,
}

pub struct CodexExecutor {
    config: CodexExecutorConfig,
    child_path: tokio::sync::OnceCell<String>,
}
impl CodexExecutor {
    pub fn new(mut config: CodexExecutorConfig) -> Self {
        let child_path = match config.child_path.take() {
            Some(path) => tokio::sync::OnceCell::new_with(Some(path)),
            None => tokio::sync::OnceCell::new(),
        };
        Self { config, child_path }
    }
    async fn program(&self) -> Result<(PathBuf, String), LaunchError> {
        if self.config.program == CodexProgram::Disabled {
            return Err(LaunchError::NotConfigured("codex is disabled".into()));
        }
        let child_path = self
            .child_path
            .get_or_init(|| async { AgentPaths::discover().await.child_path })
            .await;
        let paths = AgentPaths {
            child_path: child_path.clone(),
        };
        // 只缓存 PATH，每次启动重新核可执行文件，保留更新/删除后的诚实失败。
        let program = match &self.config.program {
            CodexProgram::Path(path) => std::fs::canonicalize(path).map_err(|e| {
                LaunchError::NotConfigured(format!("codex path {}: {e}", path.display()))
            })?,
            CodexProgram::Discover => paths.resolve("codex").ok_or_else(|| {
                LaunchError::NotConfigured(format!("codex not found in PATH: {child_path}"))
            })?,
            CodexProgram::Disabled => unreachable!(),
        };
        Ok((program, child_path.clone()))
    }
}
impl Executor for CodexExecutor {
    fn check_launch(&self) -> Result<(), LaunchError> {
        if crate::build_info::exe().is_some_and(crate::build_info::ExeIdentity::still_on_disk) {
            Ok(())
        } else {
            Err(LaunchError::BinaryReplaced)
        }
    }
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        Box::pin(async move {
            self.check_launch()?;
            let (program, child_path) = self.program().await?;
            self.check_launch()?;
            let config = CodexConfig {
                registry: self.config.registry.clone(),
                runtime_dir: self.config.runtime_dir.clone(),
                delivery_observer: req.delivery_observer,
                ..Default::default()
            };
            let start = CodexStart {
                session_id: req.session_id.clone(),
                program,
                child_path,
                ledger: req.ledger,
                spec: CodexSpec {
                    workdir: req.workdir,
                    model: req.model,
                    effort: req.effort,
                    developer_instructions: req.system_prompt,
                    initial_prompt: req.initial_prompt,
                    comms: Some(CommsSpec {
                        helper: self.config.helper.clone(),
                        data_dir: self.config.data_dir.clone(),
                        session_id: req.session_id,
                        token: req.token,
                        hooks: false,
                    }),
                    additional_env: self.config.extra_env.clone(),
                    ..Default::default()
                },
            };
            let (handle, events) = start_codex(start, config)
                .await
                .map_err(LaunchError::Start)?;
            Ok((Arc::new(CodexHandle(handle)) as Arc<dyn ExecHandle>, events))
        })
    }
}

/// Codex 接收结构化 additionalContext；不改变 Claude 的句柄能力。
struct CodexHandle(SessionHandle);
impl ExecHandle for CodexHandle {
    fn accepts_context(&self) -> bool {
        true
    }
    fn send(&self, text: String, context: &Context) -> Result<u64, ExecSendError> {
        Ok(self.0.send_with_context(text, context.clone())?)
    }
    fn send_bound(
        &self,
        text: String,
        context: &Context,
        binding: &dyn crewd_exec::driver::DeliveryBinding,
    ) -> Result<u64, ExecSendError> {
        Ok(self.0.send_bound(text, context.clone(), binding)?)
    }
    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        self.0.raw(bytes)
    }
    fn interrupt(&self) -> Result<(), Closed> {
        self.0.interrupt()
    }
    fn answer(
        &self,
        _expect: String,
        _steps: AnswerSteps,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(async { Ok(AnswerOutcome::Unsupported) })
    }
    fn stop(&self) -> Result<(), Closed> {
        self.0.stop()
    }
    fn clear_quota_health(&self) -> Result<(), Closed> {
        self.0.clear_quota_health()
    }
    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(self.0.screen_text(max_lines))
    }
    fn snapshot(&self) -> BoxFut<'_, Result<Option<crewd_term::Snapshot>, Closed>> {
        Box::pin(self.0.snapshot())
    }
    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(self.0.status())
    }
}
