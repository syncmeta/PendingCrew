//! The controller serializes native publication with all same-Core read cuts.
//! Installation requires an activated Core16 authority and a qualified sibling;
//! construction itself never initializes a namespace or loads platform secrets.
use super::{read_cut::{Access,ReadCut},worker::SiblingWorker};
use crate::{AppState,public_commands::Actor};
use crewd_data_protocol::{Operation,ENGINE_VERSION};
use crewd_exec::registry::ProcessRegistry;
use crewd_proto::{team_data::*,RpcError,Request,Response};
use crewd_store::team_data as control;
use serde::{Serialize,Deserialize};
use serde_json::Value;
use std::{path::PathBuf,sync::Arc};

pub(crate) struct Runtime {
 root:PathBuf,worker:SiblingWorker,registry:Arc<ProcessRegistry>,
 publication:Arc<tokio::sync::Mutex<()>>,
}
fn unavailable()->RpcError{RpcError::new(RpcError::FORBIDDEN,"data source unavailable or authority changed")}
fn encode(v:impl Serialize)->Result<Value,RpcError>{serde_json::to_value(v).map_err(|_|RpcError::new(RpcError::INTERNAL,"data result encoding failed"))}
fn parse<T:serde::de::DeserializeOwned>(v:Value)->Result<T,RpcError>{crewd_proto::parse_params(v)}
fn digest(v:&impl Serialize)->Result<String,String>{let v=serde_json::to_vec(v).map_err(|_|"data cursor encoding failed")?;Ok(ring::digest::digest(&ring::digest::SHA256,&v).as_ref().iter().map(|b|format!("{b:02x}")).collect())}
#[derive(Serialize,Deserialize)]#[serde(deny_unknown_fields)]struct QueryCursor{version:u32,namespace:String,commit:String,statement_sha256:String,offset:u64}
impl Runtime {
 pub(crate) fn new(root:PathBuf,worker:SiblingWorker,registry:Arc<ProcessRegistry>)->std::io::Result<Self>{worker.still_qualified()?;Ok(Self{root,worker,registry,publication:Arc::new(tokio::sync::Mutex::new(()))})}
 pub(crate) fn qualified(&self)->bool{self.worker.still_qualified().is_ok()}
 pub(crate) async fn handle(self:&Arc<Self>,state:Arc<AppState>,actor:Actor,request:Request)->Response{
  let id=request.id;
  let result=self.route(state,actor,request).await;
  match result{Ok(value)=>Response::ok(id,value),Err(error)=>Response::err(id,error)}
 }
 async fn route(self:&Arc<Self>,state:Arc<AppState>,actor:Actor,request:Request)->Result<Value,RpcError>{
  let access=Access::human(&state,actor).await?;
  self.route_access(state,access,request).await
 }
 pub(crate) async fn session(self:&Arc<Self>,state:Arc<AppState>,session:String,token:String,scope:crate::connection_scope::ConnectionScope,method:&str,args:Value)->Result<Value,RpcError>{
  if !crewd_proto::team_data_api::METHODS.contains(&method){return Err(unavailable())}
  let access=Access::attached_session(session,token,scope);
  self.route_access(state,access,Request{id:0,method:method.into(),params:args}).await
 }
 async fn route_access(self:&Arc<Self>,state:Arc<AppState>,access:Access,request:Request)->Result<Value,RpcError>{
  // Acquiring the publication guard is bounded, and moving it into the actual
  // blocking job means cancellation of the caller cannot release it early.
  let guard=tokio::time::timeout(std::time::Duration::from_secs(60),self.publication.clone().lock_owned()).await.map_err(|_|unavailable())?;
  let this=self.clone();
  tokio::task::spawn_blocking(move||{
   let _guard=guard;
   let method=request.method.as_str();
   if method=="data.workspace.get" {
    let p:DataWorkspaceGetParams=parse(request.params)?;
    let state_for=state.clone();let access_for=access.clone();let crew=p.crew_id.clone();
    let context=state.db.call_result_blocking(move|s|->crewd_collab::Result<DataContext>{
     access_for.check(&state_for,s.conn(),&crew,false)?;
     let format:i64=s.conn().query_row("PRAGMA user_version",[],|r|r.get(0))?;
     if format!=16{return Err(crewd_store::StoreError::Invalid("Core16 authority inactive".into()).into())}
     Ok(control::context(s.conn(),&crew)?)
    }).map_err(|_|unavailable())?.map_err(|_|unavailable())?;
    let cut=ReadCut::new(state,access,context.clone(),this.root.clone(),this.worker.clone(),this.registry.clone()).map_err(|_|unavailable())?;
    return encode(workspace(&cut,&context).map_err(|_|unavailable())?);
   }
   // All remaining methods carry their original origin. Parsing this envelope
   // establishes no access; ReadCut reloads the Core authority and actual ACL.
   let context:DataContext=serde_json::from_value(request.params.get("context").cloned().ok_or_else(unavailable)?).map_err(|_|unavailable())?;
   // A historical lookup reauthorizes the original crew in the CURRENT Core
   // epoch, then compares the stored capsule to the original request context.
   // It never uses an old epoch as an access credential or as a fresh submit.
   let read_origin=if matches!(method,"data.request.get"|"data.submit") {
    let state_for=state.clone();let access_for=access.clone();let original=context.clone();
    state.db.call_result_blocking(move|s|->crewd_collab::Result<DataContext>{
     access_for.check(&state_for,s.conn(),&original.crew_id,false)?;
     let current=control::context(s.conn(),&original.crew_id)?;
     if current.db_id!=original.db_id||current.namespace!=original.namespace{return Err(crewd_store::StoreError::Invalid("request namespace unavailable".into()).into())}Ok(current)
    }).map_err(|_|unavailable())?.map_err(|_|unavailable())?
   }else{context.clone()};
   let cut=ReadCut::new(state,access.clone(),read_origin,this.root.clone(),this.worker.clone(),this.registry.clone()).map_err(|_|unavailable())?;
   match method {
    "data.submit"=>{let p:DataSubmitParams=parse(request.params)?;encode(submit(&cut,&access,p).map_err(|_|unavailable())?)},
    "data.object.get"=>{let p:DataObjectGetParams=parse(request.params)?;encode(cut.detail(&p).map_err(|_|unavailable())?)},
    "data.object.list"=>{let p:DataObjectListParams=parse(request.params)?;encode(objects(&cut,p).map_err(|_|unavailable())?)},
    "data.query"=>{let p:DataQueryParams=parse(request.params)?;encode(query(&cut,p).map_err(|_|unavailable())?)},
    "data.schema.get"=>{let p:DataSchemaParams=parse(request.params)?;encode(schema(&cut,p).map_err(|_|unavailable())?)},
    "data.table.get"=>{let p:DataTableParams=parse(request.params)?;encode(table(&cut,p).map_err(|_|unavailable())?)},
    "data.branches"=>{let p:DataBranchesParams=parse(request.params)?;let value=cut.call(&p.context.namespace,None,Operation::Branches{after:p.after,limit:p.limit},false).map_err(|_|unavailable())?;encode(DataBranches{context:p.context,items:serde_json::from_value(value["items"].clone()).map_err(|_|unavailable())?,next:serde_json::from_value(value["next"].clone()).map_err(|_|unavailable())?})},
    "data.history"=>{let p:DataHistoryParams=parse(request.params)?;let value=cut.call(&p.context.namespace,None,Operation::History{after:p.after,limit:p.limit},false).map_err(|_|unavailable())?;encode(DataHistory{context:p.context,items:serde_json::from_value(value["items"].clone()).map_err(|_|unavailable())?,next:serde_json::from_value(value["next"].clone()).map_err(|_|unavailable())?})},
    "data.request.get"=>{let p:DataRequestGetParams=parse(request.params)?;encode(lookup(&cut,&access,p).map_err(|_|unavailable())?)},
    _=>Err(RpcError::new(RpcError::METHOD_NOT_FOUND,"data method unavailable")),
   }
  }).await.map_err(|_|unavailable())?
 }
}
fn workspace(cut:&ReadCut,origin:&DataContext)->Result<DataWorkspaceInfo,String>{
 let(context,_,_)=cut.context(&origin.namespace,false)?;
 let(state,reason)=if context.head.is_none(){("uninitialized",None)}else{
  match cut.call(&context.namespace,None,Operation::Inspect,false){
   Ok(value)if value.get("head").and_then(Value::as_str)==context.head.as_deref()&&value.get("branch").and_then(Value::as_str)==Some(context.branch.as_str())=>("ready",None),
   _=>("unavailable",Some("native state requires explicit inspection or recovery".into())),
  }
 };
 Ok(DataWorkspaceInfo{context,state:state.into(),engine_version:ENGINE_VERSION.into(),reason})
}
fn lookup(cut:&ReadCut,access:&Access,p:DataRequestGetParams)->Result<DataRequestGetResult,String>{
 let current=cut.context(&p.context.namespace,false)?.0;
 let expected=p.context.clone();let principal=access.principal().to_owned();
 let old=cut.db(move|c|->crewd_collab::Result<Option<control::Intent>>{
  let old=control::lookup(c,&p.request_id)?;
  if old.as_ref().is_some_and(|r|r.principal!=principal||r.context!=expected){return Err(crewd_store::StoreError::Invalid("request origin or principal differs".into()).into())}Ok(old)
 })?;
 let result=match old{
  None=>DataRequestLookup::NotFound,
  Some(intent)=>{
   let result=intent.result.unwrap_or_else(||DataChangeResult{request_id:intent.request_id,phase:DataPhase::Unknown,context:intent.context.clone(),before_head:intent.context.head.clone(),after_head:None,commit:None,created:intent.created,changed:vec![],conflicts:vec![],reason:Some("original request outcome remains Unknown; no replay".into()),resolved_by_request:None});
   if intent.context.authority_id!=current.authority_id||intent.context.restore_epoch!=current.restore_epoch{DataRequestLookup::Historical(result)}else{DataRequestLookup::Known(result)}
  }
 };
 Ok(DataRequestGetResult{workspace:workspace(cut,&current)?,lookup:result})
}
fn objects(cut:&ReadCut,p:DataObjectListParams)->Result<DataObjectPage,String>{
 if p.limit==0||p.limit>100{return Err("bounded object page required".into())}
 let current=cut.context(&p.context.namespace,false)?.0;let commit=p.at_commit.clone().or(current.head).ok_or("namespace uninitialized")?;
 let page=cut.call(&p.context.namespace,None,Operation::Objects{kind:p.object_type,after:p.after,limit:p.limit,at_commit:Some(commit.clone())},false)?;
 let items=page.get("items").and_then(Value::as_array).ok_or("invalid native object page")?;
 let mut output=Vec::new();
 for value in items{
  #[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Stored{object:DataObject,captures:Vec<crewd_data_protocol::Capture>}
  let stored:Stored=serde_json::from_value(value.clone()).map_err(|_|"invalid protected object")?;
  let archived=match &stored.object{DataObject::Block{archived,..}|DataObject::View{archived,..}|DataObject::Relation{archived,..}=>*archived};
  if archived&&!p.include_archived{continue}
  let(object,_)=super::projection::project(stored.object,&stored.captures,&p.context.namespace,&commit,p.at_commit.is_some(),cut)?;output.push(object);
 }
 Ok(DataObjectPage{context:p.context,commit,items:output,next:page.get("next").and_then(Value::as_str).map(str::to_owned)})
}
fn query(cut:&ReadCut,p:DataQueryParams)->Result<DataRows,String>{
 if p.limit==0||p.limit>1000{return Err("bounded query page required".into())}
 let statement_sha256=digest(&p.statement)?;
 let (commit,offset)=if let Some(cursor)=p.cursor{
  if cursor.len()>2048{return Err("bounded query cursor required".into())}
  let cursor:QueryCursor=serde_json::from_str(&cursor).map_err(|_|"invalid query cursor")?;
  if cursor.version!=1||cursor.namespace!=p.context.namespace||cursor.statement_sha256!=statement_sha256||cursor.offset>1_000_000||p.at_commit.as_ref().is_some_and(|c|c!=&cursor.commit){return Err("query cursor origin changed".into())}(cursor.commit,cursor.offset)
 }else{(p.at_commit.or(cut.context(&p.context.namespace,false)?.0.head).ok_or("namespace uninitialized")?,0)};
 let rows=cut.call(&p.context.namespace,None,Operation::ReadPage{at_commit:commit.clone(),sql:p.statement.sql,params:p.statement.params,offset,limit:p.limit},false)?;
 let columns=serde_json::from_value(rows.get("columns").cloned().ok_or("missing columns")?).map_err(|_|"invalid columns")?;
 let values:Vec<Vec<DataCell>>=serde_json::from_value(rows.get("rows").cloned().ok_or("missing rows")?).map_err(|_|"invalid typed cells")?;
 let truncated=rows.get("truncated").and_then(Value::as_bool).ok_or("missing truncation")?;
 let next=if truncated{Some(serde_json::to_string(&QueryCursor{version:1,namespace:p.context.namespace.clone(),commit:commit.clone(),statement_sha256,offset:offset.checked_add(values.len() as u64).ok_or("cursor overflow")?}).map_err(|_|"invalid next cursor")?)}else{None};
 // Arbitrary SQL has no promised total order. The exact snapshot is retained,
 // but clients must explicitly ORDER BY a unique key to rely on stable order.
 Ok(DataRows{columns,rows:values,truncated,next,commit,stable_order:false})
}

