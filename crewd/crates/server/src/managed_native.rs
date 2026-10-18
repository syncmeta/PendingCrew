//! Production managed-task bridge. One Core owns intent, native custody and API
//! attribution. Native events remain observations, not guessed request completion.
use crate::{AppState, native_helpers};
use crewd_exec::acp::bridge::NativePeer;
use crewd_exec::session::{AnswerOutcome, SessionEvent};
use crewd_store::{native_execution::Runtime as NativeRuntime, native_managed as ledger};
use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
};
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "managed native channel unavailable",
    )
}
fn storage(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
const MAX_HTTP: usize = 7 * 1024 * 1024;
struct PendingPermission {
    decision: PendingDecision,
    params: Value,
    answer: Option<Value>,
    operation: Option<String>,
    expires: Instant,
    ack: Option<oneshot::Sender<io::Result<()>>>,
}
pub(crate) struct Bridge {
    state: Weak<AppState>,
    native: Arc<native_helpers::Runtime>,
    session: String,
    token: String,
    nonce: uuid::Uuid,
    sequence: AtomicU64,
    ready: AtomicBool,
    closed: AtomicBool,
    queue: Mutex<VecDeque<String>>,
    callbacks: Mutex<BTreeMap<String, PendingPermission>>,
    outgoing_ack: Mutex<Option<oneshot::Sender<io::Result<()>>>>,
    active_turn: Mutex<Option<String>>,
    events: mpsc::UnboundedSender<SessionEvent>,
}
impl Bridge {
    pub(crate) fn new(
        state: &Arc<AppState>,
        native: Arc<native_helpers::Runtime>,
        session: String,
        token: String,
        nonce: uuid::Uuid,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(state),
            native,
            session,
            token,
            nonce,
            sequence: AtomicU64::new(1),
            ready: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            callbacks: Mutex::new(BTreeMap::new()),
            outgoing_ack: Mutex::new(None),
            active_turn: Mutex::new(None),
            events,
        })
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && !self.closed.load(Ordering::Acquire)
            && self.native.is_live()
    }
    async fn authority(&self) -> io::Result<(Arc<AppState>, NativeRuntime)> {
        let state = self.state.upgrade().ok_or_else(denied)?;
        if self.closed.load(Ordering::Acquire)
            || !state.admission.contains(&self.session)
            || !state.tokens.verify(&self.session, &self.token)
        {
            return Err(denied());
        }
        let (runtime, _) = self.native.projection().await?;
        Ok((state, runtime))
    }
    /// Internal Core operation, never an unauthenticated public JSON envelope.
    async fn persist_operation(
        &self,
        kind: ledger::Kind,
        value: Value,
        explicit_human: bool,
    ) -> io::Result<String> {
        if !self.is_ready() {
            return Err(denied());
        }
        let (state, runtime) = self.authority().await?;
        let operation = uuid::Uuid::new_v4().to_string();
        let saved = operation.clone();
        let body = serde_json::to_vec(&value).map_err(storage)?;
        let owner = state.clone();
        let session = self.session.clone();
        let token = self.token.clone();
        state
            .db
            .call_result(move |store| -> crewd_collab::Result<()> {
                store.with_tx(|tx| {
                    if !owner.admission.contains(&session) || !owner.tokens.verify(&session, &token)
                    {
                        return Err(crewd_store::StoreError::Invalid(
                            "managed run authority closed".into(),
                        ));
                    }
                    let row = crewd_store::sessions::get(tx, &session)?.ok_or_else(|| {
                        crewd_store::StoreError::Invalid("managed session missing".into())
                    })?;
                    crewd_collab::team_data::authorize(
                        tx,
                        &crewd_collab::Sender::Session(session.clone()),
                        &row.crew_id,
                        true,
                    )
                    .map_err(|e| crewd_store::StoreError::Invalid(e.to_string()))?;
                    ledger::prepare(
                        tx,
                        &runtime,
                        &format!("core:{session}"),
                        explicit_human,
                        kind,
                        &saved,
                        &body,
                        &crewd_store::now_ts(),
                    )?;
                    Ok(())
                })?;
                Ok(())
            })
            .await
            .map_err(storage)?
            .map_err(storage)?;
        Ok(operation)
    }
    pub(crate) async fn submit(
        &self,
        kind: ledger::Kind,
        value: Value,
        explicit_human: bool,
    ) -> io::Result<String> {
        let operation = self.persist_operation(kind, value, explicit_human).await?;
        let queued = {
            let mut queue = self.queue.lock().map_err(|_| denied())?;
            if queue.len() < 64 {
                queue.push_back(operation.clone());
                true
            } else {
                false
            }
        };
        if !queued {
            self.close().await;
            return Err(denied());
        }
        Ok(operation)
    }
    async fn poll(&self) -> io::Result<Value> {
        let operation = self.queue.lock().map_err(|_| denied())?.pop_front();
        let Some(operation) = operation else {
            return Ok(Value::Null);
        };
        let (state, runtime) = self.authority().await?;
        let saved = operation.clone();
        let owner = state.clone();
        let session = self.session.clone();
        let token = self.token.clone();
        let value = state
            .db
            .call_result(move |store| -> crewd_collab::Result<Value> {
                store
                    .with_tx(|tx| {
                        if !owner.admission.contains(&session)
                            || !owner.tokens.verify(&session, &token)
                        {
                            return Err(crewd_store::StoreError::Invalid(
                                "managed release authority closed".into(),
                            ));
                        }
                        let row = crewd_store::sessions::get(tx, &session)?.ok_or_else(|| {
                            crewd_store::StoreError::Invalid("managed session missing".into())
                        })?;
                        crewd_collab::team_data::authorize(
                            tx,
                            &crewd_collab::Sender::Session(session.clone()),
                            &row.crew_id,
                            true,
                        )
                        .map_err(|e| crewd_store::StoreError::Invalid(e.to_string()))?;
                        if !ledger::release(tx, &runtime, &saved, &crewd_store::now_ts())? {
                            return Err(crewd_store::StoreError::Invalid(
                                "managed operation cannot be released twice".into(),
                            ));
                        }
                        let body = ledger::input(tx, &runtime.id, &saved)?;
                        let mut value: Value = serde_json::from_slice(&body)?;
                        value
                            .as_object_mut()
                            .ok_or_else(|| {
                                crewd_store::StoreError::Invalid(
                                    "managed operation object missing".into(),
                                )
                            })?
                            .insert("id".into(), json!(saved));
                        Ok(value)
                    })
                    .map_err(Into::into)
            })
            .await
            .map_err(storage)?
            .map_err(storage)?;
        self.emit(&operation, "dispatched", None);
        Ok(value)
    }
    fn emit(&self, id: &str, phase: &str, result: Option<Value>) {
        let _=self.events.send(SessionEvent::Item {turn_id:String::new(),item:json!({"type":"managedApiOperation","operationId":id,"phase":phase,"result":result,"nativeTurnAttribution":"unverified","workCompletion":"notImplied"})});
    }
    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        if let Some(state) = self.state.upgrade() {
            let session = self.session.clone();
            let _ = state
                .db
                .call_result(move |store| {
                    store.with_tx(|tx| {
                        if let Some(runtime) =
                            crewd_store::native_execution::current_runtime(tx, &session)?
                        {
                            ledger::unknown(tx, &runtime.id, &crewd_store::now_ts())?;
                        }
                        Ok(())
                    })
                })
                .await;
        }
        if let Ok(mut callbacks) = self.callbacks.lock() {
            for (_, mut callback) in std::mem::take(&mut *callbacks) {
                if let Some(ack) = callback.ack.take() {
                    let _ = ack.send(Err(denied()));
                }
            }
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        if let Ok(mut pending) = self.outgoing_ack.lock() {
            if let Some(ack) = pending.take() {
                let _ = ack.send(Err(denied()));
            }
        }
    }
    pub(crate) async fn serve(self: Arc<Self>, listener: UnixListener) {
        loop {
            if self.closed.load(Ordering::Acquire) || !self.native.is_live() {
                self.close().await;
                break;
            }
            let accepted =
                tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
            match accepted {
                Err(_) => continue,
                Ok(Err(_)) => {
                    self.close().await;
                    break;
                }
                Ok(Ok((stream, _))) => {
                    if self.connection(stream).await.is_err() {
                        self.close().await;
                        break;
                    }
                }
            }
        }
    }
    async fn connection(&self, stream: UnixStream) -> io::Result<()> {
        let raw = stream.into_std()?;
        let binding = self.native.binding()?;
        NativePeer::authenticate(
            &raw,
            binding.clone(),
            self.nonce.as_bytes(),
            self.nonce.as_bytes(),
        )?;
        raw.set_nonblocking(true)?;
        let mut stream = UnixStream::from_std(raw)?;
        let bytes = tokio::time::timeout(Duration::from_secs(5), read_http(&mut stream))
            .await
            .map_err(|_| denied())??;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Frame {
            generation: String,
            token: String,
            session_id: String,
            sequence: u64,
            method: String,
            params: Value,
        }
        let frame: Frame = serde_json::from_slice(&bytes).map_err(storage)?;
        let native_id = self.native.agent_session_id().await?;
        if frame.token != self.nonce.to_string()
            || frame.generation != self.native.generation
            || frame.session_id != native_id
            || frame.sequence != self.sequence.load(Ordering::Acquire)
        {
            return Err(denied());
        }
        self.sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
            .map_err(|_| denied())?;
        // Recheck current native birth after reading the bounded request.
        if self.native.binding()?.native != binding.native {
            return Err(denied());
        }
        let result = self.route(&frame.method, frame.params).await?;
        let body = serde_json::to_vec(
            &json!({"generation":self.native.generation,"sequence":frame.sequence,"result":result}),
        )
        .map_err(storage)?;
        if body.len() > MAX_HTTP {
            return Err(denied());
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            stream.write_all(header.as_bytes()).await?;
            stream.write_all(&body).await?;
            stream.shutdown().await
        })
        .await
        .map_err(|_| denied())??;
        if let Some(ack) = self.outgoing_ack.lock().map_err(|_| denied())?.take() {
            let _ = ack.send(Ok(()));
        }
        Ok(())
    }
    async fn route(&self, method: &str, params: Value) -> io::Result<Value> {
        let (state, runtime) = self.authority().await?;
        match method {
            "hello" => {
                if params["version"] != "2.1.296" {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "managed native API version unqualified",
                    ));
                }
                self.ready.store(true, Ordering::Release);
                Ok(json!({"ready":true,"fullHistoryLoad":false}))
            }
            "poll" if params == json!({}) => self.poll().await,
            "returned" => {
                let id = params["operationId"]
                    .as_str()
                    .ok_or_else(denied)?
                    .to_owned();
                let saved = id.clone();
                let result = params.get("value").cloned().ok_or_else(denied)?;
                let owner = state.clone();
                let session = self.session.clone();
                let token = self.token.clone();
                let receipt = state
                    .db
                    .call_result(move |store| {
                        store.with_tx(|tx| {
                            recheck(tx, &owner, &session, &token)?;
                            let operation = ledger::get(tx, &saved)?
                                .filter(|r| r.runtime_id == runtime.id)
                                .ok_or_else(|| {
                                    crewd_store::StoreError::Invalid(
                                        "managed result identity missing".into(),
                                    )
                                })?;
                            let body = ledger::input(tx, &runtime.id, &saved)?;
                            let input: Value = serde_json::from_slice(&body)?;
                            if !valid_api_result(operation.kind, &input, &result) {
                                return Err(crewd_store::StoreError::Invalid(
                                    "managed result shape invalid".into(),
                                ));
                            }
                            ledger::returned(tx, &runtime, &saved, &result, &crewd_store::now_ts())
                        })
                    })
                    .await
                    .map_err(storage)?
                    .map_err(storage)?;
                self.emit(&id, "returned", receipt.result);
                Ok(json!({"recorded":true,"nativeCompletion":false}))
            }
            "unknown" => {
                state
                    .db
                    .call_result(move |store| {
                        store.with_tx(|tx| ledger::unknown(tx, &runtime.id, &crewd_store::now_ts()))
                    })
                    .await
                    .map_err(storage)?
                    .map_err(storage)?;
                self.closed.store(true, Ordering::Release);
                Ok(json!({"unknown":true}))
            }
            "turnStarted" => {
                let turn = params["turnId"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 256)
                    .ok_or_else(denied)?
                    .to_owned();
                *self.active_turn.lock().map_err(|_| denied())? = Some(turn.clone());
                let _=self.events.send(SessionEvent::Item {turn_id:turn,item:json!({"type":"nativeTurnStarted","observation":params,"requestAttribution":"unverified"})});
                Ok(json!({"recorded":true}))
            }
            "turnCompleted" => {
                let turn = params["turnId"].as_str().ok_or_else(denied)?.to_owned();
                if params.get("agentId").is_none() {
                    let mut active = self.active_turn.lock().map_err(|_| denied())?;
                    if active.as_ref() == Some(&turn) {
                        *active = None;
                    }
                }
                let _=self.events.send(SessionEvent::Item {turn_id:turn,item:json!({"type":"nativeTurnCompleted","observation":params,"requestAttribution":"unverified","workCompletion":"notImplied"})});
                Ok(json!({"recorded":true}))
            }
            "permission" => {
                let tool = params["tool_name"].as_str().ok_or_else(denied)?;
                let id = uuid::Uuid::new_v4().to_string();
                let decision = PendingDecision {
                    kind: DecisionKind::RunnerRequest {
                        request_id: id.clone(),
                    },
                    prompt: format!("Claude requests permission: {tool}"),
                    options: vec!["Allow once".into(), "Deny".into()],
                    numbered: false,
                    selected: None,
                };
                let mut callbacks = self.callbacks.lock().map_err(|_| denied())?;
                if !callbacks.is_empty() {
                    return Err(denied());
                }
                callbacks.insert(
                    id.clone(),
                    PendingPermission {
                        decision: decision.clone(),
                        params,
                        answer: None,
                        operation: None,
                        expires: Instant::now() + Duration::from_secs(15 * 60),
                        ack: None,
                    },
                );
                let _ = self
                    .events
                    .send(SessionEvent::Decision(DecisionEvent::Appeared(decision)));
                Ok(json!({"callbackId":id}))
            }
            "permissionAnswer" => {
                let id = params["callbackId"].as_str().ok_or_else(denied)?;
                let mut callbacks = self.callbacks.lock().map_err(|_| denied())?;
                let Some(callback) = callbacks.get_mut(id) else {
                    return Err(denied());
                };
                if callback.expires <= Instant::now() {
                    return Err(denied());
                }
                if callback.answer.is_none() {
                    return Ok(Value::Null);
                }
                let mut callback = callbacks.remove(id).ok_or_else(denied)?;
                let answer = callback.answer.take().ok_or_else(denied)?;
                // This ACK means this genuine pending invocation obtained the
                // durable decision, not that its tool effects occurred.
                *self.outgoing_ack.lock().map_err(|_| denied())? = callback.ack.take();
                Ok(answer)
            }
            _ => Err(denied()),
        }
    }
    pub(crate) fn pending(&self) -> io::Result<Option<crewd_exec::codex::human::RunnerRequest>> {
        let callbacks = self.callbacks.lock().map_err(|_| denied())?;
        Ok(callbacks
            .values()
            .next()
            .map(|p| crewd_exec::codex::human::RunnerRequest {
                fingerprint: p.decision.fingerprint(),
                method: "claude/permissionRequest".into(),
                params: p.params.clone(),
            }))
    }
    pub(crate) async fn answer(
        &self,
        fingerprint: &str,
        choice: usize,
    ) -> io::Result<AnswerOutcome> {
        if choice > 1 {
            return Ok(AnswerOutcome::Unsupported);
        }
        let (id, decision) = {
            let callbacks = self.callbacks.lock().map_err(|_| denied())?;
            let Some((id, p)) = callbacks
                .iter()
                .find(|(_, p)| p.decision.fingerprint() == fingerprint)
            else {
                return Ok(AnswerOutcome::Conflict {
                    now: callbacks.values().next().map(|p| p.decision.clone()),
                });
            };
            (id.clone(), p.decision.clone())
        };
        let answer = json!({"behavior":if choice==0 {"allow"}else{"deny"}});
        let operation = self
            .persist_operation(
                ledger::Kind::Permission,
                json!({"kind":"permission","callbackId":id,"decision":answer}),
                true,
            )
            .await?;
        // Permission delivery uses the held native callback, never the prompt queue.
        let (state, runtime) = self.authority().await?;
        let saved = operation.clone();
        let owner = state.clone();
        let session = self.session.clone();
        let token = self.token.clone();
        state
            .db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    recheck(tx, &owner, &session, &token)?;
                    if !ledger::release(tx, &runtime, &saved, &crewd_store::now_ts())? {
                        return Err(crewd_store::StoreError::Invalid(
                            "permission already released".into(),
                        ));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(storage)?
            .map_err(storage)?;
        let (ack, wait) = oneshot::channel();
        {
            let mut callbacks = self.callbacks.lock().map_err(|_| denied())?;
            let p = callbacks.get_mut(&id).ok_or_else(denied)?;
            if p.decision != decision || p.answer.is_some() {
                return Err(denied());
            }
            p.answer = Some(answer);
            p.operation = Some(operation);
            p.ack = Some(ack);
        }
        match tokio::time::timeout(Duration::from_secs(10), wait).await {
            Ok(Ok(Ok(()))) => {}
            _ => {
                self.close().await;
                return Err(denied());
            }
        }
        let _ = self
            .events
            .send(SessionEvent::Decision(DecisionEvent::Cleared));
        Ok(AnswerOutcome::Answered {
            cleared: true,
            now: None,
        })
    }
    pub(crate) async fn cancel(&self) -> io::Result<String> {
        let turn = self
            .active_turn
            .lock()
            .map_err(|_| denied())?
            .clone()
            .ok_or_else(denied)?;
        self.submit(
            ledger::Kind::Cancel,
            json!({"kind":"cancel","turnId":turn}),
            true,
        )
        .await
    }
}
async fn read_http(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end;
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(denied());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            end = index + 4;
            break;
        }
        if bytes.len() > 16384 {
            return Err(denied());
        }
    }
    if end > 16384 {
        return Err(denied());
    }
    let header = std::str::from_utf8(&bytes[..end]).map_err(storage)?;
    let mut lines = header.split("\r\n");
    if lines.next() != Some("POST / HTTP/1.1") {
        return Err(denied());
    }
    let mut length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(denied)?;
        if name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("content-encoding")
        {
            return Err(denied());
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(denied());
            }
            length = Some(value.trim().parse::<usize>().map_err(storage)?);
        }
    }
    let length = length.filter(|v| *v <= MAX_HTTP).ok_or_else(denied)?;
    let mut body = bytes[end..].to_vec();
    if body.len() > length {
        return Err(denied());
    }
    while body.len() < length {
        let remaining = (length - body.len()).min(chunk.len());
        let n = stream.read(&mut chunk[..remaining]).await?;
        if n == 0 {
            return Err(denied());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok(body)
}

/// Recheck ephemeral authority and current crew ACL at the FIFO write cut.
fn recheck(
    tx: &rusqlite::Transaction<'_>,
    state: &AppState,
    session: &str,
    token: &str,
) -> crewd_store::Result<()> {
    if !state.admission.contains(session) || !state.tokens.verify(session, token) {
        return Err(crewd_store::StoreError::Invalid(
            "managed authority closed".into(),
        ));
    }
    let row = crewd_store::sessions::get(tx, session)?
        .ok_or_else(|| crewd_store::StoreError::Invalid("managed session missing".into()))?;
    crewd_collab::team_data::authorize(
        tx,
        &crewd_collab::Sender::Session(session.into()),
        &row.crew_id,
        true,
    )
    .map(|_| ())
    .map_err(|e| crewd_store::StoreError::Invalid(e.to_string()))
}
/// These are actual API-return shapes, not a native completion certificate.
fn valid_api_result(kind: ledger::Kind, input: &Value, result: &Value) -> bool {
    let Some(fields) = result.as_object() else {
        return false;
    };
    match kind {
        ledger::Kind::Prompt => {
            if fields.len() == 1 && fields.get("drop").is_some_and(Value::is_string) {
                return true;
            }
            fields.contains_key("text")
                && fields
                    .keys()
                    .all(|k| matches!(k.as_str(), "text" | "context" | "origin"))
                && fields["text"].is_string()
                && fields.get("context").is_none_or(|v| {
                    v.as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
                })
                && fields.get("origin").is_none_or(Value::is_object)
        }
        ledger::Kind::Cancel => {
            fields.len() == 2
                && fields.get("cancelAcknowledged") == Some(&Value::Bool(true))
                && fields.get("turnId") == input.get("turnId")
                && fields.get("turnId").is_some_and(Value::is_string)
        }
        // A held continuation uses its own once-only response-write path.
        ledger::Kind::Permission => false,
    }
}
