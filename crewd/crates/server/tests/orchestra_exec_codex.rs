//! Codex 适配器的真实假进程验收；不接账户、真实 runner 或用户数据。
#[path = "support/t11.rs"]
mod support;
use crewd_exec::{
    driver::{DeliveryBinding, DeliverySuccessObserver, SendError},
    resume::{AgentIdLedger, MemoryLedger},
    session::{DeliveryOutcome, SessionEvent},
};
use crewd_server::orchestra::{
    exec::{Executor, LaunchError, LaunchRequest},
    exec_codex::{CodexExecutor, CodexExecutorConfig, CodexProgram},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

fn executor(root: &std::path::Path, program: CodexProgram) -> CodexExecutor {
    CodexExecutor::new(CodexExecutorConfig {
        program,
        data_dir: root.join("data"),
        helper: PathBuf::from("/synthetic/crewd"),
        registry: None,
        runtime_dir: root.join("run"),
        child_path: Some(support::CHILD_PATH.into()),
        extra_env: BTreeMap::from([(
            "FAKE_CODEX_LOG".into(),
            root.join("rpc.log").display().to_string(),
        )]),
    })
}
fn request(root: &std::path::Path, ledger: Arc<MemoryLedger>) -> LaunchRequest {
    LaunchRequest {
        initial_inputs: None,
        codex_options: None,
        title: None,

        session_id: "synthetic-worker".into(),
        crew_id: "crew-a".into(),
        workdir: root.into(),
        model: Some("synthetic-model".into()),
        effort: Some("high".into()),
        system_prompt: Some("Synthetic crew worldview".into()),
        initial_prompt: None,
        initial_context: Default::default(),
        token: "synthetic-test-token".into(),
        ledger,
        delivery_observer: None,
    }
}
#[tokio::test]
async fn disabled_and_missing_codex_refuse_before_launch_files() {
    let dir = tempfile::tempdir().unwrap();
    for program in [
        CodexProgram::Disabled,
        CodexProgram::Path(dir.path().join("missing")),
    ] {
        let error = executor(dir.path(), program)
            .launch(request(dir.path(), Arc::new(MemoryLedger::default())))
            .await
            .err()
            .unwrap();
        assert!(matches!(error, LaunchError::NotConfigured(_)));
        assert!(!dir.path().join("run").exists());
    }
}
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
        assert!(self.registered.lock().unwrap().contains(&id));
        self.successes.lock().unwrap().push(id);
    }
}
#[tokio::test]
async fn codex_adapter_preserves_context_source_thread_and_stop_resume() {
    let dir = tempfile::tempdir().unwrap();
    let executor = executor(
        dir.path(),
        CodexProgram::Path(support::artifact("fake_codex")),
    );
    let ledger = Arc::new(MemoryLedger::default());
    let witness = Arc::new(Witness::default());
    let mut req = request(dir.path(), ledger.clone());
    req.delivery_observer = Some(witness.clone());
    let (handle, mut events) = executor.launch(req).await.unwrap();
    let deadline = Duration::from_secs(10);
    assert!(matches!(
        tokio::time::timeout(deadline, events.recv()).await.unwrap(),
        Some(SessionEvent::Spawned { .. })
    ));
    let thread = ledger.recorded().unwrap().unwrap();
    assert!(handle.accepts_context());
    let context = BTreeMap::from([("crew_state".into(), "untrusted state".into())]);
    let id = handle
        .send_bound("Implement synthetic task".into(), &context, &*witness)
        .unwrap();
    tokio::time::timeout(deadline, async {
        loop {
            match events.recv().await.unwrap() {
                SessionEvent::Delivery {
                    id: got, outcome, ..
                } if got == id => {
                    assert_eq!(outcome, DeliveryOutcome::Delivered);
                    assert_eq!(*witness.successes.lock().unwrap(), vec![id]);
                }
                SessionEvent::TurnEnded { .. } => break,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(*witness.successes.lock().unwrap(), vec![id]);
    let log = || {
        std::fs::read_to_string(dir.path().join("rpc.log"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>()
    };
    let calls = log();
    let start = calls
        .iter()
        .find(|v| v["method"] == "thread/start")
        .unwrap();
    assert_eq!(
        start["params"]["developerInstructions"],
        "Synthetic crew worldview"
    );
    assert_eq!(start["params"]["model"], "synthetic-model");
    let args = start["params"]["config"]["mcp_servers"]["crew"]["args"]
        .as_array()
        .unwrap();
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--session" && w[1] == "synthetic-worker")
    );
    assert!(args.iter().any(|v| v == "--token-file"));
    assert!(!args.iter().any(|v| v == "synthetic-test-token"));
    let turn = calls.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert!(turn["params"].to_string().contains("untrusted state"));
    assert!(handle.snapshot().await.unwrap().is_none());
    assert!(handle.screen_text(100).await.unwrap().contains("GOT:"));
    handle.stop().unwrap();
    tokio::time::timeout(deadline, async {
        while !matches!(events.recv().await.unwrap(), SessionEvent::Exited(_)) {}
    })
    .await
    .unwrap();
    let (resumed, mut events) = executor.launch(request(dir.path(), ledger)).await.unwrap();
    tokio::time::timeout(deadline, async {
        loop {
            if let SessionEvent::Spawned {
                agent_session_id, ..
            } = events.recv().await.unwrap()
            {
                assert_eq!(agent_session_id, thread);
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        log()
            .iter()
            .any(|v| v["method"] == "thread/resume" && v["params"]["threadId"] == thread)
    );
    resumed.stop().unwrap();
    tokio::time::timeout(deadline, async {
        while !matches!(events.recv().await.unwrap(), SessionEvent::Exited(_)) {}
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn first_codex_turn_carries_work_context() {
    let dir = tempfile::tempdir().unwrap();
    let exec = executor(
        dir.path(),
        CodexProgram::Path(support::artifact("fake_codex")),
    );
    assert!(exec.accepts_initial_context());
    let mut req = request(dir.path(), Arc::new(MemoryLedger::default()));
    req.initial_prompt = Some("Initial synthetic task".into());
    req.initial_context =
        BTreeMap::from([("my_work_items".into(), "synthetic assigned work".into())]);
    let (h, mut events) = exec.launch(req).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(events.recv().await.unwrap(), SessionEvent::TurnEnded { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let text = std::fs::read_to_string(dir.path().join("rpc.log")).unwrap();
    let calls: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let turn = calls.iter().find(|v| v["method"] == "turn/start").unwrap();
    assert_eq!(
        turn["params"]["additionalContext"]["my_work_items"],
        serde_json::json!({"kind":"untrusted","value":"synthetic assigned work"})
    );
    h.stop().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(events.recv().await.unwrap(), SessionEvent::Exited(_)) {}
    })
    .await
    .unwrap();
}
