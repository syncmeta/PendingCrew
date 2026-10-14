//! Private helper entry. Boot fields never authenticate a process: genuine
//! parent/kernel channels and Core custody must pass before native launch.
use super::{
    birth::BirthGate,
    helper::{CoreLedger, launch_native},
    terminal::TerminalTap,
    transport::ControlReader,
    wire::Control,
};
use crate::{
    driver::{ClaudeStart, DriverConfig},
    launch::ClaudeSpec,
    registry::{self, ParentProbe, ProcessIdentity},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    os::fd::{FromRawFd, OwnedFd},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Boot {
    pub session: String,
    pub generation: String,
    pub nonce: [u8; 16],
    pub core: ProcessIdentity,
    pub ledger_socket: PathBuf,
    pub control_socket: PathBuf,
    pub terminal_socket: PathBuf,
    pub birth_fd: i32,
    pub program: PathBuf,
    pub child_path: String,
    pub workdir: PathBuf,
    pub runtime_dir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub additional_env: BTreeMap<String, String>,
}
fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "private native helper boot refused",
    )
}
impl Boot {
    fn kernel_parent(&self) -> io::Result<()> {
        if !matches!(registry::parent(std::process::id() as i32),ParentProbe::Found{identity,parent}
            if identity.pgid==identity.pid&&parent==self.core.pid)
            || !matches!(registry::probe(self.core.pid),registry::ProbeResult::Found(ref actual) if actual.pid==self.core.pid && actual.pgid==self.core.pgid && actual.start_time==self.core.start_time && actual.boot_id==self.core.boot_id)
        {
            return Err(refused());
        }
        Ok(())
    }
    fn validate(&self) -> io::Result<()> {
        self.kernel_parent()?;
        if self.nonce == [0; 16]
            || self.birth_fd < 3
            || self.birth_fd > 1024
            || [
                &self.ledger_socket,
                &self.control_socket,
                &self.terminal_socket,
                &self.program,
                &self.workdir,
                &self.runtime_dir,
            ]
            .iter()
            .any(|path| !path.is_absolute())
            || self.ledger_socket == self.control_socket
            || self.ledger_socket == self.terminal_socket
            || self.control_socket == self.terminal_socket
            || !uuid::Uuid::parse_str(&self.generation)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.generation)
        {
            return Err(refused());
        }
        crate::resume::check_agent_session_id(&self.session)?;
        // Reject unknown/stale/non-socket descriptors before assuming ownership.
        let mut kind = 0i32;
        let mut len = std::mem::size_of_val(&kind) as libc::socklen_t;
        // SAFETY: exact live descriptor query with correctly sized stack output.
        let rc = unsafe {
            libc::getsockopt(
                self.birth_fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut i32).cast(),
                &mut len,
            )
        };
        let mut address = std::mem::MaybeUninit::<libc::sockaddr_storage>::zeroed();
        let mut address_len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: correctly sized zeroed stack output; inspect only on success.
        let address_rc = unsafe {
            libc::getsockname(self.birth_fd, address.as_mut_ptr().cast(), &mut address_len)
        };
        // SAFETY: the entire buffer began initialized, and kernel success is
        // required before its family is used as descriptor evidence.
        let unix = address_rc == 0
            && address_len as usize >= std::mem::size_of::<libc::sa_family_t>()
            && unsafe { address.assume_init() }.ss_family as i32 == libc::AF_UNIX;
        if rc != 0
            || !unix
            || kind != libc::SOCK_STREAM
            || len as usize != std::mem::size_of_val(&kind)
        {
            return Err(refused());
        }
        Ok(())
    }
}
pub async fn run<R: tokio::io::AsyncRead + Unpin>(input: R) -> io::Result<()> {
    let first = tokio::time::timeout(Duration::from_secs(5), ControlReader::new(input).read())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "private helper boot deadline"))??;
    let Some(Control::Request { method, params, .. }) = first else {
        return Err(refused());
    };
    if method != "native/helper.boot" {
        return Err(refused());
    }
    let boot: Boot = serde_json::from_value(params).map_err(|_| refused())?;
    boot.validate()?;
    // SAFETY: the inherited socket was queried above, is not stdio, and this
    // entry takes sole ownership before opening any further channel or child.
    let birth = unsafe { OwnedFd::from_raw_fd(boot.birth_fd) };
    let ledger = Arc::new(
        CoreLedger::connect(
            &boot.ledger_socket,
            boot.session.clone(),
            boot.generation.clone(),
            boot.nonce,
            boot.core.clone(),
            Duration::from_secs(2),
        )
        .await?,
    );
    let control = CoreLedger::connect(
        &boot.control_socket,
        boot.session.clone(),
        boot.generation.clone(),
        boot.nonce,
        boot.core.clone(),
        Duration::from_secs(2),
    )
    .await?;
    let terminal = CoreLedger::connect(
        &boot.terminal_socket,
        boot.session.clone(),
        boot.generation.clone(),
        boot.nonce,
        boot.core.clone(),
        Duration::from_secs(2),
    )
    .await?;
    boot.kernel_parent()?;
    let tap = TerminalTap::new(1, 64).map_err(|_| refused())?;
    let output = tap.subscribe(); // subscribe before native can emit its first byte
    let mut config = DriverConfig::new(boot.runtime_dir);
    config.native_birth_gate = Some(BirthGate::new(birth, boot.nonce, Duration::from_secs(5))?);
    config.terminal_observer = Some(Arc::new(tap.clone()));
    let start = ClaudeStart {
        session_id: boot.session,
        program: boot.program,
        child_path: boot.child_path,
        spec: ClaudeSpec {
            workdir: boot.workdir,
            model: boot.model,
            effort: boot.effort,
            fast_mode: None,
            system_prompt: boot.system_prompt,
            initial_prompt: None,
            comms: None,
            additional_env: boot.additional_env,
        },
        ledger: ledger.clone(),
    };
    let (session, events) = launch_native(start, config, ledger.clone())
        .await
        .map_err(|_| io::Error::other("private native launch failed"))?;
    super::helper_runtime::run(session, events, ledger, control, terminal, tap, output).await
}
