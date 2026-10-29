//! One opaque captured-actor call to the existing Core scheduler. It is not an
//! arbitrary RPC proxy or a second execution ledger. Unknown effects are retained.
use crate::{
    AppState, dispatch,
    public_commands::{Actor, reserve_error},
};
use crewd_proto::{RpcError, commands::*};
use crewd_store::command_receipts::{self as receipts, Reserved};
use serde_json::Value;
use std::sync::{Arc, Weak};

#[derive(Clone)]
pub struct PublicExecution {
    state: Weak<AppState>,
    actor: Actor,
    principal: String,
    pub(crate) request: CommandSubmitParams,
}
pub(crate) enum Admission {
    Existing(PublicCommandReceipt),
    New(PublicCommandReceipt),
}
impl PublicExecution {
    pub(crate) async fn capture(
        state: &Arc<AppState>,
        actor: &Actor,
        request: CommandSubmitParams,
    ) -> Result<Self, RpcError> {
        if !crewd_proto::valid_remote_uuid(&request.request_id)
            || !crewd_proto::valid_remote_uuid(&request.runtime_binding_id)
        {
            return Err(
                RpcError::new(RpcError::INVALID_PARAMS, "invalid command identity")
                    .with_refusal("command.invalid"),
            );
        }
        let payload = serde_json::to_vec(&request.command).map_err(dispatch::internal)?;
        if payload.len() > MAX_COMMAND_PAYLOAD_BYTES {
            return Err(
                RpcError::new(RpcError::INVALID_PARAMS, "command payload too large")
                    .with_refusal("command.invalid"),
            );
        }
        if let PublicCommand::SessionStart(p)=&request.command {
            if let Some(inputs)=&p.codex_inputs {
                if p.brief.trim().is_empty() {
                    return Err(RpcError::new(RpcError::INVALID_PARAMS,"native inputs require a nonblank original task").with_refusal("command.invalid"));
                }
                if p.executor!="codex" {return Err(RpcError::new(RpcError::INVALID_PARAMS,"native inputs require Codex"))}
                inputs.validate().map_err(|e|RpcError::new(RpcError::INVALID_PARAMS,e))?;
            }
        }
        let principal = actor.principal(state).await?;
        crate::public_commands::validate_database(state, &request.db_id).await?;
        for operation in request.command.authorized_methods() {
            actor.authorize(state, operation).await?;
        }
        Ok(Self {
            state: Arc::downgrade(state),
            actor: actor.clone(),
            principal,
            request,
        })
    }
    fn input_owner(&self)->crewd_store::codex_inputs::Owner {
        crewd_store::codex_inputs::Owner{db_id:self.request.db_id.clone(),principal:self.principal.clone()}
    }
    pub(crate) async fn codex_original_inputs(&self,session:&str)->Result<Option<crewd_exec::codex::input::Submission>,RpcError>{
        let selection=match &self.request.command {
            PublicCommand::SessionStart(p)=>p.codex_inputs.clone(),
            PublicCommand::CodexQueue(p)|PublicCommand::CodexSteer(p)=>p.inputs.clone(),
            _=>None,
        };
        let Some(selection)=selection else{return Ok(None)};
        let call=self.clone();let session=session.to_owned();let state=self.state()?;let owner=state.clone();
        state.db.call(move |s|->Result<_,RpcError>{
            for method in call.request.command.authorized_methods(){call.actor.authorize_tx(&owner,s.conn(),method).map_err(dispatch::collab_err)?;}
            let row=crewd_collab::sessions::get(s.conn(),&session).map_err(dispatch::collab_err)?;
            let payloads=crewd_store::codex_inputs::for_original(s.conn(),&call.input_owner(),&row.crew_id,&session,&call.request.request_id,&selection).map_err(dispatch::internal)?;
            Ok(Some(crewd_exec::codex::input::Submission{
                payloads:payloads.into_iter().map(|p|crewd_exec::codex::input::Payload{kind:p.reference.kind,filename:p.filename,mime_type:p.mime_type,sha256:p.reference.sha256,body:p.body}).collect(),
                skills:selection.skills,
            }))
        }).await.map_err(dispatch::internal)?
    }
    pub(crate) fn principal(&self) -> &str { &self.principal }
    pub(crate) fn actor(&self) -> &Actor {
        &self.actor
    }
    pub(crate) fn state(&self) -> Result<Arc<AppState>, RpcError> {
        self.state.upgrade().ok_or_else(|| {
            RpcError::new(RpcError::CONFLICT, "original daemon is shutting down")
                .with_refusal("orchestra.shutting_down")
        })
    }
    /// Recheck the captured operation and immutable start payload in the very
    /// transaction that allocates the session. Worktree preparation grants no
    /// authority to register after revocation or under another daemon/database.
    pub(crate) fn authorize_registration(
        &self,
        owner: &Arc<AppState>,
        tx: &crewd_collab::Tx<'_>,
        session: &crate::orchestra::port::NewSession,
    ) -> crewd_collab::Result<()> {
        let expected = self
            .state
            .upgrade()
            .is_some_and(|state| Arc::ptr_eq(&state, owner));
        let PublicCommand::SessionStart(original) = &self.request.command else {
            return Err(
                crewd_store::StoreError::Invalid("public start operation mismatch".into()).into(),
            );
        };
        if !expected
            || self.request.runtime_binding_id != owner.runtime_binding_id
            || original.crew_id != session.crew_id
            || (original.role == crewd_proto::Role::Captain) != session.captain
            || original.executor != session.executor
            || original.brief != session.brief
            || original.title != session.title
            || original.model != session.model
            || original.effort != session.effort
            || original.codex_options != session.codex_options
            || original.isolation != session.worktree.is_some()
        {
            return Err(crewd_store::StoreError::Invalid(
                "public start immutable scope mismatch".into(),
            )
            .into());
        }
        self.actor.authorize_tx(owner, tx.conn(), "session.start")?;
        let receipt = receipts::lookup(
            tx.conn(),
            &self.principal,
            &self.request.db_id,
            &self.request.request_id,
        )?
        .ok_or_else(|| crewd_store::StoreError::Invalid("public start receipt absent".into()))?;
        if receipt.phase != CommandPhase::Dispatched
            || receipt.historical
            || receipt.result.is_some()
        {
            return Err(crewd_store::StoreError::Invalid(
                "public start is not an unallocated dispatch".into(),
            )
            .into());
        }
        Ok(())
    }
    pub(crate) fn record_registration(
        &self,
        tx: &crewd_collab::Tx<'_>,
        session: &str,
    ) -> crewd_collab::Result<()> {
        if let PublicCommand::SessionStart(p)=&self.request.command {
            if let Some(inputs)=&p.codex_inputs {
                crewd_store::codex_inputs::consume(tx.conn(),&self.input_owner(),&p.crew_id,session,&self.request.request_id,inputs)?;
            }
        }
        Ok(receipts::record_registered(
            tx.conn(),
            &self.principal,
            &self.request.db_id,
            &self.request.request_id,
            session,
            &tx.now_ts(),
        )?)
    }
    pub(crate) fn owned_runtime_birth(
        &self,
        session: &str,
        pid: u32,
    ) -> Option<crewd_exec::registry::ProcessIdentity> {
        self.state
            .upgrade()?
            .peer_policy
            .get()?
            .owned_runtime_birth(session, pid)
    }

