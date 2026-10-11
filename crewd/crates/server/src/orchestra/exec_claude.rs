//! claude 执行器：M2b 的 `start_claude` 包一层。可执行文件三种来源（裁定见 Global Constraints）：
//! `Disabled`（库里 `serve()` 的缺省：绝不起任何 agent）、`Path`（`crewd serve --claude` / 测试的假 claude）、
//! `Discover`（`crewd serve` 不给路径时按登录 shell 的 PATH 找）。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::{ClaudeSpec, CommsSpec};
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::valid_agent_session_id;

use super::exec::{BoxFut, ExecHandle, Executor, LaunchError, LaunchRequest, Launched};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ClaudeProgram {
    #[default]
    Disabled,
    Discover,
    Path(PathBuf),
}

pub struct ClaudeExecutorConfig {
    pub program: ClaudeProgram,
    pub data_dir: PathBuf,
    /// MCP / hook helper（生产 = crewd 自己的可执行文件）。
    pub helper: PathBuf,
    pub registry: Option<Arc<ProcessRegistry>>,
    /// 额外环境变量（测试给假 claude 传 `FAKE_CLAUDE_LOG`；仍过 crewd-exec 的禁用前缀）。
    pub extra_env: BTreeMap<String, String>,
    /// claude 的 `projects` 目录（认证探测与续跑被拒诊断）。
    pub projects_dir: Option<PathBuf>,
    /// Launch files; None preserves the library fixture default.
    pub runtime_dir: Option<PathBuf>,
    /// Explicit fixture PATH, or one cached login-shell discovery.
    pub child_path: Option<String>,
}

pub struct ClaudeExecutor {
    program: ClaudeProgram,
    data_dir: PathBuf,
    helper: PathBuf,
    registry: Option<Arc<ProcessRegistry>>,
    extra_env: BTreeMap<String, String>,
    projects_dir: Option<PathBuf>,
    runtime_dir: PathBuf,
    child_path: tokio::sync::OnceCell<String>,
    #[cfg(test)]
    identity_override: Option<bool>,
}

impl ClaudeExecutor {
    pub fn new(c: ClaudeExecutorConfig) -> Self {
        Self {
            #[cfg(test)]
            identity_override: None,
            program: c.program,
            data_dir: c.data_dir.clone(),
            helper: c.helper,
            registry: c.registry,
            extra_env: c.extra_env,
            projects_dir: c.projects_dir,
            runtime_dir: c.runtime_dir.unwrap_or_else(|| c.data_dir.join("run")),
            child_path: match c.child_path {
                Some(path) => tokio::sync::OnceCell::new_with(Some(path)),
                None => tokio::sync::OnceCell::new(),
            },
        }
    }

    /// (claude 可执行文件, 子进程 PATH)。
    async fn program(&self) -> Result<(PathBuf, String), LaunchError> {
        if self.program == ClaudeProgram::Disabled {
            return Err(LaunchError::NotConfigured("claude is disabled".into()));
        }
        let child_path = self
            .child_path
            .get_or_init(|| async { AgentPaths::discover().await.child_path })
            .await;
        // Cache only PATH. A program can disappear or be replaced between launches.
        let paths = AgentPaths {
            child_path: child_path.clone(),
        };
        let program = match &self.program {
            ClaudeProgram::Path(p) => std::fs::canonicalize(p).map_err(|e| {
                LaunchError::NotConfigured(format!("claude path {}: {e}", p.display()))
            })?,
            ClaudeProgram::Discover => paths.resolve("claude").ok_or_else(|| {
                LaunchError::NotConfigured(format!("claude not found in PATH: {child_path}"))
            })?,
            ClaudeProgram::Disabled => unreachable!(),
        };
        Ok((program, child_path.clone()))
    }
}

impl Executor for ClaudeExecutor {
    fn check_launch(&self) -> Result<(), LaunchError> {
        #[cfg(test)]
        if let Some(false) = self.identity_override {
            return Err(LaunchError::BinaryReplaced);
        }
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
            // Discovery can await a login shell; recheck immediately before creating launch files.
            self.check_launch()?;
            let mut config = DriverConfig::new(self.runtime_dir.clone());
            config.registry = self.registry.clone();
            config.delivery_observer = req.delivery_observer.clone();
            config.claude_projects_dir = self.projects_dir.clone();
            let comms = comms_for(
                &self.helper,
                &self.data_dir,
                &req.session_id,
                &req.crew_id,
                &req.token,
            );
            let spec = ClaudeSpec {
                workdir: req.workdir,
                model: req.model,
                effort: req.effort,
                fast_mode: None,
                system_prompt: req.system_prompt,
                initial_prompt: req.initial_prompt,
                comms: Some(comms),
                additional_env: self.extra_env.clone(),
            };
            let start = ClaudeStart {
                session_id: req.session_id,
                program,
                child_path,
                spec,
                ledger: req.ledger,
            };
            let (handle, events) = start_claude(start, config)
                .await
                .map_err(LaunchError::Start)?;
            Ok((Arc::new(handle) as Arc<dyn ExecHandle>, events))
        })
    }

    fn locate_transcript(&self, agent_session_id: &str) -> Option<PathBuf> {
        // 会话号要当路径段用：先过 M2b 的字符集校验。
        if !valid_agent_session_id(agent_session_id) {
            return None;
        }
        let root = self.projects_dir.as_ref()?;
        std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|e| e.path().join(format!("{agent_session_id}.jsonl")))
            .find(|p| p.is_file())
    }
}

