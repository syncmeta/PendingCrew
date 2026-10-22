//! Read-only Codex projections. Stopped threads are never resumed for a read.
use super::{Handle, rpc::rpc_err};
use crate::{AppState, dispatch};
use crewd_exec::codex::control::{CodexOperation as Op, CodexOutcome as Out};
use crewd_proto::{RpcError, codex::*, orchestra::SessionIdParams, parse_params};
use serde_json::{Value, json};

pub(crate) fn handles(method: &str) -> bool {
    [INSPECT, HISTORY, MODELS, TOOLS, crewd_proto::codex_inputs::PERMISSION_PROFILES].contains(&method)
}
pub(crate) async fn read(
    h: &Handle,
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let profiles=if method==crewd_proto::codex_inputs::PERMISSION_PROFILES {Some(parse_params::<crewd_proto::codex_inputs::CodexPermissionProfilesParams>(params.clone())?)} else {None};
    let catalog=if method==TOOLS {Some(parse_params::<crewd_proto::codex_inputs::CodexCatalogParams>(params.clone())?)}else{profiles.as_ref().map(|p|p.source())};
    if let Some(source)=&catalog {
        source.validate().map_err(|e|RpcError::new(RpcError::INVALID_PARAMS,e))?;
        if let Some(crew)=&source.crew_id {return read_crew_catalog(h,state,crew.clone(),profiles).await}
    }
    let (sid, history) = if let Some(source)=&catalog {(source.session_id.clone().expect("validated exclusive source"),None)} else if method == HISTORY {
        let p: CodexHistoryParams = parse_params(params)?;
        (p.session_id.clone(), Some(p))
    } else {
        let p: SessionIdParams = parse_params(params)?;
        (p.session_id, None)
    };
    let id = sid.clone();
    let (row, meta, workdir) = state
        .db
        .call_result(move |store| {
            let c = store.conn();
            let row = crewd_collab::sessions::get(c, &id)?;
            let crew = crewd_collab::crews::get(c, &row.crew_id)?;
            let work = row
                .worktree
                .clone()
                .or(row.working_directory.clone())
                .or(crew.working_directory);
            let meta = crewd_store::codex_controls::get(c, &id)?;
            Ok((row, meta, work))
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    if row.executor != "codex" {
        return Err(RpcError::new(
            RpcError::CONFLICT,
            "session is not a Codex source",
        ));
    }
    let live = h.live(&sid).await.map_err(rpc_err)?;
    let handle = h.codex_handle(&sid).await.map_err(rpc_err)?;
    if method == INSPECT {
        let local = match handle.as_ref() {
            Some(r) => r.codex(Op::Inspect).await.ok(),
            None => None,
        };
        let history_mode=local.as_ref().and_then(|o|match o {Out::Observed(v)=>v.get("historyMode").and_then(Value::as_str).map(str::to_owned),_=>None});
        let queued = match local {
            Some(Out::Observed(v)) => {
                v.get("queuedInputs").and_then(Value::as_u64).unwrap_or(0) as u32
            }
            _ => 0,
        };
        let pending = h.runner_request(&sid).await.map_err(rpc_err)?.map(|r| {
            let request_id = live
                .pending_decision
                .as_ref()
                .and_then(|d| match &d.kind {
                    crewd_term::DecisionKind::RunnerRequest { request_id } => {
                        Some(request_id.clone())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            crewd_proto::orchestra::RunnerRequestView {
                runner_request_id: request_id,
                fingerprint: r.fingerprint,
                method: r.method,
                params: r.params,
            }
        });
        let native_status=if meta.restore_unconfirmed&&meta.native_history.as_ref().is_none_or(|h|h.status!=CodexNativeHistoryStatus::Deleted){Some(CodexNativeHistoryStatus::Unknown)}else{meta.native_history.as_ref().map(|h|h.status)};
        let blocked=meta.fork_pending||matches!(native_status,Some(CodexNativeHistoryStatus::Unknown|CodexNativeHistoryStatus::Deleted|CodexNativeHistoryStatus::Archived));
        let capabilities=[
            ("configure",true,true),("compact",true,false),("steer",true,false),("queue",true,false),
            ("interrupt",true,false),("review",true,false),("history",false,false),("resume",false,false),
            ("fork",false,false),("title",false,false),("models",false,false),("tools",false,false),
            ("attachments",true,false),("skillInput",true,false),("permissionProfiles",false,true),
            ("archive",false,false),("unarchive",false,false),("delete",false,true),("revert",true,true),
        ].into_iter().map(|(id,needs_run,experimental)|{
            let requires_thread=!matches!(id,"models"|"tools"|"permissionProfiles");
            let unavailable=if requires_thread&&row.agent_session_id.as_deref().is_none_or(str::is_empty){Some("original native thread identity unavailable")}
                else if matches!(id,"models"|"tools"|"permissionProfiles"|"history"){None}
                else if id=="unarchive"&&native_status==Some(CodexNativeHistoryStatus::Archived){None}
                else if id=="delete"&&native_status==Some(CodexNativeHistoryStatus::Archived){None}
                else if blocked{Some("native history/fork is archived, deleted or unconfirmed; query original receipt")}
                else if matches!(id,"archive"|"unarchive"|"delete")&&live.running{Some("native history management requires physical runtime exit")}
                else if id=="revert"&&(history_mode.as_deref()!=Some("paginated")||live.working||queued>0||live.pending_decision.is_some()){Some("revert requires observed paginated history, idle runtime, empty queue and no pending decision; files are not reverted")}
                else{None};
            CodexCapability{id:id.into(),availability:if unavailable.is_some(){CodexCapabilityAvailability::Unavailable}else if needs_run&&!live.running{CodexCapabilityAvailability::RequiresRunning}else{CodexCapabilityAvailability::Available},protocol:if experimental{CodexProtocol::Experimental}else{CodexProtocol::Stable},detail:unavailable.map(str::to_owned).or_else(||match id{
                "attachments"=>Some("One-shot original-owner prepared image/audio/UTF8 text input; no arbitrary binary/PDF, server paths or message attachments".into()),
                "skillInput"=>Some("Explicit uniquely enabled skill from the current original workspace catalog".into()),
                "configure"=>Some("Native ACK records accepted/desired-next-turn; observed-effective fields remain separate".into()),
                "delete"=>Some("Explicit confirmation required; tombstones native binding only, retaining PCW work and receipts".into()),
                _=>None,
            })}
        }).collect();
        return serde_json::to_value(CodexSessionInspection {
            session_id: sid.clone(),
            thread_id: row.agent_session_id,
            target: h.public_target(&sid).await.map_err(rpc_err)?,
            running: live.running,
            working: live.working,
            queued_inputs: queued,
            settings: meta.settings,
            settings_observed_at: meta.settings_observed_at,
            historical: meta.observed_binding.as_deref() != Some(&state.runtime_binding_id),
            title: meta.title,
            capabilities,
            pending_request: pending,
            desired_settings: meta.desired_settings,
            settings_status: meta.settings_status,
            native_history: if meta.restore_unconfirmed&&native_status!=Some(CodexNativeHistoryStatus::Deleted){Some(CodexNativeHistoryState{status:CodexNativeHistoryStatus::Unknown,request_id:None,operation:None})}else{meta.native_history},
            history_mode,
        })
        .map_err(dispatch::internal);
    }
    let thread = if method==crewd_proto::codex_inputs::PERMISSION_PROFILES {row.agent_session_id.clone().unwrap_or_default()} else {row
        .agent_session_id
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::new(RpcError::CONFLICT, "native thread identity missing"))?};
    let op = match method {
        HISTORY => Op::History(history.unwrap()),
        MODELS => Op::Models,
        TOOLS => Op::Tools,
        crewd_proto::codex_inputs::PERMISSION_PROFILES=>Op::PermissionProfiles(profiles.unwrap()),
        _ => unreachable!(),
    };
    let outcome = match handle {
        Some(r) => r
            .codex(op)
            .await
            .map_err(|_| RpcError::new(RpcError::CONFLICT, "Codex runtime closed"))?,
        None => {
            h.codex_metadata(crewd_exec::codex::metadata::MetadataRequest {
                session_id: sid.clone(),
                thread_id: thread.clone(),
                workdir: workdir
                    .ok_or_else(|| {
                        RpcError::new(RpcError::CONFLICT, "original workspace unavailable")
                    })?
                    .into(),
                operation: op,
                permit: None,
            })
            .await
        }
    };
    let value = match outcome {
        Out::Observed(v) => v,
        Out::Rejected(e) => {
            return Err(
                RpcError::new(RpcError::CONFLICT, e).with_refusal("codex.provider_rejected")
            );
        }
        Out::Unknown { detail, .. } => {
            return Err(
                RpcError::new(RpcError::CONFLICT, detail).with_refusal("codex.read_unconfirmed")
            );
        }
    };
    if serde_json::to_vec(&value)
        .map_err(dispatch::internal)?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(RpcError::new(
            RpcError::CONFLICT,
            "Codex metadata page exceeds 4 MiB response bound",
        ));
    }
    let native = value.get("providerResult").unwrap_or(&value);
    if matches!(method, HISTORY | MODELS)
        && native
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|rows| rows.len() > 100)
    {
        return Err(RpcError::new(
            RpcError::CONFLICT,
            "native metadata response exceeds requested page bound",
        ));
    }
    let result = match method {
        HISTORY => serde_json::to_value(CodexHistoryPage {
            session_id: sid,
            thread_id: thread,
            items: native
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "native history response has no page")
                })?
                .clone(),
            next_cursor: native
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string),
            backwards_cursor: native
                .get("backwardsCursor")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
        .map_err(dispatch::internal)?,
        MODELS => {
            let rows = native
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "native model response has no page")
                })?;
            let items = rows
                .iter()
                .map(|r| serde_json::from_value::<CodexModel>(r.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    RpcError::new(RpcError::CONFLICT, "native model metadata shape differs")
                })?;
            serde_json::to_value(CodexModels {
                items,
                next_cursor: native
                    .get("nextCursor")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
            .map_err(dispatch::internal)?
        }
        crewd_proto::codex_inputs::PERMISSION_PROFILES=>{
            let rows=native.get("data").and_then(Value::as_array).filter(|v|v.len()<=100).ok_or_else(||RpcError::new(RpcError::CONFLICT,"invalid permission profile page"))?;
            let items=rows.iter().cloned().map(serde_json::from_value).collect::<Result<Vec<crewd_proto::codex_inputs::CodexPermissionProfile>,_>>().map_err(|_|RpcError::new(RpcError::CONFLICT,"permission catalog shape differs"))?;
            serde_json::to_value(crewd_proto::codex_inputs::CodexPermissionProfiles{items,next_cursor:native.get("nextCursor").and_then(Value::as_str).map(str::to_owned)}).map_err(dispatch::internal)?
        }
        TOOLS => {
            let servers = native
                .pointer("/mcpServers/data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "native MCP metadata shape differs")
                })?;
            let skills = native
                .pointer("/skills/data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "native skills metadata shape differs")
                })?;
            json!({"mcpServers":servers,"skills":skills})
        }
        _ => unreachable!(),
    };
    Ok(result)
}

