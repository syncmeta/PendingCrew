//! Owned helper control. Core authenticates each child-created channel and
//! owns all DB writes; this process owns only its genuine PTY and byte stream.
use super::{
    helper::{CoreLedger, NativePtyEvents, NativePtyObservation, NativePtySession},
    terminal::TerminalTap,
    transport::{ControlReader, write_control, write_terminal},
    wire::{Control, TerminalFrame, TerminalKind},
};
use crate::{
    driver::{ManualWritePermit, RawWriteOutcome, ResizeOutcome},
    registry::{ProbeResult, ProcessIdentity},
    session::ExitStatusKind,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bind {
    lease_id: String,
    path: std::path::PathBuf,
    nonce: [u8; 16],
    duration_ms: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    lease_id: String,
    runtime_id: String,
    revision: i64,
    bytes: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resize {
    lease_id: String,
    columns: u16,
    rows: u16,
}
struct Lease {
    fence: super::manual_fence::ManualFence,
    core: ProcessIdentity,
    native: ProcessIdentity,
    deadline: Instant,
}
impl Lease {
    fn permit(&self) -> ManualWritePermit {
        ManualWritePermit::for_native_shared(
            self.deadline,
            self.native.clone(),
            self.fence.clone(),
            self.core.clone(),
        )
    }
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "owned native helper control refused",
    )
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
/// No initial prompt or automatic content is supplied through this channel.
/// Genuine SDK content/turn observations remain the separately enrolled route.
/// The future complete provider factory must invoke this ordinary production
/// function; merely linking it is not a capability or runtime acceptance claim.
pub async fn run(
    session: NativePtySession,
    mut events: NativePtyEvents,
    ledger: Arc<CoreLedger>,
    control: CoreLedger,
    terminal: CoreLedger,
    tap: TerminalTap,
    mut output: tokio::sync::broadcast::Receiver<TerminalFrame>,
) -> io::Result<()> {
    let core = ledger.verified_core_birth()?;
    if control.verified_core_birth()? != core || terminal.verified_core_birth()? != core {
        return Err(invalid());
    }
    let control = control.into_verified_parent_socket()?;
    control.set_nonblocking(true)?;
    let terminal = terminal.into_verified_parent_socket()?;
    terminal.set_nonblocking(true)?;
    let (read, mut writer) = tokio::net::UnixStream::from_std(control)?.into_split();
    let mut reader = ControlReader::new(read);
    let mut terminal_writer = tokio::net::UnixStream::from_std(terminal)?;
    let output_tap = tap.clone();
    let terminal_task = tokio::spawn(async move {
        loop {
            let frame = match output.recv().await {
                Ok(frame) => frame,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => TerminalFrame {
                    epoch: output_tap.epoch(),
                    sequence: output_tap.sequence().max(1),
                    kind: TerminalKind::Resync,
                    bytes: vec![],
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Ok::<(), io::Error>(());
                }
            };
            // A stalled observation socket cannot hold the PTY or Core input.
            tokio::time::timeout(
                Duration::from_millis(250),
                write_terminal(&mut terminal_writer, &frame),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "native viewer stalled"))??;
        }
    });
    let mut leases = BTreeMap::<String, Lease>::new();
    let mut seen = BTreeSet::new();
    let mut watchdog = tokio::time::interval(Duration::from_millis(100));
    let outcome=async {
        loop { tokio::select! {
            _=watchdog.tick()=>{ledger.verified_core_birth()?;},
            frame=reader.read()=>{
                let (id,method,params)=match frame? {
                    None=>break,
                    Some(Control::Request{id,method,params})=>(id,method,params),
                    Some(_)=>return Err(invalid()),
                };
                ledger.verified_core_birth()?;
                let request=id.as_str().filter(|id|uuid(id)).ok_or_else(invalid)?;
                if seen.len()>=4096 || !seen.insert(request.to_owned()){return Err(invalid());}
                let result=match method.as_str(){
                    "native/manual.bind"=>{
                        let p:Bind=serde_json::from_value(params).map_err(|_|invalid())?;
                        if !uuid(&p.lease_id) || p.duration_ms==0 || p.duration_ms>30_000 || leases.contains_key(&p.lease_id){return Err(invalid());}
                        let native=session.owned_native_birth().await?;
                        let fence=ledger.open_manual_fence(&p.path,p.nonce)?;
                        leases.insert(p.lease_id.clone(),Lease{fence,core:core.clone(),native,deadline:Instant::now()+Duration::from_millis(u64::from(p.duration_ms))});
                        json!({"adopted":true,"leaseId":p.lease_id,"inputWritten":false})
                    },
                    "native/manual.write"=>{
                        let p:Raw=serde_json::from_value(params).map_err(|_|invalid())?;
                        let lease=leases.get(&p.lease_id).ok_or_else(invalid)?;
                        let committed=ledger.revoke_for_raw(&p.runtime_id,p.revision,p.bytes)?;
                        let result=session.raw_confirmed(committed,lease.permit()).await.map_err(|_|invalid())?;
                        match result {
                            RawWriteOutcome::Written{bytes}=>json!({"writtenBytes":bytes,"unknown":false,"applicationConsumed":"unconfirmed"}),
                            RawWriteOutcome::Unknown{written_bytes}=>json!({"writtenBytes":written_bytes,"unknown":true}),
                            _=>json!({"writtenBytes":0,"unknown":true}),
                        }
                    },
                    "native/manual.resize"=>{
                        let p:Resize=serde_json::from_value(params).map_err(|_|invalid())?;
                        let lease=leases.get(&p.lease_id).ok_or_else(invalid)?;
                        let result=session.resize(p.columns,p.rows,lease.permit()).await.map_err(|_|invalid())?;
                        json!({"applied":matches!(result,ResizeOutcome::Applied),"unknown":!matches!(result,ResizeOutcome::Applied|ResizeOutcome::InvalidSize)})
                    },
                    "native/manual.release"=>{
                        let lease=params.as_object().filter(|p|p.len()==1).and_then(|p|p.get("leaseId")).and_then(Value::as_str).ok_or_else(invalid)?;
                        leases.remove(lease).ok_or_else(invalid)?;json!({"released":true})
                    },
                    "native/terminal.snapshot" if params==json!({})=>{
                        let cut=session.terminal_cut().await.map_err(|_|invalid())?.ok_or_else(invalid)?;
                        if cut.screen.bytes.len()>512*1024 {
                            json!({"epoch":cut.epoch,"sequence":cut.sequence,"snapshotUnavailable":true,"truncated":true})
                        } else {
                            json!({"epoch":cut.epoch,"sequence":cut.sequence,"cols":cut.screen.cols,"rows":cut.screen.lines,"bytesBase64":STANDARD.encode(&cut.screen.bytes),"truncated":false})
                        }
                    },
                    "native/stop" if params==json!({})=>{session.stop_requested().map_err(|_|invalid())?;json!({"stopRequested":true,"physicalExit":"unconfirmed"})},
                    _=>return Err(invalid()),
                };
                send_control(&mut writer,&json!({"jsonrpc":"2.0","id":id,"result":result})).await?;
            },
            event=events.recv()=>match event{
                Some(NativePtyObservation::Spawned{pid,agent_session_id,resumed})=>{
                    let native=session.owned_native_birth().await?;
                    if native.pid!=pid as i32{return Err(invalid());}
                    send_control(&mut writer,&json!({"jsonrpc":"2.0","method":"native/physical.spawned","params":{"pid":pid,"agentSessionId":agent_session_id,"resumed":resumed}})).await?;
                },
                Some(NativePtyObservation::Exited(exit))=>{
                    let (kind,value)=match exit.status{ExitStatusKind::Code(code)=>("code",code),ExitStatusKind::Signal(signal)=>("signal",signal)};
                    send_control(&mut writer,&json!({"jsonrpc":"2.0","method":"native/physical.exited","params":{"kind":kind,"value":value,"stoppedByUser":exit.stopped_by_user}})).await?;break;
                },
                None=>break,
                // Screen decisions/quiet observations never become successful
                // native completion, structured approval or automatic delivery.
                Some(_)=>{},
            },
        }}
        Ok(())
    }.await;
    leases.clear();
    terminal_task.abort();
    if !session.is_closed() {
        let birth = session.owned_native_birth().await.ok();
        let _ = session.stop_requested();
        let reaped = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.recv().await {
                if matches!(event, NativePtyObservation::Exited(_)) {
                    return true;
                }
            }
            false
        })
        .await;
        if !matches!(reaped, Ok(true)) {
            if let Some(birth) = birth {
                if matches!(crate::registry::probe(birth.pid),ProbeResult::Found(ref current) if current.pid==birth.pid && current.pgid==birth.pgid && current.start_time==birth.start_time && current.boot_id==birth.boot_id)
                {
                    crate::registry::apply(
                        &crate::registry::decide(&birth, &crate::registry::probe(birth.pid)),
                        &birth,
                    );
                }
            }
        }
    }
    outcome
}

async fn send_control<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), write_control(writer, value))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "native Core control write unknown"))?
}
