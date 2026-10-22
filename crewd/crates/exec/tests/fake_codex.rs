//! 端到端：真进程 + 假 codex app-server（`src/bin/fake_codex.rs`）。握手、落账、续跑与退回、
//! 回合、每一种服务端请求、额度、超时、压缩、打断、控制、停止与进程组、stderr 灌满、超长行。
//! 不碰真 codex（真机见 `live_codex.rs`）。
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::driver::{Control, ControlOutcome, SessionHandle, StartError};
use crewd_exec::health::{HealthKind, LaunchVerdict};
use crewd_exec::launch::CommsSpec;
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::{
    BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, Profile, SessionEvent,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_codex");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn config(&self) -> CodexConfig {
        let mut c = CodexConfig {
            stop_grace: Duration::from_millis(500),
            runtime_dir: self.path("run"),
            ..CodexConfig::default()
        };
        c.core.initialize_timeout = Duration::from_secs(3);
        c.core.thread_timeout = Duration::from_secs(3);
        c.core.request_timeout = Duration::from_secs(2);
        c
    }

    fn start(
        &self,
        modes: &str,
        ledger: Arc<dyn AgentIdLedger>,
        brief: Option<&str>,
    ) -> CodexStart {
        let mut additional_env: BTreeMap<String, String> = [
            ("FAKE_CODEX_MODE", modes.to_string()),
            ("FAKE_CODEX_LOG", self.path("rpc.log").display().to_string()),
            (
                "FAKE_CODEX_ENV_LOG",
                self.path("env.log").display().to_string(),
            ),
            ("FAKE_CODEX_PID", self.path("pid").display().to_string()),
            (
                "FAKE_CODEX_CHILD_PID",
                self.path("child.pid").display().to_string(),
            ),
            (
                "FAKE_CODEX_TERM_LOG",
                self.path("term.log").display().to_string(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // 调用方显式给了也必须被剥掉（计费保护）。
        additional_env.insert("OPENAI_API_KEY".into(), "sk-should-never-arrive".into());
        CodexStart {
            session_id: "worker-1".into(),
            program: PathBuf::from(FAKE),
            child_path: "/usr/bin:/bin".into(),
            spec: CodexSpec {
                workdir: self.dir.path().to_path_buf(),
                developer_instructions: Some("你是 crew 的一员".into()),
                initial_prompt: brief.map(str::to_string),
                initial_context: [("crew_whiteboard".to_string(), "A: 别动 main".to_string())]
                    .into_iter()
                    .collect(),
                additional_env,
                ..CodexSpec::default()
            },
            ledger,
        }
    }

    /// 假 codex 收到的每一条消息。
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path("rpc.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// Wire a comms spec (token `token`) onto a start.
    fn with_comms(start: &mut CodexStart, token: &str) {
        start.spec.comms = Some(CommsSpec {
            helper: PathBuf::from("/opt/crewd/bin/crewd"),
            data_dir: PathBuf::from("/data"),
            session_id: "worker-1".into(),
            token: token.into(),
            hooks: false,
            managed_hooks: false,
        });
    }

    /// The `--token-file` path handed to the `n`-th `thread/start` (0-based).
    fn token_file_of(&self, n: usize) -> PathBuf {
        let start = &self.sent("thread/start")[n]["params"];
        let args = start["config"]["mcp_servers"]["crew"]["args"]
            .as_array()
            .unwrap();
        let i = args.iter().position(|a| a == "--token-file").unwrap();
        PathBuf::from(args[i + 1].as_str().unwrap())
    }

    fn sent(&self, method: &str) -> Vec<Value> {
        self.log()
            .into_iter()
            .filter(|v| v["method"] == method)
            .collect()
    }

    /// 我们对服务端请求 `id` 的回应。
    fn reply_to(&self, id: Value) -> Value {
        self.log()
            .into_iter()
            .find(|v| v.get("method").is_none() && v["id"] == id)
            .unwrap_or_else(|| panic!("没回 {id}"))
    }

    fn pid_in(&self, name: &str) -> i32 {
        std::fs::read_to_string(self.path(name))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: 信号 0 只做存在性检查。
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn gone(pid: i32) {
    for _ in 0..100 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("进程 {pid} 3s 后还活着");
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

fn exited(e: &SessionEvent) -> Option<&ExitInfo> {
    match e {
        SessionEvent::Exited(info) => Some(info),
        _ => None,
    }
}

fn is_idle_edge(e: &SessionEvent) -> bool {
    *e == SessionEvent::Working(false)
}

async fn stop(h: &SessionHandle, rx: &mut UnboundedReceiver<SessionEvent>) -> ExitInfo {
    h.stop().unwrap();
    let events = until(rx, |e| exited(e).is_some()).await;
    exited(events.last().unwrap()).unwrap().clone()
}

#[tokio::test]
async fn fresh_start_records_the_thread_and_runs_the_brief_with_its_context() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let mut start = env.start("", ledger.clone(), Some("开场任务"));
    Env::with_comms(&mut start, SENTINEL);
    let (h, mut rx) = start_codex(start, env.config()).await.unwrap();
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
    let events = until(&mut rx, is_idle_edge).await;
    assert!(events.contains(&SessionEvent::Delivery {
        id: BRIEF_DELIVERY_ID,
        subject: crewd_exec::delivery::Subject::Brief,
        outcome: DeliveryOutcome::Delivered,
    }));
    assert!(events.contains(&SessionEvent::TurnEnded {
        turn_id: "turn-1".into(),
        last_agent_message: "GOT: 开场任务".into()
    }));
    assert!(events.contains(&SessionEvent::Profile(Profile {
        model: Some("fake-model".into()),
        effort: Some("medium".into()),
        fast: None,
    })));

    let methods: Vec<String> = env
        .log()
        .iter()
        .filter_map(|v| v["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        methods,
        vec!["initialize", "initialized", "thread/start", "turn/start"]
    );
    let start = &env.sent("thread/start")[0]["params"];
    assert_eq!(start["developerInstructions"], "你是 crew 的一员");
    assert_eq!(start["approvalPolicy"], "on-request");
    assert_eq!(start["sandbox"], "workspace-write");
    assert_eq!(start["approvalsReviewer"], "auto_review");
    let crew = &start["config"]["mcp_servers"]["crew"];
    assert_eq!(crew["command"], "/opt/crewd/bin/crewd");
    let args: Vec<&str> = crew["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(args[0], "mcp");
    assert_eq!(args.len(), 7, "{args:?}");
    assert_eq!(
        &args[1..5],
        ["--data-dir", "/data", "--session", "worker-1"]
    );
    assert_eq!(args[5], "--token-file");
    assert!(!args.contains(&"--agent"), "{args:?}");
    let token_file = Path::new(args[6]);
    assert_eq!(std::fs::read_to_string(token_file).unwrap(), SENTINEL);
    assert_eq!(
        std::fs::metadata(token_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        token_file.starts_with(env.path("run")),
        "token file lives under CodexConfig.runtime_dir: {token_file:?}"
    );
    let turn = &env.sent("turn/start")[0]["params"];
    assert_eq!(turn["threadId"], agent_session_id.as_str());
    assert_eq!(turn["input"][0]["text"], "开场任务");
    assert_eq!(
        turn["additionalContext"]["crew_whiteboard"]["kind"],
        "untrusted"
    );

    for log in ["rpc.log", "env.log"] {
        let text = std::fs::read_to_string(env.path(log)).unwrap();
        assert!(!text.contains(SENTINEL), "token leaked into {log}");
    }
    let names = std::fs::read_to_string(env.path("env.log")).unwrap();
    assert!(!names.lines().any(|n| n == "OPENAI_API_KEY"), "{names}");
    assert!(names.lines().any(|n| n == "FAKE_CODEX_MODE"));

    assert!(h.screen_text(20).await.unwrap().contains("GOT: 开场任务"));
    assert_eq!(h.snapshot().await.unwrap(), None, "codex 没有终端");
    let status = h.status().await.unwrap();
    assert_eq!(status.agent_session_id, agent_session_id);
    assert!(!status.working);
    let info = stop(&h, &mut rx).await;
    assert!(info.stopped_by_user);
    assert!(
        !token_file.exists(),
        "token file is gone by the time Exited is observed"
    );
}

const SENTINEL: &str = "tok-SENTINEL-7f3a91";

/// A quick restart with the same crewd session id must keep its own token file: the old
/// launch directory is removed before `Exited`, and directories are unique per launch.
#[tokio::test]
async fn restarting_on_exited_keeps_the_new_token_file() {
    let env = Env::new();
    let mut first = env.start("", Arc::new(MemoryLedger::default()), None);
    Env::with_comms(&mut first, "tok-one");
    let (h, mut rx) = start_codex(first, env.config()).await.unwrap();
    next(&mut rx).await;
    let old = env.token_file_of(0);
    assert!(old.exists());
    stop(&h, &mut rx).await;
    assert!(!old.exists(), "old token file gone at Exited");
    let mut second = env.start("", Arc::new(MemoryLedger::default()), None);
    Env::with_comms(&mut second, "tok-two");
    let (h2, mut rx2) = start_codex(second, env.config()).await.unwrap();
    next(&mut rx2).await;
    let new = env.token_file_of(1);
    assert_ne!(old, new, "launch directories are unique per launch");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_to_string(&new).unwrap(), "tok-two");
    stop(&h2, &mut rx2).await;
    assert!(!new.exists());
}

/// A start that fails at the handshake leaves no launch directory behind when it returns.
#[tokio::test]
async fn a_failed_start_leaves_no_token_file() {
    let env = Env::new();
    let mut config = env.config();
    config.core.thread_timeout = Duration::from_millis(500);
    config.core.initialize_timeout = Duration::from_millis(500);
    let mut start = env.start("init_silent", Arc::new(MemoryLedger::default()), None);
    Env::with_comms(&mut start, "tok-fail");
    let err = start_codex(start, config).await.err().expect("must fail");
    assert!(matches!(err, StartError::Handshake { .. }), "{err:?}");
    let left: Vec<_> = std::fs::read_dir(env.path("run"))
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "launch dir left behind: {left:?}");
}

#[tokio::test]
async fn a_recorded_thread_is_resumed_without_its_turns() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("thr-old")));
    let (h, mut rx) = start_codex(env.start("", ledger.clone(), None), env.config())
        .await
        .unwrap();
    assert_eq!(
        next(&mut rx).await,
        SessionEvent::Spawned {
            pid: env.pid_in("pid") as u32,
            agent_session_id: "thr-old".into(),
            resumed: true,
        }
    );
    let resume = &env.sent("thread/resume")[0]["params"];
    assert_eq!(resume["threadId"], "thr-old");
    assert_eq!(resume["excludeTurns"], true);
    assert!(env.sent("thread/start").is_empty());
    assert_eq!(ledger.current().as_deref(), Some("thr-old"));
    stop(&h, &mut rx).await;
}

/// 续跑被拒：同一个进程里退回新 thread、新号先落账；事件与 claude 同序——旧号的 `Spawned`、
/// `ResumeRefused`、新号的 `Spawned`（每条 `ResumeRefused` 后面都跟一条 `Spawned`）。
#[tokio::test]
async fn a_refused_resume_falls_back_to_a_fresh_thread_and_records_it() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("thr-gone")));
    let (h, mut rx) = start_codex(
        env.start("resume_fails", ledger.clone(), Some("接着干")),
        env.config(),
    )
    .await
    .unwrap();
    let pid = env.pid_in("pid") as u32;
    assert_eq!(
        next(&mut rx).await,
        SessionEvent::Spawned {
            pid,
            agent_session_id: "thr-gone".into(),
            resumed: true,
        },
        "第一条事件必须是 Spawned（去接旧号）"
    );
    let SessionEvent::ResumeRefused { id, said } = next(&mut rx).await else {
        panic!("紧跟着说清楚原 thread 接不回来");
    };
    assert_eq!(id, "thr-gone");
    assert!(said.contains("no rollout found"), "{said}");
    let SessionEvent::Spawned {
        agent_session_id,
        resumed: false,
        pid: same_pid,
    } = next(&mut rx).await
    else {
        panic!("ResumeRefused 后面必须跟一条新号的 Spawned");
    };
    assert_eq!(same_pid, pid, "codex 在同一个进程里新开 thread");
    assert_ne!(agent_session_id, "thr-gone");
    assert_eq!(ledger.current(), Some(agent_session_id.clone()));
    assert_eq!(h.status().await.unwrap().agent_session_id, agent_session_id);
    until(&mut rx, is_idle_edge).await;
    assert_eq!(
        env.sent("turn/start")[0]["params"]["threadId"],
        agent_session_id.as_str()
    );
    stop(&h, &mut rx).await;
}

