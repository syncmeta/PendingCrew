//! The daemon's sole factory/cleanup owner for native helper generations.
//! This is production transport/custody, not an SDK capability declaration.
use crate::native_receipts::CoreHelperLedger;
use crewd_exec::{
    acp::{
        birth::{BirthReport, REPORT_LEN},
        bridge::HelperPeer,
        custody::Custody,
        helper_boot::Boot,
        transport::{ControlReader, TerminalReader, write_control},
        wire::{Control, TerminalFrame},
    },
    registry::{self, ProbeResult, ProcessIdentity, ProcessRegistry},
};
use crewd_store::{Db, native_execution as native};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, PermissionsExt},
            net::UnixStream,
        },
    },
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream as Socket},
    sync::{broadcast, oneshot},
    task::JoinHandle,
};
fn error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "native factory custody unavailable",
    )
}
fn same(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
fn current(identity: &ProcessIdentity) -> bool {
    matches!(registry::probe(identity.pid),ProbeResult::Found(ref actual) if same(actual,identity))
}
pub(crate) struct PrivateRun {
    pub(crate) root: PathBuf,
    channels: Vec<PathBuf>,
}
impl PrivateRun {
    pub(crate) fn new() -> io::Result<Self> {
        // Short private owner leaf: Unix socket sockaddr length is part of the
        // contract, independent of a caller’s arbitrarily long TMPDIR.
        #[cfg(target_os = "macos")]
        let parent = std::fs::canonicalize("/private/tmp")?;
        #[cfg(not(target_os = "macos"))]
        let parent = std::fs::canonicalize("/tmp")?;
        let root = parent.join(format!("pcw-native-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let channels = ["l", "c", "t"]
            .into_iter()
            .map(|name| root.join(name))
            .collect();
        Ok(Self { root, channels })
    }
    fn listeners(&self) -> io::Result<Vec<UnixListener>> {
        use std::os::unix::ffi::OsStrExt;
        let mut result = Vec::new();
        for path in &self.channels {
            if path.as_os_str().as_bytes().len() > 103 {
                return Err(error());
            }
            let listener = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            result.push(listener);
        }
        Ok(result)
    }
}
impl Drop for PrivateRun {
    fn drop(&mut self) {
        // Only our known sockets and now-empty directory. Never recursive: an
        // unknown native/plugin/config/log file must remain as evidence.
        for path in &self.channels {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&self.root);
    }
}
/// Explicit Core launch input. Never parsed from an arbitrary public RPC.
/// An executable must be validated by its configured adapter before this call.
pub struct Launch {
    pub session: String,
    pub program: PathBuf,
    pub child_path: String,
    pub workdir: PathBuf,
    pub runtime_dir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub additional_env: BTreeMap<String, String>,
}
pub struct NativeFactory {
    pub(crate) db: Db,
    pub(crate) registry: Arc<ProcessRegistry>,
    helper: PathBuf,
    helper_identity: Option<crate::build_info::ExeIdentity>,
    runtimes: Mutex<BTreeMap<String, Arc<Runtime>>>,
    launching: Mutex<BTreeSet<String>>,
    pub(crate) legacy: Mutex<BTreeMap<String, Arc<crate::legacy_terminal::LegacyTerminal>>>,
    pub(crate) legacy_adoption: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}
impl NativeFactory {
    pub fn new(db: Db, registry: Arc<ProcessRegistry>, helper: PathBuf) -> Self {
        Self {
            db,
            registry,
            helper_identity: crate::build_info::ExeIdentity::capture(&helper).ok(),
            helper,
            runtimes: Mutex::new(BTreeMap::new()),
            launching: Mutex::new(BTreeSet::new()),
            legacy: Mutex::new(BTreeMap::new()),
            legacy_adoption: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) fn get(&self, session: &str) -> Option<Arc<Runtime>> {
        self.runtimes.lock().ok()?.get(session).cloned()
    }
    pub(crate) async fn revoke_session(&self, session: &str) -> io::Result<()> {
        if let Some(runtime) = self.get(session) {
            runtime.revoke_grant(None).await?;
        }
        let legacy = self
            .legacy
            .lock()
            .map_err(|_| error())?
            .get(session)
            .cloned();
        if let Some(legacy) = legacy {
            legacy.revoke_grant(None).await?;
        }
        Ok(())
    }
    pub(crate) async fn revoke_grant(&self, grant: &str) -> io::Result<()> {
        let runtimes = self
            .runtimes
            .lock()
            .map_err(|_| error())?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for runtime in runtimes {
            runtime.revoke_grant(Some(grant)).await?;
        }
        let legacy = self
            .legacy
            .lock()
            .map_err(|_| error())?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for runtime in legacy {
            runtime.revoke_grant(Some(grant)).await?;
        }
        Ok(())
    }
    pub async fn launch(
        &self,
        launch: Launch,
    ) -> io::Result<(Arc<Runtime>, broadcast::Receiver<Value>)> {
        let _launch = LaunchSlot::new(&self.launching, launch.session.clone())?;
        if self
            .get(&launch.session)
            .is_some_and(|runtime| runtime.is_live())
        {
            return Err(error());
        }
        if !crate::build_info::exe().is_some_and(crate::build_info::ExeIdentity::still_on_disk) {
            return Err(error());
        }
        if !self
            .helper_identity
            .as_ref()
            .is_some_and(crate::build_info::ExeIdentity::still_on_disk)
        {
            return Err(error());
        }
        let ProbeResult::Found(core) = registry::probe(std::process::id() as i32) else {
            return Err(error());
        };
        let private = PrivateRun::new()?;
        let listeners = private.listeners()?;
        let (birth_parent, birth_child) = UnixStream::pair()?;
        // Parent endpoint is never evidence of helper peer identity; ONLY the
        // three actual child-connect sockets below establish HelperPeer.
        let fd = birth_child.as_raw_fd();
        let generation = uuid::Uuid::new_v4();
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let boot = Boot {
            session: launch.session.clone(),
            generation: generation.to_string(),
            nonce,
            core,
            ledger_socket: private.channels[0].clone(),
            control_socket: private.channels[1].clone(),
            terminal_socket: private.channels[2].clone(),
            birth_fd: fd,
            program: launch.program,
            child_path: launch.child_path,
            workdir: launch.workdir,
            runtime_dir: launch.runtime_dir,
            model: launch.model,
            effort: launch.effort,
            system_prompt: launch.system_prompt,
            additional_env: launch.additional_env,
        };
        use std::os::unix::fs::OpenOptionsExt;
        let stderr = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(private.root.join("helper.log"))?;
        let mut command = tokio::process::Command::new(&self.helper);
        command
            .arg("native-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe syscalls between fork and exec. The
        // inherited private birth socket is unique, and helper restores CLOEXEC
        // before spawning a native. No prompt or secret is placed in argv.
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
        let mut child = command.spawn()?;
        drop(birth_child);
        let pid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .ok_or_else(error)?;
        let custody = match Custody::enroll_helper(
            self.registry.clone(),
            launch.session.clone(),
            generation,
            nonce,
            pid,
        ) {
            Ok(c) => Arc::new(Mutex::new(c)),
            Err(e) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(e);
            }
        };
        let mut owned = LaunchOwner {
            custody: custody.clone(),
            armed: true,
        };
        let started = async {
            let input = child.stdin.as_mut().ok_or_else(error)?;
            write_control(
                input,
                &json!({"jsonrpc":"2.0","id":"boot","method":"native/helper.boot","params":boot}),
            )
            .await?;
            input.shutdown().await?;
            drop(child.stdin.take());
            let mut accepted = Vec::new();
            for listener in listeners {
                let (socket, _) = listener.accept().await?;
                let standard = socket.into_std()?;
                {
                    let held = custody.lock().map_err(|_| error())?;
                    HelperPeer::authenticate(&standard, &held)?;
                }
                accepted.push(standard);
            }
            let terminal = accepted.pop().ok_or_else(error)?;
            let control = accepted.pop().ok_or_else(error)?;
            let ledger = accepted.pop().ok_or_else(error)?;
            let route = Arc::new(tokio::sync::Mutex::new(CoreHelperLedger::authenticate(
                self.db.clone(),
                custody.clone(),
                ledger.try_clone()?,
                launch.session.clone(),
                generation.to_string(),
                nonce,
            )?));
            let ledger_task = spawn_ledger(ledger, route.clone())?;
            let birth = spawn_birth(birth_parent, custody.clone(), route.clone())?;
            let runtime = Runtime::new(
                launch.session.clone(),
                generation.to_string(),
                custody.clone(),
                control,
                terminal,
                private,
                self.db.clone(),
                ledger_task,
                birth,
            )?;
            Ok::<_, io::Error>(runtime)
        };
        let (runtime, events) = tokio::time::timeout(Duration::from_secs(8), started)
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "native helper admission unknown")
            })??;
        self.runtimes
            .lock()
            .map_err(|_| error())?
            .insert(launch.session, runtime.clone());
        owned.armed = false;
        let weak = Arc::downgrade(&runtime);
        tokio::spawn(async move {
            let status = child.wait().await;
            if let Some(runtime) = weak.upgrade() {
                runtime.cleanup_after_helper_exit(status.is_ok()).await;
            } else {
                cleanup_custody(&custody);
            }
        });
        Ok((runtime, events))
    }
}
struct LaunchSlot<'a> {
    set: &'a Mutex<BTreeSet<String>>,
    session: String,
}
impl<'a> LaunchSlot<'a> {
    fn new(set: &'a Mutex<BTreeSet<String>>, session: String) -> io::Result<Self> {
        if !set.lock().map_err(|_| error())?.insert(session.clone()) {
            return Err(error());
        }
        Ok(Self { set, session })
    }
}
impl Drop for LaunchSlot<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.session);
        }
    }
}
struct LaunchOwner {
    custody: Arc<Mutex<Custody>>,
    armed: bool,
}
impl Drop for LaunchOwner {
    fn drop(&mut self) {
        if self.armed {
            cleanup_custody(&self.custody);
        }
    }
}
fn cleanup_custody(custody: &Arc<Mutex<Custody>>) {
    if let Ok(mut held) = custody.lock() {
        let identities = held
            .native_identities()
            .map(|(_, id)| id.clone())
            .chain(std::iter::once(held.helper_identity().clone()))
            .collect::<Vec<_>>();
        held.close_admission();
        for identity in identities {
            let now = registry::probe(identity.pid);
            let _ = registry::apply(&registry::decide(&identity, &now), &identity);
        }
        // Live/unknown obligations are never forgotten by a cleanup attempt.
    }
}
fn spawn_ledger(
    socket: UnixStream,
    route: Arc<tokio::sync::Mutex<CoreHelperLedger>>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    socket.set_nonblocking(true)?;
    let (read, mut write) = Socket::from_std(socket)?.into_split();
    Ok(tokio::spawn(async move {
        let mut reader = ControlReader::new(read);
        loop {
            let Some(Control::Request { id, method, params }) = reader.read().await? else {
                return Ok(());
            };
            let frame = serde_json::to_vec(
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .map_err(|_| error())?;
            let result = route.lock().await.call_frame(&frame).await?;
            tokio::time::timeout(
                Duration::from_secs(2),
                write_control(
                    &mut write,
                    &json!({"jsonrpc":"2.0","id":id,"result":result}),
                ),
            )
            .await
            .map_err(|_| error())??;
        }
    }))
}
fn spawn_birth(
    socket: UnixStream,
    custody: Arc<Mutex<Custody>>,
    route: Arc<tokio::sync::Mutex<CoreHelperLedger>>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    socket.set_nonblocking(true)?;
    let mut socket = Socket::from_std(socket)?;
    Ok(tokio::spawn(async move {
        loop {
            let mut bytes = [0u8; REPORT_LEN];
            socket.read_exact(&mut bytes).await?;
            let report = BirthReport::decode(&bytes)?;
            let enrolled = custody
                .lock()
                .map_err(|_| error())?
                .enroll_native(&report)?;
            // ProcessRegistry registration AND SQLite runtime registration complete
            // before native exec may occur. No copied/helper JSON birth is trusted.
            route
                .lock()
                .await
                .register_native_birth(enrolled.epoch)
                .await?;
            custody
                .lock()
                .map_err(|_| error())?
                .current_native_binding(enrolled.epoch)?;
            tokio::time::timeout(
                Duration::from_secs(2),
                socket.write_all(enrolled.acknowledgement()),
            )
            .await
            .map_err(|_| error())??;
        }
    }))
}
#[derive(Clone)]
struct ManualLease {
    id: String,
    connection: crate::connection_scope::ConnectionScope,
    grant: Option<String>,
    fence: crewd_exec::acp::manual_fence::ManualFence,
    deadline: Instant,
    native: ProcessIdentity,
}
type Pending = oneshot::Sender<io::Result<Value>>;
pub struct Runtime {
    pub session: String,
    pub generation: String,
    custody: Arc<Mutex<Custody>>,
    private: PrivateRun,
    control_peer: HelperPeer,
    control_peer_socket: UnixStream,
    db: Db,
    writer: tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>,
    pending: Mutex<BTreeMap<String, Pending>>,
    healthy: AtomicBool,
    terminal: broadcast::Sender<TerminalFrame>,
    frames: Mutex<VecDeque<TerminalFrame>>,
    lease: tokio::sync::Mutex<Option<ManualLease>>,
    events: broadcast::Sender<Value>,
    tasks: Mutex<Vec<JoinHandle<io::Result<()>>>>,
}
impl Runtime {
    #[allow(clippy::too_many_arguments)]
    fn new(
        session: String,
        generation: String,
        custody: Arc<Mutex<Custody>>,
        control: UnixStream,
        terminal: UnixStream,
        private: PrivateRun,
        db: Db,
        ledger: JoinHandle<io::Result<()>>,
        birth: JoinHandle<io::Result<()>>,
    ) -> io::Result<(Arc<Self>, broadcast::Receiver<Value>)> {
        control.set_nonblocking(true)?;
        terminal.set_nonblocking(true)?;
        let control_peer_socket = control.try_clone()?;
        let control_peer = {
            let held = custody.lock().map_err(|_| error())?;
            HelperPeer::authenticate(&control_peer_socket, &held)?
        };
        let (read, write) = Socket::from_std(control)?.into_split();
        let terminal_socket = Socket::from_std(terminal)?;
        let (events, initial_events) = broadcast::channel(64);
        let (output, _) = broadcast::channel(64);
        let runtime = Arc::new(Self {
            session,
            generation,
            custody,
            private,
            control_peer,
            control_peer_socket,
            db,
            writer: tokio::sync::Mutex::new(write),
            pending: Mutex::new(BTreeMap::new()),
            healthy: AtomicBool::new(true),
            terminal: output,
            frames: Mutex::new(VecDeque::new()),
            lease: tokio::sync::Mutex::new(None),
            events,
            tasks: Mutex::new(vec![ledger, birth]),
        });
        let weak = Arc::downgrade(&runtime);
        let control_task = tokio::spawn(async move {
            let mut reader = ControlReader::new(read);
            let result = async {
                loop {
                    let Some(frame) = reader.read().await? else {
                        return Err(error());
                    };
                    let Some(runtime) = weak.upgrade() else {
                        return Ok(());
                    };
                    let held = runtime.custody.lock().map_err(|_| error())?;
                    // Canonical helper is rechecked on EVERY control frame.
                    runtime
                        .control_peer
                        .verify(&runtime.control_peer_socket, &held)?;
                    let helper = held.helper_identity();
                    if !current(helper) {
                        return Err(error());
                    }
                    drop(held);
                    match frame {
                        Control::Response { id, result } => {
                            let id = id.as_str().ok_or_else(error)?;
                            let reply = runtime
                                .pending
                                .lock()
                                .map_err(|_| error())?
                                .remove(id)
                                .ok_or_else(error)?;
                            let result = result.map_err(|_| error());
                            let _ = reply.send(result);
                        }
                        Control::Notification { method, params } => {
                            match method.as_str() {
                                "native/physical.spawned" => {
                                    let pid = params["pid"]
                                        .as_i64()
                                        .and_then(|p| i32::try_from(p).ok())
                                        .ok_or_else(error)?;
                                    let binding = runtime.binding()?;
                                    if binding.native.pid != pid {
                                        return Err(error());
                                    }
                                    if !params["agentSessionId"]
                                        .as_str()
                                        .is_some_and(crewd_exec::resume::valid_agent_session_id)
                                        || !params["resumed"].is_boolean()
                                    {
                                        return Err(error());
                                    }
                                }
                                "native/physical.exited" => {
                                    if !params["value"].is_i64()
                                        || !["code", "signal"]
                                            .contains(&params["kind"].as_str().unwrap_or(""))
                                        || !params["stoppedByUser"].is_boolean()
                                    {
                                        return Err(error());
                                    }
                                    let held = runtime.custody.lock().map_err(|_| error())?;
                                    // A typed notice alone cannot clear a physical obligation.
                                    if held.native_identities().any(
                                        |(_, id)| match registry::probe(id.pid) {
                                            ProbeResult::Missing => false,
                                            ProbeResult::Found(ref now) => same(now, id),
                                            ProbeResult::Unreadable(_) => true,
                                        },
                                    ) {
                                        return Err(error());
                                    }
                                }
                                _ => return Err(error()),
                            }
                            let _ = runtime
                                .events
                                .send(json!({"method":method,"params":params}));
                        }
                        _ => return Err(error()),
                    }
                }
            }
            .await;
            if result.is_err() {
                if let Some(runtime) = weak.upgrade() {
                    runtime.healthy.store(false, Ordering::Release);
                    cleanup_custody(&runtime.custody);
                }
            }
            result
        });
        let weak = Arc::downgrade(&runtime);
        let terminal_task = tokio::spawn(async move {
            let mut reader = TerminalReader::new(terminal_socket);
            let result = async {
                loop {
                    let Some(frame) = reader.read().await? else {
                        return Err(error());
                    };
                    let Some(runtime) = weak.upgrade() else {
                        return Ok(());
                    };
                    let binding = runtime.binding()?;
                    if frame.epoch != binding.epoch {
                        return Err(error());
                    }
                    let mut ring = runtime.frames.lock().map_err(|_| error())?;
                    if let Some(last) = ring.back() {
                        if frame.epoch == last.epoch && frame.sequence <= last.sequence {
                            if last.kind == crewd_exec::acp::wire::TerminalKind::Resync {
                                continue;
                            }
                            return Err(error());
                        }
                    }
                    ring.push_back(frame.clone());
                    while ring.len() > 64
                        || ring.iter().map(|f| f.bytes.len()).sum::<usize>() > 1024 * 1024
                    {
                        ring.pop_front();
                    }
                    let _ = runtime.terminal.send(frame);
                }
            }
            .await;
            // Observation loss is explicit and never completion authority. It
            // closes this watch; it cannot stall or synthesize native exit.
            if let Some(runtime) = weak.upgrade() {
                let _=runtime.events.send(json!({"method":"native/observation.closed","params":{"unknown":result.is_err()}}));
            }
            result
        });
        runtime
            .tasks
            .lock()
            .map_err(|_| error())?
            .extend([control_task, terminal_task]);
        Ok((runtime, initial_events))
    }
    pub(crate) fn binding(&self) -> io::Result<crewd_exec::acp::ownership::NativeBinding> {
        let held = self.custody.lock().map_err(|_| error())?;
        let epoch = held
            .native_identities()
            .map(|(epoch, _)| *epoch)
            .max()
            .ok_or_else(error)?;
        held.current_native_binding(epoch)
    }
    async fn projection(&self) -> io::Result<(native::Runtime, i64)> {
        let binding = self.binding()?;
        let session = self.session.clone();
        let current = self
            .db
            .call_result(
                move |store| -> crewd_store::Result<(native::Runtime, i64)> {
                    let runtime =
                        native::current_runtime(store.conn(), &session)?.ok_or_else(|| {
                            crewd_store::StoreError::Invalid(
                                "native current projection absent".into(),
                            )
                        })?;
                    let revision = store.conn().query_row(
                        "SELECT revision FROM native_execution_runtimes WHERE runtime_id=?1",
                        [&runtime.id],
                        |r| r.get::<_, i64>(0),
                    )?;
                    Ok((runtime, revision))
                },
            )
            .await
            .map_err(|_| error())?
            .map_err(|_| error())?;
        if current.0.generation != binding.generation
            || current.0.epoch != binding.epoch
            || current.0.native_pid != binding.native.pid
            || current.0.native_start != binding.native.start_time
            || current.0.native_boot != binding.native.boot_id
        {
            return Err(error());
        }
        Ok(current)
    }
    pub(crate) async fn manual_floor(&self) -> bool {
        let session = self.session.clone();
        let generation = self.generation.clone();
        self.db.call_result(move|store|->crewd_store::Result<bool>{Ok(store.conn().query_row("SELECT EXISTS(SELECT 1 FROM native_execution_runtimes WHERE session_id=?1 AND generation=?2 AND revoked=0 AND manual=1)",rusqlite::params![session,generation],|r|r.get(0))?)}).await.ok().and_then(Result::ok).unwrap_or(false)
    }
    pub(crate) async fn claim(
        &self,
        request: &str,
        actor: &crate::public_commands::Actor,
        previous: Option<&str>,
    ) -> io::Result<Value> {
        let mut lease = self.lease.lock().await;
        if let Some(old) = lease.as_ref() {
            if previous != Some(old.id.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "exclusive manual lease exists",
                ));
            }
            old.connection.remove(&old.id)?;
            *lease = None;
        } else if previous.is_some() {
            return Err(error());
        }
        let binding = self.binding()?;
        let (runtime, revision) = self.projection().await?;
        let id = uuid::Uuid::new_v4().to_string();
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let path = self.private.root.join(format!("lease-{id}"));
        let fence = crewd_exec::acp::manual_fence::ManualFence::create(&path, nonce)?;
        let admitted = self
            .call(
                request,
                "native/manual.bind",
                json!({"leaseId":id,"path":path,"nonce":nonce,"durationMs":30000}),
            )
            .await?;
        if admitted["adopted"] != true || admitted["leaseId"] != id {
            return Err(error());
        }
        fence.seal_after_helper_adopted()?;
        let registration = runtime.clone();
        self.db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    native::revoke_before_raw(
                        tx,
                        &registration.id,
                        revision,
                        &crewd_store::now_ts(),
                    )
                })
            })
            .await
            .map_err(|_| error())?
            .map_err(|_| error())?;
        if !same(&self.binding()?.native, &binding.native) {
            return Err(error());
        }
        let connection = actor.connection_scope().clone();
        connection.enable(id.clone(), fence.clone())?;
        *lease = Some(ManualLease {
            id: id.clone(),
            connection,
            grant: actor.remote_grant(),
            fence,
            deadline: Instant::now() + Duration::from_secs(30),
            native: binding.native,
        });
        Ok(
            json!({"leaseId":id,"durationMs":30000,"manualRevocationPersisted":true,"physicalInputWritten":false}),
        )
    }
    pub(crate) async fn release(
        &self,
        request: &str,
        actor: &crate::public_commands::Actor,
        id: &str,
    ) -> io::Result<Value> {
        let mut lease = self.lease.lock().await;
        let current = lease
            .as_ref()
            .filter(|lease| lease.id == id && lease.connection.id == actor.connection_scope().id)
            .ok_or_else(error)?;
        current.connection.remove(id)?;
        // Byte revocation is already physically acknowledged. A lost helper
        // release reply remains unknown; it is never replayed to a new helper.
        let result = self
            .call(request, "native/manual.release", json!({"leaseId":id}))
            .await;
        *lease = None;
        result
    }
    pub(crate) async fn write(
        &self,
        request: &str,
        actor: &crate::public_commands::Actor,
        id: &str,
        bytes: Vec<u8>,
    ) -> io::Result<Value> {
        let lease = self.lease.lock().await;
        let current = lease
            .as_ref()
            .filter(|lease| {
                lease.id == id
                    && lease.connection.id == actor.connection_scope().id
                    && lease.connection.is_live()
                    && Instant::now() < lease.deadline
            })
            .ok_or_else(error)?;
        if !same(&self.binding()?.native, &current.native)
            || bytes.is_empty()
            || bytes.len() > 65536
        {
            return Err(error());
        }
        let (runtime, revision) = self.projection().await?;
        let result = self
            .call(
                request,
                "native/manual.write",
                json!({"leaseId":id,"runtimeId":runtime.id,"revision":revision,"bytes":bytes}),
            )
            .await;
        if result.is_err() {
            let _ = current
                .fence
                .revoke(Instant::now() + Duration::from_secs(2));
        }
        result
    }
    pub(crate) async fn resize(
        &self,
        request: &str,
        actor: &crate::public_commands::Actor,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> io::Result<Value> {
        let lease = self.lease.lock().await;
        let current = lease
            .as_ref()
            .filter(|lease| {
                lease.id == id
                    && lease.connection.id == actor.connection_scope().id
                    && lease.connection.is_live()
                    && Instant::now() < lease.deadline
            })
            .ok_or_else(error)?;
        if !same(&self.binding()?.native, &current.native) {
            return Err(error());
        }
        self.call(
            request,
            "native/manual.resize",
            json!({"leaseId":id,"columns":cols,"rows":rows}),
        )
        .await
    }
    pub(crate) async fn revoke_grant(&self, grant: Option<&str>) -> io::Result<()> {
        let mut lease = self.lease.lock().await;
        if let Some(current) = lease
            .as_ref()
            .filter(|lease| grant.is_none() || lease.grant.as_deref() == grant)
        {
            current
                .fence
                .revoke(Instant::now() + Duration::from_secs(2))?;
            current.connection.remove(&current.id)?;
            *lease = None;
        }
        Ok(())
    }
    pub fn events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub(crate) fn original_gone(&self) -> bool {
        let Ok(held) = self.custody.lock() else {
            return false;
        };
        let Some((_, identity)) = held.native_identities().max_by_key(|(epoch, _)| *epoch) else {
            return false;
        };
        match registry::probe(identity.pid) {
            ProbeResult::Missing => true,
            ProbeResult::Found(ref actual) => !same(actual, identity),
            ProbeResult::Unreadable(_) => false,
        }
    }
    pub(crate) fn is_live(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
            && self
                .custody
                .lock()
                .ok()
                .is_some_and(|held| current(held.helper_identity()))
    }
    pub(crate) async fn call(&self, id: &str, method: &str, params: Value) -> io::Result<Value> {
        if !self.is_live()
            || !uuid::Uuid::parse_str(id).is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == id)
        {
            return Err(error());
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| error())?;
            if pending.len() >= 64 || pending.contains_key(id) {
                return Err(error());
            }
            pending.insert(id.to_owned(), tx);
        }
        let outcome = tokio::time::timeout(Duration::from_secs(2), async {
            let mut writer = self.writer.lock().await;
            write_control(
                &mut *writer,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await?;
            rx.await.map_err(|_| error())?
        })
        .await;
        if let Ok(Ok(result)) = outcome {
            return Ok(result);
        }
        self.healthy.store(false, Ordering::Release);
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "native control result unknown; never replay",
        ))
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
        let frames = self.frames.lock().map_err(|_| error())?;
        let result = frames
            .iter()
            .filter(|f| f.epoch == epoch && f.sequence > after)
            .cloned()
            .collect::<Vec<_>>();
        if result
            .first()
            .is_some_and(|f| f.sequence != after.saturating_add(1))
        {
            return Err(error());
        }
        Ok(result)
    }
    pub(crate) fn terminal(&self) -> broadcast::Receiver<TerminalFrame> {
        self.terminal.subscribe()
    }
    async fn cleanup_after_helper_exit(&self, observed: bool) {
        self.healthy.store(false, Ordering::Release);
        cleanup_custody(&self.custody);
        if let Ok(mut pending) = self.pending.lock() {
            for (_, reply) in std::mem::take(&mut *pending) {
                let _ = reply.send(Err(error()));
            }
        }
        let session = self.session.clone();
        let _ = self
            .db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    if let Some(runtime) = native::current_runtime(tx, &session)? {
                        native::invalidate_runtime_unknown(tx, &runtime.id)?;
                    }
                    Ok(())
                })
            })
            .await;
        if observed {
            if let Ok(mut held) = self.custody.lock() {
                let epochs = held
                    .native_identities()
                    .map(|(epoch, _)| *epoch)
                    .collect::<Vec<_>>();
                for epoch in epochs {
                    let _ = held.forget_reaped_native(epoch);
                }
                let _ = held.forget_reaped_helper();
            }
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.healthy.store(false, Ordering::Release);
        cleanup_custody(&self.custody);
        if let Ok(tasks) = self.tasks.lock() {
            for task in tasks.iter() {
                task.abort();
            }
        }
    }
}
