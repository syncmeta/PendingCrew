//! `crewd codex` 的胶水：库里的 thread 账本 → 第一次新开、第二次 `thread/resume` 同一条；
//! 执行器不是 codex 的 session 被拒；开场任务的来路（`--prompt-file`）；stdin 一直开着时的退出。
//! 「codex app-server」用 shell 脚本顶替：按我们固定的请求顺序（initialize 是 0 号、
//! thread/start|resume 是 1 号、第一个 turn/start 是 2 号）回包，然后退出。
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crewd_proto::{Crew, Role, Session};
use crewd_store::{Store, crews, sessions};

const FAKE_APP_SERVER: &str = r#"#!/bin/sh
read l
echo '{"id":0,"result":{}}'
read l
read l
case "$l" in
  *thread/resume*)
    tid=$(printf '%s' "$l" | sed 's/.*"threadId":"\([^"]*\)".*/\1/')
    echo "{\"id\":1,\"result\":{\"thread\":{\"id\":\"$tid\"}}}" ;;
  *)
    echo '{"id":1,"result":{"thread":{"id":"thr-cli-1"},"model":"fake-model"}}' ;;
esac
sleep 0.3
"#;

fn seed(data: &Path, executor: &str) {
    let s = Store::open(&data.join("crewd.db")).unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    sessions::upsert(
        s.conn(),
        &Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: Role::Worker,
            executor: executor.into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
}

fn run(data: &Path, fake: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        // 登录 shell 抓 PATH 走失败分支：测试不依赖本机的 rc 文件。
        .env("SHELL", "/usr/bin/false")
        .args(["codex", "--session", "w-1", "--workdir"])
        .arg(data)
        .arg("--codex")
        .arg(fake)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn fake(data: &Path) -> std::path::PathBuf {
    let fake = data.join("fake-codex");
    std::fs::write(&fake, FAKE_APP_SERVER).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake
}

#[test]
fn second_run_resumes_the_thread_recorded_by_the_first() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), "codex");
    let fake = fake(data.path());

    let first = run(data.path(), &fake);
    let out = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        out.contains("agent_session_id: \"thr-cli-1\", resumed: false"),
        "{out}"
    );
    assert!(out.contains("Exited("), "{out}");
    let recorded = {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::agent_session_id(s.conn(), "w-1").unwrap()
    };
    assert_eq!(recorded.as_deref(), Some("thr-cli-1"), "就绪之前就该落账");

    let second = run(data.path(), &fake);
    let out = String::from_utf8_lossy(&second.stdout);
    assert!(
        out.contains("agent_session_id: \"thr-cli-1\", resumed: true"),
        "{out}"
    );
    let registry = std::fs::read_to_string(data.path().join("exec-registry.json")).unwrap();
    assert_eq!(registry.trim(), "{}", "退出后登记表清空");
}

#[test]
fn a_claude_session_is_refused_and_its_resume_id_is_left_alone() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), "claude");
    {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::set_agent_session_id(s.conn(), "w-1", "claude-uuid").unwrap();
    }
    let out = run(data.path(), &fake(data.path()));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("执行器是 claude"), "{err}");
    let s = Store::open(&data.path().join("crewd.db")).unwrap();
    assert_eq!(
        sessions::agent_session_id(s.conn(), "w-1")
            .unwrap()
            .as_deref(),
        Some("claude-uuid")
    );
}

/// 收一个回合的「codex」：参数记到 `<dir>/args.log`，第 4 行（`turn/start`）原样记到
/// `<dir>/turn.log`，回了它之后退出。
const FAKE_WITH_TURN: &str = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$*" > "$d/args.log"
read l
echo '{"id":0,"result":{}}'
read l
read l
echo '{"id":1,"result":{"thread":{"id":"thr-cli-1"}}}'
read l
printf '%s\n' "$l" > "$d/turn.log"
echo '{"id":2,"result":{"turn":{"id":"t1"}}}'
sleep 0.3
"#;

fn crewd(data: &Path, fake: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_crewd"));
    cmd.env("CREWD_DATA_DIR", data)
        .env("SHELL", "/usr/bin/false")
        .args(["codex", "--workdir"])
        .arg(data)
        .arg("--codex")
        .arg(fake);
    cmd
}

/// 等子进程退出，最多 `limit`；超时就杀掉再报错（不留下挂住的 crewd）。
fn wait_with_limit(child: &mut std::process::Child, limit: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("crewd {limit:?} 内没退出");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `--prompt-file -`：开场任务从 stdin 读到 EOF，经 `turn/start` 送给 codex（走它的 stdin）——
/// crewd 与 codex 的 argv 里都没有它（`ps` 对本机所有进程可见）。
#[test]
fn the_brief_is_read_from_stdin_and_sent_by_turn_start_not_argv() {
    let data = tempfile::tempdir().unwrap();
    let fake = data.path().join("fake-codex-turn");
    std::fs::write(&fake, FAKE_WITH_TURN).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = crewd(data.path(), &fake)
        .args(["--prompt-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("从 stdin 来的开场任务\n\n".as_bytes())
        .unwrap();
    let status = wait_with_limit(&mut child, Duration::from_secs(20));
    let (mut stdout, mut stderr) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stdout}{stderr}");
    let turn: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("turn.log")).unwrap())
            .unwrap();
    assert_eq!(turn["method"], "turn/start");
    assert_eq!(turn["params"]["input"][0]["text"], "从 stdin 来的开场任务");
    assert!(stdout.contains("outcome: Delivered"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(data.path().join("args.log")).unwrap(),
        "app-server\n",
        "codex 的 argv 只有 app-server"
    );
}

/// 开场任务不能从命令行参数给（会出现在 `ps` 里）。一定带 `--codex <假的>`：万一参数又被
/// 接受了，起来的也只是个脚本，绝不会去 PATH 里找真 codex。
#[test]
fn prompt_text_on_the_command_line_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path(), &fake(data.path()))
        .args(["--prompt", "秘密任务"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("--prompt"), "{stderr}");
}

/// 会话自己结束了、stdin 却一直开着：crewd 照样马上退出（读 stdin 的那条线程不能拖住退出）。
#[test]
fn exits_when_the_session_ends_even_if_stdin_stays_open() {
    let data = tempfile::tempdir().unwrap();
    let mut child = crewd(data.path(), &fake(data.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _keep_stdin_open = child.stdin.take().unwrap();
    let status = wait_with_limit(&mut child, Duration::from_secs(15));
    assert!(status.success());
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(stdout.contains("Exited("), "{stdout}");
}
