//! Codex 编排适配器。库入口默认 Disabled，CLI 可指定路径或发现程序；真实应用验收独立进行。
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
use std::os::unix::ffi::OsStrExt;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Metadata only: never invoke the configured executable to establish readiness.
fn checked_program_path(path: &Path) -> Result<PathBuf, LaunchError> {
    let unavailable = || {
        LaunchError::NotConfigured(format!(
            "codex executable {} is unavailable or not executable",
            path.display()
        ))
    };
    let program = std::fs::canonicalize(path).map_err(|_| unavailable())?;
    if !std::fs::metadata(&program)
        .map_err(|_| unavailable())?
        .is_file()
    {
        return Err(unavailable());
    }
    let name = std::ffi::CString::new(program.as_os_str().as_bytes()).map_err(|_| unavailable())?;
    // SAFETY: the owned CString is NUL terminated and remains alive for access(2).
    if unsafe { libc::access(name.as_ptr(), libc::X_OK) } != 0 {
        return Err(unavailable());
    }
    Ok(program)
}

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
            CodexProgram::Path(path) => checked_program_path(path)?,
            CodexProgram::Discover => paths.resolve("codex").ok_or_else(|| {
                LaunchError::NotConfigured(format!("codex not found in PATH: {child_path}"))
            })?,
            CodexProgram::Disabled => unreachable!(),
        };
        Ok((program, child_path.clone()))
    }
}
impl Executor for CodexExecutor {
    fn codex_metadata(
        &self,
        req: crewd_exec::codex::metadata::MetadataRequest,
    ) -> BoxFut<'_, crewd_exec::codex::control::CodexOutcome> {
        Box::pin(async move {
            use crewd_exec::codex::{
                control::CodexOutcome,
                transport::{CodexProcessSpec, strip_billing_keys},
            };
            if let Err(e) = self.check_launch() {
                return CodexOutcome::Rejected(e.to_string());
            }
            let (program, path) = match self.program().await {
                Ok(v) => v,
                Err(e) => return CodexOutcome::Rejected(e.to_string()),
            };
            let mut env = crewd_exec::env::build_env_from_process(&path, &self.config.extra_env);
            strip_billing_keys(&mut env);
            let spec = CodexProcessSpec {
                program,
                args: vec!["app-server".into()],
                env,
                cwd: req.workdir.clone(),
            };
            crewd_exec::codex::metadata::run(spec, self.config.registry.clone(), req).await
        })
    }

    fn accepts_initial_context(&self) -> bool {
        true
    }
    fn profile(&self) -> Option<crewd_proto::executors::ExecutorProfile> {
        use crewd_proto::executors::{
            ExecutorBackend, ExecutorFeatures, ExecutorProfile, ExecutorUnavailableReason,
        };
        // Resolve only filesystem metadata; login-shell discovery belongs to an explicit launch.
        let program = match &self.config.program {
            CodexProgram::Disabled => None,
            CodexProgram::Path(path) => Some(path.clone()),
            CodexProgram::Discover => {
                let path = self
                    .child_path
                    .get()
                    .cloned()
                    .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
                AgentPaths { child_path: path }.resolve("codex")
            }
        };
        let configured = program
            .as_deref()
            .is_some_and(super::exec::executable_metadata);
        let reason = if !configured {
            Some(ExecutorUnavailableReason::NotConfigured)
        } else if self.check_launch().is_err() {
            Some(ExecutorUnavailableReason::BinaryChanged)
        } else {
            None
        };
        Some(ExecutorProfile {
            id: "codex".into(),
            executor: "codex".into(),
            display_name: "Codex".into(),
            backend: ExecutorBackend::Codex,
            available: reason.is_none(),
            features: ExecutorFeatures {
                initial_context: true,
                manual_terminal: false,
                structured_decisions: true,
                full_history_load: false,
            },
            unavailable_reason: reason,
        })
    }
    fn check_launch(&self) -> Result<(), LaunchError> {
        if self.config.program == CodexProgram::Disabled {
            return Err(LaunchError::NotConfigured("codex is disabled".into()));
        }
        if !crate::build_info::exe().is_some_and(crate::build_info::ExeIdentity::still_on_disk) {
            return Err(LaunchError::BinaryReplaced);
        }
        if let CodexProgram::Path(path) = &self.config.program {
            checked_program_path(path)?;
        }
        Ok(())
    }
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        Box::pin(async move {
            self.check_launch()?;
            let (program, child_path) = self.program().await?;
            self.check_launch()?;
            let initial_inputs=if let Some(inputs)=req.initial_inputs {
                if !inputs.skills.is_empty(){
                    let result=self.codex_metadata(crewd_exec::codex::metadata::MetadataRequest{session_id:req.session_id.clone(),thread_id:String::new(),workdir:req.workdir.clone(),operation:crewd_exec::codex::control::CodexOperation::Tools,permit:None}).await;
                    match result {crewd_exec::codex::control::CodexOutcome::Observed(catalog)=>crewd_exec::codex::input::qualify_skills(&inputs.skills,&catalog,&req.workdir).map_err(LaunchError::Failed)?,_=>return Err(LaunchError::Failed("native skill catalog unavailable; no first input sent".into()))}
                }
                Some(inputs.prepare(&self.config.runtime_dir).map_err(LaunchError::Failed)?)
            }else{None};
            self.check_launch()?;
            let config = CodexConfig {
                registry: self.config.registry.clone(),
                runtime_dir: self.config.runtime_dir.clone(),
                delivery_observer: req.delivery_observer,
                ..Default::default()
            };
            let workdir=req.workdir.clone();
            let start = CodexStart {
                session_id: req.session_id.clone(),
                program,
                child_path,
                ledger: req.ledger,
                spec: CodexSpec {
                    workdir: req.workdir,
                    title: req.title,
                    initial_settings: req.codex_options,
                    strict_resume: true,
                    model: req.model,
                    effort: req.effort,
                    developer_instructions: req.system_prompt,
                    initial_prompt: req.initial_prompt,
                    initial_inputs,
                    initial_context: req.initial_context,
                    comms: Some(CommsSpec {
                        helper: self.config.helper.clone(),
                        data_dir: self.config.data_dir.clone(),
                        session_id: req.session_id,
                        token: req.token,
                        hooks: false,
                        managed_hooks: false,
                    }),
                    additional_env: self.config.extra_env.clone(),
                    ..Default::default()
                },
            };
            let (handle, events) = start_codex(start, config)
                .await
                .map_err(LaunchError::Start)?;
            Ok((Arc::new(CodexHandle(handle,self.config.runtime_dir.clone(),workdir)) as Arc<dyn ExecHandle>, events))
        })
    }
}

