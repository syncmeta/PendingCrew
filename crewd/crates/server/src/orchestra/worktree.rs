//! 为 `isolation: true` 的 session 新建 git worktree（控制者裁定 C8，照 Swift 的 `SessionWorkspace` / `GitWorktreeService`）。
//! 只建、永不删：删除是人的动作（crewd 不碰 `git worktree remove`，也不删分支）。
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::exec::BoxFut;

pub trait Worktrees: Send + Sync {
    /// 在 `crew_dir`（机组工作目录）所在的仓库里新建 worktree；`hint` = 目录名前缀（开场 brief，空串则 `session`）。
    /// 返回新目录的绝对路径。失败 = `Err(给人看的原因)`，什么都不该留下来要人收拾的（除了 git 自己在 `worktree add` 中途失败时的残留，那归 git）。
    fn create<'a>(
        &'a self,
        crew_dir: &'a Path,
        hint: &'a str,
    ) -> BoxFut<'a, Result<PathBuf, String>>;
}

/// 生产实现：shell 出 `git`。同步的 `Command` 放进 `spawn_blocking`；Core 在派生任务里调它，不堵 actor。
pub struct GitWorktrees {
    /// 跑哪个 git（生产 = PATH 上的 `git`；测试换成假脚本）。
    pub program: PathBuf,
    /// 整次新建（几次 git 调用合计）的期限：到点杀掉正在跑的 git、回错误。
    pub timeout: Duration,
}

/// `git worktree add` 要检出整棵树，大仓库要几十秒；这个期限只挡「挂住」（hook 卡死、网络盘）。
pub const WORKTREE_TIMEOUT: Duration = Duration::from_secs(120);

impl Default for GitWorktrees {
    fn default() -> Self {
        Self {
            program: PathBuf::from("git"),
            timeout: WORKTREE_TIMEOUT,
        }
    }
}

impl Worktrees for GitWorktrees {
    fn create<'a>(
        &'a self,
        crew_dir: &'a Path,
        hint: &'a str,
    ) -> BoxFut<'a, Result<PathBuf, String>> {
        let (dir, hint) = (crew_dir.to_path_buf(), hint.to_string());
        let git = Git {
            program: self.program.clone(),
            deadline: Instant::now() + self.timeout,
            limit: self.timeout,
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || create_blocking(&git, &dir, &hint))
                .await
                .map_err(|e| format!("新建 worktree 的任务崩了：{e}"))?
        })
    }
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

struct Git {
    program: PathBuf,
    deadline: Instant,
    /// 整次新建的限时（只用于报错文字）。
    limit: Duration,
}

impl Git {
    /// 跑一次 git：不问终端（`GIT_TERMINAL_PROMPT=0`，stdin 接空），过了 `deadline` 就杀掉、回「超时」。
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<Out, String> {
        use std::io::Read;
        let mut child = Command::new(&self.program)
            .args(args)
            .current_dir(cwd)
            // 从 git hook / 别的仓库里被调起时，这几个会把 git 指到错的仓库。
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            // 要凭据（https 远端、子模块）时别在 serve 的终端上等人输入：直接失败。
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("跑不了 git（{e}）；请确认 git 在 PATH 上"))?;
        // 两个管道各一条线程读完，免得输出多时子进程堵在写管道上、永远等不到退出。
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = pipe {
                    let _ = p.read_to_end(&mut buf);
                }
                String::from_utf8_lossy(&buf).trim().to_string()
            })
        };
        let out = drain(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let err = drain(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= self.deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 读线程不等：git 的孙进程（hook）可能还拿着管道，等它们会把期限白费掉。
                    return Err(format!(
                        "无法为 session 新建 worktree：git {} 超时（新建限时 {:.1} 秒），已中止",
                        args.first().copied().unwrap_or_default(),
                        self.limit.as_secs_f64()
                    ));
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(e) => return Err(format!("等 git 结束时出错：{e}")),
            }
        };
        Ok(Out {
            ok: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }
}

/// 轮询子进程是否结束的间隔。
const POLL: Duration = Duration::from_millis(10);

