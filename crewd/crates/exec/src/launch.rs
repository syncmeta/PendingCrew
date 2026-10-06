//! claude 的启动参数与临时文件（Swift `SessionConfig.argv` / `LocalSessionLaunch` 的移植）。
//!
//! 两条铁律：
//! - **首条 prompt 绝不进 argv**：本机任何进程 `ps` 都看得见 argv（2026-08-28 出过事故）。
//!   它走 PTY 投递（[`crate::delivery`]）。
//! - `--permission-mode auto`，绝不 bypass。
//!
//! hooks / MCP 的命令行指向一个可配置的 helper（M3 实现 `crewd mcp` / `crewd hook`）。
//! 命令行的形状只在 [`HelperCommands`] 一处拼，M3 改子命令时只改这里。
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// claude 用哪个会话号：续跑旧的，或用我们生成的新号（两者互斥，claude 不接受同时给）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionArg {
    Resume(String),
    New(String),
}

impl AgentSessionArg {
    pub fn id(&self) -> &str {
        match self {
            Self::Resume(id) | Self::New(id) => id,
        }
    }
}

/// 一次 claude 启动的全部 argv 输入。只有值，没有文件 IO。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeArgs {
    pub session: AgentSessionArg,
    pub permission_mode: String,
    pub system_prompt_file: Option<PathBuf>,
    pub settings_file: Option<PathBuf>,
    pub mcp_config_file: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ClaudeArgs {
    pub fn new(session: AgentSessionArg) -> Self {
        Self {
            session,
            permission_mode: "auto".to_string(),
            system_prompt_file: None,
            settings_file: None,
            mcp_config_file: None,
            model: None,
            effort: None,
        }
    }

    /// 顺序与 Swift 版一致：会话号、权限、世界观、settings、mcp、model、effort。空值不带。
    pub fn argv(&self) -> Vec<String> {
        let mut a = Vec::new();
        match &self.session {
            AgentSessionArg::Resume(id) => a.extend(["--resume".to_string(), id.clone()]),
            AgentSessionArg::New(id) => a.extend(["--session-id".to_string(), id.clone()]),
        }
        a.extend([
            "--permission-mode".to_string(),
            self.permission_mode.clone(),
        ]);
        let files = [
            ("--append-system-prompt-file", &self.system_prompt_file),
            ("--settings", &self.settings_file),
            ("--mcp-config", &self.mcp_config_file),
        ];
        for (flag, file) in files {
            if let Some(f) = file {
                a.extend([flag.to_string(), f.display().to_string()]);
            }
        }
        for (flag, value) in [("--model", &self.model), ("--effort", &self.effort)] {
            if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
                a.extend([flag.to_string(), v.to_string()]);
            }
        }
        a
    }
}

/// 会话号能不能放心用：它会当目录名（`claude-<id>`，之后 `remove_dir_all`）和 argv 值。
/// 只收 `[A-Za-z0-9_-]`、不以 `-` 开头、1..=128 个字符：UUID（claude 会话号、codex thread id）
/// 都在内；`/`、`\`、`.`（含 `..`）、NUL、空白一概不收。
pub fn valid_agent_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 生成新会话号：小写 UUID v4（claude `--session-id` 要 UUID）。
pub fn new_agent_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// crew 通讯（MCP + hooks）要用的 helper 与身份。M2b 里 helper 只是一个路径参数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommsSpec {
    /// helper 可执行文件（终态是 crewd 自己）。
    pub helper: PathBuf,
    /// crewd 数据目录（helper 据此找 socket）。
    pub data_dir: PathBuf,
    pub crew_id: String,
    /// crewd 的 session id（不是 claude 的会话号）。
    pub session_id: String,
    pub captain: bool,
    pub structured_post: bool,
    pub label: Option<String>,
    /// false = 只挂 MCP，不注册任何 hook（M3 之前 helper 还没有 `hook` 子命令）。
    pub hooks: bool,
}

/// helper 命令行。M3 定子命令时只改这里。
pub struct HelperCommands<'a>(pub &'a CommsSpec);

