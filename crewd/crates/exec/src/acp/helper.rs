//! Provider helper reuses the native driver, but delegates session-ID writes to
//! Core. It never opens a work DB or ProcessRegistry. ACP/native bytes stay apart.
use super::wire::{self, Control, MAX_CONTROL};
use crate::{
    driver::{self, ClaudeStart, DriverConfig, SessionHandle, StartError},
    registry::{self, ProbeResult, ProcessIdentity},
    resume::{AgentIdLedger, check_agent_session_id},
    session::SessionEvent,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

struct LedgerChannel {
    stream: BufReader<UnixStream>,
    next: u64,
    closed: bool,
}
/// One private parent/Core channel; a timed-out or malformed response closes the
/// channel. Retrying a session-ID record after an unknown result is not allowed.
pub struct CoreLedger {
    session: String,
    generation: String,
    nonce: [u8; 16],
    core: ProcessIdentity,
    timeout: Duration,
    channel: Mutex<LedgerChannel>,
    raw_scope: Arc<()>,
}
impl std::fmt::Debug for CoreLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreLedger")
            .field("session", &self.session)
            .field("generation", &self.generation)
            .field("nonce", &"<private>")
            .finish_non_exhaustive()
    }
}
fn denied(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}
fn same_birth(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
#[cfg(target_os = "macos")]
fn core_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: live owned socket and exactly sized writable pid_t buffer.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            0x002,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of_val(&pid) || pid <= 1 {
        return Err(denied("Core peer unavailable"));
    }
    Ok(pid)
}
#[cfg(target_os = "linux")]
fn core_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut value = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: exact ucred buffer; read only after full successful kernel write.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            value.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(denied("Core credentials unavailable"));
    }
    // SAFETY: complete successful kernel write above.
    let value = unsafe { value.assume_init() };
    if value.pid <= 1 {
        return Err(denied("Core peer unavailable"));
    }
    Ok(value.pid)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn core_pid(_: &UnixStream) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Core peer unavailable",
    ))
}
impl CoreLedger {
    /// A genuinely child-created connection is required when Core must verify
    /// the helper socket PID. On Linux an inherited socketpair retains its
    /// creator credentials and cannot establish the reverse helper identity.
    pub async fn connect(
        path: &std::path::Path,
        session: String,
        generation: String,
        nonce: [u8; 16],
        core: ProcessIdentity,
        timeout: Duration,
    ) -> io::Result<Self> {
        if !path.is_absolute() || timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(denied("invalid private Core connect scope"));
        }
        let stream = tokio::time::timeout(timeout, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "private Core connect deadline")
            })??;
        let stream = stream.into_std()?;
        stream.set_nonblocking(false)?;
        Self::new(stream, session, generation, nonce, core, timeout)
    }
    pub fn verified_core_birth(&self) -> io::Result<ProcessIdentity> {
        let channel = self
            .channel
            .lock()
            .map_err(|_| denied("Core channel poisoned"))?;
        if channel.closed {
            return Err(denied("Core channel closed"));
        }
        self.verify(channel.stream.get_ref())?;
        Ok(self.core.clone())
    }
    /// A separately child-connected channel can carry typed Core control after
    /// the same genuine kernel-parent checks. It is never an inherited pair.
    pub fn into_verified_parent_socket(self) -> io::Result<UnixStream> {
        self.verified_core_birth()?;
        let channel = self
            .channel
            .into_inner()
            .map_err(|_| denied("Core channel poisoned"))?;
        if channel.closed {
            return Err(denied("Core channel closed"));
        }
        // Constructing this ledger already checked the current direct parent.
        // Recheck it here without accepting JSON process ownership.
        if !matches!(registry::probe(self.core.pid),ProbeResult::Found(ref actual) if same_birth(actual,&self.core))
        {
            return Err(denied("Core birth changed"));
        }
        Ok(channel.stream.into_inner())
    }
    pub fn open_manual_fence(
        &self,
        path: &std::path::Path,
        nonce: [u8; 16],
    ) -> io::Result<super::manual_fence::ManualFence> {
        let channel = self
            .channel
            .lock()
            .map_err(|_| denied("Core channel poisoned"))?;
        if channel.closed {
            return Err(denied("Core channel closed"));
        }
        self.verify(channel.stream.get_ref())?;
        super::manual_fence::ManualFence::adopt(path, nonce)
    }
    pub fn new(
        stream: UnixStream,
        session: String,
        generation: String,
        nonce: [u8; 16],
        core: ProcessIdentity,
        timeout: Duration,
    ) -> io::Result<Self> {
        check_agent_session_id(&session)?;
        let generation_id =
            uuid::Uuid::parse_str(&generation).map_err(|_| denied("invalid Core generation"))?;
        if generation_id.is_nil()
            || generation_id.to_string() != generation
            || nonce == [0; 16]
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(denied("invalid Core channel binding"));
        }
        // Restore CLOEXEC on the inherited Core channel before native spawn.
        // SAFETY: fcntl operates on this live, exclusively owned socket.
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFD) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: preserves flags and prevents this private channel leaking.
        if unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let value = Self {
            raw_scope: Arc::new(()),
            session,
            generation,
            nonce,
            core,
            timeout,
            channel: Mutex::new(LedgerChannel {
                stream: BufReader::new(stream),
                next: 1,
                closed: false,
            }),
        };
        {
            let channel = value
                .channel
                .lock()
                .map_err(|_| io::Error::other("Core channel poisoned"))?;
            value.verify(channel.stream.get_ref())?;
        }
        Ok(value)
    }
    fn verify(&self, stream: &UnixStream) -> io::Result<()> {
        if core_pid(stream)? != self.core.pid {
            return Err(denied("foreign Core channel peer"));
        }
        match registry::probe(self.core.pid) {
            ProbeResult::Found(actual) if same_birth(&actual, &self.core) => {}
            _ => return Err(denied("Core birth changed or unavailable")),
        }
        match registry::parent(std::process::id() as i32) {
            registry::ParentProbe::Found { parent, identity }
                if parent == self.core.pid && identity.pid > 1 && identity.pgid == identity.pid =>
            {
                Ok(())
            }
            _ => Err(denied("helper is not owned by Core")),
        }
    }
    fn call(&self, method: &str, extra: Value) -> io::Result<Value> {
        let mut channel = self
            .channel
            .lock()
            .map_err(|_| io::Error::other("Core channel poisoned"))?;
        if channel.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Core channel closed",
            ));
        }
        let deadline = Instant::now() + self.timeout;
        let remaining = || {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Core whole-frame deadline"))
        };
        let result = (|| {
            self.verify(channel.stream.get_ref())?;
            let id = channel.next.to_string();
            channel.next = channel
                .next
                .checked_add(1)
                .ok_or_else(|| denied("Core request sequence exhausted"))?;
            let mut params =
                json!({"sessionId":self.session,"generation":self.generation,"nonce":self.nonce});
            let extra = extra
                .as_object()
                .ok_or_else(|| denied("invalid Core private params"))?;
            for (key, value) in extra {
                if params.get(key).is_some() {
                    return Err(denied("private Core binding cannot be overridden"));
                }
                params[key] = value.clone();
            }
            let payload = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
            let mut bytes = serde_json::to_vec(&payload)
                .map_err(|_| io::Error::other("Core ledger serialization"))?;
            if bytes.len() >= MAX_CONTROL {
                return Err(io::Error::other("Core ledger frame limit"));
            }
            bytes.push(b'\n');
            let mut written = 0;
            while written < bytes.len() {
                channel
                    .stream
                    .get_ref()
                    .set_write_timeout(Some(remaining()?))?;
                match channel.stream.get_mut().write(&bytes[written..]) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Core ledger write stalled",
                        ));
                    }
                    Ok(count) => written += count,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            let mut response = Vec::new();
            loop {
                channel
                    .stream
                    .get_ref()
                    .set_read_timeout(Some(remaining()?))?;
                let available = channel.stream.fill_buf()?;
                if available.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Core ledger response missing",
                    ));
                }
                let end = available.iter().position(|b| *b == b'\n');
                let count = end.map_or(available.len(), |n| n + 1);
                if response.len().saturating_add(count) > MAX_CONTROL {
                    return Err(io::Error::other("Core ledger response limit"));
                }
                response.extend_from_slice(&available[..count]);
                channel.stream.consume(count);
                if end.is_some() {
                    break;
                }
            }
            remaining()?;
            self.verify(channel.stream.get_ref())?;
            remaining()?;
            match wire::control(&response) {
                Ok(Control::Response {
                    id: reply,
                    result: Ok(result),
                }) if reply == Value::String(id) => Ok(result),
                _ => Err(denied("Core ledger response refused or mismatched")),
            }
        })();
        if result.is_err() {
            channel.closed = true;
        }
        result
    }
    fn typed<T>(
        &self,
        method: &str,
        params: Value,
        parse: impl FnOnce(Value) -> io::Result<T>,
    ) -> io::Result<T> {
        let result = self.call(method, params).and_then(parse);
        if result.is_err()
            && let Ok(mut channel) = self.channel.lock()
        {
            channel.closed = true;
        }
        result
    }
    /// Unknown/negative ACK emits no bytes. The returned permit contains the
    /// exact bytes so it cannot later authorize a different raw write.
    pub fn revoke_for_raw(
        &self,
        runtime: &str,
        revision: i64,
        bytes: Vec<u8>,
    ) -> io::Result<CommittedRaw> {
        if !uuid::Uuid::parse_str(runtime).is_ok_and(|id| !id.is_nil() && id.to_string() == runtime)
            || revision < 0
            || bytes.is_empty()
            || bytes.len() > 65536
        {
            return Err(denied("invalid native raw request"));
        }
        let request = uuid::Uuid::new_v4().to_string();
        self.typed("native/raw.revoke",json!({"runtimeId":runtime,"expectedRevision":revision,"requestId":request,"rawBytes":bytes}),|result| {
            if result.get("committed")!=Some(&Value::Bool(true))||result.get("manual")!=Some(&Value::Bool(true))||result.get("requestId").and_then(Value::as_str)!=Some(request.as_str())||result.get("runtimeId").and_then(Value::as_str)!=Some(runtime){return Err(denied("native raw has no matching commit ACK"));}
            Ok(CommittedRaw { bytes, raw_scope: self.raw_scope.clone() })
        })
    }
}
impl AgentIdLedger for CoreLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        self.typed("native/ledger.get", json!({}), |result| {
            let value = result
                .get("agentSessionId")
                .ok_or_else(|| denied("Core session result missing"))?;
            if value.is_null() {
                return Ok(None);
            }
            let id = value
                .as_str()
                .ok_or_else(|| denied("Core session result invalid"))?;
            check_agent_session_id(id)?;
            Ok(Some(id.to_owned()))
        })
    }
    fn record(&self, id: &str) -> io::Result<()> {
        check_agent_session_id(id)?;
        self.typed(
            "native/ledger.record",
            json!({"agentSessionId":id}),
            |result| {
                if result.get("committed") != Some(&Value::Bool(true))
                    || result.get("agentSessionId").and_then(Value::as_str) != Some(id)
                {
                    return Err(denied("Core session record has no commit ACK"));
                }
                Ok(())
            },
        )
    }
}
/// The helper launches genuinely interactive native PTY only. Body submission
/// belongs to the verified Mod/Core path; neither the brief nor a send() falls
/// back to keyboard delivery. New/load decisions use the Core proxy ledger.
pub async fn launch_native(
    mut start: ClaudeStart,
    config: DriverConfig,
    ledger: Arc<CoreLedger>,
) -> Result<(NativePtySession, NativePtyEvents), StartError> {
    if config.registry.is_some()
        || config.native_birth_gate.is_none()
        || config.terminal_observer.is_none()
        || config.claude_projects_dir.is_some()
        || start.spec.initial_prompt.is_some()
    {
        return Err(StartError::Ledger(denied(
            "helper requires Core custody, separate terminal observer, no transcript probe and no keyboard prompt",
        )));
    }
    let raw_scope = ledger.raw_scope.clone();
    start.ledger = ledger;
    let (handle, events) = driver::start_claude(start, config).await?;
    Ok((
        NativePtySession { handle, raw_scope },
        NativePtyEvents { source: events },
    ))
}

