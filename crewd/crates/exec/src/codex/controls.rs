//! Extension of the original CodexCore owned queue, not another scheduler.
use super::*;
use crate::codex::control::{CodexOperation as Op,CodexOutcome as Out,observed_settings,settings_params};
use crewd_proto::codex::{CodexTitleState,CodexTitleStatus,CodexTitleSource};
use serde_json::json;

pub(super) struct NativeEcho {
    ticket:Option<u64>, deadline:Instant,
    baseline:u64, accepted:bool,
}
impl CodexCore {
    pub fn codex_control(&mut self,ticket:u64,op:Op,now:Instant) {
        if self.phase!=Phase::Ready||self.stopped_by_user {
            self.native_done(Some(ticket),Out::Rejected("original Codex runtime is not running".into()));return;
        }
        self.native_request(Some(ticket),op,now);
        self.after_input(now);
    }
    pub(super) fn native_request(&mut self,ticket:Option<u64>,op:Op,now:Instant) {
        let Some(thread)=self.thread_id.clone() else {
            self.native_done(ticket,Out::Rejected("native thread identity missing".into()));return;
        };
        let idle=!self.tracker.is_busy()&&!self.in_flight;
        let (method,params)=match &op {
            Op::Inspect=> {
                self.native_done(ticket,Out::Observed(json!({"settings":if self.native_settings.is_object(){Some(observed_settings(&self.native_settings))}else{None},"nativeName":self.native_name,"queuedInputs":self.queue.len()})));return;
            }
            Op::Queue{id,text,request_id}=> {
                if text.trim().is_empty()||text.len()>128*1024||self.queue.len()>=256||self.initial_settings_failed||!crewd_proto::valid_remote_uuid(request_id) {
                    self.native_done(ticket,Out::Rejected("invalid/blocked/bounded public queue input".into()));return;
                }
                self.public_input_ids.insert(*id,request_id.clone());
                self.send(*id,text,Default::default(),now);
                self.native_done(ticket,Out::Observed(json!({"providerAccepted":false,"application":"queued","deliveryId":id.to_string(),"detail":"next-turn queue acceptance only; native delivery is unconfirmed"})));
                return;
            }
            Op::Configure(p)=> {
                if self.native_echo.is_some()||self.fast_requested||self.echo.is_some() {
                    self.native_done(ticket,Out::Rejected("previous settings change is pending".into()));return;
                }
                let params=match settings_params(&thread,p,&self.native_settings) {Ok(v)=>v,Err(e)=>{self.native_done(ticket,Out::Rejected(e));return}};
                self.native_echo=Some(NativeEcho{ticket,baseline:self.native_settings_epoch,deadline:now+self.config.request_timeout,accepted:false});
                ("thread/settings/update",params)
            }
            Op::Name(name)=> {
                if name.trim().is_empty()||name.len()>512||name.chars().any(char::is_control) {
                    self.native_done(ticket,Out::Rejected("invalid title".into()));return;
                }
                ("thread/name/set",json!({"threadId":thread,"name":name}))
            }
            Op::Steer{text,expected_turn_id,request_id}=> {
                if text.trim().is_empty()||text.len()>128*1024||self.tracker.active_turn()!=Some(expected_turn_id.as_str())||self.in_flight {
                    self.native_done(ticket,Out::Rejected("exact active turn required for steer; no queue fallback".into()));return;
                }
                ("turn/steer",json!({"threadId":thread,"expectedTurnId":expected_turn_id,"input":[{"type":"text","text":text}],"clientUserMessageId":request_id}))
            }
            Op::Review(target)=> {
                if !idle {self.native_done(ticket,Out::Rejected("review requires idle original thread".into()));return;}
                ("review/start",json!({"threadId":thread,"delivery":"inline","target":target}))
            }
            Op::History(p)=> {
                if p.limit.is_some_and(|n|n==0||n>100)||p.cursor.as_ref().is_some_and(|s|s.len()>8192) {
                    self.native_done(ticket,Out::Rejected("invalid history page bounds".into()));return;
                }
                let mut v=json!({"threadId":thread,"limit":p.limit.unwrap_or(50)});
                if let Some(c)=&p.cursor {v["cursor"]=json!(c);}
                if let Some(k)=&p.items_view {v["itemsView"]=json!(k);}
                if let Some(k)=&p.sort_direction {v["sortDirection"]=json!(k);}
                ("thread/turns/list",v)
            }
            Op::Models=>("model/list",json!({"limit":100})),
            Op::Tools=>("mcpServerStatus/list",json!({"limit":100})),
            Op::Fork{last_turn_id}=> {
                if !idle {self.native_done(ticket,Out::Rejected("fork requires idle or stopped original thread".into()));return;}
                let mut v=json!({"threadId":thread,"excludeTurns":true});
                if let Some(id)=last_turn_id {v["lastTurnId"]=json!(id);}
                ("thread/fork",v)
            }
        };
        self.request(Tag::Native{ticket,op},method,&params,now,self.config.request_timeout);
    }
    pub(super) fn native_done(&mut self,ticket:Option<u64>,outcome:Out) {
        if let Some(ticket)=ticket {self.actions.push(CodexAction::CodexDone{ticket,outcome});}
        else if !matches!(outcome,Out::Observed(_)) {
            self.event(SessionEvent::Notice(format!("Codex initial metadata/control: {outcome:?}")));
        }
    }
    pub(super) fn native_result(&mut self,ticket:Option<u64>,op:Op,result:Result<Value,RpcFailure>,now:Instant) {
        if self.stopped_by_user && matches!(op,Op::Configure(_)|Op::Name(_)|Op::Steer{..}|Op::Review(_)|Op::Fork{..}) {
            self.native_done(ticket,Out::Unknown{provider_accepted:result.is_ok(),detail:"original runtime stopped before control settled".into()});return;
        }
        let value=match result {
            Ok(v)=>v,
            Err(f)=> {
                if matches!(op,Op::Configure(_)) {self.native_echo=None;self.native_initial_failed(ticket,now);}
                if matches!(op,Op::Name(_)) {
                    self.event(SessionEvent::CodexObservation{accepted:None,settings:None,title:Some(CodexTitleState{desired:match &op{Op::Name(n)=>Some(n.clone()),_=>None},source:CodexTitleSource::Human,native:None,status:if matches!(f,RpcFailure::Server(_)){CodexTitleStatus::Rejected}else{CodexTitleStatus::Unknown},detail:Some(f.to_string())})});
                }
                let out=match f {RpcFailure::Server(e)=>Out::Rejected(format!("{} ({})",e.message,e.code)),other=>Out::Unknown{provider_accepted:false,detail:other.to_string()}};
                self.native_done(ticket,out);return;
            }
        };
        let mut result=json!({"providerAccepted":true});
        match op {
            Op::Configure(patch)=> {
                let echo=self.native_echo.take();
                if ticket.is_none() {self.event(SessionEvent::CodexObservation{accepted:Some(patch),settings:None,title:None});}
                self.initial_settings_gate=false;
                // Native schema defines acceptance as defaults for subsequent
                // turns. It does not promise a per-field effective-value echo.
                let observed=echo.as_ref().filter(|e|self.native_settings_epoch>e.baseline).map(|_|observed_settings(&self.native_settings));
                self.native_done(ticket,Out::Observed(json!({"providerAccepted":true,"application":"followingTurns","observedSettings":observed})));
                return;
            }
            Op::Name(name)=> {
                self.native_name=Some(name.clone());
                self.event(SessionEvent::CodexObservation{accepted:None,settings:None,title:Some(CodexTitleState{desired:Some(name.clone()),native:Some(name.clone()),source:CodexTitleSource::Human,status:CodexTitleStatus::Synced,detail:None})});
                result["application"]=json!("metadata");result["nativeName"]=json!(name);
            }
            Op::Steer{expected_turn_id,..}=> {
                if value.get("turnId").and_then(Value::as_str)!=Some(expected_turn_id.as_str()) {
                    self.native_done(ticket,Out::Unknown{provider_accepted:true,detail:"steer ACK did not confirm exact requested turn".into()});return;
                }
                result["application"]=json!("currentTurn");result["acceptedTurnId"]=json!(expected_turn_id);
            }
            Op::Review(_)=> {
                result["application"]=json!("started");
                result["acceptedTurnId"]=value.pointer("/turn/id").cloned().unwrap_or(Value::Null);
                result["providerResult"]=value;
            }
            Op::Tools=> {
                let Some(ticket)=ticket else{return};
                self.request(Tag::NativeToolsSkills{ticket,servers:value},"skills/list",&json!({"cwds":[self.cwd]}),now,self.config.request_timeout);return;
            }
            _=> {result["application"]=json!("metadata");result["providerResult"]=value;}
        }
        self.native_done(ticket,Out::Observed(result));
    }
    pub(super) fn native_tools_result(&mut self,ticket:u64,servers:Value,result:Result<Value,RpcFailure>) {
        self.native_done(Some(ticket),match result {
            Ok(skills)=>Out::Observed(json!({"providerAccepted":true,"application":"metadata","mcpServers":servers,"skills":skills})),
            Err(RpcFailure::Server(e))=>Out::Rejected(e.message),
            Err(e)=>Out::Unknown{provider_accepted:false,detail:e.to_string()},
        });
    }
    pub(super) fn native_settings_echo(&mut self,params:&Value) {
        if params.get("threadId").and_then(Value::as_str)!=self.thread_id.as_deref(){return;}
        let Some(s)=params.get("threadSettings").filter(|v|v.is_object()) else{return};
        self.native_settings=s.clone();self.native_settings_epoch+=1;
        self.event(SessionEvent::CodexObservation{accepted:None,settings:Some(serde_json::to_value(observed_settings(s)).expect("observed settings")),title:None});
    }
    pub(super) fn native_name_echo(&mut self,params:&Value) {
        if params.get("threadId").and_then(Value::as_str)!=self.thread_id.as_deref(){return;}
        if let Some(name)=params.get("name").and_then(Value::as_str) {
            self.native_name=Some(name.into());
            // A provider-generated rename is observable, not a new Human title intent.
            self.event(SessionEvent::CodexObservation{accepted:None,settings:None,title:Some(CodexTitleState{desired:None,native:Some(name.into()),source:CodexTitleSource::Task,status:CodexTitleStatus::Synced,detail:None})});
        }
    }
    pub(super) fn native_tick(&mut self,now:Instant) {
        if self.native_echo.as_ref().is_some_and(|w|w.deadline<=now) {
            let e=self.native_echo.take().unwrap();
            self.native_initial_failed(e.ticket,now);
            self.native_done(e.ticket,Out::Unknown{provider_accepted:e.accepted,detail:"matching native settings echo not observed".into()});
        }
    }
    pub(super) fn native_cancel(&mut self,reason:&str) {
        if let Some(e)=self.native_echo.take() {
            self.native_done(e.ticket,Out::Unknown{provider_accepted:e.accepted,detail:reason.into()});
        }
    }
    fn native_initial_failed(&mut self,ticket:Option<u64>,now:Instant) {
        if ticket.is_none() && self.initial_settings_gate {
            self.initial_settings_failed=true;self.initial_settings_gate=false;
            self.abandon("initial Codex options not confirmed; task was not submitted",true,now);
        }
    }
    pub(super) fn native_opening(&mut self,now:Instant) {
        // Called only after the existing thread Record durable ACK.
        if let Some(title)=self.options.title.clone() {self.native_request(None,Op::Name(title),now);}
        // Start/resume response is real protocol evidence, not desired defaults.
        // Only populated response fields are eligible to resolve mode model.
        if self.native_settings.is_null() {
            self.native_settings=json!({"model":self.opened_profile.model,"effort":self.opened_profile.effort});
        }
        self.event(SessionEvent::CodexObservation{settings:Some(serde_json::to_value(observed_settings(&self.native_settings)).expect("opening settings")),title:None,accepted:None});
        if let Some(patch)=self.options.initial_settings.clone() {
            self.initial_settings_gate=true;
            self.native_request(None,Op::Configure(patch),now);
        }
    }
}
