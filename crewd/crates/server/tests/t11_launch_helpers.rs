//! Production launch configuration -> compiled helper -> real conn/RPC -> shared world.
//! Fixture socket is real; serve lifecycle and actual provider execution are separate scopes.
#![cfg(feature = "testkit")]
use crewd_exec::{resume::MemoryLedger, session::SessionEvent};
use crewd_server::{
    agent::{Role, catalog, port, testkit::Fixture},
    control::{QuotaLine, QuotaReport},
    orchestra::{
        db::DbPort,
        exec::{ExecHandle, Executor, LaunchRequest},
        exec_claude::{ClaudeExecutor, ClaudeExecutorConfig, ClaudeProgram},
        port::Port,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
struct StopOnDrop(Arc<dyn ExecHandle>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}
fn child_env(c: &mut Command, home: &Path) {
    c.env_clear()
        .env("HOME", home)
        .env("SHELL", "/usr/bin/false")
        .env("PATH", "/usr/bin:/bin")
        .env("CREWD_CLAUDE", "/usr/bin/false");
}
async fn hook(command: &str, home: &Path) -> Value {
    let mut c = Command::new("/bin/sh");
    c.args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    child_env(&mut c, home);
    let mut child = c.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(br#"{"hook_event_name":"SessionStart","source":"resume"}"#)
        .await
        .unwrap();
    drop(input);
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actual helper returned SessionStart JSON")
}
async fn mcp_tools(config: &Value, home: &Path) -> Vec<Value> {
    let server = &config["mcpServers"]["crew"];
    let mut c = Command::new(server["command"].as_str().unwrap());
    c.args(
        server["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap()),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .kill_on_drop(true);
    child_env(&mut c, home);
    let mut child = c.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    for (id, method, params) in [
        (
            1,
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic-t11","version":"0"}}),
        ),
        (2, "tools/list", json!({})),
    ] {
        input
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let response: Value = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let value: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().expect("helper reply"))
                        .unwrap();
                if value["id"] == id {
                    break value;
                }
            }
        })
        .await
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        if id == 1 {
            input
                .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                .await
                .unwrap();
        } else {
            let tools = response["result"]["tools"].as_array().unwrap().clone();
            drop(input);
            if tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .is_err()
            {
                child.kill().await.unwrap();
                child.wait().await.unwrap();
            }
            return tools;
        }
    }
    unreachable!()
}
#[tokio::test]
async fn actual_generated_helpers_re_read_role_and_shared_world_after_launch() {
    let fixture = Fixture::new().await;
    fixture.spawn_socket().await;
    fixture.set_quota(QuotaReport {
        lines: vec![QuotaLine {
            executor: "claude".into(),
            plan: Some("Synthetic Tier".into()),
            ..Default::default()
        }],
    });
    let local = tempfile::tempdir().unwrap();
    let home = local.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let runtime = local.path().join("runtime");
    let argv = local.path().join("argv");
    let fake = local.path().join("fake-claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexec /bin/sleep 30\n",
            argv.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let executable = std::env::current_exe().unwrap();
    let helper = executable
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("crewd");
    assert!(
        helper.exists(),
        "build crewd in this target before the synthetic helper caller test"
    );
    let port = DbPort::new(
        fixture.state.clone(),
        fixture.data_dir().join(crewd_store::DB_FILE),
    );
    let world = port.worldview(&fixture.worker).await.unwrap();
    let exec = ClaudeExecutor::new(ClaudeExecutorConfig {
        program: ClaudeProgram::Path(fake),
        data_dir: fixture.data_dir().into(),
        helper,
        registry: None,
        extra_env: BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            ("SHELL".into(), "/usr/bin/false".into()),
        ]),
        projects_dir: None,
        runtime_dir: Some(runtime),
        child_path: Some("/usr/bin:/bin".into()),
    });
    let (handle, mut events) = exec
        .launch(LaunchRequest {
            session_id: fixture.worker.clone(),
            crew_id: fixture.crew.clone(),
            workdir: local.path().into(),
            model: None,
            effort: None,
            system_prompt: Some(world.clone()),
            initial_prompt: None,
            token: fixture.token(&fixture.worker),
            ledger: Arc::new(MemoryLedger::default()),
        })
        .await
        .unwrap();
    let guard = StopOnDrop(handle);
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::Spawned { .. })
    ));
    let args = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&argv)
                && text.contains("--settings")
            {
                break text.lines().map(str::to_owned).collect::<Vec<_>>();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let file = |flag: &str| PathBuf::from(&args[args.iter().position(|s| s == flag).unwrap() + 1]);
    assert_eq!(
        std::fs::read_to_string(file("--append-system-prompt-file")).unwrap(),
        world
    );
    let mcp: Value = serde_json::from_slice(&std::fs::read(file("--mcp-config")).unwrap()).unwrap();
    let settings: Value =
        serde_json::from_slice(&std::fs::read(file("--settings")).unwrap()).unwrap();
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let before = hook(command, &home).await;
    let expected = port::live_world(&fixture.state, &fixture.worker)
        .await
        .unwrap();
    assert_eq!(before["hookSpecificOutput"]["additionalContext"], expected);
    assert!(expected.contains("Synthetic Tier"));
    let worker_tools = mcp_tools(&mcp, &home).await;
    assert_eq!(
        worker_tools.len(),
        catalog::tools_for(&fixture.me(&fixture.worker).await, crewd_collab::Locale::Zh).len()
    );
    fixture.set_role(&fixture.worker, Role::Captain).await;
    let after = hook(command, &home).await;
    let changed = port::live_world(&fixture.state, &fixture.worker)
        .await
        .unwrap();
    assert_eq!(after["hookSpecificOutput"]["additionalContext"], changed);
    assert_ne!(
        changed, expected,
        "the hook must not repeat the launch-time worker world"
    );
    let captain_tools = mcp_tools(&mcp, &home).await;
    assert_eq!(
        captain_tools.len(),
        catalog::tools_for(&fixture.me(&fixture.worker).await, crewd_collab::Locale::Zh).len()
    );
    assert_ne!(captain_tools.len(), worker_tools.len());
    guard.0.stop().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::Exited(_)) {
                return;
            }
        }
        panic!("missing synthetic exit");
    })
    .await
    .unwrap();
}
