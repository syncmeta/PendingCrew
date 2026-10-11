//! 起进程的 claude 执行器测试（控制者裁定 C8：起进程的测试放集成测试二进制，不进 crewd-server 的 lib 测试）。
//! shell 脚本顶替 claude：把 argv 写进文件、停在那里等被停掉；进程由驱动层 setsid，自成进程组。绝不起真 claude。
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::SessionEvent;
use crewd_server::orchestra::exec::{Executor, LaunchRequest};
use crewd_server::orchestra::exec_claude::{ClaudeExecutor, ClaudeExecutorConfig, ClaudeProgram};

fn executor(program: ClaudeProgram, data_dir: &Path) -> ClaudeExecutor {
    ClaudeExecutor::new(ClaudeExecutorConfig {
        program,
        data_dir: data_dir.to_path_buf(),
        helper: PathBuf::from("/nonexistent/crewd-helper"),
        registry: None,
        extra_env: BTreeMap::new(),
        projects_dir: None,
        runtime_dir: None,
        child_path: Some("/usr/bin:/bin".into()),
    })
}

fn request(session: &str, workdir: &Path) -> LaunchRequest {
    LaunchRequest {
        delivery_observer: None,
        session_id: session.into(),
        crew_id: "c1".into(),
        workdir: workdir.to_path_buf(),
        model: None,
        effort: None,
        system_prompt: Some("合成世界观".into()),
        initial_prompt: None,
        token: "tok-1".into(),
        ledger: Arc::new(MemoryLedger::default()),
    }
}

#[tokio::test]
async fn launches_the_given_program_with_world_model_and_comms() {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv.txt");
    let script = dir.path().join("claude.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexec sleep 30\n",
            argv.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let exec = executor(ClaudeProgram::Path(script), dir.path());
    let (handle, mut events) = exec.launch(request("s1", dir.path())).await.unwrap();
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::Spawned { .. })
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let text = loop {
        if let Ok(t) = std::fs::read_to_string(&argv)
            && t.contains("--permission-mode")
        {
            break t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "脚本 10s 内没写出 argv"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    for flag in [
        "--session-id",
        "--append-system-prompt-file",
        "--mcp-config",
    ] {
        assert!(text.lines().any(|l| l == flag), "缺 {flag}：{text}");
    }
    handle.stop().unwrap();
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = events.recv().await {
            if matches!(e, SessionEvent::Exited(_)) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(exited, "停掉之后 10s 内要见到 Exited");
}
