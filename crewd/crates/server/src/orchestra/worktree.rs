//! 为 `isolation: true` 的 session 新建 git worktree（控制者裁定 C8，照 Swift 的 `SessionWorkspace` / `GitWorktreeService`）。
//! 只建、永不删：删除是人的动作（crewd 不碰 `git worktree remove`，也不删分支）。
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

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

/// 生产实现：shell 出 `git`。同步的 `Command` 放进 `spawn_blocking`，不堵 actor。
pub struct GitWorktrees;

impl Worktrees for GitWorktrees {
    fn create<'a>(
        &'a self,
        crew_dir: &'a Path,
        hint: &'a str,
    ) -> BoxFut<'a, Result<PathBuf, String>> {
        let (dir, hint) = (crew_dir.to_path_buf(), hint.to_string());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || create_blocking(&dir, &hint))
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

fn git(cwd: &Path, args: &[&str]) -> Result<Out, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // 从 git hook / 别的仓库里被调起时，这几个会把 git 指到错的仓库。
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| format!("跑不了 git（{e}）；请确认 git 在 PATH 上"))?;
    Ok(Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

fn create_blocking(crew_dir: &Path, hint: &str) -> Result<PathBuf, String> {
    let shown = crew_dir.display();
    // 目录本身不在时 `current_dir` 会报 ENOENT，被 `git()` 说成「git 不在 PATH 上」——先查掉（预检 G-20）。
    if !crew_dir.is_dir() {
        return Err(format!(
            "无法为 session 新建 worktree：机组工作目录 {shown} 不存在"
        ));
    }
    let inside = git(crew_dir, &["rev-parse", "--is-inside-work-tree"])?;
    if !(inside.ok && inside.stdout == "true") {
        return Err(format!(
            "无法为 session 新建 worktree：{shown} 不是 git 仓库"
        ));
    }
    let head = git(crew_dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !head.ok || head.stdout.is_empty() {
        return Err(format!(
            "无法为 session 新建 worktree：{shown} 当前是 detached HEAD"
        ));
    }
    let top = git(crew_dir, &["rev-parse", "--show-toplevel"])?;
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
    let add = git(
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
    Arc::new(GitWorktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 真 git 的测试（建仓库、worktree add）按控制者裁定 C8 放在集成测试 `tests/orchestra_worktree.rs`。

    /// 预检 G-20：机组目录没了要说「不存在」，不能怪到「git 不在 PATH 上」。在跑 git 之前就挡下，不起进程。
    #[tokio::test]
    async fn missing_crew_dir_is_reported_as_missing_not_as_git_absent() {
        let e = GitWorktrees
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
