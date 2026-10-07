//! 把这个二进制的构建身份（git 提交、干不干净）编进去（U1 / S6：`system.hello` 的 `build`）。
//! 发版流水线给 `CREWD_GIT_SHA`（可选 `CREWD_GIT_DIRTY=1`）就用它；否则问 git。任何一步不成 = `unknown`，
//! 绝不让构建失败（没有 git、不在仓库里、源码包里构建都照常编）。
//! 「干不干净」= `git status --porcelain --untracked-files=no -- <crewd 工作区根>` 有没有输出（只看 crewd 自己的
//! 源码，仓库里别的东西改了不算）。脚本在 HEAD、当前分支的 ref、index、`<工作区>/crates` 下任何文件、`Cargo.lock`
//! 变了（或上面两个变量变了）时重跑，所以 dirty 不会停在旧值上（修复轮 I2）。
//! 每个 git 命令都带 `GIT_OPTIONAL_LOCKS=0`：`git status` 不会顺手刷新、写回 index——否则 index 的修改时刻
//! 每次构建都变，`rerun-if-changed` 让下一次构建永远不 Fresh。
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

/// `git rev-parse --git-path <p>` 的绝对路径（在 worktree 里它指到主仓库的 `.git/worktrees/<名字>/…`）。
fn git_path(dir: &Path, p: &str) -> Option<PathBuf> {
    let s = git(dir, &["rev-parse", "--git-path", p])?;
    let path = PathBuf::from(s);
    Some(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

fn main() {
    println!("cargo:rerun-if-env-changed=CREWD_GIT_SHA");
    println!("cargo:rerun-if-env-changed=CREWD_GIT_DIRTY");
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    // crewd 工作区根（`crates/server` 往上两层）：dirty 只看它下面；它的源码与 Cargo.lock 变了就重跑。
    let workspace = dir.join("..").join("..");
    for p in [workspace.join("crates"), workspace.join("Cargo.lock")] {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    // HEAD 换分支、ref 换提交（新提交只改 ref，不改 HEAD）、index 变（暂存 / 提交）时重跑。
    let mut watch = vec![git_path(&dir, "HEAD"), git_path(&dir, "index")];
    if let Some(r) = git(&dir, &["symbolic-ref", "-q", "HEAD"]) {
        match git_path(&dir, &r) {
            Some(p) if p.exists() => watch.push(Some(p)),
            // 打包了的 ref 不在单独的文件里。
            _ => watch.push(git_path(&dir, "packed-refs")),
        }
    }
    for p in watch.into_iter().flatten() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    let (sha, dirty) = match env::var("CREWD_GIT_SHA") {
        Ok(s) if !s.trim().is_empty() => (
            s.trim().to_string(),
            env::var("CREWD_GIT_DIRTY").is_ok_and(|v| v == "1" || v == "true"),
        ),
        _ => match git(&dir, &["rev-parse", "--short=12", "HEAD"]) {
            Some(sha) if !sha.is_empty() => {
                let root = workspace.display().to_string();
                let dirty = git(
                    &dir,
                    &["status", "--porcelain", "--untracked-files=no", "--", &root],
                )
                .is_some_and(|s| !s.is_empty());
                (sha, dirty)
            }
            _ => ("unknown".to_string(), false),
        },
    };
    println!("cargo:rustc-env=CREWD_GIT_SHA={sha}");
    println!("cargo:rustc-env=CREWD_GIT_DIRTY={dirty}");
}
