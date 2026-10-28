//! Extension of the original CodexCore owned queue, not another scheduler.
use super::*;
use crate::codex::control::{
    CodexOperation as Op, CodexOutcome as Out, observed_settings, settings_params,
};
use crewd_proto::codex::{CodexTitleSource, CodexTitleState, CodexTitleStatus};
use serde_json::json;

pub(super) struct NativeEcho {
    ticket: Option<u64>,
    baseline: u64,
}
impl CodexCore {
    pub fn codex_control(&mut self, ticket: u64, op: Op, now: Instant) {
        self.codex_controlled(ticket, op, None, now);
    }
    pub fn codex_controlled(
        &mut self,
        ticket: u64,
        op: Op,
        permit: Option<crate::driver::ManualWritePermit>,
        now: Instant,
    ) {
        if self.phase != Phase::Ready || self.stopped_by_user {
            self.native_done(
                Some(ticket),
                Out::Rejected("original Codex runtime is not running".into()),
            );
            return;
        }
        self.native_request(Some(ticket), op, permit, now);
        self.after_input(now);
    }
    pub(super) fn native_request(
        &mut self,
        ticket: Option<u64>,
        op: Op,
        permit: Option<crate::driver::ManualWritePermit>,
        now: Instant,
    ) {
        // Normalize only inside the owned runtime queue. Prepared guards survive
        // native ACKs until this physical runtime ends; ACK is not file consumption.
        let mut steering_inputs=None;
        let op=match op {
            Op::PreparedQueue{id,text,request_id,inputs}=>{self.native_queued_inputs.insert(id,inputs);Op::Queue{id,text,request_id}},
            Op::PreparedSteer{text,expected_turn_id,request_id,inputs}=>{steering_inputs=Some(inputs);Op::Steer{text,expected_turn_id,request_id}},
            other=>other,
        };
        let Some(thread) = self.thread_id.clone() else {
            self.native_done(
                ticket,
                Out::Rejected("native thread identity missing".into()),
            );
            return;
        };
        let idle = !self.tracker.is_busy() && !self.in_flight;
        let (method, params) = match &op {
            Op::PreparedQueue{..}|Op::PreparedSteer{..}=>unreachable!("normalized native input"),
            Op::Inspect => {
                self.native_done(ticket,Out::Observed(json!({"settings":if self.native_settings.is_object(){Some(observed_settings(&self.native_settings))}else{None},"nativeName":self.native_name,"queuedInputs":self.queue.len(),"historyMode":self.native_history_mode})));
                return;
            }
            Op::Queue {
                id,
                text,
                request_id,
            } => {
                if text.trim().is_empty()
                    || text.len() > 128 * 1024
                    || self.queue.len() >= 256
                    || self.initial_settings_failed
                    || !crewd_proto::valid_remote_uuid(request_id)
                {
                    self.native_done(
                        ticket,
                        Out::Rejected("invalid/blocked/bounded public queue input".into()),
                    );
                    return;
                }
                self.public_input_ids
                    .insert(*id, (request_id.clone(), permit.clone()));
                self.send(*id, text, Default::default(), now);
                self.native_done(ticket,Out::Observed(json!({"providerAccepted":false,"application":"queued","deliveryId":id.to_string(),"detail":"next-turn queue acceptance only; native delivery is unconfirmed"})));
                return;
            }
            Op::Configure(p) => {
                if self.native_echo.is_some() || self.fast_requested || self.echo.is_some() {
                    self.native_done(
                        ticket,
                        Out::Rejected("previous settings change is pending".into()),
                    );
                    return;
                }
                let params = match settings_params(&thread, p, &self.native_settings) {
                    Ok(v) => v,
                    Err(e) => {
                        self.native_initial_failed(ticket, now);
                        self.native_done(ticket, Out::Rejected(e));
                        return;
                    }
                };
                self.native_echo = Some(NativeEcho {
                    ticket,
                    baseline: self.native_settings_epoch,
                });
                if p.permissions.is_some() {
                    self.request(Tag::PermissionCheck{ticket,patch:p.clone(),permit:permit.clone(),cursors:BTreeSet::new(),pages:1},"permissionProfile/list",&json!({"cwd":self.cwd,"limit":100}),now,self.config.request_timeout);
                    return;
                }
                ("thread/settings/update", params)
            }
            Op::Name(name) => {
                if name.trim().is_empty() || name.len() > 512 || name.chars().any(char::is_control)
                {
                    self.native_done(ticket, Out::Rejected("invalid title".into()));
                    return;
                }
                ("thread/name/set", json!({"threadId":thread,"name":name}))
            }
            Op::Steer {
                text,
                expected_turn_id,
                request_id,
            } => {
                if text.trim().is_empty()
                    || text.len() > 128 * 1024
                    || self.tracker.active_turn() != Some(expected_turn_id.as_str())
                    || self.tracker.is_compacting()
                    || self.in_flight
                {
                    self.native_done(
                        ticket,
                        Out::Rejected(
                            "exact active turn required for steer; no queue fallback".into(),
                        ),
                    );
                    return;
                }
                (
                    "turn/steer",
                    {let mut params=json!({"threadId":thread,"expectedTurnId":expected_turn_id,"input":[{"type":"text","text":text}],"clientUserMessageId":request_id});
                    if let Some(inputs)=steering_inputs.take(){inputs.append(&mut params["input"]);self.native_input_files.push(inputs);}params},
                )
            }
            Op::Interrupt { expected_turn_id } => {
                if self.tracker.active_turn() != Some(expected_turn_id.as_str()) {
                    self.native_done(
                        ticket,
                        Out::Rejected("interrupt requires the exact original active turn".into()),
                    );
                    return;
                }
                self.clear_human(Some(expected_turn_id));
                (
                    "turn/interrupt",
                    json!({"threadId":thread,"turnId":expected_turn_id}),
                )
            }
            Op::Review(target) => {
                if !idle {
                    self.native_done(
                        ticket,
                        Out::Rejected("review requires idle original thread".into()),
                    );
                    return;
                }
                // The original next-turn gate owns this request until native
                // ACK/rejection. An uncertain start keeps it closed until stop.
                self.in_flight = true;
                (
                    "review/start",
                    json!({"threadId":thread,"delivery":"inline","target":target}),
                )
            }
            Op::History(p) => {
                if p.limit.is_some_and(|n| n == 0 || n > 100)
                    || p.cursor.as_ref().is_some_and(|s| s.len() > 8192)
                {
                    self.native_done(ticket, Out::Rejected("invalid history page bounds".into()));
                    return;
                }
                let mut v = json!({"threadId":thread,"limit":p.limit.unwrap_or(50)});
                if let Some(c) = &p.cursor {
                    v["cursor"] = json!(c);
                }
                if let Some(k) = &p.items_view {
                    v["itemsView"] = json!(k);
                }
                if let Some(k) = &p.sort_direction {
                    v["sortDirection"] = json!(k);
                }
                ("thread/turns/list", v)
            }
            Op::PermissionProfiles(p) => {
                if p.limit.is_some_and(|n|n==0||n>100) || p.cursor.as_ref().is_some_and(|c|c.len()>8192) {
                    self.native_done(ticket,Out::Rejected("invalid original permission profile page".into()));return;
                }
                ("permissionProfile/list",json!({"cwd":self.cwd,"cursor":p.cursor,"limit":p.limit.unwrap_or(100)}))
            }
            Op::HistoryMutation{action,before_turn_id} => {
                if *action != crewd_proto::codex::CodexHistoryAction::Revert || !idle || !self.queue.is_empty() || !self.human.is_empty() || self.native_history_mode.as_deref()!=Some("paginated") || before_turn_id.as_ref().is_none_or(|s|s.trim().is_empty()||s.len()>256||s.chars().any(char::is_control)) {
                    self.native_done(ticket,Out::Rejected("native revert requires an already loaded idle paginated thread, empty queue and no Human decision; stopped history mutations use their own metadata route".into()));return;
                }
                self.in_flight=true;
                ("thread/revert",json!({"threadId":thread,"beforeTurnId":before_turn_id}))
            }
            Op::RateLimits => { self.native_done(ticket, Out::Rejected("host quota requires metadata transport".into())); return; }
            Op::Models => ("model/list", json!({"limit":100})),
            Op::Tools => ("mcpServerStatus/list", json!({"limit":100})),
            Op::Fork { last_turn_id } => {
                if !idle {
                    self.native_done(
                        ticket,
                        Out::Rejected("fork requires idle or stopped original thread".into()),
                    );
                    return;
                }
                let mut v = json!({"threadId":thread,"excludeTurns":true});
                if let Some(id) = last_turn_id {
                    v["lastTurnId"] = json!(id);
                }
                ("thread/fork", v)
            }
        };
        let line = self.rpc.request(
            Tag::Native { ticket, op },
            method,
            &params,
            now,
            self.config.request_timeout,
        );
        self.actions.push(match permit {
            Some(permit) => CodexAction::ControlledWrite { line, permit },
            None => CodexAction::Write(line),
        });
    }
    pub(super) fn permission_checked(
        &mut self,ticket:Option<u64>,patch:crewd_proto::codex::CodexSettingsPatch,
        permit:Option<crate::driver::ManualWritePermit>,mut cursors:BTreeSet<String>,pages:u8,
        result:Result<Value,RpcFailure>,now:Instant,
    ) {
        if self.stopped_by_user || self.phase!=Phase::Ready || self.native_echo.is_none() {
            self.native_echo=None;self.native_initial_failed(ticket,now);
            self.native_done(ticket,Out::Unknown{provider_accepted:false,detail:"original runtime changed before permission profile validation; no settings write".into()});return;
        }
        let parsed=(||->Result<(bool,Option<String>),String>{
            let value=result.map_err(|_|"permission catalog unavailable; no settings write".to_string())?;
            let rows=value.get("data").and_then(Value::as_array).filter(|r|r.len()<=100).ok_or("invalid bounded permission catalog")?;
            let mut names=BTreeSet::new();let mut allowed=false;
            for row in rows {
                let profile:crewd_proto::codex_inputs::CodexPermissionProfile=serde_json::from_value(row.clone()).map_err(|_|"invalid permission profile shape")?;
                if profile.id.trim().is_empty()||profile.id.len()>256||!names.insert(profile.id.clone()){return Err("ambiguous permission catalog".into())}
                if Some(&profile.id)==patch.permissions.as_ref() {
                    if !profile.allowed {return Err("selected permission profile is not currently allowed".into())}
                    allowed=true;
                }
            }
            let next=value.get("nextCursor").and_then(Value::as_str).map(str::to_owned);
            Ok((allowed,next))
        })();
        match parsed {
            Ok((true,_))=>{
                let Some(thread)=self.thread_id.clone() else {self.native_echo=None;self.native_initial_failed(ticket,now);self.native_done(ticket,Out::Rejected("original thread identity unavailable".into()));return};
                let params=match settings_params(&thread,&patch,&self.native_settings){Ok(p)=>p,Err(e)=>{self.native_echo=None;self.native_initial_failed(ticket,now);self.native_done(ticket,Out::Rejected(e));return}};
                let line=self.rpc.request(Tag::Native{ticket,op:Op::Configure(patch)},"thread/settings/update",&params,now,self.config.request_timeout);
                self.actions.push(match permit {Some(permit)=>CodexAction::ControlledWrite{line,permit},None=>CodexAction::Write(line)});
            },
            Ok((false,Some(next))) if pages<4 && next.len()<=8192 && cursors.insert(next.clone())=>{
                self.request(Tag::PermissionCheck{ticket,patch,permit,cursors,pages:pages+1},"permissionProfile/list",&json!({"cwd":self.cwd,"limit":100,"cursor":next}),now,self.config.request_timeout);
            },
            other=>{
                let detail=match other {Err(e)=>e,_=>"selected permission profile is absent from the bounded current catalog".into()};
                self.native_echo=None;self.native_initial_failed(ticket,now);self.native_done(ticket,Out::Rejected(detail));
            },
        }
    }
    pub(super) fn native_done(&mut self, ticket: Option<u64>, outcome: Out) {
        if let Some(ticket) = ticket {
            self.actions
                .push(CodexAction::CodexDone { ticket, outcome });
        } else if !matches!(outcome, Out::Observed(_)) {
            self.event(SessionEvent::Notice(format!(
                "Codex initial metadata/control: {outcome:?}"
            )));
        }
    }
    pub(super) fn native_result(
        &mut self,
        ticket: Option<u64>,
        op: Op,
        result: Result<Value, RpcFailure>,
        now: Instant,
    ) {
        if self.stopped_by_user
            && matches!(
                op,
                Op::Configure(_)
                    | Op::Name(_)
                    | Op::Steer { .. }
                    | Op::Interrupt { .. }
                    | Op::Review(_)
                    | Op::Fork { .. }
                    | Op::HistoryMutation { .. }
            )
        {
            self.native_done(
                ticket,
                Out::Unknown {
                    provider_accepted: result.is_ok(),
                    detail: "original runtime stopped before control settled".into(),
                },
            );
            return;
        }
        let value = match result {
            Ok(v) => v,
            Err(f) => {
                if matches!(op, Op::Review(_) | Op::HistoryMutation { .. }) {
                    if matches!(f, RpcFailure::Server(_)) {
                        self.in_flight = false;
                    } else {
                        self.event(SessionEvent::Notice("Codex review start unconfirmed; following input is held until explicit stop, never retried".into()));
                    }
                }
                if matches!(op, Op::Configure(_)) {
                    self.native_echo = None;
                    self.native_initial_failed(ticket, now);
                }
                if matches!(op, Op::Name(_)) {
                    self.event(SessionEvent::CodexObservation {
                        accepted: None,
                        settings: None,
                        title: Some(CodexTitleState {
                            desired: match &op {
                                Op::Name(n) => Some(n.clone()),
                                _ => None,
                            },
                            source: CodexTitleSource::Human,
                            native: None,
                            status: if matches!(f, RpcFailure::Server(_)) {
                                CodexTitleStatus::Rejected
                            } else {
                                CodexTitleStatus::Unknown
                            },
                            detail: Some(f.to_string()),
                        }),
                    });
                }
                let out = match f {
                    RpcFailure::Server(e) => Out::Rejected(format!("{} ({})", e.message, e.code)),
                    other => Out::Unknown {
                        provider_accepted: false,
                        detail: other.to_string(),
                    },
                };
                self.native_done(ticket, out);
                return;
            }
        };
        let mut result = json!({"providerAccepted":true});
        match op {
            Op::Configure(patch) => {
                let echo = self.native_echo.take();
                if ticket.is_none() {
                    self.event(SessionEvent::CodexObservation {
                        accepted: Some(patch),
                        settings: None,
                        title: None,
                    });
                }
                self.initial_settings_gate = false;
                // Native schema defines acceptance as defaults for subsequent
                // turns. It does not promise a per-field effective-value echo.
                let observed = echo
                    .as_ref()
                    .filter(|e| self.native_settings_epoch > e.baseline)
                    .map(|_| observed_settings(&self.native_settings));
                self.native_done(ticket,Out::Observed(json!({"providerAccepted":true,"application":"followingTurns","observedSettings":observed})));
                return;
            }
            Op::HistoryMutation{..} => {
                if value.pointer("/thread/id").and_then(Value::as_str)!=self.thread_id.as_deref() {
                    self.native_done(ticket,Out::Unknown{provider_accepted:true,detail:"native revert ACK has no matching original thread; following input is held".into()});return;
                }
                self.in_flight=false;
                result["application"]=json!("metadata");
                result["providerResult"]=value;
            }
            Op::Name(name) => {
                self.native_name = Some(name.clone());
                self.event(SessionEvent::CodexObservation {
                    accepted: None,
                    settings: None,
                    title: Some(CodexTitleState {
                        desired: Some(name.clone()),
                        native: Some(name.clone()),
                        source: CodexTitleSource::Human,
                        status: CodexTitleStatus::Synced,
                        detail: None,
                    }),
                });
                result["application"] = json!("metadata");
                result["nativeName"] = json!(name);
            }
            Op::Steer {
                expected_turn_id, ..
            } => {
                if value.get("turnId").and_then(Value::as_str) != Some(expected_turn_id.as_str()) {
                    self.native_done(
                        ticket,
                        Out::Unknown {
                            provider_accepted: true,
                            detail: "steer ACK did not confirm exact requested turn".into(),
                        },
                    );
                    return;
                }
                result["application"] = json!("currentTurn");
                result["acceptedTurnId"] = json!(expected_turn_id);
            }
            Op::Interrupt { expected_turn_id } => {
                result["application"] = json!("started");
                result["acceptedTurnId"] = json!(expected_turn_id);
                result["detail"] = json!(
                    "provider accepted interruption; original turn completion and runtime exit are separate observations"
                );
            }
            Op::Review(_) => {
                let Some(turn) = value
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 8192)
                else {
                    self.native_done(ticket,Out::Unknown{provider_accepted:true,detail:"review ACK omitted native turn identity; following input held until explicit stop".into()});
                    return;
                };
                self.tracker.on_turn_accepted(turn);
                self.in_flight = false;
                result["application"] = json!("started");
                result["acceptedTurnId"] = json!(turn);
            }
            Op::Tools => {
                let Some(ticket) = ticket else { return };
                self.request(
                    Tag::NativeToolsSkills {
                        ticket,
                        servers: value,
                    },
                    "skills/list",
                    &json!({"cwds":[self.cwd]}),
                    now,
                    self.config.request_timeout,
                );
                return;
            }
            _ => {
                result["application"] = json!("metadata");
                result["providerResult"] = value;
            }
        }
        self.native_done(ticket, Out::Observed(result));
    }
    pub(super) fn native_tools_result(
        &mut self,
        ticket: u64,
        servers: Value,
        result: Result<Value, RpcFailure>,
    ) {
        self.native_done(Some(ticket),match result {
            Ok(skills)=>Out::Observed(json!({"providerAccepted":true,"application":"metadata","mcpServers":servers,"skills":skills})),
            Err(RpcFailure::Server(e))=>Out::Rejected(e.message),
            Err(e)=>Out::Unknown{provider_accepted:false,detail:e.to_string()},
        });
    }
    pub(super) fn native_settings_echo(&mut self, params: &Value) {
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref() {
            return;
        }
        let Some(s) = params.get("threadSettings").filter(|v| v.is_object()) else {
            return;
        };
        self.native_settings = s.clone();
        self.native_settings_epoch += 1;
        self.event(SessionEvent::CodexObservation {
            accepted: None,
            settings: Some(serde_json::to_value(observed_settings(s)).expect("observed settings")),
            title: None,
        });
    }
    pub(super) fn native_name_echo(&mut self, params: &Value) {
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref() {
            return;
        }
        if let Some(name) = params.get("name").and_then(Value::as_str) {
            self.native_name = Some(name.into());
            // A provider-generated rename is observable, not a new Human title intent.
            self.event(SessionEvent::CodexObservation {
                accepted: None,
                settings: None,
                title: Some(CodexTitleState {
                    desired: None,
                    native: Some(name.into()),
                    source: CodexTitleSource::Task,
                    status: CodexTitleStatus::Synced,
                    detail: None,
                }),
            });
        }
    }
    pub(super) fn native_cancel(&mut self, reason: &str) {
        if let Some(e) = self.native_echo.take() {
            self.native_done(
                e.ticket,
                Out::Unknown {
                    provider_accepted: false,
                    detail: reason.into(),
                },
            );
        }
    }
    fn native_initial_failed(&mut self, ticket: Option<u64>, now: Instant) {
        if ticket.is_none() && self.initial_settings_gate {
            self.initial_settings_failed = true;
            self.initial_settings_gate = false;
            self.abandon(
                "initial Codex options not confirmed; task was not submitted",
                true,
                now,
            );
        }
    }
    pub(super) fn native_opening(&mut self, now: Instant) {
        // Called only after the existing thread Record durable ACK.
        if let Some(title) = self.options.title.clone() {
            self.native_request(None, Op::Name(title), None, now);
        }
        // Start/resume response is real protocol evidence, not desired defaults.
        // Only populated response fields are eligible to resolve mode model.
        if self.native_settings.is_null() {
            self.native_settings =
                json!({"model":self.opened_profile.model,"effort":self.opened_profile.effort});
        }
        if self.options.strict_resume
            || self.options.title.is_some()
            || self.options.initial_settings.is_some()
        {
            self.event(SessionEvent::CodexObservation {
                settings: Some(
                    serde_json::to_value(observed_settings(&self.native_settings))
                        .expect("opening settings"),
                ),
                title: None,
                accepted: None,
            });
        }
        if let Some(patch) = self.options.initial_settings.clone() {
            self.initial_settings_gate = true;
            self.native_request(None, Op::Configure(patch), None, now);
        }
    }
}