/// Review Focus 1：每一种服务端请求都必须被回答（整数和字符串 id 都原样回），否则回合挂死。
#[tokio::test]
async fn every_server_request_is_answered_and_the_turn_finishes() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "server_requests",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| {
        matches!(
            e,
            SessionEvent::Decision(crewd_term::DecisionEvent::Appeared(_))
        )
    })
    .await;
    let request = h.status().await.unwrap().pending_decision.unwrap();
    assert_eq!(
        h.answer_choice(request.fingerprint(), 2).await.unwrap(),
        crewd_exec::session::AnswerOutcome::Answered {
            cleared: true,
            now: None
        }
    );
    let events = until(&mut rx, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
    assert!(h.screen_text(50).await.unwrap().contains("answered 6"));
    assert_eq!(env.reply_to(100.into())["result"]["decision"], "decline");
    assert!(matches!(
        h.answer_choice(request.fingerprint(), 0).await.unwrap(),
        crewd_exec::session::AnswerOutcome::Conflict { .. }
    ));
    assert_eq!(
        env.reply_to("elicit-tool".into())["result"]["action"],
        "accept"
    );
    assert_eq!(
        env.reply_to("elicit-form".into())["result"]["action"],
        "decline"
    );
    assert_eq!(env.reply_to(101.into())["error"]["code"], -32601);
    assert_eq!(env.reply_to(102.into())["error"]["code"], -32601);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let told = env.reply_to(103.into())["result"]["currentTimeAt"]
        .as_i64()
        .unwrap();
    assert!((told - now).abs() < 60, "{told} vs {now}");
    assert!(
        events.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired)
        )
    );
    let notices = events
        .iter()
        .filter(|e| matches!(e, SessionEvent::Notice(_)))
        .count();
    assert_eq!(
        notices, 2,
        "代拒的表单 + 不支持的 requestUserInput：{events:?}"
    );
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn a_usage_limit_from_the_protocol_is_health_and_marks_the_exit() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "usage_limit",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::UsageLimit),
    )
    .await;
    until(&mut rx, is_idle_edge).await;
    assert_eq!(
        h.status().await.unwrap().state,
        crewd_exec::session::DerivedState::RateLimited
    );
    let info = stop(&h, &mut rx).await;
    assert!(info.hit_limit && info.stopped_by_user);
}

