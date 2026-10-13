//! `crewd claude` 的胶水：库里的会话号账本 → 第一次新号、第二次 `--resume` 同一个号。
//! 「claude」用 shell 脚本顶替（打印参数后退出）。另有开场任务的来路（`--prompt-file`）与
//! stdin 一直开着时的退出。
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crewd_proto::{Crew, Role, Session};
use crewd_store::{Store, crews, sessions};

fn seed(data: &Path) {
    let s = Store::open(&data.join("crewd.db")).unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            management: false,
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
            executor: "claude".into(),
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

fn run(data: &Path, fake: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        // 登录 shell 抓 PATH 走失败分支：测试不依赖本机的 rc 文件。
        .env("SHELL", "/usr/bin/false")
        .args(["claude", "--session", "w-1", "--workdir"])
        .arg(data)
        .arg("--claude")
        .arg(fake)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn second_run_resumes_the_id_recorded_by_the_first() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path());
    let fake = data.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho \"ARGS $*\"\nsleep 0.2\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let first = run(data.path(), &fake);
    assert!(first.contains("resumed: false"), "{first}");
    assert!(first.contains("Exited("), "{first}");
    let recorded = {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::agent_session_id(s.conn(), "w-1")
            .unwrap()
            .expect("第一次起之前就该落账")
    };
    assert!(first.contains(&recorded), "{first}");

    let second = run(data.path(), &fake);
    assert!(
        second.contains(&format!("agent_session_id: \"{recorded}\", resumed: true")),
        "{second}"
    );
    let registry = std::fs::read_to_string(data.path().join("exec-registry.json")).unwrap();
    assert_eq!(registry.trim(), "{}", "退出后登记表清空");
}

/// Synthetic native TUI: raw/no-echo input, explicit consumed-key cursor frames,
/// real received bytes recorded outside argv. This is not kernel line-echo evidence.
fn line_reader_fake(dir: &Path) -> std::path::PathBuf {
    let fake = dir.join("fake-claude-reader");
    std::fs::write(&fake, r#"#!/usr/bin/env python3
import os, pathlib, select, sys, termios, time
root = pathlib.Path(__file__).parent
(root / 'args.txt').write_text(' '.join(sys.argv[1:]) + '\n')
attrs = termios.tcgetattr(0)
attrs[0] &= ~termios.ICRNL
attrs[3] &= ~(termios.ICANON | termios.ECHO)
attrs[6][termios.VMIN] = 1
attrs[6][termios.VTIME] = 0
termios.tcsetattr(0, termios.TCSANOW, attrs)
value = bytearray()
def draw():
    os.write(1, ('\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ ' + value.decode(errors='replace') + '\x1b[7m \x1b[27m\r\n────────────────────\r\n').encode())
draw()
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    ready, _, _ = select.select([0], [], [], 0.05)
    if not ready:
        continue
    chunk = os.read(0, 4096)
    if not chunk:
        break
    for key in chunk:
        if key == 127:
            value = value[:-1]
        elif key == 13:
            (root / 'got.txt').write_bytes(bytes(value) + b'\n')
            value.clear()
            draw()
            time.sleep(1)
            sys.exit(0)
        else:
            value.append(key)
    draw()
sys.exit(2)
"#).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake
}

fn crewd(data: &Path, fake: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_crewd"));
    cmd.env("CREWD_DATA_DIR", data)
        .env("SHELL", "/usr/bin/false")
        .args(["claude", "--workdir"])
        .arg(data)
        .arg("--claude")
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

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 开场任务从文件读、经 PTY 投进去：crewd 自己的 argv 里没有它（`ps` 对本机所有进程可见），
/// claude 的 argv 里也没有。文件末尾的换行不算正文。
#[test]
fn prompt_file_is_delivered_through_the_pty_not_argv() {
    let data = tempfile::tempdir().unwrap();
    let fake = line_reader_fake(data.path());
    let prompt = data.path().join("brief.txt");
    std::fs::write(&prompt, "从文件来的开场任务\n").unwrap();
    let out = crewd(data.path(), &fake)
        .arg("--prompt-file")
        .arg(&prompt)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        read_or_empty(&data.path().join("got.txt")),
        "从文件来的开场任务\n",
        "{stdout}"
    );
    let args = read_or_empty(&data.path().join("args.txt"));
    assert!(args.contains("--session-id"), "前提：参数记下来了：{args}");
    assert!(!args.contains("开场任务"), "{args}");
}

/// `--prompt-file -`：开场任务从 stdin 读（读到 EOF；末尾的空行不算正文）。
#[test]
fn prompt_file_dash_reads_the_brief_from_stdin() {
    let data = tempfile::tempdir().unwrap();
    let fake = line_reader_fake(data.path());
    let mut child = crewd(data.path(), &fake)
        .args(["--prompt-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
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
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stderr}");
    assert_eq!(
        read_or_empty(&data.path().join("got.txt")),
        "从 stdin 来的开场任务\n"
    );
}

/// 开场任务不再能从命令行参数给（会出现在 `ps` 里）。一定带 `--claude <假的>`：万一参数
/// 又被接受了，起来的也只是个立刻退出的脚本，绝不会去 PATH 里找真 claude。
#[test]
fn prompt_text_on_the_command_line_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let fake = data.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = crewd(data.path(), &fake)
        .args(["--prompt", "秘密任务"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("--prompt"), "{stderr}");
}

/// 会话自己结束了、stdin 却一直开着（终端里没人再按回车）：crewd 照样马上退出——
/// 读 stdin 的那条线程不能拖住退出。
#[test]
fn exits_when_the_session_ends_even_if_stdin_stays_open() {
    let data = tempfile::tempdir().unwrap();
    let fake = data.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho bye\nsleep 0.3\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = crewd(data.path(), &fake)
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
