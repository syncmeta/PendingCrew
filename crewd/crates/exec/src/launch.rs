//! claude 的启动参数与临时文件（Swift `SessionConfig.argv` / `LocalSessionLaunch` 的移植）。
//!
//! 两条铁律：
//! - **首条 prompt 绝不进 argv**：本机任何进程 `ps` 都看得见 argv（2026-08-28 出过事故）。
//!   它走 PTY 投递（[`crate::delivery`]）。
//! - `--permission-mode auto`，绝不 bypass。
//!
//! hooks / MCP 的命令行指向 helper（`crewd mcp` / `crewd hook`，spec §22.3）。
//! 命令行的形状只在 [`HelperCommands`] 一处拼。
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

/// 文件名：`<launch dir>/agent-token`。helper 读它做 `agent.attach`。
pub const TOKEN_FILE: &str = "agent-token";
/// 各 hook 的超时（秒）。官方默认 600 秒：卡住的 crewd 会把每次工具调用拖 10 分钟。
/// 审批闸也用它：审批不阻塞（照 Swift 驾驶舱 #75②，控制者裁定 C2），一次 hook 就是一次本机 socket 往返。
pub const QUICK_HOOK_TIMEOUT_SECS: u64 = 15;

/// crew 通讯（MCP + hooks）要用的 helper 与身份（spec §22.3「身份」）。
/// 机组、角色、显示名不在这里：会话中途会变（换机长），由 serve 每次现查。
#[derive(Clone, PartialEq, Eq)]
pub struct CommsSpec {
    /// helper 可执行文件（crewd 自己）。
    pub helper: PathBuf,
    /// crewd 数据目录（helper 据此找 `crewd.sock`）。
    pub data_dir: PathBuf,
    /// crewd 的 session id（不是 claude 的会话号）。
    pub session_id: String,
    /// `TokenTable::mint` 给的；只写进 0600 的 token 文件，不进任何配置或 argv。
    pub token: String,
    /// false = 只挂 MCP，不注册任何 hook。
    pub hooks: bool,
    /// 要人审批的工具名子串（serve 配置）；空 = 不注册 PreToolUse。
    pub approval_gates: Vec<String>,
}

/// `Debug` 不打印 token（它会进日志 / panic 信息）。
impl std::fmt::Debug for CommsSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommsSpec")
            .field("helper", &self.helper)
            .field("data_dir", &self.data_dir)
            .field("session_id", &self.session_id)
            .field("token", &"<redacted>")
            .field("hooks", &self.hooks)
            .field("approval_gates", &self.approval_gates)
            .finish()
    }
}

/// helper 命令行。子命令形状只在这里拼。
pub struct HelperCommands<'a>(pub &'a CommsSpec);

impl HelperCommands<'_> {
    fn identity(&self, token_file: &Path) -> Vec<String> {
        let c = self.0;
        vec![
            "--data-dir".to_string(),
            c.data_dir.display().to_string(),
            "--session".to_string(),
            c.session_id.clone(),
            "--token-file".to_string(),
            token_file.display().to_string(),
        ]
    }

    /// MCP server：exec 形式（claude 直接 spawn，不过 shell），所以不加引号。
    pub fn mcp_args(&self, token_file: &Path) -> Vec<String> {
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity(token_file));
        a
    }

    /// hook：claude 经 `sh -c` 跑（没有 `args` 字段 = shell 形式），整条按词单引号拼。
    pub fn hook_command(&self, event: HookEvent, token_file: &Path) -> String {
        let mut words = vec![
            self.0.helper.display().to_string(),
            "hook".to_string(),
            event.subcommand().to_string(),
        ];
        words.extend(self.identity(token_file));
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
    /// Fires instead of `PostToolUse` when the tool call failed (official hooks docs); handled by
    /// the same `crewd hook post-tool-use` command.
    PostToolUseFailure,
    PreToolUse,
    Stop,
}

impl HookEvent {
    pub const ALL: [HookEvent; 4] = [
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::PreToolUse,
        Self::Stop,
    ];

