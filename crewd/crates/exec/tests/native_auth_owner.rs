//! Opt-in real existing app-server auth owner; synthetic private credentials only.
use crewd_exec::{
    codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex},
    resume::MemoryLedger,
    session::SessionEvent,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "scripts/dev/run-native-auth-owner.py; actual local CLI, loopback identity provider"]
async fn actual_native_auth_owner_serializes_same_account_refresh_without_copy() {
    assert_eq!(
        std::env::var("PCW_AUTH_OWNER_PROBE").as_deref(),
        Ok("synthetic-loopback")
    );
    let base = PathBuf::from(std::env::var_os("PCW_AUTH_OWNER_BASE").unwrap());
    let spec = CodexSpec {
        workdir: base.join("project"),
        additional_env: BTreeMap::from([
            (
                "CODEX_HOME".into(),
                base.join("source").to_string_lossy().into_owned(),
            ),
            (
                "CODEX_REFRESH_TOKEN_URL_OVERRIDE".into(),
                std::env::var("PCW_AUTH_OWNER_REFRESH_URL").unwrap(),
            ),
        ]),
        ..Default::default()
    };
    let (handle, mut events) = start_codex(
        CodexStart {
            session_id: "synthetic-auth-owner".into(),
            program: PathBuf::from(std::env::var_os("PCW_AUTH_OWNER_PROGRAM").unwrap()),
            child_path: std::env::var("PATH").unwrap(),
            spec,
            ledger: Arc::new(MemoryLedger::default()),
        },
        CodexConfig {
            runtime_dir: base.join("runtime"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let pid = handle.status().await.unwrap().pid;
    let first = tokio::time::timeout(
        Duration::from_secs(8),
        handle.native_auth_access(false, None),
    )
    .await
    .unwrap()
    .unwrap();
    let (left, right) = tokio::join!(
        handle.native_auth_access(true, Some(first.clone())),
        handle.native_auth_access(true, Some(first.clone()))
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert!(first.same_identity(&left) && first.changed_access(&left));
    assert_eq!(
        left, right,
        "concurrent cases must share one refreshed owner value"
    );
    assert_eq!(handle.status().await.unwrap().pid, pid);
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
    .unwrap();
    assert!(exited);
    assert!(handle.native_auth_access(false, None).await.is_err());
    std::fs::write(base.join("owner-receipt.json"),serde_json::to_vec_pretty(&serde_json::json!({"sameOwnerPid":pid,"sameAccount":true,"changedAccess":true,"concurrentSameValue":true,"exitedBeforeCleanup":exited,"readAfterExitRejected":true,"boundary":"actual ordinary native app-server owning synthetic file auth; no case or real account"})).unwrap()).unwrap();
}
