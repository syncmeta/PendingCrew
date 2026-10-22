//! Bounded, registered metadata-only app-server operation. This code cannot
//! select resume/start/turn/login/config methods from either JSON or strings.
use super::{control::{CodexOperation as Op,CodexOutcome as Out},proto,rpc,transport::{self,CodexProcess,CodexProcessSpec,Line}};
use crate::{driver::{kill_group,REAP_LIMIT},registry::{ProcessRegistry,probe,ProbeResult}};
use serde_json::{Value,json};
use std::{path::PathBuf,sync::Arc,time::Duration};

pub struct MetadataRequest {
    pub session_id:String,pub thread_id:String,pub workdir:PathBuf,pub operation:Op,
}
struct Guard {proc:CodexProcess,registry:Option<Arc<ProcessRegistry>>,key:String,armed:bool}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed {kill_group(self.proc.pid);}
        self.proc.writer_task.abort();self.proc.stderr_task.abort();
        // A cancelled future keeps the durable registry entry for ordinary boot
        // recovery; only explicit wait/reap below removes that evidence.
    }
}
async fn response(g:&mut Guard,id:i64)->Result<Value,Out> {
    loop {
        match g.proc.stdout.next_line().await {
            Ok(Some(Line::Text(line)))=>match rpc::parse_line(&line) {
                Ok(rpc::Incoming::Response{id:rpc::RequestId::Int(got),result}) if got==id=>return result.map_err(|e|Out::Rejected(format!("{} ({})",e.message,e.code))),
                Ok(rpc::Incoming::Request{id,..})=> {
                    // Metadata must not acquire a tool, login, attestation or
                    // other side effect by answering unsolicited server calls.
                    let msg=rpc::error_line(&id,-32601,"PCW metadata-only operation cannot answer provider side-effect requests");
                    let _=g.proc.writer.send(msg.into());
                    return Err(Out::Unknown{provider_accepted:false,detail:"unexpected provider request in metadata-only child".into()});
                }
                _=>{}
            },
            Ok(Some(Line::TooLong{..}))=>return Err(Out::Unknown{provider_accepted:false,detail:"metadata response exceeds original transport bound".into()}),
            other=>return Err(Out::Unknown{provider_accepted:false,detail:format!("metadata transport closed: {other:?}")}),
        }
    }
}
pub async fn run(spec:CodexProcessSpec,registry:Option<Arc<ProcessRegistry>>,request:MetadataRequest)->Out {
    let (method,params)=match request.operation {
        Op::Name(name) if !name.trim().is_empty()&&name.len()<=512=> ("thread/name/set",json!({"threadId":request.thread_id,"name":name})),
        Op::History(p) if p.limit.is_none_or(|n|n>0&&n<=100)&&p.cursor.as_ref().is_none_or(|c|c.len()<=8192)=>{
            let mut v=json!({"threadId":request.thread_id,"limit":p.limit.unwrap_or(50)});
            if let Some(c)=p.cursor{v["cursor"]=json!(c);}
            if let Some(i)=p.items_view{v["itemsView"]=json!(i);}
            if let Some(i)=p.sort_direction{v["sortDirection"]=json!(i);}
            ("thread/turns/list",v)
        }
        Op::Fork{last_turn_id}=>{
            let mut v=json!({"threadId":request.thread_id,"excludeTurns":true});
            if let Some(t)=last_turn_id{v["lastTurnId"]=json!(t);}("thread/fork",v)
        }
        Op::Models=>("model/list",json!({"limit":100})),
        Op::Tools=>("mcpServerStatus/list",json!({"limit":100})),
        _=>return Out::Rejected("operation is not permitted in metadata-only child".into()),
    };
    let proc=match transport::spawn_codex(&spec){Ok(p)=>p,Err(e)=>return Out::Rejected(format!("metadata child not spawned: {e}"))};
    let key=format!("codex-metadata/{}/{}",request.session_id,uuid::Uuid::new_v4());
    let mut guard=Guard{proc,registry,key,armed:true};
    if let Some(r)=&guard.registry {
        let r=r.clone();let pid=guard.proc.pid;let key=guard.key.clone();
        let recorded=tokio::task::spawn_blocking(move||match probe(pid as i32){ProbeResult::Found(b) if b.pgid==pid as i32=>r.record(&key,b),_=>Err(std::io::Error::other("metadata child birth is unconfirmed"))}).await;
        if !matches!(recorded,Ok(Ok(()))) {
            kill_group(guard.proc.pid);let _=tokio::time::timeout(REAP_LIMIT,guard.proc.child.wait()).await;
            return Out::Rejected("metadata child registry failed; no provider request sent".into());
        }
    }
    let work=async {
        guard.proc.writer.send(json!({"id":1,"method":"initialize","params":proto::initialize_params(env!("CARGO_PKG_VERSION"))}).to_string().into()).map_err(|_|Out::Unknown{provider_accepted:false,detail:"metadata writer closed".into()})?;
        response(&mut guard,1).await?;
        guard.proc.writer.send(rpc::notification_line("initialized",None).into()).map_err(|_|Out::Unknown{provider_accepted:false,detail:"metadata initialized write failed".into()})?;
        guard.proc.writer.send(json!({"id":2,"method":method,"params":params}).to_string().into()).map_err(|_|Out::Unknown{provider_accepted:false,detail:"metadata request write failed".into()})?;
        let value=response(&mut guard,2).await?;
        if method=="mcpServerStatus/list" {
            guard.proc.writer.send(json!({"id":3,"method":"skills/list","params":{"cwds":[request.workdir]}}).to_string().into()).map_err(|_|Out::Unknown{provider_accepted:false,detail:"metadata skills write failed".into()})?;
            let skills=response(&mut guard,3).await?;
            Ok(json!({"mcpServers":value,"skills":skills}))
        } else {Ok(value)}
    };
    let result=match tokio::time::timeout(Duration::from_secs(30),work).await {
        Ok(Ok(v))=>Out::Observed(json!({"providerAccepted":true,"application":"metadata","providerResult":v})),
        Ok(Err(e))=>e,Err(_)=>Out::Unknown{provider_accepted:false,detail:"metadata child deadline exceeded".into()},
    };
    kill_group(guard.proc.pid);
    let reaped=matches!(tokio::time::timeout(REAP_LIMIT,guard.proc.child.wait()).await,Ok(Ok(_)));
    if reaped {
        guard.armed=false;
        if let Some(r)=&guard.registry {let r=r.clone();let key=guard.key.clone();let _=tokio::task::spawn_blocking(move||r.forget(&key)).await;}
    }
    result
}
