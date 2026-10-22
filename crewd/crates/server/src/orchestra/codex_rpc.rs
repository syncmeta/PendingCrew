//! Read-only Codex projections. Stopped threads are never resumed for a read.
use super::{Handle,rpc::rpc_err};
use crate::{AppState,dispatch};
use crewd_exec::codex::control::{CodexOperation as Op,CodexOutcome as Out};
use crewd_proto::{codex::*,orchestra::SessionIdParams,parse_params,RpcError};
use serde_json::{Value,json};

pub(crate) fn handles(method:&str)->bool{[INSPECT,HISTORY,MODELS,TOOLS].contains(&method)}
pub(crate) async fn read(h:&Handle,state:&AppState,method:&str,params:Value)->Result<Value,RpcError> {
    let (sid,history)=if method==HISTORY{let p:CodexHistoryParams=parse_params(params)?;(p.session_id.clone(),Some(p))}else{let p:SessionIdParams=parse_params(params)?;(p.session_id,None)};
    let id=sid.clone();
    let (row,meta,workdir)=state.db.call_result(move|store|{
        let c=store.conn();
        let row=crewd_collab::sessions::get(c,&id)?;
        let crew=crewd_collab::crews::get(c,&row.crew_id)?;
        let work=row.worktree.clone().or(row.working_directory.clone()).or(crew.working_directory);
        let meta=crewd_store::codex_controls::get(c,&id)?;
        Ok((row,meta,work))
    }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)?;
    if row.executor!="codex"{return Err(RpcError::new(RpcError::CONFLICT,"session is not a Codex source"));}
    let live=h.live(&sid).await.map_err(rpc_err)?;
    let handle=h.codex_handle(&sid).await.map_err(rpc_err)?;
    if method==INSPECT {
        let local=match handle.as_ref(){Some(r)=>r.codex(Op::Inspect).await.ok(),None=>None};
        let queued=match local {Some(Out::Observed(v))=>v.get("queuedInputs").and_then(Value::as_u64).unwrap_or(0) as u32,_=>0};
        let pending=h.runner_request(&sid).await.map_err(rpc_err)?.map(|r| {
            let request_id=live.pending_decision.as_ref().and_then(|d|match &d.kind{crewd_term::DecisionKind::RunnerRequest{request_id}=>Some(request_id.clone()),_=>None}).unwrap_or_default();
            crewd_proto::orchestra::RunnerRequestView{runner_request_id:request_id,fingerprint:r.fingerprint,method:r.method,params:r.params}
        });
        let capabilities=[("configure",true,true),("compact",true,false),("steer",true,false),("queue",true,false),("interrupt",true,false),("review",true,false),("history",false,false),("resume",false,false),("fork",false,false),("title",false,false),("models",false,false),("tools",false,false),("attachments",false,false),("skillInput",false,false)].into_iter().map(|(id,needs_run,experimental)|CodexCapability{id:id.into(),availability:if matches!(id,"attachments"|"skillInput"){CodexCapabilityAvailability::PcwNotImplemented}else if needs_run&&!live.running{CodexCapabilityAvailability::RequiresRunning}else{CodexCapabilityAvailability::Available},protocol:if experimental{CodexProtocol::Experimental}else{CodexProtocol::Stable},detail:if matches!(id,"attachments"|"skillInput"){Some("native input exists; trusted PCW attachment/skill transport remains a separate unfinished delivery unit".into())}else{None}}).collect();
        return serde_json::to_value(CodexSessionInspection{session_id:sid.clone(),thread_id:row.agent_session_id,target:h.public_target(&sid).await.map_err(rpc_err)?,running:live.running,working:live.working,queued_inputs:queued,settings:meta.settings,settings_observed_at:meta.settings_observed_at,historical:meta.observed_binding.as_deref()!=Some(&state.runtime_binding_id),title:meta.title,capabilities,pending_request:pending,desired_settings:meta.desired_settings,settings_status:meta.settings_status}).map_err(dispatch::internal);
    }
    let thread=row.agent_session_id.filter(|s|!s.is_empty()).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native thread identity missing"))?;
    let op=match method{HISTORY=>Op::History(history.unwrap()),MODELS=>Op::Models,TOOLS=>Op::Tools,_=>unreachable!()};
    let outcome=match handle {
        Some(r)=>r.codex(op).await.map_err(|_|RpcError::new(RpcError::CONFLICT,"Codex runtime closed"))?,
        None=>h.codex_metadata(crewd_exec::codex::metadata::MetadataRequest{session_id:sid.clone(),thread_id:thread.clone(),workdir:workdir.ok_or_else(||RpcError::new(RpcError::CONFLICT,"original workspace unavailable"))?.into(),operation:op}).await,
    };
    let value=match outcome{Out::Observed(v)=>v,Out::Rejected(e)=>return Err(RpcError::new(RpcError::CONFLICT,e).with_refusal("codex.provider_rejected")),Out::Unknown{detail,..}=>return Err(RpcError::new(RpcError::CONFLICT,detail).with_refusal("codex.read_unconfirmed"))};
    if serde_json::to_vec(&value).map_err(dispatch::internal)?.len()>4*1024*1024{return Err(RpcError::new(RpcError::CONFLICT,"Codex metadata page exceeds 4 MiB response bound"));}
    let native=value.get("providerResult").unwrap_or(&value);
    let result=match method {
        HISTORY=>serde_json::to_value(CodexHistoryPage{session_id:sid,thread_id:thread,items:native.get("data").and_then(Value::as_array).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native history response has no page"))?.clone(),next_cursor:native.get("nextCursor").and_then(Value::as_str).map(str::to_string),backwards_cursor:native.get("backwardsCursor").and_then(Value::as_str).map(str::to_string)}).map_err(dispatch::internal)?,
        MODELS=> {
            let rows=native.get("data").and_then(Value::as_array).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native model response has no page"))?;
            let items=rows.iter().map(|r|serde_json::from_value::<CodexModel>(r.clone())).collect::<Result<Vec<_>,_>>().map_err(|_|RpcError::new(RpcError::CONFLICT,"native model metadata shape differs"))?;
            serde_json::to_value(CodexModels{items,next_cursor:native.get("nextCursor").and_then(Value::as_str).map(str::to_string)}).map_err(dispatch::internal)?
        }
        TOOLS=>json!({"mcpServers":native.pointer("/mcpServers/data").and_then(Value::as_array).cloned().unwrap_or_default(),"skills":native.pointer("/skills/data").and_then(Value::as_array).cloned().unwrap_or_default()}),
        _=>unreachable!(),
    };
    Ok(result)
}
