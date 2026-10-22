//! The existing Claude PTY entry with Core-owned managed input. Hooks attest a
//! prompt UUID and response waiting; this is not ACP or a native turn lifecycle.
use super::exec::{AnswerSteps, BoxFut, Context, ExecHandle, ExecSendError};
use crate::{native_helpers::NativeFactory, terminal_runtime::TerminalRuntime};
use crewd_exec::{
    delivery::Subject,
    driver::{
        Closed, DeliveryBinding, DeliverySuccessObserver, SendError, SessionHandle, SessionStatus,
    },
    health::{Health, HealthKind},
    registry,
    session::{AnswerOutcome, DeliveryOutcome, SessionEvent},
};
use crewd_proto::agent::{HookKind, HookResult};
use crewd_store::{Db, native_execution as n};
use serde_json::{Value, json};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{mpsc, oneshot};

fn refused() -> SendError {
    SendError::BindingRejected
}
fn unknown_health() -> Health {
    Health::new(
        HealthKind::TurnFailed,
        "Claude managed request outcome Unknown; original input is retained and will not be sent again. Stop this run before explicit recovery.",
    )
}
fn manual_health() -> Health {
    Health::new(
        HealthKind::TurnFailed,
        "Human manual custody revoked managed automatic authority; original managed input is retained and will not replay",
    )
}
fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|id| !id.is_nil() && id.to_string() == s)
}
/// Exact supported hook contract. Never infer old/new versions compatible.
pub(crate) fn supported_version(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|s| s.trim() == "2.1.301 (Claude Code)")
}
pub(crate) async fn supported_program(
    program: &std::path::Path,
    child_path: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(identity) = crate::build_info::ExeIdentity::capture(program) else {
        return false;
    };
    let mut command = tokio::process::Command::new(program);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command
        .env_clear()
        .envs(crewd_exec::env::build_env_from_process(child_path, env));
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        let mut out = child.stdout.take().ok_or(())?.take(257);
        out.read_to_end(&mut bytes).await.map_err(|_| ())?;
        if bytes.len() > 256 {
            return Err(());
        }
        let status = child.wait().await.map_err(|_| ())?;
        Ok::<_, ()>(status.success() && supported_version(&bytes))
    })
    .await;
    let qualified = matches!(outcome, Ok(Ok(true))) && identity.still_on_disk();
    if !qualified {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    qualified
}
fn envelope(request: &str, text: &str) -> Result<String, SendError> {
    if text.trim().is_empty() {
        return Err(SendError::EmptyText);
    }
    let value = format!(
        "<pendingcrew-managed-request id=\"{request}\">\n{text}\n</pendingcrew-managed-request id=\"{request}\">"
    );
    if value.len() > 1_048_576 {
        return Err(refused());
    }
    Ok(value)
}
struct Submission {
    request: String,
    external: u64,
    subject: Subject,
    body: String,
    local: Option<u64>,
    receipt: Option<n::Receipt>,
    prompt: Option<String>,
    stopped: bool,
    last_response: String,
    delivered: bool,
    unobserved_delivery: Option<std::time::Instant>,
}
enum Command {
    Submit(Submission),
    Hook(HookKind, Value, oneshot::Sender<HookResult>),
    Driver(SessionEvent),
    Interrupt,
    Stop,
    Disconnected,
}
pub(crate) struct ManagedHandle {
    driver: SessionHandle,
    queue: mpsc::UnboundedSender<Command>,
    next: AtomicU64,
    busy: Arc<AtomicBool>,
    blocked: Arc<AtomicBool>,
    working: Arc<AtomicBool>,
    manual: Arc<AtomicBool>,
}
impl ManagedHandle {
    pub(crate) async fn create(
        factory: &Arc<NativeFactory>,
        session: String,
        driver: SessionHandle,
        mut original: mpsc::UnboundedReceiver<SessionEvent>,
        initial: Option<String>,
        observer: Option<Arc<dyn DeliverySuccessObserver>>,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<SessionEvent>), String> {
        let terminal = factory
            .terminal_for(&session, Some(driver.clone()))
            .await
            .map_err(|e| e.to_string())?;
        let binding = terminal.binding().map_err(|e| e.to_string())?;
        let status = driver
            .status()
            .await
            .map_err(|_| "managed driver closed".to_owned())?;
        let registration = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: session.clone(),
            generation: binding.generation.clone(),
            epoch: binding.epoch,
            native_pid: binding.native.pid,
            native_start: binding.native.start_time,
            native_boot: binding.native.boot_id.clone(),
        };
        let registered = registration.clone();
        let seal = factory
            .db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    if let Some(old) = n::current_runtime(tx, &registered.session)? {
                        let gone = match registry::probe(old.native_pid) {
                            registry::ProbeResult::Missing => true,
                            registry::ProbeResult::Found(actual) => {
                                actual.start_time != old.native_start
                                    || actual.boot_id != old.native_boot
                            }
                            registry::ProbeResult::Unreadable(_) => false,
                        };
                        if !gone {
                            return Err(crewd_store::StoreError::Invalid(
                                "previous managed birth live or unknown".into(),
                            ));
                        }
                        n::retire_verified_native(tx, &old.id, &crewd_store::now_ts())?;
                    }
                    n::register_managed(tx, &registered)
                })
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        terminal.binding().map_err(|e| e.to_string())?;
        let (ready, ready_rx) = oneshot::channel();
        let (queue, rx) = mpsc::unbounded_channel();
        let (events, output) = mpsc::unbounded_channel();
        let handle = Arc::new(Self {
            driver: driver.clone(),
            queue: queue.clone(),
            next: AtomicU64::new(1),
            busy: Arc::new(AtomicBool::new(false)),
            blocked: Arc::new(AtomicBool::new(false)),
            working: Arc::new(AtomicBool::new(false)),
            manual: Arc::new(AtomicBool::new(false)),
        });
        factory
            .managed
            .lock()
            .map_err(|_| "managed map poisoned")?
            .insert(session, Arc::downgrade(&handle));
        let state = Actor {
            driver: driver.clone(),
            terminal,
            db: factory.db.clone(),
            seal: Arc::new(seal),
            agent_session: status.agent_session_id,
            pending: None,
            last_waiting: None,
            events,
            observer,
            ready: Some(ready),
            busy: handle.busy.clone(),
            blocked: handle.blocked.clone(),
            working: handle.working.clone(),
            manual: handle.manual.clone(),
        };
        tokio::spawn(state.run(rx));
        tokio::spawn(async move {
            while let Some(event) = original.recv().await {
                if queue.send(Command::Driver(event)).is_err() {
                    return;
                }
            }
            let _ = queue.send(Command::Disconnected);
        });
        tokio::time::timeout(std::time::Duration::from_secs(8), ready_rx)
            .await
            .map_err(|_| "managed SessionStart guard installation not witnessed")?
            .map_err(|_| "managed native exited before guard readiness")?;
        if let Some(text) = initial.filter(|s| !s.trim().is_empty()) {
            handle
                .enqueue(text, 0, Subject::Brief, None)
                .map_err(|e| e.to_string())?;
        }
        Ok((handle, output))
    }
    fn enqueue(
        &self,
        text: String,
        id: u64,
        subject: Subject,
        binding: Option<&dyn DeliveryBinding>,
    ) -> Result<u64, SendError> {
        if self.blocked.load(Ordering::Acquire) || self.busy.swap(true, Ordering::AcqRel) {
            return Err(refused());
        }
        let request = uuid::Uuid::new_v4().to_string();
        let body = match envelope(&request, &text) {
            Ok(v) => v,
            Err(e) => {
                self.busy.store(false, Ordering::Release);
                return Err(e);
            }
        };
        if let Some(binding) = binding {
            if let Err(e) = binding.register(id) {
                self.busy.store(false, Ordering::Release);
                return Err(e);
            }
        }
        self.working.store(true, Ordering::Release);
        if self
            .queue
            .send(Command::Submit(Submission {
                request,
                external: id,
                subject,
                body,
                local: None,
                receipt: None,
                prompt: None,
                stopped: false,
                last_response: String::new(),
                delivered: false,
                unobserved_delivery: None,
            }))
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            if let Some(binding) = binding {
                binding.cancelled(id);
            }
            return Err(SendError::Closed);
        }
        Ok(id)
    }
    pub(crate) async fn hook(&self, kind: HookKind, payload: Value) -> HookResult {
        let (tx, rx) = oneshot::channel();
        if self.queue.send(Command::Hook(kind, payload, tx)).is_err() {
            return denied_hook();
        }
        rx.await.unwrap_or_else(|_| denied_hook())
    }
}
fn denied_hook() -> HookResult {
    HookResult{stdout:Some(json!({"decision":"block","reason":"PendingCrew managed input has no matching current durable authorization"}).to_string())}
}
struct Actor {
    driver: SessionHandle,
    terminal: TerminalRuntime,
    db: Db,
    seal: Arc<n::ManagedRuntime>,
    agent_session: String,
    pending: Option<Submission>,
    last_waiting: Option<String>,
    events: mpsc::UnboundedSender<SessionEvent>,
    observer: Option<Arc<dyn DeliverySuccessObserver>>,
    ready: Option<oneshot::Sender<()>>,
    busy: Arc<AtomicBool>,
    blocked: Arc<AtomicBool>,
    working: Arc<AtomicBool>,
    manual: Arc<AtomicBool>,
}
impl Actor {
    async fn invalidate(&mut self) {
        self.blocked.store(true, Ordering::Release);
        let _ = self.driver.hold_automatic().await;
        let runtime = self.seal.runtime().id.clone();
        if self.terminal.manual_hook_allowed().await {
            self.manual.store(true, Ordering::Release);
            let _ = self
                .events
                .send(SessionEvent::Health(Some(manual_health())));
            return;
        }
        let _ = self
            .db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    if n::current_runtime(tx, &store_session(tx, &runtime)?)?
                        .is_some_and(|r| r.id == runtime)
                    {
                        n::invalidate_runtime_unknown(tx, &runtime)?;
                    }
                    Ok(())
                })
            })
            .await;
        if self.pending.is_some() || self.last_waiting.is_some() {
            let _ = self
                .events
                .send(SessionEvent::Health(Some(unknown_health())));
        }
    }
    async fn retract_waiting(&self) {
        if let Some(prompt) = &self.last_waiting {
            let seal = self.seal.clone();
            let witness = prompt.clone();
            let _ = self
                .db
                .call_result(move |store| {
                    store.with_tx(|tx| {
                        n::managed_response_resumed(tx, &seal, &witness, &crewd_store::now_ts())
                    })
                })
                .await;
        }
    }
    async fn current(&self) -> bool {
        if self.blocked.load(Ordering::Acquire)
            || self.terminal.binding().is_err()
            || !crate::build_info::exe().is_some_and(crate::build_info::ExeIdentity::still_on_disk)
        {
            return false;
        }
        self.driver
            .status()
            .await
            .is_ok_and(|s| s.agent_session_id == self.agent_session)
    }
    async fn submit(&mut self, mut input: Submission) {
        if self.pending.is_some() || !self.current().await {
            self.pending = Some(input);
            self.invalidate().await;
            return;
        }
        let runtime = self.seal.runtime().id.clone();
        let request = input.request.clone();
        let body = input.body.as_bytes().to_vec();
        let prepared = self
            .db
            .call_result(move |store| {
                store
                    .with_tx(|tx| n::prepare(tx, &runtime, &request, &body, &crewd_store::now_ts()))
            })
            .await;
        match prepared {
            Ok(Ok((receipt, false))) => input.receipt = Some(receipt),
            _ => {
                self.pending = Some(input);
                self.invalidate().await;
                return;
            }
        }
        self.pending = Some(input);
        if !self.current().await {
            self.invalidate().await;
            return;
        }
        let body = self.pending.as_ref().unwrap().body.clone();
        match self.driver.send(body) {
            Ok(local) => self.pending.as_mut().unwrap().local = Some(local),
            Err(_) => self.invalidate().await,
        }
    }
    async fn hook(&mut self, kind: HookKind, payload: Value) -> HookResult {
        if self.terminal.manual_hook_allowed().await {
            self.blocked.store(true, Ordering::Release);
            if !self.manual.swap(true, Ordering::AcqRel) {
                let _ = self
                    .events
                    .send(SessionEvent::Health(Some(manual_health())));
            }
            return if kind == HookKind::UserPromptSubmit {
                HookResult {
                    stdout: Some(json!({"continue":true}).to_string()),
                }
            } else {
                HookResult { stdout: None }
            };
        }
        if !self.current().await {
            return denied_hook();
        }
        let event = payload["hook_event_name"].as_str().unwrap_or("");
        let valid = match kind {
            HookKind::PostToolUse => matches!(event, "PostToolUse" | "PostToolUseFailure"),
            HookKind::Stop => event == "Stop",
            HookKind::SessionStart => event == "SessionStart",
            HookKind::UserPromptSubmit => event == "UserPromptSubmit",
            HookKind::StopFailure => event == "StopFailure",
            HookKind::SessionEnd => event == "SessionEnd",
            HookKind::Notification => event == "Notification",
            HookKind::ManagedActivity => event == "PreToolUse",
        };
        if !valid {
            if self.pending.is_some() {
                self.invalidate().await;
            }
            return denied_hook();
        }
        if payload.get("agent_id").is_some() {
            return if kind == HookKind::UserPromptSubmit {
                if self.pending.is_some() {
                    self.invalidate().await;
                }
                denied_hook()
            } else {
                HookResult { stdout: None }
            };
        }
        if payload["session_id"].as_str() != Some(self.agent_session.as_str()) {
            if self.pending.is_some() {
                self.invalidate().await;
            }
            return denied_hook();
        }
        if kind == HookKind::SessionStart
            && self.ready.is_some()
            && matches!(payload["source"].as_str(), Some("startup" | "resume"))
        {
            let _ = self.ready.take().unwrap().send(());
            return HookResult { stdout: None };
        }
        let prompt = payload["prompt_id"]
            .as_str()
            .filter(|s| uuid(s))
            .map(str::to_owned);
        if kind == HookKind::UserPromptSubmit {
            let Some(input) = self.pending.as_ref() else {
                return denied_hook();
            };
            if input.prompt.is_some()
                || prompt.is_none()
                || payload["prompt"].as_str() != Some(input.body.as_str())
            {
                self.invalidate().await;
                return denied_hook();
            }
            let input = self.pending.as_ref().unwrap();
            let receipt = input.receipt.as_ref().unwrap();
            let seal = self.seal.clone();
            let request = receipt.request_id.clone();
            let revision = receipt.revision;
            let body = input.body.as_bytes().to_vec();
            let witness = prompt.clone().unwrap();
            let committed = self
                .db
                .call_result(move |store| {
                    store.with_tx(|tx| {
                        n::managed_accept_content(
                            tx,
                            &seal,
                            &request,
                            revision,
                            &witness,
                            &body,
                            &crewd_store::now_ts(),
                        )
                    })
                })
                .await;
            let Ok(Ok(receipt)) = committed else {
                self.invalidate().await;
                return denied_hook();
            };
            if !self.current().await {
                self.invalidate().await;
                return denied_hook();
            }
            let input = self.pending.as_mut().unwrap();
            input.receipt = Some(receipt);
            input.prompt = prompt;
            let _ = self.events.send(SessionEvent::Working(true));
            return HookResult {
                stdout: Some(json!({"continue":true}).to_string()),
            };
        }
        if matches!(
            kind,
            HookKind::StopFailure | HookKind::SessionEnd | HookKind::SessionStart
        ) {
            if self.pending.is_none() {
                self.retract_waiting().await;
            }
            self.invalidate().await;
            return HookResult { stdout: None };
        }
        if self.pending.is_none() {
            if prompt
                .as_ref()
                .is_some_and(|p| Some(p) == self.last_waiting.as_ref())
                && matches!(
                    kind,
                    HookKind::PostToolUse | HookKind::ManagedActivity | HookKind::Stop
                )
            {
                self.retract_waiting().await;
                self.invalidate().await;
            }
            return HookResult { stdout: None };
        }
        if prompt.is_none() || self.pending.as_ref().unwrap().prompt != prompt {
            self.invalidate().await;
            return HookResult { stdout: None };
        }
        if matches!(
            kind,
            HookKind::PostToolUse | HookKind::ManagedActivity | HookKind::Stop
        ) && !self.pending.as_ref().unwrap().delivered
        {
            let input = self.pending.as_mut().unwrap();
            input.delivered = true;
            if let Some(observer) = &self.observer {
                observer.emitted(input.external);
            }
            let _ = self.events.send(SessionEvent::Delivery {
                id: input.external,
                subject: input.subject,
                outcome: DeliveryOutcome::Delivered,
            });
        }
        match kind {
            HookKind::PostToolUse | HookKind::ManagedActivity => {
                self.pending.as_mut().unwrap().stopped = false;
            }
            HookKind::Stop => {
                let input = self.pending.as_mut().unwrap();
                input.stopped = true;
                input.last_response = payload["last_assistant_message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
            }
            HookKind::Notification if payload["notification_type"] == "idle_prompt" => {
                let input = self.pending.as_ref().unwrap();
                if !input.stopped {
                    self.invalidate().await;
                    return HookResult { stdout: None };
                }
                let receipt = input.receipt.as_ref().unwrap();
                let request = receipt.request_id.clone();
                let revision = receipt.revision;
                let prompt = prompt.unwrap();
                let seal = self.seal.clone();
                let witness = prompt.clone();
                let result = self
                    .db
                    .call_result(move |store| {
                        store.with_tx(|tx| {
                            n::managed_response_waiting(
                                tx,
                                &seal,
                                &request,
                                revision,
                                &witness,
                                &crewd_store::now_ts(),
                            )
                        })
                    })
                    .await;
                if !matches!(result, Ok(Ok(_))) || !self.current().await {
                    self.invalidate().await;
                    return HookResult { stdout: None };
                }
                self.last_waiting = Some(prompt);
                self.pending = None;
                self.busy.store(false, Ordering::Release);
                self.working.store(false, Ordering::Release);
                let _ = self.events.send(SessionEvent::Working(false));
            }
            _ => {}
        }
        HookResult { stdout: None }
    }
    async fn run(mut self, mut queue: mpsc::UnboundedReceiver<Command>) {
        let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            let command = tokio::select! {
                value=queue.recv()=>{let Some(value)=value else{break;};value},
                _=watchdog.tick()=>{
                    if self.pending.as_ref().is_some_and(|p|p.prompt.is_none()&&p.unobserved_delivery.is_some_and(|t|t.elapsed()>std::time::Duration::from_secs(20))) && !self.blocked.load(Ordering::Acquire) {self.invalidate().await;}
                    continue;
                },
            };
            match command {
                Command::Submit(input) => self.submit(input).await,
                Command::Hook(kind, payload, reply) => {
                    let result = self.hook(kind, payload).await;
                    let _ = reply.send(result);
                }
                Command::Stop => {
                    if self.pending.is_none() {
                        self.last_waiting = None;
                    }
                    self.invalidate().await;
                    let _ = self.driver.stop();
                }
                Command::Interrupt => {
                    if self.pending.is_none() {
                        self.last_waiting = None;
                    }
                    self.invalidate().await;
                    let _ = self.driver.interrupt();
                }
                Command::Disconnected => {
                    self.invalidate().await;
                    break;
                }
                Command::Driver(event) => match event {
                    SessionEvent::Working(value) if self.manual.load(Ordering::Acquire) => {
                        let _ = self.events.send(SessionEvent::Working(value));
                    }
                    SessionEvent::Working(_) | SessionEvent::TurnEnded { .. } => {}
                    SessionEvent::Delivery { id, outcome, .. } => {
                        let matches = self
                            .pending
                            .as_ref()
                            .is_some_and(|p| p.local == Some(id) && p.prompt.is_none());
                        if matches && outcome == DeliveryOutcome::Delivered {
                            self.pending.as_mut().unwrap().unobserved_delivery =
                                Some(std::time::Instant::now());
                        }
                        if matches
                            && matches!(
                                outcome,
                                DeliveryOutcome::Undelivered(_)
                                    | DeliveryOutcome::Unsubmitted(_)
                                    | DeliveryOutcome::Abandoned(_)
                            )
                        {
                            self.invalidate().await;
                        }
                    }
                    SessionEvent::Spawned {
                        ref agent_session_id,
                        ..
                    } if agent_session_id != &self.agent_session => {
                        self.invalidate().await;
                    }
                    SessionEvent::ResumeRefused { .. } => {
                        self.invalidate().await;
                        let _ = self.events.send(event);
                    }
                    SessionEvent::Exited(_) => {
                        if self.pending.is_some() {
                            self.invalidate().await;
                        } else {
                            self.blocked.store(true, Ordering::Release);
                            let runtime = self.seal.runtime().id.clone();
                            let session = self.seal.runtime().session.clone();
                            let _ = self
                                .db
                                .call_result(move |store| {
                                    store.with_tx(|tx| {
                                        if n::current_runtime(tx, &session)?
                                            .is_some_and(|r| r.id == runtime)
                                        {
                                            n::invalidate_runtime_unknown(tx, &runtime)?;
                                        }
                                        Ok(())
                                    })
                                })
                                .await;
                        }
                        let _ = self.events.send(event);
                        break;
                    }
                    other => {
                        let _ = self.events.send(other);
                    }
                },
            }
        }
    }
}
fn store_session(tx: &rusqlite::Transaction<'_>, runtime: &str) -> crewd_store::Result<String> {
    Ok(tx.query_row(
        "SELECT session_id FROM native_execution_runtimes WHERE runtime_id=?1",
        [runtime],
        |r| r.get(0),
    )?)
}
impl Drop for ManagedHandle {
    fn drop(&mut self) {
        self.blocked.store(true, Ordering::Release);
        let _ = self.queue.send(Command::Stop);
    }
}
impl ExecHandle for ManagedHandle {
    fn send(&self, text: String, context: &Context) -> Result<u64, ExecSendError> {
        super::exec::check_context(false, context)?;
        let id = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| ExecSendError::Send(SendError::DeliveryIdExhausted))?;
        self.enqueue(text, id, Subject::Steer, None)
            .map_err(Into::into)
    }
    fn send_bound(
        &self,
        text: String,
        context: &Context,
        binding: &dyn DeliveryBinding,
    ) -> Result<u64, ExecSendError> {
        super::exec::check_context(false, context)?;
        let id = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| ExecSendError::Send(SendError::DeliveryIdExhausted))?;
        self.enqueue(text, id, Subject::Steer, Some(binding))
            .map_err(Into::into)
    }
    fn terminal_driver(&self) -> Option<SessionHandle> {
        Some(self.driver.clone())
    }
    fn raw(&self, _: Vec<u8>) -> Result<(), Closed> {
        Err(Closed)
    }
    fn interrupt(&self) -> Result<(), Closed> {
        self.blocked.store(true, Ordering::Release);
        self.queue.send(Command::Interrupt).map_err(|_| Closed)
    }
    fn answer(
        &self,
        expect: String,
        steps: AnswerSteps,
    ) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
        Box::pin(
            self.driver.answer(
                expect,
                steps
                    .into_iter()
                    .map(|(bytes, selected_before)| crewd_exec::session::AnswerStep {
                        bytes,
                        selected_before,
                    })
                    .collect(),
            ),
        )
    }
    fn stop(&self) -> Result<(), Closed> {
        self.blocked.store(true, Ordering::Release);
        self.queue.send(Command::Stop).map_err(|_| Closed)
    }
    fn clear_quota_health(&self) -> Result<(), Closed> {
        self.driver.clear_quota_health()
    }
    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(self.driver.screen_text(max_lines))
    }
    fn snapshot(&self) -> BoxFut<'_, Result<Option<crewd_term::Snapshot>, Closed>> {
        Box::pin(self.driver.snapshot())
    }
    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(async move {
            let mut status = self.driver.status().await?;
            if !self.manual.load(Ordering::Acquire) {
                status.working = self.working.load(Ordering::Acquire);
            }
            if self.blocked.load(Ordering::Acquire) {
                status.health = Some(if self.manual.load(Ordering::Acquire) {
                    manual_health()
                } else {
                    unknown_health()
                });
            }
            Ok(status)
        })
    }
}
pub(crate) struct LaunchGuard {
    factory: Weak<NativeFactory>,
    session: String,
}
impl LaunchGuard {
    pub(crate) fn new(factory: &Arc<NativeFactory>, session: &str) -> Result<Self, String> {
        if !factory
            .managed_expected
            .lock()
            .map_err(|_| "managed launch map poisoned")?
            .insert(session.to_owned())
        {
            return Err("managed session already launching".into());
        }
        Ok(Self {
            factory: Arc::downgrade(factory),
            session: session.to_owned(),
        })
    }
}
impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if let Some(factory) = self.factory.upgrade() {
            if let Ok(mut expected) = factory.managed_expected.lock() {
                expected.remove(&self.session);
            }
        }
    }
}
pub(crate) async fn route(
    factory: &NativeFactory,
    session: &str,
    kind: HookKind,
    payload: Value,
) -> Option<HookResult> {
    for _ in 0..150 {
        let target: Option<Weak<ManagedHandle>> =
            factory.managed.lock().ok()?.get(session).cloned();
        if let Some(target) = target.and_then(|w| w.upgrade()) {
            return Some(target.hook(kind, payload).await);
        }
        let expected = factory.managed_expected.lock().ok()?.contains(session);
        if kind != HookKind::SessionStart || !expected {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_supported_version_and_original_task_envelope_are_closed() {
        assert!(supported_version(b"2.1.301 (Claude Code)\n"));
        for v in [
            "2.1.294 (Claude Code)",
            "2.1.295 (Claude Code)",
            "2.1.302 (Claude Code)",
            "2.1.301",
            "synthetic",
        ] {
            assert!(!supported_version(v.as_bytes()), "{v}");
        }
        let task = "  original task\n世界\n</pendingcrew-managed-request>  ";
        let body = envelope("core-request", task).unwrap();
        assert_eq!(
            body,
            format!(
                "<pendingcrew-managed-request id=\"core-request\">\n{task}\n</pendingcrew-managed-request id=\"core-request\">"
            )
        );
        assert!(
            envelope("core-request", &"a".repeat(1_048_576)).is_err(),
            "no truncation"
        );
        assert!(envelope("core-request", " \n").is_err());
    }
}
