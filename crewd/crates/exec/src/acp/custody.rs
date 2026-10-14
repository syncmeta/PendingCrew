//! Core-owned helper/native enrollment. An ACK is returned only after the actual
//! kernel birth is durably registered and the parent binding is checked again.
use super::birth::{ACK_LEN, BirthReport};
use crate::registry::{
    self, ParentProbe, ProbeResult, ProcessIdentity, ProcessParticipant, ProcessRegistry,
};
use std::{collections::BTreeMap, io, sync::Arc};

/// Created only after Core verifies the old kernel birth is gone and clears
/// its durable registry obligation. This cannot be reconstructed from RPC JSON.
#[derive(Debug)]
pub struct SettledNative {
    pub(crate) session: String,
    pub(crate) generation: String,
    pub(crate) epoch: u64,
    pub(crate) identity: ProcessIdentity,
}

/// Actual stdio proxy, enrolled only under the exact native process and group.
/// The Core keeps cleanup responsibility even if final admission fails.
#[derive(Debug)]
pub struct BridgeEnrollment {
    pub(crate) binding: super::ownership::NativeBinding,
    pub(crate) identity: ProcessIdentity,
    pub(crate) registry_key: String,
    slot: u64,
}
impl BridgeEnrollment {
    pub fn binding(&self) -> &super::ownership::NativeBinding {
        &self.binding
    }
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    pub fn registry_key(&self) -> &str {
        &self.registry_key
    }
    pub fn slot(&self) -> u64 {
        self.slot
    }
}
pub struct NativeEnrollment {
    pub epoch: u64,
    pub identity: ProcessIdentity,
    pub registry_key: String,
    acknowledgement: [u8; ACK_LEN],
}
impl NativeEnrollment {
    pub fn acknowledgement(&self) -> &[u8; ACK_LEN] {
        &self.acknowledgement
    }
}
impl std::fmt::Debug for NativeEnrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEnrollment")
            .field("epoch", &self.epoch)
            .field("identity", &self.identity)
            .field("registry_key", &self.registry_key)
            .field("acknowledgement", &"<private>")
            .finish()
    }
}
/// This object exists in crewd, never in the provider helper. Runtime state only
/// indexes the single persistent ProcessRegistry; it is not another authority file.
pub struct Custody {
    registry: Arc<ProcessRegistry>,
    session: String,
    generation: uuid::Uuid,
    nonce: [u8; 16],
    helper: ProcessIdentity,
    helper_key: String,
    last_epoch: u64,
    natives: BTreeMap<u64, ProcessIdentity>,
    closed: bool,
    bridges: BTreeMap<(u64, u64), ProcessIdentity>,
    last_bridge: u64,
}
fn same_birth(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
fn denied(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}
impl Custody {
    /// The caller supplies the PID obtained from its owned helper spawn, not RPC
    /// data. The actual helper must be this core's direct child and own its group.
    pub fn enroll_helper(
        registry: Arc<ProcessRegistry>,
        session: String,
        generation: uuid::Uuid,
        nonce: [u8; 16],
        helper_pid: i32,
    ) -> io::Result<Self> {
        if generation.is_nil() || nonce == [0; 16] {
            return Err(denied("invalid helper enrollment binding"));
        }
        let ParentProbe::Found {
            identity: helper,
            parent,
        } = registry::parent(helper_pid)
        else {
            return Err(denied("helper kernel birth unavailable"));
        };
        if parent != std::process::id() as i32 || helper.pid <= 1 || helper.pgid != helper.pid {
            return Err(denied("helper is not an owned process group"));
        }
        let helper_key = registry.record_participant(
            &session,
            ProcessParticipant::AcpHelper { generation },
            helper.clone(),
        )?;
        let custody = Self {
            registry,
            session,
            generation,
            nonce,
            helper,
            helper_key,
            last_epoch: 0,
            natives: BTreeMap::new(),
            closed: false,
            bridges: BTreeMap::new(),
            last_bridge: 0,
        };
        custody.check_helper()?;
        Ok(custody)
    }
    pub(super) fn check_helper(&self) -> io::Result<()> {
        match registry::parent(self.helper.pid) {
            ParentProbe::Found { identity, parent }
                if same_birth(&self.helper, &identity) && parent == std::process::id() as i32 =>
            {
                Ok(())
            }
            _ => Err(denied("helper birth or parent changed")),
        }
    }
    fn actual_native(&self, report: &BirthReport) -> io::Result<ProcessIdentity> {
        self.check_helper()?;
        match registry::parent(report.pid) {
            ParentProbe::Found { identity, parent }
                if parent == self.helper.pid
                    && identity.pid > 1
                    && identity.pgid == identity.pid =>
            {
                Ok(identity)
            }
            _ => Err(denied("native birth is not bound to this helper")),
        }
    }
    pub fn enroll_native(&mut self, report: &BirthReport) -> io::Result<NativeEnrollment> {
        if self.closed
            || report.nonce != self.nonce
            || self.last_epoch.checked_add(1) != Some(report.epoch)
        {
            return Err(denied("stale or foreign native enrollment"));
        }
        let identity = self.actual_native(report)?;
        if self
            .natives
            .values()
            .any(|saved| same_birth(saved, &identity))
        {
            return Err(denied("native birth already enrolled in another epoch"));
        }
        // Retain the observed physical obligation even when persistence fails.
        // The core must still stop/reap it; an error cannot erase a live birth.
        self.last_epoch = report.epoch;
        self.natives.insert(report.epoch, identity.clone());
        let registry_key = match self.registry.record_participant(
            &self.session,
            ProcessParticipant::AcpNative {
                generation: self.generation,
                epoch: report.epoch,
            },
            identity.clone(),
        ) {
            Ok(key) => key,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        let current = match self.actual_native(report) {
            Ok(identity) => identity,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        if !same_birth(&identity, &current) {
            self.closed = true;
            return Err(denied("native birth changed before ACK"));
        }
        Ok(NativeEnrollment {
            epoch: report.epoch,
            identity,
            registry_key,
            acknowledgement: report.acknowledge(),
        })
    }
    fn actual_bridge(&self, epoch: u64, pid: i32) -> io::Result<ProcessIdentity> {
        self.check_helper()?;
        let native = self
            .natives
            .get(&epoch)
            .ok_or_else(|| denied("bridge native epoch unavailable"))?;
        match registry::parent(native.pid) {
            ParentProbe::Found { identity, parent }
                if same_birth(native, &identity) && parent == self.helper.pid => {}
            _ => return Err(denied("bridge native birth or parent changed")),
        }
        match registry::parent(pid) {
            ParentProbe::Found { identity, parent }
                if parent == native.pid && identity.pid > 1 && identity.pgid == native.pgid =>
            {
                Ok(identity)
            }
            _ => Err(denied(
                "bridge is not a direct native child in its owned group",
            )),
        }
    }
    /// The private transport separately checks the socket PID/nonce. Calling
    /// code must supply that actual kernel PID, never a reported PID from JSON.
    pub fn enroll_bridge(&mut self, epoch: u64, pid: i32) -> io::Result<BridgeEnrollment> {
        if self.closed || epoch != self.last_epoch {
            return Err(denied("bridge admission closed or stale"));
        }
        let identity = self.actual_bridge(epoch, pid)?;
        if self.bridges.values().any(|old| same_birth(old, &identity)) {
            return Err(denied("bridge birth already enrolled"));
        }
        let slot = self
            .last_bridge
            .checked_add(1)
            .ok_or_else(|| denied("bridge slot exhausted"))?;
        self.last_bridge = slot;
        self.bridges.insert((epoch, slot), identity.clone());
        let registry_key = match self.registry.record_participant(
            &self.session,
            ProcessParticipant::AcpBridge {
                generation: self.generation,
                epoch,
                slot,
            },
            identity.clone(),
        ) {
            Ok(key) => key,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        let current = match self.actual_bridge(epoch, pid) {
            Ok(current) => current,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        if !same_birth(&current, &identity) {
            self.closed = true;
            return Err(denied("bridge birth changed before ACK"));
        }
        Ok(BridgeEnrollment {
            binding: self.native_binding(epoch)?,
            identity,
            registry_key,
            slot,
        })
    }
    pub(crate) fn check_bridge_nonce(&self, received: &[u8; 16]) -> io::Result<()> {
        let difference = self
            .nonce
            .iter()
            .zip(received)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        if self.closed || difference != 0 {
            return Err(denied("bridge admission closed or nonce mismatch"));
        }
        Ok(())
    }
    /// Recheck the durable enrollment and every immediate parent. A proxy
    /// cannot retain permission after its native/helper birth changes.
    pub(crate) fn check_bridge(&self, enrollment: &BridgeEnrollment) -> io::Result<()> {
        if self.closed
            || enrollment.binding.session != self.session
            || enrollment.binding.generation != self.generation.to_string()
            || enrollment.binding.epoch != self.last_epoch
        {
            return Err(denied("bridge custody is closed or stale"));
        }
        let saved = self
            .bridges
            .get(&(enrollment.binding.epoch, enrollment.slot))
            .ok_or_else(|| denied("bridge enrollment is no longer held"))?;
        if !same_birth(saved, &enrollment.identity)
            || !self
                .natives
                .get(&enrollment.binding.epoch)
                .is_some_and(|native| same_birth(native, &enrollment.binding.native))
        {
            return Err(denied("bridge enrollment binding changed"));
        }
        let current = self.actual_bridge(enrollment.binding.epoch, enrollment.identity.pid)?;
        if !same_birth(&current, &enrollment.identity) {
            return Err(denied("bridge birth changed"));
        }
        Ok(())
    }
    pub fn bridge_identities(&self) -> impl Iterator<Item = (&(u64, u64), &ProcessIdentity)> {
        self.bridges.iter()
    }
    pub fn forget_reaped_bridge(&mut self, epoch: u64, slot: u64) -> io::Result<()> {
        let identity = self
            .bridges
            .get(&(epoch, slot))
            .ok_or_else(|| denied("unknown bridge incarnation"))?;
        match registry::probe(identity.pid) {
            ProbeResult::Missing => {}
            ProbeResult::Found(actual) if !same_birth(identity, &actual) => {}
            _ => return Err(denied("bridge has not been verified reaped")),
        }
        self.registry.forget(
            &ProcessParticipant::AcpBridge {
                generation: self.generation,
                epoch,
                slot,
            }
            .key()?,
        )?;
        self.bridges.remove(&(epoch, slot));
        Ok(())
    }
    /// Reject further births immediately; actual stopping/reaping is performed
    /// by the core owner using these recorded identities and its normal guards.
    pub fn close_admission(&mut self) {
        self.closed = true;
    }
    /// Canonical Core binding; never populate a native bridge expectation from
    /// peer-supplied session/PID JSON. Enrollment has already committed this birth.
    pub fn native_binding(&self, epoch: u64) -> io::Result<super::ownership::NativeBinding> {
        let native = self
            .natives
            .get(&epoch)
            .ok_or_else(|| denied("unknown enrolled native epoch"))?;
        Ok(super::ownership::NativeBinding {
            session: self.session.clone(),
            generation: self.generation.to_string(),
            epoch,
            native: native.clone(),
        })
    }
    /// Core/helper channel binding is checked against custody's canonical
    /// launch scope, never against an expected value supplied by peer JSON.
    /// Current kernel birth, not merely the previously enrolled projection.
    pub fn current_native_binding(
        &self,
        epoch: u64,
    ) -> io::Result<super::ownership::NativeBinding> {
        if self.closed {
            return Err(denied("native admission closed"));
        }
        let binding = self.native_binding(epoch)?;
        let actual = self.actual_native(&BirthReport {
            pid: binding.native.pid,
            epoch,
            nonce: self.nonce,
        })?;
        if !same_birth(&actual, &binding.native) {
            return Err(denied("native birth changed"));
        }
        Ok(binding)
    }
    pub fn verify_helper_channel(
        &self,
        session: &str,
        generation: &str,
        nonce: &[u8; 16],
    ) -> io::Result<()> {
        self.check_helper()?;
        if session != self.session || generation != self.generation.to_string() {
            return Err(denied("foreign helper launch binding"));
        }
        self.check_bridge_nonce(nonce)
    }
    pub fn helper_identity(&self) -> &ProcessIdentity {
        &self.helper
    }
    pub fn native_identities(&self) -> impl Iterator<Item = (&u64, &ProcessIdentity)> {
        self.natives.iter()
    }
    pub fn forget_reaped_native(&mut self, epoch: u64) -> io::Result<SettledNative> {
        if self
            .bridges
            .keys()
            .any(|(native_epoch, _)| *native_epoch == epoch)
        {
            return Err(denied("native bridge custody is not settled"));
        }
        let identity = self
            .natives
            .get(&epoch)
            .ok_or_else(|| denied("unknown native incarnation"))?;
        match registry::probe(identity.pid) {
            ProbeResult::Missing => {}
            ProbeResult::Found(current) if !same_birth(identity, &current) => {}
            _ => return Err(denied("native has not been verified reaped")),
        }
        let key = ProcessParticipant::AcpNative {
            generation: self.generation,
            epoch,
        }
        .key()?;
        self.registry.forget(&key)?;
        let identity = self
            .natives
            .remove(&epoch)
            .expect("verified native retained");
        Ok(SettledNative {
            session: self.session.clone(),
            generation: self.generation.to_string(),
            epoch,
            identity,
        })
    }
    pub fn forget_reaped_helper(&mut self) -> io::Result<()> {
        if !self.closed || !self.natives.is_empty() || !self.bridges.is_empty() {
            return Err(denied("native custody is not settled"));
        }
        match registry::probe(self.helper.pid) {
            ProbeResult::Missing => {}
            ProbeResult::Found(current) if !same_birth(&self.helper, &current) => {}
            _ => return Err(denied("helper has not been verified reaped")),
        }
        self.registry.forget(&self.helper_key)
    }
}

#[cfg(test)]
mod tests {
    use super::super::birth::REPORT_LEN;
    use super::*;
    use std::{
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::{Child, Command},
    };
    struct Peer {
        child: Child,
        channel: UnixStream,
        native: Option<ProcessIdentity>,
    }
    impl Drop for Peer {
        fn drop(&mut self) {
            if let Some(native) = &self.native {
                registry::apply(
                    &registry::decide(native, &registry::probe(native.pid)),
                    native,
                );
            }
            if let ProbeResult::Found(helper) = registry::probe(self.child.id() as i32) {
                registry::apply(
                    &registry::decide(&helper, &registry::probe(helper.pid)),
                    &helper,
                );
            }
            let _ = self.child.wait();
        }
    }
    fn peer(dir: &std::path::Path, nonce: [u8; 16]) -> Peer {
        let script = dir.join("owned-acp-peer.py");
        std::fs::write(
            &script,
            r#"
import os, socket, struct, pty, termios, fcntl
control = socket.socket(fileno=int(os.environ['CONTROL_FD']))
nonce = bytes.fromhex(os.environ['BIRTH_NONCE'])
master, slave = pty.openpty()
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2): os.dup2(slave, fd)
    if slave > 2: os.close(slave)
    report = b'PCB1' + struct.pack('>iQ', os.getpid(), 1) + nonce
    control.sendall(report)
    ack = b''
    while len(ack) < 33:
        part = control.recv(33 - len(ack))
        if not part: os._exit(2)
        ack += part
    if ack != report + b'\x01': os._exit(3)
    with open(os.environ['EXEC_MARKER'], 'w') as f:
        f.write('tty' if all(os.isatty(fd) for fd in (0,1,2)) else 'not-tty')
    proxy_pid = None
    if os.environ.get('PROXY_SOCKET'):
        proxy_pid = os.fork()
        if proxy_pid == 0:
            control.close()
            proxy = socket.socket(socket.AF_UNIX)
            proxy.connect(os.environ['PROXY_SOCKET'])
            proxy.recv(1)
            os._exit(0)
    control.sendall(b'R')
    control.recv(1)
    if proxy_pid is not None: os.waitpid(proxy_pid, 0)
    os._exit(0)
os.close(slave)
control.close()
os.waitpid(pid, 0)
os.close(master)
"#,
        )
        .unwrap();
        let (channel, inherited) = UnixStream::pair().unwrap();
        channel
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let inherited_fd = inherited.as_raw_fd();
        let mut command = Command::new("/usr/bin/python3");
        command
            .arg(script)
            .env("CONTROL_FD", inherited_fd.to_string())
            .env(
                "BIRTH_NONCE",
                nonce.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            )
            .env("EXEC_MARKER", dir.join("actual-native-exec"));
        let proxy_socket = dir.join("proxy.sock");
        if proxy_socket.exists() {
            command.env("PROXY_SOCKET", proxy_socket);
        }
        // SAFETY: only async-signal-safe setsid/fcntl on an owned descriptor.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(inherited_fd, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(inherited_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop(inherited);
        Peer {
            child,
            channel,
            native: None,
        }
    }
    fn receive(peer: &mut Peer) -> BirthReport {
        let mut bytes = [0; REPORT_LEN];
        peer.channel.read_exact(&mut bytes).unwrap();
        let report = BirthReport::decode(&bytes).unwrap();
        let ProbeResult::Found(identity) = registry::probe(report.pid) else {
            panic!("actual native birth absent")
        };
        peer.native = Some(identity);
        report
    }
    #[test]
    fn actual_helper_native_births_are_durable_before_ack_and_cannot_be_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("processes.json");
        let registry = Arc::new(ProcessRegistry::load(&path).unwrap());
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let mut peer = peer(dir.path(), nonce);
        let report = receive(&mut peer);
        let mut custody = Custody::enroll_helper(
            registry.clone(),
            "worker-owned".into(),
            uuid::Uuid::new_v4(),
            nonce,
            peer.child.id() as i32,
        )
        .unwrap();
        assert!(!dir.path().join("actual-native-exec").exists());
        let before = registry.entries();
        let mut foreign = report.clone();
        foreign.nonce[0] ^= 1;
        assert!(custody.enroll_native(&foreign).is_err());
        assert_eq!(registry.entries(), before);
        let native = custody.enroll_native(&report).unwrap();
        assert_eq!(ProcessRegistry::load(&path).unwrap().entries().len(), 2);
        assert!(
            !dir.path().join("actual-native-exec").exists(),
            "native proceeded without actual ACK bytes"
        );
        assert!(custody.enroll_native(&report).is_err());
        let mut duplicate_birth = report.clone();
        duplicate_birth.epoch = 2;
        assert!(custody.enroll_native(&duplicate_birth).is_err());
        assert_eq!(registry.entries().len(), 2);
        peer.channel.write_all(native.acknowledgement()).unwrap();
        let mut ready = [0];
        peer.channel.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [b'R']);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("actual-native-exec")).unwrap(),
            "tty"
        );
        assert!(custody.forget_reaped_native(1).is_err());
        use super::super::ownership::{Ownership, Phase};
        let mut ownership = Ownership::default();
        let request = uuid::Uuid::new_v4().to_string();
        let binding = custody.native_binding(1).unwrap();
        assert!(custody.native_binding(2).is_err());
        ownership
            .prepare(
                &request,
                binding.clone(),
                "own pending input".into(),
                |_| Ok(()),
            )
            .unwrap();
        ownership.revoke_before_raw(&binding, |_| Ok(())).unwrap();
        assert!(
            ownership
                .prepare(
                    &uuid::Uuid::new_v4().to_string(),
                    binding.clone(),
                    "next".into(),
                    |_| Ok(())
                )
                .is_err()
        );
        custody.close_admission();
        assert!(custody.enroll_native(&duplicate_birth).is_err());
        assert!(custody.forget_reaped_helper().is_err());
        peer.channel.write_all(b"Q").unwrap();
        assert!(peer.child.wait().unwrap().success());
        let settled = custody.forget_reaped_native(1).unwrap();
        ownership.native_exited(&settled, |_| Ok(())).unwrap();
        assert_eq!(
            ownership.phase(&request),
            Some(Phase::Failed),
            "OS reaping is never model completion"
        );
        custody.forget_reaped_helper().unwrap();
        assert!(registry.entries().is_empty());
        assert_eq!(
            registry.authority_snapshot().unwrap().spawned.len(),
            2,
            "retirement cannot elevate managed descendants"
        );
    }
    #[test]
    fn actual_process_api_proxy_requires_durable_direct_native_custody_and_fresh_events() {
        use super::super::bridge::NativeProxyPeer;
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("processes.json");
        let registry = Arc::new(ProcessRegistry::load(&path).unwrap());
        let listener = UnixListener::bind(dir.path().join("proxy.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let mut physical = peer(dir.path(), nonce);
        let report = receive(&mut physical);
        let mut custody = Custody::enroll_helper(
            registry.clone(),
            "worker-owned".into(),
            uuid::Uuid::new_v4(),
            nonce,
            physical.child.id() as i32,
        )
        .unwrap();
        let native = custody.enroll_native(&report).unwrap();
        physical
            .channel
            .write_all(native.acknowledgement())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(error) => panic!("proxy connection absent: {error}"),
            }
        };
        let mut ready = [0];
        physical.channel.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [b'R']);
        let before = registry.entries();
        assert!(NativeProxyPeer::authenticate(&stream, &mut custody, 1, &[0; 16]).is_err());
        assert_eq!(
            registry.entries(),
            before,
            "foreign nonce cannot register a proxy"
        );
        let mut proxy = NativeProxyPeer::authenticate(&stream, &mut custody, 1, &nonce).unwrap();
        assert_eq!(ProcessRegistry::load(&path).unwrap().entries().len(), 3);
        assert_eq!(proxy.enrollment().identity().pgid, native.identity.pgid);
        assert_ne!(proxy.enrollment().identity().pid, native.identity.pid);
        assert!(NativeProxyPeer::authenticate(&stream, &mut custody, 1, &nonce).is_err());
        assert!(
            proxy
                .admit(&stream, &custody, &custody.generation.to_string(), 2, 1)
                .is_err()
        );
        assert!(
            proxy
                .admit(&stream, &custody, &custody.generation.to_string(), 1, 2)
                .is_err()
        );
        proxy
            .admit(&stream, &custody, &custody.generation.to_string(), 1, 1)
            .unwrap();
        assert!(
            proxy
                .admit(&stream, &custody, &custody.generation.to_string(), 1, 1)
                .is_err()
        );
        assert!(
            custody
                .forget_reaped_bridge(1, proxy.enrollment().slot())
                .is_err()
        );
        assert!(custody.forget_reaped_native(1).is_err());
        custody.close_admission();
        assert!(
            proxy
                .admit(&stream, &custody, &custody.generation.to_string(), 1, 2)
                .is_err()
        );
        stream.write_all(b"Q").unwrap();
        physical.channel.write_all(b"Q").unwrap();
        assert!(physical.child.wait().unwrap().success());
        // The native/helper waitpid receipts have reaped both actual descendants.
        custody
            .forget_reaped_bridge(1, proxy.enrollment().slot())
            .unwrap();
        custody.forget_reaped_native(1).unwrap();
        custody.forget_reaped_helper().unwrap();
        assert!(registry.entries().is_empty());
        assert_eq!(registry.authority_snapshot().unwrap().spawned.len(), 3);
    }
    #[test]
    fn real_persistence_failure_returns_no_ack_and_retains_stop_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("processes.json");
        let registry = Arc::new(ProcessRegistry::load(&path).unwrap());
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let mut peer = peer(dir.path(), nonce);
        let report = receive(&mut peer);
        let mut custody = Custody::enroll_helper(
            registry.clone(),
            "worker-owned".into(),
            uuid::Uuid::new_v4(),
            nonce,
            peer.child.id() as i32,
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(custody.enroll_native(&report).is_err());
        assert_eq!(custody.native_identities().count(), 1);
        assert!(custody.closed);
        assert!(!dir.path().join("actual-native-exec").exists());
        peer.channel.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(peer.child.wait().unwrap().success());
        assert!(matches!(registry::probe(report.pid), ProbeResult::Missing));
        std::fs::remove_dir(&path).unwrap();
        custody.forget_reaped_native(1).unwrap();
        custody.forget_reaped_helper().unwrap();
        assert!(registry.entries().is_empty());
    }
}
