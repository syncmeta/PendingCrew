//! Private native Mod bridge admission. Same UID is insufficient: the connected
//! process must be the exact Core-enrolled native birth, with its private nonce.
//! This is not a public RPC identity or a second provider/work authority.
use super::ownership::NativeBinding;
use crate::registry::{self, ProbeResult, ProcessIdentity};
use std::{io, os::fd::AsRawFd, os::unix::net::UnixStream};

pub struct NativePeer {
    binding: NativeBinding,
    next_sequence: u64,
}
impl std::fmt::Debug for NativePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePeer")
            .field("binding", &self.binding)
            .field("next_sequence", &self.next_sequence)
            .finish()
    }
}
fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn same_birth(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: exact owned socket and correctly sized writable pid_t buffer.
    // SOL_LOCAL/LOCAL_PEERPID are the same SDK constants used by crewd admission.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            0x002,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of_val(&pid) || pid <= 1 {
        return Err(denied("native peer PID unavailable"));
    }
    Ok(pid)
}
#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: correctly sized ucred buffer, initialized only on exact successful result.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(denied("native peer credentials unavailable"));
    }
    // SAFETY: the successful call above wrote the complete structure.
    let credentials = unsafe { credentials.assume_init() };
    if credentials.pid <= 1 {
        return Err(denied("native peer PID unavailable"));
    }
    Ok(credentials.pid)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_pid(_: &UnixStream) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native bridge kernel identity unavailable",
    ))
}
fn verify_birth(stream: &UnixStream, binding: &NativeBinding) -> io::Result<()> {
    if peer_pid(stream)? != binding.native.pid {
        return Err(denied("foreign native socket peer"));
    }
    match registry::probe(binding.native.pid) {
        ProbeResult::Found(actual) if same_birth(&actual, &binding.native) => Ok(()),
        _ => Err(denied("native socket birth changed or unknown")),
    }
}
impl NativePeer {
    /// Core supplies its custody binding and never takes it from peer JSON. A
    /// native Mod must connect from the native process itself; an unregistered
    /// external hook/helper proxy is unsupported, even if it has the secret.
    pub fn authenticate(
        stream: &UnixStream,
        binding: NativeBinding,
        expected_nonce: &[u8; 16],
        received_nonce: &[u8; 16],
    ) -> io::Result<Self> {
        let difference = expected_nonce
            .iter()
            .zip(received_nonce)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if expected_nonce == &[0; 16] || difference != 0 {
            return Err(denied("native bridge nonce mismatch"));
        }
        verify_birth(stream, &binding)?;
        Ok(Self {
            binding,
            next_sequence: 1,
        })
    }
    pub fn binding(&self) -> &NativeBinding {
        &self.binding
    }
    /// Every event rechecks actual kernel identity. A gap/duplicate/foreign
    /// epoch has no sequence or authority effect, and callers must not dispatch
    /// it into ownership state. Sequence exhaustion requires a fresh bridge.
    pub fn admit(
        &mut self,
        stream: &UnixStream,
        generation: &str,
        epoch: u64,
        sequence: u64,
    ) -> io::Result<()> {
        if generation != self.binding.generation
            || epoch != self.binding.epoch
            || sequence != self.next_sequence
        {
            return Err(denied("stale or out-of-order native event"));
        }
        let next = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| denied("native event sequence exhausted"))?;
        verify_birth(stream, &self.binding)?;
        self.next_sequence = next;
        Ok(())
    }
}

