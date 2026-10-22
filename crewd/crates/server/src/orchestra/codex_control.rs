//! Human Codex controls use the original actor and command receipt owner.
use super::{core::{Core,Input},port::Port};
use crate::{dispatch,public_execution::{Admission,PublicExecution}};
use crewd_exec::{codex::control::{CodexOperation as Op,CodexOutcome as Out},driver::{Control,ControlOutcome}};
use crewd_proto::{RpcError,commands::{PublicCommand as Cmd,CommandPhase,CommandRuntimeTarget}};
use serde_json::{Value,json};

pub(crate) fn is_command(c:&Cmd)->bool {matches!(c,Cmd::CodexConfigure(_)|Cmd::CodexCompact(_)|Cmd::CodexSteer(_)|Cmd::CodexQueue(_)|Cmd::CodexReview(_)|Cmd::CodexTitleSet(_)|Cmd::CodexFork(_))}
fn target(c:&Cmd)->CommandRuntimeTarget {match c {
    Cmd::CodexConfigure(p)=>p.target.clone(),Cmd::CodexCompact(p)=>p.clone(),Cmd::CodexSteer(p)|Cmd::CodexQueue(p)=>p.target.clone(),Cmd::CodexReview(p)=>p.target.clone(),Cmd::CodexTitleSet(p)=>p.target.clone(),Cmd::CodexFork(p)=>p.target.clone(),_=>unreachable!("closed Codex commands")
}}
impl<P:Port> Core<P> {
    pub(crate) async fn execute_public_codex(&mut self,call:PublicExecution)->Result<Value,RpcError> {
        if let Admission::Existing(r)=call.admit().await? {return serde_json::to_value(r).map_err(dispatch::internal);}
        let target=target(&call.request.command);
        let row=self.port.session(&target.session_id).await.map_err(dispatch::internal)?;
        let row=match row.filter(|r|r.executor=="codex"&&!r.crew_removed) {
            Some(row) if !self.shutting_down&&self.matches_public_target(&target)=>row,
            _=>return self.codex_suppress(&call,"command.stale_or_non_codex_target").await,
        };
        let live=self.live.get(&target.session_id).filter(|l|!l.stopping&&!l.halting&&!l.hibernate_cut);
        let metadata=matches!(call.request.command,Cmd::CodexTitleSet(_)|Cmd::CodexFork(_));
        if live.is_none()&&!metadata {return self.codex_suppress(&call,"command.codex_requires_running").await;}
        match &call.request.command {
            Cmd::CodexConfigure(p)=>{if let Err(e)=p.patch.validate(){return self.codex_suppress(&call,e).await;}},
            Cmd::CodexSteer(p) if p.target.turn_id.is_none()||p.text.trim().is_empty()||p.text.len()>128*1024=>return self.codex_suppress(&call,"command.codex_steer_requires_exact_turn").await,
            Cmd::CodexQueue(p) if p.text.trim().is_empty()||p.text.len()>128*1024=>return self.codex_suppress(&call,"command.codex_invalid_body").await,
            Cmd::CodexFork(_)|Cmd::CodexReview(_) if live.is_some_and(|l|l.working||l.decision.is_some())=>return self.codex_suppress(&call,"command.codex_requires_idle").await,
            Cmd::CodexTitleSet(p)=>{if let Err(e)=crewd_collab::sessions::resolve_title(Some(&p.title),None){return self.codex_suppress(&call,&e.to_string()).await;}},
            _=>{}
        }
        let handle=live.map(|l|l.handle.clone());
        let thread=row.agent_session_id.clone().filter(|s|!s.is_empty()).ok_or_else(||RpcError::new(RpcError::CONFLICT,"recorded native thread missing"))?;
        let workdir=row.workdir.clone().ok_or_else(||RpcError::new(RpcError::CONFLICT,"original workdir missing"))?;
        let exec=self.execs.get("codex").map_err(dispatch::internal)?;
        let receipt=call.dispatch().await?;
        self.advance_public_control(&target.session_id);
        let after=self.current_public_target(&target.session_id);
        let inbox=self.inbox.clone();let original=call.clone();
        tokio::spawn(async move {
            let op=match &call.request.command {
                Cmd::CodexConfigure(p)=>Some(Op::Configure(p.patch.clone())),
                Cmd::CodexTitleSet(p)=>Some(Op::Name(p.title.clone())),
                Cmd::CodexSteer(p)=>Some(Op::Steer{text:p.text.clone(),expected_turn_id:p.target.turn_id.clone().expect("validated exact turn"),request_id:call.request.request_id.clone()}),
                Cmd::CodexReview(p)=>Some(Op::Review(p.review_target.clone())),
                Cmd::CodexFork(p)=>Some(Op::Fork{last_turn_id:p.last_turn_id.clone()}),
                _=>None,
            };
            let outcome=if let Err(e)=call.recheck_dispatched().await {Out::Unknown{provider_accepted:false,detail:format!("dispatch authority no longer current: {e:?}")}}
            else if let Some(op)=op {
                match handle {
                    Some(h)=>h.codex(op).await.unwrap_or(Out::Unknown{provider_accepted:false,detail:"original Codex handle closed".into()}),
                    None=>exec.codex_metadata(crewd_exec::codex::metadata::MetadataRequest{session_id:target.session_id.clone(),thread_id:thread.clone(),workdir,operation:op}).await,
                }
            } else if let Some(h)=handle {
                match &call.request.command {
                    Cmd::CodexCompact(_)=>match h.control(Control::Compact).await {
                        Ok(ControlOutcome::Applied(detail))=>Out::Observed(json!({"providerAccepted":true,"application":"started","detail":detail})),
                        Ok(ControlOutcome::Rejected(detail)|ControlOutcome::Unsupported(detail))=>Out::Rejected(detail),
                        Ok(ControlOutcome::NoConfirmation(detail))=>Out::Unknown{provider_accepted:true,detail},
                        Err(_)=>Out::Unknown{provider_accepted:false,detail:"compact handle closed".into()},
                    },
                    Cmd::CodexQueue(p)=>h.codex_queue(p.text.clone(),call.request.request_id.clone()).await.unwrap_or(Out::Unknown{provider_accepted:false,detail:"queue original handle closed".into()}),
                    _=>unreachable!("closed operation"),
                }
            } else {Out::Rejected("runtime closed".into())};
            let _=inbox.send(Input::PublicCodexDone{call:original,target:after,thread,outcome});
        });
        serde_json::to_value(receipt).map_err(dispatch::internal)
    }
    async fn codex_suppress(&self,call:&PublicExecution,why:&str)->Result<Value,RpcError> {
        let r=call.settle(CommandPhase::Admitted,CommandPhase::Suppressed,json!({"refusal":why,"providerAccepted":false,"application":"none"})).await?;
        serde_json::to_value(r).map_err(dispatch::internal)
    }
    pub(crate) async fn finish_public_codex(&mut self,call:PublicExecution,target:CommandRuntimeTarget,thread:String,outcome:Out) {
        let generation_current=self.current_public_target(&target.session_id).generation==target.generation;
        let not_stopping=self.live.get(&target.session_id).is_none_or(|l|!l.stopping&&!l.halting&&!l.hibernate_cut);
        let (phase,mut result)=match outcome {
            Out::Observed(value) if generation_current&&not_stopping=>(CommandPhase::Confirmed,value),
            Out::Observed(_)=>(CommandPhase::Unknown,json!({"providerAccepted":true,"application":"unknown","detail":"original runtime generation/stop changed before settlement"})),
            Out::Rejected(detail)=>(CommandPhase::Rejected,json!({"providerAccepted":false,"application":"none","detail":detail})),
            Out::Unknown{provider_accepted,detail}=>(CommandPhase::Unknown,json!({"providerAccepted":provider_accepted,"application":"unknown","detail":detail})),
        };
        result["sessionId"]=json!(target.session_id);result["threadId"]=json!(thread);result["operation"]=json!(call.request.command.method());
        if matches!(call.request.command,Cmd::CodexFork(_))&&phase==CommandPhase::Confirmed {
            let native=result.pointer("/providerResult/thread/id").and_then(Value::as_str).map(str::to_string);
            match native {Some(id)=>result["nativeThreadId"]=json!(id),None=>{result["application"]=json!("unknown");result["detail"]=json!("fork ACK lacks native thread identity");}}
        }
        let phase=if matches!(call.request.command,Cmd::CodexFork(_))&&result.get("nativeThreadId").is_none()&&phase==CommandPhase::Confirmed{CommandPhase::Unknown}else{phase};
        if let Err(e)=call.settle(CommandPhase::Dispatched,phase,result).await {eprintln!("Codex original control receipt remains unknown: {e:?}");}
        if let Ok(state)=call.state(){state.hub.publish(crewd_proto::Event{event:crewd_proto::codex::CHANGED.into(),data:json!({"sessionId":target.session_id})});}
    }
}