/// First-session menus are read-only reference snapshots of the original crew.
/// Registration/worktree isolation never occurs to obtain a catalog.
async fn read_crew_catalog(h:&Handle,state:&AppState,crew_id:String,profiles:Option<crewd_proto::codex_inputs::CodexPermissionProfilesParams>)->Result<Value,RpcError>{
    let source=crew_id.clone();
    let work=state.db.call_result(move |store|{
        let crew=crewd_collab::crews::get(store.conn(),&source)?;
        if crewd_collab::crews::is_archived(store.conn(),&source)?{return Err(crewd_collab::resource_invalid("codex.catalogCrewArchived"))}
        crew.working_directory.ok_or_else(||crewd_collab::resource_invalid("codex.catalogWorkspaceMissing"))
    }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)?;
    let workdir=tokio::task::spawn_blocking(move||std::fs::canonicalize(work)).await.map_err(dispatch::internal)?.map_err(|_|RpcError::new(RpcError::CONFLICT,"original crew workspace unavailable"))?;
    if !workdir.is_dir(){return Err(RpcError::new(RpcError::CONFLICT,"original crew workspace is not a directory"))}
    let requested_profiles=profiles.is_some();
    let operation=match profiles {Some(p)=>Op::PermissionProfiles(p),None=>Op::Tools};
    let outcome=h.codex_metadata(crewd_exec::codex::metadata::MetadataRequest{session_id:format!("crew-catalog/{crew_id}"),thread_id:String::new(),workdir,operation,permit:None}).await;
    let native=match outcome{Out::Observed(v)=>v,Out::Rejected(e)=>return Err(RpcError::new(RpcError::CONFLICT,e).with_refusal("codex.provider_rejected")),Out::Unknown{detail,..}=>return Err(RpcError::new(RpcError::CONFLICT,detail).with_refusal("codex.read_unconfirmed"))};
    if serde_json::to_vec(&native).map_err(dispatch::internal)?.len()>4*1024*1024{return Err(RpcError::new(RpcError::CONFLICT,"native catalog exceeds bounded response"))}
    let native=native.get("providerResult").unwrap_or(&native);
    if !requested_profiles{
        let servers=native.pointer("/mcpServers/data").and_then(Value::as_array).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native MCP catalog shape differs"))?;
        let skills=native.pointer("/skills/data").and_then(Value::as_array).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native skill catalog shape differs"))?;
        Ok(json!({"mcpServers":servers,"skills":skills}))
    }else{
        let rows=native.get("data").and_then(Value::as_array).filter(|v|v.len()<=100).ok_or_else(||RpcError::new(RpcError::CONFLICT,"native profile page shape differs"))?;
        let items=rows.iter().cloned().map(serde_json::from_value).collect::<Result<Vec<crewd_proto::codex_inputs::CodexPermissionProfile>,_>>().map_err(|_|RpcError::new(RpcError::CONFLICT,"native profile shape differs"))?;
        serde_json::to_value(crewd_proto::codex_inputs::CodexPermissionProfiles{items,next_cursor:native.get("nextCursor").and_then(Value::as_str).map(str::to_owned)}).map_err(dispatch::internal)
    }
}
