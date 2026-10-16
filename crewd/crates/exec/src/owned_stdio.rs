//! A Core-owned bounded stdio child, without PTY, principal or listener.
//! No provider environment is inherited. Unknown cleanup retains the registry.
use crate::registry::{self,ProcessRegistry,ProcessIdentity,ProbeResult};
use std::{ffi::OsString,io::{self,Read,Write},os::{fd::AsRawFd,unix::process::CommandExt},path::Path,process::{Child,ChildStdin,ChildStdout,Command,Stdio},sync::Arc,time::{Instant,Duration}};
fn error(s:&str)->io::Error{io::Error::other(s)}
pub struct OwnedStdio{child:Child,input:Option<ChildStdin>,output:ChildStdout,pending:Vec<u8>,registry:Arc<ProcessRegistry>,key:String,birth:ProcessIdentity,finished:bool,reaped:bool}
fn nonblocking(fd:i32)->io::Result<()>{let flags=unsafe{libc::fcntl(fd,libc::F_GETFL)};if flags<0||unsafe{libc::fcntl(fd,libc::F_SETFL,flags|libc::O_NONBLOCK)}<0{return Err(io::Error::last_os_error())}Ok(())}
fn pause(deadline:Instant)->io::Result<()>{if Instant::now()>=deadline{return Err(error("owned stdio deadline exhausted"))}std::thread::sleep(Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())));Ok(())}
impl OwnedStdio{
 pub fn spawn(executable:&Path,args:&[OsString],cwd:&Path,registry:Arc<ProcessRegistry>,key:String)->io::Result<Self>{
  let mut enrollment=registry.begin_enrollment()?;let mut limits=std::mem::MaybeUninit::<libc::rlimit>::uninit();if unsafe{libc::getrlimit(libc::RLIMIT_NOFILE,limits.as_mut_ptr())}!=0{return Err(io::Error::last_os_error())}let limit=unsafe{limits.assume_init()}.rlim_cur;if limit>4_194_304{return Err(io::Error::other(format!("fd inheritance finite bound unavailable: {limit}")))}
  let mut command=Command::new(executable);command.args(args).current_dir(cwd).env_clear().env("LANG","C").env("TZ","UTC").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0);
  // fcntl is async-signal-safe. Mark all inherited non-stdio descriptors CLOEXEC,
  // preserving Rust's error pipe until exec; never close another task's FDs.
  unsafe{command.pre_exec(move||{libc::umask(0o077);for fd in 3..limit as i32{let flags=libc::fcntl(fd,libc::F_GETFD);if flags>=0{if libc::fcntl(fd,libc::F_SETFD,flags|libc::FD_CLOEXEC)<0{return Err(io::Error::last_os_error())}}else{let e=io::Error::last_os_error();if e.raw_os_error()!=Some(libc::EBADF){return Err(e)}}}Ok(())});}
  let mut child=command.spawn()?;enrollment.arm_owned();
  let outcome=(||{
   let birth=match registry::probe(child.id() as i32){ProbeResult::Found(v)if v.pid==child.id() as i32&&v.pgid==v.pid&&!v.boot_id.is_empty()=>v,_=>return Err(error("owned worker birth unknown"))};
   match registry::parent(birth.pid){registry::ParentProbe::Found{identity:ref actual,parent}if parent==std::process::id() as i32&&actual.pid==birth.pid&&actual.pgid==birth.pgid&&actual.start_time==birth.start_time&&actual.boot_id==birth.boot_id=>{},_=>return Err(error("owned worker direct parent unknown"))}
   registry.record(&key,birth.clone())?;
   let input=child.stdin.take().ok_or_else(||error("worker stdin absent"))?;let output=child.stdout.take().ok_or_else(||error("worker stdout absent"))?;nonblocking(input.as_raw_fd())?;nonblocking(output.as_raw_fd())?;Ok((birth,input,output))
  })();
  match outcome{Ok((birth,input,output))=>{enrollment.settle();Ok(Self{child,input:Some(input),output,pending:vec![],registry,key,birth,finished:false,reaped:false})},Err(e)=>{let _=child.kill();let deadline=Instant::now()+Duration::from_secs(2);while Instant::now()<deadline{if child.try_wait().ok().flatten().is_some(){break}let _=pause(deadline);} // Unresolved enrollment deliberately remains restrictive, even if PID was reaped.
   drop(enrollment);Err(e)}}
 }
 pub fn identity(&self)->&ProcessIdentity{&self.birth}
 pub fn write_frame(&mut self,bytes:&[u8],bound:usize,deadline:Instant)->io::Result<()>{if bytes.len()>bound||bytes.contains(&b'\n'){return Err(error("invalid bounded stdio frame"))}let input=self.input.as_mut().ok_or_else(||error("worker input closed"))?;let mut framed=bytes.to_vec();framed.push(b'\n');let mut offset=0;while offset<framed.len(){if Instant::now()>=deadline{return Err(error("worker write timeout"))}match input.write(&framed[offset..]){Ok(0)=>return Err(error("worker write EOF")),Ok(n)=>offset+=n,Err(e)if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>pause(deadline)?,Err(e)=>return Err(e)}}Ok(())}
 pub fn read_frame(&mut self,bound:usize,deadline:Instant)->io::Result<Vec<u8>>{loop{if let Some(end)=self.pending.iter().position(|b|*b==b'\n'){if end>bound{return Err(error("worker frame too large"))}let rest=self.pending.split_off(end+1);let mut frame=std::mem::replace(&mut self.pending,rest);frame.pop();return Ok(frame)}if self.pending.len()>bound{return Err(error("worker incomplete frame too large"))}if Instant::now()>=deadline{return Err(error("worker read timeout"))}let mut buffer=[0;8192];match self.output.read(&mut buffer){Ok(0)=>return Err(error("worker output EOF")),Ok(n)=>self.pending.extend_from_slice(&buffer[..n]),Err(e)if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)if e.kind()==io::ErrorKind::WouldBlock=>pause(deadline)?,Err(e)=>return Err(e)}}}
 pub fn finish(mut self,verify:&dyn Fn()->io::Result<()>)->io::Result<()>{
  self.input.take();let deadline=Instant::now()+Duration::from_secs(2);let grace=Instant::now()+Duration::from_millis(250);let mut killed=false;let mut status=None;
  while Instant::now()<deadline{if let Some(s)=self.child.try_wait()?{self.reaped=true;status=Some(s);break}if !killed&&Instant::now()>=grace{self.child.kill()?;killed=true;}pause(deadline)?;}
  let Some(status)=status else{return Err(error("owned worker terminal unknown"))};
  let leader_gone=match registry::probe(self.birth.pid){ProbeResult::Missing=>true,ProbeResult::Found(v)=>v.start_time!=self.birth.start_time||v.boot_id!=self.birth.boot_id,_=>false};
  if !leader_gone||!matches!(registry::recovery::observe_group_until(self.birth.pgid,deadline),registry::recovery::GroupObservation::Absent){return Err(error("owned worker physical group settlement unknown"))}
  verify()?;self.registry.forget(&self.key)?;self.finished=true;
  if !status.success()||killed{return Err(error("worker required termination or returned failure"))}Ok(())
 }
}
impl Drop for OwnedStdio{fn drop(&mut self){if !self.finished{self.input.take();if self.reaped{return}let _=self.child.kill();let end=Instant::now()+Duration::from_secs(1);while Instant::now()<end{if self.child.try_wait().ok().flatten().is_some(){break}let _=pause(end);}/* Registry entry is retained until whole-group physical settlement. */}}}
