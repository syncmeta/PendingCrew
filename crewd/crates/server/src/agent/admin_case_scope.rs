//! Narrow collaboration-tool authority for a bound administrative case.
//! This is not a sandbox for an executor's native filesystem or shell tools.
use super::Identity;
use crate::{AppState, connection_scope::ConnectionScope};
use crewd_collab::{CollabError, Result, Tx, admin_case_execution as cases};
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Authority {
    state: Arc<AppState>,
    identity: Identity,
    connection: ConnectionScope,
    token: String,
    witness: cases::ExecutionWitness,
    tool: String,
    args: Value,
    work: String,
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
            || !self
                .state
                .admission
                .contains(self.witness.source_session_id())
        {
            return Err(denied());
        }
        let actual = crewd_collab::sessions::get(c, &self.identity.session_id)?;
        if actual.crew_id != self.identity.crew_id
            || crewd_collab::sessions::role_of(c, &actual.id)? != self.identity.role
            || self.witness.session_id() != actual.id
            || self.witness.crew_id() != actual.crew_id
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) fn validate_read(&self, c: &Connection) -> Result<()> {
        self.validate_actor(c)?;
        cases::validate_read(
            c,
            &self.witness,
            cases::ReadScope::Work {
                id: self.work.clone(),
            },
            crewd_store::now_ms(),
        )
    }
    pub(crate) fn effect(&self, tx: &mut Tx<'_>) -> Result<cases::EffectPermit> {
        self.validate_actor(tx.conn())?;
        let action = match self.tool.as_str() {
            "work_item_close" => cases::ActionScope::WorkClose {
                id: self.work.clone(),
                kind: serde_json::from_value(self.args.get("kind").cloned().ok_or_else(denied)?)
                    .map_err(|_| denied())?,
            },
            "work_item_reassign_lost" => cases::ActionScope::WorkReassignLost {
                id: self.work.clone(),
                new_owner: self
                    .args
                    .get("owner")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?
                    .into(),
            },
            _ => return Err(denied()),
        };
        cases::validate_effect(tx, &self.witness, action)
    }
}
pub(crate) async fn bound(st: &AppState, session: &str) -> Result<bool> {
    let session = session.to_owned();
    st.db
        .call_result(move |s| cases::session_binding(s.conn(), &session).map(|b| b.is_some()))
        .await?
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
    let witness = st
        .db
        .call_result(move |s| cases::session_binding(s.conn(), &id))
        .await??;
    let Some(witness) = witness else {
        return Ok(None);
    };
    let a = Authority {
        state: st.clone(),
        identity: me.clone(),
        connection,
        token,
        witness,
        tool: tool.into(),
        args: args.clone(),
        work: work(tool, args)?,
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
    } else if bound(st, &me.session_id).await? {
        return Err(denied());
    }
    Ok(())
}
