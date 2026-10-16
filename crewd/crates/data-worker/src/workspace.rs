//! Engine-side normalized operations. Core owns admission, durable intent, aliases and ACL.
use crate::engine::{Engine,Policy};
use crewd_data_protocol::*;
use crewd_proto::team_data::{DataRefState,DataViewDefinition,DataBindingSource};
use serde_json::{Value,json};
use std::{collections::BTreeSet,path::{Path,PathBuf}};
use uuid::Uuid;
const SCHEMA:&[&str]=&[
 "CREATE TABLE _pc_meta(name TEXT PRIMARY KEY,value TEXT NOT NULL)",
 "CREATE TABLE _pc_objects(kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(kind,id))",
 "CREATE TABLE _pc_requests(id TEXT PRIMARY KEY,payload_sha TEXT NOT NULL,result TEXT NOT NULL)",
 "CREATE TABLE _pc_import_documents(id TEXT PRIMARY KEY,source_sha TEXT NOT NULL,complete INTEGER NOT NULL DEFAULT 0)",
 "CREATE TABLE _pc_legacy(document_id TEXT NOT NULL,kind TEXT NOT NULL,row_key TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(document_id,kind,row_key))",
];
fn text(s:impl Into<String>)->Cell{Cell::Text(s.into())}
fn id(v:&str)->Result<(),String>{let x=Uuid::parse_str(v).map_err(|_|"stable entity ID must be UUID")?;if x.to_string()!=v{Err("noncanonical UUID".into())}else{Ok(())}}
fn revision(v:&str)->Result<i64,String>{let n=v.parse::<i64>().map_err(|_|"invalid revision")?;if n<=0||n.to_string()!=v{Err("noncanonical revision".into())}else{Ok(n)}}
fn identifier(v:&str)->Result<String,String>{if v.is_empty()||v.len()>512||v.contains('\0'){Err("invalid bounded identifier".into())}else{Ok(format!("\"{}\"",v.replace('"',"\"\"")))}}
fn hash(v:&impl serde::Serialize)->Result<String,String>{let bytes=serde_json::to_vec(v).map_err(|e|e.to_string())?;Ok(ring::digest::digest(&ring::digest::SHA256,&bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect())}
fn conflict(msg:&str)->String{format!("conflict: {msg}")}
fn object_fields(o:&DataObject)->(&'static str,&str,&str){match o{DataObject::Block{id,revision,..}=>("block",id,revision),DataObject::View{id,revision,..}=>("view",id,revision),DataObject::Relation{id,revision,..}=>("relation",id,revision)}}
fn visible(r:&DataRefState)->Result<(),String>{match r{DataRefState::Visible{reference}=>{id(&reference.namespace)?;if reference.kind.is_empty()||reference.kind.len()>128||reference.id.is_empty()||reference.id.len()>256{return Err("invalid typed reference".into())}if ["block","view","relation"].contains(&reference.kind.as_str()){id(&reference.id)?;}Ok(())},DataRefState::Unavailable=>Err("redaction placeholders cannot become stored references".into())}}
fn validate_object(o:&DataObject)->Result<(),String>{
 let (_,key,rev)=object_fields(o);id(key)?;revision(rev)?;
 let provenance=match o{
  DataObject::Block{content_type,content,provenance,..}=>{if content_type.is_empty()||content_type.len()>256||serde_json::to_vec(content).map_err(|e|e.to_string())?.len()>MAX_LOGICAL_BYTES{return Err("bounded semantic block exceeded".into())}provenance},
  DataObject::View{title,definition,provenance,..}=>{if title.len()>512{return Err("bounded view title exceeded".into())}match definition{
   DataViewDefinition::Document{placements,..}=>{if placements.len()>4096{return Err("placement count exceeded".into())}let mut ids=BTreeSet::new();for p in placements{id(&p.id)?;visible(&p.block)?;if !ids.insert(&p.id){return Err("duplicate placement ID".into())}}},
   DataViewDefinition::Web{html,css,javascript,bindings,..}=>{if html.len()+css.len()+javascript.len()>MAX_LOGICAL_BYTES||bindings.len()>4096{return Err("bounded web bundle exceeded".into())}let mut ids=BTreeSet::new();for b in bindings{if b.id.is_empty()||b.id.len()>128||!ids.insert(&b.id){return Err("duplicate or invalid view-local binding name".into())}if let DataBindingSource::Object{reference,..}=&b.source{visible(reference)?}}},
  }provenance},
  DataObject::Relation{subject,predicate,object,provenance,..}=>{visible(subject)?;visible(object)?;if predicate.is_empty()||predicate.len()>256{return Err("bounded relation predicate exceeded".into())}provenance}
 };
 for source in &provenance.sources{if let Some(reference)=&source.reference{visible(reference)?}}
 Ok(())
}
pub struct Workspace{pub engine:Engine,path:PathBuf,namespace:String,pending_initialize:Option<(String,std::fs::File)>}
impl Workspace{
 pub fn open_existing(path:&Path,namespace:&str,readonly:bool)->Result<Self,String>{
  id(namespace)?;let meta=std::fs::symlink_metadata(path).map_err(|_|"existing namespace unavailable")?;if !meta.is_file()||meta.file_type().is_symlink(){return Err("existing regular namespace required".into())}
  let engine=Engine::open(path,true)?;Self::verify(&engine,namespace)?;
  if readonly{return Ok(Self{engine,path:path.to_owned(),namespace:namespace.into(),pending_initialize:None})}drop(engine);
  let engine=Engine::open(path,false)?;Self::verify(&engine,namespace)?;Ok(Self{engine,path:path.to_owned(),namespace:namespace.into(),pending_initialize:None})
 }
 pub fn open(path:&Path,namespace:&str)->Result<Self,String>{
  if path.try_exists().map_err(|e|e.to_string())?{return Self::open_existing(path,namespace,false)}Self::initialize(path,namespace,None,false,false)
 }
 /// Explicit first-write action only. UUID fact and format/content namespace
 /// share the real final Dolt SQL+version commit. A partial file is retained on
 /// failure; its existence must become Unknown, never automatic initialization.
 pub fn open_for_request(path:&Path,namespace:&str,request:&str)->Result<Self,String>{id(request)?;Self::initialize(path,namespace,Some(request),false,false)}
 pub fn prepare_for_request(path:&Path,namespace:&str,request:&str)->Result<Self,String>{id(request)?;Self::initialize(path,namespace,Some(request),true,false)}
 pub fn open_for_import(path:&Path,namespace:&str,request:&str)->Result<Self,String>{id(request)?;Self::initialize(path,namespace,Some(request),false,true)}
 fn initialize(path:&Path,namespace:&str,request:Option<&str>,deferred:bool,import:bool)->Result<Self,String>{
  use std::os::unix::fs::{MetadataExt,OpenOptionsExt};
  id(namespace)?;let reservation=std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(path).map_err(|_|"fresh namespace reservation refused")?;
  let inode=reservation.metadata().map_err(|_|"namespace reservation metadata unavailable")?;
  let check=||->Result<(),String>{let m=std::fs::symlink_metadata(path).map_err(|_|"namespace reservation disappeared")?;if !m.is_file()||m.nlink()!=1||m.dev()!=inode.dev()||m.ino()!=inode.ino(){return Err("namespace reservation replaced".into())}Ok(())};
  check()?;let engine=Engine::open(path,false)?;check()?;engine.execute("BEGIN IMMEDIATE",&[])?;
  let result:Result<(),String>=(||{
   for sql in SCHEMA{engine.execute(sql,&[])?;}engine.execute("INSERT INTO _pc_meta VALUES('namespace',?)",&[text(namespace)])?;engine.execute("INSERT INTO _pc_meta VALUES('format','1')",&[])?;
   if import{engine.execute("INSERT INTO _pc_meta VALUES('importMode','open')",&[])?;}
   if deferred{return Ok(())}
   let message=if let Some(request)=request{let payload=hash(&json!({"operation":"initialize","namespace":namespace,"requestId":request}))?;let result=json!({"namespace":namespace,"branch":"main","initialized":true});engine.execute("INSERT INTO _pc_requests VALUES(?,?,?)",&[text(request),text(&payload),text(serde_json::to_string(&result).map_err(|e|e.to_string())?)])?;format!("pc:{request}:{payload}\nPendingCrew knowledge format1 initialization")}else{"PendingCrew knowledge format1 initialization".into()};
   check()?;engine.scalar("SELECT dolt_commit('-A','-m',?,'--author','PendingCrew <local@pendingcrew.invalid>')",&[text(message)])?;check()?;reservation.sync_all().map_err(|_|"namespace reservation sync failed")?;
   let parent=path.parent().ok_or("namespace parent absent")?;std::fs::File::open(parent).and_then(|p|p.sync_all()).map_err(|_|"namespace parent sync failed")?;Ok(())
  })();
  if result.is_err()&&!engine.autocommit(){let _=engine.execute("ROLLBACK",&[]);}result?;Ok(Self{engine,path:path.to_owned(),namespace:namespace.into(),pending_initialize:if deferred{Some((request.ok_or("initial UUID missing")?.into(),reservation))}else{None}})
 }
 fn verify(engine:&Engine,namespace:&str)->Result<(),String>{if engine.scalar("SELECT value FROM _pc_meta WHERE name='namespace'",&[])?.as_str()!=Some(namespace)||engine.scalar("SELECT value FROM _pc_meta WHERE name='format'",&[])?.as_str()!=Some("1"){return Err("unknown format or namespace; refuse before writable open".into())}Ok(())}
 pub fn head(&self)->Result<String,String>{self.engine.scalar("SELECT dolt_hashof('HEAD')",&[])?.as_str().map(str::to_owned).ok_or("missing HEAD".into())}
 pub fn branch(&self)->Result<String,String>{self.engine.scalar("SELECT active_branch()",&[])?.as_str().map(str::to_owned).ok_or("missing branch".into())}
 fn context(&self,r:&Request)->Result<(),String>{if r.namespace!=self.namespace{return Err(conflict("namespace changed"))}if r.branch!=self.branch()?{self.engine.scalar("SELECT hash FROM dolt_branches WHERE name=?",&[text(&r.branch)])?;self.engine.scalar("SELECT dolt_connect_branch(?)",&[text(&r.branch)])?;}Ok(())}
 fn expected(&self,h:&str)->Result<(),String>{if self.head()?!=h{Err(conflict("HEAD changed"))}else{Ok(())}}
 fn clean(&self)->Result<(),String>{if self.engine.execute("SELECT * FROM dolt_status LIMIT 1",&[])?["rows"].as_array().is_none_or(|v|!v.is_empty()){Err("uncommitted working set; explicit recovery required".into())}else{Ok(())}}
 pub fn inspect(&self)->Result<Value,String>{Ok(json!({"namespace":self.namespace,"format":1,"engineVersion":ENGINE_VERSION,"engine":"prolly","head":self.head()?,"branch":self.branch()?,"workingSet":self.engine.execute("SELECT * FROM dolt_status",&[])?}))}
 pub fn lookup(&self,request:&str)->Result<Value,String>{
  id(request)?;let row=self.engine.execute("SELECT payload_sha,result FROM _pc_requests WHERE id=?",&[text(request)])?;
  let Some(row)=row["rows"].get(0)else{return Ok(json!({"status":"notFound","requestId":request}))};
  let commits=self.engine.execute("SELECT commit_hash FROM dolt_log WHERE message LIKE ? LIMIT 2",&[text(format!("pc:{request}:%"))])?;
  if commits["rows"].as_array().is_none_or(|v|v.len()!=1){return Ok(json!({"status":"unknown","requestId":request,"reason":"no unique version witness"}))}
  Ok(json!({"status":"known","requestId":request,"payloadSHA256":row[0],"commit":commits["rows"][0][0],"result":serde_json::from_str::<Value>(row[1].as_str().ok_or("invalid UUID witness")?).map_err(|_|"corrupt UUID witness")?}))
 }
 fn get_from(e:&Engine,kind:&str,key:&str)->Result<Option<Value>,String>{let r=e.execute("SELECT data FROM _pc_objects WHERE kind=? AND id=?",&[text(kind),text(key)])?;r["rows"].get(0).map(|r|serde_json::from_str(r[0].as_str().ok_or("corrupt object witness")?).map_err(|_|"corrupt object JSON".into())).transpose()}
 fn snapshot(&self,commit:&str)->Result<Engine,String>{if !(20..=64).contains(&commit.len())||!commit.bytes().all(|b|b.is_ascii_alphanumeric()){return Err("exact historical commit required".into())}Engine::open(&self.path.join(commit),true)}
 pub fn object(&self,kind:&str,key:&str,at:Option<&str>)->Result<Value,String>{id(key)?;if !["block","view","relation"].contains(&kind){return Err("invalid object kind".into())}let snapshot=at.map(|v|self.snapshot(v)).transpose()?;Ok(Self::get_from(snapshot.as_ref().unwrap_or(&self.engine),kind,key)?.unwrap_or(Value::Null))}
 pub fn objects(&self,kind:&str,after:Option<&str>,limit:u32,at:Option<&str>)->Result<Value,String>{
  if !["block","view","relation"].contains(&kind)||limit==0||limit>100{return Err("invalid object page".into())}let snapshot=at.map(|v|self.snapshot(v)).transpose()?;
  let r=snapshot.as_ref().unwrap_or(&self.engine).execute("SELECT id,data FROM _pc_objects WHERE kind=? AND id>? ORDER BY id LIMIT ?",&[text(kind),text(after.unwrap_or("")),Cell::Integer((limit+1).to_string())])?;
  let rows=r["rows"].as_array().ok_or("invalid object rows")?;let objects=rows.iter().take(limit as usize).map(|r|serde_json::from_str::<Value>(r[1].as_str().ok_or("invalid object")?).map_err(|_|"invalid object JSON".into())).collect::<Result<Vec<_>,String>>()?;
  let next=if rows.len()>limit as usize{rows.get(limit as usize-1).and_then(|r|r[0].as_str())}else{None};Ok(json!({"items":objects,"next":next}))
 }
 fn put(&self,object:&DataObject,expected:Option<&str>,captures:&[Capture])->Result<Value,String>{self.put_inner(object,expected,captures,false)}
 fn put_inner(&self,object:&DataObject,expected:Option<&str>,captures:&[Capture],import:bool)->Result<Value,String>{
  validate_object(object)?;let(kind,key,rev)=object_fields(object);let next=revision(rev)?;let old=Self::get_from(&self.engine,kind,key)?;
  match(old.as_ref(),expected){(None,None)if next==1||import=>{},(Some(old),Some(expected))if old["object"]["revision"].as_str()==Some(expected)&&revision(expected)?.checked_add(1)==Some(next)=>{},_=>return Err(conflict("object revision changed or identity reused"))}
  if let Some(old)=old{let old=&old["object"]["provenance"];let now=serde_json::to_value(object).map_err(|e|e.to_string())?;for key in ["createdBy","createdAt"]{if old[key]!=now["provenance"][key]{return Err("immutable creation provenance changed".into())}}}
  let mut anchors=BTreeSet::new();for c in captures{id(&c.namespace)?;id(&c.id)?;revision(&c.revision)?;if c.anchor.is_empty()||c.anchor.len()>256||!anchors.insert(&c.anchor)||!(c.commit=="self"||((20..=64).contains(&c.commit.len())&&c.commit.bytes().all(|b|b.is_ascii_alphanumeric()))){return Err("invalid protected history capture".into())}}
  let value=json!({"object":object,"captures":captures});let encoded=serde_json::to_string(&value).map_err(|e|e.to_string())?;if encoded.len()>MAX_CONTENT_BYTES{return Err("encoded object bound exceeded".into())}
  self.engine.execute("INSERT INTO _pc_objects VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,data=excluded.data",&[text(kind),text(key),Cell::Integer(next.to_string()),text(encoded)])?;
  Ok(json!({"reference":{"namespace":self.namespace,"kind":kind,"id":key},"revision":rev}))
 }
 fn remember(&self,request:&str,payload:&str,result:&Value)->Result<(),String>{self.engine.execute("INSERT INTO _pc_requests VALUES(?,?,?)",&[text(request),text(payload),text(serde_json::to_string(result).map_err(|e|e.to_string())?)])?;Ok(())}
 fn commit(&self,request:&str,payload:&str,message:&str,author:&ResourceActor)->Result<Value,String>{let author=serde_json::to_string(author).map_err(|e|e.to_string())?;self.engine.scalar("SELECT dolt_commit('-A','-m',?,'--author',?)",&[text(format!("pc:{request}:{payload}\n{message}")),text(format!("{} <actor@pendingcrew.invalid>",author.replace(['<','>','\n','\r'],"_")))])}
 pub fn apply(&self,intent:&WriteIntent)->Result<Value,String>{
  id(&intent.request_id)?;if intent.message.len()>2048||intent.mutations.is_empty()||intent.mutations.len()>64{return Err("invalid bounded batch".into())}let payload=hash(intent)?;let previous=self.lookup(&intent.request_id)?;
  if previous["status"]=="known"{if previous["payloadSHA256"]!=payload{return Err(conflict("UUID payload changed"))}return Ok(previous)}if previous["status"]=="unknown"{return Err("unknown UUID cannot replay".into())}
  self.expected(&intent.expected_head)?;self.clean()?;self.engine.execute("BEGIN IMMEDIATE",&[])?;
  let result=(||{let results=intent.mutations.iter().map(|m|match m{Mutation::PutObject{object,expected_revision,captures}=>self.put(object,expected_revision.as_deref(),captures),Mutation::Sql{sql,params}=>self.engine.query(sql,params,Policy::AgentWrite)}).collect::<Result<Vec<_>,_>>()?;
   let result=json!({"changes":results,"namespace":self.namespace,"branch":self.branch()?});self.remember(&intent.request_id,&payload,&result)?;let commit=self.commit(&intent.request_id,&payload,&intent.message,&intent.author)?;Ok(json!({"status":"known","requestId":intent.request_id,"payloadSHA256":payload,"commit":commit,"result":result}))})();
  if result.is_err()&&!self.engine.autocommit(){let _=self.engine.execute("ROLLBACK",&[]);}result
 }
 fn merge(&self,request:&str,branch:&str,source:&str,head:&str,message:&str,author:&ResourceActor,resolutions:&[DataConflictResolution])->Result<Value,String>{
  id(request)?;self.expected(head)?;self.clean()?;if self.engine.scalar("SELECT hash FROM dolt_branches WHERE name=?",&[text(branch)])?.as_str()!=Some(source){return Err(conflict("merge source changed"))}if message.len()>2048||resolutions.len()>100{return Err("bounded merge exceeded".into())}
  self.engine.execute("BEGIN IMMEDIATE",&[])?;let staged=self.engine.scalar("SELECT dolt_merge('--no-ff','--no-commit',?)",&[text(branch)]);
  let outcome=(||{if self.engine.autocommit()||self.head()?!=head{return Err("merge published before controlled final commit".into())}
   let conflicts=self.engine.execute("SELECT * FROM dolt_conflicts",&[])?;let schema=self.engine.execute("SELECT * FROM dolt_schema_conflicts",&[])?;
   if schema["rows"].as_array().is_none_or(|r|!r.is_empty()){return Ok(json!({"status":"conflicted","rows":conflicts,"schema":schema}))}
   if staged.is_err()&&conflicts["rows"].as_array().is_some_and(|r|r.is_empty()){return Err(staged.err().unwrap_or("merge staging failed".into()))}
   let mut names=BTreeSet::new();for r in resolutions{if r.table.starts_with("_pc_")||r.table.starts_with("dolt_")||!names.insert(&r.table)||!["ours","theirs"].contains(&r.choice.as_str()){return Err("invalid merge resolution".into())}self.engine.scalar("SELECT dolt_conflicts_resolve(?,?)",&[text(format!("--{}",r.choice)),text(&r.table)])?;}
   let remaining=self.engine.execute("SELECT * FROM dolt_conflicts",&[])?;if remaining["rows"].as_array().is_none_or(|r|!r.is_empty()){return Ok(json!({"status":"conflicted","rows":remaining,"schema":schema}))}
   let payload=hash(&(request,branch,source,head,message,author,resolutions))?;let result=json!({"mergedSource":source});self.remember(request,&payload,&result)?;let commit=self.commit(request,&payload,message,author)?;Ok(json!({"status":"known","requestId":request,"commit":commit,"result":result}))})();
  if !self.engine.autocommit(){let rollback=self.engine.execute("ROLLBACK",&[]);if rollback.is_err(){return Err("merge rollback unresolved".into())}}outcome
 }
 pub fn handle(&self,request:Request)->Response{
  // Pure lookup is independent of the current branch/HEAD, including after reset.
  if request.namespace!=self.namespace{return Response::Refused{code:"conflict".into(),message:"namespace changed".into()}}
  if let Operation::Lookup{request_id}=&request.operation{return match self.lookup(request_id){Ok(v)=>Response::Ok(v),Err(e)=>Response::Refused{code:"data.refused".into(),message:e}}}
  let original_head=self.head();let original_branch=self.branch();let request_id=match &request.operation{Operation::Apply(i)|Operation::FirstApply(i)=>Some(i.request_id.clone()),Operation::Import(i)=>Some(i.request_id.clone()),Operation::SealImport{request_id,..}=>Some(request_id.clone()),Operation::Snapshot{file_id}=>Some(file_id.clone()),Operation::Branch{request_id,..}|Operation::Checkout{request_id,..}|Operation::Merge{request_id,..}|Operation::Restore{request_id,..}=>Some(request_id.clone()),_=>None};let mut dispatched=false;let mut nontransactional=false;
  let result=(||{self.context(&request)?;match request.operation{
   Operation::Catalog{at_commit,after,limit}=>self.catalog(&at_commit,after.as_deref(),limit),
   Operation::Table{at_commit,name,after_key,limit}=>self.table(&at_commit,&name,after_key.as_deref(),limit),
   Operation::Branches{after,limit}=>self.branches(after.as_deref(),limit),
   Operation::History{after,limit}=>self.history(after.as_deref(),limit),
   Operation::Import(batch)=>{dispatched=true;self.import_batch(&batch)},
   Operation::ImportedRows{document_id,kind,after,limit}=>self.imported_rows(&document_id,&kind,after.as_deref(),limit),
   Operation::SealImport{request_id,expected_head,manifest_sha256}=>{dispatched=true;self.seal_import(&request_id,&expected_head,&manifest_sha256)},
   Operation::Snapshot{file_id}=>{dispatched=true;nontransactional=true;self.snapshot_file(&file_id)},
   Operation::Inspect=>self.inspect(),Operation::Lookup{..}=>unreachable!(),Operation::Object{kind,id,at_commit}=>self.object(&kind,&id,at_commit.as_deref()),Operation::Objects{kind,after,limit,at_commit}=>self.objects(&kind,after.as_deref(),limit,at_commit.as_deref()),
   Operation::Apply(intent)=>{dispatched=true;self.apply(&intent)},
   Operation::FirstApply(intent)=>{dispatched=true;nontransactional=true;self.first_apply(&intent)},Operation::ReadPage{at_commit,sql,params,offset,limit}=>{let e=self.snapshot(&at_commit)?;e.query_page(&sql,&params,offset,limit)},Operation::Read{at_commit,sql,params}=>{if let Some(at)=at_commit{let e=self.snapshot(&at)?;e.query(&sql,&params,Policy::AgentRead)}else{self.engine.query(&sql,&params,Policy::AgentRead)}},
   Operation::Log{limit}=>{if limit==0||limit>100{return Err("invalid history limit".into())}self.engine.execute("SELECT * FROM dolt_log LIMIT ?",&[Cell::Integer(limit.to_string())])},
   Operation::Diff{from,to,table,limit}=>{if table.starts_with("_pc_")||table.starts_with("dolt_")||limit==0||limit>100{return Err("invalid user diff".into())}let function=identifier(&format!("dolt_diff_{table}"))?;self.engine.execute(&format!("SELECT * FROM {function}(?,?) LIMIT ?"),&[text(from),text(to),Cell::Integer(limit.to_string())])},
   Operation::Branch{request_id,name,expected_head}=>{id(&request_id)?;self.expected(&expected_head)?;self.clean()?;dispatched=true;nontransactional=true;self.engine.scalar("SELECT dolt_branch(?)",&[text(&name)])?;Ok(json!({"branchCreated":name,"head":self.head()?,"branch":self.branch()?}))},
   Operation::Checkout{request_id,branch,expected_head}=>{id(&request_id)?;self.expected(&expected_head)?;self.clean()?;self.engine.scalar("SELECT hash FROM dolt_branches WHERE name=?",&[text(&branch)])?;dispatched=true;nontransactional=true;self.engine.scalar("SELECT dolt_connect_branch(?)",&[text(branch)])?;self.inspect()},
   Operation::Merge{request_id,branch,expected_head,expected_source_head,author,message,resolutions}=>{dispatched=true;self.merge(&request_id,&branch,&expected_source_head,&expected_head,&message,&author,&resolutions)},
   Operation::Restore{request_id,target_commit,expected_head}=>{id(&request_id)?;self.expected(&expected_head)?;self.clean()?;drop(self.snapshot(&target_commit)?);dispatched=true;nontransactional=true;let recovery=format!("pc-before-reset-{request_id}");self.engine.scalar("SELECT dolt_branch(?)",&[text(&recovery)])?;self.engine.scalar("SELECT dolt_reset('--hard',?)",&[text(&target_commit)])?;Self::verify(&self.engine,&self.namespace)?;Ok(json!({"head":self.head()?,"recoveryBranch":recovery}))},
  }})();
  match result{Ok(v)=>Response::Ok(v),Err(message)=>{let proven_unchanged=!dispatched||(!nontransactional&&self.engine.autocommit()&&original_head.is_ok()&&self.head()==original_head&&self.branch()==original_branch&&self.clean().is_ok());if proven_unchanged{Response::Refused{code:if message.starts_with("conflict:"){"conflict"}else{"data.refused"}.into(),message}}else{Response::Unknown{request_id,reason:message}}}}
 }
}

fn user_name(name:&str)->bool{
 let n=name.to_ascii_lowercase();!name.is_empty()&&name.len()<=512&&!name.contains('\0')&&!n.starts_with("_pc_")&&!n.starts_with("dolt_")&&!n.starts_with("sqlite_")&&!n.starts_with("pragma_")
}
impl Workspace {
 /// Metadata is selected by the controller, not arbitrary Agent SQL. Protected
 /// tables, system virtual tables and UUID/control envelopes never appear.
 pub fn catalog(&self,at:&str,after:Option<&str>,limit:u32)->Result<Value,String>{
  if limit==0||limit>100{return Err("bounded schema page required".into())}
  let db=self.snapshot(at)?;
  let result=db.execute("SELECT type,name,sql FROM sqlite_schema WHERE type IN ('table','view','index','trigger') AND substr(lower(name),1,4) != '_pc_' AND substr(lower(name),1,5) != 'dolt_' AND substr(lower(name),1,7) != 'sqlite_' AND substr(lower(name),1,7) != 'pragma_' AND name>? ORDER BY name LIMIT ?",&[text(after.unwrap_or("")),Cell::Integer((limit+1).to_string())])?;
  let rows=result["rows"].as_array().ok_or("invalid schema inventory")?;
  let mut objects=Vec::new();let mut next=None;
  for row in rows{
   let name=row[1].as_str().ok_or("invalid schema name")?;if !user_name(name){continue}
   if objects.len()==limit as usize{next=objects.last().and_then(|o:&Value|o.get("name")).and_then(Value::as_str).map(str::to_owned);break}
   let kind=row[0].as_str().ok_or("invalid schema kind")?;let mut columns=vec![];
   if matches!(kind,"table"|"view"){
    let values=db.execute(&format!("PRAGMA table_info({})",identifier(name)?),&[])?;
    if values["truncated"].as_bool()!=Some(false){return Err("schema column bound exceeded".into())}
    for column in values["rows"].as_array().ok_or("invalid schema columns")?{
     columns.push(json!({"name":column[1],"declaredType":column[2],"notNull":column[3].as_i64().ok_or("invalid nullability")?!=0,"primaryKeyPosition":column[5],"defaultSql":column[4],"attributes":{}}));
    }
   }
   objects.push(json!({"kind":kind,"name":name,"createSql":row[2].as_str().unwrap_or(""),"columns":columns,"attributes":{}}));
  }
  Ok(json!({"objects":objects,"next":next}))
 }
 pub fn table(&self,at:&str,name:&str,after:Option<&[Cell]>,limit:u32)->Result<Value,String>{
  if !user_name(name)||limit==0||limit>1000{return Err("bounded business table required".into())}
  let db=self.snapshot(at)?;let present=db.execute("SELECT type FROM sqlite_schema WHERE name=? AND type='table'",&[text(name)])?;
  if present["rows"].as_array().is_none_or(|r|r.len()!=1){return Err("business table unavailable".into())}
  let info=db.execute(&format!("PRAGMA table_info({})",identifier(name)?),&[])?;
  let columns=info["rows"].as_array().ok_or("invalid table columns")?;
  let mut keys=columns.iter().filter_map(|r|r[5].as_u64().filter(|n|*n>0).map(|n|(n,r[1].as_str().unwrap_or(""),r[0].as_u64().unwrap_or(u64::MAX) as usize))).collect::<Vec<_>>();keys.sort_by_key(|v|v.0);
  // Ordinary rowid tables may permit NULL in a declared composite PK. A NULL
  // key cannot define strict keyset continuation, so explicitly refuse such a
  // cursor instead of silently dropping remaining rows.
  if let Some(after)=after{if keys.is_empty()||after.len()!=keys.len()||after.iter().any(|c|matches!(c,Cell::Null)){return Err("stable nonnull key cursor required".into())}}
  let names=keys.iter().map(|(_,n,_)|identifier(n)).collect::<Result<Vec<_>,_>>()?;
  let mut sql=format!("SELECT * FROM {}",identifier(name)?);let mut params=vec![];
  if let Some(after)=after{sql.push_str(&format!(" WHERE ({}) > ({})",names.join(","),vec!["?";after.len()].join(",")));params.extend_from_slice(after);}
  if !names.is_empty(){sql.push_str(&format!(" ORDER BY {}",names.join(",")));}
  let mut rows=db.query_page(&sql,&params,0,limit)?;
  let values=rows["rows"].as_array().ok_or("invalid typed table rows")?;let truncated=rows["truncated"].as_bool().ok_or("invalid table truncation")?;
  let stable=!keys.is_empty()&&values.iter().all(|r|keys.iter().all(|(_,_,column)|r.get(*column).is_some_and(|c|c.get("type").and_then(Value::as_str)!=Some("null"))));
  let next=if stable&&truncated{values.last().map(|r|Value::Array(keys.iter().map(|(_,_,column)|r[*column].clone()).collect()))}else{None};
  rows["nextKey"]=next.unwrap_or(Value::Null);rows["stableOrder"]=Value::Bool(stable);Ok(rows)
 }
 pub fn branches(&self,after:Option<&str>,limit:u32)->Result<Value,String>{
  if limit==0||limit>100{return Err("bounded branches required".into())}let active=self.branch()?;
  let rows=self.engine.execute("SELECT name,hash FROM dolt_branches WHERE name>? ORDER BY name LIMIT ?",&[text(after.unwrap_or("")),Cell::Integer((limit+1).to_string())])?;
  let raw=rows["rows"].as_array().ok_or("invalid branch inventory")?;let items=raw.iter().take(limit as usize).map(|r|json!({"name":r[0],"head":r[1],"active":r[0].as_str()==Some(active.as_str())})).collect::<Vec<_>>();
  let next=if raw.len()>limit as usize{items.last().and_then(|r|r.get("name")).cloned()}else{None};Ok(json!({"items":items,"next":next}))
 }
 pub fn history(&self,after:Option<&str>,limit:u32)->Result<Value,String>{
  if limit==0||limit>100{return Err("bounded history required".into())}
  // The cursor pins the original history head and offset. New commits cannot
  // insert ahead of or silently change the pending history traversal.
  #[derive(serde::Serialize,serde::Deserialize)]#[serde(deny_unknown_fields)]struct Cursor{head:String,offset:u64}
  let(head,offset)=if let Some(after)=after{if after.len()>1024{return Err("history cursor bound".into())}let cursor:Cursor=serde_json::from_str(after).map_err(|_|"invalid history cursor")?;if cursor.offset>1_000_000{return Err("history cursor bound".into())}(cursor.head,cursor.offset)}else{(self.head()?,0)};
  let snapshot=self.snapshot(&head)?;
  let rows=snapshot.execute("SELECT commit_hash,committer,date,message FROM dolt_log LIMIT ? OFFSET ?",&[Cell::Integer((limit+1).to_string()),Cell::Integer(offset.to_string())])?;
  let raw=rows["rows"].as_array().ok_or("invalid history rows")?;
  let mut items=Vec::new();
  for row in raw.iter().take(limit as usize){
   let parents=snapshot.execute("SELECT parent_hash FROM dolt_commit_ancestors WHERE commit_hash=? ORDER BY parent_index",&[text(row[0].as_str().ok_or("invalid commit hash")?)])?;
   let parents=parents["rows"].as_array().ok_or("invalid parent graph")?.iter().filter_map(|r|r[0].as_str().map(str::to_owned)).collect::<Vec<_>>();
   items.push(json!({"hash":row[0],"parents":parents,"author":row[1],"timestamp":row[2],"message":row[3]}));
  }
  let next=if raw.len()>limit as usize{Some(serde_json::to_string(&Cursor{head,offset:offset+items.len() as u64}).map_err(|_|"history cursor encoding")?)}else{None};Ok(json!({"items":items,"next":next}))
 }
}

impl Workspace{
 /// A fresh file's format, typed content/business SQL and original UUID fact
 /// publish in ONE native version commit. No separate initialization commit can
 /// steal the Apply UUID or make a failed first batch appear committed.
 pub fn first_apply(&self,intent:&WriteIntent)->Result<Value,String>{
  use std::os::unix::fs::MetadataExt;
  let(request,reservation)=self.pending_initialize.as_ref().ok_or("first Apply requires an explicitly prepared fresh namespace")?;
  if request!=&intent.request_id||!intent.expected_head.is_empty()||self.engine.autocommit()||intent.mutations.is_empty()||intent.mutations.len()>64||intent.message.len()>2048{return Err("invalid original first Apply intent".into())}
  let original=reservation.metadata().map_err(|_|"fresh reservation unavailable")?;
  let check=||->Result<(),String>{let current=std::fs::symlink_metadata(&self.path).map_err(|_|"fresh namespace unavailable")?;if !current.is_file()||current.nlink()!=1||current.dev()!=original.dev()||current.ino()!=original.ino(){return Err("fresh namespace replaced".into())}Ok(())};
  let result=(||{
   check()?;let payload=hash(intent)?;
   let changes=intent.mutations.iter().map(|m|match m{Mutation::PutObject{object,expected_revision,captures}=>self.put(object,expected_revision.as_deref(),captures),Mutation::Sql{sql,params}=>self.engine.query(sql,params,Policy::AgentWrite)}).collect::<Result<Vec<_>,_>>()?;
   let result=json!({"changes":changes,"namespace":self.namespace,"branch":self.branch()?,"initialized":true});self.remember(&intent.request_id,&payload,&result)?;
   check()?;let commit=self.commit(&intent.request_id,&payload,&intent.message,&intent.author)?;check()?;reservation.sync_all().map_err(|_|"first publication sync failed")?;
   std::fs::File::open(self.path.parent().ok_or("native parent absent")?).and_then(|p|p.sync_all()).map_err(|_|"first publication parent sync failed")?;
   Ok(json!({"status":"known","requestId":intent.request_id,"payloadSHA256":payload,"commit":commit,"result":result}))
  })();
  if result.is_err()&&!self.engine.autocommit(){let _=self.engine.execute("ROLLBACK",&[]);}result
 }
}

#[path="legacy_import.rs"] mod legacy_import;