/// PTY observations do not contain a successful automatic delivery or an idle
/// turn edge. Those require the authenticated native content/turn receipts.
pub enum NativePtyObservation {
    Spawned {
        pid: u32,
        agent_session_id: String,
        resumed: bool,
    },
    ResumeRefused {
        id: String,
        said: String,
    },
    Decision(crewd_term::DecisionEvent),
    Health(Option<crate::health::Health>),
    Notice(String),
    InputDraft(Option<crate::session::HeldDraft>),
    Exited(crate::session::ExitInfo),
}
impl std::fmt::Debug for NativePtyObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawned { pid, resumed, .. } => f
                .debug_struct("Spawned")
                .field("pid", pid)
                .field("resumed", resumed)
                .finish_non_exhaustive(),
            Self::ResumeRefused { .. } => f.write_str("ResumeRefused(<private>)"),
            Self::Decision(_) => f.write_str("Decision(<private>)"),
            Self::Health(_) => f.write_str("Health(<private>)"),
            Self::Notice(_) => f.write_str("Notice(<private>)"),
            Self::InputDraft(_) => f.write_str("InputDraft(<private>)"),
            Self::Exited(value) => f.debug_tuple("Exited").field(value).finish(),
        }
    }
}
fn observation(event: SessionEvent) -> Option<NativePtyObservation> {
    match event {
        SessionEvent::Spawned {
            pid,
            agent_session_id,
            resumed,
        } => Some(NativePtyObservation::Spawned {
            pid,
            agent_session_id,
            resumed,
        }),
        SessionEvent::ResumeRefused { id, said } => {
            Some(NativePtyObservation::ResumeRefused { id, said })
        }
        SessionEvent::Decision(value) => Some(NativePtyObservation::Decision(value)),
        SessionEvent::Health(value) => Some(NativePtyObservation::Health(value)),
        SessionEvent::Notice(value) => Some(NativePtyObservation::Notice(value)),
        SessionEvent::InputDraft(value) => Some(NativePtyObservation::InputDraft(value)),
        SessionEvent::Exited(value) => Some(NativePtyObservation::Exited(value)),
        // Quiet TUI, keyboard-delivery and foreign runner events can never
        // finish or accept an automatic native Mod prompt in this transport.
        SessionEvent::Working(_)
        | SessionEvent::Delivery { .. }
        | SessionEvent::Item { .. }
        | SessionEvent::TurnEnded { .. }
        | SessionEvent::NativeTurnOutcome { .. }
        | SessionEvent::ManagedResponseFailed { .. }
        | SessionEvent::Profile(_)
        | SessionEvent::Usage(_)
        | SessionEvent::CodexObservation { .. } => None,
    }
}
pub struct NativePtyEvents {
    source: mpsc::UnboundedReceiver<SessionEvent>,
}
impl NativePtyEvents {
    pub async fn recv(&mut self) -> Option<NativePtyObservation> {
        while let Some(event) = self.source.recv().await {
            if let Some(value) = observation(event) {
                return Some(value);
            }
        }
        None
    }
}