/// A process-API stdio proxy has a distinct kernel peer. It is admitted only
/// after Core persists its direct-native-child enrollment. No JSON PID,
/// generic ancestor, or same-UID fallback is accepted here.
pub struct NativeProxyPeer {
    enrollment: super::custody::BridgeEnrollment,
    next_sequence: u64,
}
impl std::fmt::Debug for NativeProxyPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeProxyPeer")
            .field("enrollment", &self.enrollment)
            .field("next_sequence", &self.next_sequence)
            .finish()
    }
}
impl NativeProxyPeer {
    pub fn authenticate(
        stream: &UnixStream,
        custody: &mut super::custody::Custody,
        epoch: u64,
        received_nonce: &[u8; 16],
    ) -> io::Result<Self> {
        custody.check_bridge_nonce(received_nonce)?;
        let enrollment = custody.enroll_bridge(epoch, peer_pid(stream)?)?;
        custody.check_bridge(&enrollment)?;
        Ok(Self {
            enrollment,
            next_sequence: 1,
        })
    }
    pub fn binding(&self) -> &NativeBinding {
        self.enrollment.binding()
    }
    pub fn enrollment(&self) -> &super::custody::BridgeEnrollment {
        &self.enrollment
    }
    pub fn admit(
        &mut self,
        stream: &UnixStream,
        custody: &super::custody::Custody,
        generation: &str,
        epoch: u64,
        sequence: u64,
    ) -> io::Result<()> {
        if generation != self.enrollment.binding().generation
            || epoch != self.enrollment.binding().epoch
            || sequence != self.next_sequence
        {
            return Err(denied("stale or out-of-order native proxy event"));
        }
        let next = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| denied("native proxy sequence exhausted"))?;
        if peer_pid(stream)? != self.enrollment.identity().pid {
            return Err(denied("foreign native proxy socket peer"));
        }
        custody.check_bridge(&self.enrollment)?;
        self.next_sequence = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::{net::UnixListener, process::CommandExt},
        process::{Child, Command},
    };
    struct ChildPeer {
        child: Child,
        stream: UnixStream,
        binding: NativeBinding,
    }
    impl Drop for ChildPeer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    fn child_peer(path: &std::path::Path) -> ChildPeer {
        let listener = UnixListener::bind(path).unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.arg("-c").arg("import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.recv(1)").arg(path).env_clear();
        // SAFETY: setsid is async-signal-safe and touches no Rust state after fork.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn().unwrap();
        let (stream, _) = listener.accept().unwrap();
        let ProbeResult::Found(native) = registry::probe(child.id() as i32) else {
            panic!("real child kernel identity absent")
        };
        let binding = NativeBinding {
            session: "core-owned-session".into(),
            generation: uuid::Uuid::new_v4().to_string(),
            epoch: 1,
            native,
        };
        ChildPeer {
            child,
            stream,
            binding,
        }
    }
    #[test]
    fn real_kernel_peer_cannot_be_replaced_by_nonce_uid_or_claimed_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = child_peer(&dir.path().join("owner.sock"));
        let foreign = child_peer(&dir.path().join("foreign.sock"));
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        assert!(
            NativePeer::authenticate(&foreign.stream, owner.binding.clone(), &nonce, &nonce)
                .is_err()
        );
        assert!(
            NativePeer::authenticate(&owner.stream, owner.binding.clone(), &nonce, &[0; 16])
                .is_err()
        );
        let mut peer =
            NativePeer::authenticate(&owner.stream, owner.binding.clone(), &nonce, &nonce).unwrap();
        assert!(
            peer.admit(&owner.stream, &owner.binding.generation, 2, 1)
                .is_err()
        );
        assert!(
            peer.admit(&owner.stream, &foreign.binding.generation, 1, 1)
                .is_err()
        );
        assert!(
            peer.admit(&owner.stream, &owner.binding.generation, 1, 2)
                .is_err()
        );
        peer.admit(&owner.stream, &owner.binding.generation, 1, 1)
            .unwrap();
        assert!(
            peer.admit(&owner.stream, &owner.binding.generation, 1, 1)
                .is_err()
        );
        assert!(
            peer.admit(&foreign.stream, &owner.binding.generation, 1, 2)
                .is_err()
        );
        peer.admit(&owner.stream, &owner.binding.generation, 1, 2)
            .unwrap();
        owner.stream.write_all(b"Q").unwrap();
        assert!(owner.child.wait().unwrap().success());
        assert!(
            peer.admit(&owner.stream, &owner.binding.generation, 1, 3)
                .is_err()
        );
    }
}
