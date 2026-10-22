//! Versioned, bounded per-session control metadata in the existing Core store.
//! This is neither provider history nor execution permission. Unknown receipts
//! remain in the original command ledger, never scheduled from this value.
use crate::{Result,StoreError};
use crewd_proto::codex::*;
use rusqlite::Connection;
use serde::{Serialize,Deserialize};

#[derive(Debug,Default,Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct State {
    pub version:u8,
    #[serde(default)] pub fork_pending:bool,
    pub desired_settings:Option<CodexSettingsPatch>,
    pub settings_status:Option<String>,
    pub settings:Option<CodexObservedSettings>,
    pub settings_observed_at:Option<String>,
    pub observed_binding:Option<String>,
    pub observed_thread:Option<String>,
    pub title:CodexTitleState,
    pub intent_request:Option<String>,
    pub title_request:Option<String>,
}
pub fn get(c:&Connection,session:&str)->Result<State> {
    let key=format!("codex.controls.v1/{session}");
    let Some(raw)=crate::meta_get(c,&key)? else{return Ok(State{version:1,..Default::default()})};
    if raw.len()>65536{return Err(StoreError::Invalid("Codex metadata exceeds bound".into()));}
    let state:State=serde_json::from_str(&raw).map_err(|_|StoreError::Invalid("invalid Codex control metadata".into()))?;
    if state.version!=1{return Err(StoreError::Invalid("unknown Codex metadata version".into()));}
    Ok(state)
}
pub fn put(c:&Connection,session:&str,state:&State)->Result<()> {
    if crate::sessions::executor(c,session)?!="codex"{return Err(StoreError::Invalid("Codex control metadata requires Codex source session".into()));}
    let raw=serde_json::to_string(state).map_err(|_|StoreError::Invalid("Codex metadata serialization".into()))?;
    if state.version!=1||raw.len()>65536{return Err(StoreError::Invalid("invalid Codex metadata size/version".into()));}
    crate::meta_set(c,&format!("codex.controls.v1/{session}"),&raw)
}
pub fn initial(c:&Connection,session:&str,patch:Option<CodexSettingsPatch>,human_title:bool)->Result<()> {
    let row=crate::sessions::get(c,session)?.ok_or_else(||StoreError::NotFound(session.into()))?;
    if row.executor!="codex" {
        if patch.is_some(){return Err(StoreError::Invalid("codexOptions require Codex executor".into()));}
        return Ok(());
    }
    let title=row.title.clone().filter(|s|!s.trim().is_empty()).or_else(||row.brief.as_deref().and_then(|b|b.lines().find(|l|!l.trim().is_empty())).map(|l|l.chars().take(120).collect()));
    if let Some(p)=&patch{p.validate().map_err(|s|StoreError::Invalid(s.into()))?;}
    let state=State{version:1,desired_settings:patch,settings_status:None,title:CodexTitleState{desired:title,source:if human_title{CodexTitleSource::Human}else{CodexTitleSource::Task},..Default::default()},..Default::default()};
    put(c,session,&state)
}
pub fn observe(c:&Connection,session:&str,settings:Option<serde_json::Value>,title:Option<CodexTitleState>,accepted:Option<CodexSettingsPatch>,binding:&str)->Result<()> {
    let mut state=get(c,session)?;
    let row=crate::sessions::get(c,session)?.ok_or_else(||StoreError::NotFound(session.into()))?;
    if accepted.as_ref().is_some_and(|p|state.desired_settings.as_ref()==Some(p)){state.settings_status=Some("accepted".into());}
    if let Some(s)=settings {
        // Internal producer already normalized the native spelling.
        state.settings=Some(serde_json::from_value(s).map_err(|_|StoreError::Invalid("invalid observed Codex settings".into()))?);
        state.settings_observed_at=Some(crate::now_ts());
    }
    if let Some(t)=title {
        state.title.native=t.native;
        state.title.status=if state.title.native==state.title.desired&&matches!(t.status,CodexTitleStatus::Synced){CodexTitleStatus::Synced}else{t.status};
        if matches!(state.title.status,CodexTitleStatus::Synced)&&state.title.native!=state.title.desired{state.title.status=CodexTitleStatus::Unknown;}
        state.title.detail=t.detail;
    }
    state.observed_binding=Some(binding.into());state.observed_thread=row.agent_session_id;
    put(c,session,&state)
}
pub fn configure_intent(c:&Connection,session:&str,patch:&CodexSettingsPatch,request:&str)->Result<()> {
    patch.validate().map_err(|s|StoreError::Invalid(s.into()))?;
    let mut state=get(c,session)?;
    let mut merged=serde_json::to_value(state.desired_settings.clone().unwrap_or_default()).map_err(|_|StoreError::Invalid("settings intent serialization".into()))?;
    let next=serde_json::to_value(patch).map_err(|_|StoreError::Invalid("settings patch serialization".into()))?;
    merged.as_object_mut().unwrap().extend(next.as_object().unwrap().clone());
    state.desired_settings=Some(serde_json::from_value(merged).map_err(|_|StoreError::Invalid("settings merge".into()))?);
    state.settings_status=Some("pending".into());state.intent_request=Some(request.into());
    put(c,session,&state)
}
