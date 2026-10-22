//! Human Codex controls use the original actor and command receipt owner.
use super::{
    core::{Core, Input},
    port::Port,
};
use crate::{
    dispatch,
    public_execution::{Admission, PublicExecution},
};
use crewd_exec::{
    codex::control::{CodexOperation as Op, CodexOutcome as Out},
    driver::{Control, ControlOutcome},
};
use crewd_proto::{
    RpcError,
    commands::{CommandPhase, CommandRuntimeTarget, PublicCommand as Cmd},
};
use serde_json::{Value, json};

pub(crate) fn is_command(c: &Cmd) -> bool {
    matches!(
        c,
        Cmd::CodexConfigure(_)
            | Cmd::CodexCompact(_)
            | Cmd::CodexSteer(_)
            | Cmd::CodexQueue(_)
            | Cmd::CodexReview(_)
            | Cmd::CodexTitleSet(_)
            | Cmd::CodexFork(_)
            | Cmd::CodexArchive(_) | Cmd::CodexUnarchive(_) | Cmd::CodexDelete(_) | Cmd::CodexRevert(_)
    )
}
fn target(c: &Cmd) -> CommandRuntimeTarget {
    match c {
        Cmd::SessionInterrupt(p) => p.clone(),
        Cmd::CodexConfigure(p) => p.target.clone(),
        Cmd::CodexCompact(p) => p.target.clone(),
        Cmd::CodexSteer(p) | Cmd::CodexQueue(p) => p.target.clone(),
        Cmd::CodexReview(p) => p.target.clone(),
        Cmd::CodexTitleSet(p) => p.target.clone(),
        Cmd::CodexFork(p) => p.target.clone(),
        Cmd::CodexArchive(p)|Cmd::CodexUnarchive(p)=>p.target.clone(),Cmd::CodexDelete(p)=>p.target.clone(),Cmd::CodexRevert(p)=>p.target.clone(),
        _ => unreachable!("closed Codex commands"),
    }
}
impl<P: Port> Core<P> {
    pub(crate) async fn execute_public_codex(
        &mut self,
        call: PublicExecution,
    ) -> Result<Value, RpcError> {
        if let Admission::Existing(r) = call.admit().await? {
            return serde_json::to_value(r).map_err(dispatch::internal);
        }
        let target = target(&call.request.command);
        let row = self
            .port
            .session(&target.session_id)
            .await
            .map_err(dispatch::internal)?;
        let row = match row.filter(|r| r.executor == "codex" && !r.crew_removed) {
            Some(row) if !self.shutting_down && self.matches_public_target(&target) => row,
            _ => {
                return self
                    .codex_suppress(&call, "command.stale_or_non_codex_target")
                    .await;
            }
        };
        if !call.codex_native_ready().await? {return self.codex_suppress(&call,"command.native_history_unavailable_or_unknown").await}
        if let Cmd::CodexQueue(p)|Cmd::CodexSteer(p)=&call.request.command {
            if let Some(inputs)=&p.inputs {if let Err(e)=inputs.validate(){return self.codex_suppress(&call,e).await}}
        }
        let live = self
            .live
            .get(&target.session_id)
            .filter(|l| !l.stopping && !l.halting && !l.hibernate_cut);
        let metadata = matches!(
            call.request.command,
            Cmd::CodexTitleSet(_) | Cmd::CodexFork(_) | Cmd::CodexArchive(_) | Cmd::CodexUnarchive(_) | Cmd::CodexDelete(_) | Cmd::CodexRevert(_)
        );
        if metadata
            && self
                .public_fences
                .get(&target.session_id)
                .is_some_and(|f| f.metadata.is_some())
        {
            return self
                .codex_suppress(&call, "command.codex_metadata_pending")
                .await;
        }
        if live.is_none() && !metadata {
            return self
                .codex_suppress(&call, "command.codex_requires_running")
                .await;
        }
        match &call.request.command {
            Cmd::CodexArchive(_) | Cmd::CodexUnarchive(_) | Cmd::CodexDelete(_)
                if self.live.contains_key(&target.session_id) || self.starting.contains_key(&target.session_id) => {
                return self.codex_suppress(&call,"command.native_history_requires_physical_stop").await;
            }
            Cmd::CodexDelete(p) if !p.confirm_native_history_delete=>{
                return self.codex_suppress(&call,"command.native_delete_requires_explicit_confirmation").await;
            }
            Cmd::CodexRevert(p) if live.is_none_or(|l|l.working||l.decision.is_some()) || p.before_turn_id.trim().is_empty() || p.before_turn_id.len()>256 || p.before_turn_id.chars().any(char::is_control)=>{
                return self.codex_suppress(&call,"command.native_revert_requires_idle_loaded_thread").await;
            }
            Cmd::SessionInterrupt(p) if p.turn_id.is_none() => {
                return self
                    .codex_suppress(&call, "command.codex_interrupt_requires_exact_turn")
                    .await;
            }
            Cmd::CodexConfigure(p) => {
                if let Err(e) = p.patch.validate() {
                    return self.codex_suppress(&call, e).await;
                }
            }
            Cmd::CodexSteer(p)
                if p.target.turn_id.is_none()
                    || p.text.trim().is_empty()
                    || p.text.len() > 128 * 1024 =>
            {
                return self
                    .codex_suppress(&call, "command.codex_steer_requires_exact_turn")
                    .await;
            }
            Cmd::CodexQueue(p) if p.text.trim().is_empty() || p.text.len() > 128 * 1024 => {
                return self
                    .codex_suppress(&call, "command.codex_invalid_body")
                    .await;
            }
            Cmd::CodexFork(_) | Cmd::CodexReview(_)
                if live.is_some_and(|l| l.working || l.decision.is_some()) =>
            {
                return self
                    .codex_suppress(&call, "command.codex_requires_idle")
                    .await;
            }
            Cmd::CodexTitleSet(p) => {
                if let Err(e) = crewd_collab::sessions::resolve_title(Some(&p.title), None) {
                    return self.codex_suppress(&call, &e.to_string()).await;
                }
            }
            _ => {}
        }
        let handle = live.map(|l| l.handle.clone());
        let Some(thread) = row.agent_session_id.clone().filter(|s| !s.is_empty()) else {
            return self
                .codex_suppress(&call, "command.codex_native_thread_missing")
                .await;
        };
        let Some(workdir) = row.workdir.clone() else {
            return self
                .codex_suppress(&call, "command.codex_workdir_missing")
                .await;
        };
        let exec = self.execs.get("codex").map_err(dispatch::internal)?;
        let permit = if handle.is_some() {
            let birth = self
                .public_fences
                .get(&target.session_id)
                .and_then(|f| f.birth.clone())
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "original Codex birth is unconfirmed")
                })?;
            crewd_exec::driver::ManualWritePermit::for_native_connection_lifetime(
                birth,
                call.actor().connection_scope().liveness(),
            )
        } else {
            crewd_exec::driver::ManualWritePermit::for_connection(
                std::time::Instant::now() + std::time::Duration::from_secs(30),
                call.actor().connection_scope().liveness(),
            )
        };
        let receipt = call.dispatch().await?;
        if metadata {
            self.public_fences
                .entry(target.session_id.clone())
                .or_default()
                .metadata = Some(super::public_control::PendingMetadataControl {
                request_id: call.request.request_id.clone(),
                permit: permit.clone(),
                revoked: false,
            });
        }
        self.advance_public_control(&target.session_id);
        let after = self.current_public_target(&target.session_id);
        let inbox = self.inbox.clone();
        let original = call.clone();
        tokio::spawn(async move {
            let op = match &call.request.command {
                Cmd::SessionInterrupt(p) => Some(Op::Interrupt {
                    expected_turn_id: p.turn_id.clone().expect("validated turn"),
                }),
                Cmd::CodexConfigure(p) => Some(Op::Configure(p.patch.clone())),
                Cmd::CodexTitleSet(p) => Some(Op::Name(
                    crewd_collab::sessions::resolve_title(Some(&p.title), None)
                        .expect("validated immutable title"),
                )),
                Cmd::CodexSteer(p) => Some(Op::Steer {
                    text: p.text.clone(),
                    expected_turn_id: p.target.turn_id.clone().expect("validated exact turn"),
                    request_id: call.request.request_id.clone(),
                }),
                Cmd::CodexReview(p) => Some(Op::Review(p.review_target.clone())),
                Cmd::CodexArchive(_)|Cmd::CodexUnarchive(_)|Cmd::CodexDelete(_)|Cmd::CodexRevert(_)=>Some(Op::HistoryMutation{action:call.request.command.native_history_action().expect("closed history action"),before_turn_id:match &call.request.command{Cmd::CodexRevert(p)=>Some(p.before_turn_id.clone()),_=>None}}),
                Cmd::CodexFork(p) => Some(Op::Fork {
                    last_turn_id: p.last_turn_id.clone(),
                }),
                _ => None,
            };
            let native_inputs=call.codex_original_inputs(&target.session_id).await;
            let outcome = if let Err(e) = call.recheck_dispatched().await {
                Out::Unknown {
                    provider_accepted: false,
                    detail: format!("dispatch authority no longer current: {e:?}"),
                }
            } else if let Err(e)=native_inputs {
                Out::Unknown{provider_accepted:false,detail:format!("original native input unavailable: {e:?}")}
            } else if let Some(inputs)=native_inputs.unwrap() {
                match (&call.request.command,handle) {
                    (Cmd::CodexQueue(p)|Cmd::CodexSteer(p),Some(h))=>h.codex_submit_inputs(p.text.clone(),call.request.request_id.clone(),if matches!(call.request.command,Cmd::CodexSteer(_)){p.target.turn_id.clone()}else{None},inputs,permit).await.unwrap_or(Out::Unknown{provider_accepted:false,detail:"original native input handle closed".into()}),
                    _=>Out::Rejected("native inputs require original live Codex handle".into()),
                }
            } else if let Some(op) = op {
                match handle {
                    Some(h) => h
                        .codex_controlled(op, permit)
                        .await
                        .unwrap_or(Out::Unknown {
                            provider_accepted: false,
                            detail: "original Codex handle closed".into(),
                        }),
                    None => {
                        exec.codex_metadata(crewd_exec::codex::metadata::MetadataRequest {
                            session_id: target.session_id.clone(),
                            thread_id: thread.clone(),
                            workdir,
                            operation: op,
                            permit: Some(permit),
                        })
                        .await
                    }
                }
            } else if let Some(h) = handle {
                match &call.request.command {
                    Cmd::CodexCompact(_) => {
                        match h.control_controlled(Control::Compact, permit).await {
                            Ok(ControlOutcome::Applied(detail)) => Out::Observed(
                                json!({"providerAccepted":true,"application":"started","detail":detail}),
                            ),
                            Ok(
                                ControlOutcome::Rejected(detail)
                                | ControlOutcome::Unsupported(detail),
                            ) => Out::Rejected(detail),
                            Ok(ControlOutcome::NoConfirmation(detail)) => Out::Unknown {
                                provider_accepted: true,
                                detail,
                            },
                            Err(_) => Out::Unknown {
                                provider_accepted: false,
                                detail: "compact handle closed".into(),
                            },
                        }
                    }
                    Cmd::CodexQueue(p) => h
                        .codex_queue_controlled(
                            p.text.clone(),
                            call.request.request_id.clone(),
                            permit,
                        )
                        .await
                        .unwrap_or(Out::Unknown {
                            provider_accepted: false,
                            detail: "queue original handle closed".into(),
                        }),
                    _ => unreachable!("closed operation"),
                }
            } else {
                Out::Rejected("runtime closed".into())
            };
            let _ = inbox.send(Input::PublicCodexDone {
                call: original,
                target: after,
                thread,
                outcome,
            });
        });
        serde_json::to_value(receipt).map_err(dispatch::internal)
    }
    async fn codex_suppress(&self, call: &PublicExecution, why: &str) -> Result<Value, RpcError> {
        let r = call
            .settle(
                CommandPhase::Admitted,
                CommandPhase::Suppressed,
                json!({"refusal":why,"providerAccepted":false,"application":"none"}),
            )
            .await?;
        serde_json::to_value(r).map_err(dispatch::internal)
    }
    pub(crate) async fn finish_public_codex(
        &mut self,
        call: PublicExecution,
        target: CommandRuntimeTarget,
        thread: String,
        outcome: Out,
    ) {
        let generation_current =
            self.current_public_target(&target.session_id).generation == target.generation;
        let not_stopping = self
            .live
            .get(&target.session_id)
            .is_none_or(|l| !l.stopping && !l.halting && !l.hibernate_cut);
        let metadata = matches!(
            call.request.command,
            Cmd::CodexTitleSet(_) | Cmd::CodexFork(_) | Cmd::CodexArchive(_) | Cmd::CodexUnarchive(_) | Cmd::CodexDelete(_) | Cmd::CodexRevert(_)
        );
        let metadata_current = !metadata
            || self
                .public_fences
                .get(&target.session_id)
                .and_then(|f| f.metadata.as_ref())
                .is_some_and(|p| p.request_id == call.request.request_id && !p.revoked);
        let (phase, mut result) = match outcome {
            Out::Observed(value)
                if generation_current
                    && not_stopping
                    && metadata_current
                    && !self.shutting_down =>
            {
                (CommandPhase::Confirmed, value)
            }
            Out::Observed(_) => (
                CommandPhase::Unknown,
                json!({"providerAccepted":true,"application":"unknown","detail":"original runtime generation/stop changed before settlement"}),
            ),
            Out::Rejected(detail) => (
                CommandPhase::Rejected,
                json!({"providerAccepted":false,"application":"none","detail":detail}),
            ),
            Out::Unknown {
                provider_accepted,
                detail,
            } => (
                CommandPhase::Unknown,
                json!({"providerAccepted":provider_accepted,"application":"unknown","detail":detail}),
            ),
        };
        result["sessionId"] = json!(target.session_id);
        result["threadId"] = json!(thread);
        result["operation"] = json!(call.request.command.method());
        if matches!(call.request.command, Cmd::CodexFork(_)) && phase == CommandPhase::Confirmed {
            let native = result
                .pointer("/providerResult/thread/id")
                .and_then(Value::as_str)
                .map(str::to_string);
            match native {
                Some(id) => result["nativeThreadId"] = json!(id),
                None => {
                    result["application"] = json!("unknown");
                    result["detail"] = json!("fork ACK lacks native thread identity");
                }
            }
        }
        if let Some(r) = result.as_object_mut() {
            r.remove("providerResult");
        }
        let phase = if matches!(call.request.command, Cmd::CodexFork(_))
            && result.get("nativeThreadId").is_none()
            && phase == CommandPhase::Confirmed
        {
            CommandPhase::Unknown
        } else {
            phase
        };
        if let Err(e) = call.settle(CommandPhase::Dispatched, phase, result).await {
            eprintln!("Codex original control receipt remains unknown: {e:?}");
        }
        // Keep ownership through child reaping and durable settlement. No second
        // metadata child may overtake a still-owned first request, even after Stop.
        if metadata
            && self
                .public_fences
                .get(&target.session_id)
                .and_then(|f| f.metadata.as_ref())
                .is_some_and(|p| p.request_id == call.request.request_id)
        {
            self.public_fences
                .get_mut(&target.session_id)
                .expect("owned metadata fence")
                .metadata = None;
        }
        if let Ok(state) = call.state() {
            state.hub.publish(crewd_proto::Event {
                event: crewd_proto::codex::CHANGED.into(),
                data: json!({"sessionId":target.session_id}),
            });
        }
    }
}