#[tokio::test]
async fn a_turn_start_that_never_answers_times_out_and_the_session_stays_controllable() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "turn_hangs",
            Arc::new(MemoryLedger::default()),
            Some("开场"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    let Some(SessionEvent::Delivery {
        outcome: DeliveryOutcome::Undelivered(why),
        ..
    }) = events.last()
    else {
        panic!("{events:?}");
    };
    assert!(why.contains("没有回应"), "{why}");
    assert!(!h.status().await.unwrap().working);
    stop(&h, &mut rx).await;
}

/// Review Focus 2：codex 往 stderr 狂写日志，没人读就卡死在第 64KB——握手必须照样完成。
#[tokio::test]
async fn a_stderr_flood_does_not_stall_the_handshake() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("stderr_flood", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .expect("stderr 没被排空，握手卡住了");
    assert!(matches!(next(&mut rx).await, SessionEvent::Spawned { .. }));
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn handshake_failures_are_start_errors_and_leave_no_process() {
    let env = Env::new();
    let err = start_codex(
        env.start("die_on_init", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .err()
    .expect("必须报错");
    let StartError::Handshake { health } = err else {
        panic!("{err:?}");
    };
    assert_eq!(health.launch_verdict, Some(LaunchVerdict::DiedSilently));
    assert!(health.detail.contains("not logged in"), "{}", health.detail);

    let env = Env::new();
    let mut config = env.config();
    config.core.initialize_timeout = Duration::from_millis(500);
    let err = start_codex(
        env.start("init_silent", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .err()
    .expect("必须报错");
    let StartError::Handshake { health } = err else {
        panic!("{err:?}");
    };
    assert_eq!(health.launch_verdict, Some(LaunchVerdict::Stalled));
    gone(env.pid_in("pid")).await;

    let env = Env::new();
    let mut start = env.start("", Arc::new(MemoryLedger::default()), None);
    start.spec.workdir = env.path("no-such-dir");
    let err = start_codex(start, env.config())
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Spawn { .. }), "{err:?}");
}

/// 账本写不进去。
struct BrokenLedger;

impl AgentIdLedger for BrokenLedger {
    fn recorded(&self) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    fn record(&self, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("库坏了"))
    }
}

/// Review Focus 3：thread id 落不了账就不能干活（起了也接不回来）——报错、杀掉进程、一个回合都不开。
#[tokio::test]
async fn ledger_failure_kills_the_process_before_any_turn() {
    let env = Env::new();
    let err = start_codex(
        env.start("", Arc::new(BrokenLedger), Some("开场")),
        env.config(),
    )
    .await
    .err()
    .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    gone(env.pid_in("pid")).await;
    assert!(env.sent("turn/start").is_empty(), "落账失败后不该开回合");
}

/// Review Focus 4：codex 起的工具子命令（`sleep 600`）在停止时一起收掉；codex 不理 SIGTERM 时
/// 宽限后升级成整组 SIGKILL。
#[tokio::test]
async fn stop_kills_the_whole_group_including_tool_children() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "spawn_child,ignore_term",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, is_idle_edge).await;
    let child = env.pid_in("child.pid");
    assert!(alive(child));
    let info = stop(&h, &mut rx).await;
    assert_eq!(
        info,
        ExitInfo {
            status: ExitStatusKind::Signal(9),
            stopped_by_user: true,
            hit_limit: false,
        }
    );
    gone(child).await;
}

