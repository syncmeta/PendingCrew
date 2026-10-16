//! One authenticated, bounded Core read cut shared by compilation/projection.
//! No database file is opened by Core and no JSON identity establishes access.
use super::{compile,projection,worker::SiblingWorker};
use crate::{AppState,connection_scope::ConnectionScope,public_commands::Actor};
use crewd_collab::{self as collab,Sender};
use crewd_data_protocol::{Operation,Request,Response,Capture};
use crewd_exec::registry::ProcessRegistry;
use crewd_proto::{ResourceActor,team_data::*};
use crewd_store::team_data::{self as control,Namespace};
use std::{path::{Path,PathBuf},sync::Arc,time::{Instant,Duration},os::unix::fs::MetadataExt,io};
use serde::Deserialize;

#[derive(Clone)]
pub(crate) struct Access{source:Source,principal:String}
#[derive(Clone)]enum Source{Human(Actor),Session{session:String,token:String,scope:ConnectionScope}}
impl Access{
 pub(crate) async fn human(state:&Arc<AppState>,actor:Actor)->Result<Self,crewd_proto::RpcError>{let principal=actor.principal(state).await?;Ok(Self{source:Source::Human(actor),principal})}
 /// Called only by the authenticated MCP connection/token adapter, never from
 /// public JSON session IDs. Every actual job still reloads crew membership.
 pub(crate) fn attached_session(session:String,token:String,scope:ConnectionScope)->Self{Self{principal:format!("session:{session}"),source:Source::Session{session,token,scope}}}
 pub(crate) fn principal(&self)->&str{&self.principal}
 pub(crate) fn is_human(&self)->bool{matches!(self.source,Source::Human(_))}
 pub(crate) fn check(&self,state:&Arc<AppState>,c:&rusqlite::Connection,crew:&str,write:bool)->collab::Result<ResourceActor>{
  let by=match &self.source{
   Source::Human(actor)=>{actor.authorize_tx(state,c,if write{"data.submit"}else{"data.workspace.get"})?;Sender::Human},
   Source::Session{session,token,scope}=>{if !scope.is_live()||!state.tokens.verify(session,token){return Err(collab::CollabError::Invalid(collab::refusal::Refusal::new(collab::refusal::Code::ResourceForbidden)))}Sender::Session(session.clone())}
  };
  collab::team_data::authorize(c,&by,crew,write)
 }
}
fn same_identity(a:&DataContext,b:&DataContext)->bool{a.db_id==b.db_id&&a.authority_id==b.authority_id&&a.restore_epoch==b.restore_epoch&&a.crew_id==b.crew_id&&a.namespace==b.namespace}
fn unavailable()->String{"data source unavailable or access changed".into()}
fn qualify_root(root:&Path)->Result<(),String>{let m=std::fs::symlink_metadata(root).map_err(|_|unavailable())?;if !root.is_absolute()||std::fs::canonicalize(root).map_err(|_|unavailable())?!=root||!m.is_dir()||m.uid()!=unsafe{libc::geteuid()}||m.mode()&0o777!=0o700{return Err(unavailable())}Ok(())}

