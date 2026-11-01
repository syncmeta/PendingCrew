//! Narrow collaboration-tool authority for a bound administrative case.
//! This is not a sandbox for an executor's native filesystem or shell tools.
use super::Identity;
use crate::{AppState, connection_scope::ConnectionScope};
use crewd_collab::{
    CollabError, Result, Tx, admin_case_execution as cases, admin_case_handoff as handoff,
};
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Authority {
    state: Arc<AppState>,
    identity: Identity,
    connection: ConnectionScope,
    token: String,
    scope: Scope,
    tool: String,
    args: Value,
}
#[derive(Clone)]
enum Scope {
    Work {
        witness: cases::ExecutionWitness,
        id: String,
    },
    Handoff(handoff::Operation),
}

pub(crate) enum MutationPermit {
    Work(cases::EffectPermit),
    HandoffRequest(handoff::RequestPermit),
    HandoffAccept(handoff::AcceptPermit),
}
impl MutationPermit {
    pub(crate) fn finish(self, tx: &mut Tx<'_>) -> Result<()> {
        match self {
            Self::Work(p) => cases::finish_effect(tx, p),
            Self::HandoffRequest(p) => handoff::finish_request(tx, p),
            Self::HandoffAccept(p) => handoff::finish_accept(tx, p),
        }
    }
}
fn handoff_tool(tool: &str) -> bool {
    matches!(
        tool,
        "admin_case_handoff_request"
            | "admin_case_handoff_get"
            | "admin_case_handoff_accept"
            | "admin_case_handoff_list"
    )
}
fn argument(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(denied)
}
fn handoff_operation(tool: &str, args: &Value) -> Result<handoff::Operation> {
    if tool == "admin_case_handoff_list" {
        let after = match args.get("after") {
            None | Some(Value::Null) => None,
            Some(Value::String(v)) => Some(v.clone()),
            _ => return Err(denied()),
        };
        let limit = match args.get("limit") {
            None | Some(Value::Null) => 50,
            Some(v) => v
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(denied)?,
        };
        return Ok(handoff::Operation::List { after, limit });
    }
    let request_id = argument(args, "request_id")?;
    match tool {
        "admin_case_handoff_request" => Ok(handoff::Operation::Request {
            request_id,
            reason: argument(args, "reason")?,
        }),
        "admin_case_handoff_get" => Ok(handoff::Operation::Get { request_id }),
        "admin_case_handoff_accept" => Ok(handoff::Operation::Accept {
            request_id,
            expected_revision: argument(args, "expected_revision")?,
            note: argument(args, "note")?,
        }),
        _ => Err(denied()),
    }
}
tokio::task_local! {static AUTHORITY: Authority;}
pub(crate) fn capture() -> Option<Authority> {
    AUTHORITY.try_with(Clone::clone).ok()
}
pub(crate) fn denied() -> CollabError {
    CollabError::Forbidden(crewd_collab::refusal::Refusal::new(
        crewd_collab::refusal::Code::ResourceForbidden,
    ))
}
pub(crate) fn allowed_tool(tool: &str) -> bool {
    matches!(
        tool,
        "work_item_get"
            | "work_item_history"
            | "work_closure_history"
            | "work_item_close"
            | "work_item_reassign_lost"
            | "admin_case_handoff_request"
            | "admin_case_handoff_get"
    )
}
fn work(tool: &str, args: &Value) -> Result<String> {
    if !allowed_tool(tool) {
        return Err(denied());
    }
    args.get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(denied)
}
impl Authority {
    fn validate_actor(&self, c: &Connection) -> Result<()> {
        if !self.connection.is_live()
            || !self
                .state
                .tokens
                .verify(&self.identity.session_id, &self.token)
            || !self.state.admission.contains(&self.identity.session_id)
        {
            return Err(denied());
        }
        let actual = crewd_collab::sessions::get(c, &self.identity.session_id)?;
        if actual.crew_id != self.identity.crew_id
            || crewd_collab::sessions::role_of(c, &actual.id)? != self.identity.role
        {
            return Err(denied());
        }
        if let Scope::Work { witness, .. } = &self.scope
            && (!self.state.admission.contains(witness.source_session_id())
                || witness.session_id() != actual.id
                || witness.crew_id() != actual.crew_id)
        {
            return Err(denied());
        }
        Ok(())
    }
    fn participant_admission(&self, source: &str, admin: &str, captain: &str) -> Result<()> {
        if [source, admin, captain]
            .into_iter()
            .all(|id| self.state.admission.contains(id))
        {
            Ok(())
        } else {
            Err(denied())
        }
    }
    pub(crate) fn validate_read(&self, c: &Connection) -> Result<()> {
        self.validate_actor(c)?;
        match &self.scope {
            Scope::Work { witness, id } => cases::validate_read(
                c,
                witness,
                cases::ReadScope::Work { id: id.clone() },
                crewd_store::now_ms(),
            ),
            Scope::Handoff(operation) => {
                let p = handoff::validate_operation(
                    c,
                    &crewd_collab::Sender::Session(self.identity.session_id.clone()),
                    operation,
                    crewd_store::now_ms(),
                )?;
                // Historical reads remain available after the executor has parked.
                if !matches!(
                    operation,
                    handoff::Operation::Get { .. } | handoff::Operation::List { .. }
                ) {
                    self.participant_admission(
                        p.source_session_id(),
                        p.admin_session_id(),
                        p.captain_session_id(),
                    )?;
                }
                Ok(())
            }
        }
    }
    pub(crate) fn effect(&self, tx: &mut Tx<'_>) -> Result<MutationPermit> {
        self.validate_actor(tx.conn())?;
        match &self.scope {
            Scope::Handoff(operation) => {
                let by = crewd_collab::Sender::Session(self.identity.session_id.clone());
                match operation {
                    handoff::Operation::Request { request_id, reason } => {
                        let p = handoff::begin_request(tx, &by, request_id, reason)?;
                        self.participant_admission(
                            p.source_session_id(),
                            p.admin_session_id(),
                            p.captain_session_id(),
                        )?;
                        Ok(MutationPermit::HandoffRequest(p))
                    }
                    handoff::Operation::Accept {
                        request_id,
                        expected_revision,
                        note,
                    } => {
                        let p =
                            handoff::begin_accept(tx, &by, request_id, expected_revision, note)?;
                        self.participant_admission(
                            p.source_session_id(),
                            p.admin_session_id(),
                            p.captain_session_id(),
                        )?;
                        Ok(MutationPermit::HandoffAccept(p))
                    }
                    handoff::Operation::Get { .. } | handoff::Operation::List { .. } => {
                        Err(denied())
                    }
                }
            }
            Scope::Work { witness, id } => {
                let action = match self.tool.as_str() {
                    "work_item_close" => cases::ActionScope::WorkClose {
                        id: id.clone(),
                        kind: serde_json::from_value(
                            self.args.get("kind").cloned().ok_or_else(denied)?,
                        )
                        .map_err(|_| denied())?,
                    },
                    "work_item_reassign_lost" => cases::ActionScope::WorkReassignLost {
                        id: id.clone(),
                        new_owner: argument(&self.args, "owner")?,
                    },
                    _ => return Err(denied()),
                };
                cases::validate_effect(tx, witness, action).map(MutationPermit::Work)
            }
        }
    }
}
pub(crate) async fn bound(st: &AppState, session: &str) -> Result<bool> {
    if st.tokens.is_admin_case(session) {
        return Ok(true);
    }
    let id = session.to_owned();
    let bound = st
        .db
        .call_result(move |s| cases::session_binding(s.conn(), &id).map(|b| b.is_some()))
        .await??;
    if bound {
        st.tokens.mark_admin_case(session);
    }
    Ok(bound || st.tokens.is_admin_case(session))
}

