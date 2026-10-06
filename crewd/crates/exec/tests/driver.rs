//! 驱动层：真 PTY、真进程，「claude」用 shell 脚本顶替（只验启动、账本、停止、续跑被拒重起、
//! 登记表、认证探测；投递与对话框见 `fake_claude.rs`）。
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::driver::{ClaudeStart, DriverConfig, SessionHandle, StartError, start_claude};
use crewd_exec::health::{HealthKind, LaunchVerdict};
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
use tokio::sync::mpsc::UnboundedReceiver;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn script(&self, body: &str) -> PathBuf {
        let path = self.dir.path().join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn config(&self) -> DriverConfig {
        let mut c = DriverConfig::new(self.dir.path().join("run"));
        c.stop_grace = Duration::from_millis(500);
        c
    }

    fn start(&self, program: PathBuf, ledger: Arc<MemoryLedger>) -> ClaudeStart {
        ClaudeStart {
            session_id: "worker-1".into(),
            program,
            child_path: "/usr/bin:/bin".into(),
            spec: ClaudeSpec {
                workdir: self.dir.path().to_path_buf(),
                ..ClaudeSpec::default()
            },
            ledger,
        }
    }
}

async fn next(rx: &mut UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("10s 内没有事件")
        .expect("事件流提前结束")
}

/// 等到满足条件的事件，返回它之前（含）收到的全部事件。
async fn until(
    rx: &mut UnboundedReceiver<SessionEvent>,
    pred: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    loop {
        let e = next(rx).await;
        let hit = pred(&e);
        seen.push(e);
        if hit {
            return seen;
        }
    }
}

async fn screen_contains(h: &SessionHandle, needle: &str) -> String {
    for _ in 0..100 {
        let text = h.screen_text(100).await.unwrap();
        if text.contains(needle) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "屏上 5s 内没出现 {needle:?}：{}",
        h.screen_text(100).await.unwrap()
    );
}

fn exited(e: &SessionEvent) -> Option<&ExitInfo> {
    match e {
        SessionEvent::Exited(info) => Some(info),
        _ => None,
    }
}

#[tokio::test]
async fn fresh_start_records_the_id_before_spawning_and_passes_it() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let program = env.script("echo \"ARGS $*\"; sleep 30");
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), env.config())
        .await
        .unwrap();
    let SessionEvent::Spawned {
        agent_session_id,
        resumed,
        pid,
    } = next(&mut rx).await
    else {
        panic!("第一条事件必须是 Spawned");
    };
    assert!(!resumed && pid > 0);
    assert_eq!(ledger.current().as_deref(), Some(agent_session_id.as_str()));
    let text = screen_contains(&h, "ARGS").await;
    assert!(
        text.contains(&format!(
            "--session-id {agent_session_id} --permission-mode auto"
        )),
        "{text}"
    );
    h.stop().unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert_eq!(
        exited(events.last().unwrap()),
        Some(&ExitInfo {
            status: ExitStatusKind::Signal(15),
            stopped_by_user: true,
            hit_limit: false,
        })
    );
}

// 各测试续跑的号互不相同：同一进程里同一个会话号同时只能跑一个（测试并行跑）。
#[tokio::test]
async fn recorded_id_is_resumed() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("kept-id")));
    let program = env.script("echo \"ARGS $*\"; sleep 30");
    let (h, mut rx) = start_claude(env.start(program, ledger), env.config())
        .await
        .unwrap();
    assert!(matches!(
        next(&mut rx).await,
        SessionEvent::Spawned { resumed: true, ref agent_session_id, .. } if agent_session_id == "kept-id"
    ));
    screen_contains(&h, "ARGS --resume kept-id").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn refused_resume_restarts_once_with_a_fresh_recorded_id() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("old-id")));
    let program = env.script(
        "case \"$*\" in\n\
         *--resume*) echo \"No conversation found with session ID: old-id\"; exit 1;;\n\
         *) echo \"FRESH $*\"; sleep 30;;\n\
         esac",
    );
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), env.config())
        .await
        .unwrap();
    let events = until(&mut rx, |e| {
        matches!(e, SessionEvent::Spawned { resumed: false, .. })
    })
    .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::ResumeRefused { id, said }
            if id == "old-id" && said.contains("No conversation found"))),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| exited(e).is_some()),
        "被拒的那次不算退出：{events:?}"
    );
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_ne!(agent_session_id, "old-id");
    assert_eq!(
        ledger.current().as_deref(),
        Some(agent_session_id.as_str()),
        "新号先落账"
    );
    screen_contains(&h, "FRESH --session-id").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn stop_escalates_to_killing_the_process_group() {
    let env = Env::new();
    // 孙进程连 HUP 一起忽略：只靠会话首进程退出时内核发的 SIGHUP 收不掉它，必须 killpg。
    let program = env.script("trap '' TERM HUP; sleep 30 & echo \"CHILD $!\"; wait");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let text = screen_contains(&h, "CHILD ").await;
    let grandchild: i32 = text
        .split("CHILD ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .expect("没读到孙进程 pid");
    h.stop().unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert_eq!(
        exited(events.last().unwrap()).map(|i| i.status.clone()),
        Some(ExitStatusKind::Signal(9)),
        "忽略 SIGTERM 的进程要在 grace 后被 SIGKILL"
    );
    let gone =
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(grandchild).unwrap());
    assert!(gone.is_err(), "同组的孙进程也要一起收掉");
}

