//! Optional notification facts; only caller-owned FIFO transactions mutate these.
//! Business state remains authoritative. No external send occurs in this crate.
use crate::{Result,StoreError};
use crewd_proto::notifications::*;
use rusqlite::{Connection,OptionalExtension,params};
use serde::{Serialize,Deserialize};
use ring::digest::{digest,SHA256};

pub const DDL:&str=r#"
CREATE TABLE notification_settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1),revision INTEGER NOT NULL CHECK(revision>0),enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),binding TEXT CHECK(binding IS NULL OR json_valid(binding)));
CREATE TABLE notification_installations(id TEXT PRIMARY KEY,authority_id TEXT NOT NULL,db_id TEXT NOT NULL,grant_id TEXT NOT NULL,grant_revision TEXT NOT NULL,client_pin TEXT NOT NULL,generation INTEGER NOT NULL CHECK(generation>0),revoked INTEGER NOT NULL CHECK(revoked IN(0,1)),summary TEXT NOT NULL CHECK(json_valid(summary)),token BLOB);
CREATE INDEX notification_installation_grant ON notification_installations(grant_id);
CREATE TABLE notification_requests(request_id TEXT PRIMARY KEY,owner TEXT NOT NULL,operation TEXT NOT NULL,digest TEXT NOT NULL,result TEXT CHECK(result IS NULL OR json_valid(result)));
CREATE TABLE notification_outbox(id INTEGER PRIMARY KEY AUTOINCREMENT,installation_id TEXT NOT NULL REFERENCES notification_installations(id),source_key TEXT NOT NULL,context TEXT NOT NULL CHECK(json_valid(context)),session_id TEXT,generation TEXT,enqueued_at INTEGER NOT NULL,expires_at INTEGER NOT NULL,state TEXT NOT NULL CHECK(state IN('pending','unknown','accepted','rejected','suppressed')),attempt TEXT UNIQUE,attempt_generation INTEGER,reason TEXT,UNIQUE(installation_id,source_key));
CREATE INDEX notification_pending ON notification_outbox(state,id);
"#;
fn invalid(s:&str)->StoreError {StoreError::Invalid(s.into())}
fn tx(c:&Connection)->Result<()> {if c.is_autocommit(){Err(invalid("notification mutation requires owner transaction"))}else{Ok(())}}
pub fn hash(data:&[u8])->String {digest(&SHA256,data).as_ref().iter().map(|b|format!("{b:02x}")).collect()}
fn json<T:Serialize>(v:&T)->Result<String>{Ok(serde_json::to_string(v)?)}
pub fn number(s:&str,zero:bool)->Result<i64> {let n=s.parse::<i64>().map_err(|_|invalid("invalid notification revision"))?;if n.to_string()!=s||n<if zero{0}else{1}{return Err(invalid("invalid notification revision"))}Ok(n)}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ProviderBinding {pub authority_id:String,pub db_id:String,pub team_id:String,pub key_id:String,pub environment:PushEnvironment,pub topic:String,pub key_slot:String,pub seal_slot:String}
#[derive(Clone,Debug)]
pub struct Settings {pub revision:i64,pub enabled:bool,pub binding:Option<ProviderBinding>}
pub fn settings(c:&Connection)->Result<Settings> {
    let row:Option<(i64,bool,Option<String>)>=c.query_row("SELECT revision,enabled,binding FROM notification_settings WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    match row {Some((revision,enabled,binding))=>Ok(Settings{revision,enabled,binding:binding.map(|s|serde_json::from_str(&s)).transpose()?}),None=>Ok(Settings{revision:0,enabled:false,binding:None})}
}
#[derive(Clone,Debug)]
pub struct Owner {pub authority_id:String,pub db_id:String,pub grant_id:String,pub grant_revision:String,pub client_pin:String}
impl Owner {
    pub fn key(&self)->String {format!("{}:{}:{}",self.authority_id,self.db_id,self.grant_id)}
    pub fn authorize(&self,c:&Connection)->Result<()> {
        let identity=crate::remote::identity(c)?.ok_or_else(||invalid("notification authority unavailable"))?;
        if identity.authority_id!=self.authority_id||identity.db_id!=self.db_id||crate::meta_get(c,"dbId")?.as_deref()!=Some(self.db_id.as_str()){return Err(invalid("notification identity mismatch"))}
        let grant=crate::remote::grant(c,&self.grant_id,&self.authority_id)?.ok_or_else(||invalid("notification owner unavailable"))?;
        if grant.revoked_at.is_some()||grant.revision!=self.grant_revision||grant.client_spki_sha256!=self.client_pin||!crate::remote::configuration(c)?.enabled{return Err(invalid("notification owner revoked"))}Ok(())
    }
}
pub fn request_lookup(c:&Connection,owner:&str,request:&str)->Result<NotificationRequestLookup> {
    let row:Option<(String,String,Option<String>)>=c.query_row("SELECT owner,operation,result FROM notification_requests WHERE request_id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    match row {None=>Ok(NotificationRequestLookup::NotFound),Some((o,_,_)) if o!=owner=>Err(invalid("notification request owner mismatch")),Some((_,operation,None))=>Ok(NotificationRequestLookup::Unknown{operation}),Some((_,operation,Some(result)))=>Ok(NotificationRequestLookup::Known{operation,result:serde_json::from_str(&result)?})}
}
/// True means new. Existing identical UUID is read-only, never redoes an effect.
pub fn reserve(c:&Connection,owner:&str,request:&str,operation:&str,body_hash:&str)->Result<bool> {
    tx(c)?;if !crewd_proto::valid_remote_uuid(request)||body_hash.len()!=64{return Err(invalid("invalid notification request"))}
    let old:Option<(String,String,String)>=c.query_row("SELECT owner,operation,digest FROM notification_requests WHERE request_id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some(old)=old {if old!=(owner.into(),operation.into(),body_hash.into()){return Err(invalid("notification original request conflict"))}return Ok(false)}
    c.execute("INSERT INTO notification_requests(request_id,owner,operation,digest) VALUES(?1,?2,?3,?4)",params![request,owner,operation,body_hash])?;Ok(true)
}
pub fn result(c:&Connection,owner:&str,request:&str,value:&serde_json::Value)->Result<()> {
    tx(c)?;if c.execute("UPDATE notification_requests SET result=?1 WHERE request_id=?2 AND owner=?3 AND result IS NULL",params![json(value)?,request,owner])?!=1{return Err(invalid("notification result no longer pending"))}Ok(())
}
pub fn configure(c:&Connection,expected:i64,enabled:bool,binding:Option<&ProviderBinding>)->Result<Settings> {
    tx(c)?;let old=settings(c)?;if old.revision!=expected||expected==i64::MAX||enabled&&binding.is_none(){return Err(invalid("notification settings CAS conflict"))}
    let rev=expected+1;c.execute("INSERT INTO notification_settings(singleton,revision,enabled,binding) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,enabled=excluded.enabled,binding=excluded.binding",params![rev,enabled,binding.map(json).transpose()?])?;
    // A provider change never sends old pending observations through a new binding.
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='configuration_changed' WHERE state='pending'",[])?;
    settings(c)
}
pub fn installation(c:&Connection,owner:&Owner,id:&str)->Result<Option<NotificationInstallation>> {
    let row:Option<(String,String,String)>=c.query_row("SELECT grant_id,client_pin,summary FROM notification_installations WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    match row {None=>Ok(None),Some((grant,pin,_)) if grant!=owner.grant_id||pin!=owner.client_pin=>Err(invalid("notification installation owner mismatch")),Some((_,_,summary))=>Ok(Some(serde_json::from_str(&summary)?))}
}
pub fn token_aad(owner:&Owner,summary:&NotificationInstallation)->Result<Vec<u8>> {Ok(serde_json::to_vec(&(owner.key(),&summary.installation_id,&summary.generation,summary.environment,&summary.topic))?)}
pub fn register(c:&Connection,owner:&Owner,p:&NotificationRegisterParams,sealed:&[u8])->Result<NotificationInstallation> {
    tx(c)?;owner.authorize(c)?;
    if !crewd_proto::valid_remote_uuid(&p.installation_id)||p.crew_ids.is_empty()||p.crew_ids.len()>64||p.kinds.is_empty()||p.kinds.len()>8||sealed.len()<28||sealed.len()>1024{return Err(invalid("invalid notification installation"))}
    let config=settings(c)?;let binding=config.binding.ok_or_else(||invalid("notifications not configured"))?;
    if !config.enabled||binding.authority_id!=owner.authority_id||binding.db_id!=owner.db_id||p.topic!=binding.topic||p.environment!=binding.environment{return Err(invalid("notification provider scope mismatch"))}
    for (i,crew) in p.crew_ids.iter().enumerate(){if p.crew_ids[..i].contains(crew)||!crew_live(c,crew)?{return Err(invalid("notification crew subscription denied"))}}
    for (i,kind) in p.kinds.iter().enumerate(){if p.kinds[..i].contains(kind){return Err(invalid("duplicate notification kind"))}}
    let expected=number(&p.expected_generation,true)?;let old=installation(c,owner,&p.installation_id)?;
    if old.as_ref().is_some_and(|v|v.revoked)||old.as_ref().map(|v|number(&v.generation,false)).transpose()?.unwrap_or(0)!=expected||expected==i64::MAX{return Err(invalid("notification installation CAS conflict"))}
    if old.is_none(){let count:i64=c.query_row("SELECT COUNT(*) FROM notification_installations",[],|r|r.get(0))?;if count>=256{return Err(invalid("notification installation bound reached"))}}
    let value=NotificationInstallation{installation_id:p.installation_id.clone(),generation:(expected+1).to_string(),revoked:false,environment:p.environment,topic:p.topic.clone(),crew_ids:p.crew_ids.clone(),kinds:p.kinds.clone()};
    c.execute("INSERT INTO notification_installations(id,authority_id,db_id,grant_id,grant_revision,client_pin,generation,revoked,summary,token) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8,?9) ON CONFLICT(id) DO UPDATE SET generation=excluded.generation,summary=excluded.summary,token=excluded.token",params![p.installation_id,owner.authority_id,owner.db_id,owner.grant_id,owner.grant_revision,owner.client_pin,expected+1,json(&value)?,sealed])?;Ok(value)
}
pub fn revoke(c:&Connection,owner:&Owner,id:&str,expected:i64)->Result<NotificationInstallation> {
    tx(c)?;owner.authorize(c)?;let mut value=installation(c,owner,id)?.ok_or_else(||invalid("notification installation missing"))?;
    if value.revoked||number(&value.generation,false)?!=expected||expected==i64::MAX{return Err(invalid("notification installation CAS conflict"))}
    value.generation=(expected+1).to_string();value.revoked=true;
    c.execute("UPDATE notification_installations SET generation=?1,revoked=1,summary=?2,token=NULL WHERE id=?3",params![expected+1,json(&value)?,id])?;
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='installation_revoked' WHERE installation_id=?1 AND state='pending'",[id])?;Ok(value)
}
fn crew_live(c:&Connection,crew:&str)->Result<bool>{Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1 AND archived_at IS NULL)",[crew],|r|r.get(0))?)}
/// Only called after a genuine source event is admitted by its owning producer.
/// Zero configured/matching subscriptions creates no observation rows.
pub fn enqueue(c:&Connection,crew:&str,kind:NotificationKind,source:NotificationSource,session:Option<(&str,&str)>,now:i64)->Result<()> {
    tx(c)?;let config=settings(c)?;if !config.enabled{return Ok(())}let Some(binding)=config.binding else{return Ok(())};
    if !crew_live(c,crew)?||now<0{return Ok(())}
    let context=NotificationContext{action:"open_context".into(),authority_id:binding.authority_id.clone(),db_id:binding.db_id.clone(),crew_id:crew.into(),kind,source};
    let raw=json(&context)?;if raw.len()>3072{return Err(invalid("notification source context exceeds bound"))}let source_key=hash(raw.as_bytes());
    let mut query=c.prepare("SELECT id,grant_id,grant_revision,client_pin,summary FROM notification_installations WHERE revoked=0 AND authority_id=?1 AND db_id=?2 ORDER BY id LIMIT 257")?;
    let rows=query.query_map(params![binding.authority_id,binding.db_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len()>256{return Err(invalid("notification installation bound exceeded"))}
    for (id,grant_id,grant_revision,client_pin,summary) in rows {
        let summary:NotificationInstallation=serde_json::from_str(&summary)?;
        let owner=Owner{authority_id:binding.authority_id.clone(),db_id:binding.db_id.clone(),grant_id,grant_revision,client_pin};
        if owner.authorize(c).is_err()||!summary.crew_ids.iter().any(|v|v==crew)||!summary.kinds.contains(&kind){continue}
        let expires=now.checked_add(86_400_000).ok_or_else(||invalid("notification expiry overflow"))?;
        c.execute("INSERT INTO notification_outbox(installation_id,source_key,context,session_id,generation,enqueued_at,expires_at,state) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending') ON CONFLICT(installation_id,source_key) DO NOTHING",params![id,source_key,raw,session.map(|v|v.0),session.map(|v|v.1),now,expires])?;
    }Ok(())
}
pub fn enqueue_work(c:&Connection,crew:&str,event:&crewd_work::Event)->Result<()> {
    // Historical pre-activation migration seams have no notification writer.
    // Production Store admission enforces current19 before any public mutation.
    let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;
    if version<19{return Ok(())}
    use crewd_work::EventKind::*;use crate::object_refs::ObjectKind;
    let (kind,work,ask,object)=match &event.kind{
        Done{id,..}=>(NotificationKind::Done,Some(*id),None,ObjectKind::Plan),
        Reported{id,..}=>(NotificationKind::Reported,Some(*id),None,ObjectKind::Plan),
        Blocked{id,..}=>(NotificationKind::Blocked,Some(*id),None,ObjectKind::Plan),
        AskOpened{ask,item,..}=>(NotificationKind::AskOpened,*item,Some(*ask),ObjectKind::HumanTask),
        _=>return Ok(())};
    // Do not resolve/index anything while notification delivery is unconfigured.
    if !settings(c)?.enabled{return Ok(())}
    let core=ask.map(|v|v.to_string()).or_else(||work.map(|v|v.to_string())).ok_or_else(||invalid("notification source ID missing"))?;
    let reference=crate::work_organization::reference_for_core(c,object,&core)?.ok_or_else(||invalid("notification public source identity missing"))?;
    enqueue(c,crew,kind,NotificationSource::WorkEvent{event_seq:event.seq.to_string(),work_id:work.map(|v|v.to_string()),ask_id:ask.map(|v|v.to_string()),entry_no:None,object_id:reference.id()},None,event.at.0)
}
/// These reminders represent a current live observation, not permanent work truth.
pub fn suppress_session(c:&Connection,session:&str,reason:&str)->Result<()> {tx(c)?;c.execute("UPDATE notification_outbox SET state='suppressed',reason=?1 WHERE session_id=?2 AND state='pending'",params![reason,session])?;Ok(())}
pub fn restart(c:&Connection)->Result<()> {tx(c)?;c.execute("UPDATE notification_outbox SET state='suppressed',reason='runtime_reopened' WHERE session_id IS NOT NULL AND state='pending'",[])?;Ok(())}
pub fn restored(c:&Connection)->Result<()> {
    tx(c)?;
    let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;
    if version<19{return Ok(())}c.execute("UPDATE notification_settings SET enabled=0",[])?;
    let mut q=c.prepare("SELECT id,summary FROM notification_installations WHERE revoked=0")?;let rows=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;drop(q);
    for (id,raw) in rows{let mut value:NotificationInstallation=serde_json::from_str(&raw)?;value.revoked=true;c.execute("UPDATE notification_installations SET revoked=1,token=NULL,summary=?1 WHERE id=?2",params![json(&value)?,id])?;}
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='database_restored' WHERE state='pending'",[])?;Ok(())
}
pub struct Claimed {pub delivery_id:i64,pub installation:NotificationInstallation,pub owner:Owner,pub sealed_token:Vec<u8>,pub context:NotificationContext,pub attempt:String,pub expires_at:i64,pub config_revision:i64}
/// Unknown is durable before returning the external-I/O permit. Never selects Unknown again.
pub fn claim(c:&Connection,now:i64)->Result<Option<Claimed>> {
    tx(c)?;let config=settings(c)?;if !config.enabled{return Ok(None)}
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='expired' WHERE state='pending' AND expires_at<=?1",[now])?;
    let candidate:Option<(i64,String,String,i64)>=c.query_row("SELECT id,installation_id,context,expires_at FROM notification_outbox WHERE state='pending' ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((delivery_id,id,context,expires_at))=candidate else{return Ok(None)};
    let context:NotificationContext=serde_json::from_str(&context)?;
    let row:(String,String,String,String,String,String,Option<Vec<u8>>,bool)=c.query_row("SELECT authority_id,db_id,grant_id,grant_revision,client_pin,summary,token,revoked FROM notification_installations WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
    let owner=Owner{authority_id:row.0,db_id:row.1,grant_id:row.2,grant_revision:row.3,client_pin:row.4};let installation:NotificationInstallation=serde_json::from_str(&row.5)?;
    let live= !row.7&&row.6.is_some()&&owner.authorize(c).is_ok()&&crew_live(c,&context.crew_id)?&&installation.crew_ids.contains(&context.crew_id)&&installation.kinds.contains(&context.kind)&&config.binding.as_ref().is_some_and(|b|b.authority_id==context.authority_id&&b.db_id==context.db_id&&b.environment==installation.environment&&b.topic==installation.topic);
    if !live {c.execute("UPDATE notification_outbox SET state='suppressed',reason='authority_changed' WHERE id=?1 AND state='pending'",[delivery_id])?;return Ok(None)}
    let attempt=uuid::Uuid::new_v4().to_string();let generation=number(&installation.generation,false)?;
    if c.execute("UPDATE notification_outbox SET state='unknown',attempt=?1,attempt_generation=?2 WHERE id=?3 AND state='pending'",params![attempt,generation,delivery_id])?!=1{return Err(invalid("notification claim conflict"))}
    Ok(Some(Claimed{delivery_id,installation,owner,sealed_token:row.6.unwrap(),context,attempt,expires_at,config_revision:config.revision}))
}
pub fn still_allowed(c:&Connection,claimed:&Claimed,now:i64)->Result<bool>{let config=settings(c)?;Ok(config.enabled&&config.revision==claimed.config_revision&&claimed.expires_at>now&&claimed.owner.authorize(c).is_ok()&&installation(c,&claimed.owner,&claimed.installation.installation_id)?.is_some_and(|s|!s.revoked&&s.generation==claimed.installation.generation)&&crew_live(c,&claimed.context.crew_id)?)}
pub fn settle(c:&Connection,id:i64,attempt:&str,state:NotificationDeliveryState,reason:Option<&str>,invalidate_token:bool)->Result<()> {
    tx(c)?;let target=match state{NotificationDeliveryState::AcceptedByApns=>"accepted",NotificationDeliveryState::Rejected=>"rejected",NotificationDeliveryState::Suppressed=>"suppressed",NotificationDeliveryState::Unknown=>return Ok(()),NotificationDeliveryState::Pending=>return Err(invalid("notification cannot requeue"))};
    if c.execute("UPDATE notification_outbox SET state=?1,reason=?2 WHERE id=?3 AND attempt=?4 AND state='unknown'",params![target,reason,id,attempt])?!=1{return Err(invalid("notification original attempt conflict"))}
    if invalidate_token {
        let (installation_id,generation):(String,i64)=c.query_row("SELECT installation_id,attempt_generation FROM notification_outbox WHERE id=?1 AND attempt=?2",params![id,attempt],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let summary:Option<String>=c.query_row("SELECT summary FROM notification_installations WHERE id=?1 AND generation=?2 AND revoked=0",params![installation_id,generation],|r|r.get(0)).optional()?;
        if let Some(raw)=summary {let mut value:NotificationInstallation=serde_json::from_str(&raw)?;value.revoked=true;c.execute("UPDATE notification_installations SET revoked=1,token=NULL,summary=?1 WHERE id=?2 AND generation=?3",params![json(&value)?,installation_id,generation])?;c.execute("UPDATE notification_outbox SET state='suppressed',reason='invalid_token' WHERE installation_id=?1 AND state='pending'",[installation_id])?;}
    }Ok(())
}
pub fn deliveries(c:&Connection,owner:&Owner,p:&NotificationDeliveryListParams)->Result<NotificationDeliveryPage>{
    owner.authorize(c)?;installation(c,owner,&p.installation_id)?.ok_or_else(||invalid("notification installation missing"))?;if p.limit==0||p.limit>100{return Err(invalid("notification page bound"))}let after=p.after_delivery_id.as_deref().map(|v|number(v,true)).transpose()?.unwrap_or(0);
    let mut q=c.prepare("SELECT id,context,state,attempt,reason,expires_at FROM notification_outbox WHERE installation_id=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
    let rows=q.query_map(params![p.installation_id,after,p.limit+1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;let more=rows.len()>p.limit as usize;let mut items=Vec::new();
    for (id,context,state,apns_id,reason,expiry) in rows.into_iter().take(p.limit as usize){let state=match state.as_str(){"pending"=>NotificationDeliveryState::Pending,"unknown"=>NotificationDeliveryState::Unknown,"accepted"=>NotificationDeliveryState::AcceptedByApns,"rejected"=>NotificationDeliveryState::Rejected,"suppressed"=>NotificationDeliveryState::Suppressed,_=>return Err(invalid("invalid notification state"))};items.push(NotificationDelivery{delivery_id:id.to_string(),context:serde_json::from_str(&context)?,state,apns_id,reason,expires_at:crate::ts_from_ms(expiry)});}
    Ok(NotificationDeliveryPage{next_after_delivery_id:if more{items.last().map(|v|v.delivery_id.clone())}else{None},items})
}
