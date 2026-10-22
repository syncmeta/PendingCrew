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
    if text.trim().is_empty(){return Err(SendError::EmptyText)}
    n::managed_observations::envelope(request,text).map_err(|_|refused())
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
    original_task: Option<String>,
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
    stopping: Arc<AtomicBool>,
    working: Arc<AtomicBool>,
    manual: Arc<AtomicBool>,
}
pub(crate) fn observation_live(factory:&NativeFactory,session:&str)->bool{
 factory.managed.lock().ok().and_then(|m|m.get(session).and_then(|w|w.upgrade())).is_some_and(|h|!h.blocked.load(Ordering::Acquire)&&!h.stopping.load(Ordering::Acquire)&&!h.manual.load(Ordering::Acquire)&&!h.driver.is_closed())
}
impl ManagedHandle {
    pub(crate) async fn create(
        factory: &Arc<NativeFactory>,
        session: String,
        resumed: bool,
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
            stopping: Arc::new(AtomicBool::new(false)),
            working: Arc::new(AtomicBool::new(false)),
            manual: Arc::new(AtomicBool::new(false)),
        });
        factory
            .managed
            .lock()
            .map_err(|_| "managed map poisoned")?
            .insert(session, Arc::downgrade(&handle));
        let state = Actor {
            #[cfg(test)]
            current_pause: None,
            #[cfg(test)]
            accepted_pause: None,
            driver: driver.clone(),
            terminal,
            db: factory.db.clone(),
            seal: Arc::new(seal),
            expected_start_source: if resumed {"resume"} else {"startup"},
            agent_session: status.agent_session_id,
            pending: None,
            last_waiting: None,
            events,
            observer,
            ready: Some(ready),
            busy: handle.busy.clone(),
            blocked: handle.blocked.clone(),
            stopping: handle.stopping.clone(),
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
                original_task: Some(text),
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
#[cfg(test)]
#[derive(Default)]
struct GuardPause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[cfg(test)]
impl GuardPause {
    async fn pause(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}
struct Actor {
    #[cfg(test)]
    current_pause: Option<Arc<GuardPause>>,
    #[cfg(test)]
    accepted_pause: Option<Arc<GuardPause>>,
    driver: SessionHandle,
    terminal: TerminalRuntime,
    db: Db,
    seal: Arc<n::ManagedRuntime>,
    expected_start_source: &'static str,
    agent_session: String,
    pending: Option<Submission>,
    last_waiting: Option<String>,
    events: mpsc::UnboundedSender<SessionEvent>,
    observer: Option<Arc<dyn DeliverySuccessObserver>>,
    ready: Option<oneshot::Sender<()>>,
    busy: Arc<AtomicBool>,
    blocked: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
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
    fn current_now(&self) -> bool {
        if self.blocked.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            return false;
        }
        let bound = self.terminal.binding().is_ok()
            && crate::build_info::exe().is_some_and(crate::build_info::ExeIdentity::still_on_disk);
        bound && !self.stopping.load(Ordering::Acquire) && !self.blocked.load(Ordering::Acquire)
    }
    async fn current(&self) -> bool {
        if !self.current_now() {
            return false;
        }
        #[cfg(test)]
        if let Some(pause) = &self.current_pause {
            pause.pause().await;
        }
        let same = self
            .driver
            .status()
            .await
            .is_ok_and(|s| s.agent_session_id == self.agent_session);
        same && self.current_now()
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
        let original_task=input.original_task.clone();
        let seal=self.seal.clone();
        let prepared = self
            .db
            .call_result(move |store| {
                store.with_tx(|tx| {
                    let prepared=n::prepare(tx,&runtime,&request,&body,&crewd_store::now_ts())?;
                    if !prepared.1 {n::managed_observations::record_task(tx,&seal,&request,original_task.as_deref())?;}
                    Ok(prepared)
                })
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
            if self.stopping.load(Ordering::Acquire) {
                return denied_hook();
            }
            let old_auto = kind == HookKind::UserPromptSubmit
                && self
                    .pending
                    .as_ref()
                    .is_some_and(|p| payload["prompt"].as_str() == Some(p.body.as_str()));
            self.blocked.store(true, Ordering::Release);
            if !self.manual.swap(true, Ordering::AcqRel) {
                let _ = self
                    .events
                    .send(SessionEvent::Health(Some(manual_health())));
            }
            return if kind == HookKind::UserPromptSubmit && !old_auto {
                HookResult {
                    stdout: Some(json!({"continue":true}).to_string()),
                }
            } else if kind == HookKind::UserPromptSubmit {
                denied_hook()
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
            && payload["source"].as_str()==Some(self.expected_start_source)
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
            #[cfg(test)]
            if let Some(pause) = &self.accepted_pause {
                pause.pause().await;
            }
            if !self.current().await {
                self.invalidate().await;
                return denied_hook();
            }
            let terminal = self.terminal.clone();
            let permit = terminal.automatic_hook_authority(self.seal.clone()).await;
            if permit.is_none() || !self.current_now() {
                drop(permit);
                self.invalidate().await;
                return denied_hook();
            }
            // No further await: raw claim shares the held lease gate, while
            // Stop/native birth are rechecked at this allow linearization point.
            let input = self.pending.as_mut().unwrap();
            input.receipt = Some(receipt);
            input.prompt = prompt;
            let _ = self.events.send(SessionEvent::Working(true));
            return HookResult {
                stdout: Some(json!({"continue":true}).to_string()),
            };
        }
        // The hook transport and session checks above are insufficient alone:
        // only this current managed input's actual prompt can attest failure.
        if kind == HookKind::StopFailure {
            let qualified = self
                .pending
                .as_ref()
                .filter(|input| prompt.is_some() && input.prompt == prompt)
                .and_then(|input| input.receipt.as_ref())
                .cloned();
            if let Some(original) = qualified {
                let terminal = self.terminal.clone();
                let permit = terminal.automatic_hook_authority(self.seal.clone()).await;
                let allowed = permit.is_some() && self.current_now();
                drop(permit);
                if allowed {
                    self.invalidate().await;
                    let request = original.request_id.clone();
                    let runtime = self.seal.runtime().id.clone();
                    let proof = self
                        .db
                        .call_result(move |store| {
                            let receipt = n::get(store.conn(), &request)?;
                            Ok::<_, crewd_store::StoreError>(receipt.filter(|r| {
                                r.runtime_id == runtime
                                    && r.prompt_id.is_some()
                                    && r.phase == n::Phase::Unknown
                                    && r.turn_id.is_none()
                            }))
                        })
                        .await;
                    if let Ok(Ok(Some(receipt))) = proof {
                        self.working.store(false, Ordering::Release);
                        let _ = self.events.send(SessionEvent::ManagedResponseFailed {
                            runtime_id: receipt.runtime_id,
                            request_id: receipt.request_id,
                            prompt_id: receipt.prompt_id.unwrap(),
                            receipt_revision: receipt.revision,
                        });
                    }
                    return HookResult { stdout: None };
                }
            }
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
                if kind==HookKind::Stop {
                    let seal=self.seal.clone();let prompt=prompt.clone().unwrap();
                    let text=payload["last_assistant_message"].as_str().map(str::to_owned);
                    let _=self.db.call_result(move|s|s.with_tx(|tx| {
                        use rusqlite::OptionalExtension;
                        let request:Option<String>=tx.query_row("SELECT request_id FROM native_execution_receipts WHERE runtime_id=?1 AND prompt_id=?2 AND turn_id IS NULL",rusqlite::params![seal.runtime().id,prompt],|r|r.get(0)).optional()?;
                        if let Some(request)=request {n::managed_observations::observe_stop(tx,&seal,&request,&prompt,text.as_deref(),&crewd_store::now_ts())?;}
                        n::managed_response_resumed(tx,&seal,&prompt,&crewd_store::now_ts())?;
                        Ok(())
                    })).await;
                } else {self.retract_waiting().await;}
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
                let request=self.pending.as_ref().unwrap().request.clone();
                let prompt=prompt.as_ref().unwrap().clone();
                let text=payload["last_assistant_message"].as_str().map(str::to_owned);
                let seal=self.seal.clone();
                let saved=self.db.call_result(move |s|s.with_tx(|tx|n::managed_observations::observe_stop(tx,&seal,&request,&prompt,text.as_deref(),&crewd_store::now_ts()))).await;
                if !matches!(saved,Ok(Ok(())))||!self.current().await {self.invalidate().await;return HookResult{stdout:None}}
                self.pending.as_mut().unwrap().stopped=true;
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
                            let receipt=n::managed_response_waiting(tx,&seal,&request,revision,&witness,&crewd_store::now_ts())?;
                            n::managed_observations::mark_waiting(tx,&seal,&request,&witness)?;
                            Ok(receipt)
                        })
                    })
                    .await;
                if !matches!(result, Ok(Ok(_))) || !self.current().await {
                    self.invalidate().await;
                    return HookResult { stdout: None };
                }
                let terminal = self.terminal.clone();
                let permit = terminal.automatic_hook_authority(self.seal.clone()).await;
                if permit.is_none() || !self.current_now() {
                    drop(permit);
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
        self.stopping.store(true, Ordering::Release);
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
        self.stopping.store(true, Ordering::Release);
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
        self.stopping.store(true, Ordering::Release);
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
    struct RaceRun {
        actor: Actor,
        terminal: Arc<crate::legacy_terminal::LegacyTerminal>,
        fixture: crate::agent::testkit::Fixture,
        _root: tempfile::TempDir,
        registry: Arc<registry::ProcessRegistry>,
        _events: mpsc::UnboundedReceiver<SessionEvent>,
    }
    impl Drop for RaceRun {
        fn drop(&mut self) {
            let _ = self.actor.driver.stop();
            let _ = registry::reap_orphans(&self.registry);
        }
    }
    async fn race_run() -> RaceRun {
        use crewd_exec::{
            driver::{ClaudeStart, DriverConfig, start_claude},
            launch::ClaudeSpec,
            resume::MemoryLedger,
        };
        let fixture = crate::agent::testkit::Fixture::new().await;
        let root = tempfile::tempdir().unwrap();
        let registry =
            Arc::new(registry::ProcessRegistry::load(&root.path().join("registry.json")).unwrap());
        let mut config = DriverConfig::new(root.path().join("runtime"));
        config.registry = Some(registry.clone());
        config.terminal_observer = Some(Arc::new(
            crewd_exec::acp::terminal::TerminalTap::new(1, 64).unwrap(),
        ));
        config.stop_grace = std::time::Duration::from_millis(100);
        let fake = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fake_claude");
        let (driver, events) = start_claude(
            ClaudeStart {
                session_id: fixture.worker.clone(),
                program: fake,
                child_path: "/usr/bin:/bin".into(),
                spec: ClaudeSpec {
                    workdir: root.path().to_owned(),
                    ..Default::default()
                },
                ledger: Arc::new(MemoryLedger::default()),
            },
            config,
        )
        .await
        .unwrap();
        let terminal = crate::legacy_terminal::LegacyTerminal::adopt(
            fixture.worker.clone(),
            driver.clone(),
            fixture.state.db.clone(),
            &registry,
        )
        .await
        .unwrap();
        let binding = terminal.binding().unwrap();
        let status = driver.status().await.unwrap();
        let runtime = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: fixture.worker.clone(),
            generation: binding.generation.clone(),
            epoch: binding.epoch,
            native_pid: binding.native.pid,
            native_start: binding.native.start_time,
            native_boot: binding.native.boot_id,
        };
        let seal = fixture
            .state
            .db
            .call_result(move |store| store.with_tx(|tx| n::register_managed(tx, &runtime)))
            .await
            .unwrap()
            .unwrap();
        let (events_out, _) = mpsc::unbounded_channel();
        let actor = Actor {
            current_pause: None,
            accepted_pause: None,
            driver,
            terminal: TerminalRuntime::Legacy(terminal.clone()),
            db: fixture.state.db.clone(),
            seal: Arc::new(seal),
            expected_start_source: "startup",
            agent_session: status.agent_session_id,
            pending: None,
            last_waiting: None,
            events: events_out,
            observer: None,
            ready: None,
            busy: Arc::new(AtomicBool::new(false)),
            blocked: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            working: Arc::new(AtomicBool::new(false)),
            manual: Arc::new(AtomicBool::new(false)),
        };
        RaceRun {
            actor,
            terminal,
            fixture,
            _root: root,
            registry,
            _events: events,
        }
    }
    fn human_actor(registry: Arc<registry::ProcessRegistry>) -> crate::public_commands::Actor {
        let policy = Arc::new(crate::peer::PeerPolicy::new(Some(registry)).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        crate::public_commands::Actor::Local(policy.accept(&server).unwrap())
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notification_managed_failure_requires_actual_current_request_prompt_and_session() {
        for mode in [
            "matched",
            "foreign-prompt",
            "foreign-session",
            "subagent",
            "stopped",
        ] {
            let mut run = race_run().await;
            let request = uuid::Uuid::new_v4().to_string();
            let prompt = uuid::Uuid::new_v4().to_string();
            let body = envelope(&request, "SYNTHETIC_NOTIFICATION_FAILURE").unwrap();
            let seal = run.actor.seal.clone();
            let rid = request.clone();
            let pid = prompt.clone();
            let original = body.as_bytes().to_vec();
            let receipt = run
                .actor
                .db
                .call_result(move |s| {
                    s.with_tx(|tx| {
                        n::prepare(tx, &seal.runtime().id, &rid, &original, "t")?;
                        n::managed_accept_content(tx, &seal, &rid, 0, &pid, &original, "t")
                    })
                })
                .await
                .unwrap()
                .unwrap();
            run.actor.pending = Some(Submission {
                request: request.clone(),
                external: 1,
                subject: Subject::Brief,
                body,
                local: None,
                receipt: Some(receipt),
                prompt: Some(prompt.clone()),
                stopped: false,
                original_task: None,
                delivered: false,
                unobserved_delivery: None,
            });
            let (events, mut observed) = mpsc::unbounded_channel();
            run.actor.events = events;
            run.actor.working.store(true, Ordering::Release);
            let mut payload = json!({"session_id":run.actor.agent_session,"hook_event_name":"StopFailure","prompt_id":prompt});
            match mode {
                "foreign-prompt" => payload["prompt_id"] = json!(uuid::Uuid::new_v4().to_string()),
                "foreign-session" => payload["session_id"] = json!("other-session"),
                "subagent" => payload["agent_id"] = json!("subagent"),
                "stopped" => run.actor.stopping.store(true, Ordering::Release),
                _ => {}
            }
            run.actor.hook(HookKind::StopFailure, payload).await;
            let mut failures = Vec::new();
            while let Ok(event) = observed.try_recv() {
                if let SessionEvent::ManagedResponseFailed {
                    runtime_id,
                    request_id,
                    prompt_id,
                    receipt_revision,
                } = event
                {
                    failures.push((runtime_id, request_id, prompt_id, receipt_revision));
                }
            }
            if mode == "matched" {
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].0, run.actor.seal.runtime().id);
                assert_eq!(failures[0].1, request);
                assert_eq!(failures[0].2, prompt);
                assert!(failures[0].3 > 1);
                assert!(!run.actor.working.load(Ordering::Acquire));
                assert!(run.actor.blocked.load(Ordering::Acquire));
            } else {
                assert!(
                    failures.is_empty(),
                    "{mode} cannot invent a provider failure"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owned_claude_stop_text_is_durable_before_idle_without_claiming_completion() {
        let mut run = race_run().await;
        let request = uuid::Uuid::new_v4().to_string();
        let prompt = uuid::Uuid::new_v4().to_string();
        let body = envelope(&request, "Specific owned task, exact text").unwrap();
        let seal = run.actor.seal.clone(); let rid = request.clone();
        let pid = prompt.clone(); let original = body.as_bytes().to_vec();
        let receipt = run.actor.db.call_result(move |s| s.with_tx(|tx| {
            n::prepare(tx, &seal.runtime().id, &rid, &original, "t")?;
            n::managed_accept_content(tx, &seal, &rid, 0, &pid, &original, "t")
        })).await.unwrap().unwrap();
        run.actor.pending = Some(Submission {
            request:request.clone(), external:1, subject:Subject::Brief, body,
            local:None, receipt:Some(receipt), prompt:Some(prompt.clone()), stopped:false,
            original_task:None, delivered:false, unobserved_delivery:None,
        });
        let text = "  Exactly observed reply\n世界\nnot business Done  ";
        assert!(run.actor.current().await, "original sealed runtime must be current before Stop");
        run.actor.hook(HookKind::Stop, json!({"session_id":run.actor.agent_session,
            "hook_event_name":"Stop","prompt_id":prompt,"last_assistant_message":text})).await;
        assert!(run.actor.pending.as_ref().unwrap().stopped, "actual matched Stop must have been accepted by the original actor");
        let key = format!("claude.managed.observations.v1/{request}");
        let observed = run.actor.db.call_result(move |s| crewd_store::meta_get(s.conn(), &key)).await.unwrap().unwrap();
        let value:Value = serde_json::from_str(&observed.expect("same-seal Stop text must survive actor drop before idle")).unwrap();
        assert_eq!(value["requestId"],request); assert_eq!(value["runtimeId"],run.actor.seal.runtime().id);
        assert_eq!(value["observations"][0]["promptId"],prompt);
        assert_eq!(value["observations"][0]["text"],text);
        let rid=request.clone(); let receipt=run.actor.db.call_result(move |s|n::get(s.conn(), &rid)).await.unwrap().unwrap().unwrap();
        assert_ne!(receipt.phase,n::Phase::Completed,"Stop alone is provisional");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn managed_guard_race_stop_across_real_driver_status_cannot_remain_current() {
        let mut run = race_run().await;
        let pause = Arc::new(GuardPause::default());
        run.actor.current_pause = Some(pause.clone());
        let (queue, _pending_commands) = mpsc::unbounded_channel();
        let handle = ManagedHandle {
            driver: run.actor.driver.clone(),
            queue,
            next: AtomicU64::new(1),
            busy: run.actor.busy.clone(),
            blocked: run.actor.blocked.clone(),
            stopping: run.actor.stopping.clone(),
            working: run.actor.working.clone(),
            manual: run.actor.manual.clone(),
        };
        let stop = async {
            pause.entered.notified().await;
            handle.stop().unwrap();
            pause.release.notify_one();
        };
        let (current, ()) = tokio::join!(run.actor.current(), stop);
        assert!(
            !current,
            "Stop linearized during current() await must deny the old guard"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn managed_guard_race_raw_claim_after_durable_accept_denies_old_auto_allow() {
        let mut run = race_run().await;
        let request = uuid::Uuid::new_v4().to_string();
        let body = envelope(&request, "SYNTHETIC_RACE_ORIGINAL").unwrap();
        let runtime = run.actor.seal.runtime().id.clone();
        let rid = request.clone();
        let original = body.as_bytes().to_vec();
        let receipt = run
            .actor
            .db
            .call_result(move |store| {
                store.with_tx(|tx| n::prepare(tx, &runtime, &rid, &original, "now"))
            })
            .await
            .unwrap()
            .unwrap()
            .0;
        run.actor.pending = Some(Submission {
            request,
            external: 1,
            subject: Subject::Brief,
            body: body.clone(),
            local: None,
            receipt: Some(receipt),
            prompt: None,
            stopped: false,
            original_task: None,
            delivered: false,
            unobserved_delivery: None,
        });
        let pause = Arc::new(GuardPause::default());
        run.actor.accepted_pause = Some(pause.clone());
        let human = human_actor(run.registry.clone());
        let terminal = run.terminal.clone();
        let owner = run.fixture.state.clone();
        let claim = async {
            pause.entered.notified().await;
            terminal.claim(&human, &owner, None).await.unwrap();
            pause.release.notify_one();
        };
        let payload = json!({"session_id":run.actor.agent_session,"hook_event_name":"UserPromptSubmit","prompt_id":uuid::Uuid::new_v4().to_string(),"prompt":body});
        let (result, ()) = tokio::join!(
            run.actor.hook(HookKind::UserPromptSubmit, payload.clone()),
            claim
        );
        assert_ne!(
            result.stdout.as_deref(),
            Some("{\"continue\":true}"),
            "durable raw claim must close old automatic allow"
        );
        let again = run.actor.hook(HookKind::UserPromptSubmit, payload).await;
        assert_ne!(
            again.stdout.as_deref(),
            Some("{\"continue\":true}"),
            "manual custody cannot reauthorize the retained old automatic envelope"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn managed_guard_race_manual_expiry_during_real_db_queue_denies_allow() {
        let run = race_run().await;
        let human = human_actor(run.registry.clone());
        run.terminal
            .claim(&human, &run.fixture.state, None)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        run.terminal.test_set_lease_deadline(deadline).await;
        let (release, held) = std::sync::mpsc::channel();
        let (entered, ready) = oneshot::channel();
        let db = run.actor.db.clone();
        let blocker = tokio::spawn(async move {
            db.call(move |_| {
                let _ = entered.send(());
                held.recv_timeout(std::time::Duration::from_secs(3))
                    .unwrap();
            })
            .await
            .unwrap()
        });
        ready.await.unwrap();
        let expire = async {
            while !run.terminal.test_lease_locked() {
                tokio::task::yield_now().await;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "initial lease check must precede actual expiry"
            );
            tokio::time::sleep_until(
                tokio::time::Instant::from_std(deadline) + std::time::Duration::from_millis(1),
            )
            .await;
            release.send(()).unwrap();
        };
        let (allowed, ()) = tokio::join!(run.terminal.manual_hook_allowed(), expire);
        blocker.await.unwrap();
        assert!(
            !allowed,
            "DB queue crossing the actual lease expiry cannot allow native input"
        );
    }
}
