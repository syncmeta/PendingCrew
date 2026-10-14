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
                        CommandPhase::Reserved,
                        CommandPhase::Admitted,
                        None,
                        &tx.now_ts(),
                    )?)),
                }
            })
            .await
            .map_err(dispatch::collab_err)
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
                let result=if let PublicCommand::SessionStop(params)=&call.request.command {
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
                    CommandPhase::Admitted,
                    CommandPhase::Dispatched,
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
                Ok(receipts::transition(
                    tx.conn(),
                    &call.principal,
                    &call.request.db_id,
                    &call.request.request_id,
                    from,
                    to,
                    Some(&result),
                    &tx.now_ts(),
                )?)
            })
            .await
            .map_err(dispatch::collab_err)
    }
}
