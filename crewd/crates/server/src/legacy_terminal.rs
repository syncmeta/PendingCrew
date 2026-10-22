//! Existing Core-owned Claude PTY adapter for M14 manual mechanisms.
//! This neither certifies ACP/SDK automatic content nor replaces WorkBook.
use crate::{
    connection_scope::ConnectionScope, native_helpers::PrivateRun, public_commands::Actor,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_exec::{
    acp::{
        manual_fence::ManualFence,
        ownership::NativeBinding,
        wire::{TerminalFrame, TerminalKind},
    },
    driver::{ManualWritePermit, RawWriteOutcome, ResizeOutcome, SessionHandle},
    registry::{self, ParentProbe, ProbeResult, ProcessIdentity, ProcessRegistry},
};
use crewd_store::{Db, native_execution as n};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "original Core PTY binding or lease unavailable",
    )
}
fn same(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
#[derive(Clone)]
struct Lease {
    id: String,
    connection: ConnectionScope,
    grant: Option<String>,
    fence: ManualFence,
    deadline: Instant,
}
/// Retains the original raw-claim mutex through the caller's synchronous allow.
pub(crate) struct AutomaticHookPermit<'a> {
    _lease: tokio::sync::MutexGuard<'a, Option<Lease>>,
}
pub(crate) struct LegacyTerminal {
    pub generation: String,
    session: String,
    driver: SessionHandle,
    binding: NativeBinding,
    db: Db,
    private: PrivateRun,
    lease: tokio::sync::Mutex<Option<Lease>>,
    output: broadcast::Sender<TerminalFrame>,
    frames: Mutex<VecDeque<TerminalFrame>>,
}
impl LegacyTerminal {
    pub(crate) async fn adopt(
        session: String,
        driver: SessionHandle,
        db: Db,
        registry: &ProcessRegistry,
    ) -> io::Result<Arc<Self>> {
        if !driver.manual_pty() {
            return Err(refused());
        }
        let status = tokio::time::timeout(Duration::from_secs(2), driver.status())
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())?;
        let ParentProbe::Found { identity, parent } = registry::parent(status.pid as i32) else {
            return Err(refused());
        };
        if parent != std::process::id() as i32
            || identity.pgid != identity.pid
            || !registry
                .try_entries()?
                .iter()
                .any(|(key, id)| key == &session && same(id, &identity))
        {
            return Err(refused());
        }
        let mut source = driver.terminal_output().ok_or_else(refused)?;
        let cut = driver
            .terminal_cut()
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        // Observation is read-only: native registration/manual floor belongs
        // to the later authorized claim transaction, never inspect/watch.
        let runtime = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: session.clone(),
            generation: uuid::Uuid::new_v4().to_string(),
            epoch: cut.epoch,
            native_pid: identity.pid,
            native_start: identity.start_time,
            native_boot: identity.boot_id.clone(),
        };
        let (output, _) = broadcast::channel(64);
        let this = Arc::new(Self {
            generation: runtime.generation.clone(),
            session: session.clone(),
            driver,
            binding: NativeBinding {
                session,
                generation: runtime.generation,
                epoch: runtime.epoch,
                native: identity,
            },
            db,
            private: PrivateRun::new()?,
            lease: tokio::sync::Mutex::new(None),
            output,
            frames: Mutex::new(VecDeque::new()),
        });
        this.binding()?;
        let weak = Arc::downgrade(&this);
        tokio::spawn(async move {
            loop {
                let frame = match source.recv().await {
                    Ok(frame) => frame,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some(this) = weak.upgrade() else {
                            break;
                        };
                        let Ok(Some(cut)) = this.driver.terminal_cut().await else {
                            break;
                        };
                        TerminalFrame {
                            epoch: cut.epoch,
                            sequence: cut.sequence.max(1),
                            kind: TerminalKind::Resync,
                            bytes: Vec::new(),
                        }
                    }
                    Err(_) => break,
                };
                let Some(this) = weak.upgrade() else {
                    break;
                };
                if this.binding().is_err() || frame.epoch != this.binding.epoch {
                    break;
                }
                let Ok(mut ring) = this.frames.lock() else {
                    break;
                };
                if ring
                    .back()
                    .is_some_and(|last| last.sequence >= frame.sequence)
                {
                    continue;
                }
                ring.push_back(frame.clone());
                while ring.len() > 64
                    || ring.iter().map(|f| f.bytes.len()).sum::<usize>() > 1024 * 1024
                {
                    ring.pop_front();
                }
                let _ = this.output.send(frame);
            }
        });
        Ok(this)
    }
    pub(crate) fn binding(&self) -> io::Result<NativeBinding> {
        match registry::parent(self.binding.native.pid) {
            ParentProbe::Found { identity, parent }
                if same(&identity, &self.binding.native) && parent == std::process::id() as i32 =>
            {
                Ok(self.binding.clone())
            }
            _ => Err(refused()),
        }
    }
    pub(crate) fn original_gone(&self) -> bool {
        match registry::probe(self.binding.native.pid) {
            ProbeResult::Missing => true,
            ProbeResult::Found(ref actual) => !same(actual, &self.binding.native),
            ProbeResult::Unreadable(_) => false,
        }
    }
    pub(crate) fn is_live(&self) -> bool {
        self.binding().is_ok() && !self.driver.is_closed()
    }
    async fn projection(&self) -> io::Result<(n::Runtime, i64)> {
        self.binding()?;
        let session = self.session.clone();
        let result = self
            .db
            .call_result(move |store| -> crewd_store::Result<_> {
                let runtime = n::current_runtime(store.conn(), &session)?.ok_or_else(|| {
                    crewd_store::StoreError::Invalid("manual native projection absent".into())
                })?;
                let revision = store.conn().query_row(
                    "SELECT revision FROM native_execution_runtimes WHERE runtime_id=?1",
                    [&runtime.id],
                    |r| r.get::<_, i64>(0),
                )?;
                Ok((runtime, revision))
            })
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())?;
        if result.0.generation != self.generation
            || result.0.native_pid != self.binding.native.pid
            || result.0.native_start != self.binding.native.start_time
            || result.0.native_boot != self.binding.native.boot_id
        {
            return Err(refused());
        }
        Ok(result)
    }
    /// A historical manual floor is not permission to submit a new prompt.
    /// Only the actual current Human lease may use the native input path.
    #[cfg(test)]
    pub(crate) async fn test_set_lease_deadline(&self, deadline: Instant) {
        self.lease.lock().await.as_mut().unwrap().deadline = deadline;
    }
    #[cfg(test)]
    pub(crate) fn test_lease_locked(&self) -> bool {
        self.lease.try_lock().is_err()
    }
    pub(crate) async fn manual_hook_allowed(&self) -> bool {
        if self.binding().is_err() {
            return false;
        }
        let lease = self.lease.lock().await;
        if !lease
            .as_ref()
            .is_some_and(|l| l.connection.is_live() && Instant::now() < l.deadline)
        {
            return false;
        }
        let floor = self.manual_floor().await;
        floor
            && self.binding().is_ok()
            && lease
                .as_ref()
                .is_some_and(|l| l.connection.is_live() && Instant::now() < l.deadline)
    }
    pub(crate) async fn automatic_hook_authority(
        &self,
        seal: Arc<n::ManagedRuntime>,
    ) -> Option<AutomaticHookPermit<'_>> {
        let lease = self.lease.lock().await;
        if self.binding().is_err() {
            return None;
        }
        let valid = self
            .db
            .call_result(move |store| store.with_tx(|tx| n::verify_managed_authority(tx, &seal)))
            .await;
        if !matches!(valid, Ok(Ok(()))) || self.binding().is_err() {
            return None;
        }
        Some(AutomaticHookPermit { _lease: lease })
    }
    pub(crate) async fn manual_floor(&self) -> bool {
        let session = self.session.clone();
        let generation = self.generation.clone();
        self.db.call_result(move|store|->crewd_store::Result<bool>{Ok(store.conn().query_row("SELECT EXISTS(SELECT 1 FROM native_execution_runtimes WHERE session_id=?1 AND generation=?2 AND revoked=0 AND manual=1)",rusqlite::params![session,generation],|r|r.get(0))?)}).await.ok().and_then(Result::ok).unwrap_or(false)
    }
    pub(crate) async fn snapshot(&self) -> io::Result<Value> {
        self.binding()?;
        let cut = tokio::time::timeout(Duration::from_secs(2), self.driver.terminal_cut())
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        self.binding()?;
        if cut.epoch != self.binding.epoch {
            return Err(refused());
        }
        if cut.screen.bytes.len() > 512 * 1024 {
            return Ok(
                json!({"epoch":cut.epoch,"sequence":cut.sequence,"truncated":true,"snapshotUnavailable":true}),
            );
        }
        Ok(
            json!({"epoch":cut.epoch,"sequence":cut.sequence,"cols":cut.screen.cols,"rows":cut.screen.lines,"bytesBase64":STANDARD.encode(&cut.screen.bytes),"truncated":false}),
        )
    }
    pub(crate) async fn claim(
        &self,
        actor: &Actor,
        owner: &Arc<crate::AppState>,
        previous: Option<&str>,
    ) -> io::Result<Value> {
        let mut lease = self.lease.lock().await;
        let expired_previous = lease
            .as_ref()
            .filter(|old| !old.connection.is_live() || Instant::now() >= old.deadline)
            .map(|old| old.id.clone());
        if expired_previous.is_some() {
            let old = lease.as_ref().expect("expired lease selected");
            // A lost connection or elapsed clock does not by itself prove the
            // old writer is fenced. Wait for its physical chunk barrier before
            // a new explicit Human claim can adopt another lease.
            old.fence.revoke(Instant::now() + Duration::from_secs(2))?;
            old.connection.remove(&old.id)?;
            *lease = None;
        }
        if let Some(old) = lease.as_ref() {
            if previous != Some(old.id.as_str()) {
                return Err(refused());
            }
        } else if previous.is_some() && previous != expired_previous.as_deref() {
            return Err(refused());
        }
        let binding = self.binding()?;
        let generation = self.generation.clone();
        let session = self.session.clone();
        let tx_actor = actor.clone();
        let owner = owner.clone();
        // Final inner-operation ACL and kernel-linked floor are one FIFO Tx.
        // A read observer cannot create this admission or promote history.
        self.db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    tx_actor
                        .authorize_tx(&owner, tx, "session.terminal.claim")
                        .map_err(|_| {
                            crewd_store::StoreError::Invalid(
                                "manual claim actor permission revoked".into(),
                            )
                        })?;
                    let runtime = if let Some(prior) = n::current_runtime(tx, &session)? {
                        if prior.generation == generation
                            && prior.native_pid == binding.native.pid
                            && prior.native_start == binding.native.start_time
                            && prior.native_boot == binding.native.boot_id
                        {
                            prior
                        } else {
                            let gone = match registry::probe(prior.native_pid) {
                                ProbeResult::Missing => true,
                                ProbeResult::Found(ref actual) => {
                                    actual.start_time != prior.native_start
                                        || actual.boot_id != prior.native_boot
                                }
                                ProbeResult::Unreadable(_) => false,
                            };
                            if !gone {
                                return Err(crewd_store::StoreError::Invalid(
                                    "previous native still live or unknown".into(),
                                ));
                            }
                            n::retire_verified_native(tx, &prior.id, &crewd_store::now_ts())?;
                            let runtime = n::Runtime {
                                id: uuid::Uuid::new_v4().to_string(),
                                session,
                                generation,
                                epoch: binding.epoch,
                                native_pid: binding.native.pid,
                                native_start: binding.native.start_time,
                                native_boot: binding.native.boot_id,
                            };
                            n::register(tx, &runtime)?;
                            runtime
                        }
                    } else {
                        let runtime = n::Runtime {
                            id: uuid::Uuid::new_v4().to_string(),
                            session,
                            generation,
                            epoch: binding.epoch,
                            native_pid: binding.native.pid,
                            native_start: binding.native.start_time,
                            native_boot: binding.native.boot_id,
                        };
                        n::register(tx, &runtime)?;
                        runtime
                    };
                    let revision = tx.query_row(
                        "SELECT revision FROM native_execution_runtimes WHERE runtime_id=?1",
                        [&runtime.id],
                        |r| r.get::<_, i64>(0),
                    )?;
                    n::revoke_before_raw(tx, &runtime.id, revision, &crewd_store::now_ts())
                })
            })
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())?;
        if let Some(old) = lease.as_ref() {
            old.connection.remove(&old.id)?;
        }
        *lease = None;
        if !tokio::time::timeout(Duration::from_secs(2), self.driver.hold_automatic())
            .await
            .map_err(|_| refused())?
            .map_err(|_| refused())?
        {
            return Err(refused());
        }
        self.binding()?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.private.root.join(format!("lease-{id}"));
        let fence = ManualFence::create(&path, *uuid::Uuid::new_v4().as_bytes())?;
        // The Core-owned driver clone is the adopted writer for this SAME File;
        // no helper adoption or native application consumption is claimed.
        fence.seal_after_helper_adopted()?;
        let connection = actor.connection_scope().clone();
        connection.enable(id.clone(), fence.clone())?;
        *lease = Some(Lease {
            id: id.clone(),
            connection,
            grant: actor.remote_grant(),
            fence,
            deadline: Instant::now() + Duration::from_secs(30),
        });
        Ok(
            json!({"leaseId":id,"durationMs":30000,"manualRevocationPersisted":true,"automaticInputPaused":true,"physicalInputWritten":false,"backend":"existing_core_owned_claude_pty"}),
        )
    }
    pub(crate) async fn release(&self, actor: &Actor, id: &str) -> io::Result<Value> {
        let mut lease = self.lease.lock().await;
        let current = lease
            .as_ref()
            .filter(|l| l.id == id && l.connection.id == actor.connection_scope().id)
            .ok_or_else(refused)?;
        current.connection.remove(id)?;
        *lease = None;
        Ok(json!({"released":true,"automaticInputResumed":false}))
    }
    fn permit(&self, lease: &Lease, actor: &Actor, id: &str) -> io::Result<ManualWritePermit> {
        self.binding()?;
        if lease.id != id
            || lease.connection.id != actor.connection_scope().id
            || !lease.connection.is_live()
            || Instant::now() >= lease.deadline
        {
            return Err(refused());
        }
        Ok(ManualWritePermit::for_native_fenced(
            lease.deadline,
            self.binding.native.clone(),
            lease.fence.clone(),
            lease.connection.liveness(),
        ))
    }
    pub(crate) async fn write(&self, actor: &Actor, id: &str, bytes: Vec<u8>) -> io::Result<Value> {
        if bytes.is_empty() || bytes.len() > 65536 {
            return Err(refused());
        }
        let lease = self.lease.lock().await;
        let current = lease.as_ref().ok_or_else(refused)?;
        let permit = self.permit(current, actor, id)?;
        self.projection().await?;
        let cancellation = permit.clone();
        let write = self.driver.raw_confirmed(bytes, permit);
        tokio::pin!(write);
        // Reserve half of the existing two-second whole budget for the driver
        // to observe revocation and report its exact accepted prefix. Dropping
        // the only receiver at the first deadline would discard that evidence.
        let outcome = match tokio::time::timeout(Duration::from_secs(1), &mut write).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => return Err(refused()),
            Err(_) => {
                cancellation.revoke();
                match tokio::time::timeout(Duration::from_secs(1), &mut write).await {
                    Ok(Ok(outcome)) => outcome,
                    _ => return Err(refused()),
                }
            }
        };
        Ok(match outcome {
            RawWriteOutcome::Written { bytes } => {
                json!({"writtenBytes":bytes,"unknown":false,"applicationConsumed":"unconfirmed"})
            }
            RawWriteOutcome::Unknown { written_bytes } => {
                json!({"writtenBytes":written_bytes,"unknown":true})
            }
            _ => json!({"writtenBytes":0,"unknown":true}),
        })
    }
    pub(crate) async fn resize(
        &self,
        actor: &Actor,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> io::Result<Value> {
        let lease = self.lease.lock().await;
        let current = lease.as_ref().ok_or_else(refused)?;
        let permit = self.permit(current, actor, id)?;
        let cancellation = permit.clone();
        let outcome = match tokio::time::timeout(
            Duration::from_secs(2),
            self.driver.resize_controlled(cols, rows, permit),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            _ => {
                cancellation.revoke();
                return Err(refused());
            }
        };
        Ok(
            json!({"applied":matches!(outcome,ResizeOutcome::Applied),"unknown":!matches!(outcome,ResizeOutcome::Applied|ResizeOutcome::InvalidSize)}),
        )
    }
    pub(crate) async fn revoke_grant(&self, grant: Option<&str>) -> io::Result<()> {
        let mut lease = self.lease.lock().await;
        if let Some(current) = lease
            .as_ref()
            .filter(|l| grant.is_none() || l.grant.as_deref() == grant)
        {
            current.connection.remove(&current.id)?;
            *lease = None;
        }
        Ok(())
    }
    pub(crate) fn terminal(&self) -> broadcast::Receiver<TerminalFrame> {
        self.output.subscribe()
    }
    pub(crate) fn oldest_sequence(&self, epoch: u64) -> Option<u64> {
        self.frames
            .lock()
            .ok()?
            .iter()
            .find(|f| f.epoch == epoch)
            .map(|f| f.sequence)
    }
    pub(crate) fn after(&self, epoch: u64, after: u64) -> io::Result<Vec<TerminalFrame>> {
        let frames = self.frames.lock().map_err(|_| refused())?;
        let selected = frames
            .iter()
            .filter(|f| f.epoch == epoch && f.sequence > after)
            .cloned()
            .collect::<Vec<_>>();
        if selected
            .first()
            .is_some_and(|f| f.sequence != after.saturating_add(1))
        {
            return Err(refused());
        }
        Ok(selected)
    }
}
