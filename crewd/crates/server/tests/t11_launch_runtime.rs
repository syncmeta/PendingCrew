//! Synthetic PTY launch caller proof. The shell records argv/PATH and never runs a helper/provider.
use crewd_exec::{resume::MemoryLedger, session::SessionEvent};
use crewd_server::orchestra::{
    exec::{ExecHandle, Executor, LaunchRequest},
    exec_claude::{ClaudeExecutor, ClaudeExecutorConfig, ClaudeProgram},
};
use std::{
    collections::BTreeMap, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration,
};
struct StopOnDrop(Arc<dyn ExecHandle>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}
#[tokio::test]
async fn generated_launch_files_and_token_use_the_external_private_runtime() {
    let fixture = tempfile::tempdir().unwrap();
    let data = fixture.path().join("data");
    let runtime = fixture.path().join("external-runtime");
    let work = fixture.path().join("work");
    let home = fixture.path().join("home");
    for p in [&data, &work, &home] {
        std::fs::create_dir(p).unwrap();
    }
    let argv = fixture.path().join("argv");
    let path_log = fixture.path().join("path");
    let fake = fixture.path().join("synthetic-claude");
    std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' \"$PATH\" > '{}'\nexec /bin/sleep 30\n", argv.display(), path_log.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let child_path = "/synthetic/tool-bin:/usr/bin:/bin";
    let helper = fixture.path().join("synthetic-helper-not-executed");
    let exec = ClaudeExecutor::new(ClaudeExecutorConfig {
        program: ClaudeProgram::Path(fake),
        data_dir: data.clone(),
        helper: helper.clone(),
        registry: None,
        extra_env: BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            ("SHELL".into(), "/usr/bin/false".into()),
        ]),
        projects_dir: None,
        runtime_dir: Some(runtime.clone()),
        child_path: Some(child_path.into()),
    });
    let (handle, mut events) = exec
        .launch(LaunchRequest {
            delivery_observer: None,
            session_id: "synthetic-session".into(),
            crew_id: "synthetic-crew".into(),
            workdir: work,
            model: None,
            effort: None,
            system_prompt: Some("synthetic shared world".into()),
            initial_prompt: None,
            token: "synthetic-token".into(),
            ledger: Arc::new(MemoryLedger::default()),
        })
        .await
        .unwrap();
    let guard = StopOnDrop(handle);
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::Spawned { .. })
    ));
    let lines = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(s) = std::fs::read_to_string(&argv)
                && s.contains("--mcp-config")
                && path_log.exists()
            {
                break s.lines().map(str::to_owned).collect::<Vec<_>>();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("synthetic executable wrote argv");
    let file_for =
        |flag: &str| PathBuf::from(&lines[lines.iter().position(|s| s == flag).unwrap() + 1]);
    let mcp_file = file_for("--mcp-config");
    let world_file = file_for("--append-system-prompt-file");
    let settings_file = file_for("--settings");
    for p in [&mcp_file, &world_file, &settings_file] {
        assert!(p.starts_with(&runtime), "{}", p.display());
    }
    assert_eq!(
        std::fs::read_to_string(world_file).unwrap(),
        "synthetic shared world"
    );
    assert_eq!(std::fs::read_to_string(path_log).unwrap(), child_path);
    assert!(!data.join("run").exists());
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mcp: serde_json::Value = serde_json::from_slice(&std::fs::read(mcp_file).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["crew"]["command"],
        helper.display().to_string()
    );
    let args = mcp["mcpServers"]["crew"]["args"].as_array().unwrap();
    let token_pos = args.iter().position(|a| a == "--token-file").unwrap();
    let token_file = PathBuf::from(args[token_pos + 1].as_str().unwrap());
    assert!(token_file.starts_with(&runtime));
    assert_eq!(
        std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(token_file).unwrap(),
        "synthetic-token"
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(settings_file).unwrap()).unwrap();
    assert!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--token-file")
    );
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