/// Codex 接收结构化 additionalContext；不改变 Claude 的句柄能力。
struct CodexHandle(SessionHandle,PathBuf,PathBuf);
impl ExecHandle for CodexHandle {
    fn control_controlled(
        &self,
        op: crewd_exec::driver::Control,
        permit: crewd_exec::driver::ManualWritePermit,
    ) -> BoxFut<'_, Result<crewd_exec::driver::ControlOutcome, Closed>> {
        Box::pin(self.0.control_controlled(op, permit))
    }
    fn fence_inputs(&self) {
        self.0.fence_provider_input();
    }
    fn codex_controlled(
        &self,
        op: crewd_exec::codex::control::CodexOperation,
        permit: crewd_exec::driver::ManualWritePermit,
    ) -> BoxFut<'_, Result<crewd_exec::codex::control::CodexOutcome, Closed>> {
        Box::pin(self.0.codex_controlled(op, permit))
    }
    fn codex_submit_inputs(&self,text:String,request_id:String,expected_turn_id:Option<String>,inputs:crewd_exec::codex::input::Submission,permit:crewd_exec::driver::ManualWritePermit)->BoxFut<'_,Result<crewd_exec::codex::control::CodexOutcome,Closed>> {
        Box::pin(async move {
            use crewd_exec::codex::{control::{CodexOperation as Op,CodexOutcome as Out},input};
            if !inputs.skills.is_empty(){
                match self.0.codex(Op::Tools).await? {
                    Out::Observed(catalog)=>if let Err(e)=input::qualify_skills(&inputs.skills,&catalog,&self.2){return Ok(Out::Rejected(e))},
                    _=>return Ok(Out::Rejected("native skill catalog unavailable".into())),
                }
            }
            let root=self.1.clone();
            let inputs=match tokio::task::spawn_blocking(move||inputs.prepare(&root)).await {
                Ok(Ok(v))=>v,Ok(Err(e))=>return Ok(Out::Rejected(e)),Err(_)=>return Ok(Out::Rejected("native input materialization unavailable".into())),
            };
            self.0.codex_prepared_controlled(text,request_id,expected_turn_id,inputs,permit).await
        })
    }
    fn codex_queue_controlled(
        &self,
        text: String,
        request_id: String,
        permit: crewd_exec::driver::ManualWritePermit,
    ) -> BoxFut<'_, Result<crewd_exec::codex::control::CodexOutcome, Closed>> {
        Box::pin(self.0.codex_queue_controlled(text, request_id, permit))
    }

    fn codex_queue(
        &self,
        text: String,
        request_id: String,
    ) -> BoxFut<'_, Result<crewd_exec::codex::control::CodexOutcome, Closed>> {
        Box::pin(self.0.codex_queue(text, request_id))
    }

    fn codex(
        &self,
        op: crewd_exec::codex::control::CodexOperation,
    ) -> BoxFut<'_, Result<crewd_exec::codex::control::CodexOutcome, Closed>> {
        Box::pin(self.0.codex(op))
    }
    fn control(
        &self,
        op: crewd_exec::driver::Control,
    ) -> BoxFut<'_, Result<crewd_exec::driver::ControlOutcome, Closed>> {
        Box::pin(self.0.control(op))
    }

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
    fn answer_choice(
        &self,
        expect: String,
        choice: usize,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(self.0.answer_choice(expect, choice))
    }
    fn respond_request(
        &self,
        expect: String,
        response: serde_json::Value,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(self.0.respond_request(expect, response))
    }
    fn runner_request(
        &self,
    ) -> BoxFut<'_, Result<Option<crewd_exec::codex::human::RunnerRequest>, Closed>> {
        Box::pin(self.0.runner_request())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn executor(program: CodexProgram, directory: &std::path::Path) -> CodexExecutor {
        CodexExecutor::new(CodexExecutorConfig {
            program,
            data_dir: directory.to_path_buf(),
            helper: std::env::current_exe().unwrap(),
            registry: None,
            extra_env: BTreeMap::new(),
            runtime_dir: directory.join("runtime"),
            child_path: Some("/usr/bin:/bin".into()),
        })
    }

    #[tokio::test]
    async fn codex_explicit_program_preflight_is_filesystem_only_and_rechecks_changes() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("codex");
        let selected = executor(CodexProgram::Path(program.clone()), directory.path());
        assert!(matches!(
            selected.check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
        std::fs::create_dir(&program).unwrap();
        assert!(matches!(
            selected.check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
        std::fs::remove_dir(&program).unwrap();
        let marker = directory.path().join("must-not-run");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            selected.check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        selected.check_launch().unwrap();
        let resolved = selected.program().await.unwrap();
        assert_eq!(resolved.0, std::fs::canonicalize(&program).unwrap());
        assert_eq!(resolved.1, "/usr/bin:/bin");
        assert!(
            !marker.exists(),
            "preflight must not execute the selected program"
        );
        std::fs::remove_file(&program).unwrap();
        symlink(&program, &program).unwrap();
        assert!(matches!(
            selected.check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
        std::fs::remove_file(&program).unwrap();
        assert!(matches!(
            selected.check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
        executor(CodexProgram::Discover, directory.path())
            .check_launch()
            .unwrap();
        assert!(matches!(
            executor(CodexProgram::Disabled, directory.path()).check_launch(),
            Err(LaunchError::NotConfigured(_))
        ));
    }
}
