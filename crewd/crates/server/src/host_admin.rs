//! Root systemd socket management. This is not a public RPC or a Human bootstrap.
use crate::{AppState, peer::RootAdminPermit};
use crewd_proto::*;
use std::{io, path::Path, sync::Arc};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct HostAdminSocket {
    listener: UnixListener,
    admissions: Arc<Semaphore>,
}
impl HostAdminSocket {
    /// Only the explicitly named root-created listening FD is accepted. No JSON identity.
    #[cfg(target_os = "linux")]
    pub fn from_systemd(path: &Path) -> anyhow::Result<Self> {
        use std::os::{fd::FromRawFd, unix::fs::MetadataExt};
        anyhow::ensure!(
            path.is_absolute() && path.file_name().is_some(),
            "absolute root admin socket path required"
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("admin socket directory missing"))?;
        let directory = std::fs::symlink_metadata(parent)?;
        anyhow::ensure!(
            directory.is_dir()
                && !directory.file_type().is_symlink()
                && directory.uid() == 0
                && directory.mode() & 0o777 == 0o700,
            "root0700 admin directory required"
        );
        anyhow::ensure!(
            std::env::var("LISTEN_PID")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                == Some(std::process::id())
                && std::env::var("LISTEN_FDS").as_deref() == Ok("1")
                && std::env::var("LISTEN_FDNAMES").as_deref() == Ok("pcw-admin"),
            "exact systemd pcw-admin activation required"
        );
        // The protected0700 directory intentionally prevents the unprivileged service
        // from statting its root0600 pathname. Unit owns that mode; the inherited FD,
        // root owner, listening state, address and parent protection are checked here.
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        anyhow::ensure!(
            unsafe { libc::fstat(3, info.as_mut_ptr()) } == 0,
            "admin descriptor missing"
        );
        let info = unsafe { info.assume_init() };
        anyhow::ensure!(
            info.st_uid == 0 && info.st_mode & libc::S_IFMT == libc::S_IFSOCK,
            "root socket descriptor required"
        );
        let mut accepting: libc::c_int = 0;
        let mut size = std::mem::size_of_val(&accepting) as libc::socklen_t;
        anyhow::ensure!(
            unsafe {
                libc::getsockopt(
                    3,
                    libc::SOL_SOCKET,
                    libc::SO_ACCEPTCONN,
                    (&mut accepting as *mut libc::c_int).cast(),
                    &mut size,
                )
            } == 0
                && accepting == 1,
            "listening descriptor required"
        );
        anyhow::ensure!(
            unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "cannot seal inherited descriptor"
        );
        let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
        anyhow::ensure!(
            listener.local_addr()?.as_pathname() == Some(path),
            "admin descriptor address mismatch"
        );
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener: UnixListener::from_std(listener)?,
            admissions: Arc::new(Semaphore::new(8)),
        })
    }
    #[cfg(not(target_os = "linux"))]
    pub fn from_systemd(_path: &Path) -> anyhow::Result<Self> {
        anyhow::bail!("systemd host service management requires Linux")
    }
    pub(crate) async fn accept(&self) -> io::Result<(UnixStream, OwnedSemaphorePermit)> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            if let Ok(permit) = self.admissions.clone().try_acquire_owned() {
                return Ok((stream, permit));
            }
        }
    }
}
pub(crate) async fn run(state: Arc<AppState>, stream: UnixStream, _budget: OwnedSemaphorePermit) {
    let Some(policy) = state.peer_policy.get() else {
        return;
    };
    let Ok(permit) = policy.accept_root_admin(&stream) else {
        return;
    };
    let _ = serve(state, stream, permit.clone()).await;
    permit.close();
}
async fn serve(
    state: Arc<AppState>,
    stream: UnixStream,
    permit: RootAdminPermit,
) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        id: u64,
        method: String,
        params: Box<serde_json::value::RawValue>,
    }
    let (rd, mut wr) = stream.into_split();
    let mut rd = BufReader::new(rd);
    for _ in 0..16 {
        let mut bytes = Vec::new();
        let mut bounded = (&mut rd).take(MAX_ENROLLMENT_FRAME_BYTES as u64 + 1);
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            bounded.read_until(b'\n', &mut bytes),
        )
        .await
        .map_err(io::Error::other)??;
        if count == 0 {
            return Ok(());
        }
        if bytes.len() > MAX_ENROLLMENT_FRAME_BYTES || bytes.last() != Some(&b'\n') {
            return Err(io::Error::other("admin frame bound"));
        }
        let wire: Wire =
            serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid admin frame"))?;
        if wire.id == 0 {
            return Err(io::Error::other("invalid admin frame id"));
        }
        let outcome = route(&state, &permit, &wire.method, wire.params.get()).await;
        let response = match outcome {
            Ok(value) => Response::ok(wire.id, value),
            Err(error) => Response::err(wire.id, error),
        };
        let mut reply = serde_json::to_vec(&response).map_err(io::Error::other)?;
        reply.push(b'\n');
        tokio::time::timeout(std::time::Duration::from_secs(5), wr.write_all(&reply))
            .await
            .map_err(io::Error::other)??;
    }
    Ok(())
}
pub(crate) async fn route(
    state: &Arc<AppState>,
    permit: &RootAdminPermit,
    method: &str,
    args: &str,
) -> Result<serde_json::Value, RpcError> {
    permit.check()?;
    let remote = state
        .remote
        .get()
        .ok_or_else(|| RpcError::new(RpcError::FORBIDDEN, "remote management unavailable"))?;
    match method {
        "host.notificationConfigure"=>{
            state.notifications.get().ok_or_else(||RpcError::new(RpcError::CONFLICT,"notifications unavailable"))?.admin_configure(permit.clone(),decode_params(args)?).await
        }
        "host.notificationRequestLookup"=>{
            state.notifications.get().ok_or_else(||RpcError::new(RpcError::CONFLICT,"notifications unavailable"))?.admin_lookup(permit.clone(),decode_params(args)?).await
        }
        "host.status" => {
            let _: NoParams = decode_params(args)?;
            let result =
                serde_json::to_value(remote.status().await?).map_err(crate::dispatch::internal)?;
            permit.check()?;
            Ok(result)
        }
        "host.grants" => {
            let _: NoParams = decode_params(args)?;
            let result = remote
                .local_route("remote.grants", serde_json::json!({}))
                .await?;
            permit.check()?;
            Ok(result)
        }
        "host.configurationLookup" => {
            remote
                .admin_configuration_lookup(permit.clone(), decode_params(args)?)
                .await
        }
        "host.configure" => {
            remote
                .configure_with_admin(decode_params(args)?, Some(permit.clone()))
                .await
        }
        "host.enrollmentCreate" => {
            remote
                .admin_enrollment_create(permit.clone(), decode_params(args)?)
                .await
        }
        "host.enrollmentLookup" => {
            remote
                .admin_enrollment_lookup(permit.clone(), decode_params(args)?)
                .await
        }
        "host.revoke" => {
            remote
                .revoke_with_admin(decode_params(args)?, Some(permit.clone()))
                .await
        }
        "host.stop" => {
            let _: NoParams = decode_params(args)?;
            crate::lifecycle::finish_exit(state, crate::lifecycle::begin_admin_exit(state, permit)?)
                .await
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "unknown typed host management method",
        )),
    }
}

fn decode_params<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RpcError> {
    serde_json::from_str(text).map_err(|_| {
        RpcError::new(
            RpcError::INVALID_PARAMS,
            "invalid typed host management fields",
        )
    })
}
