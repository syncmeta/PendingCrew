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
/// `CLAUDE_CONFIG_DIR`：设了才有；claude 按它放会话日志，认证探测
/// （`AuthProbe::projects_dir_for`）按 crewd 自己的环境找日志——两边要看到同一个值。
pub const PASSTHROUGH_KEYS: [&str; 7] = [
    "HOME",
    "SHELL",
    "LANG",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "CLAUDE_CONFIG_DIR",
];
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
/// 超时、退出码非 0、没有哨兵、哨兵之间为空 → `None`。
///
/// shell 在**新会话**里跑（setsid：自成会话、自成进程组、没有控制终端）；读到收尾哨兵
/// 就不再等 EOF（rc 文件留下的后台任务会一直攥着 stdout 管道），等 shell 自己退出拿退出码。
/// 无论成败、超时，最后都对整个进程组 SIGKILL 再收尸，rc 文件起的后代一个不留。
///
/// 为什么是 setsid 而不是 `process_group(0)`：crewd 从终端里起时有控制终端，交互式 zsh
/// 落在同一会话的后台进程组里，一碰 tty 就被 SIGTTIN/SIGTTOU 停住，抓 PATH 超时丢失。
/// 新会话没有控制终端，交互 shell 照常跑完（daemon 场景本来就没有 tty，两种路径一致）。
pub async fn login_shell_path(shell: &Path, timeout: Duration) -> Option<String> {
    use tokio::io::AsyncReadExt;

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
    // SAFETY: pre_exec 在 fork 之后、exec 之前运行，只调 async-signal-safe 的 setsid(2)，
    // 不分配内存、不拿锁。
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let mut child = cmd.spawn().ok()?;
    // 会话组长的 pid 就是 pgid。超时路径上 shell 还没被收尸，killpg 时这个 pid 不会被复用；
    // 成功 / 失败路径上 shell 已先被收尸，只要它的后代还在，进程组就还在、pgid 就不会被复用；
    // 后代都没了则 killpg 得 ESRCH，无害（pid 恰好被新进程占用并自成一组的窗口可忽略）。
    let pgid = child
        .id()
        .and_then(|id| rustix::process::Pid::from_raw(id as i32));
    let mut stdout = child.stdout.take()?;

    let capture = async {
        let mut buf: Vec<u8> = Vec::new();
        while stdout.read_buf(&mut buf).await.ok()? > 0 {
            if String::from_utf8_lossy(&buf).contains(PATH_SENTINEL_CLOSE) {
                break;
            }
        }
        let status = child.wait().await.ok()?;
        status.success().then_some(buf)
    };
    let captured = tokio::time::timeout(timeout, capture).await;

    if let Some(pgid) = pgid {
        let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
    }
    let _ = child.start_kill();
    let _ = child.wait().await;

    extract_sentinel_path(&String::from_utf8_lossy(&captured.ok()??))
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

    /// 脚本把自己的 pid 写进 `pidfile`（登录 shell 是自己进程组的组长，pid = pgid）。
    /// 轮询到整组进程消失为止；2s 内没消失 = 有后代逃过了 killpg。
    fn assert_group_gone(pidfile: &Path) {
        let pgid: i32 = std::fs::read_to_string(pidfile)
            .expect("脚本没写 pidfile")
            .trim()
            .parse()
            .unwrap();
        let pgid = rustix::process::Pid::from_raw(pgid).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match rustix::process::test_kill_process_group(pgid) {
                Err(rustix::io::Errno::SRCH) => return,
                _ if Instant::now() > deadline => panic!("进程组 {pgid:?} 里还有活着的进程"),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    #[tokio::test]
    async fn hanging_login_shell_times_out_and_leaves_no_survivors() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let shell = script(
            dir.path(),
            "stuck-shell",
            &format!("echo $$ > {}; sleep 30", pidfile.display()),
        );
        let started = Instant::now();
        // 超时要留够脚本冷启动写下 pidfile 的时间，否则测不到进程组。
        let got = login_shell_path(&shell, Duration::from_secs(1)).await;
        assert_eq!(got, None);
        assert!(started.elapsed() < Duration::from_secs(5), "超时必须生效");
        assert_group_gone(&pidfile);
    }

    /// 登录 shell 必须是自己会话的组长（getsid(pid) == pid、且不等于我们的会话）：
    /// 这样它没有控制终端，从终端里起的 crewd 不会让交互 zsh 落进后台进程组被 SIGTTIN 停住。
    /// 测试进程本身未必有控制终端，所以断言会话归属而不是「碰 tty 不会卡」。
    #[tokio::test]
    async fn login_shell_runs_as_leader_of_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let shell = script(
            dir.path(),
            "slow-shell",
            &format!("echo $$ > {}; sleep 30", pidfile.display()),
        );
        let inspect = async {
            let deadline = Instant::now() + Duration::from_secs(5);
            let pid = loop {
                if let Ok(text) = std::fs::read_to_string(&pidfile)
                    && let Ok(raw) = text.trim().parse::<i32>()
                {
                    break rustix::process::Pid::from_raw(raw).unwrap();
                }
                assert!(Instant::now() < deadline, "脚本没写 pidfile");
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let session = rustix::process::getsid(Some(pid)).unwrap();
            assert_eq!(session, pid, "shell 应是自己会话的组长");
            assert_ne!(
                session,
                rustix::process::getsid(None).unwrap(),
                "不能和 crewd 同一会话"
            );
        };
        let (got, ()) = tokio::join!(login_shell_path(&shell, Duration::from_secs(1)), inspect);
        assert_eq!(got, None);
        assert_group_gone(&pidfile);
    }

    #[tokio::test]
    async fn background_job_holding_stdout_does_not_delay_or_survive() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let shell = script(
            dir.path(),
            "rc-with-bg-job",
            &format!(
                "echo $$ > {}; sleep 8 &\nprintf '{PATH_SENTINEL_OPEN}/x/bin{PATH_SENTINEL_CLOSE}'",
                pidfile.display()
            ),
        );
        let started = Instant::now();
        let got = login_shell_path(&shell, Duration::from_secs(5)).await;
        assert_eq!(got.as_deref(), Some("/x/bin"));
        // 要证明的是没等管道 EOF：后台 sleep 8s、超时 5s，只要明显早于 5s 就判得出来。
        // 界限放宽到 3s 是为了并行测试负载下脚本冷启动可能超过 1s。
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "不能为后台任务攥着的管道等到 EOF"
        );
        assert_group_gone(&pidfile);
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

    /// claude 用 `CLAUDE_CONFIG_DIR` 定会话日志在哪；认证探测按 crewd 自己的环境找日志——
    /// 两边必须看到同一个值，否则探测永远在错的目录里找、什么都读不到。
    #[test]
    fn claude_config_dir_is_passed_through_when_set() {
        let env = build_env(
            [("HOME", "/home/u"), ("CLAUDE_CONFIG_DIR", "/cfg")],
            "/p",
            &BTreeMap::new(),
        );
        assert_eq!(
            env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/cfg")
        );
        let env = build_env([("HOME", "/home/u")], "/p", &BTreeMap::new());
        assert!(!env.contains_key("CLAUDE_CONFIG_DIR"), "没设就不出现");
    }

    #[test]
    fn additional_env_can_override_whitelisted_values() {
        let mut extra = BTreeMap::new();
        extra.insert("HOME".to_string(), "/elsewhere".to_string());
        let env = build_env([("HOME", "/home/u")], "/p", &extra);
        assert_eq!(env["HOME"], "/elsewhere");
    }
}
