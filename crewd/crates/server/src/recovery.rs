//! Closed offline recovery diagnostics. No ordinary RPC or Human admission.
use crewd_exec::registry::recovery::{self, RecoveryReport};
use crewd_exec::registry::{self, ProcessRegistry};
use crewd_store::Store;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn validate_registry_file(data: &Path) -> anyhow::Result<()> {
    let path = data.join(crate::PROCESS_REGISTRY_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(info) => {
            // SAFETY: uid is a read-only kernel query.
            let owner = unsafe { libc::geteuid() };
            if !info.is_file()
                || info.nlink() != 1
                || info.uid() != owner
                || info.mode() & 0o022 != 0
            {
                anyhow::bail!(
                    "registry source ownership/alias cannot be verified; retained without signals or retirement"
                );
            }
            if info.len() > 16 * 1024 * 1024 {
                anyhow::bail!(
                    "registry diagnostic exceeds bounded input; retained without signals or retirement"
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn recover_locked(
    data: &Path,
    signal_owned: bool,
) -> anyhow::Result<(Arc<ProcessRegistry>, Vec<RecoveryReport>)> {
    // Held DataDirLock is required by every caller. Version/WAL/identity and
    // private-copy integrity checks precede registry retirement or signalling.
    let guard = Store::recovery_input_guard(data)?;
    validate_registry_file(data)?;
    let registry = Arc::new(ProcessRegistry::load(
        &data.join(crate::PROCESS_REGISTRY_FILE),
    )?);
    let reports = recovery::recover_bounded(&registry, signal_owned, &|| {
        guard.recheck().map_err(std::io::Error::other)
    })?;
    guard.recheck()?;
    Ok((registry, reports))
}

pub fn require_settled(registry: &ProcessRegistry) -> anyhow::Result<()> {
    let pending = registry.try_entries()?;
    if !pending.is_empty() {
        anyhow::bail!(
            "{} unresolved orphan obligations retained; ordinary serve/executor admission refused. Use recovery inspect; an independent Human OS action is required for unprovable ownership, then recovery reconcile. Never delete the registry to bypass this boundary",
            pending.len()
        );
    }
    Ok(())
}

/// Only lock-file IO is permitted by this diagnostic/reconciliation command.
/// The database connection is never opened on original files.
pub fn maintenance(data: &Path, reconcile: bool) -> anyhow::Result<serde_json::Value> {
    let data = std::fs::canonicalize(data)?;
    let _lock = crate::lock_data_dir(&data)?;
    if reconcile {
        let (registry, reports) = recover_locked(&data, false)?;
        let unresolved = registry.try_entries()?;
        return Ok(serde_json::json!({
            "operation":"recovery.reconcile", "lockFileIO":true,
            "signalsPermitted":false,"ordinaryAdmission":false,
            "resolved":unresolved.is_empty(),"reports":reports,"remaining":unresolved,
        }));
    }
    validate_registry_file(&data)?;
    let registry = ProcessRegistry::load(&data.join(crate::PROCESS_REGISTRY_FILE))?;
    let original = registry.try_entries()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut observations = Vec::new();
    for (key, birth) in &original {
        let current = match registry::probe(birth.pid) {
            registry::ProbeResult::Found(value) => {
                serde_json::json!({"state":"found","identity":value})
            }
            registry::ProbeResult::Missing => serde_json::json!({"state":"missing"}),
            registry::ProbeResult::Unreadable(errno) => {
                serde_json::json!({"state":"unreadable","errno":errno})
            }
        };
        let group = match recovery::observe_group_until(birth.pgid, deadline) {
            recovery::GroupObservation::Absent => serde_json::json!({"state":"absent"}),
            recovery::GroupObservation::Present(members) => {
                serde_json::json!({"state":"present","members":members})
            }
            recovery::GroupObservation::Unknown(errno) => {
                serde_json::json!({"state":"unknown","errno":errno})
            }
        };
        observations
            .push(serde_json::json!({"key":key,"original":birth,"current":current,"group":group}));
    }
    Ok(serde_json::json!({
        "operation":"recovery.inspect","lockFileIO":true,
        "registryModified":false,"signalsPermitted":false,"ordinaryAdmission":false,
        "readOnlyDiagnosticNotAuthority":true,"registrySnapshot":original,
        "observations":observations,
    }))
}
