//! `GitWorktrees` 对真 git 的测试（控制者裁定 C7 / C8：只有这几条显式的临时仓库测试调真 git，且放在集成测试二进制里）。
//! 合成仓库：临时目录里 `git init` + 一次空提交；绝不碰真仓库。要求 `git` 在 PATH 上。
use std::path::Path;
use std::process::Command;

use crewd_server::orchestra::worktree::{GitWorktrees, Worktrees};

/// (成功?, stdout 去首尾空白, stderr)。清掉会把 git 指到别的仓库的环境变量；不读开发者的全局 hook / 签名配置。
fn git(cwd: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git 要在 PATH 上");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let (ok, _, err) = git(dir.path(), args);
        assert!(ok, "git {args:?}：{err}");
    };
    run(&["init", "-b", "main"]);
    run(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "init",
    ]);
    dir
}

#[tokio::test]
async fn creates_a_worktree_on_a_new_branch_under_dot_pendingcrew() {
    let r = repo();
    let root = r.path().canonicalize().unwrap(); // macOS：/var → /private/var，git 报的是真路径
    let p = GitWorktrees
        .create(&root, "fix scroll 补测试")
        .await
        .unwrap();
    assert!(
        p.starts_with(root.join(".pendingcrew").join("worktrees")),
        "{p:?}"
    );
    assert!(
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("fix-scroll-"),
        "{p:?}"
    );
    assert!(p.is_dir());
    let b = git(&p, &["symbolic-ref", "--short", "HEAD"]).1;
    assert!(
        b.starts_with("pendingcrew/session-") && b.len() == "pendingcrew/session-".len() + 6,
        "{b}"
    );
    assert!(
        std::fs::read_to_string(root.join(".git/info/exclude"))
            .unwrap()
            .contains("/.pendingcrew/")
    );
    let again = GitWorktrees
        .create(&root, "fix scroll 补测试")
        .await
        .unwrap();
    assert_ne!(p, again, "目录名带随机后缀，不撞");
    assert_eq!(
        git(&root, &["status", "--porcelain"]).1,
        "",
        "exclude 生效：主工作区不脏"
    );
}

#[tokio::test]
async fn starts_from_a_subdirectory_of_the_repo() {
    let r = repo();
    let root = r.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let p = GitWorktrees.create(&root.join("sub"), "").await.unwrap();
    assert!(
        p.starts_with(root.join(".pendingcrew/worktrees"))
            && p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-"),
        "{p:?}"
    );
}

#[tokio::test]
async fn refuses_non_repos_and_detached_head_and_never_falls_back() {
    let plain = tempfile::tempdir().unwrap();
    let e = GitWorktrees.create(plain.path(), "x").await.unwrap_err();
    assert!(e.contains("不是 git 仓库"), "{e}");
    assert!(!plain.path().join(".pendingcrew").exists());
    let r = repo();
    let root = r.path().canonicalize().unwrap();
    assert!(git(&root, &["checkout", "--detach"]).0);
    let e = GitWorktrees.create(&root, "x").await.unwrap_err();
    assert!(e.contains("detached HEAD"), "{e}");
    assert!(!root.join(".pendingcrew").exists(), "失败不留半成品");
}