/// 反复叫停（比 `stop_grace` 还勤）：SIGTERM 只发一次，SIGKILL 的期限也不往后推——
/// 否则每次 `stop()` 都把升级重新计时，一个不理 SIGTERM 的 codex 就永远杀不掉（与 claude 驱动同一条）。
#[tokio::test]
async fn repeated_stop_neither_resends_term_nor_postpones_the_kill() {
    let env = Env::new();
    let mut config = env.config();
    config.stop_grace = Duration::from_secs(1);
    let (h, mut rx) = start_codex(
        env.start("count_term", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let t0 = std::time::Instant::now();
    for _ in 0..3 {
        h.stop().unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
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
    assert_eq!(
        std::fs::read_to_string(env.path("term.log")).unwrap(),
        "T",
        "SIGTERM 只发一次"
    );
}

/// 登记表写不进去（孤儿兜底失效）：杀掉刚起的进程、报 `StartError::Registry`，不挂住。
#[tokio::test]
async fn a_registry_that_cannot_be_written_kills_the_process() {
    let env = Env::new();
    // 父目录不存在：load 当空表，record 落盘失败。
    let registry = Arc::new(ProcessRegistry::load(&env.path("missing").join("r.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        start_codex(
            env.start("", Arc::new(MemoryLedger::default()), None),
            config,
        ),
    )
    .await
    .expect("登记表失败后 10s 内要返回（收尸不能挂住）")
    .err()
    .expect("必须报错");
    assert!(matches!(err, StartError::Registry(_)), "{err:?}");
    // 不杀的话 5s 收尸上限到点才回来，说明里带着「没能收尸」。
    assert!(!err.to_string().contains("没能收尸"), "{err}");
    assert!(registry.entries().is_empty());
    // 进程可能在写下 pid 文件之前就被杀了；写下了就必须已经不在。
    if env.path("pid").exists() {
        gone(env.pid_in("pid")).await;
    }
    assert!(env.sent("initialize").is_empty(), "登记失败后不该握手");
}

/// codex 自己退出时（崩溃 / 被人在外面杀掉），它起的工具子命令也不能变成孤儿。
#[tokio::test]
async fn tool_children_do_not_outlive_codex_exiting_on_its_own() {
    let env = Env::new();
    let (_h, mut rx) = start_codex(
        env.start(
            "spawn_child,exit_after_turn",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令就走"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    let info = exited(events.last().unwrap()).unwrap();
    assert_eq!(info.status, ExitStatusKind::Code(0));
    assert!(!info.stopped_by_user);
    gone(env.pid_in("child.pid")).await;
}

/// 工具子命令继承了 codex 的 stdout / stderr、codex 退出后管道仍被握着：收尾对读管道的等待有上限
/// （各 500ms），之后整组补刀、`Exited` 照样发出，子命令不留成孤儿（计划裁定 21）。
#[tokio::test]
async fn a_tool_child_holding_the_pipes_open_neither_delays_exited_nor_survives() {
    let env = Env::new();
    let (_h, mut rx) = start_codex(
        env.start(
            "spawn_child_inherit,exit_after_turn",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令就走"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
    let t0 = std::time::Instant::now();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    let took = t0.elapsed();
    let info = exited(events.last().unwrap()).unwrap();
    assert_eq!(info.status, ExitStatusKind::Code(0));
    assert!(!info.stopped_by_user);
    assert!(
        took < Duration::from_millis(2500),
        "被子命令握着的管道把收尾拖住了：{took:?}"
    );
    // `Exited` 之前就已经整组补过刀：子命令这时不该还活着（等它被 init 收尸最多一小会儿）。
    gone(env.pid_in("child.pid")).await;
}

/// 同上，走用户叫停的路径（codex 听 SIGTERM 自己退，子命令握着管道）。
#[tokio::test]
async fn stopping_with_a_tool_child_holding_the_pipes_still_exits_and_cleans_the_group() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "spawn_child_inherit",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, is_idle_edge).await;
    let child = env.pid_in("child.pid");
    assert!(alive(child));
    let t0 = std::time::Instant::now();
    let info = stop(&h, &mut rx).await;
    assert!(info.stopped_by_user);
    assert!(
        t0.elapsed() < Duration::from_millis(2500),
        "{:?}",
        t0.elapsed()
    );
    gone(child).await;
}

#[tokio::test]
async fn high_context_triggers_one_compaction_and_a_send_waits_for_it() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "big_context",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
    // 回合结束后的空闲边沿之后，下一次「干活」只可能是自动压缩（token 读数晚于完成到达）。
    // 等它开始了再投：投早了会赶在读数之前开回合，那一轮就不压（见核心的测试）。
    until(&mut rx, |e| *e == SessionEvent::Working(true)).await;
    let id = h.send("压缩之后").unwrap();
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Delivery { id: i, .. } if *i == id),
    )
    .await;
    until(&mut rx, is_idle_edge).await;
    let order: Vec<String> = env
        .log()
        .iter()
        .filter_map(|v| v["method"].as_str().map(str::to_string))
        .filter(|m| m == "turn/start" || m == "thread/compact/start")
        .collect();
    assert_eq!(
        order,
        vec!["turn/start", "thread/compact/start", "turn/start"]
    );
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn interrupt_ends_a_running_turn_and_raw_keys_are_refused_with_a_notice() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "slow_turn",
            Arc::new(MemoryLedger::default()),
            Some("慢慢来"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    assert!(h.status().await.unwrap().working);
    h.raw(b"y\r".to_vec()).unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::Notice(_))).await;
    h.interrupt().unwrap();
    let events = until(&mut rx, is_idle_edge).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnEnded { .. })),
        "被打断的回合不算一次回答：{events:?}"
    );
    assert_eq!(env.sent("turn/interrupt")[0]["params"]["turnId"], "turn-1");
    stop(&h, &mut rx).await;
}