/// No public send()/keyboard-brief path exists on this wrapper. Automatic
/// submissions use the authenticated Mod path after Core content admission.
pub struct NativePtySession {
    handle: SessionHandle,
    raw_scope: Arc<()>,
}
pub struct CommittedRaw {
    bytes: Vec<u8>,
    raw_scope: Arc<()>,
}
impl std::fmt::Debug for CommittedRaw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommittedRaw")
            .field("bytes", &"<private>")
            .finish()
    }
}
impl NativePtySession {
    /// Only a sealed, durably acknowledged manual-revocation input can reach
    /// the physical writer. The lease gate remains independent of queue ACK.
    /// Written bytes mean kernel acceptance, never native application consumption.
    pub fn raw_confirmed(
        &self,
        committed: CommittedRaw,
        lease: driver::ManualWritePermit,
    ) -> impl std::future::Future<Output = Result<driver::RawWriteOutcome, driver::Closed>>
    + Send
    + 'static {
        let bound = lease.is_native_bound()
            && lease.is_connection_bound()
            && Arc::ptr_eq(&self.raw_scope, &committed.raw_scope);
        let input = if bound { committed.bytes } else { Vec::new() };
        // Unbound native input cannot enter the writer. This invalid result is
        // independent of whether its earlier revocation transaction committed.
        self.handle.raw_confirmed(input, lease)
    }
    pub async fn resize(
        &self,
        columns: u16,
        rows: u16,
        lease: driver::ManualWritePermit,
    ) -> Result<driver::ResizeOutcome, driver::Closed> {
        if !lease.is_native_bound() || !lease.is_connection_bound() {
            return Ok(driver::ResizeOutcome::Unknown);
        }
        self.handle.resize_controlled(columns, rows, lease).await
    }
    pub async fn owned_native_birth(&self) -> io::Result<ProcessIdentity> {
        let status = self
            .handle
            .status()
            .await
            .map_err(|_| denied("native driver closed"))?;
        match registry::parent(status.pid as i32) {
            registry::ParentProbe::Found { identity, parent }
                if parent == std::process::id() as i32 =>
            {
                Ok(identity)
            }
            _ => Err(denied("native is not this helper's current child")),
        }
    }
    pub async fn terminal_cut(
        &self,
    ) -> Result<Option<super::terminal::TerminalCut>, driver::Closed> {
        self.handle.terminal_cut().await
    }
    pub async fn snapshot(&self) -> Result<Option<crewd_term::Snapshot>, driver::Closed> {
        self.handle.snapshot().await
    }
    /// The caller separately awaits actual Exited and Core custody settlement.
    pub fn stop_requested(&self) -> Result<(), driver::Closed> {
        self.handle.stop()
    }
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::CommandExt,
        },
        process::{Child, Command},
    };
    #[test]
    fn tui_quiet_and_keyboard_receipts_cannot_confirm_native_mod_turns() {
        assert!(observation(SessionEvent::Working(false)).is_none());
        assert!(observation(SessionEvent::Working(true)).is_none());
        assert!(
            observation(SessionEvent::TurnEnded {
                turn_id: "not-native".into(),
                last_agent_message: "opaque".into()
            })
            .is_none()
        );
        assert!(matches!(
            observation(SessionEvent::Spawned {
                pid: 42,
                agent_session_id: "session".into(),
                resumed: false
            }),
            Some(NativePtyObservation::Spawned { pid: 42, .. })
        ));
    }
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    /// Explicitly launched by the parent control below; no CLI/provider involved.
    #[test]
    #[ignore = "private helper subprocess entered only by owned Core fixture"]
    fn private_ledger_child() {
        let fd: i32 = std::env::var("PCW_OWNED_CORE_FD").unwrap().parse().unwrap();
        // SAFETY: parent handed this single inherited descriptor to this child.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        let core: ProcessIdentity =
            serde_json::from_str(&std::env::var("PCW_TEST_CORE_IDENTITY").unwrap()).unwrap();
        let nonce: [u8; 16] =
            serde_json::from_str(&std::env::var("PCW_TEST_NONCE").unwrap()).unwrap();
        let ledger = CoreLedger::new(
            stream,
            "owned-helper".into(),
            std::env::var("PCW_TEST_GENERATION").unwrap(),
            nonce,
            core,
            if std::env::var_os("PCW_TEST_DRIBBLE").is_some() {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(5)
            },
        )
        .unwrap();
        // SAFETY: the constructor still owns the live FD.
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        if std::env::var_os("PCW_TEST_DRIBBLE").is_some() {
            let began = std::time::Instant::now();
            assert!(
                ledger.recorded().is_err(),
                "dribbling full frame must time out"
            );
            assert!(
                began.elapsed() < Duration::from_millis(600),
                "whole frame budget exceeded: {:?}",
                began.elapsed()
            );
            assert_eq!(
                ledger.recorded().unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            return;
        }
        assert_eq!(ledger.recorded().unwrap(), None);
        ledger.record("native-session-1").unwrap();
        let permit = ledger
            .revoke_for_raw(
                &std::env::var("PCW_TEST_RUNTIME").unwrap(),
                0,
                b"raw input".to_vec(),
            )
            .unwrap();
        assert_eq!(permit.bytes, b"raw input");
        // A valid frame with a negative commit result poisons this channel,
        // even though its JSON-RPC request ID is correct.
        assert!(ledger.record("native-session-2").is_err());
        assert_eq!(
            ledger.recorded().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[test]
    fn actual_owned_helper_rejects_dribbling_response_within_whole_frame_budget() {
        let (mut parent, inherited) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        parent
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let ProbeResult::Found(core) = registry::probe(std::process::id() as i32) else {
            panic!("Core birth unavailable")
        };
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let generation = uuid::Uuid::new_v4().to_string();
        let runtime = uuid::Uuid::new_v4().to_string();
        let fd = inherited.as_raw_fd();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "acp::helper::tests::private_ledger_child",
            ])
            .env_clear()
            .env("PCW_OWNED_CORE_FD", fd.to_string())
            .env(
                "PCW_TEST_CORE_IDENTITY",
                serde_json::to_string(&core).unwrap(),
            )
            .env("PCW_TEST_NONCE", serde_json::to_string(&nonce).unwrap())
            .env("PCW_TEST_GENERATION", &generation)
            .env("PCW_TEST_RUNTIME", &runtime)
            .env("PCW_TEST_DRIBBLE", "1");
        // SAFETY: only async-signal-safe setsid/fcntl in the owned child's pre-exec context.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = OwnedChild(command.spawn().unwrap());
        drop(inherited);
        let mut reader = BufReader::new(parent.try_clone().unwrap());
        let mut frame = String::new();
        reader.read_line(&mut frame).unwrap();
        let request: Value = serde_json::from_str(&frame).unwrap();
        let response = serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":request["id"],"result":{"agentSessionId":null}}),
        )
        .unwrap();
        for byte in response.iter().chain([b'\n'].iter()) {
            if parent.write_all(&[*byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        assert!(child.0.wait().unwrap().success());
    }
    #[test]
    fn actual_owned_helper_requires_matching_commit_ack_and_keeps_core_fd_private() {
        let (mut parent, inherited) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        parent
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let ProbeResult::Found(core) = registry::probe(std::process::id() as i32) else {
            panic!("Core birth unavailable")
        };
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let generation = uuid::Uuid::new_v4().to_string();
        let runtime = uuid::Uuid::new_v4().to_string();
        let fd = inherited.as_raw_fd();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "acp::helper::tests::private_ledger_child",
            ])
            .env_clear()
            .env("PCW_OWNED_CORE_FD", fd.to_string())
            .env(
                "PCW_TEST_CORE_IDENTITY",
                serde_json::to_string(&core).unwrap(),
            )
            .env("PCW_TEST_NONCE", serde_json::to_string(&nonce).unwrap())
            .env("PCW_TEST_GENERATION", &generation)
            .env("PCW_TEST_RUNTIME", &runtime);
        // SAFETY: only async-signal-safe setsid/fcntl in the owned child's pre-exec context.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = OwnedChild(command.spawn().unwrap());
        drop(inherited);
        let mut reader = BufReader::new(parent.try_clone().unwrap());
        for (index, method) in [
            "native/ledger.get",
            "native/ledger.record",
            "native/raw.revoke",
            "native/ledger.record",
        ]
        .iter()
        .enumerate()
        {
            let mut frame = String::new();
            reader.read_line(&mut frame).unwrap();
            let request: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(request["id"], (index + 1).to_string());
            assert_eq!(request["method"], *method);
            assert_eq!(request["params"]["generation"], generation);
            assert_eq!(request["params"]["nonce"], json!(nonce));
            let result = match index {
                0 => json!({"agentSessionId":null}),
                1 => json!({"committed":true,"agentSessionId":"native-session-1"}),
                2 => {
                    assert_eq!(request["params"]["rawBytes"], json!(b"raw input".to_vec()));
                    json!({"committed":true,"manual":true,"runtimeId":runtime,"requestId":request["params"]["requestId"]})
                }
                _ => json!({"committed":false,"agentSessionId":"native-session-2"}),
            };
            let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
            parent
                .write_all(&serde_json::to_vec(&response).unwrap())
                .unwrap();
            parent.write_all(b"\n").unwrap();
        }
        assert!(child.0.wait().unwrap().success());
    }
}