pub(crate) struct ReadCut{
 state:Arc<AppState>,access:Access,origin:DataContext,root:PathBuf,worker:SiblingWorker,registry:Arc<ProcessRegistry>,deadline:Instant,
}
impl ReadCut{
 /// The owning runtime must hold its global namespace publication mutex for
 /// this cut. This makes same-Core foreign captures coherent; it does not claim
 /// cross-process or cross-file SQL atomicity.
 pub(crate) fn new(state:Arc<AppState>,access:Access,origin:DataContext,root:PathBuf,worker:SiblingWorker,registry:Arc<ProcessRegistry>)->Result<Self,String>{
  qualify_root(&root)?;worker.still_qualified().map_err(|_|unavailable())?;
  let this=Self{state,access,origin,root,worker,registry,deadline:Instant::now()+Duration::from_secs(60)};this.context(&this.origin.namespace,false)?;Ok(this)
 }
 pub(crate) fn db<T:Send+'static>(&self,f:impl FnOnce(&rusqlite::Connection)->collab::Result<T>+Send+'static)->Result<T,String>{self.state.db.call_result_blocking(move|s|f(s.conn())).map_err(|_|unavailable())?.map_err(|_|unavailable())}
 pub(crate) fn context(&self,namespace:&str,write:bool)->Result<(DataContext,Namespace,ResourceActor),String>{
  if Instant::now()>=self.deadline{return Err("data read cut deadline exhausted".into())}
  let namespace=namespace.to_owned();let state=self.state.clone();let access=self.access.clone();let origin=self.origin.clone();
  self.db(move|c|{
   let format:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?;if format!=16{return Err(crewd_store::StoreError::Invalid("Core16 data authority not active".into()).into())}
   let authority=control::authority(c)?.ok_or_else(||crewd_store::StoreError::Invalid("data authority unavailable".into()))?;
   let (crew,n)=authority.namespaces.iter().find(|(_,n)|n.namespace==namespace).ok_or_else(||crewd_store::StoreError::Invalid("namespace unavailable".into()))?;
   // Origin is independently qualified, even when reading an authorized target.
   let original=control::context(c,&origin.crew_id)?;access.check(&state,c,&origin.crew_id,false)?;
   if !same_identity(&original,&origin){return Err(crewd_store::StoreError::Invalid("data origin changed".into()).into())}
   let actor=access.check(&state,c,crew,write)?;let context=control::context(c,crew)?;
   Ok((context,n.clone(),actor))
  })
 }
 pub(crate) fn call(&self,namespace:&str,at_context:Option<&DataContext>,operation:Operation,write:bool)->Result<serde_json::Value,String>{
  let (context,n,_)=self.context(namespace,write)?;
  if let Some(expected)=at_context{if !same_identity(expected,&context)||(write&&*expected!=context){return Err("data context changed".into())}}
  let file=self.root.join(n.file_name);let m=std::fs::symlink_metadata(&file).map_err(|_|unavailable())?;
  if !m.is_file()||m.nlink()!=1||m.uid()!=unsafe{libc::geteuid()}||m.mode()&0o022!=0{return Err(unavailable())}
  let expected=context.clone();let request=Request{id:uuid::Uuid::new_v4().to_string(),namespace:namespace.into(),branch:context.branch.clone(),operation};
  let verify=||->io::Result<()>{qualify_root(&self.root).map_err(io::Error::other)?;let (now,_,_)=self.context(&expected.namespace,write).map_err(io::Error::other)?;if !same_identity(&now,&expected)||(write&&now!=expected){return Err(io::Error::other("data context changed"))}Ok(())};
  let (response,_)=self.worker.exchange_until(&file,if write{"existing"}else{"readonly"},request,self.registry.clone(),&verify,self.deadline).map_err(|_|"data worker outcome unavailable".to_string())?;
  match response{Response::Ok(value)=>Ok(value),Response::Refused{..}=>Err("data operation refused".into()),Response::Unknown{..}=>Err("data operation outcome Unknown".into())}
 }
 pub(crate) fn affected_live_views(&self,target:&DataRef)->Result<(Vec<DataRef>,bool),String>{
  let state=self.state.clone();let access=self.access.clone();let origin=self.origin.clone();
  let namespaces=self.db(move|c|{
   access.check(&state,c,&origin.crew_id,false)?;let authority=control::authority(c)?.ok_or_else(||crewd_store::StoreError::Invalid("data authority missing".into()))?;
   let mut visible=Vec::new();for(crew,n)in authority.namespaces{if access.check(&state,c,&crew,false).is_ok()&&n.head.is_some(){visible.push(n.namespace);}}Ok(visible)
  })?;
  let mut affected=Vec::new();let mut truncated=false;
  for namespace in namespaces{
   if Instant::now()>=self.deadline{truncated=true;break}
   let mut after=None;
   loop{
    if Instant::now()>=self.deadline{truncated=true;break}
    let page=match self.call(&namespace,None,Operation::Objects{kind:"view".into(),after:after.clone(),limit:100,at_commit:None},false){Ok(page)=>page,Err(_)=>{self.context(&self.origin.namespace,false)?;truncated=true;break}};
    let items=page.get("items").and_then(serde_json::Value::as_array).ok_or_else(unavailable)?;
    for item in items{
     #[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Stored{object:DataObject,captures:Vec<Capture>}
     let stored:Stored=serde_json::from_value(item.clone()).map_err(|_|unavailable())?;
     let DataObject::View{id,archived,..}=stored.object else{return Err(unavailable())};
     if !archived&&stored.captures.iter().any(|c|c.live&&c.namespace==target.namespace&&c.kind==target.kind&&c.id==target.id){
      if affected.len()==100{truncated=true;break}
      affected.push(DataRef{namespace:namespace.clone(),kind:"view".into(),id});
     }
    }
    if affected.len()==100&&truncated{break}
    let next=page.get("next").and_then(serde_json::Value::as_str).map(str::to_owned);
    if next.is_none(){break}if next==after||items.is_empty(){return Err("data page cursor did not advance".into())}after=next;
   }
   if affected.len()==100&&truncated{break}
  }
  self.context(&self.origin.namespace,false)?;Ok((affected,truncated))
 }
 fn stored(&self,reference:&DataRef,at:Option<&str>)->Result<(compile::Stored,DataContext,String),String>{
  if !["block","view","relation"].contains(&reference.kind.as_str()){return Err(unavailable())}
  let (context,_,_)=self.context(&reference.namespace,false)?;let commit=at.map(str::to_owned).or_else(||context.head.clone()).ok_or_else(unavailable)?;
  let value=self.call(&reference.namespace,None,Operation::Object{kind:reference.kind.clone(),id:reference.id.clone(),at_commit:Some(commit.clone())},false)?;
  #[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Stored{object:DataObject,captures:Vec<Capture>}
  let stored:Stored=serde_json::from_value(value).map_err(|_|unavailable())?;
  let(kind,id)=match &stored.object{DataObject::Block{id,..}=>("block",id),DataObject::View{id,..}=>("view",id),DataObject::Relation{id,..}=>("relation",id)};
  if kind!=reference.kind||id!=&reference.id{return Err(unavailable())}
  Ok((compile::Stored{object:stored.object,captures:stored.captures},context,commit))
 }
 pub(crate) fn detail(&self,params:&DataObjectGetParams)->Result<DataObjectDetail,String>{
  if params.context!=self.origin{return Err("original data read context changed".into())}
  let (stored,resolved_context,commit)=self.stored(&params.reference,params.at_commit.as_deref())?;
  let (object,references)=projection::project(stored.object,&stored.captures,&params.reference.namespace,&commit,params.at_commit.is_some(),self)?;
  let (affected_live_views,affected_truncated)=self.affected_live_views(&params.reference)?;
  Ok(DataObjectDetail{context:self.origin.clone(),resolved_context,commit,object,references,affected_live_views,affected_truncated})
 }
}
impl compile::Port for ReadCut{
 fn object(&self,reference:&DataRef,at:Option<&str>)->Result<compile::Stored,String>{self.stored(reference,at).map(|v|v.0)}
 fn target(&self,reference:&DataRef,at:Option<&str>)->Result<compile::Target,String>{
  if ["block","view","relation"].contains(&reference.kind.as_str()){
   let (stored,_,commit)=self.stored(reference,at)?;let revision=match stored.object{DataObject::Block{revision,..}|DataObject::View{revision,..}|DataObject::Relation{revision,..}=>revision};return Ok(compile::Target{commit:Some(commit),revision:Some(revision)})
  }
  if at.is_some(){return Err("Core references do not have Dolt snapshots".into())}
  let (context,_,_)=self.context(&reference.namespace,false)?;let reference=reference.clone();
  self.db(move|c|{
   let value=match reference.kind.as_str(){"requirement"=>serde_json::to_value(collab::requirements::get(c,&reference.id)?),"plan"=>serde_json::to_value(collab::work::get(c,&reference.id)?),_=>return Err(crewd_store::StoreError::Invalid("unsupported Core link kind".into()).into())}.map_err(crewd_store::StoreError::from)?;
   if value.get("crewId").and_then(serde_json::Value::as_str)!=Some(context.crew_id.as_str()){return Err(crewd_store::StoreError::Invalid("Core link crew mismatch".into()).into())}Ok(())
  })?;Ok(compile::Target{commit:None,revision:None})
 }
}
impl projection::Resolver for ReadCut{
 fn resolve(&self,reference:&DataRef,at:Option<&str>)->Result<Option<projection::Resolved>,String>{
  use compile::Port;
  match self.target(reference,at){Ok(target)=>Ok(Some(projection::Resolved{complete:target.commit.is_some()&&target.revision.is_some(),commit:target.commit,revision:target.revision})),Err(_)=>{
   // Target-specific absence/denial is intentionally indistinguishable. But a
   // closed original connection/restore cut must fail the entire response.
   self.context(&self.origin.namespace,false)?;Ok(None)
  }}
 }
}

