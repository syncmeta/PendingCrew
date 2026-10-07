//! 把这个二进制的构建身份（git 提交、干不干净）编进去（U1 / S6：`system.hello` 的 `build`）。
//! 发版流水线给 `CREWD_GIT_SHA`（可选 `CREWD_GIT_DIRTY=1`）就用它；否则问 git。任何一步不成 = `unknown`，
//! 绝不让构建失败（没有 git、不在仓库里、源码包里构建都照常编）。
//! 「干不干净」= 构建脚本上次跑的时候 `git status --porcelain --untracked-files=no` 有没有输出：脚本只在 HEAD、
//! 当前分支的 ref、index 变了（或上面两个变量变了）时重跑，所以之后在工作区里改的、没暂存的不会让它翻成 dirty。
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
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
                let dirty = git(&dir, &["status", "--porcelain", "--untracked-files=no"])
                    .is_some_and(|s| !s.is_empty());
                (sha, dirty)
            }
            _ => ("unknown".to_string(), false),
        },
    };
    println!("cargo:rustc-env=CREWD_GIT_SHA={sha}");
    println!("cargo:rustc-env=CREWD_GIT_DIRTY={dirty}");
}
