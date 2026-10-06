//! 找 agent 可执行文件、拼子进程环境（Swift `LocalCodingAgentExecutable` /
//! `LocalCodingAgentEnv` 的移植）。
//!
//! 从 launchd / systemd 拉起的进程只有一条最小 PATH，用户装在 `~/.local/bin`、
//! npm、bun 下的 `claude` 找不到。所以先起一次登录交互 shell 把它的 `$PATH` 抓回来，
//! 用同一条 PATH 定位可执行文件**并**注入子进程（否则「找得到 codex、codex 找不到
//! node」）。抓 PATH 是一次性的：调用方拿到 [`AgentPaths`] 后自己留着。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// 哨兵：把真正的 `$PATH` 从交互 rc 文件往 stdout 吐的噪声里摘出来。
pub const PATH_SENTINEL_OPEN: &str = "<<<PCREW_PATH:";
pub const PATH_SENTINEL_CLOSE: &str = ":PCREW_PATH>>>";
/// 登录 shell 最多等多久（rc 文件可能卡住）。
pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// 任何 PATH 都必须带上的系统目录（`env` / `sh` / `git` 在这里）。
pub const SYSTEM_DIRS: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];
/// 从父进程透传的环境变量（另加全部 `LC_*`）。PATH 与 TERM 不透传，见 [`build_env`]。
pub const PASSTHROUGH_KEYS: [&str; 6] = ["HOME", "SHELL", "LANG", "USER", "LOGNAME", "TMPDIR"];
/// 调用方给的额外变量里绝不放行的前缀（大小写不敏感）：我们自己的凭据与配置。
pub const FORBIDDEN_PREFIXES: [&str; 4] = ["PENDINGBOT_", "PENDINGCREW_", "SUPABASE_", "CREWD_"];
/// 子进程的 TERM。终端是我们的 `crewd_term::Screen`（xterm 系），不是 crewd 自己所在的终端。
pub const CHILD_TERM: &str = "xterm-256color";

/// 抓一次登录 shell PATH 之后的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPaths {
    /// 子进程用的 PATH，也是找可执行文件用的那一条。
    pub child_path: String,
}

impl AgentPaths {
    /// 起一次 `$SHELL -lic`（缺省 macOS `/bin/zsh`、其它 `/bin/sh`）抓 PATH，
    /// 再并上父进程 PATH 与系统目录。抓失败只是少了登录 shell 那几段，不报错。
    pub async fn discover() -> Self {
        let shell = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(default_shell()));
        let login = login_shell_path(&shell, LOGIN_SHELL_TIMEOUT).await;
        let search: Vec<String> = login
            .as_deref()
            .unwrap_or("")
            .split(':')
            .map(str::to_string)
            .collect();
        let parent = std::env::var("PATH").ok();
        Self {
            child_path: compose_child_path(&search, parent.as_deref()),
        }
    }

    /// 在 `child_path` 里找第一个可执行的 `name`。
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        resolve_in_path(name, &self.child_path)
    }
}

fn default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/sh"
    }
}

/// 起 `shell -lic "printf '<哨兵>%s<哨兵>' "$PATH""`，取哨兵之间那段。
/// 超时、退出码非 0、没有哨兵、哨兵之间为空 → `None`。超时会杀掉 shell。
pub async fn login_shell_path(shell: &Path, timeout: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg("-lic")
        .arg(format!(
            "/usr/bin/printf '{PATH_SENTINEL_OPEN}%s{PATH_SENTINEL_CLOSE}' \"$PATH\""
        ))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    extract_sentinel_path(&String::from_utf8_lossy(&out.stdout))
}

/// 从 shell 输出里取哨兵之间的 PATH。
pub fn extract_sentinel_path(output: &str) -> Option<String> {
    let start = output.find(PATH_SENTINEL_OPEN)? + PATH_SENTINEL_OPEN.len();
    let len = output[start..].find(PATH_SENTINEL_CLOSE)?;
    let path = &output[start..start + len];
    (!path.is_empty()).then(|| path.to_string())
}

/// 登录 shell 目录 → 父进程 PATH → 系统目录，去空白、去重、保序拼成一条。
pub fn compose_child_path(search_dirs: &[String], parent_path: Option<&str>) -> String {
    let mut out: Vec<&str> = Vec::new();
    let candidates = search_dirs
        .iter()
        .map(String::as_str)
        .chain(parent_path.unwrap_or("").split(':'))
        .chain(SYSTEM_DIRS);
    for dir in candidates {
        let dir = dir.trim();
        if !dir.is_empty() && !out.contains(&dir) {
            out.push(dir);
        }
    }
    out.join(":")
}

/// PATH 里第一个是普通文件且可执行的 `name`。`name` 含 `/` 或 NUL、或为空 → `None`。
pub fn resolve_in_path(name: &str, path: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return None;
    }
    path.split(':').filter(|d| !d.is_empty()).find_map(|dir| {
        let candidate = Path::new(dir).join(name);
        let is_file = std::fs::metadata(&candidate).is_ok_and(|m| m.is_file());
        let executable = rustix::fs::access(&candidate, rustix::fs::Access::EXEC_OK).is_ok();
        (is_file && executable).then_some(candidate)
    })
}