    pub(crate) async fn admit(&self) -> Result<Admission, RpcError> {
        let state = self.state()?;
        let call = self.clone();
        let owner = state.clone();
        state
            .collab(move |tx| {
                for method in call.request.command.authorized_methods() {
                    call.actor.authorize_tx(&owner, tx.conn(), method)?;
                }
                let existing = receipts::lookup(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                )?;
                if existing.is_none() && call.request.runtime_binding_id != owner.runtime_binding_id
                {
                    return Err(crewd_collab::CollabError::Conflict(
                        crewd_collab::refusal::Refusal::new(
                            crewd_collab::refusal::Code::ResourceRequestConflict,
                        )
                        .arg("request", &call.request.request_id),
                    ));
                }
                match receipts::reserve(tx.conn(), &call.principal, &call.request, &tx.now_ts())
                    .map_err(|e| reserve_error(e, &call.request.request_id))?
                {
                    Reserved::Existing(mut receipt) => {
                        receipt.historical |=
                            receipt.runtime_binding_id != owner.runtime_binding_id;
                        Ok(Admission::Existing(receipt))
                    }
                    Reserved::New(_) => Ok(Admission::New(receipts::transition(
                        tx.conn(),
                        &call.principal,
                        &call.request.db_id,
                        &call.request.request_id,
                        (CommandPhase::Reserved, CommandPhase::Admitted),
                        None,
                        &tx.now_ts(),
                    )?)),
                }
            })
            .await
            .map_err(dispatch::collab_err)
    }
    pub(crate) async fn codex_native_ready(&self) -> Result<bool,RpcError> {
        let Some(target)=self.request.command.codex_target() else {return Ok(false)};
        let session=target.session_id.clone();
        let action=self.request.command.native_history_action();
        let state=self.state()?;let owner=state.clone();let actor=self.actor.clone();let method=self.request.command.method();
        state.db.call_result(move |s| {
            actor.authorize_tx(&owner,s.conn(),method)?;
            Ok::<_,crewd_collab::CollabError>(crewd_store::codex_controls::native_ready(s.conn(),&session,action)?)
        }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)
    }
    /// Last durable authorization before dispatch. Lookup/duplicate never calls it.
    pub(crate) async fn dispatch(&self) -> Result<PublicCommandReceipt, RpcError> {
        let state = self.state()?;
        let call = self.clone();
        let owner = state.clone();
        state
            .collab(move |tx| {
                for method in call.request.command.authorized_methods() {
                    call.actor.authorize_tx(&owner, tx.conn(), method)?;
                }
                if let Some(target)=call.request.command.codex_target() {
                    let action=call.request.command.native_history_action();
                    if !crewd_store::codex_controls::native_ready(tx.conn(),&target.session_id,action)? {
                        return Err(crewd_collab::resource_invalid("codex.nativeHistoryNotReady"));
                    }
                    if let Some(action)=action {
                        crewd_store::codex_controls::history_intent(tx.conn(),&target.session_id,&call.request.request_id,action)?;
                        crewd_store::notifications::suppress_session(tx.conn(),&target.session_id,"native_history_changed")?;
                    }
                }
                if let PublicCommand::CodexQueue(p)|PublicCommand::CodexSteer(p)=&call.request.command {
                    if let Some(inputs)=&p.inputs {
                        let session=crewd_collab::sessions::get(tx.conn(),&p.target.session_id)?;
                        crewd_store::codex_inputs::consume(tx.conn(),&call.input_owner(),&session.crew_id,&session.id,&call.request.request_id,inputs)?;
                    }
                }
                if let PublicCommand::CodexConfigure(p)=&call.request.command {
                    crewd_store::codex_controls::configure_intent(tx.conn(),&p.target.session_id,&p.patch,&call.request.request_id)?;
                }
                if let PublicCommand::CodexTitleSet(p)=&call.request.command {
                    let title=crewd_collab::sessions::resolve_title(Some(&p.title),None)?;
                    let mut m=crewd_store::codex_controls::get(tx.conn(),&p.target.session_id)?;
                    m.title.desired=Some(title.clone());m.title.source=crewd_proto::codex::CodexTitleSource::Human;
                    m.title.status=crewd_proto::codex::CodexTitleStatus::Pending;m.title_attempted=true;m.title_intent_observation=m.name_revision;m.title_request=Some(call.request.request_id.clone());
                    crewd_store::codex_controls::put(tx.conn(),&p.target.session_id,&m)?;
                    tx.conn().execute("UPDATE sessions SET title=?2,updated_at=?3 WHERE id=?1",rusqlite::params![p.target.session_id,title,tx.now_ts()])?;
                    tx.note(crewd_collab::Change::SessionChanged{session_id:p.target.session_id.clone()});
                }
                let result=if let PublicCommand::CodexFork(p)=&call.request.command {
                    let original=crewd_collab::sessions::get(tx.conn(),&p.target.session_id)?;
                    let fork=crewd_collab::sessions::register(tx,crewd_collab::sessions::NewSession{
                        crew_id:original.crew_id,role:crewd_proto::Role::Worker,executor:"codex".into(),
                        title:p.title.clone().or(original.title),brief:Some("Explicit conversation fork; no task submitted".into()),
                        working_directory:original.worktree.or(original.working_directory),model:original.model,effort:original.effort,..Default::default()
                    })?;
                    crewd_store::codex_controls::initial(tx.conn(),&fork.id,None,p.title.is_some())?;
                    let mut m=crewd_store::codex_controls::get(tx.conn(),&fork.id)?;m.fork_pending=true;crewd_store::codex_controls::put(tx.conn(),&fork.id,&m)?;
                    crewd_collab::wakes::human_stop(tx,&fork.id)?;
                    Some(serde_json::json!({"newSessionId":fork.id}))
                } else if let PublicCommand::SessionStop(params)=&call.request.command {
                    // Stop intent/floor and receipt dispatch are ONE authority
                    // transaction. A failed floor cannot admit an OS stop.
                    crewd_collab::wakes::human_stop(tx,&params.target.session_id)?;
                    Some(serde_json::json!({"stopIntentPersisted":true,"physicalExit":"unconfirmed"}))
                }else{None};
                Ok(receipts::transition(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                    (CommandPhase::Admitted, CommandPhase::Dispatched),
                    result.as_ref(),
                    &tx.now_ts(),
                )?)
            })
            .await
            .map_err(dispatch::collab_err)
    }
    pub(crate) async fn recheck_dispatched(&self) -> Result<(), RpcError> {
        let owner = self.state()?;
        let call = self.clone();
        let state = owner.clone();
        owner
            .collab(move |tx| {
                for method in call.request.command.authorized_methods() {
                    call.actor.authorize_tx(&state, tx.conn(), method)?;
                }
                let receipt = receipts::lookup(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                )?
                .ok_or_else(|| crewd_store::StoreError::Invalid("public dispatch absent".into()))?;
                if receipt.phase != CommandPhase::Dispatched
                    || receipt.historical
                    || call.request.runtime_binding_id != state.runtime_binding_id
                {
                    return Err(crewd_store::StoreError::Invalid(
                        "public dispatch no longer current".into(),
                    )
                    .into());
                }
                Ok(())
            })
            .await
            .map_err(dispatch::collab_err)
    }
    /// Observations are recorded in the already-authenticated namespace even if
    /// the grant is now revoked; disclosure separately rechecks current rights.
    pub(crate) async fn settle(
        &self,
        from: CommandPhase,
        to: CommandPhase,
        result: Value,
    ) -> Result<PublicCommandReceipt, RpcError> {
        let state = self.state()?;
        let call = self.clone();
        state
            .collab(move |tx| {
                let prior = receipts::lookup(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                )?;
                let mut result = result;
                if let (Some(previous), Some(next)) = (
                    prior
                        .and_then(|receipt| receipt.result)
                        .and_then(|value| value.as_object().cloned()),
                    result.as_object_mut(),
                ) {
                    for (key, value) in previous {
                        next.entry(key).or_insert(value);
                    }
                }
                if call.request.command.native_history_action().is_some() {
                    let target=call.request.command.codex_target().expect("closed history target");
                    crewd_store::codex_controls::settle_history(tx.conn(),&target.session_id,&call.request.request_id,to)?;
                }
                if let PublicCommand::CodexConfigure(p) = &call.request.command {
                    let mut m = crewd_store::codex_controls::get(tx.conn(), &p.target.session_id)?;
                    if m.intent_request.as_deref() == Some(call.request.request_id.as_str()) {
                        m.settings_status = Some(
                            if to == CommandPhase::Confirmed {
                                "accepted"
                            } else if to == CommandPhase::Rejected {
                                "rejected"
                            } else {
                                "unknown"
                            }
                            .into(),
                        );
                        crewd_store::codex_controls::put(tx.conn(), &p.target.session_id, &m)?;
                    }
                }
                if let PublicCommand::CodexTitleSet(p) = &call.request.command {
                    let mut m = crewd_store::codex_controls::get(tx.conn(), &p.target.session_id)?;
                    if m.title_request.as_deref() == Some(call.request.request_id.as_str()) {
                        m.title.status = if to == CommandPhase::Confirmed {
                            crewd_proto::codex::CodexTitleStatus::Synced
                        } else if to == CommandPhase::Rejected {
                            crewd_proto::codex::CodexTitleStatus::Rejected
                        } else {
                            crewd_proto::codex::CodexTitleStatus::Unknown
                        };
                        if to == CommandPhase::Confirmed {
                            if m.name_revision == m.title_intent_observation {
                                m.title.native = m.title.desired.clone();
                            } else if m.title.native != m.title.desired {
                                m.title.status = crewd_proto::codex::CodexTitleStatus::Unknown;
                            }
                        }
                        crewd_store::codex_controls::put(tx.conn(), &p.target.session_id, &m)?;
                    }
                }
                if matches!(call.request.command, PublicCommand::CodexFork(_))
                    && to == CommandPhase::Confirmed
                {
                    let id = result
                        .get("newSessionId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            crewd_store::StoreError::Invalid("fork Core identity absent".into())
                        })?;
                    let thread = result
                        .get("nativeThreadId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            crewd_store::StoreError::Invalid("fork native identity absent".into())
                        })?;
                    crewd_store::sessions::set_agent_session_id(tx.conn(), id, thread)?;
                    let mut m = crewd_store::codex_controls::get(tx.conn(), id)?;
                    m.fork_pending = false;
                    crewd_store::codex_controls::put(tx.conn(), id, &m)?;
                }
                Ok(receipts::transition(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                    (from, to),
                    Some(&result),
                    &tx.now_ts(),
                )?)
            })
            .await
            .map_err(dispatch::collab_err)
    }
}
