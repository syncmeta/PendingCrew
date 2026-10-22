//! Public controls pin Core's actual launch and input/turn revision. These
//! projections confer no actor authority and never replace the work authority.
use super::core::{ControlError, Core};
use super::port::Port;
use crewd_exec::session::SessionEvent;
use crewd_proto::commands::CommandRuntimeTarget;

#[derive(Default)]
pub(crate) struct RuntimeFence {
    pub generation: u64,
    pub revision: u64,
    pub physical_epoch: u64,
    pub turn_id: Option<String>,
    pub birth: Option<crewd_exec::registry::ProcessIdentity>,
    pub manual: bool,
}

pub(crate) struct PendingPhysicalControl {
    call: crate::public_execution::PublicExecution,
    generator: u64,
    birth: Option<crewd_exec::registry::ProcessIdentity>,
    launching: bool,
}
pub(crate) struct PendingOwnedHibernate {
    call: crate::public_execution::PublicExecution,
    reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, crewd_proto::RpcError>>,
    births: Vec<Option<crewd_exec::registry::ProcessIdentity>>,
}
impl<P: Port> Core<P> {
    pub(crate) async fn execute_public_hibernate(
        &mut self,
        call: crate::public_execution::PublicExecution,
        reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, crewd_proto::RpcError>>,
    ) {
        use crate::public_execution::Admission;
        use crewd_proto::commands::{CommandPhase, PublicCommand};
        let admission = call.admit().await;
        match admission {
            Ok(Admission::Existing(receipt)) => {
                let _ =
                    reply.send(serde_json::to_value(receipt).map_err(crate::dispatch::internal));
                return;
            }
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
            Ok(Admission::New(_)) => {}
        }
        let PublicCommand::Hibernate(params) = &call.request.command else {
            unreachable!("closed owned Quit command")
        };
        if self.shutting_down && self.hibernation.is_none() {
            let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"orchestra.shutting_down","effectsDispatched":false})).await.and_then(|receipt|serde_json::to_value(receipt).map_err(crate::dispatch::internal));
            let _ = reply.send(receipt);
            return;
        }
        if let Err(error) = call.dispatch().await {
            let _ = reply.send(Err(error));
            return;
        }
        let tracked: std::collections::BTreeSet<_> = self
            .live
            .keys()
            .chain(self.starting.keys())
            .chain(
                self.hibernation
                    .iter()
                    .flat_map(|state| state.tracked.keys()),
            )
            .cloned()
            .collect();
        let births = tracked
            .iter()
            .map(|session| {
                self.public_fences
                    .get(session)
                    .and_then(|fence| fence.birth.clone())
                    .filter(|birth| {
                        call.owned_runtime_birth(session, birth.pid as u32)
                            .is_some_and(|current| {
                                current.start_time == birth.start_time
                                    && current.boot_id == birth.boot_id
                            })
                    })
            })
            .collect();
        let deadline = params
            .deadline_secs
            .map(|seconds| i64::from(seconds) * 1000);
        self.public_owned_hibernate.push(PendingOwnedHibernate {
            call,
            reply,
            births,
        });
        let (legacy, mut acknowledgement) = tokio::sync::oneshot::channel();
        self.begin_hibernate(deadline, legacy).await;
        // Core's existing hibernation owns physical stopping. No detached
        // receipt job may race DB teardown or require another scheduler.
        match acknowledgement.try_recv() {
            Ok(Ok(report)) => {
                // A completed earlier hibernation replies immediately. Its
                // actor step will never run again, so settle this waiter here.
                self.finish_public_hibernate(report.completed).await;
            }
            Ok(Err(error)) => {
                let error = super::rpc::rpc_err(error);
                if let Some(pending) = self.public_owned_hibernate.pop() {
                    let result = pending
                        .call
                        .settle(
                            CommandPhase::Dispatched,
                            CommandPhase::Unknown,
                            serde_json::json!({"refusal":error.data,"daemonExit":"unconfirmed"}),
                        )
                        .await
                        .and_then(|receipt| {
                            serde_json::to_value(receipt).map_err(crate::dispatch::internal)
                        });
                    let _ = pending.reply.send(result);
                }
            }
            Err(_) => {}
        }
    }
    /// Settle before the existing exit watch closes intake/storage. A confirmed
    /// hibernation record is separate from the App's own-child daemon exit wait.
    pub(crate) async fn finish_public_hibernate(&mut self, completed: bool) {
        use crewd_exec::registry::{self, ProbeResult};
        use crewd_proto::commands::CommandPhase;
        if self.public_owned_hibernate.is_empty() {
            return;
        }
        let persisted = match (&self.hibernation, self.port.hibernate_record().await) {
            (Some(state), Ok(Some(stored))) => stored == state.record,
            _ => false,
        };
        for pending in std::mem::take(&mut self.public_owned_hibernate) {
            let physical_children_exited = pending.births.iter().all(|birth| {
                birth
                    .as_ref()
                    .is_some_and(|birth| match registry::probe(birth.pid) {
                        ProbeResult::Missing => true,
                        ProbeResult::Found(current) => {
                            current.start_time != birth.start_time
                                || current.boot_id != birth.boot_id
                        }
                        ProbeResult::Unreadable(_) => false,
                    })
            });
            let phase = if completed && persisted && physical_children_exited {
                CommandPhase::Confirmed
            } else {
                CommandPhase::Unknown
            };
            let result=pending.call.settle(CommandPhase::Dispatched,phase,serde_json::json!({"hibernationRecordPersisted":persisted,"physicalChildrenExited":physical_children_exited,"actorCompleted":completed,"daemonExit":"must_observe_original_owned_child"})).await.and_then(|receipt|serde_json::to_value(receipt).map_err(crate::dispatch::internal));
            let _ = pending.reply.send(result);
        }
    }

    pub(crate) async fn execute_public_terminal(
        &mut self,
        call: crate::public_execution::PublicExecution,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use crate::public_execution::Admission;
        use base64::{Engine, engine::general_purpose::STANDARD};
        use crewd_proto::commands::{CommandPhase, PublicCommand};
        let target = match &call.request.command {
            PublicCommand::TerminalClaim(p) => p.target.clone(),
            PublicCommand::TerminalRelease(p) => p.target.clone(),
            PublicCommand::TerminalWrite(p) => p.target.clone(),
            PublicCommand::TerminalResize(p) => p.target.clone(),
            _ => unreachable!("closed terminal control"),
        };
        if let Admission::Existing(receipt) = call.admit().await? {
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        }
        let state = call.state()?;
        let runtime = match state.native_helpers.get() {
            Some(factory) => factory
                .terminal_for(
                    &target.session_id,
                    self.live
                        .get(&target.session_id)
                        .and_then(|live| live.handle.terminal_driver()),
                )
                .await
                .ok(),
            None => None,
        };
        if self.shutting_down
            || !self.matches_public_target(&target)
            || runtime.as_ref().is_none_or(|runtime| !runtime.is_live())
        {
            let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"command.native_runtime_unavailable_or_stale","effectsDispatched":false})).await?;
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        }
        let runtime = runtime.expect("checked actual runtime");
        let binding = runtime.binding().map_err(crate::dispatch::internal)?;
        if call
            .owned_runtime_birth(&target.session_id, binding.native.pid as u32)
            .is_none_or(|actual| {
                actual.start_time != binding.native.start_time
                    || actual.boot_id != binding.native.boot_id
            })
        {
            let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"command.native_birth_unconfirmed","effectsDispatched":false})).await?;
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        }
        // Validate bytes/size BEFORE physical dispatch. Invalid input does not
        // consume a PTY command, rotate a generation or revoke another lease.
        let bytes = if let PublicCommand::TerminalWrite(p) = &call.request.command {
            let decoded = STANDARD
                .decode(&p.bytes_base64)
                .ok()
                .filter(|bytes| !bytes.is_empty() && bytes.len() <= 65536);
            if decoded.is_none() {
                let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"command.invalid_terminal_bytes","effectsDispatched":false})).await?;
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            }
            decoded
        } else {
            None
        };
        let input_bytes = bytes.as_ref().map(|bytes| bytes.len());
        call.dispatch().await?;
        let result = match &call.request.command {
            PublicCommand::TerminalClaim(p) => {
                runtime
                    .claim(
                        &call.request.request_id,
                        call.actor(),
                        &state,
                        p.previous_lease_id.as_deref(),
                    )
                    .await
            }
            PublicCommand::TerminalRelease(p) => {
                runtime
                    .release(&call.request.request_id, call.actor(), &p.lease_id)
                    .await
            }
            PublicCommand::TerminalWrite(p) => {
                runtime
                    .write(
                        &call.request.request_id,
                        call.actor(),
                        &p.lease_id,
                        bytes.expect("decoded before dispatch"),
                    )
                    .await
            }
            PublicCommand::TerminalResize(p) => {
                runtime
                    .resize(
                        &call.request.request_id,
                        call.actor(),
                        &p.lease_id,
                        p.cols,
                        p.rows,
                    )
                    .await
            }
            _ => unreachable!("closed terminal operation"),
        };
        if runtime.manual_floor().await {
            self.public_fences
                .entry(target.session_id.clone())
                .or_default()
                .manual = true;
        }
        // This call invalidates delayed controls even on unknown physical
        // results. The immutable original request and old generation remain.
        self.advance_public_control(&target.session_id);
        let next = self.current_public_target(&target.session_id);
        let (phase, mut observed) = match result {
            Ok(mut value) => {
                let phase = match &call.request.command {
                    PublicCommand::TerminalWrite(_) if value["unknown"] == false => {
                        CommandPhase::Written
                    }
                    PublicCommand::TerminalResize(_) if value["applied"] == true => {
                        CommandPhase::Resized
                    }
                    PublicCommand::TerminalClaim(_) | PublicCommand::TerminalRelease(_) => {
                        CommandPhase::Confirmed
                    }
                    _ => CommandPhase::Unknown,
                };
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "target".into(),
                        serde_json::to_value(&next).map_err(crate::dispatch::internal)?,
                    );
                }
                (phase, value)
            }
            Err(_) => (
                CommandPhase::Unknown,
                serde_json::json!({"physicalResult":"unconfirmed","replayAllowed":false,"target":next}),
            ),
        };
        if let Some(object) = observed.as_object_mut() {
            if let Some(input_bytes) = input_bytes {
                object.insert("inputBytes".into(), serde_json::json!(input_bytes));
            }
            object.insert(
                "applicationConsumed".into(),
                serde_json::json!("unconfirmed"),
            );
        }
        let receipt = call
            .settle(CommandPhase::Dispatched, phase, observed)
            .await?;
        serde_json::to_value(receipt).map_err(crate::dispatch::internal)
    }

    pub(crate) async fn execute_public_control(
        &mut self,
        call: crate::public_execution::PublicExecution,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use crate::public_execution::Admission;
        use crewd_proto::commands::{CommandPhase, PublicCommand};
        if matches!(
            &call.request.command,
            PublicCommand::TerminalClaim(_)
                | PublicCommand::TerminalRelease(_)
                | PublicCommand::TerminalWrite(_)
                | PublicCommand::TerminalResize(_)
        ) {
            return self.execute_public_terminal(call).await;
        }
        if matches!(
            &call.request.command,
            PublicCommand::SessionAnswer(_) | PublicCommand::SessionRespondRunnerRequest(_)
        ) {
            return self.execute_public_answer(call).await;
        }
        if matches!(&call.request.command, PublicCommand::SessionStart(_)) {
            return self.execute_public_start(call).await;
        }
        let target = match &call.request.command {
            PublicCommand::SessionStop(p) => p.target.clone(),
            PublicCommand::SessionResume(p) | PublicCommand::SessionInterrupt(p) => p.clone(),
            _ => {
                return Err(crewd_proto::RpcError::new(
                    crewd_proto::RpcError::INVALID_PARAMS,
                    "unsupported Core control",
                ));
            }
        };
        let admitted = match call.admit().await? {
            Admission::Existing(receipt) => {
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            }
            Admission::New(receipt) => receipt,
        };
        if admitted.phase != CommandPhase::Admitted || admitted.historical {
            return Err(crate::dispatch::internal(
                "public dispatch lost its current durable admission",
            ));
        }
        // A stale target is a durable zero-effect suppression, never a replay
        // against whichever runtime happens to occupy the session now.
        if self.shutting_down
            || !self.matches_public_target(&target)
            || self
                .port
                .session(&target.session_id)
                .await
                .map_err(crate::dispatch::internal)?
                .is_none()
        {
            let receipt = call
                .settle(
                    CommandPhase::Admitted,
                    CommandPhase::Suppressed,
                    serde_json::json!({"refusal":"command.stale_target"}),
                )
                .await?;
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        }
        let dispatched = call.dispatch().await?;
        let previous_generator = self.live.get(&target.session_id).map(|l| l.generator);
        let previous_birth = self
            .public_fences
            .get(&target.session_id)
            .and_then(|f| f.birth.clone())
            .filter(|birth| {
                call.owned_runtime_birth(&target.session_id, birth.pid as u32)
                    .is_some_and(|current| {
                        current.start_time == birth.start_time && current.boot_id == birth.boot_id
                    })
            });
        self.advance_public_control(&target.session_id);
        let result = match &call.request.command {
            PublicCommand::SessionStop(_) => {
                // Intent already committed beside the public receipt above.
                // Revoke all byte leases before queuing physical stop; neither
                // an empty observation stream nor this queue ACK proves exit.
                let revoke = match call.state() {
                    Ok(state) => match state.native_helpers.get() {
                        Some(factory) => factory.revoke_session(&target.session_id).await,
                        None => Ok(()),
                    },
                    Err(_) => Err(std::io::Error::other("original stop owner closed")),
                };
                if revoke.is_err() {
                    Err(ControlError::NotRunning(target.session_id.clone()))
                } else {
                    if let Some(a) = &self.cfg.admission {
                        a.revoke(&target.session_id);
                    }
                    self.held.remove(&target.session_id);
                    if let Some(live) = self.live.get_mut(&target.session_id) {
                        live.stopping = true;
                        super::core::refuse_queued_nudges(live, &target.session_id);
                        let _ = live.handle.stop();
                    } else if let Some(starting) = self.starting.get_mut(&target.session_id) {
                        starting.stop_requested = true;
                    }
                    Ok(())
                }
            }
            PublicCommand::SessionResume(_) => {
                self.resume(&target.session_id, super::core::Origin::Human)
                    .await
            }
            PublicCommand::SessionInterrupt(_) => match self.live.get(&target.session_id) {
                Some(l)
                    if !l.stopping
                        && !l.halting
                        && !l.hibernate_cut
                        && (l.working || l.decision.is_some()) =>
                {
                    l.handle
                        .interrupt()
                        .map_err(|_| ControlError::NotRunning(target.session_id.clone()))
                }
                Some(_) => Err(ControlError::NothingToInterrupt(target.session_id.clone())),
                None => Err(ControlError::NotRunning(target.session_id.clone())),
            },
            _ => unreachable!("closed controls above"),
        };
        // Persisted dispatch is returned promptly; physical confirmation is a
        // later, exact-runtime observation. A crash leaves this unknown at boot.
        if result.is_ok() {
            let pending = match &call.request.command {
                PublicCommand::SessionStop(_) => {
                    previous_generator
                        .zip(previous_birth)
                        .map(|(generator, birth)| PendingPhysicalControl {
                            call: call.clone(),
                            generator,
                            birth: Some(birth),
                            launching: false,
                        })
                }
                PublicCommand::SessionResume(_) => {
                    self.starting
                        .get(&target.session_id)
                        .map(|s| PendingPhysicalControl {
                            call: call.clone(),
                            generator: s.generator,
                            birth: None,
                            launching: true,
                        })
                }
                _ => None,
            };
            if let Some(pending) = pending {
                self.public_physical_controls
                    .entry(target.session_id.clone())
                    .or_default()
                    .push(pending);
                return serde_json::to_value(dispatched).map_err(crate::dispatch::internal);
            }
        }
        let observed = match result {
            Ok(()) => serde_json::json!({"admittedByCore":true,"actualOutcome":"unconfirmed"}),
            Err(e) => {
                let error = super::rpc::rpc_err(e);
                serde_json::json!({"admittedByCore":false,"refusal":error.data.as_ref().map(|d|d.refusal.clone()),"actualOutcome":"unconfirmed"})
            }
        };
        let receipt = call
            .settle(CommandPhase::Dispatched, CommandPhase::Unknown, observed)
            .await?;
        serde_json::to_value(receipt).map_err(crate::dispatch::internal)
    }

    pub(crate) async fn execute_public_answer(
        &mut self,
        call: crate::public_execution::PublicExecution,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use crate::public_execution::Admission;
        use crewd_proto::commands::{CommandPhase, PublicCommand};
        let (params, choice) = match &call.request.command {
            PublicCommand::SessionAnswer(params) => (params.clone(), true),
            PublicCommand::SessionRespondRunnerRequest(params) => (params.clone(), false),
            _ => unreachable!("closed structured public response"),
        };
        match call.admit().await? {
            Admission::Existing(receipt) => {
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            }
            Admission::New(_) => {}
        }
        let decision = self
            .live
            .get(&params.target.session_id)
            .and_then(|live| live.decision.clone());
        let valid=decision.as_ref().is_some_and(|decision|matches!(&decision.kind,crewd_term::DecisionKind::RunnerRequest{request_id} if request_id==&params.runner_request_id) && decision.fingerprint()==params.fingerprint);
        if self.shutting_down || !self.matches_public_target(&params.target) || !valid {
            let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"command.stale_runner_request","responseWritten":false})).await?;
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        }
        let action = if choice {
            let index = params
                .response
                .as_object()
                .filter(|response| response.len() == 1)
                .and_then(|response| response.get("choice"))
                .and_then(serde_json::Value::as_u64)
                .and_then(|index| usize::try_from(index).ok());
            let Some(index) = index.filter(|index| {
                *index < decision.as_ref().expect("validated decision").options.len()
            }) else {
                let receipt=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,serde_json::json!({"refusal":"decision.out_of_range","responseWritten":false})).await?;
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            };
            super::core::AnswerAction::Choice(index)
        } else {
            super::core::AnswerAction::Response(params.response)
        };
        let receipt = call.dispatch().await?;
        self.advance_public_control(&params.target.session_id);
        let target = self.current_public_target(&params.target.session_id);
        let Some(live) = self
            .live
            .get_mut(&params.target.session_id)
            .filter(|live| !live.stopping && !live.halting && !live.hibernate_cut)
        else {
            let receipt = call
                .settle(
                    CommandPhase::Dispatched,
                    CommandPhase::Unknown,
                    serde_json::json!({"refusal":"command.runtime_closed","responseWritten":false}),
                )
                .await?;
            return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
        };
        live.writes.push_back(super::core::Write::Answer {
            expect: params.fingerprint,
            action,
            reply: super::core::AnswerReply::Public {
                call: Box::new(call),
                inbox: self.inbox.clone(),
            },
            public_target: Some(target),
        });
        self.pump_writes(&params.target.session_id)
            .await
            .map_err(crate::dispatch::internal)?;
        serde_json::to_value(receipt).map_err(crate::dispatch::internal)
    }
    pub(crate) async fn settle_public_answer(
        &self,
        call: crate::public_execution::PublicExecution,
        outcome: Result<super::core::AnswerReport, ControlError>,
    ) {
        use crewd_proto::commands::CommandPhase;
        let (phase, observed) = match outcome {
            Ok(report) => (
                CommandPhase::Written,
                serde_json::json!({"structuredResponseWritten":true,"nativeApplicationApplied":"unconfirmed","clearedByDriver":report.cleared}),
            ),
            Err(error) => {
                let error = super::rpc::rpc_err(error);
                (
                    CommandPhase::Unknown,
                    serde_json::json!({"structuredResponseWritten":"unconfirmed","refusal":error.data}),
                )
            }
        };
        if let Err(error) = call.settle(CommandPhase::Dispatched, phase, observed).await {
            eprintln!("public structured response settlement remains unconfirmed: {error:?}");
        }
    }

    pub(crate) async fn execute_public_start(
        &mut self,
        call: crate::public_execution::PublicExecution,
    ) -> Result<serde_json::Value, crewd_proto::RpcError> {
        use crate::public_execution::Admission;
        use crewd_proto::commands::{CommandPhase, PublicCommand};
        let PublicCommand::SessionStart(params) = &call.request.command else {
            unreachable!("closed public start")
        };
        let req = super::core::StartSession {
            crew_id: params.crew_id.clone(),
            captain: params.role == crewd_proto::Role::Captain,
            executor: params.executor.clone(),
            brief: params.brief.clone(),
            title: params.title.clone(),
            codex_options: params.codex_options.clone(),
            model: params.model.clone(),
            effort: params.effort.clone(),
            isolation: params.isolation,
        };
        match call.admit().await? {
            Admission::Existing(receipt) => {
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            }
            Admission::New(_) => {}
        }
        let base = match self.check_start(&req, super::core::Origin::Human).await {
            Ok(base) => base,
            Err(error) => {
                let error = super::rpc::rpc_err(error);
                let receipt = call
                    .settle(
                        CommandPhase::Admitted,
                        CommandPhase::Suppressed,
                        serde_json::json!({"refusal":error.data,"effectsDispatched":false}),
                    )
                    .await?;
                return serde_json::to_value(receipt).map_err(crate::dispatch::internal);
            }
        };
        let receipt = call.dispatch().await?;
        if let Some(base) = base {
            self.preparing += 1;
            if req.captain {
                self.captain_pending.insert(req.crew_id.clone());
            }
            let worktrees = self.hooks.worktrees.clone();
            let inbox = self.inbox.clone();
            tokio::spawn(async move {
                let result = worktrees.create(&base, &req.brief).await;
                let _ = inbox.send(super::core::Input::PublicWorktreeReady { req, call, result });
            });
        } else if let Err(error) = self.finish_public_start(req, None, call.clone()).await {
            self.public_start_failed(&call, error).await;
        }
        // Admission/dispatch is not a launch. The same receipt will be updated
        // only by the actual driver/native kernel observation.
        serde_json::to_value(receipt).map_err(crate::dispatch::internal)
    }

    pub(crate) async fn finish_public_start(
        &mut self,
        req: super::core::StartSession,
        worktree: Option<std::path::PathBuf>,
        call: crate::public_execution::PublicExecution,
    ) -> Result<(), ControlError> {
        if self.shutting_down {
            return Err(ControlError::ShuttingDown);
        }
        let executor = self
            .execs
            .get(&req.executor)
            .map_err(|_| ControlError::Unsupported(req.executor.clone()))?;
        self.check_launch(&executor, &req.crew_id, &req.executor)?;
        let brief = Some(req.brief.clone()).filter(|brief| !brief.trim().is_empty());
        let row = self
            .port
            .insert_public_session(
                super::port::NewSession {
                    codex_options: None,

                    crew_id: req.crew_id,
                    captain: req.captain,
                    executor: req.executor,
                    brief: req.brief,
                    title: req.title,
                    model: req.model,
                    effort: req.effort,
                    worktree,
                },
                call.clone(),
            )
            .await
            .map_err(ControlError::from_port)?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        if let Some(admission) = &self.cfg.admission {
            admission.allow(&row.id);
        }
        self.launch(row.clone(), executor, brief, vec![], vec![])
            .await
            .map_err(ControlError::from_port)?;
        if let Some(starting) = self.starting.get(&row.id) {
            self.public_physical_controls
                .entry(row.id.clone())
                .or_default()
                .push(PendingPhysicalControl {
                    call,
                    generator: starting.generator,
                    birth: None,
                    launching: true,
                });
        } else {
            return Err(ControlError::NotRunning(row.id));
        }
        Ok(())
    }

    /// Worktree preparation returned before session allocation or model launch.
    /// Git may have left preparation artifacts; that is distinct from an unknown
    /// session execution outcome and must not block every later explicit start.
    pub(crate) async fn public_worktree_rejected(
        &self,
        call: &crate::public_execution::PublicExecution,
        error: super::worktree::WorktreeError,
    ) {
        use crewd_proto::commands::CommandPhase;
        let error = super::rpc::rpc_err(ControlError::Worktree(error));
        if let Err(error) = call
            .settle(
                CommandPhase::Dispatched,
                CommandPhase::Rejected,
                serde_json::json!({
                    "actualOutcome":"start_rejected_before_session",
                    "sessionExecutionDispatched":false,
                    "refusal":error.data
                }),
            )
            .await
        {
            eprintln!("public worktree refusal settlement unconfirmed: {error:?}");
        }
    }

    pub(crate) async fn public_start_failed(
        &self,
        call: &crate::public_execution::PublicExecution,
        error: ControlError,
    ) {
        use crewd_proto::commands::CommandPhase;
        let error = super::rpc::rpc_err(error);
        if let Err(error) = call
            .settle(
                CommandPhase::Dispatched,
                CommandPhase::Unknown,
                serde_json::json!({"actualOutcome":"start_not_confirmed","refusal":error.data}),
            )
            .await
        {
            eprintln!("public start terminal settlement unconfirmed: {error:?}");
        }
    }

    /// Only the driver event belonging to the current scheduler generation may
    /// discharge an obligation. Kernel unreadability never becomes success.
    pub(crate) async fn settle_public_physical_event(
        &mut self,
        session: &str,
        generator: u64,
        event: &SessionEvent,
    ) {
        use crewd_exec::registry::{self, ProbeResult};
        use crewd_proto::commands::CommandPhase;
        let Some(mut pending) = self.public_physical_controls.remove(session) else {
            return;
        };
        let mut retained = Vec::new();
        for obligation in pending.drain(..) {
            if obligation.generator != generator {
                retained.push(obligation);
                continue;
            }
            let settlement = match (obligation.launching, event) {
                (true, SessionEvent::Spawned { pid, .. }) => {
                    match obligation.call.owned_runtime_birth(session, *pid) {
                        Some(birth) => Some((
                            CommandPhase::Launched,
                            serde_json::json!({"actualGeneration":self.current_public_target(session).generation,"birth":birth}),
                        )),
                        _ => Some((
                            CommandPhase::Unknown,
                            serde_json::json!({"actualOutcome":"native_birth_unconfirmed"}),
                        )),
                    }
                }
                (false, SessionEvent::Exited(_)) => {
                    let gone =
                        obligation.birth.as_ref().is_some_and(|birth| {
                            match registry::probe(birth.pid) {
                                ProbeResult::Missing => true,
                                ProbeResult::Found(current) => {
                                    current.start_time != birth.start_time
                                        || current.boot_id != birth.boot_id
                                }
                                ProbeResult::Unreadable(_) => false,
                            }
                        });
                    Some((
                        if gone {
                            CommandPhase::Confirmed
                        } else {
                            CommandPhase::Unknown
                        },
                        serde_json::json!({"physicalExit":if gone { "confirmed" } else { "unconfirmed" },"physicalExitConfirmed":gone,"originalBirth":obligation.birth}),
                    ))
                }
                (true, SessionEvent::Exited(_)) => Some((
                    CommandPhase::Unknown,
                    serde_json::json!({"actualOutcome":"launch_exited_without_birth_confirmation"}),
                )),
                _ => None,
            };
            if let Some((phase, result)) = settlement {
                if let Err(error) = obligation
                    .call
                    .settle(CommandPhase::Dispatched, phase, result)
                    .await
                {
                    eprintln!(
                        "public physical settlement remains durable-dispatched for {session}: {error:?}"
                    );
                    // Do not issue effects again. Restart converts the unresolved
                    // receipt to unknown even if this settlement cannot commit.
                }
            } else {
                retained.push(obligation);
            }
        }
        if !retained.is_empty() {
            self.public_physical_controls
                .insert(session.into(), retained);
        }
    }

    pub(crate) async fn public_physical_lost(
        &mut self,
        session: &str,
        generator: u64,
        reason: &str,
    ) {
        use crewd_proto::commands::CommandPhase;
        let Some(pending) = self.public_physical_controls.remove(session) else {
            return;
        };
        let mut retained = Vec::new();
        for obligation in pending {
            if obligation.generator != generator {
                retained.push(obligation);
                continue;
            }
            if let Err(error) = obligation
                .call
                .settle(
                    CommandPhase::Dispatched,
                    CommandPhase::Unknown,
                    serde_json::json!({"actualOutcome":reason}),
                )
                .await
            {
                eprintln!("unconfirmed public control settlement failed for {session}: {error:?}");
            }
        }
        if !retained.is_empty() {
            self.public_physical_controls
                .insert(session.into(), retained);
        }
    }

    pub(crate) fn advance_public_control(&mut self, session: &str) {
        let fence = self.public_fences.entry(session.into()).or_default();
        // Exhaustion is fail-closed. There is no wrapping value a delayed
        // request could match; the daemon must be explicitly restarted.
        fence.revision = fence
            .revision
            .checked_add(1)
            .expect("public control revision exhausted");
    }
    pub(crate) fn observe_public_runtime(&mut self, session: &str, generation: u64) {
        self.advance_public_control(session);
        let fence = self.public_fences.get_mut(session).expect("advanced fence");
        fence.generation = generation;
        fence.physical_epoch = 0;
        fence.turn_id = None;
        fence.birth = None;
    }
    pub(crate) fn observe_public_event(&mut self, session: &str, event: &SessionEvent) {
        let changed = match event {
            SessionEvent::Spawned { .. } => true,
            SessionEvent::Working(true) => self.live.get(session).is_some_and(|l| !l.working),
            SessionEvent::Item { turn_id, .. } | SessionEvent::TurnEnded { turn_id, .. } => {
                self.public_fences
                    .get(session)
                    .is_none_or(|f| f.turn_id.as_deref() != Some(turn_id.as_str()))
                    || matches!(event, SessionEvent::TurnEnded { .. })
            }
            _ => false,
        };
        if changed {
            self.advance_public_control(session);
        }
        if let SessionEvent::Spawned { pid, .. } = event {
            let fence = self.public_fences.entry(session.into()).or_default();
            fence.manual = false;
            fence.birth = match crewd_exec::registry::probe(*pid as i32) {
                crewd_exec::registry::ProbeResult::Found(birth) => Some(birth),
                _ => None,
            };
            fence.physical_epoch = fence
                .physical_epoch
                .checked_add(1)
                .expect("public physical epoch exhausted");
        }
        if let SessionEvent::Item { turn_id, .. } = event {
            self.public_fences
                .entry(session.into())
                .or_default()
                .turn_id = (!turn_id.is_empty()).then(|| turn_id.clone());
        } else if matches!(
            event,
            SessionEvent::TurnEnded { .. } | SessionEvent::Spawned { .. }
        ) {
            self.public_fences
                .entry(session.into())
                .or_default()
                .turn_id = None;
        }
    }
    pub(crate) fn current_public_target(&self, session: &str) -> CommandRuntimeTarget {
        let fence = self.public_fences.get(session);
        let generation = self
            .live
            .get(session)
            .map(|l| l.generator)
            .or_else(|| self.starting.get(session).map(|s| s.generator))
            .or_else(|| fence.map(|f| f.generation))
            .unwrap_or(0);
        CommandRuntimeTarget {
            session_id: session.into(),
            generation: format!(
                "{}:{}:{}",
                self.cfg.boot_id,
                generation,
                fence.map(|f| f.physical_epoch).unwrap_or(0)
            ),
            control_revision: fence.map(|f| f.revision).unwrap_or(0).to_string(),
            turn_id: fence.and_then(|f| f.turn_id.clone()),
        }
    }
    pub(crate) async fn public_target(
        &self,
        session: &str,
    ) -> Result<CommandRuntimeTarget, ControlError> {
        if self
            .port
            .session(session)
            .await
            .map_err(ControlError::from_port)?
            .is_none()
        {
            return Err(ControlError::NotFound(session.into()));
        }
        Ok(self.current_public_target(session))
    }
    pub(crate) fn matches_public_target(&self, target: &CommandRuntimeTarget) -> bool {
        self.current_public_target(&target.session_id) == *target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::{core::Cmd, testkit::Rig};
    #[tokio::test]
    async fn real_core_target_changes_on_new_turn_and_stop_without_accepting_old_controls() {
        let mut rig = Rig::new().await;
        rig.crew("public-crew", "captain");
        rig.run("captain").await;
        let before = rig.core.public_target("captain").await.unwrap();
        assert!(rig.core.matches_public_target(&before));
        rig.emit(
            "captain",
            SessionEvent::Item {
                turn_id: "actual-structured-turn".into(),
                item: serde_json::json!({"id":"owned-item"}),
            },
        )
        .await;
        let turn = rig.core.public_target("captain").await.unwrap();
        assert_eq!(turn.turn_id.as_deref(), Some("actual-structured-turn"));
        assert!(!rig.core.matches_public_target(&before));
        assert!(rig.core.matches_public_target(&turn));
        rig.cmd(|reply| Cmd::Stop {
            session: "captain".into(),
            reply,
        })
        .await
        .unwrap();
        assert!(
            !rig.core.matches_public_target(&turn),
            "a delayed interrupt cannot affect the stopped/replaced turn"
        );
    }
    #[tokio::test]
    async fn actual_core_respawn_retires_generation_and_missing_turn_stays_unconfirmed() {
        let mut rig = Rig::new().await;
        rig.crew("public-crew", "captain");
        rig.run("captain").await;
        let old = rig.core.public_target("captain").await.unwrap();
        rig.emit(
            "captain",
            SessionEvent::Spawned {
                pid: 999,
                agent_session_id: "replacement-thread".into(),
                resumed: false,
            },
        )
        .await;
        let changed = rig.core.public_target("captain").await.unwrap();
        assert_ne!(
            old.generation, changed.generation,
            "a driver respawn retires the prior control generation"
        );
        assert!(!rig.core.matches_public_target(&old));
        rig.emit(
            "captain",
            SessionEvent::Item {
                turn_id: String::new(),
                item: serde_json::json!({"id":"no-origin-turn"}),
            },
        )
        .await;
        assert!(
            rig.core
                .public_target("captain")
                .await
                .unwrap()
                .turn_id
                .is_none()
        );
        assert!(matches!(
            rig.core.public_target("foreign-session").await,
            Err(ControlError::NotFound(_))
        ));
    }
}
