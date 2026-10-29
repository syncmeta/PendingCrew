//! Explicit disposition of uncertain managed resumes. Never changes their receipts.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{RpcError, claude::*, commands::*};
use crewd_store::command_receipts as receipts;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) fn refused() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "original Claude recovery evidence changed or unavailable",
    )
    .with_refusal("claude.recovery_evidence_unavailable")
}
fn gone(pid: i32, start: u64, boot: &str) -> bool {
    use crewd_exec::registry::{ProbeResult, probe};
    match probe(pid) {
        ProbeResult::Missing => true,
        ProbeResult::Found(p) => p.start_time != start || p.boot_id != boot,
        ProbeResult::Unreadable(_) => false,
    }
}
// Read-only and bounded. Foreign-principal blockers are never disclosed.
pub(crate) fn snapshot(
    c: &rusqlite::Connection,
    state: &Arc<AppState>,
    principal: &str,
    target: &CommandRuntimeTarget,
    running: bool,
    exclude: Option<&str>,
) -> Result<Value, RpcError> {
    let sid = &target.session_id;
    let row = crewd_collab::sessions::get(c, sid).map_err(dispatch::collab_err)?;
    if row.executor != "claude" {
        return Err(refused());
    }
    let crew = crewd_collab::crews::get(c, &row.crew_id).map_err(dispatch::collab_err)?;
    let native = row
        .agent_session_id
        .filter(|s| crewd_exec::driver::exact_managed_id(s));
    let mut q=c.prepare("SELECT principal,request_id,runtime_binding_id,phase,historical,payload_json,result_json FROM public_command_receipts WHERE ((operation='session.resume' AND phase IN ('reserved','admitted','dispatched','unknown')) OR (operation='session.claude.recoveryResolve' AND phase='committed' AND historical=0 AND principal=?2 AND json_extract(payload_json,'$.params.originalAgentSessionId')=?3)) AND COALESCE(json_extract(payload_json,'$.params.target.sessionId'),json_extract(payload_json,'$.params.sessionId'))=?1 ORDER BY request_id LIMIT 257").map_err(dispatch::internal)?;
    let rows = q
        .query_map(rusqlite::params![sid, principal, native], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(dispatch::internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(dispatch::internal)?;
    let mut covered = BTreeSet::new();
    let mut dispositions = Vec::new();
    for (owner, id, _, phase, historical, payload, _) in &rows {
        let p: Value = serde_json::from_str(payload).map_err(dispatch::internal)?;
        if owner == principal
            && phase == "committed"
            && !historical
            && p["operation"] == "session.claude.recoveryResolve"
            && p["params"]["originalAgentSessionId"].as_str() == native.as_deref()
        {
            let ids = p["params"]["originalRequestIds"]
                .as_array()
                .ok_or_else(refused)?;
            for id in ids {
                covered.insert(id.as_str().ok_or_else(refused)?.to_owned());
            }
            dispositions.push(json!({"requestId":id,"originalRequestIds":ids}));
        }
    }
    let mut unresolved = Vec::new();
    let mut foreign = false;
    let mut evidence = Vec::new();
    for (owner, id, binding, phase, historical, payload, result) in &rows {
        let p: Value = serde_json::from_str(payload).map_err(dispatch::internal)?;
        if p["operation"] != "session.resume"
            || (owner == principal && exclude == Some(id.as_str()))
            || (owner == principal && covered.contains(id))
        {
            continue;
        }
        if !matches!(
            phase.as_str(),
            "reserved" | "admitted" | "dispatched" | "unknown"
        ) {
            continue;
        }
        if owner != principal || phase != "unknown" || *historical {
            foreign = true;
            continue;
        }
        unresolved.push(json!({"requestId":id,"runtimeBindingId":binding,"phase":phase,"historical":*historical || binding!=&state.runtime_binding_id}));
        evidence.push(json!([
            owner, id, binding, phase, historical, payload, result
        ]));
    }
    let mut births = Vec::new();
    let mut all_gone = true;
    let mut q=c.prepare("SELECT native_pid,native_start,native_boot FROM native_execution_runtimes WHERE session_id=?1 ORDER BY runtime_id").map_err(dispatch::internal)?;
    for birth in q
        .query_map([sid], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(dispatch::internal)?
    {
        let (pid, start, boot) = birth.map_err(dispatch::internal)?;
        all_gone &= gone(pid, start as u64, &boot);
        births.push(json!([pid, start, boot]));
    }
    let configured = state.native_helpers.get().is_some()
        && state.orchestra.get().is_some_and(|h| {
            h.executor_profiles()
                .iter()
                .any(|p| p.id == "claude" && p.available)
        });
    if let Some(f) = state.native_helpers.get() {
        let mut entries = f.registry.try_entries().map_err(|_| refused())?;
        entries.extend(
            f.registry
                .authority_snapshot()
                .map_err(|_| refused())?
                .spawned,
        );
        for (_, b) in entries.iter().filter(|(s, _)| s == sid) {
            all_gone &= gone(b.pid, b.start_time, &b.boot_id);
            births.push(json!([b.pid, b.start_time, b.boot_id]));
        }
    } else {
        all_gone = false
    }
    let work = row.worktree.or(row.working_directory);
    let eligibility = if running {
        "running"
    } else if crew.hidden_at.is_some() {
        "removedCrew"
    } else if work
        .as_ref()
        .is_none_or(|p| !std::path::Path::new(p).is_dir())
    {
        "missingWorkdir"
    } else if native.is_none() {
        "missingOriginalId"
    } else if !configured {
        "notConfigured"
    } else if !all_gone {
        "previousBirthLiveOrUnknown"
    } else if foreign || rows.len() > 256 {
        "evidenceUnavailable"
    } else if unresolved.is_empty() {
        "noUnresolvedRequests"
    } else {
        "eligible"
    };
    let token = if eligibility == "eligible" {
        let bytes = serde_json::to_vec(&json!([
            state.runtime_binding_id,
            target,
            native,
            work,
            evidence,
            births,
            dispositions
        ]))
        .map_err(dispatch::internal)?;
        Some(
            ring::digest::digest(&ring::digest::SHA256, &bytes)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
    } else {
        None
    };
    Ok(
        json!({"sessionId":sid,"originalAgentSessionId":native,"target":target,"eligibility":eligibility,"evidenceToken":token,"unresolved":unresolved,"dispositions":dispositions}),
    )
}
pub(crate) async fn inspect(
    state: &Arc<AppState>,
    actor: &Actor,
    p: Value,
) -> Result<Value, RpcError> {
    actor.authorize(state, "session.inspect").await?;
    let p: ClaudeSessionParams = crewd_proto::parse_params(p)?;
    let h = state.orchestra.get().ok_or_else(refused)?;
    let target = h
        .public_target(&p.session_id)
        .await
        .map_err(crate::orchestra::rpc::rpc_err)?;
    let running = h
        .live(&p.session_id)
        .await
        .map_err(crate::orchestra::rpc::rpc_err)?
        .running;
    let principal = actor.principal(state).await?;
    let actor = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |s| {
            actor
                .authorize_tx(&owner, s.conn(), "session.inspect")
                .map_err(dispatch::collab_err)?;
            snapshot(s.conn(), &owner, &principal, &target, running, None)
        })
        .await
        .map_err(dispatch::internal)?
}

pub(crate) async fn resolve(
    call: crate::public_execution::PublicExecution,
    target_current: bool,
    running: bool,
) -> Result<Value, RpcError> {
    let PublicCommand::ClaudeRecoveryResolve(p) = call.request.command.clone() else {
        return Err(refused());
    };
    let state = call.state()?;
    let owner = state.clone();
    state.db.call(move|s| -> Result<Value,RpcError>{
        call.actor().authorize_tx(&owner,s.conn(),"session.resume").map_err(dispatch::collab_err)?;
        let tx=s.conn().unchecked_transaction().map_err(dispatch::internal)?;
        // Idempotent return precedes freshness checks; never redo an old disposition.
        if let Some(mut old)=receipts::lookup(&tx,call.principal(),&call.request.db_id,&call.request.request_id).map_err(dispatch::internal)? {
            if old.command!=call.request.command || old.runtime_binding_id!=call.request.runtime_binding_id{return Err(refused())}
            old.historical |= old.runtime_binding_id!=owner.runtime_binding_id;
            return serde_json::to_value(old).map_err(dispatch::internal)
        }
        if !target_current || call.request.runtime_binding_id!=owner.runtime_binding_id{return Err(refused())}
        let evidence=snapshot(&tx,&owner,call.principal(),&p.target,running,None)?;
        let ids:Vec<_>=evidence["unresolved"].as_array().ok_or_else(refused)?.iter().filter_map(|r|r["requestId"].as_str().map(str::to_owned)).collect();
        if evidence["eligibility"]!="eligible" || evidence["evidenceToken"]!=p.evidence_token || evidence["originalAgentSessionId"]!=p.original_agent_session_id || ids!=p.original_request_ids{return Err(refused())}
        let now=crewd_store::now_ts();
        if !matches!(receipts::reserve(&tx,call.principal(),&call.request,&now).map_err(dispatch::internal)?,receipts::Reserved::New(_)){return Err(refused())}
        let result=json!({"dispositionRecorded":true,"sessionId":p.target.session_id,"originalRequestIds":ids,"originalOutcome":"unknown","effectsDispatched":false});
        let receipt=receipts::transition(&tx,call.principal(),&call.request.db_id,&call.request.request_id,(CommandPhase::Reserved,CommandPhase::Committed),Some(&result),&now).map_err(dispatch::internal)?;
        tx.commit().map_err(dispatch::internal)?;
        serde_json::to_value(receipt).map_err(dispatch::internal)
    }).await.map_err(dispatch::internal)?
}

/// Final executor gate also covers legacy and wake-driven launch paths.
pub(crate) fn unresolved_resume(
    c: &rusqlite::Connection,
    session: &str,
    native: &str,
) -> crewd_store::Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM public_command_receipts r WHERE r.operation='session.resume' AND r.phase='unknown' AND COALESCE(json_extract(r.payload_json,'$.params.target.sessionId'),json_extract(r.payload_json,'$.params.sessionId'))=?1 AND NOT EXISTS(SELECT 1 FROM public_command_receipts d,json_each(d.payload_json,'$.params.originalRequestIds') i WHERE d.operation='session.claude.recoveryResolve' AND d.phase='committed' AND d.historical=0 AND d.principal=r.principal AND json_extract(d.payload_json,'$.params.target.sessionId')=?1 AND json_extract(d.payload_json,'$.params.originalAgentSessionId')=?2 AND i.value=r.request_id))",rusqlite::params![session,native],|r|r.get(0))?)
}
