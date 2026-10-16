//! Private inherited stdin/stdout only; Core supplies a qualified path and namespace.
use crewd_data_protocol::{MAX_FRAME_BYTES,Request,IPC_VERSION,ENGINE_VERSION};
use crewd_data_worker::workspace::Workspace;
use serde_json::json;
use std::{io::{self,BufRead,Read,Write},path::PathBuf,time::{Duration,Instant}};
fn run()->Result<(),String>{
 let args=std::env::args().collect::<Vec<_>>();
 if args.len()==4&&args[1]=="--probe"&&args[2]=="--ipc-version"&&args[3]==IPC_VERSION.to_string(){
  // An actual isolated in-memory engine validates C engine/version and IPC
  // without creating or opening a namespace file during capability checks.
  let engine=crewd_data_worker::engine::Engine::open(std::path::Path::new(":memory:"),false)?;
  let ready=json!({"kind":"workerIdentity","protocol":IPC_VERSION,"engine":"prolly","engineVersion":ENGINE_VERSION});
  let stdout=io::stdout();let mut writer=stdout.lock();serde_json::to_writer(&mut writer,&ready).map_err(|e|e.to_string())?;writer.write_all(b"\n").and_then(|_|writer.flush()).map_err(|e|e.to_string())?;drop(engine);return Ok(())
 }
 if ![9,11].contains(&args.len())||args[1]!="--database"||args[3]!="--namespace"||args[5]!="--mode"||args[7]!="--ipc-version"||args[8]!=IPC_VERSION.to_string(){return Err("expected --database PATH --namespace UUID --mode readonly|existing|initialize --ipc-version1".into())}
 let path=PathBuf::from(&args[2]);if !path.is_absolute(){return Err("absolute private data path required".into())}
 if !["initialize","prepare-initialize","initialize-import"].contains(&args[6].as_str())&&args.len()!=9{return Err("unexpected worker arguments".into())}
 let workspace=match args[6].as_str(){"readonly"=>Workspace::open_existing(&path,&args[4],true)?,"existing"=>Workspace::open_existing(&path,&args[4],false)?,"initialize"|"prepare-initialize"|"initialize-import"=>{if args.len()!=11||args[9]!="--initial-request-id"{return Err("initialize requires the original durable request UUID".into())}if args[6]=="initialize-import"{Workspace::open_for_import(&path,&args[4],&args[10])?}else if args[6]=="prepare-initialize"{Workspace::prepare_for_request(&path,&args[4],&args[10])?}else{Workspace::open_for_request(&path,&args[4],&args[10])?}},_=>return Err("unknown open mode".into())};
 let stdin=io::stdin();let mut reader=stdin.lock();let stdout=io::stdout();let mut writer=stdout.lock();
 let ready=json!({"kind":"ready","protocol":IPC_VERSION,"engine":"prolly","engineVersion":ENGINE_VERSION,"namespace":args[4]});
 serde_json::to_writer(&mut writer,&ready).map_err(|e|e.to_string())?;writer.write_all(b"\n").and_then(|_|writer.flush()).map_err(|e|e.to_string())?;
 loop{
  let mut bytes=Vec::new();let n=reader.by_ref().take(MAX_FRAME_BYTES as u64+1).read_until(b'\n',&mut bytes).map_err(|e|e.to_string())?;
  if n==0{break}if n>MAX_FRAME_BYTES||bytes.last()!=Some(&b'\n'){return Err("oversize or incomplete private protocol frame".into())}
  let request:Request=serde_json::from_slice(&bytes).map_err(|_|"invalid typed private protocol request")?;
  let id=request.id.clone();workspace.engine.set_request_deadline(Some(Instant::now()+Duration::from_secs(60)));let outcome=workspace.handle(request);workspace.engine.set_request_deadline(None);
  let response=serde_json::to_vec(&json!({"id":id,"outcome":outcome})).map_err(|e|e.to_string())?;
  if response.len()>MAX_FRAME_BYTES{return Err("private response bound exceeded".into())}
  writer.write_all(&response).and_then(|_|writer.write_all(b"\n")).and_then(|_|writer.flush()).map_err(|e|e.to_string())?;
 }
 Ok(())
}
fn main(){if let Err(error)=run(){let _=writeln!(io::stderr(),"data-worker: {error}");std::process::exit(1)}}