impl ReadCut {
 pub(crate) fn authorize_write(&self)->Result<ResourceActor,String>{self.context(&self.origin.namespace,true).map(|v|v.2)}
 pub(crate) fn reserve(&self,params:&DataSubmitParams,payload:&str,allocations:Vec<control::Allocation>)->Result<control::Reservation,String>{
  let state=self.state.clone();let access=self.access.clone();let origin=self.origin.clone();let p=params.clone();let payload=payload.to_owned();
  let principal=access.principal().to_owned();
  self.state.db.call_result_blocking(move|s|s.with_tx(|c|{
   access.check(&state,c,&origin.crew_id,true).map_err(|_|crewd_store::StoreError::Invalid("data write permission changed".into()))?;
   if !same_identity(&control::context(c,&origin.crew_id)?,&origin){return Err(crewd_store::StoreError::Invalid("data authority changed".into()))}
   control::reserve(c,&p.request_id,&p.context,&principal,&payload,&allocations)
  })).map_err(|_|unavailable())?.map_err(|_|unavailable())
 }
 /// Dispatch always uses the durable original UUID, including initialization.
 /// The caller has committed the Core intent and holds the publication guard.
 pub(crate) fn dispatch(&self,request:Request,initialize:bool)->Result<Response,String>{
  let(context,n,_)=self.context(&self.origin.namespace,true)?;
  if context!=self.origin||request.namespace!=context.namespace||request.branch!=context.branch{return Err("data dispatch context changed".into())}
  let file=self.root.join(n.file_name);
  if !initialize{let m=std::fs::symlink_metadata(&file).map_err(|_|unavailable())?;if !m.is_file()||m.nlink()!=1||m.uid()!=unsafe{libc::geteuid()}||m.mode()&0o022!=0{return Err(unavailable())}}
  else if file.try_exists().map_err(|_|unavailable())?{return Err("partial initialization requires explicit recovery".into())}
  let verify=||->io::Result<()>{qualify_root(&self.root).map_err(io::Error::other)?;let(now,_,_)=self.context(&context.namespace,true).map_err(io::Error::other)?;if now!=context{return Err(io::Error::other("data dispatch authority changed"))}Ok(())};
  self.worker.exchange_until(&file,if initialize{if matches!(request.operation,Operation::FirstApply(_)){"prepare-initialize"}else{"initialize"}}else{"existing"},request,self.registry.clone(),&verify,self.deadline).map(|v|v.0).map_err(|_|"accepted native operation outcome Unknown".into())
 }
 /// Settlement is custody of an already admitted request, not a new dispatch.
 /// Losing the original caller does not erase its durable accepted capsule.
 /// The original authority/epoch and before-head are rechecked by the ledger.
 pub(crate) fn settle(&self,result:DataChangeResult)->Result<DataChangeResult,String>{
  let hub=self.state.hub.clone();
  self.state.db.call_result_blocking(move|s|{
   let request=result.request_id.clone();let committed=matches!(result.phase,DataPhase::Committed);let crew=result.context.crew_id.clone();
   let settled=s.with_tx(|c|control::settle(c,&request,result))?;
   if committed{hub.emit(s.conn(),&[collab::Change::DataChanged{crew_id:crew}]);}
   settled.result.ok_or_else(||crewd_store::StoreError::Invalid("settled data result missing".into()))
  }).map_err(|_|unavailable())?.map_err(|_|unavailable())
 }
 pub(crate) fn acknowledge(&self,p:&DataSubmitParams,payload:&str,original:&str,observed:&str)->Result<DataChangeResult,String>{
  if !self.access.is_human(){return Err("explicit Human recovery required".into())}
  let state=self.state.clone();let access=self.access.clone();let p=p.clone();let payload=payload.to_owned();let original=original.to_owned();let observed=observed.to_owned();let hub=state.hub.clone();
  self.state.db.call_result_blocking(move|s|{
   let record=s.with_tx(|c|{
    access.check(&state,c,&p.context.crew_id,true).map_err(|_|crewd_store::StoreError::Invalid("data recovery permission changed".into()))?;
    control::resolve_unknown(c,&p.request_id,&p.context,access.principal(),&payload,&original,&observed)
   })?;
   // Acknowledging observed state changes the recovery control plane, never
   // upgrades the original Unknown to a known committed content operation.
   hub.emit(s.conn(),&[collab::Change::DataChanged{crew_id:p.context.crew_id}]);
   record.result.ok_or_else(||crewd_store::StoreError::Invalid("recovery result missing".into()))
  }).map_err(|_|unavailable())?.map_err(|_|unavailable())
 }
}
