//! Actual kernel birth + owned filesystem failure. Isolated integration binary.
use crewd_exec::registry::{ProbeResult, ProcessRegistry, probe};
use std::{io, process::Command};
#[test]
fn s1_real_kernel_birth_is_restrictive_even_when_actual_registry_persist_fails() {
    // Exact owned live child, not a manufactured registry identity or provider.
    struct Owned(std::process::Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    let registry = std::sync::Arc::new(ProcessRegistry::load(&path).unwrap());
    let enrollment = registry.begin_enrollment().unwrap();
    let mut owned = Owned(
        Command::new("/bin/sleep")
            .arg("20")
            .env_clear()
            .env("HOME", dir.path())
            .env("SHELL", "/usr/bin/false")
            .spawn()
            .unwrap(),
    );
    let pid = owned.0.id() as i32;
    let birth = probe(pid);
    // True rename failure after serializing/fsyncing: target is an owned directory.
    std::fs::create_dir(&path).unwrap();
    let record = match &birth {
        ProbeResult::Found(identity) => registry.record("S1_FAILED_PERSIST", identity.clone()),
        other => Err(io::Error::other(format!(
            "actual birth unavailable: {other:?}"
        ))),
    };
    let retained_before_forget = registry.authority_snapshot();
    let forget = registry.forget("S1_FAILED_PERSIST");
    let retained_after_forget = registry.authority_snapshot();
    let signalled = owned.0.kill();
    let terminal = owned.0.wait();
    drop(owned);
    drop(enrollment);
    let retired = registry.authority_snapshot();
    // All exact-child cleanup controls precede the first functional oracle.
    assert!(
        signalled.is_ok() && terminal.is_ok(),
        "owned OS terminal receipt"
    );
    let ProbeResult::Found(identity) = birth else {
        panic!("actual live kernel birth required")
    };
    assert!(
        record.is_err(),
        "actual filesystem persist failure required"
    );
    assert!(
        forget.is_err(),
        "actual directory still prevents orphan-file persist"
    );
    for snapshot in [
        retained_before_forget.unwrap(),
        retained_after_forget.unwrap(),
        retired.unwrap(),
    ] {
        assert!(
            snapshot
                .spawned
                .iter()
                .any(|(session, stored)| session == "S1_FAILED_PERSIST" && stored == &identity),
            "trusted live birth must be restrictive before failed persist and survive forget"
        );
    }
}