/// 调用方给的额外变量名是否该拒收。
pub fn is_forbidden_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    FORBIDDEN_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// 子进程的完整环境（子进程**不**继承 crewd 的环境，只拿这一份）。
/// 白名单键 + `LC_*` 从 `parent` 取；PATH = `child_path`；TERM = [`CHILD_TERM`]；
/// 最后叠上 `additional`（拒收 [`FORBIDDEN_PREFIXES`]，可覆盖前面的值）。
pub fn build_env<'a>(
    parent: impl IntoIterator<Item = (&'a str, &'a str)>,
    child_path: &str,
    additional: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (key, value) in parent {
        if PASSTHROUGH_KEYS.contains(&key) || key.starts_with("LC_") {
            env.insert(key.to_string(), value.to_string());
        }
    }
    env.insert("PATH".to_string(), child_path.to_string());
    env.insert("TERM".to_string(), CHILD_TERM.to_string());
    for (key, value) in additional {
        if !is_forbidden_key(key) {
            env.insert(key.clone(), value.clone());
        }
    }
    env
}

/// [`build_env`] 用当前进程环境做 `parent`。非 UTF-8 的变量跳过。
pub fn build_env_from_process(
    child_path: &str,
    additional: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let parent: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    build_env(
        parent.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        child_path,
        additional,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn sentinel_is_found_amid_rc_noise() {
        let out = format!("motd\n{PATH_SENTINEL_OPEN}/a:/b c{PATH_SENTINEL_CLOSE}\nbye");
        assert_eq!(extract_sentinel_path(&out).as_deref(), Some("/a:/b c"));
        assert_eq!(extract_sentinel_path("no sentinel"), None);
        assert_eq!(
            extract_sentinel_path(&format!("{PATH_SENTINEL_OPEN}{PATH_SENTINEL_CLOSE}")),
            None
        );
        assert_eq!(
            extract_sentinel_path(&format!("{PATH_SENTINEL_OPEN}/a")),
            None
        );
    }

    #[tokio::test]
    async fn login_shell_output_is_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(
            dir.path(),
            "noisy-shell",
            &format!(
                "echo welcome; printf '{PATH_SENTINEL_OPEN}/x/bin:/y/bin{PATH_SENTINEL_CLOSE}'"
            ),
        );
        let got = login_shell_path(&shell, Duration::from_secs(5)).await;
        assert_eq!(got.as_deref(), Some("/x/bin:/y/bin"));
    }

    #[tokio::test]
    async fn hanging_login_shell_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(dir.path(), "stuck-shell", "sleep 30");
        let started = Instant::now();
        let got = login_shell_path(&shell, Duration::from_millis(300)).await;
        assert_eq!(got, None);
        assert!(started.elapsed() < Duration::from_secs(5), "超时必须生效");
    }

    #[tokio::test]
    async fn failing_or_missing_shell_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(
            dir.path(),
            "bad-shell",
            &format!("printf '{PATH_SENTINEL_OPEN}/x{PATH_SENTINEL_CLOSE}'; exit 3"),
        );
        assert_eq!(login_shell_path(&shell, Duration::from_secs(5)).await, None);
        let missing = dir.path().join("nope");
        assert_eq!(
            login_shell_path(&missing, Duration::from_secs(5)).await,
            None
        );
    }

    #[test]
    fn child_path_prefers_login_dirs_then_parent_then_system() {
        let search = vec![
            " /home/u/.local/bin ".to_string(),
            "/usr/bin".to_string(),
            String::new(),
        ];
        let path = compose_child_path(&search, Some("/opt/x:/home/u/.local/bin::/bin"));
        assert_eq!(
            path,
            "/home/u/.local/bin:/usr/bin:/opt/x:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(
            compose_child_path(&[], None),
            "/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn resolve_takes_first_executable_file() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("claude"), "not executable").unwrap();
        std::fs::create_dir(b.path().join("claude-dir")).unwrap();
        let hit = script(b.path(), "claude", "true");
        let path = format!("{}:{}", a.path().display(), b.path().display());
        assert_eq!(resolve_in_path("claude", &path), Some(hit));
        assert_eq!(resolve_in_path("claude-dir", &path), None, "目录不算");
        assert_eq!(resolve_in_path("", &path), None);
        assert_eq!(resolve_in_path("../claude", &path), None);
        assert_eq!(resolve_in_path("missing", &path), None);
    }

    #[test]
    fn env_is_a_whitelist_not_an_inheritance() {
        let parent = [
            ("HOME", "/home/u"),
            ("SHELL", "/bin/zsh"),
            ("LANG", "zh_CN.UTF-8"),
            ("LC_ALL", "C"),
            ("TERM", "xterm-ghostty"),
            ("PATH", "/short"),
            ("SECRET_TOKEN", "x"),
            ("PENDINGCREW_DATA_DIR", "/d"),
            ("CREWD_DATA_DIR", "/d"),
        ];
        let mut extra = BTreeMap::new();
        extra.insert("ANTHROPIC_API_KEY".to_string(), "k".to_string());
        extra.insert("supabase_secret_key".to_string(), "s".to_string());
        extra.insert("Crewd_Thing".to_string(), "t".to_string());
        extra.insert("PendingBot_Token".to_string(), "t".to_string());
        let env = build_env(parent, "/full:/usr/bin", &extra);
        let keys: Vec<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "ANTHROPIC_API_KEY",
                "HOME",
                "LANG",
                "LC_ALL",
                "PATH",
                "SHELL",
                "TERM"
            ]
        );
        assert_eq!(env["PATH"], "/full:/usr/bin");
        assert_eq!(
            env["TERM"], CHILD_TERM,
            "终端是我们的 Screen，不透传 crewd 所在终端的 TERM"
        );
    }

    #[test]
    fn additional_env_can_override_whitelisted_values() {
        let mut extra = BTreeMap::new();
        extra.insert("HOME".to_string(), "/elsewhere".to_string());
        let env = build_env([("HOME", "/home/u")], "/p", &extra);
        assert_eq!(env["HOME"], "/elsewhere");
    }
}