#[tokio::test]
async fn missing_program_is_a_start_error_with_launch_health() {
    let env = Env::new();
    let err = start_claude(
        env.start(
            env.dir.path().join("nope"),
            Arc::new(MemoryLedger::default()),
        ),
        env.config(),
    )
    .await
    .err()
    .expect("起不来必须报错");
    match err {
        StartError::Spawn { health, .. } => {
            assert_eq!(health.kind, HealthKind::LaunchFailed);
            assert_eq!(health.launch_verdict, Some(LaunchVerdict::SpawnFailed));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn dying_without_output_is_reported_as_launch_failure() {
    let env = Env::new();
    let program = env.script("exit 7");
    let (_h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(
        events.iter().any(|e| matches!(e, SessionEvent::Health(Some(h))
            if h.launch_verdict == Some(LaunchVerdict::DiedSilently) && h.detail.contains("exit 7"))),
        "{events:?}"
    );
}

#[tokio::test]
async fn registry_holds_the_live_process_and_is_cleared_on_exit() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.dir.path().join("registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        config,
    )
    .await
    .unwrap();
    let SessionEvent::Spawned { pid, .. } = next(&mut rx).await else {
        panic!()
    };
    let entries = registry.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0.as_str(), entries[0].1.pid),
        ("worker-1", pid as i32)
    );
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
    // 任务在发出 Exited 之后才清账，给它一拍。
    for _ in 0..50 {
        if registry.entries().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("退出后登记表没清：{:?}", registry.entries());
}

fn write_auth_failure(projects: &Path, id: &str) {
    let dir = projects.join("-tmp-project");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
        + chrono::Duration::seconds(1);
    let record = serde_json::json!({
        "sessionId": id, "type": "assistant", "isSidechain": false,
        "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "isApiErrorMessage": true, "error": "authentication_failed",
        "message": {"role": "assistant", "id": "msg_x", "model": "m", "content": []},
    });
    std::fs::write(dir.join(format!("{id}.jsonl")), format!("{record}\n")).unwrap();
}

#[tokio::test]
async fn auth_failure_in_the_transcript_raises_health() {
    let env = Env::new();
    let projects = env.dir.path().join("projects");
    let mut config = env.config();
    config.claude_projects_dir = Some(projects.clone());
    config.auth_poll = Duration::from_millis(100);
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        config,
    )
    .await
    .unwrap();
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = next(&mut rx).await
    else {
        panic!()
    };
    write_auth_failure(&projects, &agent_session_id);
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired),
    )
    .await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn raw_input_and_resize_reach_the_child() {
    let env = Env::new();
    let program = env.script("stty size; read l; echo \"got:$l\"; stty size; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    screen_contains(&h, "25 80").await;
    h.resize(100, 30).unwrap();
    h.raw(b"hi\r".to_vec()).unwrap();
    screen_contains(&h, "got:hi").await;
    screen_contains(&h, "30 100").await;
    let status = h.status().await.unwrap();
    assert_eq!(status.agent_session_id.len(), 36);
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

#[tokio::test]
async fn dropping_every_handle_stops_the_session() {
    let env = Env::new();
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    let h2 = h.clone();
    drop(h);
    drop(h2);
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(exited(events.last().unwrap()).unwrap().stopped_by_user);
}

/// 账本写不进去（session 行没了 / 库坏了）。
struct BrokenLedger;

impl AgentIdLedger for BrokenLedger {
    fn recorded(&self) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    fn record(&self, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("库坏了"))
    }
}

/// Review Focus 4：账本写不进去就绝不起进程——起了也接不回来，下次重启又是新对话。
#[tokio::test]
async fn ledger_failure_means_no_process() {
    let env = Env::new();
    let marker = env.dir.path().join("ran");
    let program = env.script(&format!("touch '{}'; sleep 30", marker.display()));
    let start = ClaudeStart {
        ledger: Arc::new(BrokenLedger),
        ..env.start(program, Arc::new(MemoryLedger::default()))
    };
    let err = start_claude(start, env.config())
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "账本失败后进程不该被起过");
}

/// 同一个 claude 会话号在一个 crewd 里同时只能跑一个：第二次起会先删掉第一个的启动目录
/// （`prepare_claude` 清旧目录），两个进程还会写同一份对话。见到 `Exited` 时号已经放开，
/// 调用方可以马上用它重起。
#[tokio::test]
async fn a_live_agent_session_id_cannot_be_started_twice_but_is_free_once_exited() {
    let env = Env::new();
    let program = env.script("echo up; sleep 30");
    let start = || ClaudeStart {
        spec: ClaudeSpec {
            workdir: env.dir.path().to_path_buf(),
            system_prompt: Some("world".into()),
            ..ClaudeSpec::default()
        },
        ..env.start(
            program.clone(),
            Arc::new(MemoryLedger::with(Some("twin-id"))),
        )
    };
    let (h, mut rx) = start_claude(start(), env.config()).await.unwrap();
    next(&mut rx).await;
    let prompt_file = env
        .dir
        .path()
        .join("run")
        .join("claude-twin-id")
        .join("system-prompt.md");
    assert!(prompt_file.exists());
    let err = start_claude(start(), env.config())
        .await
        .err()
        .expect("同一个号不能起第二次");
    assert!(
        matches!(&err, StartError::AlreadyRunning(id) if id == "twin-id"),
        "{err:?}"
    );
    assert!(prompt_file.exists(), "被拒的第二次不能动第一个的启动文件");
    assert!(!h.is_closed());
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
    let (h, mut rx) = start_claude(start(), env.config())
        .await
        .expect("见到 Exited 之后同一个号可以马上重起");
    next(&mut rx).await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}