/// Review Focus 5：codex 不读 stdin 了（卡死 / 正忙），我们却要写一大段——会话不能被卡死。
#[tokio::test]
async fn a_server_that_stops_reading_cannot_wedge_the_session() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("deaf", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let big = "很长的正文。".repeat(200_000);
    let id = h.send(big).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(1), h.status())
        .await
        .expect("写阻塞把会话卡住了")
        .unwrap();
    assert!(status.working);
    let events = until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    assert!(matches!(
        events.last(),
        Some(SessionEvent::Delivery { id: i, outcome: DeliveryOutcome::Undelivered(_), .. }) if *i == id
    ));
    let info = tokio::time::timeout(Duration::from_secs(5), stop(&h, &mut rx))
        .await
        .expect("停不掉");
    assert!(info.stopped_by_user);
}

#[tokio::test]
async fn a_three_megabyte_item_arrives_whole() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "huge_item",
            Arc::new(MemoryLedger::default()),
            Some("写个大的"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, is_idle_edge).await;
    let big = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Item { item, .. } if item["id"] == "turn-1-big" => {
                item["text"].as_str().map(str::len)
            }
            _ => None,
        })
        .expect("3MB 的条目没到");
    assert_eq!(big, 3 * 1024 * 1024);
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn controls_round_trip_through_the_protocol() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    next(&mut rx).await;
    assert!(matches!(
        h.control(Control::Fast(true)).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    assert!(matches!(
        h.control(Control::Model("o5".into())).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    assert!(matches!(
        h.control(Control::Compact).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    let updates = env.sent("thread/settings/update");
    assert_eq!(updates[0]["params"]["serviceTier"], "fast");
    assert_eq!(updates[1]["params"]["model"], "o5");
    assert_eq!(env.sent("thread/compact/start").len(), 1);
    h.resize(10, 10).unwrap();
    let events = until(
        &mut rx,
        |e| matches!(e, SessionEvent::Profile(p) if p.fast == Some(true)),
    )
    .await;
    assert!(!events.is_empty());
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn the_registry_holds_the_live_process_and_is_cleared_on_exit() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.path("exec-registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let entries = registry.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "worker-1");
    assert_eq!(entries[0].1.pid, env.pid_in("pid"));
    stop(&h, &mut rx).await;
    for _ in 0..50 {
        if registry.entries().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("退出后登记表没清");
}

#[tokio::test]
async fn dropping_every_handle_stops_the_session() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    let h2 = h.clone();
    drop(h);
    drop(h2);
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(exited(events.last().unwrap()).unwrap().stopped_by_user);
    gone(env.pid_in("pid")).await;
}

/// 同一个 thread 在本进程里只能跑一个（与 claude 共用一张表）；前一个退出后立刻能再起。
#[tokio::test]
async fn a_running_thread_cannot_be_resumed_twice_but_is_free_once_exited() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let (h, mut rx) = start_codex(env.start("", ledger.clone(), None), env.config())
        .await
        .unwrap();
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = next(&mut rx).await
    else {
        panic!("第一条事件必须是 Spawned");
    };
    let twin = Env::new();
    let err = start_codex(
        twin.start(
            "",
            Arc::new(MemoryLedger::with(Some(&agent_session_id))),
            None,
        ),
        twin.config(),
    )
    .await
    .err()
    .expect("同一个 thread 不能同时再起");
    assert!(
        matches!(&err, StartError::AlreadyRunning(id) if *id == agent_session_id),
        "{err:?}"
    );
    assert!(!twin.path("pid").exists(), "占号失败时不该起进程");
    stop(&h, &mut rx).await;
    let (h2, mut rx2) = start_codex(
        twin.start(
            "",
            Arc::new(MemoryLedger::with(Some(&agent_session_id))),
            None,
        ),
        twin.config(),
    )
    .await
    .expect("前一个见到 Exited 之后就能再起");
    assert!(matches!(
        next(&mut rx2).await,
        SessionEvent::Spawned { resumed: true, .. }
    ));
    stop(&h2, &mut rx2).await;
}

/// 启动失败也是终态：`start_codex` 返回 `Err` 时，占着的 thread id 已放开、进程已收尸——
/// 同一个账本立刻就能再起（计划裁定 21 对启动失败同样成立）。
#[tokio::test]
async fn a_failed_start_releases_the_thread_id_before_it_returns() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("thr-stale")));
    let mut config = env.config();
    config.core.thread_timeout = Duration::from_millis(500);
    config.core.initialize_timeout = Duration::from_millis(500);
    let err = start_codex(env.start("init_silent", ledger.clone(), None), config)
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Handshake { .. }), "{err:?}");
    let (h, mut rx) = start_codex(env.start("", ledger, None), env.config())
        .await
        .expect("失败的启动返回之后，同一个账本要能立刻再起（不是 AlreadyRunning）");
    assert!(matches!(
        next(&mut rx).await,
        SessionEvent::Spawned { resumed: true, .. }
    ));
    stop(&h, &mut rx).await;
}