impl HelperCommands<'_> {
    fn identity(&self) -> Vec<String> {
        let c = self.0;
        vec![
            "--data-dir".to_string(),
            c.data_dir.display().to_string(),
            "--crew".to_string(),
            c.crew_id.clone(),
            "--session".to_string(),
            c.session_id.clone(),
        ]
    }

    /// MCP server：exec 形式（claude 直接 spawn，不过 shell），所以不加引号。
    pub fn mcp_args(&self) -> Vec<String> {
        self.mcp_args_for("claude")
    }

    /// 同 [`Self::mcp_args`]，`--agent` 由调用方给（codex 驱动传 `"codex"`）。
    pub fn mcp_args_for(&self, agent: &str) -> Vec<String> {
        let c = self.0;
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity());
        a.extend(["--agent".to_string(), agent.to_string()]);
        if c.captain {
            a.push("--captain".to_string());
        }
        if c.structured_post {
            a.push("--structured-post".to_string());
        }
        if let Some(label) = c.label.as_deref().filter(|l| !l.is_empty()) {
            a.extend(["--label".to_string(), label.to_string()]);
        }
        a
    }

    /// hook：claude 经 `sh -c` 跑，所以整条按词单引号拼成一个 shell 串。
    pub fn hook_command(&self, event: HookEvent) -> String {
        let c = self.0;
        let mut words = vec![
            c.helper.display().to_string(),
            "hook".to_string(),
            event.subcommand().to_string(),
        ];
        words.extend(self.identity());
        match event {
            HookEvent::PostToolUse => {
                if c.captain {
                    words.push("--captain".to_string());
                }
            }
            HookEvent::PreToolUse => {
                words.extend(["--gate".to_string(), "computer-use".to_string()]);
            }
            HookEvent::Stop => {
                if c.captain {
                    words.push("--captain".to_string());
                }
                if let Some(label) = c.label.as_deref().filter(|l| !l.is_empty()) {
                    words.extend(["--label".to_string(), label.to_string()]);
                }
            }
        }
        words
            .iter()
            .map(|w| shell_quote(w))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 我们注册的 claude hook 事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    PostToolUse,
    PreToolUse,
    Stop,
}

impl HookEvent {
    pub const ALL: [HookEvent; 3] = [Self::PostToolUse, Self::PreToolUse, Self::Stop];

    /// claude settings.json 里的事件名。
    pub fn settings_key(self) -> &'static str {
        match self {
            Self::PostToolUse => "PostToolUse",
            Self::PreToolUse => "PreToolUse",
            Self::Stop => "Stop",
        }
    }

    fn subcommand(self) -> &'static str {
        match self {
            Self::PostToolUse => "post-tool-use",
            Self::PreToolUse => "pre-tool-use",
            Self::Stop => "stop",
        }
    }
}

