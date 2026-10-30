//! 驱动层：真 PTY、真进程，「claude」用 shell 脚本顶替（只验启动、账本、停止、续跑被拒重起、
//! 登记表、认证探测；投递与对话框见 `fake_claude.rs`）。
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::driver::{
    ClaudeStart, Control, ControlOutcome, DriverConfig, SendError, SessionHandle, StartError,
    start_claude,
};
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
    let last_working = events.iter().rev().find_map(|e| match e {
        SessionEvent::Working(w) => Some(*w),
        _ => None,
    });
    assert_eq!(
        last_working,
        Some(false),
        "被拒的那个进程吐过字（Working(true)），重起前要补一个 Working(false)：{events:?}"
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

/// 被拒之后进程没退、还在吐字：驱动层杀掉它再收尸。macOS 上被杀的会话首进程要等 PTY 里的
/// 输出被读走才收得了尸——收尸时不读主端，会话任务就永远挂住（号也一直占着）。
#[tokio::test]
async fn refused_resume_with_trailing_output_still_restarts() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("noisy-id")));
    let program = env.script(
        "case \"$*\" in\n\
         *--resume*) echo \"No conversation found with session ID: noisy-id\"; \
         i=0; while [ $i -lt 2000 ]; do printf 'x\\n'; i=$((i+1)); done; sleep 30;;\n\
         *) echo \"FRESH $*\"; sleep 30;;\n\
         esac",
    );
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), env.config())
        .await
        .unwrap();
    until(&mut rx, |e| {
        matches!(e, SessionEvent::Spawned { resumed: false, .. })
    })
    .await;
    screen_contains(&h, "FRESH --session-id").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}

/// 已经在停（`stop()` / 句柄全丢）之后才出现续跑被拒的原话：照常退出，不重起、不动账本，
/// 也不发 `ResumeRefused`（每条 `ResumeRefused` 后面都该跟一条 `Spawned`；是我们要它停的）。
/// `stop` 返回要留着的句柄（`stop()` 那条不能顺带丢句柄，否则两条测成一条）。
async fn refusal_while_stopping(
    id: &str,
    stop: impl FnOnce(SessionHandle) -> Option<SessionHandle>,
) {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some(id)));
    let printed = env.dir.path().join("refusal-printed");
    // 忽略 TERM：停的时候它还活着，被拒的原话在 SIGTERM 之后、SIGKILL 之前才出来；
    // 打完原话再落一个标记文件，证明那行字确实是在停的过程中吐出来的。
    let program = env.script(&format!(
        "trap '' TERM\n\
         echo armed\n\
         case \"$*\" in\n\
         *--resume*) sleep 0.8; echo \"No conversation found with session ID: {id}\"; touch '{}'; sleep 30;;\n\
         *) echo FRESH; sleep 30;;\n\
         esac",
        printed.display()
    ));
    let mut config = env.config();
    config.stop_grace = Duration::from_millis(1500);
    let (h, mut rx) = start_claude(env.start(program, ledger.clone()), config)
        .await
        .unwrap();
    next(&mut rx).await;
    // 等 trap 装好再停（冷启动时 sh 可能还没执行到 trap，SIGTERM 会直接把它带走）。
    screen_contains(&h, "armed").await;
    let _keep = stop(h);
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(
        printed.exists(),
        "前提：被拒的原话在退出之前（停的过程中）已经吐出来了：{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::ResumeRefused { .. })),
        "在停就不报续跑被拒：{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::Spawned { .. })),
        "在停就不能重起：{events:?}"
    );
    let info = exited(events.last().unwrap()).unwrap();
    assert!(info.stopped_by_user, "{info:?}");
    assert_eq!(ledger.current().as_deref(), Some(id), "账本不能被改写");
}

#[tokio::test]
async fn refusal_after_stop_does_not_restart() {
    refusal_while_stopping("stopped-id", |h| {
        h.stop().unwrap();
        Some(h)
    })
    .await;
}

#[tokio::test]
async fn refusal_after_every_handle_dropped_does_not_restart() {
    refusal_while_stopping("dropped-id", |h| {
        drop(h);
        None
    })
    .await;
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

/// 反复叫停（比 `stop_grace` 还勤）：SIGTERM 只发一次，SIGKILL 的期限也不往后推——
/// 否则每次 `stop()` 都把升级重新计时，一个忽略 SIGTERM 的进程就永远杀不掉。
#[tokio::test]
async fn repeated_stop_neither_resends_term_nor_postpones_the_kill() {
    let env = Env::new();
    // 收到 TERM 只打一行、不退：看得出 TERM 来了几次。
    let program = env.script("trap 'echo GOT-TERM' TERM; echo armed; while :; do sleep 0.1; done");
    let mut config = env.config();
    config.stop_grace = Duration::from_secs(1);
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        config,
    )
    .await
    .unwrap();
    screen_contains(&h, "armed").await;
    let t0 = std::time::Instant::now();
    for _ in 0..3 {
        h.stop().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // 三次 stop 都发出去了（0 / 300 / 600ms），trap 每 0.1s 有机会跑一次。
    let text = h.screen_text(100).await.unwrap();
    assert_eq!(
        text.matches("GOT-TERM").count(),
        1,
        "SIGTERM 只发一次：{text}"
    );
    let info = loop {
        tokio::select! {
            e = next(&mut rx) => if let SessionEvent::Exited(info) = e { break info },
            _ = tokio::time::sleep(Duration::from_millis(400)) => { let _ = h.stop(); }
        }
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "一直在叫停也要在 grace 后被 SIGKILL"
        );
    };
    let took = t0.elapsed();
    assert_eq!(info.status, ExitStatusKind::Signal(9), "{info:?}");
    assert!(
        took < Duration::from_millis(2500),
        "期限从第一次 stop 起算（grace 1s），实际 {took:?}"
    );
}

