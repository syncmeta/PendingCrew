//! Exact sibling worker transport. Package trust is established by the creator's
//! sealed artifact policy; these hashes bind that already-qualified package.
use crewd_data_protocol::{Request,Response,MAX_FRAME_BYTES,IPC_VERSION,ENGINE_VERSION};
use crewd_exec::{owned_stdio::OwnedStdio,registry::{ProcessRegistry,ProcessIdentity}};
use serde::{Serialize,Deserialize};
use std::{ffi::OsString,fs::{OpenOptions,Metadata},io::{self,Read},os::unix::fs::{MetadataExt,OpenOptionsExt},path::{Path,PathBuf},sync::Arc,time::{Duration,Instant}};
fn invalid(s:&str)->io::Error{io::Error::other(s)}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WorkerBinding{pub version:u32,pub protocol:u32,pub engine_version:String,pub source_ref:String,pub core_sha256:String,pub worker_sha256:String}
#[derive(Clone,Debug,PartialEq,Eq)]
struct Stamp{dev:u64,ino:u64,len:u64,mtime:i64,mtime_ns:i64,ctime:i64,ctime_ns:i64,mode:u32,uid:u32,nlink:u64}
fn stamp(m:&Metadata)->Stamp{Stamp{dev:m.dev(),ino:m.ino(),len:m.len(),mtime:m.mtime(),mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec(),mode:m.mode(),uid:m.uid(),nlink:m.nlink()}}
fn digest(s:&str)->bool{s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
fn capture(path:&Path)->io::Result<(String,Stamp)>{
 let file=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC|libc::O_NONBLOCK).open(path)?;let before=file.metadata()?;let euid=unsafe{libc::geteuid()};
 if !before.is_file()||before.nlink()!=1||before.len()>512*1024*1024||!matches!(before.uid(),0) && before.uid()!=euid||before.mode()&0o6022!=0||before.mode()&0o111==0{return Err(invalid("worker package executable qualification failed"))}
 let mut context=ring::digest::Context::new(&ring::digest::SHA256);let mut bytes=[0;65536];let mut file=file;loop{let n=file.read(&mut bytes)?;if n==0{break}context.update(&bytes[..n]);}
 if stamp(&before)!=stamp(&file.metadata()?)||stamp(&before)!=stamp(&std::fs::symlink_metadata(path)?){return Err(invalid("executable changed during hash"))}
 Ok((context.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect(),stamp(&before)))
}
#[derive(Clone)]
pub struct SiblingWorker{path:PathBuf,identity:Stamp,binding:WorkerBinding}
impl SiblingWorker{
 pub fn qualify(binding:WorkerBinding)->io::Result<Self>{
  if binding.version!=1||binding.protocol!=IPC_VERSION||binding.engine_version!=ENGINE_VERSION||binding.source_ref.len()!=40||!binding.source_ref.bytes().all(|b|b.is_ascii_hexdigit())||!digest(&binding.core_sha256)||!digest(&binding.worker_sha256){return Err(invalid("worker binding format unsupported"))}
  let core=std::fs::canonicalize(std::env::current_exe()?)?;if capture(&core)?.0!=binding.core_sha256{return Err(invalid("worker binding does not name this actual Core"))}let path=core.parent().ok_or_else(||invalid("Core package parent absent"))?.join("crewd-data-worker");
  let (sha,identity)=capture(&path)?;if sha!=binding.worker_sha256{return Err(invalid("sibling worker hash mismatch"))}Ok(Self{path,identity,binding})
 }
 pub fn still_qualified(&self)->io::Result<()>{let (sha,identity)=capture(&self.path)?;if sha!=self.binding.worker_sha256||identity!=self.identity{return Err(invalid("sibling worker package changed"))}Ok(())}
 /// Global capability qualification only: no namespace file or identity is
 /// created. The worker actually opens a fresh in-memory pinned prolly engine.
 pub fn probe(&self,cwd:&Path,registry:Arc<ProcessRegistry>)->io::Result<ProcessIdentity>{
  self.still_qualified()?;let args=[OsString::from("--probe"),OsString::from("--ipc-version"),IPC_VERSION.to_string().into()];
  let deadline=Instant::now()+Duration::from_secs(10);let mut child=OwnedStdio::spawn(&self.path,&args,cwd,registry,format!("@data/probe/{}",uuid::Uuid::new_v4()))?;let birth=child.identity().clone();
  let read=(||{let frame:serde_json::Value=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,deadline)?).map_err(|_|invalid("worker capability probe invalid"))?;if frame!=serde_json::json!({"kind":"workerIdentity","protocol":IPC_VERSION,"engine":"prolly","engineVersion":ENGINE_VERSION}){return Err(invalid("worker capability probe mismatch"))}self.still_qualified()})();
  let clean=child.finish(&||self.still_qualified());read?;clean?;Ok(birth)
 }
 /// Call only after the Core intent transaction is durably committed and the
 /// namespace lock/authority/ACL have been rechecked by the typed controller.
 pub fn exchange(&self,database:&Path,mode:&str,request:Request,registry:Arc<ProcessRegistry>,verify:&dyn Fn()->io::Result<()>)->io::Result<(Response,ProcessIdentity)>{
  self.exchange_until(database,mode,request,registry,verify,Instant::now()+Duration::from_secs(60))
 }
 pub fn exchange_until(&self,database:&Path,mode:&str,request:Request,registry:Arc<ProcessRegistry>,verify:&dyn Fn()->io::Result<()>,budget:Instant)->io::Result<(Response,ProcessIdentity)>{
  if !["readonly","existing","initialize","prepare-initialize","initialize-import"].contains(&mode)||!database.is_absolute(){return Err(invalid("worker open mode/path invalid"))}self.still_qualified()?;verify()?;
  let mut args=vec![OsString::from("--database"),database.as_os_str().into(),OsString::from("--namespace"),request.namespace.clone().into(),OsString::from("--mode"),mode.into(),OsString::from("--ipc-version"),IPC_VERSION.to_string().into()];if ["initialize","prepare-initialize","initialize-import"].contains(&mode){args.push("--initial-request-id".into());args.push(request.id.clone().into());}
  let deadline=budget.min(Instant::now()+Duration::from_secs(60));if deadline<=Instant::now(){return Err(invalid("worker request budget exhausted"))}let key=format!("@data/{}/{}",request.namespace,uuid::Uuid::new_v4());let mut child=OwnedStdio::spawn(&self.path,&args,database.parent().ok_or_else(||invalid("namespace parent missing"))?,registry,key)?;let birth=child.identity().clone();
  let outcome=(||{
   self.still_qualified()?;let ready:serde_json::Value=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,deadline)?).map_err(|_|invalid("worker handshake invalid"))?;
   if ready!=serde_json::json!({"kind":"ready","protocol":IPC_VERSION,"engine":"prolly","engineVersion":ENGINE_VERSION,"namespace":request.namespace}){return Err(invalid("worker handshake does not match qualified package/namespace"))}verify()?;
   let bytes=serde_json::to_vec(&request).map_err(|_|invalid("worker request encoding failed"))?;child.write_frame(&bytes,MAX_FRAME_BYTES,deadline)?;
   #[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Reply{id:String,outcome:Response}
   let reply:Reply=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,deadline)?).map_err(|_|invalid("worker response invalid"))?;if reply.id!=request.id{return Err(invalid("worker response correlation mismatch"))}Ok(reply.outcome)
  })();
  let cleanup=child.finish(verify);match(outcome,cleanup){(Ok(response),Ok(()))=>Ok((response,birth)),(Err(e),_)=>Err(e),(_,Err(e))=>Err(e)}
 }
}
