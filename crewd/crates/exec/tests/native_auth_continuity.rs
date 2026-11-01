//! Actual Codex external-auth protocol; synthetic broker is not source-manager proof.
use base64::Engine;
use crewd_exec::{
    codex::{
        driver::{CodexConfig, CodexSpec, CodexStart, start_codex},
        profile::{AuthFailure, ExternalChatgptAuth, NativeAuthSource, NativePolicy},
    },
    launch::CommsSpec,
    resume::MemoryLedger,
    session::SessionEvent,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

fn credential(account: &str, version: usize) -> ExternalChatgptAuth {
    let claims = json!({"sub":"synthetic-user","email":"probe@example.invalid","exp":4102444800_i64,
        "nonce":version,"https://api.openai.com/auth":{"chatgpt_account_id":account,
        "chatgpt_user_id":"synthetic-user","chatgpt_plan_type":"plus"}});
    let token = format!(
        "e30.{}.synthetic",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap())
    );
    ExternalChatgptAuth::from_native(
        &json!({"authMethod":"chatgpt","requiresOpenaiAuth":true,"authToken":token}),
        1,
    )
    .unwrap()
}
struct Broker {
    mode: String,
    calls: Mutex<Vec<(bool, bool)>>,
}
impl NativeAuthSource for Broker {
    fn access(
        &self,
        refresh: bool,
        previous: Option<ExternalChatgptAuth>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ExternalChatgptAuth, AuthFailure>> + Send + '_>,
    > {
        let generation = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((refresh, previous.is_some()));
            calls.len()
        };
        Box::pin(async move {
            if !refresh {
                return Ok(credential("synthetic-account", 1));
            }
            match self.mode.as_str() {
                "success" => Ok(credential("synthetic-account", generation)),
                "reject" => Err(AuthFailure::Unavailable),
                "mismatch" => Ok(credential("different-account", 2)),
                _ => panic!("unsupported probe mode"),
            }
        })
    }
}

#[tokio::test]
#[ignore = "actual native external-auth test; scripts/dev/run-native-auth-continuity.py"]
async fn actual_native_401_uses_typed_broker_without_persistent_credentials() {
    assert_eq!(std::env::var("PCW_AUTH_PROBE").as_deref(), Ok("loopback"));
    let mode = std::env::var("PCW_AUTH_MODE").unwrap();
    let base = PathBuf::from(std::env::var_os("PCW_AUTH_EVIDENCE").unwrap());
    let broker = Arc::new(Broker {
        mode: mode.clone(),
        calls: Mutex::new(vec![]),
    });
    let (h, mut events) = start_codex(
        CodexStart {
            session_id: "native-auth-continuity-probe".into(),
            program: PathBuf::from(std::env::var_os("PCW_AUTH_PROGRAM").unwrap()),
            child_path: std::env::var("PATH").unwrap(),
            spec: CodexSpec {
                native_policy: NativePolicy::AdminCaseRestricted,
                workdir: base.join("project"),
                initial_prompt: Some(
                    "Reply with the isolated authentication probe completion marker.".into(),
                ),
                comms: Some(CommsSpec {
                    helper: PathBuf::from(std::env::var_os("PCW_AUTH_HELPER").unwrap()),
                    data_dir: base.join("data"),
                    session_id: "native-auth-continuity-probe".into(),
                    token: "synthetic-crew-handshake-only".into(),
                    hooks: false,
                    managed_hooks: false,
                }),
                ..Default::default()
            },
            ledger: Arc::new(MemoryLedger::default()),
        },
        CodexConfig {
            runtime_dir: base.join("runtime"),
            native_auth_source: Some(broker.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("external auth login and policy handshake");
    let mut completed = false;
    let mut tool_effects = 0;
    let mut exited_before_cleanup = false;
    let mut auth_required = false;
    let observed = tokio::time::timeout(Duration::from_secs(35), async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::Item { item, .. } => {
                    if matches!(
                        item["type"].as_str(),
                        Some("commandExecution" | "fileChange" | "mcpToolCall")
                    ) {
                        tool_effects += 1;
                    }
                }
                SessionEvent::TurnEnded {
                    last_agent_message, ..
                } => {
                    completed = last_agent_message.contains("AUTH_PROBE_COMPLETED");
                    break;
                }
                SessionEvent::Health(Some(health))
                    if health.kind == crewd_exec::health::HealthKind::AuthRequired =>
                {
                    auth_required = true;
                }
                SessionEvent::Exited(_) => {
                    exited_before_cleanup = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await;
    let _ = h.stop();
    let reaped = if exited_before_cleanup {
        Ok(true)
    } else {
        tokio::time::timeout(Duration::from_secs(12), async {
            while let Some(event) = events.recv().await {
                if matches!(event, SessionEvent::Exited(_)) {
                    return true;
                }
            }
            false
        })
        .await
    };
    let calls = broker.calls.lock().unwrap().clone();
    std::fs::write(
        base.join("broker-evidence.json"),
        serde_json::to_vec_pretty(&json!({
            "mode":mode,"calls":calls,"completed":completed,"toolEffects":tool_effects,
            "boundedCompletion":observed.is_ok(),"reaped":matches!(reaped,Ok(true)),
            "authRequired":auth_required,"exitedBeforeCleanup":exited_before_cleanup
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        matches!(reaped, Ok(true)),
        "real native process must be reaped"
    );
    assert!(observed.is_ok(), "auth refresh cannot hang indefinitely");
    assert!(
        calls.contains(&(true, true)),
        "must receive actual native 401 refresh request"
    );
    assert_eq!(completed, mode == "success");
    if mode != "success" {
        assert!(
            auth_required && exited_before_cleanup,
            "rejected refresh must really terminate native before test cleanup"
        );
    }
    assert_eq!(tool_effects, 0);
    assert!(
        !base
            .join("runtime/codex-case-home-native-auth-continuity-probe/auth.json")
            .exists(),
        "case must never persist credentials"
    );
}