    /// claude settings.json 里的事件名。
    pub fn settings_key(self) -> &'static str {
        match self {
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PreToolUse => "PreToolUse",
            Self::Stop => "Stop",
        }
    }

    /// `crewd hook <它>`；与 `crewd_proto::agent::HookKind` 的线上名一致（PostToolUseFailure 与 PostToolUse 共用
    /// `post-tool-use`：serve 按 payload 里的 `hook_event_name` 区分）。
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::PostToolUse | Self::PostToolUseFailure => "post-tool-use",
            Self::PreToolUse => "pre-tool-use",
            Self::Stop => "stop",
        }
    }

    /// 显式超时（秒）。PreToolUse 超时**不挡**工具调用（官方 hooks 文档），所以 helper 自己的期限
    /// （`hook_cmd::QUICK` = 5s）必须短于它：helper 到期 / 连不上 crewd 时对命中闸的调用输出 deny。
    pub fn timeout_secs(self) -> u64 {
        QUICK_HOOK_TIMEOUT_SECS
    }
}

/// 审批闸 → PreToolUse 的 matcher。外层括号让 claude 走正则路径（只含字母数字 `_ - , |`
/// 空格的 matcher 是精确匹配，`computer-use` 就匹配不到 `mcp__computer-use__x`）；
/// 不锚定 = 子串匹配，与 Swift `toolName.contains(gate)` 同义。空表 = None。
pub fn gate_matcher(gates: &[String]) -> Option<String> {
    let parts: Vec<String> = gates
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .map(|g| {
            let mut s = String::new();
            for ch in g.chars() {
                if r"\^$.|?*+()[]{}/".contains(ch) {
                    s.push('\\');
                }
                s.push(ch);
            }
            s
        })
        .collect();
    (!parts.is_empty()).then(|| format!("({})", parts.join("|")))
}