fn recorded(intent:control::Intent)->DataChangeResult{
 intent.result.unwrap_or_else(||DataChangeResult{request_id:intent.request_id,phase:DataPhase::Unknown,context:intent.context.clone(),before_head:intent.context.head,after_head:None,commit:None,created:intent.created,changed:vec![],conflicts:vec![],reason:Some("original outcome remains Unknown; no redispatch".into()),resolved_by_request:None})
}
fn submit(cut:&ReadCut,access:&Access,p:DataSubmitParams)->Result<DataChangeResult,String>{
 let payload=digest(&p)?;
 // Read an existing immutable capsule BEFORE comparing today's branch/HEAD or
 // validating edits against current objects. A retry never compiles/reexecutes.
 let principal=access.principal().to_owned();let original=p.clone();
 if let Some(old)=cut.db(move|c|->crewd_collab::Result<Option<control::Intent>>{
  let old=control::lookup(c,&original.request_id)?;
  if old.as_ref().is_some_and(|i|i.context!=original.context||i.principal!=principal){return Err(crewd_store::StoreError::Invalid("request immutable origin conflict".into()).into())}Ok(old)
 })?{
  if old.payload_sha256!=payload{return Err("request immutable payload conflict".into())}
  let current=cut.context(&p.context.namespace,false)?.0;let historical=old.context.restore_epoch!=current.restore_epoch||old.context.authority_id!=current.authority_id;let mut result=recorded(old);if historical{result.phase=DataPhase::Historical;}return Ok(result);
 }
 let author=cut.authorize_write()?;
 if let DataChange::ResolveUnknown{original_request_id,observed_head,acknowledge_observed_state}= &p.change{
  if !*acknowledge_observed_state||!access.is_human(){return Err("explicit Human acknowledgement required".into())}
  let observed=cut.call(&p.context.namespace,None,Operation::Inspect,false)?;
  if observed.get("head").and_then(Value::as_str)!=Some(observed_head.as_str())||observed.get("branch").and_then(Value::as_str)!=Some(p.context.branch.as_str()){return Err("observed native head/branch changed".into())}
  return cut.acknowledge(&p,&payload,original_request_id,observed_head);
 }
 if !access.is_human()&&!matches!(p.change,DataChange::Apply{..}){return Err("data version control requires explicit Human authority".into())}
 let allocations=match &p.change{DataChange::Apply{edits,..}=>super::compile::allocations(&p.context,edits)?,_=>vec![]};
 let intent=match cut.reserve(&p,&payload,allocations)?{control::Reservation::Existing(i)=>return Ok(recorded(i)),control::Reservation::New(i)=>i};
 let base=||DataChangeResult{request_id:p.request_id.clone(),phase:DataPhase::Unknown,context:p.context.clone(),before_head:p.context.head.clone(),after_head:None,commit:None,created:intent.created.clone(),changed:vec![],conflicts:vec![],reason:None,resolved_by_request:None};
 let mut changed=vec![];
 let operation:Result<Operation,String>=(||{
  let head=||p.context.head.clone().ok_or_else(||"namespace uninitialized".to_owned());
  Ok(match &p.change{
   DataChange::Initialize=>{if p.context.head.is_some(){return Err("namespace already initialized".into())}Operation::Inspect},
   DataChange::Apply{message,edits}=>{
    if message.len()>2048{return Err("bounded change message exceeded".into())}
    let mutations=super::compile::compile(&p.context,&intent.created,&author,&crewd_store::ts_from_ms(crewd_store::now_ms()),edits,cut)?;
    for mutation in &mutations{if let crewd_data_protocol::Mutation::PutObject{object,..}=mutation{
     let(kind,id,revision)=match object{DataObject::Block{id,revision,..}=>("block",id,revision),DataObject::View{id,revision,..}=>("view",id,revision),DataObject::Relation{id,revision,..}=>("relation",id,revision)};
     changed.push(DataObjectVersion{reference:DataRef{namespace:p.context.namespace.clone(),kind:kind.into(),id:id.clone()},revision:revision.clone()});
    }}
    let intent=crewd_data_protocol::WriteIntent{request_id:p.request_id.clone(),expected_head:p.context.head.clone().unwrap_or_default(),author:author.clone(),message:message.clone(),mutations};
    if p.context.head.is_none(){Operation::FirstApply(intent)}else{Operation::Apply(intent)}
   },
   DataChange::CreateBranch{name}=>{if name.is_empty()||name.len()>256{return Err("invalid bounded branch name".into())}Operation::Branch{request_id:p.request_id.clone(),name:name.clone(),expected_head:head()?}},
   DataChange::SwitchBranch{name}=>{if name.is_empty()||name.len()>256{return Err("invalid bounded branch name".into())}Operation::Checkout{request_id:p.request_id.clone(),branch:name.clone(),expected_head:head()?}},
   DataChange::MergeBranch{name,expected_source_head,message,resolutions}=>Operation::Merge{request_id:p.request_id.clone(),branch:name.clone(),expected_head:head()?,expected_source_head:expected_source_head.clone(),author:author.clone(),message:message.clone(),resolutions:resolutions.clone()},
   DataChange::RestoreVersion{target_commit}=>Operation::Restore{request_id:p.request_id.clone(),target_commit:target_commit.clone(),expected_head:head()?},
   DataChange::ResolveUnknown{..}=>return Err("recovery cannot dispatch native content".into()),
  })
 })();
 let operation=match operation{Ok(op)=>op,Err(_)=>{let mut result=base();result.phase=DataPhase::Rejected;result.reason=Some("change validation rejected before native dispatch".into());return cut.settle(result)}};
 let native=crewd_data_protocol::Request{id:p.request_id.clone(),namespace:p.context.namespace.clone(),branch:p.context.branch.clone(),operation};
 if serde_json::to_vec(&native).map_err(|_|"native encoding failed")?.len()>=crewd_data_protocol::MAX_FRAME_BYTES{
  let mut result=base();result.phase=DataPhase::Rejected;result.reason=Some("normalized batch exceeds private frame bound before dispatch".into());return cut.settle(result)
 }
 let response=cut.dispatch(native,p.context.head.is_none());
 let mut result=base();
 match response{
  Ok(crewd_data_protocol::Response::Ok(value))=>{
   if value.get("status").and_then(Value::as_str)==Some("conflicted"){
    result.phase=DataPhase::Conflicted;result.reason=Some("merge conflicts require explicit resolution".into());
   }else{
    let head=value.get("commit").or_else(||value.get("head")).and_then(Value::as_str).map(str::to_owned);
    let branch=value.get("branch").or_else(||value.get("result").and_then(|r|r.get("branch"))).and_then(Value::as_str).unwrap_or(&p.context.branch).to_owned();
    if let Some(head)=head.filter(|h|(20..=64).contains(&h.len())&&h.bytes().all(|b|b.is_ascii_alphanumeric())){
     result.phase=DataPhase::Committed;result.context.head=Some(head.clone());result.context.branch=branch;result.after_head=Some(head);result.commit=value.get("commit").and_then(Value::as_str).map(str::to_owned);result.changed=changed;
    }else{result.reason=Some("native publication lacks an exact version witness".into());}
   }
  },
  Ok(crewd_data_protocol::Response::Refused{..}) if p.context.head.is_some()=>{result.phase=DataPhase::Rejected;result.reason=Some("native operation refused without publication".into());},
  _=>{result.reason=Some("accepted native operation outcome Unknown; original UUID retained".into());},
 }
 cut.settle(result)
}

