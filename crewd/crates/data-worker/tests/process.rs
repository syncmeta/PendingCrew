//! Real separate worker executable and inherited stdio. No Core service, account or model.
use crewd_exec::{owned_stdio::OwnedStdio,registry::{ProcessRegistry,ProbeResult,self,recovery::{observe_group,GroupObservation}}};
use crewd_data_protocol::*;
use serde_json::{Value,json};
use std::{path::Path,sync::Arc,ffi::OsString,time::{Instant,Duration}};
fn uuid()->String{uuid::Uuid::new_v4().to_string()}
fn spawn(path:&Path,namespace:&str,mode:&str,registry:Arc<ProcessRegistry>)->OwnedStdio{
 let mut args=vec![OsString::from("--database"),path.as_os_str().into(),"--namespace".into(),namespace.into(),"--mode".into(),mode.into(),"--ipc-version".into(),"1".into()];if mode=="initialize"{args.push("--initial-request-id".into());args.push(uuid().into());}
 OwnedStdio::spawn(Path::new(env!("CARGO_BIN_EXE_crewd-data-worker")),&args,path.parent().unwrap(),registry,format!("@data/{namespace}/{}",uuid())).unwrap()
}
fn ready(child:&mut OwnedStdio,ns:&str){let v:Value=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(10)).unwrap()).unwrap();assert_eq!(v,json!({"kind":"ready","protocol":1,"engine":"prolly","engineVersion":"0.51.0","namespace":ns}));}
fn exchange(child:&mut OwnedStdio,ns:&str,op:Operation)->Value{let id=uuid();let request=Request{id:id.clone(),namespace:ns.into(),branch:"main".into(),operation:op};let deadline=Instant::now()+Duration::from_secs(10);child.write_frame(&serde_json::to_vec(&request).unwrap(),MAX_FRAME_BYTES,deadline).unwrap();let value:Value=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,deadline).unwrap()).unwrap();assert_eq!(value["id"],id);value["outcome"].clone()}
#[test]
fn separate_process_lost_reply_lookup_no_replay_and_real_registry_settlement(){
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("workspace.dolt");let ns=uuid();let registry=Arc::new(ProcessRegistry::load(&dir.path().join("processes.json")).unwrap());let mut child=spawn(&path,&ns,"initialize",registry.clone());let birth=child.identity().clone();ready(&mut child,&ns);assert_eq!(registry.try_entries().unwrap().len(),1);
 let info=exchange(&mut child,&ns,Operation::Inspect);assert_eq!(info["status"],"ok");let head=info["result"]["head"].as_str().unwrap().to_owned();let request_id=uuid();let intent=WriteIntent{request_id:request_id.clone(),expected_head:head,author:ResourceActor{kind:"human".into(),session_id:None},message:"lost transport reply actual worker".into(),mutations:vec![Mutation::Sql{sql:"CREATE TABLE facts(id INTEGER PRIMARY KEY,value TEXT)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO facts VALUES(1,'once')".into(),params:vec![]}]};
 // Submit exactly once. Deliberately do not consume this response. Close stdin,
 // then wait the genuine worker; next incarnation performs only UUID lookup.
 let request=Request{id:uuid(),namespace:ns.clone(),branch:"main".into(),operation:Operation::Apply(intent)};child.write_frame(&serde_json::to_vec(&request).unwrap(),MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(10)).unwrap();child.finish(&||Ok(())).unwrap();
 assert!(matches!(registry::probe(birth.pid),ProbeResult::Missing));assert!(matches!(observe_group(birth.pgid),GroupObservation::Absent));assert!(registry.try_entries().unwrap().is_empty());
 let mut child=spawn(&path,&ns,"readonly",registry.clone());ready(&mut child,&ns);let lookup=exchange(&mut child,&ns,Operation::Lookup{request_id});assert_eq!(lookup["status"],"ok");assert_eq!(lookup["result"]["status"],"known");let count=exchange(&mut child,&ns,Operation::Read{at_commit:None,sql:"SELECT count(*) FROM facts".into(),params:vec![]});assert_eq!(count["result"]["rows"][0][0],json!({"type":"integer","value":"1"}));child.finish(&||Ok(())).unwrap();assert!(registry.try_entries().unwrap().is_empty());
}
#[test]
fn missing_readonly_never_creates_database_and_malformed_owned_worker_is_reaped(){
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("absent.dolt");let ns=uuid();let registry=Arc::new(ProcessRegistry::load(&dir.path().join("processes.json")).unwrap());let mut child=spawn(&path,&ns,"readonly",registry.clone());assert!(child.read_frame(MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(5)).is_err());assert!(child.finish(&||Ok(())).is_err());assert!(!path.exists());assert!(registry.try_entries().unwrap().is_empty());
 let mut child=spawn(&path,&ns,"initialize",registry.clone());ready(&mut child,&ns);let birth=child.identity().clone();child.write_frame(b"{not valid JSON}",MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(5)).unwrap();assert!(child.read_frame(MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(5)).is_err());assert!(child.finish(&||Ok(())).is_err());assert!(registry.try_entries().unwrap().is_empty());assert!(matches!(registry::probe(birth.pid),ProbeResult::Missing));assert!(matches!(observe_group(birth.pgid),GroupObservation::Absent));
}
#[test]
fn actual_in_memory_probe_reaps_own_birth_without_initializing_a_namespace(){
 use std::os::unix::fs::PermissionsExt;
 let dir=tempfile::tempdir().unwrap();let registry=Arc::new(ProcessRegistry::load(&dir.path().join("processes.json")).unwrap());
 let args=[OsString::from("--probe"),OsString::from("--ipc-version"),OsString::from("1")];let mut child=OwnedStdio::spawn(Path::new(env!("CARGO_BIN_EXE_crewd-data-worker")),&args,dir.path(),registry.clone(),format!("@data/probe/{}",uuid())).unwrap();let birth=child.identity().clone();
 let actual:Value=serde_json::from_slice(&child.read_frame(MAX_FRAME_BYTES,Instant::now()+Duration::from_secs(10)).unwrap()).unwrap();assert_eq!(actual,json!({"kind":"workerIdentity","protocol":1,"engine":"prolly","engineVersion":"0.51.0"}));child.finish(&||Ok(())).unwrap();
 assert!(registry.try_entries().unwrap().is_empty());assert!(matches!(registry::probe(birth.pid),ProbeResult::Missing));assert!(matches!(observe_group(birth.pgid),GroupObservation::Absent));
 assert!(!dir.path().read_dir().unwrap().any(|p|p.unwrap().path().extension().is_some_and(|x|x=="dolt")));
 let path=dir.path().join("new.dolt");let ns=uuid();let mut child=spawn(&path,&ns,"initialize",registry.clone());ready(&mut child,&ns);child.finish(&||Ok(())).unwrap();assert_eq!(path.metadata().unwrap().permissions().mode()&0o777,0o600);assert!(registry.try_entries().unwrap().is_empty());
}
