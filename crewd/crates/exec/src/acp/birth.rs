//! Native pre-exec custody barrier. Control carries no prompt, credential or terminal data.
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
pub const REPORT_LEN: usize = 32;
pub const ACK_LEN: usize = 33;
const MAGIC: &[u8; 4] = b"PCB1";
#[derive(Clone)]
pub struct BirthGate {
    fd: Arc<OwnedFd>,
    nonce: [u8; 16],
    next: Arc<AtomicU64>,
    timeout_ms: i32,
}
impl std::fmt::Debug for BirthGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BirthGate")
            .field("nonce", &"<private>")
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct BirthReport {
    pub pid: i32,
    pub epoch: u64,
    pub nonce: [u8; 16],
}
impl std::fmt::Debug for BirthReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BirthReport")
            .field("pid", &self.pid)
            .field("epoch", &self.epoch)
            .field("nonce", &"<private>")
            .finish()
    }
}
impl BirthReport {
    pub fn decode(bytes: &[u8; REPORT_LEN]) -> io::Result<Self> {
        if &bytes[..4] != MAGIC {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let pid = i32::from_be_bytes(bytes[4..8].try_into().expect("fixed pid"));
        let epoch = u64::from_be_bytes(bytes[8..16].try_into().expect("fixed epoch"));
        if pid <= 1 || epoch == 0 {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        Ok(Self {
            pid,
            epoch,
            nonce: bytes[16..32].try_into().expect("fixed nonce"),
        })
    }
    pub fn acknowledge(&self) -> [u8; ACK_LEN] {
        let mut b = [0; ACK_LEN];
        b[..4].copy_from_slice(MAGIC);
        b[4..8].copy_from_slice(&self.pid.to_be_bytes());
        b[8..16].copy_from_slice(&self.epoch.to_be_bytes());
        b[16..32].copy_from_slice(&self.nonce);
        b[32] = 1;
        b
    }
}
impl BirthGate {
    /// The inherited descriptor is private, close-on-exec, and connected before a child is spawned.
    pub fn new(fd: OwnedFd, nonce: [u8; 16], timeout: Duration) -> io::Result<Self> {
        let ms = i32::try_from(timeout.as_millis())
            .ok()
            .filter(|n| *n > 0 && *n <= 30_000)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        // SAFETY: query/set descriptor flags on our owned live descriptor.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            fd: Arc::new(fd),
            nonce,
            next: Arc::new(AtomicU64::new(0)),
            timeout_ms: ms,
        })
    }
    /// Read only after spawn_pty_gated returned: its exact pre-exec Core ACK
    /// has completed before this epoch may label native output.
    pub(crate) fn acknowledged_epoch(&self) -> u64 {
        self.next.load(Ordering::Acquire)
    }
    pub(crate) fn prepare(&self) -> io::Result<PreparedBirth> {
        let old = self
            .next
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        Ok(PreparedBirth {
            fd: self.fd.clone(),
            nonce: self.nonce,
            epoch: old + 1,
            timeout_ms: self.timeout_ms,
        })
    }
}
pub(crate) struct PreparedBirth {
    fd: Arc<OwnedFd>,
    nonce: [u8; 16],
    epoch: u64,
    timeout_ms: i32,
}
impl PreparedBirth {
    /// Only raw async-signal-safe syscalls and fixed stack buffers. Called after setsid/CTTY.
    pub(crate) fn wait_before_exec(&self) -> io::Result<()> {
        let mut report = [0; REPORT_LEN];
        report[..4].copy_from_slice(MAGIC);
        // SAFETY: getpid has no preconditions and is async-signal-safe.
        let pid = unsafe { libc::getpid() };
        report[4..8].copy_from_slice(&pid.to_be_bytes());
        report[8..16].copy_from_slice(&self.epoch.to_be_bytes());
        report[16..].copy_from_slice(&self.nonce);
        let fd = self.fd.as_raw_fd();
        let deadline = monotonic_ms()?.saturating_add(self.timeout_ms as i64);
        transfer(fd, &mut report, false, deadline)?;
        let mut ack = [0; ACK_LEN];
        transfer(fd, &mut ack, true, deadline)?;
        if ack[..REPORT_LEN] != report || ack[REPORT_LEN] != 1 {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        Ok(())
    }
}
fn monotonic_ms() -> io::Result<i64> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime is async-signal-safe and fills a valid stack timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(t.tv_sec
        .saturating_mul(1000)
        .saturating_add(t.tv_nsec / 1_000_000))
}
fn transfer(fd: i32, buffer: &mut [u8], read: bool, deadline: i64) -> io::Result<()> {
    let mut offset = 0;
    let mut waits = 0;
    while offset < buffer.len() {
        let remaining = deadline.saturating_sub(monotonic_ms()?);
        if remaining <= 0 {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        let timeout_ms = i32::try_from(remaining).unwrap_or(i32::MAX);
        let mut poll = libc::pollfd {
            fd,
            events: if read { libc::POLLIN } else { libc::POLLOUT },
            revents: 0,
        };
        // SAFETY: one valid pollfd and live owned fd; these calls are safe between fork and exec.
        let result = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
        if result == 0 {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        if result < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) && waits < 16 {
                waits += 1;
                continue;
            }
            return Err(e);
        }
        // SAFETY: remaining contiguous initialized bytes are valid for the selected operation.
        let n = unsafe {
            if read {
                libc::read(
                    fd,
                    buffer[offset..].as_mut_ptr().cast(),
                    buffer.len() - offset,
                )
            } else {
                libc::write(fd, buffer[offset..].as_ptr().cast(), buffer.len() - offset)
            }
        };
        if n == 0 {
            return Err(io::Error::from_raw_os_error(libc::EPIPE));
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if matches!(e.raw_os_error(), Some(libc::EINTR) | Some(libc::EAGAIN)) && waits < 16 {
                waits += 1;
                continue;
            }
            return Err(e);
        }
        offset += n as usize;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::{SpawnSpec, spawn_pty_gated};
    use crate::registry::{self, ParentProbe, ProbeResult};
    use std::collections::BTreeMap;
    use std::os::unix::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn spec(marker: &std::path::Path) -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "test -t 0 && test -t 1 && test -t 2 && test -r /dev/tty && printf 'TTY-OK\\n'; touch '{}'; stty size",
                    marker.display()
                ),
            ],
            env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
            cwd: std::env::temp_dir(),
            cols: 80,
            lines: 25,
        }
    }
    async fn case(ack: Option<bool>, timeout: Duration) {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("native-exec");
        let s = spec(&marker);
        let (p, c) = UnixStream::pair().unwrap();
        p.set_nonblocking(true).unwrap();
        let mut peer = tokio::net::UnixStream::from_std(p).unwrap();
        let gate = BirthGate::new(c.into(), [7; 16], timeout).unwrap();
        let runtime = tokio::runtime::Handle::current();
        let task = tokio::task::spawn_blocking(move || {
            let _entered = runtime.enter();
            spawn_pty_gated(&s, Some(&gate))
        });
        let mut packet = [0; REPORT_LEN];
        tokio::time::timeout(Duration::from_secs(5), peer.read_exact(&mut packet))
            .await
            .unwrap()
            .unwrap();
        let report = BirthReport::decode(&packet).unwrap();
        assert!(
            !marker.exists(),
            "native executable ran before kernel birth enrollment"
        );
        let actual = match registry::probe(report.pid) {
            ProbeResult::Found(v) => v,
            v => panic!("actual kernel birth {v:?}"),
        };
        assert_eq!(actual.pgid, report.pid);
        assert!(
            matches!(registry::parent(report.pid),ParentProbe::Found{parent,..} if parent==std::process::id() as i32)
        );
        if let Some(valid) = ack {
            let mut a = report.acknowledge();
            if !valid {
                a[8] ^= 1;
            }
            peer.write_all(&a).await.unwrap();
        } else if timeout > Duration::from_millis(100) {
            drop(peer);
        }
        let spawned = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        if ack == Some(true) {
            let mut p = spawned.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 1024];
            loop {
                let n = tokio::time::timeout(Duration::from_secs(5), p.master.read(&mut buf))
                    .await
                    .unwrap()
                    .unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            assert!(p.child.wait().await.unwrap().success());
            let output = String::from_utf8_lossy(&bytes);
            assert!(
                output.contains("TTY-OK") && output.contains("25 80"),
                "{output}"
            );
            assert!(marker.exists());
        } else {
            assert!(
                spawned.is_err(),
                "refused/closed/timed-out enrollment must fail before exec"
            );
            assert!(!marker.exists());
            assert!(
                matches!(registry::probe(report.pid), ProbeResult::Missing),
                "pre-exec failure left a native orphan"
            );
        }
    }
    #[tokio::test]
    async fn actual_kernel_birth_ack_then_exec_has_controlling_tty() {
        case(Some(true), Duration::from_secs(2)).await
    }
    #[tokio::test]
    async fn stale_epoch_ack_never_executes_native() {
        case(Some(false), Duration::from_secs(2)).await
    }
    #[tokio::test]
    async fn parent_disconnect_before_enrollment_leaves_no_exec_or_orphan() {
        case(None, Duration::from_secs(2)).await
    }
    #[tokio::test]
    async fn enrollment_timeout_leaves_no_exec_or_orphan() {
        case(None, Duration::from_millis(60)).await
    }
}