fn schema(cut:&ReadCut,p:DataSchemaParams)->Result<DataSchemaResult,String>{
 let commit=p.at_commit.or(cut.context(&p.context.namespace,false)?.0.head).ok_or("namespace uninitialized")?;
 let value=cut.call(&p.context.namespace,None,Operation::Catalog{at_commit:commit.clone(),after:p.after,limit:p.limit},false)?;
 Ok(DataSchemaResult{context:p.context,commit,objects:serde_json::from_value(value["objects"].clone()).map_err(|_|"invalid schema result")?,next:serde_json::from_value(value["next"].clone()).map_err(|_|"invalid schema cursor")?})
}
fn table(cut:&ReadCut,p:DataTableParams)->Result<DataTablePage,String>{
 let commit=p.at_commit.or(cut.context(&p.context.namespace,false)?.0.head).ok_or("namespace uninitialized")?;
 let value=cut.call(&p.context.namespace,None,Operation::Table{at_commit:commit.clone(),name:p.table,after_key:p.after_key,limit:p.limit},false)?;
 Ok(DataTablePage{context:p.context,commit,columns:serde_json::from_value(value["columns"].clone()).map_err(|_|"invalid columns")?,rows:serde_json::from_value(value["rows"].clone()).map_err(|_|"invalid rows")?,next_key:serde_json::from_value(value["nextKey"].clone()).map_err(|_|"invalid key cursor")?,truncated:value["truncated"].as_bool().ok_or("invalid truncation")?,stable_order:value["stableOrder"].as_bool().ok_or("invalid ordering")?})
}