fn create_blocking(git: &Git, crew_dir: &Path, hint: &str) -> Result<PathBuf, String> {
    let shown = crew_dir.display();
    // 目录本身不在时 `current_dir` 会报 ENOENT，被 `git()` 说成「git 不在 PATH 上」——先查掉（预检 G-20）。
    if !crew_dir.is_dir() {
        return Err(format!(
            "无法为 session 新建 worktree：机组工作目录 {shown} 不存在"
        ));
    }
    let inside = git.run(crew_dir, &["rev-parse", "--is-inside-work-tree"])?;
    if !(inside.ok && inside.stdout == "true") {
        return Err(format!(
            "无法为 session 新建 worktree：{shown} 不是 git 仓库"
        ));
    }
    let head = git.run(crew_dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !head.ok || head.stdout.is_empty() {
        return Err(format!(
            "无法为 session 新建 worktree：{shown} 当前是 detached HEAD"
        ));
    }
    let top = git.run(crew_dir, &["rev-parse", "--show-toplevel"])?;
    if !top.ok || top.stdout.is_empty() {
        return Err(format!(
            "无法为 session 新建 worktree：查不到 {shown} 的仓库根（{}）",
            top.stderr
        ));
    }
    let root = PathBuf::from(top.stdout);
    let container = root.join(".pendingcrew").join("worktrees");
    std::fs::create_dir_all(&container)
        .map_err(|e| format!("建不了 {}：{e}", container.display()))?;
    ensure_local_exclude(&root);
    let hex = |n: usize| uuid::Uuid::new_v4().simple().to_string()[..n].to_string();
    let hint = if hint.is_empty() { "session" } else { hint };
    let target = container.join(format!("{}-{}", slug(hint), hex(8)));
    let branch = format!("pendingcrew/session-{}", hex(6));
    let add = git.run(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &target.display().to_string(),
            &head.stdout,
        ],
    )?;
    if !add.ok {
        return Err(format!("git worktree add 失败：{}", add.stderr));
    }
    Ok(target)
}

/// 把 `/.pendingcrew/` 记进仓库本地的 exclude（Swift `ensureLocalGitignore`：文件不存在就跳过，不冒险写错地方）。
fn ensure_local_exclude(root: &Path) {
    use std::io::Write;
    let exclude = root.join(".git").join("info").join("exclude");
    let needle = "/.pendingcrew/";
    if !exclude.is_file()
        || std::fs::read_to_string(&exclude)
            .map(|c| c.contains(needle))
            .unwrap_or(false)
    {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&exclude) {
        let _ = f.write_all(
            format!("\n# Added by crewd — worktree scratch dir.\n{needle}\n").as_bytes(),
        );
    }
}

fn slug(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = ch == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let s: String = out.trim_matches('-').chars().take(32).collect();
    if s.is_empty() { "crew".into() } else { s }
}

/// 默认挂真实现（`Hooks::default()`）。
pub fn real() -> Arc<dyn Worktrees> {
    Arc::new(GitWorktrees::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 真 git 的测试（建仓库、worktree add）按控制者裁定 C8 放在集成测试 `tests/orchestra_worktree.rs`。

    /// 预检 G-20：机组目录没了要说「不存在」，不能怪到「git 不在 PATH 上」。在跑 git 之前就挡下，不起进程。
    #[tokio::test]
    async fn missing_crew_dir_is_reported_as_missing_not_as_git_absent() {
        let e = GitWorktrees::default()
            .create(Path::new("/nonexistent/crewd-crew-dir"), "x")
            .await
            .unwrap_err();
        assert!(e.contains("不存在") && !e.contains("PATH"), "{e}");
    }

    #[test]
    fn slug_follows_the_swift_rules() {
        assert_eq!(slug("fix scroll 补测试"), "fix-scroll");
        assert_eq!(slug("补测试"), "crew");
        assert_eq!(slug("--a__b--"), "a__b");
        assert_eq!(slug(&"a".repeat(40)).len(), 32);
        assert_eq!(slug("a   b"), "a-b");
    }
}
