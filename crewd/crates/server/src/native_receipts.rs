//! Private Core native receipt route. No public RPC, provider capability, or
//! helper DB writer is created here. The actual SDK adapter is separately gated.
use crewd_exec::acp::{
    bridge::{NativePeer, NativeProxyPeer},
    custody::Custody,
};
use crewd_store::{Db, StoreError, native_execution as n};
use std::{
    io,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

/// Only structured, authenticated native SDK observations may construct these
/// at the adapter boundary. Terminal text and helper keyboard receipts do not.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeEvent {
    Content {
        request: String,
        revision: i64,
        prompt: String,
        effective: Vec<u8>,
    },
    Started {
        request: String,
        revision: i64,
        prompt: String,
        turn: String,
    },
    Completed {
        request: String,
        revision: i64,
        prompt: String,
        turn: String,
        outcome: n::Phase,
    },
}
impl std::fmt::Debug for NativeEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Content { .. } => "NativeEvent::Content(<private>)",
            Self::Started { .. } => "NativeEvent::Started(<private>)",
            Self::Completed { .. } => "NativeEvent::Completed(<private>)",
        })
    }
}
/// The Core-owned transport holds custody; JSON never chooses a runtime/birth.
/// A failed/unknown call poisons this connection. Recovery creates a fresh
/// kernel-bound runtime, preserving old receipts as unknown observations.
enum NativeReceiptPeer {
    Proxy(NativeProxyPeer),
    Direct(NativePeer),
}
impl NativeReceiptPeer {
    fn binding(&self) -> &crewd_exec::acp::ownership::NativeBinding {
        match self {
            Self::Proxy(peer) => peer.binding(),
            Self::Direct(peer) => peer.binding(),
        }
    }
    fn verify(&self, socket: &UnixStream, custody: &Custody) -> io::Result<()> {
        match self {
            Self::Proxy(peer) => peer.verify(socket, custody),
            Self::Direct(peer) => peer.verify(socket, custody),
        }
    }
    fn admit(
        &mut self,
        socket: &UnixStream,
        custody: &Custody,
        generation: &str,
        epoch: u64,
        sequence: u64,
    ) -> io::Result<()> {
        match self {
            Self::Proxy(peer) => peer.admit(socket, custody, generation, epoch, sequence),
            Self::Direct(peer) => {
                peer.verify(socket, custody)?;
                peer.admit(socket, generation, epoch, sequence)
            }
        }
    }
}
pub struct CoreNativeReceipts {
    db: Db,
    custody: Arc<Mutex<Custody>>,
    peer: NativeReceiptPeer,
    socket: UnixStream,
    runtime: n::Runtime,
    closed: bool,
    effective_content: Option<crewd_exec::acp::content_chunks::Assembly>,
}
impl CoreNativeReceipts {
    pub async fn authenticate(
        db: Db,
        custody: Arc<Mutex<Custody>>,
        socket: UnixStream,
        epoch: u64,
        nonce: [u8; 16],
    ) -> io::Result<Self> {
        let peer = {
            let mut held = custody
                .lock()
                .map_err(|_| io::Error::other("native custody poisoned"))?;
            NativeProxyPeer::authenticate(&socket, &mut held, epoch, &nonce)?
        };
        Self::authenticate_peer(db, custody, socket, NativeReceiptPeer::Proxy(peer)).await
    }
    /// Genuine SDK Unix HTTP requests originate from the enrolled native
    /// process itself, as the isolated identity/ACK gate demonstrated. This
    /// enables no producer capability; subsequent observations still need their
    /// exact request, generation, native birth and sequence admission.
    pub async fn authenticate_direct(
        db: Db,
        custody: Arc<Mutex<Custody>>,
        socket: UnixStream,
        epoch: u64,
        expected_nonce: [u8; 16],
        received_nonce: [u8; 16],
    ) -> io::Result<Self> {
        let binding = {
            let held = custody
                .lock()
                .map_err(|_| io::Error::other("native custody poisoned"))?;
            let binding = held.current_native_binding(epoch)?;
            held.verify_helper_channel(&binding.session, &binding.generation, &expected_nonce)?;
            binding
        };
        let peer = NativePeer::authenticate(&socket, binding, &expected_nonce, &received_nonce)?;
        Self::authenticate_peer(db, custody, socket, NativeReceiptPeer::Direct(peer)).await
    }
    async fn authenticate_peer(
        db: Db,
        custody: Arc<Mutex<Custody>>,
        socket: UnixStream,
        peer: NativeReceiptPeer,
    ) -> io::Result<Self> {
        let b = peer.binding();
        let runtime = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: b.session.clone(),
            generation: b.generation.clone(),
            epoch: b.epoch,
            native_pid: b.native.pid,
            native_start: b.native.start_time,
            native_boot: b.native.boot_id.clone(),
        };
        let registration = runtime.clone();
        let runtime = db
            .call_result(move |s| {
                s.with_tx(|tx| {
                    if let Some(current) = n::current_runtime(tx, &registration.session)? {
                        if current.generation != registration.generation
                            || current.epoch != registration.epoch
                            || current.native_pid != registration.native_pid
                            || current.native_start != registration.native_start
                            || current.native_boot != registration.native_boot
                        {
                            return Err(StoreError::Invalid(
                                "foreign current native runtime".into(),
                            ));
                        }
                        Ok(current)
                    } else {
                        n::register(tx, &registration)?;
                        Ok(registration)
                    }
                })
            })
            .await
            .map_err(storage)?
            .map_err(storage)?;
        let route = Self {
            db,
            custody,
            peer,
            socket,
            runtime,
            closed: false,
            effective_content: None,
        };
        route.verify()?;
        Ok(route)
    }
    /// HTTP connection replacement preserves the ONE logical peer sequence.
    /// An unknown/closed route cannot be reset by reconnecting a same-UID peer.
    pub fn rebind_direct_socket(&mut self, socket: UnixStream) -> io::Result<()> {
        if self.closed || !matches!(self.peer, NativeReceiptPeer::Direct(_)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "native route rebind refused",
            ));
        }
        let held = self
            .custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?;
        self.peer.verify(&socket, &held)?;
        self.socket = socket;
        Ok(())
    }
    pub fn runtime_id(&self) -> &str {
        &self.runtime.id
    }
    fn verify(&self) -> io::Result<()> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native receipt route closed",
            ));
        }
        let held = self
            .custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?;
        self.peer.verify(&self.socket, &held)
    }
    async fn commit<T: Send + 'static>(
        &mut self,
        action: impl FnOnce(&rusqlite::Transaction<'_>, &str) -> crewd_store::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let result = async {
            self.verify()?;
            // Poison before the first await: dropping this caller cannot turn
            // an admitted, possibly committed FIFO job into permission to retry.
            self.closed = true;
            let runtime = self.runtime.id.clone();
            let value = self
                .db
                .call_result(move |s| s.with_tx(|tx| action(tx, &runtime)))
                .await
                .map_err(storage)?
                .map_err(storage)?;
            // Persistence is real; a dead/replaced peer receives no effect ACK.
            {
                let held = self
                    .custody
                    .lock()
                    .map_err(|_| io::Error::other("native custody poisoned"))?;
                self.peer.verify(&self.socket, &held)?;
            }
            self.closed = false;
            Ok(value)
        }
        .await;
        if result.is_err() {
            self.closed = true;
        }
        result
    }
    /// Core admission returns the one immutable receipt. A retry confirms it,
    /// and never emits keyboard content or starts a second provider turn.
    pub async fn prepare(
        &mut self,
        request: String,
        body: Vec<u8>,
    ) -> io::Result<(n::Receipt, bool)> {
        if self.effective_content.is_some() {
            self.effective_content = None;
            self.closed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native request interleaves incomplete content",
            ));
        }
        self.commit(move |tx, runtime| {
            n::prepare(tx, runtime, &request, &body, &crewd_store::now_ts())
        })
        .await
    }
    pub async fn cancel(&mut self, request: String, revision: i64) -> io::Result<n::Receipt> {
        // Cancellation discards every incomplete body. Its durable phase guard
        // still owns acceptance; a later remainder cannot become a fresh body.
        self.effective_content = None;
        self.commit(move |tx, runtime| {
            n::request_cancel(tx, runtime, &request, revision, &crewd_store::now_ts())
        })
        .await
    }
    /// The transport may release a matching manual-input ACK only after this
    /// succeeds. A cancelled waiter remains an unknown admitted FIFO outcome.
    pub async fn revoke_before_raw(&mut self, revision: i64) -> io::Result<()> {
        self.effective_content = None;
        self.commit(move |tx, runtime| {
            n::revoke_before_raw(tx, runtime, revision, &crewd_store::now_ts())
        })
        .await
    }
    /// Decode only the bounded private structured stream, after kernel enrollment.
    /// Terminal frames, ACP captions and unknown fields cannot reach SQL effects.
    pub async fn observation_frame(&mut self, bytes: &[u8]) -> io::Result<n::Receipt> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Frame {
            generation: String,
            epoch: u64,
            sequence: u64,
            event: NativeEvent,
        }
        if bytes.len() > crewd_exec::acp::wire::MAX_CONTROL {
            self.closed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native receipt frame limit",
            ));
        }
        let frame: Frame = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(_) => {
                self.closed = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native receipt frame malformed",
                ));
            }
        };
        self.observation(&frame.generation, frame.epoch, frame.sequence, frame.event)
            .await
    }
    /// Separate closed chunk envelope. Native/Mod JSON cannot select a peer or
    /// runtime; every part still passes the same physical admission and sequence.
    /// None is transport progress only, never a durable-content acknowledgement.
    pub async fn content_chunk_frame(&mut self, bytes: &[u8]) -> io::Result<Option<n::Receipt>> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Frame {
            generation: String,
            epoch: u64,
            sequence: u64,
            chunk: crewd_exec::acp::content_chunks::ContentChunk,
        }
        if bytes.len() > crewd_exec::acp::wire::MAX_CONTROL {
            self.effective_content = None;
            self.closed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native content frame limit",
            ));
        }
        let frame: Frame = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(_) => {
                self.effective_content = None;
                self.closed = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native content frame malformed",
                ));
            }
        };
        self.content_chunk(&frame.generation, frame.epoch, frame.sequence, frame.chunk)
            .await
    }
    /// Large effective content uses a bounded sequence of authenticated native
    /// observations. Intermediate frames carry no content/effect ACK. Only the
    /// complete body enters the existing FIFO receipt transaction, and only its
    /// durable result may release the final ACK. This grants no SDK capability.
    pub async fn content_chunk(
        &mut self,
        generation: &str,
        epoch: u64,
        sequence: u64,
        chunk: crewd_exec::acp::content_chunks::ContentChunk,
    ) -> io::Result<Option<n::Receipt>> {
        let result = async {
            self.verify()?;
            {
                let held = self
                    .custody
                    .lock()
                    .map_err(|_| io::Error::other("native custody poisoned"))?;
                self.peer
                    .admit(&self.socket, &held, generation, epoch, sequence)?;
            }
            if self.effective_content.is_none() {
                self.effective_content = Some(crewd_exec::acp::content_chunks::Assembly::new(
                    chunk.request.clone(),
                    chunk.revision,
                    chunk.prompt.clone(),
                    chunk.total_bytes as usize,
                    std::time::Duration::from_secs(2),
                )?);
            }
            let request = chunk.request.clone();
            let revision = chunk.revision;
            let prompt = chunk.prompt.clone();
            let body = self
                .effective_content
                .as_mut()
                .ok_or_else(|| io::Error::other("native assembly unavailable"))?
                .push(chunk)?;
            let Some(body) = body else {
                return Ok(None);
            };
            self.effective_content = None;
            self.commit(move |tx, runtime| {
                n::accept_content(
                    tx,
                    runtime,
                    &request,
                    revision,
                    &prompt,
                    &body,
                    &crewd_store::now_ts(),
                )
            })
            .await
            .map(Some)
        }
        .await;
        if result.is_err() {
            self.effective_content = None;
            self.closed = true;
        }
        result
    }
    pub async fn observation(
        &mut self,
        generation: &str,
        epoch: u64,
        sequence: u64,
        event: NativeEvent,
    ) -> io::Result<n::Receipt> {
        if self.effective_content.is_some() {
            self.effective_content = None;
            self.closed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native observation interleaves incomplete content",
            ));
        }
        let admitted = {
            let held = self
                .custody
                .lock()
                .map_err(|_| io::Error::other("native custody poisoned"))?;
            self.peer
                .admit(&self.socket, &held, generation, epoch, sequence)
        };
        if let Err(error) = admitted {
            self.closed = true;
            return Err(error);
        }
        self.commit(move |tx, runtime| {
            let now = crewd_store::now_ts();
            match event {
                NativeEvent::Content {
                    request,
                    revision,
                    prompt,
                    effective,
                } => n::accept_content(tx, runtime, &request, revision, &prompt, &effective, &now),
                NativeEvent::Started {
                    request,
                    revision,
                    prompt,
                    turn,
                } => n::turn_started(tx, runtime, &request, revision, &prompt, &turn, &now),
                NativeEvent::Completed {
                    request,
                    revision,
                    prompt,
                    turn,
                    outcome,
                } => n::turn_completed(
                    tx,
                    runtime,
                    &request,
                    revision,
                    n::Completion {
                        prompt_id: &prompt,
                        turn_id: &turn,
                        outcome,
                    },
                    &now,
                ),
            }
        })
        .await
    }
    /// Disconnect does not turn unknown into success, retire a live process,
    /// or unblock automatic input. The existing Core custody owner must stop
    /// and kernel-settle the registered process group separately.
    pub async fn disconnected(&mut self) -> io::Result<()> {
        self.closed = true;
        self.custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?
            .close_admission();
        let runtime = self.runtime.id.clone();
        self.db
            .call_result(move |s| s.with_tx(|tx| n::invalidate_runtime_unknown(tx, &runtime)))
            .await
            .map_err(storage)?
            .map_err(storage)
    }
}
/// Core's actual private helper channel persists session-ID facts on the same
/// DB actor as business receipts. The helper holds no database writer.
pub struct CoreHelperLedger {
    db: Db,
    custody: Arc<Mutex<Custody>>,
    peer: crewd_exec::acp::bridge::HelperPeer,
    socket: UnixStream,
    session: String,
    generation: String,
    nonce: [u8; 16],
    next: u64,
    closed: bool,
    native: Option<Arc<tokio::sync::Mutex<CoreNativeReceipts>>>,
    manual_runtime: Option<n::Runtime>,
}
impl CoreHelperLedger {
    pub fn authenticate(
        db: Db,
        custody: Arc<Mutex<Custody>>,
        socket: UnixStream,
        session: String,
        generation: String,
        nonce: [u8; 16],
    ) -> io::Result<Self> {
        if nonce == [0; 16] {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "missing helper nonce",
            ));
        }
        let peer = {
            let held = custody
                .lock()
                .map_err(|_| io::Error::other("native custody poisoned"))?;
            held.verify_helper_channel(&session, &generation, &nonce)?;
            crewd_exec::acp::bridge::HelperPeer::authenticate(&socket, &held)?
        };
        Ok(Self {
            db,
            custody,
            peer,
            socket,
            session,
            generation,
            nonce,
            next: 1,
            closed: false,
            native: None,
            manual_runtime: None,
        })
    }
    /// Called by Core after actual native birth enrollment, before its pre-exec
    /// ACK. This sole runtime row serves both public manual custody and the
    /// later genuine SDK observation route; it grants no producer capability.
    pub async fn register_native_birth(&mut self, epoch: u64) -> io::Result<n::Runtime> {
        self.verify()?;
        let binding = self
            .custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?
            .current_native_binding(epoch)?;
        let runtime = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: binding.session,
            generation: binding.generation,
            epoch: binding.epoch,
            native_pid: binding.native.pid,
            native_start: binding.native.start_time,
            native_boot: binding.native.boot_id,
        };
        let registration = runtime.clone();
        self.db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    if let Some(prior) = n::current_runtime(tx, &registration.session)? {
                        // A resumed/refused native can get a new epoch only after
                        // the actual old birth is gone. A PID or JSON epoch cannot
                        // supersede a still-live/unknown runtime or raw authority.
                        let gone = match crewd_exec::registry::probe(prior.native_pid) {
                            crewd_exec::registry::ProbeResult::Missing => true,
                            crewd_exec::registry::ProbeResult::Found(actual) => {
                                actual.start_time != prior.native_start
                                    || actual.boot_id != prior.native_boot
                            }
                            crewd_exec::registry::ProbeResult::Unreadable(_) => false,
                        };
                        if !gone {
                            return Err(StoreError::Invalid(
                                "previous native birth still live or unknown".into(),
                            ));
                        }
                        n::retire_verified_native(tx, &prior.id, &crewd_store::now_ts())?;
                    }
                    n::register(tx, &registration)
                })
            })
            .await
            .map_err(storage)?
            .map_err(storage)?;
        self.verify()?;
        self.custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?
            .current_native_binding(epoch)?;
        self.manual_runtime = Some(runtime.clone());
        Ok(runtime)
    }
    pub async fn attach_native(
        &mut self,
        native: Arc<tokio::sync::Mutex<CoreNativeReceipts>>,
    ) -> io::Result<()> {
        {
            let route = native.lock().await;
            if !Arc::ptr_eq(&self.custody, &route.custody)
                || route.runtime.session != self.session
                || route.runtime.generation != self.generation
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "foreign native route",
                ));
            }
        }
        self.native = Some(native);
        Ok(())
    }
    fn verify(&self) -> io::Result<()> {
        let held = self
            .custody
            .lock()
            .map_err(|_| io::Error::other("native custody poisoned"))?;
        held.verify_helper_channel(&self.session, &self.generation, &self.nonce)?;
        self.peer.verify(&self.socket, &held)
    }
    pub async fn call_frame(&mut self, bytes: &[u8]) -> io::Result<serde_json::Value> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "helper ledger closed",
            ));
        }
        // Drop/cancel during a possibly admitted commit must poison the route.
        self.closed = true;
        let result = self.call_admitted(bytes).await;
        if result.is_ok() {
            self.closed = false;
        }
        result
    }
    async fn call_admitted(&mut self, bytes: &[u8]) -> io::Result<serde_json::Value> {
        use crewd_exec::acp::wire::{self, Control};
        use serde_json::{Value, json};
        self.verify()?;
        let Control::Request { id, method, params } = wire::control(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "helper ledger frame malformed")
        })?
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "helper request required",
            ));
        };
        if id != Value::String(self.next.to_string()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "helper request sequence mismatch",
            ));
        }
        let bound = params
            .as_object()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "helper params required"))?;
        if params["sessionId"] != self.session
            || params["generation"] != self.generation
            || params["nonce"] != json!(self.nonce)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "helper binding mismatch",
            ));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| io::Error::other("helper sequence exhausted"))?;
        let session = self.session.clone();
        let result = match method.as_str() {
            "native/ledger.get" if bound.len() == 3 => {
                let value = self
                    .db
                    .call_result(move |s| {
                        crewd_store::sessions::agent_session_id(s.conn(), &session)
                    })
                    .await
                    .map_err(storage)?
                    .map_err(storage)?;
                json!({"agentSessionId":value})
            }
            "native/ledger.record" if bound.len() == 4 => {
                let value = params["agentSessionId"]
                    .as_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "helper session ID missing")
                    })?
                    .to_owned();
                crewd_exec::resume::check_agent_session_id(&value)?;
                let committed = value.clone();
                self.db
                    .call_result(move |s| {
                        s.with_tx(|tx| {
                            let actual = tx.execute(
                                "UPDATE sessions SET agent_session_id=?2,updated_at=?3 WHERE id=?1",
                                rusqlite::params![session, committed, crewd_store::now_ts()],
                            )?;
                            if actual != 1 {
                                return Err(StoreError::Invalid(
                                    "helper session row not committed".into(),
                                ));
                            }
                            Ok(())
                        })
                    })
                    .await
                    .map_err(storage)?
                    .map_err(storage)?;
                json!({"committed":true,"agentSessionId":value})
            }
            "native/raw.revoke" if bound.len() == 7 => {
                let runtime = params["runtimeId"].as_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "native runtime missing")
                })?;
                let revision = params["expectedRevision"]
                    .as_i64()
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "native revision invalid")
                    })?;
                let request = params["requestId"].as_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "raw request missing")
                })?;
                if !uuid::Uuid::parse_str(request)
                    .is_ok_and(|id| !id.is_nil() && id.to_string() == request)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "raw request invalid",
                    ));
                }
                let bytes: Vec<u8> = serde_json::from_value(params["rawBytes"].clone())
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "raw bytes invalid"))?;
                if bytes.is_empty() || bytes.len() > 65536 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "raw bytes limit",
                    ));
                }
                if let Some(native) = &self.native {
                    let mut route = native.lock().await;
                    if runtime != route.runtime_id() {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "raw runtime mismatch",
                        ));
                    }
                    route.revoke_before_raw(revision).await?;
                } else {
                    let bound = self
                        .manual_runtime
                        .as_ref()
                        .filter(|bound| bound.id == runtime)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "native route unavailable",
                            )
                        })?
                        .clone();
                    let birth = self
                        .custody
                        .lock()
                        .map_err(|_| io::Error::other("native custody poisoned"))?
                        .current_native_binding(bound.epoch)?;
                    if birth.native.pid != bound.native_pid
                        || birth.native.start_time != bound.native_start
                        || birth.native.boot_id != bound.native_boot
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "native manual birth changed",
                        ));
                    }
                    self.db
                        .call_result(move |store| {
                            store.with_tx(|tx| {
                                n::revoke_before_raw(
                                    tx,
                                    &bound.id,
                                    revision,
                                    &crewd_store::now_ts(),
                                )
                            })
                        })
                        .await
                        .map_err(storage)?
                        .map_err(storage)?;
                }
                json!({"committed":true,"manual":true,"runtimeId":runtime,"requestId":request})
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "helper ledger method unavailable",
                ));
            }
        };
        self.verify()?;
        Ok(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

fn storage(error: StoreError) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_exec::{
        acp::birth::{BirthReport, REPORT_LEN},
        registry::{self, ProbeResult, ProcessIdentity, ProcessRegistry},
    };
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixListener, process::CommandExt},
        },
        process::{Child, Command},
    };
    struct Peer {
        child: Child,
        channel: UnixStream,
        native: Option<ProcessIdentity>,
    }
    impl Drop for Peer {
        fn drop(&mut self) {
            if let Some(native) = &self.native {
                registry::apply(
                    &registry::decide(native, &registry::probe(native.pid)),
                    native,
                );
            }
            if let ProbeResult::Found(helper) = registry::probe(self.child.id() as i32) {
                registry::apply(
                    &registry::decide(&helper, &registry::probe(helper.pid)),
                    &helper,
                );
            }
            let _ = self.child.wait();
        }
    }
    fn peer(dir: &std::path::Path, nonce: [u8; 16]) -> Peer {
        let script = dir.join("owned-acp-peer.py");
        std::fs::write(
            &script,
            r#"
import os, socket, struct, pty, termios, fcntl
control = socket.socket(fileno=int(os.environ['CONTROL_FD']))
nonce = bytes.fromhex(os.environ['BIRTH_NONCE'])
master, slave = pty.openpty()
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2): os.dup2(slave, fd)
    if slave > 2: os.close(slave)
    report = b'PCB1' + struct.pack('>iQ', os.getpid(), 1) + nonce
    control.sendall(report)
    ack = b''
    while len(ack) < 33:
        part = control.recv(33 - len(ack))
        if not part: os._exit(2)
        ack += part
    if ack != report + b'\x01': os._exit(3)
    with open(os.environ['EXEC_MARKER'], 'w') as f:
        f.write('tty' if all(os.isatty(fd) for fd in (0,1,2)) else 'not-tty')
    proxy_pid = None
    if os.environ.get('PROXY_SOCKET'):
        proxy_pid = os.fork()
        if proxy_pid == 0:
            control.close()
            proxy = socket.socket(socket.AF_UNIX)
            proxy.connect(os.environ['PROXY_SOCKET'])
            pending = b''
            while True:
                chunk = proxy.recv(65536)
                if not chunk: break
                pending += chunk
                while b'\n' in pending:
                    frame,pending = pending.split(b'\n',1)
                    if frame == b'Q': os._exit(0)
                    proxy.sendall(frame+b'\n')
            os._exit(0)
    control.sendall(b'R')
    control.recv(1)
    if proxy_pid is not None: os.waitpid(proxy_pid, 0)
    os._exit(0)
os.close(slave)
control.close()
os.waitpid(pid, 0)
os.close(master)
"#,
        )
        .unwrap();
        let (channel, inherited) = UnixStream::pair().unwrap();
        channel
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let inherited_fd = inherited.as_raw_fd();
        let mut command = Command::new("/usr/bin/python3");
        command
            .arg(script)
            .env("CONTROL_FD", inherited_fd.to_string())
            .env(
                "BIRTH_NONCE",
                nonce.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            )
            .env("EXEC_MARKER", dir.join("actual-native-exec"));
        let proxy_socket = dir.join("proxy.sock");
        if proxy_socket.exists() {
            command.env("PROXY_SOCKET", proxy_socket);
        }
        // SAFETY: only async-signal-safe setsid/fcntl on an owned descriptor.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(inherited_fd, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(inherited_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop(inherited);
        Peer {
            child,
            channel,
            native: None,
        }
    }
    fn receive(peer: &mut Peer) -> BirthReport {
        let mut bytes = [0; REPORT_LEN];
        peer.channel.read_exact(&mut bytes).unwrap();
        let report = BirthReport::decode(&bytes).unwrap();
        let ProbeResult::Found(identity) = registry::probe(report.pid) else {
            panic!("actual native birth absent")
        };
        peer.native = Some(identity);
        report
    }

    #[tokio::test]
    #[ignore = "owned private Core/helper subprocess only"]
    async fn actual_ledger_runtime_child() {
        use crewd_exec::resume::AgentIdLedger;
        use std::os::fd::FromRawFd;
        let fd: i32 = std::env::var("PCW_CORE_FD").unwrap().parse().unwrap();
        // SAFETY: only the owned parent provides this single inherited descriptor.
        let mut inherited = unsafe { UnixStream::from_raw_fd(fd) };
        // Actual writer evidence for the Linux negative socketpair control.
        inherited.write_all(b"H").unwrap();
        let core = serde_json::from_str(&std::env::var("PCW_CORE_BIRTH").unwrap()).unwrap();
        let nonce = serde_json::from_str(&std::env::var("PCW_CORE_NONCE").unwrap()).unwrap();
        let path = std::path::PathBuf::from(std::env::var_os("PCW_CORE_SOCKET").unwrap());
        let ledger = crewd_exec::acp::helper::CoreLedger::connect(
            &path,
            "helper-ledger".into(),
            std::env::var("PCW_CORE_GENERATION").unwrap(),
            nonce,
            core,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(ledger.recorded().unwrap(), None);
        ledger.record("genuine-isolated-session").unwrap();
        assert_eq!(
            ledger.recorded().unwrap(),
            Some("genuine-isolated-session".into())
        );
        assert!(
            ledger
                .revoke_for_raw(
                    &uuid::Uuid::new_v4().to_string(),
                    0,
                    b"not-authorized".to_vec()
                )
                .is_err()
        );
        assert_eq!(
            ledger.recorded().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[tokio::test]
    async fn actual_rust_helper_core_private_ledger_new_load_durable_and_no_foreign_raw() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProcessRegistry::load(&dir.path().join("processes.json")).unwrap());
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let generation = uuid::Uuid::new_v4();
        let path = dir.path().join("core-ledger.sock");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (mut creator_pair, inherited) = UnixStream::pair().unwrap();
        creator_pair
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let fd = inherited.as_raw_fd();
        let ProbeResult::Found(core) = registry::probe(std::process::id() as i32) else {
            panic!("actual Core birth missing")
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "native_receipts::tests::actual_ledger_runtime_child",
            ])
            .env_clear()
            .env("PCW_CORE_FD", fd.to_string())
            .env("PCW_CORE_SOCKET", &path)
            .env("PCW_CORE_BIRTH", serde_json::to_string(&core).unwrap())
            .env("PCW_CORE_NONCE", serde_json::to_string(&nonce).unwrap())
            .env("PCW_CORE_GENERATION", generation.to_string());
        // SAFETY: only async-signal-safe setsid/fcntl before owned child exec.
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
        let child = command.spawn().unwrap();
        drop(inherited);
        let mut physical = Peer {
            child,
            channel: creator_pair.try_clone().unwrap(),
            native: None,
        };
        let owned = Custody::enroll_helper(
            registry.clone(),
            "helper-ledger".into(),
            generation,
            nonce,
            physical.child.id() as i32,
        )
        .unwrap();
        let custody = Arc::new(Mutex::new(owned));
        let mut marker = [0];
        creator_pair.read_exact(&mut marker).unwrap();
        assert_eq!(marker, [b'H']);
        #[cfg(target_os = "linux")]
        {
            let mut value = std::mem::MaybeUninit::<libc::ucred>::uninit();
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            // SAFETY: live owned socket and exact ucred output buffer.
            assert_eq!(
                unsafe {
                    libc::getsockopt(
                        creator_pair.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        value.as_mut_ptr().cast(),
                        &mut len,
                    )
                },
                0
            );
            assert_eq!(len as usize, std::mem::size_of::<libc::ucred>());
            // SAFETY: full successful kernel write above.
            let credentials = unsafe { value.assume_init() };
            assert_eq!(credentials.pid, std::process::id() as i32);
            assert_ne!(credentials.pid, physical.child.id() as i32);
            assert!(
                crewd_exec::acp::bridge::HelperPeer::authenticate(
                    &creator_pair,
                    &custody.lock().unwrap()
                )
                .is_err(),
                "actual child wrote H but inherited creator credentials must NOT authenticate helper"
            );
            eprintln!(
                "Linux inherited socketpair creator PID {} is not actual writer helper PID {}; guard rejected as required",
                credentials.pid,
                physical.child.id()
            );
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(e) => panic!("actual helper connect absent: {e}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let store = crewd_store::Store::open_in_memory().unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('c','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('helper-ledger','c','worker','claude','now');").unwrap();
        let db = Db::spawn(store).unwrap();
        assert!(
            CoreHelperLedger::authenticate(
                db.clone(),
                custody.clone(),
                socket.try_clone().unwrap(),
                "foreign".into(),
                generation.to_string(),
                nonce
            )
            .is_err()
        );
        let mut route = CoreHelperLedger::authenticate(
            db.clone(),
            custody.clone(),
            socket.try_clone().unwrap(),
            "helper-ledger".into(),
            generation.to_string(),
            nonce,
        )
        .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        for index in 0..4 {
            let mut frame = Vec::new();
            reader.read_until(b'\n', &mut frame).unwrap();
            let reply = route.call_frame(&frame).await;
            let value = if index < 3 {
                reply.unwrap()
            } else {
                assert!(reply.is_err());
                let id: serde_json::Value = serde_json::from_slice(&frame).unwrap();
                serde_json::json!({"jsonrpc":"2.0","id":id["id"],"error":{"code":-32000,"message":"native route unavailable"}})
            };
            socket
                .write_all(&serde_json::to_vec(&value).unwrap())
                .unwrap();
            socket.write_all(b"\n").unwrap();
        }
        assert!(physical.child.wait().unwrap().success());
        let saved = db
            .call_result(move |s| {
                crewd_store::sessions::agent_session_id(s.conn(), "helper-ledger")
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, Some("genuine-isolated-session".into()));
        {
            let mut held = custody.lock().unwrap();
            held.close_admission();
            held.forget_reaped_helper().unwrap();
        }
        assert!(registry.entries().is_empty());
        db.shutdown();
    }
    async fn observed(
        route: &mut CoreNativeReceipts,
        generation: &str,
        sequence: u64,
        event: NativeEvent,
    ) -> n::Receipt {
        let mut wire = route.socket.try_clone().unwrap();
        let frame = serde_json::json!({"generation":generation,"epoch":1,"sequence":sequence,"event":event});
        wire.write_all(&serde_json::to_vec(&frame).unwrap())
            .unwrap();
        wire.write_all(b"\n").unwrap();
        let mut echoed = Vec::new();
        BufReader::new(wire).read_until(b'\n', &mut echoed).unwrap();
        route.observation_frame(&echoed).await.unwrap()
    }
    #[tokio::test]
    async fn actual_enrolled_pty_proxy_core_fifo_exact_content_cancel_late_start_raw_and_disconnect()
     {
        actual_flow(false, false).await;
    }
    #[tokio::test]
    async fn actual_enrolled_proxy_cancelled_fifo_waiter_is_unknown_and_cannot_reemit() {
        actual_flow(true, false).await;
    }
    #[tokio::test]
    async fn actual_enrolled_proxy_full_body_chunks_commit_only_after_complete_authenticated_cut() {
        actual_flow(false, true).await;
    }
    async fn actual_flow(cancel_waiter: bool, chunked: bool) {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProcessRegistry::load(&dir.path().join("processes.json")).unwrap());
        let listener = UnixListener::bind(dir.path().join("proxy.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let generation = uuid::Uuid::new_v4();
        let mut physical = peer(dir.path(), nonce);
        let report = receive(&mut physical);
        let mut owned = Custody::enroll_helper(
            registry.clone(),
            "worker-owned".into(),
            generation,
            nonce,
            physical.child.id() as i32,
        )
        .unwrap();
        let native = owned.enroll_native(&report).unwrap();
        physical
            .channel
            .write_all(native.acknowledgement())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(e) => panic!("proxy absent: {e}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut ready = [0];
        physical.channel.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [b'R']);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("actual-native-exec")).unwrap(),
            "tty"
        );
        let store = crewd_store::Store::open_in_memory().unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('c','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('worker-owned','c','worker','claude','now');").unwrap();
        let db = Db::spawn(store).unwrap();
        let custody = Arc::new(Mutex::new(owned));
        let mut route =
            CoreNativeReceipts::authenticate(db.clone(), custody.clone(), socket, 1, nonce)
                .await
                .unwrap();
        assert_eq!(
            ProcessRegistry::load(&dir.path().join("processes.json"))
                .unwrap()
                .entries()
                .len(),
            3
        );
        let request = uuid::Uuid::new_v4().to_string();
        let prompt = uuid::Uuid::new_v4().to_string();
        let body = if chunked {
            // JSON byte arrays would exceed the control frame bound for this
            // legal body. Chunk framing must retain every control byte losslessly.
            vec![b'\r'; 1_048_576]
        } else {
            b" exact body\n".to_vec()
        };
        if cancel_waiter {
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = db.clone();
            let blocked = tokio::spawn(async move {
                blocker
                    .call(move |_| {
                        let _ = entered_tx.send(());
                        release_rx.recv().unwrap();
                    })
                    .await
                    .unwrap();
            });
            entered_rx.await.unwrap();
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(30),
                    route.prepare(request.clone(), body.clone())
                )
                .await
                .is_err()
            );
            assert!(route.closed, "dropped waiter must leave route poisoned");
            release_tx.send(()).unwrap();
            blocked.await.unwrap();
            let rid = request.clone();
            let committed = db
                .call_result(move |s| n::get(s.conn(), &rid))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(committed.phase, n::Phase::Prepared);
            assert!(
                route.prepare(request.clone(), body.clone()).await.is_err(),
                "unknown socket cannot re-emit"
            );
            route.disconnected().await.unwrap();
            let rid = request.clone();
            let unknown = db
                .call_result(move |s| n::get(s.conn(), &rid))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(unknown.phase, n::Phase::Unknown);
            let count = db
                .call(move |s| {
                    s.conn()
                        .query_row("SELECT COUNT(*) FROM native_execution_inputs", [], |r| {
                            r.get::<_, i64>(0)
                        })
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(count, 1);
            let slot = route.peer.enrollment().slot();
            route.socket.write_all(b"Q\n").unwrap();
            physical.channel.write_all(b"Q").unwrap();
            assert!(physical.child.wait().unwrap().success());
            {
                let mut owned = custody.lock().unwrap();
                owned.forget_reaped_bridge(1, slot).unwrap();
                owned.forget_reaped_native(1).unwrap();
                owned.forget_reaped_helper().unwrap();
            }
            assert!(registry.entries().is_empty());
            db.shutdown();
            return;
        }
        let (prepared, retry) = route.prepare(request.clone(), body.clone()).await.unwrap();
        assert!(!retry);
        let (same, retry) = route.prepare(request.clone(), body.clone()).await.unwrap();
        assert!(retry);
        assert_eq!(same, prepared);
        let mut next_sequence = 1;
        let accepted = if chunked {
            let chunks = crewd_exec::acp::content_chunks::encode(
                &request,
                prepared.revision,
                &prompt,
                std::str::from_utf8(&body).unwrap(),
            )
            .unwrap();
            let count = chunks.len();
            let mut final_receipt = None;
            for (index, chunk) in chunks.into_iter().enumerate() {
                let frame = serde_json::json!({"generation":generation.to_string(),"epoch":1,"sequence":next_sequence,"chunk":chunk});
                let encoded = serde_json::to_vec(&frame).unwrap();
                assert!(encoded.len() <= crewd_exec::acp::wire::MAX_CONTROL);
                let mut wire = route.socket.try_clone().unwrap();
                wire.write_all(&encoded).unwrap();
                wire.write_all(b"\n").unwrap();
                let mut echoed = Vec::new();
                BufReader::new(wire).read_until(b'\n', &mut echoed).unwrap();
                let receipt = route.content_chunk_frame(&echoed).await.unwrap();
                next_sequence += 1;
                if index + 1 < count {
                    assert!(
                        receipt.is_none(),
                        "partial content must not return a durable ACK"
                    );
                    let request = request.clone();
                    let durable = db
                        .call_result(move |store| n::get(store.conn(), &request))
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    assert_eq!(durable.phase, n::Phase::Prepared);
                    assert_eq!(durable.revision, prepared.revision);
                } else {
                    final_receipt = receipt;
                }
            }
            let receipt = final_receipt.expect("only complete content has durable ACK");
            let request = request.clone();
            let expected = body.clone();
            let runtime = route.runtime_id().to_owned();
            let effective = db
                .call_result(move |store| {
                    n::original_input(store.conn(), &runtime, &request)
                        .map(|row| row.map(|v| v.bytes().to_vec()))
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(effective, Some(expected));
            receipt
        } else {
            let receipt = observed(
                &mut route,
                &generation.to_string(),
                1,
                NativeEvent::Content {
                    request: request.clone(),
                    revision: prepared.revision,
                    prompt: prompt.clone(),
                    effective: body.clone(),
                },
            )
            .await;
            next_sequence += 1;
            receipt
        };
        assert_eq!(accepted.phase, n::Phase::ContentAccepted);
        let canceled = route
            .cancel(request.clone(), accepted.revision)
            .await
            .unwrap();
        assert_eq!(canceled.phase, n::Phase::CancelRequested);
        let started = observed(
            &mut route,
            &generation.to_string(),
            next_sequence,
            NativeEvent::Started {
                request: request.clone(),
                revision: canceled.revision,
                prompt: prompt.clone(),
                turn: "actual-synthetic-turn".into(),
            },
        )
        .await;
        assert_eq!(started.phase, n::Phase::CancelRequested);
        route.revoke_before_raw(0).await.unwrap();
        let rid = request.clone();
        let revoked = db
            .call_result(move |s| n::get(s.conn(), &rid))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(revoked.phase, n::Phase::RawRevoked);
        let completed = observed(
            &mut route,
            &generation.to_string(),
            next_sequence + 1,
            NativeEvent::Completed {
                request: request.clone(),
                revision: revoked.revision,
                prompt: prompt.clone(),
                turn: "actual-synthetic-turn".into(),
                outcome: n::Phase::Aborted,
            },
        )
        .await;
        assert_eq!(completed.phase, n::Phase::Aborted);
        let runtime = route.runtime_id().to_owned();
        let check = runtime.clone();
        assert_eq!(
            db.call(move |s| s.conn().query_row(
                "SELECT manual FROM native_execution_runtimes WHERE runtime_id=?1",
                [check],
                |r| r.get::<_, i64>(0)
            ))
            .await
            .unwrap()
            .unwrap(),
            1
        );
        route.disconnected().await.unwrap();
        assert!(
            route
                .prepare(uuid::Uuid::new_v4().to_string(), b"new".to_vec())
                .await
                .is_err()
        );
        let slot = route.peer.enrollment().slot();
        route.socket.write_all(b"Q\n").unwrap();
        physical.channel.write_all(b"Q").unwrap();
        assert!(physical.child.wait().unwrap().success());
        {
            let mut owned = custody.lock().unwrap();
            owned.forget_reaped_bridge(1, slot).unwrap();
            owned.forget_reaped_native(1).unwrap();
            owned.forget_reaped_helper().unwrap();
        }
        assert!(registry.entries().is_empty());
        db.shutdown();
    }
}
