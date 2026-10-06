//! claude 执行器：M2b 的 `start_claude` 包一层。可执行文件三种来源（裁定见 Global Constraints）：
//! `Disabled`（库里 `serve()` 的缺省：绝不起任何 agent）、`Path`（`crewd serve --claude` / 测试的假 claude）、
//! `Discover`（`crewd serve` 不给路径时按登录 shell 的 PATH 找）。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::{AgentPaths, compose_child_path};
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
}

pub struct ClaudeExecutor {
    program: ClaudeProgram,
    data_dir: PathBuf,
    helper: PathBuf,
    registry: Option<Arc<ProcessRegistry>>,
    extra_env: BTreeMap<String, String>,
    projects_dir: Option<PathBuf>,
    paths: tokio::sync::OnceCell<AgentPaths>,
}

impl ClaudeExecutor {
    pub fn new(c: ClaudeExecutorConfig) -> Self {
        Self {
            program: c.program,
            data_dir: c.data_dir,
            helper: c.helper,
            registry: c.registry,
            extra_env: c.extra_env,
            projects_dir: c.projects_dir,
            paths: tokio::sync::OnceCell::new(),
        }
    }

    /// (claude 可执行文件, 子进程 PATH)。
    async fn program(&self) -> Result<(PathBuf, String), LaunchError> {
        match &self.program {
            ClaudeProgram::Disabled => Err(LaunchError::NotConfigured(
                "这个 crewd 没配置 claude（serve 没给 --claude，也没开自动查找）".into(),
            )),
            ClaudeProgram::Path(p) => {
                let parent = std::env::var("PATH").ok();
                Ok((p.clone(), compose_child_path(&[], parent.as_deref())))
            }
            ClaudeProgram::Discover => {
                let paths = self.paths.get_or_init(AgentPaths::discover).await;
                let program = paths.resolve("claude").ok_or_else(|| {
                    LaunchError::NotConfigured(format!(
                        "PATH 里找不到 claude：{}",
                        paths.child_path
                    ))
                })?;
                Ok((program, paths.child_path.clone()))
            }
        }
    }
}

impl Executor for ClaudeExecutor {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        Box::pin(async move {
            let (program, child_path) = self.program().await?;
            let mut config = DriverConfig::new(self.data_dir.join("run"));
            config.registry = self.registry.clone();
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

/// 唯一拼 `CommsSpec` 的地方（裁定 18）。M3b 改了它的形状后，后合者只改这里。
/// 写法 A：M3b 还没合进 main（`CommsSpec` 仍是 M2b 的 `crew_id / captain / structured_post / label` 形状）。
pub fn comms_for(
    helper: &Path,
    data_dir: &Path,
    session_id: &str,
    crew_id: &str,
    token: &str,
) -> CommsSpec {
    let _ = token; // M3b 之前 helper 还不读 token
    CommsSpec {
        helper: helper.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        crew_id: crew_id.to_string(),
        session_id: session_id.to_string(),
        captain: false,
        structured_post: false,
        label: None,
        // M3b 之前 helper 没有 `hook` 子命令。
        hooks: false,
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
        })
    }

    fn request(session: &str, workdir: &Path) -> LaunchRequest {
        LaunchRequest {
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
