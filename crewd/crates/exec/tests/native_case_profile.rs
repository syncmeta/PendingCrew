//! Opt-in actual Codex native sandbox acceptance, deterministic loopback provider.
//! Run scripts/dev/run-native-case-profile.py. This is not real model/OAuth proof.
use std::{path::PathBuf, sync::Arc, time::Duration};

use crewd_exec::{
    codex::{
        driver::{CodexConfig, CodexSpec, CodexStart, start_codex},
        profile::NativePolicy,
    },
    launch::CommsSpec,
    resume::MemoryLedger,
    session::SessionEvent,
};

#[tokio::test]
#[ignore = "real Codex tool execution; use scripts/dev/run-native-case-profile.py"]
async fn restricted_profile_real_native_tools_and_exact_resume() {
    assert_eq!(
        std::env::var("PCW_NATIVE_CASE_PROBE").as_deref(),
        Ok("loopback")
    );
    let program = PathBuf::from(std::env::var_os("PCW_NATIVE_CASE_PROGRAM").expect("wrapper"));
    let project =
        PathBuf::from(std::env::var_os("PCW_NATIVE_CASE_PROJECT").expect("isolated project"));
    let evidence =
        PathBuf::from(std::env::var_os("PCW_NATIVE_CASE_EVIDENCE").expect("evidence directory"));
    let ledger = Arc::new(MemoryLedger::default());
    let mut original_thread = None;
    for resumed in [false, true] {
        let (handle, mut events) = start_codex(CodexStart {
            session_id: "native-case-profile-probe".into(),
            program: program.clone(),
            child_path: std::env::var("PATH").unwrap(),
            spec: CodexSpec {
                workdir: project.clone(),
                native_policy: NativePolicy::AdminCaseRestricted,
                strict_resume: true,
                comms: Some(CommsSpec {
                    helper: PathBuf::from(std::env::var_os("PCW_NATIVE_CASE_HELPER").expect("handshake helper")),
                    data_dir: evidence.join("data"),
                    session_id: "native-case-profile-probe".into(),
                    token: "synthetic-native-profile-handshake-only".into(),
                    hooks: false,
                    managed_hooks: false,
                }),
                initial_prompt: Some("Execute the isolated acceptance probe. Do not request expanded permissions.".into()),
                ..CodexSpec::default()
            },
            ledger: ledger.clone(),
        }, CodexConfig { runtime_dir: evidence.join("runtime"), ..CodexConfig::default() })
        .await.expect("restricted profile handshake must succeed before a probe turn");
        let mut items = Vec::new();
        let outcome = tokio::time::timeout(Duration::from_secs(80), async {
            let mut spawned = false;
            while let Some(event) = events.recv().await {
                match event {
                    SessionEvent::Spawned {
                        agent_session_id,
                        resumed: actual,
                        ..
                    } => {
                        assert_eq!(actual, resumed, "no fallback to a fresh conversation");
                        if let Some(original) = &original_thread {
                            assert_eq!(&agent_session_id, original);
                        } else {
                            original_thread = Some(agent_session_id);
                        }
                        spawned = true;
                    }
                    SessionEvent::Item { item, .. } => items.push(item),
                    SessionEvent::TurnEnded { .. } => return spawned,
                    SessionEvent::Exited(_) => return false,
                    _ => {}
                }
            }
            false
        })
        .await;
        // Always reap the real process before checking the result.
        let _ = handle.stop();
        let reaped = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(event) = events.recv().await {
                if matches!(event, SessionEvent::Exited(_)) {
                    return true;
                }
            }
            false
        })
        .await;
        std::fs::write(
            evidence.join(if resumed {
                "resume-items.json"
            } else {
                "fresh-items.json"
            }),
            serde_json::to_vec_pretty(&items).unwrap(),
        )
        .unwrap();
        assert_eq!(reaped.ok(), Some(true), "real Codex process must be reaped");
        assert_eq!(
            outcome.ok(),
            Some(true),
            "tool probe did not complete; not a sandbox pass"
        );
        assert!(
            items.iter().any(|i| i["type"] == "commandExecution"),
            "must observe real native command execution"
        );
        let changes: Vec<_> = items.iter().filter(|i| i["type"] == "fileChange").collect();
        // readOnly Codex may omit apply_patch entirely. In that case the
        // loopback provider's actual custom_tool_call_output must prove its
        // rejection; the Python runner checks that receipt (not model prose).
        assert!(
            changes
                .iter()
                .all(|i| matches!(i["status"].as_str(), Some("failed" | "declined"))),
            "patch must be denied, not just verbally declined: {changes:?}"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("sentinel.txt")).unwrap(),
            "unchanged isolated native profile sentinel\n"
        );
        assert!(!project.join("shell-write").exists());
        assert!(!project.join("escalated-write").exists());
    }
}
