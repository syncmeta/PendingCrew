//! Core-owned control plane only: no document, SQL, code or cell payloads here.
//! All admission/ACL checks belong to the authenticated caller, before disclosure.
use crate::{Result,StoreError,meta_get,meta_set};
use crewd_proto::team_data::{DataContext,DataChangeResult,DataPhase,DataCreated,DataObjectVersion,DataRef};
use rusqlite::Connection;
use serde::{Serialize,Deserialize};
use std::collections::BTreeMap;
const AUTHORITY:&str="teamData.authority.v1";
const PREFIX:&str="teamData.intent.v1.";
const MAX_REQUESTS:i64=16384;
fn invalid(s:&str)->StoreError{StoreError::Invalid(s.into())}
fn same_authority(a:&DataContext,b:&DataContext)->bool{a.db_id==b.db_id&&a.authority_id==b.authority_id&&a.restore_epoch==b.restore_epoch&&a.crew_id==b.crew_id&&a.namespace==b.namespace}
fn transaction(c:&Connection)->Result<()>{if c.is_autocommit(){Err(invalid("data control mutation requires caller transaction"))}else{Ok(())}}
fn uuid(s:&str)->bool{uuid::Uuid::parse_str(s).is_ok_and(|v|v.to_string()==s)}
fn sha(s:&str)->bool{s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Namespace {pub namespace:String,pub file_name:String,pub active_branch:String,pub head:Option<String>}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Authority {pub format:u32,pub authority_id:String,pub restore_epoch:String,pub namespaces:BTreeMap<String,Namespace>}
pub fn authority(c:&Connection)->Result<Option<Authority>>{
 let raw=meta_get(c,AUTHORITY)?;let Some(raw)=raw else{return Ok(None)};
 let value:Authority=serde_json::from_str(&raw).map_err(|_|invalid("data authority corrupt"))?;
 if value.format!=1||!uuid(&value.authority_id)||!uuid(&value.restore_epoch){return Err(invalid("data authority unknown"))}
 let mut namespaces=std::collections::BTreeSet::new();for (crew,n) in &value.namespaces{if crew.is_empty()||crew.len()>256||!uuid(&n.namespace)||n.file_name!=format!("{}.dolt",n.namespace)||n.active_branch.is_empty()||n.active_branch.len()>256||!namespaces.insert(&n.namespace){return Err(invalid("data authority namespace corrupt"))}}
 Ok(Some(value))
}
pub fn set_authority(c:&Connection,value:&Authority)->Result<()>{transaction(c)?;meta_set(c,AUTHORITY,&serde_json::to_string(value)?)?;authority(c)?.ok_or_else(||invalid("authority disappeared"))?;Ok(())}
pub fn context(c:&Connection,crew:&str)->Result<DataContext>{
 let a=authority(c)?.ok_or_else(||invalid("data authority unavailable"))?;let n=a.namespaces.get(crew).ok_or_else(||invalid("data namespace unavailable"))?;
 Ok(DataContext{db_id:meta_get(c,"dbId")?.ok_or_else(||invalid("database identity missing"))?,authority_id:a.authority_id,restore_epoch:a.restore_epoch,crew_id:crew.into(),namespace:n.namespace.clone(),branch:n.active_branch.clone(),head:n.head.clone()})
}
/// Lookup does not compare current HEAD/branch. Historical coordinates remain readable.
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Intent {pub request_id:String,pub context:DataContext,pub principal:String,pub payload_sha256:String,pub created:BTreeMap<String,DataCreated>,pub state:IntentState,pub result:Option<DataChangeResult>}
#[derive(Clone,Copy,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
pub enum IntentState{Pending,Unknown,Settled}
fn key(id:&str)->Result<String>{if !uuid(id){return Err(invalid("invalid data request UUID"))}Ok(format!("{PREFIX}{id}"))}
pub fn lookup(c:&Connection,request:&str)->Result<Option<Intent>>{
 let Some(raw)=meta_get(c,&key(request)?)?else{return Ok(None)};let record:Intent=serde_json::from_str(&raw).map_err(|_|invalid("data intent corrupt"))?;
 if record.request_id!=request||!sha(&record.payload_sha256)||record.principal.is_empty(){return Err(invalid("data intent unknown"))}Ok(Some(record))
}
#[derive(Clone,Debug)]
pub enum Allocation{Object{alias:String,kind:String},Placement{alias:String,parent:PlacementParent}}
#[derive(Clone,Debug)]
pub enum PlacementParent{Created(String),Existing(DataObjectVersion)}
fn alias(s:&str)->bool{!s.is_empty()&&s.len()<=64&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-')}
#[derive(Debug)]
pub enum Reservation{New(Intent),Existing(Intent)}
/// The caller commits this capsule BEFORE dispatch. IDs are allocated once here,
/// not recreated from a rolled-back Dolt branch or a lost reply.
pub fn reserve(c:&Connection,request:&str,expected:&DataContext,principal:&str,payload:&str,allocations:&[Allocation])->Result<Reservation>{
 transaction(c)?;key(request)?;if !sha(payload)||principal.is_empty()||principal.len()>512||allocations.len()>8192{return Err(invalid("invalid bounded data intent"))}
 if let Some(existing)=lookup(c,request)?{if existing.principal!=principal||existing.payload_sha256!=payload||existing.context!=*expected{return Err(invalid("data request UUID immutable payload conflict"))}return Ok(Reservation::Existing(existing))}
 if context(c,&expected.crew_id)?!=*expected{return Err(invalid("data authority, namespace or HEAD changed"))}
 let outstanding:i64=c.query_row("SELECT count(*) FROM meta WHERE key LIKE 'teamData.intent.v1.%'",[],|r|r.get(0))?;if outstanding>=MAX_REQUESTS{return Err(invalid("data retained request quota reached"))}
 let mut stmt=c.prepare("SELECT value FROM meta WHERE key LIKE 'teamData.intent.v1.%'")?;
 for r in stmt.query_map([],|r|r.get::<_,String>(0))?{let r:Intent=serde_json::from_str(&r?).map_err(|_|invalid("data pending witness corrupt"))?;if r.context.namespace==expected.namespace&&r.context.restore_epoch==expected.restore_epoch&&r.state!=IntentState::Settled{return Err(invalid("data namespace has unresolved request; no replay"))}}
 let mut created=BTreeMap::new();let mut names=std::collections::BTreeSet::new();
 for allocation in allocations{let name=match allocation{Allocation::Object{alias,..}|Allocation::Placement{alias,..}=>alias};if !alias(name)||!names.insert(name){return Err(invalid("duplicate or invalid data alias"))}}
 for allocation in allocations{if let Allocation::Object{alias,kind}=allocation{if !["block","view","relation"].contains(&kind.as_str()){return Err(invalid("invalid created object kind"))}created.insert(alias.clone(),DataCreated::Object{object:DataObjectVersion{reference:DataRef{namespace:expected.namespace.clone(),kind:kind.clone(),id:uuid::Uuid::new_v4().to_string()},revision:"1".into()}});}}
 for allocation in allocations{if let Allocation::Placement{alias,parent:source}=allocation{let parent=match source{PlacementParent::Created(name)=>match created.get(name){Some(DataCreated::Object{object})=>object.clone(),_=>return Err(invalid("placement parent alias unavailable"))},PlacementParent::Existing(v)=>v.clone()};if parent.reference.namespace!=expected.namespace||parent.reference.kind!="view"{return Err(invalid("placement parent must be view"))}created.insert(alias.clone(),DataCreated::Placement{id:uuid::Uuid::new_v4().to_string(),parent_view:parent.clone()});}}
 let record=Intent{request_id:request.into(),context:expected.clone(),principal:principal.into(),payload_sha256:payload.into(),created,state:IntentState::Pending,result:None};meta_set(c,&key(request)?,&serde_json::to_string(&record)?)?;Ok(Reservation::New(record))
}
pub fn settle(c:&Connection,request:&str,result:DataChangeResult)->Result<Intent>{
 transaction(c)?;let mut record=lookup(c,request)?.ok_or_else(||invalid("data intent missing"))?;
 if record.state==IntentState::Settled{return Err(invalid("data result already settled"))}
 if result.request_id!=request||!same_authority(&result.context,&record.context)||result.before_head!=record.context.head||serde_json::to_value(&result.created)?!=serde_json::to_value(&record.created)?{return Err(invalid("data settlement immutable attribution changed"))}
 let current=context(c,&record.context.crew_id)?;if current.authority_id!=record.context.authority_id||current.restore_epoch!=record.context.restore_epoch||current.namespace!=record.context.namespace{return Err(invalid("data settlement historical"))}
 record.state=if matches!(result.phase,DataPhase::Unknown){IntentState::Unknown}else{IntentState::Settled};
 // Only known published results advance the authoritative observed head.
 if matches!(result.phase,DataPhase::Committed){let mut a=authority(c)?.ok_or_else(||invalid("data authority missing"))?;let n=a.namespaces.get_mut(&record.context.crew_id).ok_or_else(||invalid("data namespace missing"))?;if n.head!=record.context.head||n.active_branch!=record.context.branch{return Err(invalid("data settlement head changed"))}if result.context.head!=result.after_head{return Err(invalid("data post-context head mismatched"))}n.head=result.after_head.clone();n.active_branch=result.context.branch.clone();set_authority(c,&a)?;}
 record.result=Some(result);meta_set(c,&key(request)?,&serde_json::to_string(&record)?)?;Ok(record)
}
/// Startup never redispatches a pending capsule, even if Dolt currently lacks its row.
pub fn pending_to_unknown(c:&Connection)->Result<usize>{
 transaction(c)?;let rows={let mut s=c.prepare("SELECT key,value FROM meta WHERE key LIKE 'teamData.intent.v1.%'")?;s.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?};let mut changed=0;
 for(k,v)in rows{let mut record:Intent=serde_json::from_str(&v).map_err(|_|invalid("data startup intent corrupt"))?;if record.state==IntentState::Pending{record.state=IntentState::Unknown;meta_set(c,&k,&serde_json::to_string(&record)?)?;changed+=1;}}Ok(changed)
}
/// Paired official restore rotates the epoch; old requests remain history, never replay.
pub fn restored(c:&Connection)->Result<()>{
 transaction(c)?;
 if let Some(mut a)=authority(c)?{
  let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;
  let mut activation=if version==16{
   validate_active(c)?;
   Some(serde_json::from_str::<serde_json::Value>(&meta_get(c,"teamData.activation.v1")?.ok_or_else(||invalid("native restore activation missing"))?).map_err(|_|invalid("native activation corrupt"))?)
  }else{None};
  a.restore_epoch=uuid::Uuid::new_v4().to_string();
  if let Some(ref mut manifest)=activation{manifest["restoreEpoch"]=serde_json::Value::String(a.restore_epoch.clone());meta_set(c,"teamData.activation.v1",&serde_json::to_string(manifest)?)?;}
  set_authority(c,&a)?;
 }Ok(())
}

/// Explicit Human acknowledgement is a control transaction, not a replay of the
/// original operation and not evidence that its effects did or did not happen.
/// `observed_head` must come from the qualified worker under the namespace lock;
/// the caller rechecks authenticated Human/connection/current ACL in this Tx.
pub fn resolve_unknown(c:&Connection,request:&str,expected:&DataContext,principal:&str,payload:&str,original_request:&str,observed_head:&str)->Result<Intent>{
 transaction(c)?;key(request)?;key(original_request)?;
 if request==original_request||!sha(payload)||principal.is_empty()||principal.len()>512||observed_head.len()<20||observed_head.len()>64||!observed_head.bytes().all(|b|b.is_ascii_alphanumeric())||expected.head.as_deref()!=Some(observed_head){return Err(invalid("invalid explicit Unknown acknowledgement"))}
 if let Some(existing)=lookup(c,request)?{
  if existing.principal!=principal||existing.payload_sha256!=payload||existing.context!=*expected{return Err(invalid("data acknowledgement UUID immutable payload conflict"))}
  return Ok(existing)
 }
 let current=context(c,&expected.crew_id)?;
 if !same_authority(&current,expected)||current.branch!=expected.branch{return Err(invalid("data acknowledgement authority or branch changed"))}
 let mut original=lookup(c,original_request)?.ok_or_else(||invalid("original Unknown request unavailable"))?;
 if original.state!=IntentState::Unknown||!same_authority(&original.context,expected){return Err(invalid("original request is not current unresolved Unknown"))}
 // Do not acknowledge a second unknown or supersede a live Pending operation.
 let mut s=c.prepare("SELECT value FROM meta WHERE key LIKE 'teamData.intent.v1.%'")?;
 for row in s.query_map([],|r|r.get::<_,String>(0))?{let other:Intent=serde_json::from_str(&row?).map_err(|_|invalid("data acknowledgement pending witness corrupt"))?;if other.context.namespace==expected.namespace&&other.context.restore_epoch==expected.restore_epoch&&other.state!=IntentState::Settled&&other.request_id!=original_request{return Err(invalid("additional unresolved data request"))}}
 drop(s);
 let count:i64=c.query_row("SELECT count(*) FROM meta WHERE key LIKE 'teamData.intent.v1.%'",[],|r|r.get(0))?;if count>=MAX_REQUESTS{return Err(invalid("data retained request quota reached"))}
 let mut historical=original.result.clone().unwrap_or_else(||DataChangeResult{request_id:original.request_id.clone(),phase:DataPhase::Unknown,context:original.context.clone(),before_head:original.context.head.clone(),after_head:None,commit:None,created:original.created.clone(),changed:vec![],conflicts:vec![],reason:Some("original execution remains Unknown; explicitly acknowledged observed state".into()),resolved_by_request:None});
 if !matches!(historical.phase,DataPhase::Unknown){return Err(invalid("unresolved request has inconsistent phase"))}
 historical.resolved_by_request=Some(request.into());original.result=Some(historical);original.state=IntentState::Settled;
 let result=DataChangeResult{request_id:request.into(),phase:DataPhase::Committed,context:expected.clone(),before_head:expected.head.clone(),after_head:Some(observed_head.into()),commit:None,created:BTreeMap::new(),changed:vec![],conflicts:vec![],reason:Some("observed state acknowledged; original operation was not replayed".into()),resolved_by_request:None};
 let acknowledged=Intent{request_id:request.into(),context:expected.clone(),principal:principal.into(),payload_sha256:payload.into(),created:BTreeMap::new(),state:IntentState::Settled,result:Some(result)};
 let mut authority=authority(c)?.ok_or_else(||invalid("data authority missing"))?;let namespace=authority.namespaces.get_mut(&expected.crew_id).ok_or_else(||invalid("data namespace missing"))?;namespace.head=Some(observed_head.into());set_authority(c,&authority)?;
 meta_set(c,&key(original_request)?,&serde_json::to_string(&original)?)?;meta_set(c,&key(request)?,&serde_json::to_string(&acknowledged)?)?;Ok(acknowledged)
}

#[cfg(test)]
mod tests{
 use super::*;
 fn setup(c:&Connection)->DataContext{let ns=uuid::Uuid::new_v4().to_string();let crew=uuid::Uuid::new_v4().to_string();let a=Authority{format:1,authority_id:uuid::Uuid::new_v4().to_string(),restore_epoch:uuid::Uuid::new_v4().to_string(),namespaces:BTreeMap::from([(crew.clone(),Namespace{namespace:ns.clone(),file_name:format!("{ns}.dolt"),active_branch:"main".into(),head:Some("before".into())})])};set_authority(c,&a).unwrap();context(c,&crew).unwrap()}
 fn result(i:&Intent,phase:DataPhase,head:&str)->DataChangeResult{let mut context=i.context.clone();context.head=Some(head.into());DataChangeResult{request_id:i.request_id.clone(),phase,context,before_head:i.context.head.clone(),after_head:Some(head.into()),commit:Some(head.into()),created:i.created.clone(),changed:vec![],conflicts:vec![],reason:None,resolved_by_request:None}}
 #[test]
 fn disk_pending_restart_is_unknown_and_created_ids_survive_without_content(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("core.db");let mut s=crate::Store::open(&path).unwrap();let ctx=s.with_tx(|c|Ok(setup(c))).unwrap();let request=uuid::Uuid::new_v4().to_string();let payload="a".repeat(64);
  let created=s.with_tx(|c|match reserve(c,&request,&ctx,"human",&payload,&[Allocation::Object{alias:"block".into(),kind:"block".into()}])?{Reservation::New(i)=>Ok(i.created),_=>panic!()}).unwrap();drop(s);
  let mut s=crate::Store::open(&path).unwrap();assert_eq!(s.with_tx(|c|pending_to_unknown(c)).unwrap(),1);let existing=lookup(s.conn(),&request).unwrap().unwrap();assert_eq!(existing.state,IntentState::Unknown);assert_eq!(serde_json::to_value(existing.created).unwrap(),serde_json::to_value(&created).unwrap());
  let repeated=s.with_tx(|c|reserve(c,&request,&ctx,"human",&payload,&[])).unwrap();assert!(matches!(repeated,Reservation::Existing(_)));
  assert!(s.with_tx(|c|reserve(c,&uuid::Uuid::new_v4().to_string(),&ctx,"human",&payload,&[])).is_err());
  assert!(s.with_tx(|c|reserve(c,&request,&ctx,"human",&"b".repeat(64),&[])).is_err());
 }
 #[test]
 fn disk_committed_lookup_ignores_later_head_and_restore_preserves_historical_attribution(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("core.db");let mut s=crate::Store::open(&path).unwrap();let ctx=s.with_tx(|c|Ok(setup(c))).unwrap();let request=uuid::Uuid::new_v4().to_string();let payload="c".repeat(64);
  let intent=s.with_tx(|c|match reserve(c,&request,&ctx,"human",&payload,&[])?{Reservation::New(i)=>Ok(i),_=>panic!()}).unwrap();
  s.with_tx(|c|settle(c,&request,result(&intent,DataPhase::Committed,"after"))).unwrap();drop(s);
  let mut s=crate::Store::open(&path).unwrap();assert_eq!(s.with_tx(|c|pending_to_unknown(c)).unwrap(),0);let original=lookup(s.conn(),&request).unwrap().unwrap();assert_eq!(original.state,IntentState::Settled);assert_eq!(original.context.head,ctx.head);assert_eq!(context(s.conn(),&ctx.crew_id).unwrap().head.as_deref(),Some("after"));
  assert!(matches!(s.with_tx(|c|reserve(c,&request,&ctx,"human",&payload,&[])).unwrap(),Reservation::Existing(_)));
  s.with_tx(|c|restored(c)).unwrap();let current=context(s.conn(),&ctx.crew_id).unwrap();assert_ne!(current.restore_epoch,ctx.restore_epoch);let historical=lookup(s.conn(),&request).unwrap().unwrap();assert_eq!(historical.context.restore_epoch,ctx.restore_epoch);assert_eq!(historical.result.unwrap().after_head.as_deref(),Some("after"));
  assert!(s.with_tx(|c|settle(c,&request,result(&intent,DataPhase::Committed,"replay"))).is_err());
 } #[test]
 fn disk_explicit_unknown_acknowledges_observed_head_without_reclassifying_original_execution(){
  let dir=tempfile::tempdir().unwrap();let path=dir.path().join("core.db");let mut s=crate::Store::open(&path).unwrap();let ctx=s.with_tx(|c|Ok(setup(c))).unwrap();let old=uuid::Uuid::new_v4().to_string();
  s.with_tx(|c|reserve(c,&old,&ctx,"human",&"a".repeat(64),&[])).unwrap();s.with_tx(|c|pending_to_unknown(c)).unwrap();
  let mut observed=ctx.clone();observed.head=Some("a".repeat(32));let request=uuid::Uuid::new_v4().to_string();
  s.with_tx(|c|resolve_unknown(c,&request,&observed,"human",&"b".repeat(64),&old,observed.head.as_deref().unwrap())).unwrap();drop(s);
  let mut s=crate::Store::open(&path).unwrap();let original=lookup(s.conn(),&old).unwrap().unwrap();assert_eq!(original.state,IntentState::Settled);let result=original.result.unwrap();assert!(matches!(result.phase,DataPhase::Unknown));assert_eq!(result.resolved_by_request.as_deref(),Some(request.as_str()));assert_eq!(result.commit,None);
  assert_eq!(context(s.conn(),&ctx.crew_id).unwrap(),observed);assert!(matches!(lookup(s.conn(),&request).unwrap().unwrap().result.unwrap().phase,DataPhase::Committed));
  let fresh=uuid::Uuid::new_v4().to_string();assert!(matches!(s.with_tx(|c|reserve(c,&fresh,&observed,"human",&"c".repeat(64),&[])).unwrap(),Reservation::New(_)));
  assert!(s.with_tx(|c|resolve_unknown(c,&uuid::Uuid::new_v4().to_string(),&observed,"human",&"d".repeat(64),&old,observed.head.as_deref().unwrap())).is_err());
 }

}

/// Crew creation and its namespace reservation share the existing Core Tx.
/// A workspace GET is never allowed to allocate or repair this mapping.
pub fn register_crew(c:&Connection,crew:&str)->Result<()>{
 let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;if version!=16{return Ok(())}
 transaction(c)?;let mut a=authority(c)?.ok_or_else(||invalid("Core16 authority absent"))?;
 if !a.namespaces.contains_key(crew){
  let namespace=uuid::Uuid::new_v4().to_string();a.namespaces.insert(crew.into(),Namespace{file_name:format!("{namespace}.dolt"),namespace,active_branch:"main".into(),head:None});set_authority(c,&a)?;
 }Ok(())
}
/// The caller has exclusively prepared and verified every native file and every
/// legacy source witness under DataDirLock. This function performs no external
/// effect: the authority and format barrier become visible in one Core Tx.
pub fn activate_v16(c:&Connection,prepared:&Authority,manifest:&serde_json::Value)->Result<()>{
 transaction(c)?;let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;
 if version!=15{return Err(invalid("native authority activation requires exact Core15"))}
 if authority(c)?.is_some(){return Err(invalid("native authority already exists; explicit preparation resume required"))}
 let crews={let mut s=c.prepare("SELECT id FROM crews ORDER BY id")?;s.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<std::collections::BTreeSet<_>,_>>()?};
 if prepared.namespaces.keys().cloned().collect::<std::collections::BTreeSet<_>>()!=crews{return Err(invalid("native authority crew coverage changed"))}
 if manifest.get("version").and_then(serde_json::Value::as_u64)!=Some(1)||manifest.get("authorityId").and_then(serde_json::Value::as_str)!=Some(prepared.authority_id.as_str())||manifest.get("restoreEpoch").and_then(serde_json::Value::as_str)!=Some(prepared.restore_epoch.as_str()){return Err(invalid("native activation manifest identity mismatch"))}
 set_authority(c,prepared)?;
 meta_set(c,"teamData.activation.v1",&serde_json::to_string(manifest)?)?;
 meta_set(c,"teamData.format.v1","1")?;
 c.execute_batch("PRAGMA user_version=16;")?;Ok(())
}

pub fn validate_active(c:&Connection)->Result<()>{
 let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;
 if version!=16||meta_get(c,"teamData.format.v1")?.as_deref()!=Some("1"){return Err(invalid("Core16 native format marker missing or unknown"))}
 let a=authority(c)?.ok_or_else(||invalid("Core16 native authority missing"))?;
 let raw=meta_get(c,"teamData.activation.v1")?.ok_or_else(||invalid("Core16 native activation manifest missing"))?;
 let m:serde_json::Value=serde_json::from_str(&raw).map_err(|_|invalid("Core16 activation manifest corrupt"))?;
 if m.get("version").and_then(serde_json::Value::as_u64)!=Some(1)||m.get("authorityId").and_then(serde_json::Value::as_str)!=Some(a.authority_id.as_str())||m.get("restoreEpoch").and_then(serde_json::Value::as_str)!=Some(a.restore_epoch.as_str()){return Err(invalid("Core16 activation identity mismatch"))}Ok(())
}