pub(crate) async fn prepare(
    st: &Arc<AppState>,
    me: &Identity,
    connection: ConnectionScope,
    token: String,
    tool: &str,
    args: &Value,
) -> Result<Option<Authority>> {
    let id = me.session_id.clone();
    let state = st.clone();
    let witness = st
        .db
        .call_result(move |s| {
            let binding = cases::session_binding(s.conn(), &id)?;
            if binding.is_some() {
                state.tokens.mark_admin_case(&id);
            } else if state.tokens.is_admin_case(&id) {
                return Err(denied());
            }
            Ok(binding)
        })
        .await??;
    let scope = if handoff_tool(tool) {
        // Even an ordinary Captain needs an authenticated RPC envelope for handoff.
        Scope::Handoff(handoff_operation(tool, args)?)
    } else if let Some(witness) = witness {
        Scope::Work {
            witness,
            id: work(tool, args)?,
        }
    } else {
        return Ok(None);
    };
    let a = Authority {
        state: st.clone(),
        identity: me.clone(),
        connection,
        token,
        scope,
        tool: tool.into(),
        args: args.clone(),
    };
    let checked = a.clone();
    st.db
        .call_result(move |s| checked.validate_read(s.conn()))
        .await??;
    Ok(Some(a))
}
pub(crate) async fn within<F: std::future::Future>(a: Authority, f: F) -> F::Output {
    AUTHORITY.scope(a, f).await
}
/// Direct internal tools::call must not bypass the authenticated RPC envelope.
pub(crate) async fn check_call(
    st: &AppState,
    me: &Identity,
    tool: &str,
    args: &Value,
) -> Result<()> {
    if let Some(a) = capture() {
        if !std::ptr::eq(a.state.as_ref(), st)
            || a.identity != *me
            || a.tool != tool
            || a.args != *args
        {
            return Err(denied());
        }
        st.db
            .call_result(move |s| a.validate_read(s.conn()))
            .await??;
    } else if handoff_tool(tool) || bound(st, &me.session_id).await? {
        return Err(denied());
    }
    Ok(())
}