#[cfg(test)]
mod public_manual_scope_tests {
    use super::*;
    #[tokio::test]
    async fn foreign_native_raw_commit_scope_cannot_queue_any_physical_input() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = SessionHandle::new(tx);
        let session = NativePtySession {
            handle,
            raw_scope: Arc::new(()),
        };
        let committed = CommittedRaw {
            bytes: b"foreign raw".to_vec(),
            raw_scope: Arc::new(()),
        };
        let native = match registry::probe(std::process::id() as i32) {
            ProbeResult::Found(native) => native,
            other => panic!("kernel self: {other:?}"),
        };
        let permit = driver::ManualWritePermit::for_native_connection(
            Instant::now() + Duration::from_secs(1),
            native,
            Arc::new(Mutex::new(true)),
        );
        assert_eq!(
            session.raw_confirmed(committed, permit).await.unwrap(),
            driver::RawWriteOutcome::InvalidInput
        );
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
    #[tokio::test]
    async fn missing_actual_connection_scope_cannot_queue_native_manual_bytes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scope = Arc::new(());
        let session = NativePtySession {
            handle: SessionHandle::new(tx),
            raw_scope: scope.clone(),
        };
        let committed = CommittedRaw {
            bytes: b"unbound raw".to_vec(),
            raw_scope: scope,
        };
        let native = match registry::probe(std::process::id() as i32) {
            ProbeResult::Found(native) => native,
            other => panic!("kernel self: {other:?}"),
        };
        let permit =
            driver::ManualWritePermit::for_native(Instant::now() + Duration::from_secs(1), native);
        assert_eq!(
            session.raw_confirmed(committed, permit).await.unwrap(),
            driver::RawWriteOutcome::InvalidInput
        );
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}