/// 写 token 文件（0600、`create_new`）。claude 的启动目录与 codex 驱动共用。
pub fn write_token_file(dir: &Path, token: &str) -> std::io::Result<PathBuf> {
    write_private(dir, TOKEN_FILE, token.as_bytes())
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
    /// 建 `runtime_dir/<name>/`（目录 0700；已存在的同名目录先清掉）。claude 与 codex 驱动共用。
    pub fn create(runtime_dir: &Path, name: &str) -> std::io::Result<LaunchFiles> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(runtime_dir)?;
        let dir = runtime_dir.join(name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(LaunchFiles { dir })
    }

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
    let files = LaunchFiles::create(runtime_dir, &format!("claude-{}", session.id()))?;

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
        let token_file = write_token_file(files.dir(), &comms.token)?;
        let helper = HelperCommands(comms);
        let mcp = json!({"mcpServers": {"crew": {
            "type": "stdio",
            "command": comms.helper.display().to_string(),
            "args": helper.mcp_args(&token_file),
        }}});
        args.mcp_config_file = Some(write_json(files.dir(), "mcp.json", &mcp)?);
        if comms.hooks {
            let mut hooks = serde_json::Map::new();
            for e in HookEvent::ALL {
                let mut entry = serde_json::Map::new();
                if e == HookEvent::PreToolUse {
                    match gate_matcher(&comms.approval_gates) {
                        Some(m) => {
                            entry.insert("matcher".to_string(), Value::String(m));
                        }
                        None => continue,
                    }
                }
                entry.insert(
                    "hooks".to_string(),
                    json!([{
                        "type": "command",
                        "command": helper.hook_command(e, &token_file),
                        "timeout": e.timeout_secs(),
                    }]),
                );
                hooks.insert(
                    e.settings_key().to_string(),
                    Value::Array(vec![Value::Object(entry)]),
                );
            }
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
            session_id: "worker-1".to_string(),
            token: "tok-SECRET".to_string(),
            hooks: true,
            approval_gates: vec!["computer-use".to_string()],
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
    fn mcp_config_is_exec_form_with_token_file_only() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-3".into()), rt.path()).unwrap();
        let token_file = files.dir().join(TOKEN_FILE);
        assert_eq!(std::fs::read_to_string(&token_file).unwrap(), "tok-SECRET");
        assert_eq!(mode(&token_file), 0o600);
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(args.mcp_config_file.clone().unwrap()).unwrap())
                .unwrap();
        let crew = &mcp["mcpServers"]["crew"];
        assert_eq!(crew["type"], "stdio");
        assert_eq!(crew["command"], "/opt/crew d/crewd");
        assert_eq!(
            crew["args"],
            json!([
                "mcp",
                "--data-dir",
                "/data",
                "--session",
                "worker-1",
                "--token-file",
                token_file.display().to_string()
            ])
        );
        let all = std::fs::read_to_string(files.dir().join("mcp.json")).unwrap()
            + &std::fs::read_to_string(files.dir().join("settings.json")).unwrap()
            + &args.argv().join(" ");
        assert!(
            !all.contains("tok-SECRET"),
            "token 只在 0600 文件里，不进配置与 argv"
        );
    }

    #[test]
    fn hooks_are_quoted_shell_strings_with_explicit_timeouts() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-4".into()), rt.path()).unwrap();
        let tf = files.dir().join(TOKEN_FILE).display().to_string();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        let post = &settings["hooks"]["PostToolUse"][0];
        assert!(post.get("matcher").is_none(), "PostToolUse 对所有工具");
        assert_eq!(
            post["hooks"][0]["command"],
            format!(
                "'/opt/crew d/crewd' 'hook' 'post-tool-use' '--data-dir' '/data' \
                 '--session' 'worker-1' '--token-file' '{tf}'"
            )
        );
        assert_eq!(post["hooks"][0]["type"], "command");
        assert_eq!(post["hooks"][0]["timeout"], 15);
        // PostToolUseFailure (official hooks docs: fires instead of PostToolUse when the tool
        // fails) runs the very same helper command with the same timeout.
        let failed = &settings["hooks"]["PostToolUseFailure"][0];
        assert!(failed.get("matcher").is_none());
        assert_eq!(failed["hooks"][0], post["hooks"][0]);
        assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["timeout"], 15);
        let pre = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(pre["matcher"], "(computer-use)");
        assert_eq!(
            pre["hooks"][0]["timeout"], 15,
            "审批不阻塞（C2）：闸也是一次本机往返"
        );
        assert!(
            pre["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("'hook' 'pre-tool-use'"),
            "{}",
            pre["hooks"][0]["command"]
        );
    }

    #[test]
    fn no_gates_means_no_pre_tool_use_hook() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.approval_gates.clear();
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-5".into()), rt.path()).unwrap();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert!(settings["hooks"].get("PreToolUse").is_none());
        assert!(settings["hooks"].get("PostToolUse").is_some());
    }

    #[test]
    fn hooks_can_still_be_switched_off() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.hooks = false;
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-6".into()), rt.path()).unwrap();
        assert!(args.mcp_config_file.is_some());
        assert_eq!(args.settings_file, None);
        assert!(files.dir().join(TOKEN_FILE).exists(), "MCP 也要 token");
    }

    #[test]
    fn gate_matcher_escapes_and_forces_the_regex_path() {
        assert_eq!(gate_matcher(&[]), None);
        assert_eq!(gate_matcher(&["".into(), "  ".into()]), None, "空项忽略");
        assert_eq!(
            gate_matcher(&["computer-use".into()]).unwrap(),
            "(computer-use)"
        );
        assert_eq!(
            gate_matcher(&["a.b".into(), "x|y".into(), "(z)".into()]).unwrap(),
            r"(a\.b|x\|y|\(z\))"
        );
    }

    #[test]
    fn comms_debug_redacts_the_token() {
        let shown = format!("{:?}", comms());
        assert!(!shown.contains("tok-SECRET"), "{shown}");
        assert!(shown.contains("worker-1"), "{shown}");
    }

    #[test]
    fn token_file_is_private_and_refuses_to_overwrite() {
        let rt = tempfile::tempdir().unwrap();
        let p = write_token_file(rt.path(), "t1").unwrap();
        assert_eq!(mode(&p), 0o600);
        assert!(
            write_token_file(rt.path(), "t2").is_err(),
            "已存在 = 有人在抢"
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
