//! Human-only PCW-owned observations; these reads never call a provider or resume.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, claude::*, parse_params};
use serde_json::Value;
use std::sync::Arc;
pub(crate) fn is_method(method: &str) -> bool {
    matches!(method, INSPECT | HISTORY | TASK_GET)
}
fn unavailable() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "PCW managed Claude source/observation unavailable",
    )
    .with_refusal("claude.owned_history_unavailable")
}
fn birth_gone(pid: i32, start: u64, boot: &str) -> bool {
    use crewd_exec::registry::{ProbeResult, probe};
    match probe(pid) {
        ProbeResult::Missing => true,
        ProbeResult::Found(actual) => actual.start_time != start || actual.boot_id != boot,
        ProbeResult::Unreadable(_) => false,
    }
}
async fn read(
    state: &Arc<AppState>,
    actor: &Actor,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    actor.authorize(state, method).await?;
    let sid = match method {
        INSPECT => parse_params::<ClaudeSessionParams>(params.clone())?.session_id,
        HISTORY => parse_params::<ClaudeHistoryParams>(params.clone())?.session_id,
        TASK_GET => parse_params::<ClaudeTaskGetParams>(params.clone())?.session_id,
        _ => return Err(unavailable()),
    };
    let h = state.orchestra.get().ok_or_else(unavailable)?;
    let live = h.live(&sid).await.map_err(crate::orchestra::rpc::rpc_err)?;
    let configured = state.native_helpers.get().is_some()
        && h.executor_profiles()
            .iter()
            .any(|p| p.id == "claude" && p.available);
    let managed_live = state
        .native_helpers
        .get()
        .is_some_and(|f| crate::orchestra::exec_claude_managed::observation_live(f, &sid));
    let mut registry_gone = true;
    if let Some(factory) = state.native_helpers.get() {
        let mut births = factory.registry.try_entries().map_err(|_| unavailable())?;
        births.extend(
            factory
                .registry
                .authority_snapshot()
                .map_err(|_| unavailable())?
                .spawned,
        );
        registry_gone = births
            .iter()
            .filter(|(s, _)| s == &sid)
            .all(|(_, b)| birth_gone(b.pid, b.start_time, &b.boot_id));
    }
    let state2 = state.clone();
    let actor2 = actor.clone();
    let method2 = method.to_owned();
    let result=state.db.call(move|store|{
  let c=store.conn();
  actor2.authorize_tx(&state2,c,&method2).map_err(dispatch::collab_err)?;
  let row=crewd_collab::sessions::get(c,&sid).map_err(dispatch::collab_err)?;
  if row.executor!="claude" {return Err(unavailable())}
  let crew=crewd_collab::crews::get(c,&row.crew_id).map_err(dispatch::collab_err)?;
  let current=crewd_store::native_execution::current_runtime(c,&sid).map_err(|_|unavailable())?;
  let automatic=if let Some(n)=&current {c.query_row("SELECT manual=0 AND revoked=0 FROM native_execution_runtimes WHERE runtime_id=?1",[&n.id],|r|r.get::<_,bool>(0)).map_err(|_|unavailable())?}else{false};
  let live_runtime=current.as_ref().filter(|n|live.running&&automatic&&managed_live&&!birth_gone(n.native_pid,n.native_start,&n.native_boot)).map(|n|n.id.as_str());
  let value=match method2.as_str(){
   INSPECT=>{
    let mut q=c.prepare("SELECT native_pid,native_start,native_boot FROM native_execution_runtimes WHERE session_id=?1").map_err(|_|unavailable())?;
    let births=q.query_map([&sid],|r|Ok((r.get::<_,i32>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).map_err(|_|unavailable())?;
    let mut gone=registry_gone;
    for birth in births {let(pid,start,boot)=birth.map_err(|_|unavailable())?;gone&=birth_gone(pid,start as u64,&boot)}
    let original=row.agent_session_id.filter(|s|crewd_exec::driver::exact_managed_id(s));
    let eligibility=if live.running {ClaudeResumeEligibility::Running}
      else if crew.hidden_at.is_some(){ClaudeResumeEligibility::RemovedCrew}
      else if row.worktree.or(row.working_directory).is_none(){ClaudeResumeEligibility::MissingWorkdir}
      else if original.is_none(){ClaudeResumeEligibility::MissingOriginalId}
      else if !configured{ClaudeResumeEligibility::NotConfigured}
      else if !gone{ClaudeResumeEligibility::PreviousBirthLiveOrUnknown}
      else{ClaudeResumeEligibility::Eligible};
    serde_json::to_value(ClaudeInspect{session_id:sid,crew_id:row.crew_id,executor_profile_id:"claude".into(),backend:"existingClaudePty".into(),history_scope:"pcwManagedTasks".into(),full_history_load:false,native_acp:false,manual_resume:ClaudeManualResume{eligibility,original_agent_session_id:original,rechecks_program_and_source_on_launch:true},current_runtime_id:live_runtime.map(str::to_owned),task_text_limit_bytes:1_048_576,response_observation_limit_bytes:256*1024,retained_observations_per_task:4})
   },
   HISTORY=>serde_json::to_value(crewd_store::native_execution::managed_observations::reads::history(c,&parse_params(params)?,live_runtime).map_err(|_|unavailable())?),
   TASK_GET=>serde_json::to_value(crewd_store::native_execution::managed_observations::reads::task_get(c,&parse_params(params)?,live_runtime).map_err(|_|unavailable())?),
   _=>return Err(unavailable()),
  }.map_err(dispatch::internal)?;
  actor2.authorize_tx(&state2,c,&method2).map_err(dispatch::collab_err)?;
  Ok(value)
 }).await.map_err(|_|unavailable())??;
    actor.authorize(state, method).await?;
    Ok(result)
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    match read(state, actor, &req.method, req.params).await {
        Ok(v) => Response::ok(req.id, v),
        Err(e) => Response::err(req.id, e),
    }
}