/// 落账失败：返回 `Err` 的那一刻登记表已经清了（同步检查，不轮询）。
#[tokio::test]
async fn a_ledger_failure_has_cleared_the_registry_by_the_time_start_returns() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.path("exec-registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let err = start_codex(env.start("", Arc::new(BrokenLedger), None), config)
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    assert!(
        registry.entries().is_empty(),
        "返回时登记表里还留着死进程：{:?}",
        registry.entries()
    );
    gone(env.pid_in("pid")).await;
}

/// 失败后立刻用同一个 session id 重起：旧任务迟到的 `forget` 不能抹掉新进程的登记。
#[tokio::test]
async fn restarting_right_after_a_failed_start_keeps_the_new_registry_entry() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.path("exec-registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let err = start_codex(env.start("", Arc::new(BrokenLedger), None), config.clone())
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let new_pid = env.pid_in("pid");
    // 给旧任务足够的时间把迟到的 forget 做完（修复前它会抹掉下面这条）。
    tokio::time::sleep(Duration::from_millis(300)).await;
    let entries = registry.entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].1.pid, new_pid);
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn codex_success_is_observed_before_delivery_broadcast_and_only_after_acceptance() {
    use crewd_exec::driver::{DeliveryBinding, DeliverySuccessObserver, SendError};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Witness {
        registered: Mutex<Vec<u64>>,
        successes: Mutex<Vec<u64>>,
    }
    impl DeliveryBinding for Witness {
        fn register(&self, id: u64) -> Result<(), SendError> {
            self.registered.lock().unwrap().push(id);
            Ok(())
        }
        fn cancelled(&self, id: u64) {
            self.registered.lock().unwrap().retain(|x| *x != id);
        }
    }
    impl DeliverySuccessObserver for Witness {
        fn emitted(&self, id: u64) {
            assert!(
                self.registered.lock().unwrap().contains(&id),
                "success without exact source"
            );
            self.successes.lock().unwrap().push(id);
        }
    }
    for mode in ["", "turn_hangs"] {
        let env = Env::new();
        let witness = Arc::new(Witness::default());
        let mut config = env.config();
        config.delivery_observer = Some(witness.clone());
        let (handle, mut events) = start_codex(
            env.start(mode, Arc::new(MemoryLedger::default()), None),
            config,
        )
        .await
        .unwrap();
        let id = handle
            .send_bound("bound task".into(), BTreeMap::new(), &*witness)
            .unwrap();
        let result = until(
            &mut events,
            |e| matches!(e, SessionEvent::Delivery { id: delivered, .. } if *delivered == id),
        )
        .await;
        let SessionEvent::Delivery { outcome, .. } = result.last().unwrap() else {
            unreachable!()
        };
        if mode.is_empty() {
            assert_eq!(outcome, &DeliveryOutcome::Delivered);
            assert_eq!(
                *witness.successes.lock().unwrap(),
                vec![id],
                "observer must run before the delivered event is visible"
            );
        } else {
            assert!(matches!(outcome, DeliveryOutcome::Undelivered(_)));
            assert!(
                witness.successes.lock().unwrap().is_empty(),
                "queueing, stdin write or timeout must not count as success"
            );
        }
        stop(&handle, &mut events).await;
        assert_eq!(
            witness.successes.lock().unwrap().len(),
            usize::from(mode.is_empty())
        );
    }
}