/// 启动身份只带 session + token；角色与机组由 M3b 的 helper 每次向 serve 现查。
pub fn comms_for(
    helper: &Path,
    data_dir: &Path,
    session_id: &str,
    _crew_id: &str,
    token: &str,
) -> CommsSpec {
    CommsSpec {
        helper: helper.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        session_id: session_id.to_string(),
        token: token.to_string(),
        hooks: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_exec::resume::MemoryLedger;

    fn executor(program: ClaudeProgram, data_dir: &Path) -> ClaudeExecutor {
        ClaudeExecutor::new(ClaudeExecutorConfig {
            program,
            data_dir: data_dir.to_path_buf(),
            helper: PathBuf::from("/nonexistent/crewd-helper"),
            registry: None,
            extra_env: BTreeMap::new(),
            projects_dir: None,
            runtime_dir: None,
            child_path: Some("/usr/bin:/bin".into()),
        })
    }

    fn request(session: &str, workdir: &Path) -> LaunchRequest {
        LaunchRequest {
            delivery_observer: None,
            session_id: session.into(),
            crew_id: "c1".into(),
            workdir: workdir.to_path_buf(),
            model: None,
            effort: None,
            system_prompt: Some("合成世界观".into()),
            initial_prompt: None,
            token: "tok-1".into(),
            ledger: Arc::new(MemoryLedger::default()),
        }
    }

    #[tokio::test]
    async fn disabled_program_never_starts_anything() {
        let dir = tempfile::tempdir().unwrap();
        let err = executor(ClaudeProgram::Disabled, dir.path())
            .launch(request("s1", dir.path()))
            .await
            .err()
            .expect("没配置就不起");
        assert!(matches!(err, LaunchError::NotConfigured(_)), "{err}");
    }

    #[tokio::test]
    async fn t11_discovery_resolves_again_using_only_the_cached_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude");
        std::fs::write(&path, "synthetic, never executed").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut exec = executor(ClaudeProgram::Discover, dir.path());
        exec.child_path = tokio::sync::OnceCell::new_with(Some(dir.path().display().to_string()));
        assert_eq!(exec.program().await.unwrap().0, path);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            exec.program().await,
            Err(LaunchError::NotConfigured(_))
        ));
    }

    #[tokio::test]
    async fn t11_explicit_program_keeps_child_path_and_canonicalizes_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fake");
        let link = dir.path().join("claude");
        std::fs::write(&target, "synthetic, never executed").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let exec = executor(ClaudeProgram::Path(link), dir.path());
        let (program, path) = exec.program().await.unwrap();
        assert_eq!(program, target.canonicalize().unwrap());
        assert_eq!(path, "/usr/bin:/bin");
        std::fs::remove_file(target).unwrap();
        assert!(exec.program().await.is_err());
    }

    #[tokio::test]
    async fn t11_replaced_binary_refuses_before_any_launch_files_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let mut exec = executor(
            ClaudeProgram::Path(dir.path().join("never-run")),
            dir.path(),
        );
        assert!(
            exec.check_launch().is_ok(),
            "actual running test executable is present"
        );
        exec.identity_override = Some(false);
        let error = exec
            .launch(request("synthetic", dir.path()))
            .await
            .err()
            .unwrap();
        assert!(matches!(error, LaunchError::BinaryReplaced));
        assert!(!dir.path().join("run").exists());
        exec.identity_override = None;
        assert!(exec.check_launch().is_ok());
    }

    // `launches_the_given_program_with_world_model_and_comms` 要起进程：按控制者裁定 C8 放在
    // 集成测试 `tests/orchestra_exec_claude.rs`，不进 lib 测试二进制。

    #[test]
    fn transcript_lookup_scans_every_project_dir_and_refuses_path_like_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("p1")).unwrap();
        std::fs::create_dir_all(dir.path().join("p2")).unwrap();
        std::fs::write(dir.path().join("p2/abc-1.jsonl"), "{}").unwrap();
        let mut exec = executor(ClaudeProgram::Disabled, dir.path());
        exec.projects_dir = Some(dir.path().to_path_buf());
        assert_eq!(
            exec.locate_transcript("abc-1"),
            Some(dir.path().join("p2/abc-1.jsonl"))
        );
        assert_eq!(exec.locate_transcript("nope"), None);
        assert_eq!(exec.locate_transcript("../p2/abc-1"), None);
    }
}
