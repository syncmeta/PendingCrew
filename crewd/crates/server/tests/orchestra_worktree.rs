//! `GitWorktrees` 对真 git 的测试（控制者裁定 C7 / C8：只有这几条显式的临时仓库测试调真 git，且放在集成测试二进制里）。
//! 合成仓库：临时目录里 `git init` + 一次空提交；绝不碰真仓库。要求 `git` 在 PATH 上。
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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
    // 仓库本地钉住：`GitWorktrees` 自己的 git 调用（`worktree add` 会跑 post-checkout）不带 `-c`，
    // 不能让开发者全局的 hook / 签名配置混进测试结果（Task 6 评审 (1)）。
    run(&["config", "core.hooksPath", "/dev/null"]);
    run(&["config", "commit.gpgsign", "false"]);
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
    let p = GitWorktrees::default()
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
    let again = GitWorktrees::default()
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
    let p = GitWorktrees::default()
        .create(&root.join("sub"), "")
        .await
        .unwrap();
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
    let e = GitWorktrees::default()
        .create(plain.path(), "x")
        .await
        .unwrap_err();
    assert!(e.contains("不是 git 仓库"), "{e}");
    assert!(!plain.path().join(".pendingcrew").exists());
    let r = repo();
    let root = r.path().canonicalize().unwrap();
    assert!(git(&root, &["checkout", "--detach"]).0);
    let e = GitWorktrees::default()
        .create(&root, "x")
        .await
        .unwrap_err();
    assert!(e.contains("detached HEAD"), "{e}");
    assert!(!root.join(".pendingcrew").exists(), "失败不留半成品");
}

/// 期限测试给假 git 的期限。不能太短：脚本里的第一行（写 pidfile）要在期限之前跑到，否则量不到孙进程
/// （Task 8b 记下的抖动：负载下新写出来的脚本冷启动 250–380 ms，撞上过 300 ms 的期限）。
const DEADLINE: Duration = Duration::from_secs(2);

/// 一个假的 `git`（shell 脚本）：不碰真仓库，只用来量 `GitWorktrees` 怎么调它。
/// 写好后先空跑一次（带 `CREWD_FAKE_GIT_WARM`，脚本第一行就退出）：新文件第一次 exec 的冷启动不算进被测的期限。
fn fake_git(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("fake-git");
    std::fs::write(
        &p,
        format!("#!/bin/sh\n[ -n \"$CREWD_FAKE_GIT_WARM\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let warm = Command::new(&p)
        .env("CREWD_FAKE_GIT_WARM", "1")
        .status()
        .unwrap();
    assert!(warm.success(), "假 git 空跑失败：{warm:?}");
    p
}

/// Task 6 评审 (2)：git 永不在终端上问凭据（`GIT_TERMINAL_PROMPT=0`；ssh 走 BatchMode），stdin 也不接。
#[tokio::test]
async fn git_runs_with_terminal_prompts_disabled_and_no_stdin() {
    let t = tempfile::tempdir().unwrap();
    let seen = t.path().join("seen");
    let body = format!(
        "printf '%s|%s|' \"$GIT_TERMINAL_PROMPT\" \"$GIT_SSH_COMMAND\" > '{0}'; if read -r line; then echo got-stdin >> '{0}'; fi; exit 1",
        seen.display()
    );
    let wt = GitWorktrees {
        program: fake_git(t.path(), &body),
        timeout: Duration::from_secs(10),
    };
    let e = wt.create(t.path(), "x").await.unwrap_err();
    assert!(e.contains("不是 git 仓库"), "{e}");
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        "0|ssh -oBatchMode=yes|"
    );
}

/// Task 6 评审 (2)：卡住的 git（hook 挂住、网络盘）有期限：到点杀掉、回一个说得清的错，不无限等。
#[tokio::test]
async fn a_hung_git_is_killed_at_the_deadline() {
    let t = tempfile::tempdir().unwrap();
    let wt = GitWorktrees {
        program: fake_git(t.path(), "exec sleep 30"),
        timeout: DEADLINE,
    };
    let started = Instant::now();
    let e = wt.create(t.path(), "x").await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(e.contains("超时"), "{e}");
}

/// 到点杀的是整个进程组：git 起的 hook / 子进程（这里是后台的 `sleep`）也一起没了，不留下拿着管道的孤儿。
#[tokio::test]
async fn the_deadline_kills_the_whole_process_group() {
    let t = tempfile::tempdir().unwrap();
    let pidfile = t.path().join("child.pid");
    let body = format!(
        "sleep 30 & echo $! > '{}'; exec sleep 30",
        pidfile.display()
    );
    let wt = GitWorktrees {
        program: fake_git(t.path(), &body),
        timeout: DEADLINE,
    };
    let e = wt.create(t.path(), "x").await.unwrap_err();
    assert!(e.contains("超时"), "{e}");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // 被杀的孙进程由 init / launchd 收尸；收之前 kill(pid, 0) 仍会成功，所以等一会儿。
    let gone = (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        !Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    });
    assert!(gone, "后台子进程 {pid} 还活着");
}

/// Task 7 评审：git 自己正常退了，但它起的后台进程（hook 的后台任务）还拿着输出管道——读输出也受同一个期限，
/// 到点连组杀掉、回「超时」，不无限等 EOF。
#[tokio::test]
async fn output_held_open_after_git_exits_is_bounded_by_the_deadline() {
    let t = tempfile::tempdir().unwrap();
    let pidfile = t.path().join("child.pid");
    let body = format!("sleep 30 & echo $! > '{}'; exit 0", pidfile.display());
    let wt = GitWorktrees {
        program: fake_git(t.path(), &body),
        timeout: DEADLINE,
    };
    let started = Instant::now();
    let e = wt.create(t.path(), "x").await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(e.contains("超时"), "{e}");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let gone = (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        !Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    });
    assert!(gone, "拿着管道的后台进程 {pid} 还活着");
}