/// 会话首进程收到 SIGTERM 就乖乖退了，但它留下的子进程连 HUP / TERM 都忽略：
/// 退出后也要整组补一刀，不能留下孤儿。
#[tokio::test]
async fn graceful_exit_still_kills_the_rest_of_the_group() {
    let env = Env::new();
    let program = env.script(
        "(trap '' HUP TERM; exec sleep 30) & echo \"CHILD $!\"\n\
         trap 'exit 0' TERM\n\
         while :; do sleep 0.1; done",
    );
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
        Some(ExitStatusKind::Code(0)),
        "前提：首进程自己在 grace 内退了（没走 SIGKILL 升级）"
    );
    let pid = rustix::process::Pid::from_raw(grandchild).unwrap();
    // 被杀的孙进程要等 init 收尸，僵尸期间 kill(pid, 0) 仍成功：给它一点时间。
    let mut gone = false;
    for _ in 0..60 {
        if rustix::process::test_kill_process(pid).is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !gone {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    assert!(gone, "首进程退出后同组的孙进程 {grandchild} 还活着");
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
    // 先清账、后发 Exited：见到 Exited 时登记表必须已经是空的。
    assert!(
        registry.entries().is_empty(),
        "Exited 发出时登记表没清：{:?}",
        registry.entries()
    );
}

/// 登记表写不进去：杀掉刚起的进程并报错。进程一起来就狂吐输出——macOS 上没人读主端时
/// 被 SIGKILL 的会话首进程收不了尸（关从端要等输出被读走），收尸时必须同时读掉主端。
#[tokio::test]
async fn registry_failure_kills_the_child_even_if_it_floods_the_pty() {
    let env = Env::new();
    // 父目录不存在：load 当空表，record 落盘失败。
    let registry =
        Arc::new(ProcessRegistry::load(&env.dir.path().join("missing").join("r.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let program = env.script("while :; do echo flood-flood-flood-flood; done");
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        start_claude(
            env.start(program, Arc::new(MemoryLedger::default())),
            config,
        ),
    )
    .await
    .expect("登记表失败后 10s 内要返回（收尸不能挂住）")
    .err()
    .expect("必须报错");
    assert!(matches!(err, StartError::Registry(_)), "{err:?}");
    // Failed persistence retains the restrictive in-memory obligation. Actual
    // launch failure must nevertheless reap its exact physical child; it must
    // not erase provenance merely because orphan-file IO is unavailable.
    let entries = registry.entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        crewd_exec::registry::probe(entries[0].1.pid),
        crewd_exec::registry::ProbeResult::Missing
    ));
    assert!(!env.dir.path().join("missing/r.json").exists());
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
async fn confirmed_native_resize_matches_kernel_and_snapshot_without_queue_guess() {
    use crewd_exec::driver::ResizeOutcome;
    let env = Env::new();
    let program = env.script("stty size; read l; stty size; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    screen_contains(&h, "25 80").await;
    assert_eq!(
        h.resize_confirmed(0, 30).await.unwrap(),
        ResizeOutcome::InvalidSize
    );
    assert_eq!(
        h.resize_confirmed(513, 30).await.unwrap(),
        ResizeOutcome::InvalidSize
    );
    assert_eq!(h.snapshot().await.unwrap().unwrap().cols, 80);
    assert_eq!(
        h.resize_confirmed(100, 30).await.unwrap(),
        ResizeOutcome::Applied
    );
    let snapshot = h.snapshot().await.unwrap().unwrap();
    assert_eq!((snapshot.cols, snapshot.lines), (100, 30));
    h.raw(b"size-check\r".to_vec()).unwrap();
    screen_contains(&h, "30 100").await;
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
    assert!(h.resize_confirmed(90, 20).await.is_err());
}

/// 空 / 全空白的正文在句柄上就拒掉：不占投递编号，也不会有「投递结果」那条事件可等。
#[tokio::test]
async fn empty_text_is_rejected_at_the_handle() {
    let env = Env::new();
    let program = env.script("echo up; sleep 30");
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    next(&mut rx).await;
    assert_eq!(h.send(""), Err(SendError::EmptyText));
    assert_eq!(h.send(" \n\t "), Err(SendError::EmptyText));
    let id = h.send("x").unwrap();
    assert_eq!(id, 1, "被拒的不占编号");
    h.stop().unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    let ids: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Delivery { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(ids, [1], "{events:?}");
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

/// M2c 给窄接口加的三个口子在 claude 一侧：打断 = 写一个 Esc；控制一律 `Unsupported`
/// （claude 的 /model、/compact 归 M3 用屏幕驱动）；快照是 `Some`。
#[tokio::test]
async fn interrupt_is_a_lone_escape_and_control_is_unsupported_on_claude() {
    let env = Env::new();
    let program = env.script(
        "stty raw -echo; echo ready; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
         stty sane; echo \"byte:$b\"; sleep 30",
    );
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    screen_contains(&h, "ready").await;
    h.interrupt().unwrap();
    screen_contains(&h, "byte:1b").await;
    assert!(matches!(
        h.control(Control::Model("opus".into())).await.unwrap(),
        ControlOutcome::Unsupported(_)
    ));
    assert!(h.snapshot().await.unwrap().is_some());
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}