/// 单引号包一个 shell 词（内部的 `'` 写成 `'\''`）。
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// 一次启动要的全部输入（调用方给的）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeSpec {
    pub workdir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)`：settings 写 `"fastMode": true`，且模型不含 opus 时强制 `opus`；
    /// `Some(false)`：写 `"fastMode": false`；`None`：不写。
    pub fast_mode: Option<bool>,
    /// 世界观（M3 渲染）。写成 0600 文件走 `--append-system-prompt-file`。
    pub system_prompt: Option<String>,
    /// 开场任务。**只走 PTY 投递**，这里只是捎带给驱动层。
    pub initial_prompt: Option<String>,
    pub comms: Option<CommsSpec>,
    /// 额外环境变量（如 API key）。过 [`crate::env::build_env`] 的禁用前缀。
    pub additional_env: BTreeMap<String, String>,
}

/// 本次启动写出来的临时文件所在目录。drop 时整个删掉（会话结束就不需要了）。
#[derive(Debug)]
pub struct LaunchFiles {
    dir: PathBuf,
}

impl LaunchFiles {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for LaunchFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 写临时文件、拼 argv。文件放在 `runtime_dir/<会话号>/`（目录 0700、文件 0600）。
/// 同一会话号重复准备会先清掉旧目录。
pub fn prepare_claude(
    spec: &ClaudeSpec,
    session: AgentSessionArg,
    runtime_dir: &Path,
) -> std::io::Result<(ClaudeArgs, LaunchFiles)> {
    if !valid_agent_session_id(session.id()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("非法的会话号 {:?}", session.id()),
        ));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(runtime_dir)?;
    let dir = runtime_dir.join(format!("claude-{}", session.id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let files = LaunchFiles { dir };

    let mut args = ClaudeArgs::new(session);
    args.effort = spec.effort.clone();
    args.model = spec.model.clone();
    if spec.fast_mode == Some(true)
        && !args
            .model
            .as_deref()
            .is_some_and(|m| m.to_lowercase().contains("opus"))
    {
        args.model = Some("opus".to_string());
    }
    if let Some(text) = &spec.system_prompt {
        args.system_prompt_file = Some(write_private(
            files.dir(),
            "system-prompt.md",
            text.as_bytes(),
        )?);
    }
    let mut settings = serde_json::Map::new();
    if let Some(comms) = &spec.comms {
        let helper = HelperCommands(comms);
        let mcp = json!({"mcpServers": {"crew": {
            "command": comms.helper.display().to_string(),
            "args": helper.mcp_args(),
        }}});
        args.mcp_config_file = Some(write_json(files.dir(), "mcp.json", &mcp)?);
        if comms.hooks {
            let hooks: serde_json::Map<String, Value> = HookEvent::ALL
                .iter()
                .map(|e| {
                    let cmd = helper.hook_command(*e);
                    (
                        e.settings_key().to_string(),
                        json!([{"hooks": [{"type": "command", "command": cmd}]}]),
                    )
                })
                .collect();
            settings.insert("hooks".to_string(), Value::Object(hooks));
        }
    }
    if let Some(fast) = spec.fast_mode {
        settings.insert("fastMode".to_string(), Value::Bool(fast));
    }
    if !settings.is_empty() {
        args.settings_file = Some(write_json(
            files.dir(),
            "settings.json",
            &Value::Object(settings),
        )?);
    }
    Ok((args, files))
}

fn write_json(dir: &Path, name: &str, value: &Value) -> std::io::Result<PathBuf> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_private(dir, name, &bytes)
}

/// 0600 新建（已存在即失败——目录是刚建的，撞上说明有人在抢）。
fn write_private(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let path = dir.join(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn comms() -> CommsSpec {
        CommsSpec {
            helper: PathBuf::from("/opt/crew d/crewd"),
            data_dir: PathBuf::from("/data"),
            crew_id: "local-aaaa".to_string(),
            session_id: "worker-1".to_string(),
            captain: true,
            structured_post: true,
            label: Some("O'Neil".to_string()),
            hooks: true,
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn agent_session_id_validation() {
        for ok in [
            "u-1",
            "same",
            "0199c1a2-7b3e-7c4d-8e5f-0123456789ab",
            &new_agent_session_id(),
            &"a".repeat(128),
        ] {
            assert!(valid_agent_session_id(ok), "{ok}");
        }
        for bad in [
            "",
            "../x",
            "..",
            "a/b",
            "a\\b",
            "x\0",
            "x y",
            "x\n",
            "-rf",
            "a.b",
            &"a".repeat(129),
        ] {
            assert!(!valid_agent_session_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn prepare_refuses_invalid_ids_before_touching_the_filesystem() {
        let rt = tempfile::tempdir().unwrap();
        let runtime = rt.path().join("run");
        for bad in ["../x", "", "-rf", "a/b", "x\0"] {
            let err = prepare_claude(
                &ClaudeSpec::default(),
                AgentSessionArg::Resume(bad.into()),
                &runtime,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert!(!runtime.exists(), "拒绝发生在碰文件系统之前");
    }

    #[test]
    fn argv_order_and_auto_mode() {
        let mut a = ClaudeArgs::new(AgentSessionArg::New("u-1".into()));
        a.system_prompt_file = Some("/r/sp.md".into());
        a.settings_file = Some("/r/s.json".into());
        a.mcp_config_file = Some("/r/m.json".into());
        a.model = Some("opus".into());
        a.effort = Some("high".into());
        assert_eq!(
            a.argv(),
            [
                "--session-id",
                "u-1",
                "--permission-mode",
                "auto",
                "--append-system-prompt-file",
                "/r/sp.md",
                "--settings",
                "/r/s.json",
                "--mcp-config",
                "/r/m.json",
                "--model",
                "opus",
                "--effort",
                "high",
            ]
        );
    }

    #[test]
    fn resume_replaces_session_id_and_empty_values_are_dropped() {
        let mut a = ClaudeArgs::new(AgentSessionArg::Resume("old".into()));
        a.model = Some(String::new());
        assert_eq!(a.argv(), ["--resume", "old", "--permission-mode", "auto"]);
    }

    #[test]
    fn new_ids_are_lowercase_uuids() {
        let id = new_agent_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_lowercase());
        assert_ne!(id, new_agent_session_id());
    }

    #[test]
    fn prompt_never_reaches_argv_and_files_are_private() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            system_prompt: Some("世界观".into()),
            initial_prompt: Some("SECRET-BRIEF 第一条任务".into()),
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-2".into()), rt.path()).unwrap();
        let argv = args.argv().join(" ");
        assert!(
            !argv.contains("SECRET-BRIEF"),
            "首条 prompt 绝不进 argv：{argv}"
        );
        assert_eq!(mode(files.dir()), 0o700);
        for f in [
            &args.system_prompt_file,
            &args.settings_file,
            &args.mcp_config_file,
        ] {
            let f = f.as_ref().expect("三个文件都该写出来");
            assert!(f.starts_with(files.dir()));
            assert_eq!(mode(f), 0o600, "{}", f.display());
        }
        assert_eq!(
            std::fs::read_to_string(args.system_prompt_file.as_ref().unwrap()).unwrap(),
            "世界观"
        );
        let dir = files.dir().to_path_buf();
        drop(files);
        assert!(!dir.exists(), "LaunchFiles drop 时删掉整个目录");
    }

    #[test]
    fn mcp_config_is_exec_form_and_hooks_are_quoted_shell_strings() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, _files) =
            prepare_claude(&spec, AgentSessionArg::New("u-3".into()), rt.path()).unwrap();
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(args.mcp_config_file.unwrap()).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["crew"]["command"], "/opt/crew d/crewd");
        assert_eq!(
            mcp["mcpServers"]["crew"]["args"],
            json!([
                "mcp",
                "--data-dir",
                "/data",
                "--crew",
                "local-aaaa",
                "--session",
                "worker-1",
                "--agent",
                "claude",
                "--captain",
                "--structured-post",
                "--label",
                "O'Neil"
            ])
        );
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/opt/crew d/crewd' 'hook' 'pre-tool-use' '--data-dir' '/data' '--crew' \
             'local-aaaa' '--session' 'worker-1' '--gate' 'computer-use'"
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "'/opt/crew d/crewd' 'hook' 'stop' '--data-dir' '/data' '--crew' 'local-aaaa' \
             '--session' 'worker-1' '--captain' '--label' 'O'\\''Neil'"
        );
        assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["type"], "command");
        assert!(settings.get("fastMode").is_none());
    }

    #[test]
    fn hooks_can_be_omitted_until_m3() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.hooks = false;
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, _files) =
            prepare_claude(&spec, AgentSessionArg::New("u-4".into()), rt.path()).unwrap();
        assert!(args.mcp_config_file.is_some());
        assert_eq!(
            args.settings_file, None,
            "没有 hook 也没有 fastMode 就不带 --settings"
        );
    }

    #[test]
    fn fast_mode_forces_opus_and_writes_setting() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            model: Some("sonnet".into()),
            fast_mode: Some(true),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-5".into()), rt.path()).unwrap();
        assert_eq!(args.model.as_deref(), Some("opus"));
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert_eq!(settings, json!({"fastMode": true}));

        let spec = ClaudeSpec {
            model: Some("claude-opus-5".into()),
            fast_mode: Some(true),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-6".into()), rt.path()).unwrap();
        assert_eq!(
            args.model.as_deref(),
            Some("claude-opus-5"),
            "已是 opus 系就不改"
        );
    }

    #[test]
    fn preparing_the_same_id_twice_starts_clean() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            system_prompt: Some("v1".into()),
            ..ClaudeSpec::default()
        };
        let (_a, first) =
            prepare_claude(&spec, AgentSessionArg::New("same".into()), rt.path()).unwrap();
        std::mem::forget(first);
        let spec = ClaudeSpec {
            system_prompt: Some("v2".into()),
            ..ClaudeSpec::default()
        };
        let (args, _second) =
            prepare_claude(&spec, AgentSessionArg::New("same".into()), rt.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(args.system_prompt_file.unwrap()).unwrap(),
            "v2"
        );
    }
}
