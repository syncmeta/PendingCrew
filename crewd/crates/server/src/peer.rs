//! Kernel local caller authority. JSON identity and session tokens never confer Human authority.
use crewd_exec::registry::{self, ParentProbe, ProcessIdentity, ProcessRegistry};
use crewd_proto::{RpcError, methods};
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, Mutex},
};
use tokio::net::UnixStream;

fn same_birth(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagedParentState {
    Alive,
    Gone,
    Unknown,
}

fn managed_parent_state(
    parent: &ProcessIdentity,
    daemon: &ProcessIdentity,
    mut probe: impl FnMut(i32) -> ParentProbe,
) -> ManagedParentState {
    match probe(parent.pid) {
        ParentProbe::Missing => ManagedParentState::Gone,
        ParentProbe::Found { identity, .. } if !same_birth(parent, &identity) => {
            ManagedParentState::Gone
        }
        ParentProbe::Unreadable(_) => ManagedParentState::Unknown,
        ParentProbe::Found { .. } => match probe(daemon.pid) {
            ParentProbe::Found {
                identity,
                parent: pid,
            } if same_birth(daemon, &identity) && pid == parent.pid => ManagedParentState::Alive,
            _ => ManagedParentState::Unknown,
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Authority {
    Human,
    Managed { session: String },
    Unknown { reason: &'static str },
}
pub(crate) struct PeerPolicy {
    roots: Option<Arc<ProcessRegistry>>,
    // Captured from our own kernel lineage, not from a client-provided name.
    human_anchors: Vec<ProcessIdentity>,
    trusted_clients: std::sync::RwLock<Vec<crewd_store::local_clients::TrustedClient>>,
    trusted_connections:
        Mutex<BTreeMap<String, (String, crate::connection_scope::ConnectionScope)>>,
    // Exact daemon process captured before any driver spawn, never a parent/common ancestor.
    daemon_birth: ProcessIdentity,
    owner_uid: u32,
    // Runtime-only restrictive evidence; never a source of Human authority.
    managed_peers: Mutex<BTreeMap<(i32, u64, String), String>>,
}
#[derive(Clone)]
pub(crate) struct ConnectionPeer {
    pub(crate) policy: Arc<PeerPolicy>,
    identity: Option<ProcessIdentity>,
    kernel_uid: u32,
    pub(crate) connection_scope: crate::connection_scope::ConnectionScope,
}
/// A separate sealed service-management capability. Never converts to Actor/Human.
#[derive(Clone)]
pub(crate) struct RootAdminPermit(ConnectionPeer);
impl RootAdminPermit {
    pub(crate) fn check(&self) -> Result<(), RpcError> {
        let peer = &self.0;
        let Some(identity) = peer.identity.as_ref() else {
            return Err(root_admin_denied());
        };
        if peer.kernel_uid != 0
            || !peer.connection_scope.is_live()
            || !matches!(registry::parent(identity.pid),ParentProbe::Found{identity:now,..} if same_birth(identity,&now))
            || !peer.signed_lineage_current(identity)
        {
            return Err(root_admin_denied());
        }
        Ok(())
    }
    pub(crate) fn close(&self) {
        let _ = self
            .0
            .connection_scope
            .close_until(std::time::Instant::now() + std::time::Duration::from_secs(2));
    }
}
fn root_admin_denied() -> RpcError {
    RpcError::new(
        RpcError::FORBIDDEN,
        "current root service-management peer required",
    )
    .with_refusal("host.admin_peer_denied")
}
impl PeerPolicy {
    pub(crate) fn new(roots: Option<Arc<ProcessRegistry>>) -> io::Result<Self> {
        let self_pid = i32::try_from(std::process::id()).map_err(io::Error::other)?;
        let ParentProbe::Found { identity, parent } = registry::parent(self_pid) else {
            return Err(io::Error::other("cannot establish daemon process birth"));
        };
        let daemon_birth = identity.clone();
        let mut human_anchors = vec![identity];
        // An init/launchd parent is not an authenticated human client. In that case
        // only the same-process library client is qualified until an explicit capability exists.
        if parent > 1
            && let ParentProbe::Found { identity, .. } = registry::parent(parent)
        {
            human_anchors.push(identity);
        }
        // SAFETY: getuid has no pointer arguments and reads the daemon owning uid.
        let owner_uid = unsafe { libc::getuid() };
        Ok(Self {
            roots,
            human_anchors,
            trusted_clients: Default::default(),
            trusted_connections: Default::default(),
            daemon_birth,
            owner_uid,
            managed_peers: Mutex::new(BTreeMap::new()),
        })
    }
    pub(crate) fn has_managed_parent(&self) -> bool {
        self.human_anchors.get(1).is_some()
    }

    /// Optional Mac helper lifetime only. Caller JSON and socket ownership never
    /// choose the monitored process; it is the original kernel-captured parent.
    pub(crate) async fn wait_for_managed_parent_exit(
        &self,
        admission: &crate::lifecycle::Admission,
    ) {
        let Some(parent) = self.human_anchors.get(1) else {
            admission.close();
            std::future::pending::<()>().await;
            return;
        };
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut warned = false;
        loop {
            tick.tick().await;
            match managed_parent_state(parent, &self.daemon_birth, registry::parent) {
                ManagedParentState::Alive => {}
                ManagedParentState::Gone => {
                    admission.close();
                    return;
                }
                ManagedParentState::Unknown => {
                    admission.close();
                    if !warned {
                        eprintln!("crewd: managed parent identity unavailable; new execution held");
                        warned = true;
                    }
                }
            }
        }
    }

    /// A driver observation becomes public physical evidence only when the
    /// exact current kernel child matches this session's spawn enrollment.
    pub(crate) fn owned_runtime_birth(&self, session: &str, pid: u32) -> Option<ProcessIdentity> {
        let pid = i32::try_from(pid).ok()?;
        let ParentProbe::Found { identity, parent } = registry::parent(pid) else {
            return None;
        };
        let roots = self.roots.as_ref()?;
        let snapshot = roots.authority_snapshot().ok()?;
        let owns = |birth: &ProcessIdentity| {
            snapshot
                .spawned
                .iter()
                .any(|(owner, recorded)| owner == session && same_birth(recorded, birth))
        };
        if !owns(&identity) {
            return None;
        }
        let daemon_pid = if parent == self.daemon_birth.pid {
            parent
        } else {
            // The native process may be the exact enrolled helper's child,
            // never an arbitrary same-UID descendant or JSON-claimed PID.
            let ParentProbe::Found {
                identity: helper,
                parent: core,
            } = registry::parent(parent)
            else {
                return None;
            };
            if core != self.daemon_birth.pid
                || helper.pgid != helper.pid
                || identity.pgid != identity.pid
                || !owns(&helper)
            {
                return None;
            }
            let entries = roots.try_entries().ok()?;
            let native_key = entries.iter().find_map(|(key, birth)| {
                if !same_birth(birth, &identity) {
                    return None;
                }
                let parts = key.split('/').collect::<Vec<_>>();
                if parts.len() != 4 || parts[0] != "@acp" || parts[2] != "native" {
                    return None;
                }
                let generation = uuid::Uuid::parse_str(parts[1]).ok()?;
                let epoch = parts[3].parse::<u64>().ok()?;
                (!generation.is_nil() && generation.to_string() == parts[1] && epoch > 0)
                    .then(|| format!("@acp/{generation}/helper"))
            })?;
            if !entries
                .iter()
                .any(|(key, birth)| key == &native_key && same_birth(birth, &helper))
            {
                return None;
            }
            core
        };
        let ParentProbe::Found {
            identity: daemon, ..
        } = registry::parent(daemon_pid)
        else {
            return None;
        };
        same_birth(&daemon, &self.daemon_birth).then_some(identity)
    }

    pub(crate) fn accept_root_admin(
        self: &Arc<Self>,
        stream: &UnixStream,
    ) -> io::Result<RootAdminPermit> {
        let credential = stream.peer_cred()?;
        if credential.uid() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "root service-management peer required",
            ));
        }
        let pid = kernel_pid(stream)?;
        let ParentProbe::Found { identity, .. } = registry::parent(pid) else {
            return Err(io::Error::other("root peer birth unavailable"));
        };
        let permit = RootAdminPermit(ConnectionPeer {
            policy: self.clone(),
            identity: Some(identity),
            kernel_uid: credential.uid(),
            connection_scope: crate::connection_scope::ConnectionScope::new(),
        });
        permit.check().map_err(|_| {
            io::Error::new(io::ErrorKind::PermissionDenied, "root peer lineage refused")
        })?;
        Ok(permit)
    }
    pub(crate) fn accept(self: &Arc<Self>, stream: &UnixStream) -> io::Result<ConnectionPeer> {
        let cred = stream.peer_cred()?;
        let kernel_uid = cred.uid();
        if kernel_uid != self.owner_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer UID differs from daemon owner",
            ));
        }
        let pid = kernel_pid(stream)?;
        let identity = match registry::parent(pid) {
            ParentProbe::Found { identity, .. } => Some(identity),
            ParentProbe::Missing | ParentProbe::Unreadable(_) => None,
        };
        Ok(ConnectionPeer {
            policy: self.clone(),
            identity,
            kernel_uid,
            connection_scope: crate::connection_scope::ConnectionScope::new(),
        })
    }
}
impl PeerPolicy {
    fn retained_managed(
        &self,
        peer: &ProcessIdentity,
        probe: impl FnOnce(i32) -> ParentProbe,
    ) -> Option<Authority> {
        let key = (peer.pid, peer.start_time, peer.boot_id.clone());
        let session = match self.managed_peers.lock() {
            Ok(seen) => seen.get(&key).cloned(),
            Err(_) => {
                return Some(Authority::Unknown {
                    reason: "managed_history_unavailable",
                });
            }
        }?;
        // Release history lock before an actual kernel query. A PID-only match is never enough.
        Some(match probe(peer.pid) {
            ParentProbe::Found { identity, .. } if same_birth(peer, &identity) => {
                Authority::Managed { session }
            }
            _ => Authority::Unknown {
                reason: "retained_peer_birth_unavailable_or_changed",
            },
        })
    }
    fn remember_managed(
        &self,
        peer: &ProcessIdentity,
        authority: Authority,
        probe: impl FnOnce(i32) -> ParentProbe,
    ) -> Authority {
        let Authority::Managed { session } = authority else {
            return authority;
        };
        if !matches!(probe(peer.pid),ParentProbe::Found {identity,..} if same_birth(peer,&identity))
        {
            return Authority::Unknown {
                reason: "peer_birth_changed_before_managed_record",
            };
        }
        match self.managed_peers.lock() {
            Ok(mut seen) => {
                seen.entry((peer.pid, peer.start_time, peer.boot_id.clone()))
                    .or_insert(session.clone());
                Authority::Managed { session }
            }
            Err(_) => Authority::Unknown {
                reason: "managed_history_unavailable",
            },
        }
    }
}
impl PeerPolicy {
    pub(crate) fn replace_trusted_clients(
        &self,
        pins: Vec<crewd_store::local_clients::TrustedClient>,
    ) -> io::Result<()> {
        let live_ids = pins.iter().map(|p| p.id.clone()).collect::<BTreeSet<_>>();
        *self
            .trusted_clients
            .write()
            .map_err(|_| io::Error::other("local trust policy unavailable"))? = pins;
        let mut connections = self
            .trusted_connections
            .lock()
            .map_err(|_| io::Error::other("local trusted connection registry unavailable"))?;
        connections.retain(|_, (_, scope)| scope.is_live());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut first_error = None;
        for (_, (trust, scope)) in connections.iter() {
            if !live_ids.contains(trust)
                && let Err(error) = scope.close_until(deadline)
            {
                first_error.get_or_insert(error);
            }
        }
        connections.retain(|_, (_, scope)| scope.is_live());
        first_error.map_or(Ok(()), Err)
    }
}
impl ConnectionPeer {
    pub(crate) fn is_human_anchor(&self) -> bool {
        matches!(self.lineage_authority(), Authority::Human)
    }
    pub(crate) fn grant_frontend(
        &self,
        p: &crewd_proto::commands::LocalClientTrustParams,
        db: &str,
        at: &str,
    ) -> io::Result<crewd_store::local_clients::TrustedClient> {
        if !self.is_human_anchor() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "old trusted Human anchor required",
            ));
        }
        let pid = p
            .client_pid
            .map(i32::try_from)
            .transpose()
            .map_err(io::Error::other)?
            .or(self.identity.as_ref().map(|p| p.pid))
            .ok_or_else(|| io::Error::other("kernel caller unavailable"))?;
        let ParentProbe::Found { identity, .. } = registry::parent(pid) else {
            return Err(io::Error::other("frontend birth unavailable"));
        };
        #[cfg(target_os = "macos")]
        {
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
            let n = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            if n != size || unsafe { info.assume_init() }.pbi_uid != self.kernel_uid {
                return Err(io::Error::other(
                    "frontend kernel UID unavailable or foreign",
                ));
            }
        }
        let mode = match p.mode {
            crewd_proto::commands::LocalClientTrustMode::ExactCode => "exactCode",
            crewd_proto::commands::LocalClientTrustMode::Publisher => "publisher",
        };
        let trust = crate::signed_client::grant(
            &identity,
            mode,
            &p.expected_cd_hash,
            p.expected_team.as_deref(),
            db,
            at,
        )?;
        if !self.is_human_anchor() {
            return Err(io::Error::other("bootstrap Human anchor changed"));
        }
        Ok(trust)
    }
    /// Owned-process shutdown requires the actual IPC caller to be the daemon's
    /// current direct parent with both kernel births unchanged. A same-UID peer,
    /// JSON PID, path or old connection does not establish this association.
    pub(crate) fn owns_daemon_child(&self) -> bool {
        let Some(peer) = &self.identity else {
            return false;
        };
        if !matches!(self.authority(), Authority::Human) {
            return false;
        }
        let ParentProbe::Found {
            identity: daemon,
            parent,
        } = registry::parent(self.policy.daemon_birth.pid)
        else {
            return false;
        };
        if !same_birth(&daemon, &self.policy.daemon_birth) || parent != peer.pid {
            return false;
        }
        matches!(registry::parent(parent), ParentProbe::Found { identity, .. } if same_birth(peer, &identity))
    }
    pub(crate) fn local_trust_status(
        &self,
        pins: &[crewd_store::local_clients::TrustedClient],
    ) -> Result<crewd_proto::commands::LocalClientTrustStatus, RpcError> {
        self.command_principal()?;
        let bootstrap = self.is_human_anchor();
        let trust_id = self.identity.as_ref().and_then(|peer| {
            pins.iter()
                .find(|pin| {
                    pin.mode == "exactCode"
                        && self.signed_lineage_current(peer)
                        && crate::signed_client::verify(peer, pin).is_ok()
                        && self.signed_lineage_current(peer)
                })
                .map(|pin| pin.id.clone())
        });
        self.command_principal()?;
        Ok(crewd_proto::commands::LocalClientTrustStatus {
            valid_exact_code: trust_id.is_some(),
            bootstrap_allowed: bootstrap && self.is_human_anchor(),
            trust_id,
        })
    }

    pub(crate) fn command_principal(&self) -> Result<String, RpcError> {
        if !matches!(self.authority(), Authority::Human) {
            return Err(RpcError::new(
                RpcError::FORBIDDEN,
                "public commands require trusted Human IPC",
            ));
        }
        Ok(format!("local-human:{}", self.policy.owner_uid))
    }
    // Revalidate before every parsed request. No await or database access while any registry lock is held.
    pub(crate) fn authority(&self) -> Authority {
        if !self.connection_scope.is_live() {
            return Authority::Unknown {
                reason: "original_connection_closed",
            };
        }
        let lineage = self.lineage_authority();
        // Registry/enrollment/read failures and every known managed ancestor
        // dominate code signing. Only a stable unrelated lineage may re-admit.
        if !matches!(
            lineage,
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage"
            }
        ) {
            return lineage;
        }
        let Some(peer) = self.identity.as_ref() else {
            return lineage;
        };
        let Ok(pins) = self.policy.trusted_clients.read() else {
            return lineage;
        };
        for pin in pins.iter() {
            if self.signed_lineage_current(peer)
                && crate::signed_client::verify(peer, pin).is_ok()
                && self.signed_lineage_current(peer)
            {
                let Ok(mut connections) = self.policy.trusted_connections.lock() else {
                    return Authority::Unknown {
                        reason: "trusted_client_registry_unavailable",
                    };
                };
                connections.retain(|_, (_, scope)| scope.is_live());
                if connections.len() >= 256 && !connections.contains_key(&self.connection_scope.id)
                {
                    return Authority::Unknown {
                        reason: "trusted_client_capacity",
                    };
                }
                connections.insert(
                    self.connection_scope.id.clone(),
                    (pin.id.clone(), self.connection_scope.clone()),
                );
                return Authority::Human;
            }
        }
        lineage
    }
    fn signed_lineage_current(&self, peer: &ProcessIdentity) -> bool {
        let roots = match &self.policy.roots {
            Some(r) => match r.authority_snapshot() {
                Ok(s) => s,
                Err(_) => return false,
            },
            None => Default::default(),
        };
        if roots.enrolling != 0
            || self
                .policy
                .retained_managed(peer, registry::parent)
                .is_some()
        {
            return false;
        }
        let mut captured = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pid = peer.pid;
        for _ in 0..64 {
            if !seen.insert(pid) {
                return false;
            }
            let ParentProbe::Found { identity, parent } = registry::parent(pid) else {
                return false;
            };
            if pid == peer.pid && !same_birth(peer, &identity) {
                return false;
            }
            if roots
                .spawned
                .iter()
                .any(|(_, birth)| same_birth(birth, &identity))
            {
                return false;
            }
            captured.push((identity, parent));
            if parent <= 1 {
                return captured.iter().all(|(old,p)|matches!(registry::parent(old.pid),ParentProbe::Found{identity,parent} if same_birth(old,&identity)&&parent==*p))
                    && match &self.policy.roots{Some(r)=>r.authority_snapshot().is_ok_and(|s|s.revision==roots.revision&&s.enrolling==0),None=>true};
            }
            pid = parent;
        }
        false
    }
    fn lineage_authority(&self) -> Authority {
        if self.kernel_uid != self.policy.owner_uid {
            return Authority::Unknown {
                reason: "peer_uid_mismatch",
            };
        }
        let Some(peer) = self.identity.as_ref() else {
            return Authority::Unknown {
                reason: "peer_birth_unavailable",
            };
        };
        if let Some(retained) = self.policy.retained_managed(peer, registry::parent) {
            return retained;
        }
        let roots = match &self.policy.roots {
            Some(r) => match r.authority_snapshot() {
                Ok(s) => s,
                Err(_) => {
                    return Authority::Unknown {
                        reason: "registry_unavailable",
                    };
                }
            },
            None => registry::AuthoritySnapshot::default(),
        };
        let authority = classify_lineage(
            peer,
            &roots,
            &self.policy.human_anchors,
            Some(&self.policy.daemon_birth),
            registry::parent,
            || match &self.policy.roots {
                Some(r) => r
                    .authority_snapshot()
                    .ok()
                    .map(|s| s.revision == roots.revision),
                None => Some(true),
            },
        );
        self.policy
            .remember_managed(peer, authority, registry::parent)
    }

    pub(crate) async fn authorize_after_enrollment(
        &self,
        method: &str,
        drain: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), RpcError> {
        await_enrollment_authority(method, drain, || {
            let authority = self.authority();
            #[cfg(feature = "testkit")]
            if matches!(
                &authority,
                Authority::Unknown {
                    reason: "spawn_enrollment_in_progress"
                }
            ) && let (Some(roots), Some(identity)) = (&self.policy.roots, &self.identity)
            {
                roots.test_observe_enrollment_pending(identity, method);
            }
            authority
        })
        .await
    }
}

// One parsed request, one fixed monotonic deadline. Polling carries no authority.
const ENROLLMENT_AUTH_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);
const ENROLLMENT_AUTH_POLL: std::time::Duration = std::time::Duration::from_millis(10);
fn draining_denial() -> RpcError {
    RpcError::new(
        RpcError::FORBIDDEN,
        "caller authority unavailable: server_draining",
    )
}
async fn await_enrollment_authority(
    method: &str,
    drain: &mut tokio::sync::watch::Receiver<bool>,
    mut classify: impl FnMut() -> Authority,
) -> Result<(), RpcError> {
    let deadline = tokio::time::Instant::now() + ENROLLMENT_AUTH_LIMIT;
    let mut last_denial = None;
    loop {
        if *drain.borrow() {
            return Err(draining_denial());
        }
        let authority = classify();
        // A kernel walk itself may cross the deadline. A late result cannot
        // grant authority after an earlier transient refusal of this request.
        if last_denial.is_some() && tokio::time::Instant::now() >= deadline {
            return if *drain.borrow() {
                Err(draining_denial())
            } else {
                Err(last_denial.take().expect("previous transient refusal"))
            };
        }
        let transient = matches!(
            &authority,
            Authority::Unknown {
                reason: "spawn_enrollment_in_progress" | "registry_revision_changed"
            }
        );
        let result = authorize_method(authority, method);
        if result.is_ok() || !transient {
            // This is a cooperative pre-effect boundary, not an atomic drain+dispatch claim.
            return if *drain.borrow() {
                Err(draining_denial())
            } else {
                result
            };
        }
        if tokio::time::Instant::now() >= deadline {
            return result;
        }
        last_denial = Some(result.expect_err("transient protected request was refused"));
        let wake = (tokio::time::Instant::now() + ENROLLMENT_AUTH_POLL).min(deadline);
        tokio::select! {
            biased;
            _ = drain.changed() => return Err(draining_denial()),
            _ = tokio::time::sleep_until(wake) => {}
        }
        if *drain.borrow() {
            return Err(draining_denial());
        }
        // No fresh success may start after expiry and revision churn never renews the deadline.
        if tokio::time::Instant::now() >= deadline {
            return Err(last_denial.take().expect("previous transient refusal"));
        }
    }
}

// Shared policy used by the actual kernel caller and deterministic contract controls.
// Controls below do not claim kernel or subprocess evidence.
fn classify_lineage(
    peer: &ProcessIdentity,
    roots: &registry::AuthoritySnapshot,
    human_anchors: &[ProcessIdentity],
    daemon_birth: Option<&ProcessIdentity>,
    mut probe: impl FnMut(i32) -> ParentProbe,
    stable_registry: impl FnOnce() -> Option<bool>,
) -> Authority {
    // Only this exact already-captured daemon birth is provably not a new spawned child.
    // All other peers, including trusted-parent descendants, remain guarded during enrollment.
    let exact_daemon_peer = daemon_birth.is_some_and(|own| same_birth(peer, own));
    if roots.enrolling != 0 && !exact_daemon_peer {
        return Authority::Unknown {
            reason: "spawn_enrollment_in_progress",
        };
    }
    let mut pid = peer.pid;
    let mut seen = BTreeSet::new();
    let mut captured = Vec::new();
    let mut anchor = false;
    for _ in 0..64 {
        if !seen.insert(pid) {
            return Authority::Unknown {
                reason: "ancestry_cycle",
            };
        }
        let ParentProbe::Found { identity, parent } = probe(pid) else {
            return Authority::Unknown {
                reason: "ancestry_unavailable",
            };
        };
        if identity.pid != pid {
            return Authority::Unknown {
                reason: "ancestry_identity_mismatch",
            };
        }
        if pid == peer.pid && !same_birth(peer, &identity) {
            return Authority::Unknown {
                reason: "peer_birth_changed",
            };
        }
        if let Some((session, _)) = roots.spawned.iter().find(|(_, r)| same_birth(r, &identity)) {
            return Authority::Managed {
                session: session.clone(),
            };
        }
        anchor |= human_anchors.iter().any(|h| same_birth(h, &identity));
        captured.push((identity, parent));
        if parent <= 1 {
            // A stable leaf alone cannot justify Human after an intermediate process
            // was reparented. Recheck every captured birth/parent plus the registry.
            let stable_chain = captured.iter().all(|(old, old_parent)| {
                matches!(probe(old.pid), ParentProbe::Found { identity, parent }
                    if same_birth(old, &identity) && parent == *old_parent)
            });
            // The exact daemon birth does not derive Human from absence of a new registry row.
            // Keep complete kernel ancestry/birth/parent revalidation and managed dominance;
            // unrelated registry revision churn cannot turn this existing process into its child.
            let registry_stable = stable_registry();
            return if anchor
                && stable_chain
                && (registry_stable == Some(true)
                    || (exact_daemon_peer && registry_stable == Some(false)))
            {
                Authority::Human
            } else if anchor && stable_chain && registry_stable == Some(false) {
                // Only concurrent registry churn is transient. The same parsed request
                // must recapture all ancestry and roots before any business effect.
                Authority::Unknown {
                    reason: "registry_revision_changed",
                }
            } else {
                Authority::Unknown {
                    reason: "unqualified_or_changed_lineage",
                }
            };
        }
        pid = parent;
    }
    Authority::Unknown {
        reason: "ancestry_depth",
    }
}
fn authorize_method(authority: Authority, method: &str) -> Result<(), RpcError> {
    match authority {
        Authority::Human => Ok(()),
        Authority::Managed { .. } | Authority::Unknown { .. }
            if matches!(
                method,
                "agent.attach" | "agent.tools" | "agent.call" | "agent.hook"
            ) =>
        {
            Ok(())
        }
        Authority::Managed { .. } | Authority::Unknown { .. } if method == methods::HELLO => Ok(()),
        Authority::Managed { .. } => Err(RpcError::new(
            RpcError::FORBIDDEN,
            "managed session peer cannot call human API",
        )),
        Authority::Unknown { reason } => Err(RpcError::new(
            RpcError::FORBIDDEN,
            format!("caller authority unavailable: {reason}"),
        )),
    }
}
#[cfg(target_os = "macos")]
fn kernel_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SOL_LOCAL/LOCAL_PEERPID from local macOS SDK; compile/kernel execution remain independent acceptance.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            0x002,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != std::mem::size_of_val(&pid) || pid <= 0 {
        return Err(io::Error::other("invalid kernel peer PID"));
    }
    Ok(pid)
}
#[cfg(target_os = "linux")]
fn kernel_pid(stream: &UnixStream) -> io::Result<i32> {
    stream
        .peer_cred()?
        .pid()
        .filter(|p| *p > 0)
        .ok_or_else(|| io::Error::other("kernel peer PID unavailable"))
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn kernel_pid(_: &UnixStream) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "kernel peer identity unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn managed_parent_observation_requires_original_birth_and_never_calls_unknown_dead() {
        let parent = identity(20, 100);
        let daemon = identity(30, 200);
        let found_parent = || ParentProbe::Found {
            identity: parent.clone(),
            parent: 1,
        };
        let found_daemon = |pid| ParentProbe::Found {
            identity: daemon.clone(),
            parent: pid,
        };
        assert_eq!(
            managed_parent_state(&parent, &daemon, |pid| {
                if pid == parent.pid {
                    found_parent()
                } else {
                    found_daemon(parent.pid)
                }
            }),
            ManagedParentState::Alive
        );
        assert_eq!(
            managed_parent_state(&parent, &daemon, |_| ParentProbe::Missing),
            ManagedParentState::Gone
        );
        assert_eq!(
            managed_parent_state(&parent, &daemon, |_| ParentProbe::Found {
                identity: identity(parent.pid, 999),
                parent: 1,
            }),
            ManagedParentState::Gone
        );
        assert_eq!(
            managed_parent_state(&parent, &daemon, |_| ParentProbe::Unreadable(libc::EACCES)),
            ManagedParentState::Unknown
        );
        assert_eq!(
            managed_parent_state(&parent, &daemon, |pid| {
                if pid == parent.pid {
                    found_parent()
                } else {
                    found_daemon(1)
                }
            }),
            ManagedParentState::Unknown
        );
        assert_eq!(
            managed_parent_state(&parent, &daemon, |pid| {
                if pid == parent.pid {
                    found_parent()
                } else {
                    ParentProbe::Unreadable(libc::EACCES)
                }
            }),
            ManagedParentState::Unknown
        );
        assert!(!crate::ServeOptions::default().managed_parent);
    }

    fn identity(pid: i32, birth: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            pgid: pid,
            start_time: birth,
            boot_id: "BOOT".into(),
            command: "diagnostic-only".into(),
        }
    }
    fn found(pid: i32, parent: i32) -> ParentProbe {
        ParentProbe::Found {
            identity: identity(pid, pid as u64),
            parent,
        }
    }
    fn chain() -> BTreeMap<i32, ParentProbe> {
        BTreeMap::from([(30, found(30, 20)), (20, found(20, 10)), (10, found(10, 1))])
    }
    fn classify(
        map: &BTreeMap<i32, ParentProbe>,
        roots: &registry::AuthoritySnapshot,
    ) -> Authority {
        classify_lineage(
            &identity(30, 30),
            roots,
            &[identity(10, 10)],
            None,
            |pid| map.get(&pid).cloned().unwrap_or(ParentProbe::Missing),
            || Some(true),
        )
    }
    #[test]
    fn s1_shared_policy_stable_human_lineage_is_a_positive() {
        assert_eq!(
            classify(&chain(), &registry::AuthoritySnapshot::default()),
            Authority::Human
        );
    }
    #[test]
    fn s1_shared_policy_managed_ancestor_dominates_human_anchor_and_retired_root() {
        let roots = registry::AuthoritySnapshot {
            spawned: vec![("SESSION".into(), identity(20, 20))],
            ..Default::default()
        };
        assert_eq!(
            classify(&chain(), &roots),
            Authority::Managed {
                session: "SESSION".into()
            }
        );
        // Retired root remains in runtime history; no active-index flag grants Human.
        for method in ["message.post", "events.subscribe", "session.start"] {
            assert!(authorize_method(classify(&chain(), &roots), method).is_err());
        }
    }
    #[test]
    fn s1_shared_policy_peer_birth_and_boot_reuse_are_not_human() {
        for changed in [
            identity(30, 31),
            ProcessIdentity {
                boot_id: "OTHERBOOT".into(),
                ..identity(30, 30)
            },
        ] {
            let mut map = chain();
            map.insert(
                30,
                ParentProbe::Found {
                    identity: changed,
                    parent: 20,
                },
            );
            assert_eq!(
                classify(&map, &Default::default()),
                Authority::Unknown {
                    reason: "peer_birth_changed"
                }
            );
        }
    }
    #[test]
    fn s1_shared_policy_unseen_reparent_never_becomes_human() {
        let map = BTreeMap::from([(30, found(30, 1))]);
        assert_eq!(
            classify(&map, &Default::default()),
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage"
            }
        );
    }
    #[test]
    fn s1_shared_policy_changed_intermediate_parent_is_unknown() {
        let map = chain();
        let mut twenty_reads = 0;
        let outcome = classify_lineage(
            &identity(30, 30),
            &Default::default(),
            &[identity(10, 10)],
            None,
            |pid| {
                if pid == 20 {
                    twenty_reads += 1;
                    if twenty_reads == 2 {
                        return found(20, 1);
                    }
                }
                map.get(&pid).cloned().unwrap_or(ParentProbe::Missing)
            },
            || Some(true),
        );
        assert_eq!(
            outcome,
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage"
            }
        );
    }
    #[test]
    fn s1_shared_policy_unreadable_cycle_and_depth_are_fail_closed() {
        let mut map = chain();
        map.insert(20, ParentProbe::Unreadable(13));
        assert_eq!(
            classify(&map, &Default::default()),
            Authority::Unknown {
                reason: "ancestry_unavailable"
            }
        );
        map.insert(20, found(20, 30));
        assert_eq!(
            classify(&map, &Default::default()),
            Authority::Unknown {
                reason: "ancestry_cycle"
            }
        );
        let result = classify_lineage(
            &identity(30, 30),
            &Default::default(),
            &[],
            None,
            |pid| found(pid, pid + 1),
            || Some(true),
        );
        assert_eq!(
            result,
            Authority::Unknown {
                reason: "ancestry_depth"
            }
        );
    }
    #[test]
    fn s1_shared_policy_enrollment_or_revision_change_withholds_human() {
        let roots = registry::AuthoritySnapshot {
            enrolling: 1,
            ..Default::default()
        };
        assert_eq!(
            classify(&chain(), &roots),
            Authority::Unknown {
                reason: "spawn_enrollment_in_progress"
            }
        );
        let map = chain();
        let result = classify_lineage(
            &identity(30, 30),
            &Default::default(),
            &[identity(10, 10)],
            None,
            |pid| map.get(&pid).cloned().unwrap_or(ParentProbe::Missing),
            || Some(false),
        );
        assert_eq!(
            result,
            Authority::Unknown {
                reason: "registry_revision_changed"
            }
        );
    }
    #[test]
    fn s1_shared_route_policy_keeps_three_entry_classes_separate() {
        for authority in [
            Authority::Managed {
                session: "SESSION".into(),
            },
            Authority::Unknown {
                reason: "ancestry_unavailable",
            },
        ] {
            for method in [
                "agent.attach",
                "agent.tools",
                "agent.call",
                "agent.hook",
                methods::HELLO,
            ] {
                assert!(authorize_method(authority.clone(), method).is_ok());
            }
            for method in [
                "message.post",
                "events.subscribe",
                "session.start",
                "loop.recover",
                "agent.unknown",
            ] {
                let error = authorize_method(authority.clone(), method).unwrap_err();
                assert_eq!(error.code, RpcError::FORBIDDEN);
            }
        }
        for method in ["message.post", "events.subscribe", "agent.attach"] {
            assert!(authorize_method(Authority::Human, method).is_ok());
        }
    }
    fn policy() -> PeerPolicy {
        PeerPolicy {
            roots: None,
            human_anchors: vec![identity(10, 10)],
            trusted_clients: Default::default(),
            trusted_connections: Default::default(),
            daemon_birth: identity(10, 10),
            owner_uid: 0,
            managed_peers: Mutex::new(BTreeMap::new()),
        }
    }
    #[test]
    fn s1_shared_history_never_elevates_observed_managed_birth_after_human_anchor_reparent() {
        let policy = policy();
        let peer = identity(30, 30);
        let managed = Authority::Managed {
            session: "actual-observed-contract".into(),
        };
        assert_eq!(
            policy.remember_managed(&peer, managed.clone(), |_| found(30, 20)),
            managed
        );
        // Exact same observed birth later has a genuinely otherwise-qualified parent.
        let map = BTreeMap::from([(30, found(30, 10)), (10, found(10, 1))]);
        assert_eq!(
            classify_lineage(
                &peer,
                &Default::default(),
                &policy.human_anchors,
                None,
                |p| map.get(&p).cloned().unwrap_or(ParentProbe::Missing),
                || Some(true)
            ),
            Authority::Human
        );
        assert_eq!(
            policy.retained_managed(&peer, |_| found(30, 10)),
            Some(managed.clone())
        );
        // Shared policy, new connection with the same actual birth: restrictive history persists.
        let reconnected = peer.clone();
        assert_eq!(
            policy.retained_managed(&reconnected, |_| found(30, 10)),
            Some(managed)
        );
        assert!(
            authorize_method(
                policy
                    .retained_managed(&reconnected, |_| found(30, 10))
                    .unwrap(),
                "message.post"
            )
            .is_err()
        );
    }
    #[test]
    fn s1_shared_history_rechecks_birth_and_does_not_taint_reused_pid() {
        let policy = policy();
        let old = identity(30, 30);
        policy.remember_managed(
            &old,
            Authority::Managed {
                session: "old".into(),
            },
            |_| found(30, 20),
        );
        assert_eq!(
            policy.retained_managed(&old, |_| ParentProbe::Found {
                identity: identity(30, 31),
                parent: 10
            }),
            Some(Authority::Unknown {
                reason: "retained_peer_birth_unavailable_or_changed"
            })
        );
        assert_eq!(
            policy.retained_managed(&identity(30, 31), |_| panic!(
                "no old history for new birth"
            )),
            None
        );
        let changed = ProcessIdentity {
            boot_id: "NEWBOOT".into(),
            ..old.clone()
        };
        assert_eq!(
            policy.retained_managed(&changed, |_| panic!("no cross-boot history")),
            None
        );
    }
    #[test]
    fn s1_shared_history_unknown_never_becomes_human_or_fabricates_persistence() {
        let policy = policy();
        let peer = identity(30, 30);
        let unknown = Authority::Unknown {
            reason: "unqualified_or_changed_lineage",
        };
        assert_eq!(
            policy.remember_managed(&peer, unknown.clone(), |_| panic!(
                "Unknown is not managed evidence"
            )),
            unknown
        );
        assert_eq!(
            policy.retained_managed(&peer, |_| panic!("empty history")),
            None
        );
        assert!(policy.managed_peers.lock().unwrap().is_empty());
        let restarted = PeerPolicy {
            roots: None,
            human_anchors: vec![identity(10, 10)],
            trusted_clients: Default::default(),
            trusted_connections: Default::default(),
            daemon_birth: identity(10, 10),
            owner_uid: 0,
            managed_peers: Mutex::new(BTreeMap::new()),
        };
        assert_eq!(
            restarted.retained_managed(&peer, |_| panic!("restart has no persisted history")),
            None
        );
    }

    // Append inside existing peer::tests. Real kernel socket + unchanged production conn route.
    #[tokio::test]
    async fn s1_same_process_human_rpc_succeeds_during_unrelated_spawn_enrollment() {
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProcessRegistry::load(&dir.path().join("registry.json")).unwrap());
        let policy = Arc::new(PeerPolicy::new(Some(registry.clone())).unwrap());
        let db = crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap();
        let state = Arc::new(crate::AppState::new(db.clone()));
        assert!(state.peer_policy.set(policy.clone()).is_ok());
        // Uses the same real registry guard as driver::spawn: no altered registry flags/policy.
        let enrollment = registry.begin_enrollment().unwrap();
        let held_before = registry.authority_snapshot().unwrap().enrolling;
        let (client, server) = UnixStream::pair().unwrap();
        let captured = policy.accept(&server);
        let captured_authority = captured.as_ref().ok().map(ConnectionPeer::authority);
        let mut task = tokio::spawn(crate::conn::run(state.clone(), server));
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);
        let request = serde_json::json!({"id":981,"method":"crew.create","params":{
            "title":"S1_UNRELATED_ENROLLMENT_HUMAN","workingDirectory":dir.path()
        }});
        let bytes = format!("{request}\n");
        let written =
            tokio::time::timeout(Duration::from_secs(5), writer.write_all(bytes.as_bytes())).await;
        let mut line = String::new();
        let received =
            tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
        let response = serde_json::from_str::<serde_json::Value>(&line);
        let rows = db
            .call(|s| {
                s.conn().query_row(
                    "SELECT COUNT(*) FROM crews WHERE title='S1_UNRELATED_ENROLLMENT_HUMAN'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await;
        let held_after = registry.authority_snapshot();
        let eof = tokio::time::timeout(Duration::from_secs(5), writer.shutdown()).await;
        drop(writer);
        drop(reader);
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
        let normal = matches!(&joined, Ok(Ok(Ok(()))));
        if joined.is_err() {
            task.abort();
            let _ = task.await;
        }
        drop(enrollment);
        let retired = registry.authority_snapshot();
        drop(state);
        drop(policy);
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(registry);
        drop(dir);
        assert!(
            closed.is_ok() && normal && matches!(eof, Ok(Ok(()))),
            "normal owned conn/Db cleanup before functionality"
        );
        assert!(
            matches!(written, Ok(Ok(()))) && matches!(received, Ok(Ok(n)) if n > 0),
            "actual kernel NDJSON caller controls"
        );
        assert_eq!(held_before, 1);
        assert_eq!(
            held_after.unwrap().enrolling,
            1,
            "no enroll release/retry secretly grants Human"
        );
        assert_eq!(retired.unwrap().enrolling, 0);
        assert_eq!(
            captured_authority,
            Some(Authority::Human),
            "actual accepted daemon-own birth stays Human during unrelated enrollment"
        );
        let response = response.unwrap();
        assert_eq!(response["id"], 981);
        assert!(
            response.get("error").is_none() && response["result"]["id"].is_string(),
            "same-process Human must not be globally refused"
        );
        assert_eq!(
            rows.unwrap().unwrap(),
            1,
            "actual ordinary Tx committed once without retry"
        );
    }

    #[test]
    fn s1_enrollment_exact_daemon_exception_keeps_pending_children_and_unknown_fail_closed() {
        let roots = registry::AuthoritySnapshot {
            enrolling: 1,
            ..Default::default()
        };
        let own = identity(10, 10);
        let map = chain();
        // Even an exact trusted parent in its ancestry cannot qualify a not-yet-enrolled child.
        let child = classify_lineage(
            &identity(30, 30),
            &roots,
            std::slice::from_ref(&own),
            Some(&own),
            |pid| map.get(&pid).cloned().unwrap_or(ParentProbe::Missing),
            || Some(true),
        );
        assert_eq!(
            child,
            Authority::Unknown {
                reason: "spawn_enrollment_in_progress"
            }
        );
        for method in ["message.post", "events.subscribe", "session.start"] {
            assert_eq!(
                authorize_method(child.clone(), method).unwrap_err().code,
                RpcError::FORBIDDEN
            );
        }
        for peer in [
            identity(10, 11),
            ProcessIdentity {
                boot_id: "OTHERBOOT".into(),
                ..own.clone()
            },
        ] {
            assert_eq!(
                classify_lineage(
                    &peer,
                    &roots,
                    std::slice::from_ref(&own),
                    Some(&own),
                    |_| found(10, 1),
                    || Some(true)
                ),
                Authority::Unknown {
                    reason: "spawn_enrollment_in_progress"
                }
            );
        }
        assert_eq!(
            classify_lineage(
                &own,
                &roots,
                std::slice::from_ref(&own),
                Some(&own),
                |_| ParentProbe::Unreadable(libc::EPERM),
                || Some(true)
            ),
            Authority::Unknown {
                reason: "ancestry_unavailable"
            }
        );
        let mut reads = 0;
        assert_eq!(
            classify_lineage(
                &own,
                &roots,
                std::slice::from_ref(&own),
                Some(&own),
                |_| {
                    reads += 1;
                    if reads == 1 {
                        found(10, 1)
                    } else {
                        found(10, 2)
                    }
                },
                || Some(true)
            ),
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage"
            }
        );
        // A trusted runtime managed record still dominates, even if a fixture records the daemon.
        let known = registry::AuthoritySnapshot {
            enrolling: 1,
            spawned: vec![("recorded".into(), own.clone())],
            ..Default::default()
        };
        assert_eq!(
            classify_lineage(
                &own,
                &known,
                std::slice::from_ref(&own),
                Some(&own),
                |_| found(10, 1),
                || Some(true)
            ),
            Authority::Managed {
                session: "recorded".into()
            }
        );
        assert_eq!(
            classify_lineage(
                &own,
                &roots,
                std::slice::from_ref(&own),
                Some(&own),
                |_| found(10, 1),
                || None
            ),
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage"
            }
        );
        // Genuine stable self is independent only of unrelated registry revision, not kernel birth.
        assert_eq!(
            classify_lineage(
                &own,
                &roots,
                std::slice::from_ref(&own),
                Some(&own),
                |_| found(10, 1),
                || Some(false)
            ),
            Authority::Human
        );
    }

    #[test]
    fn s1_exact_daemon_kernel_birth_survives_real_registry_revision_crossing() {
        let dir = tempfile::tempdir().unwrap();
        let roots = Arc::new(ProcessRegistry::load(&dir.path().join("registry.json")).unwrap());
        let policy = PeerPolicy::new(Some(roots.clone())).unwrap();
        let before = roots.authority_snapshot().unwrap();
        let mut held = None;
        let mut polled_revision = None;
        let authority = classify_lineage(
            &policy.daemon_birth,
            &before,
            &policy.human_anchors,
            Some(&policy.daemon_birth),
            |pid| {
                if held.is_none() {
                    held = Some(roots.begin_enrollment().unwrap());
                }
                registry::parent(pid)
            },
            || {
                let revision = roots.authority_snapshot().unwrap().revision;
                polled_revision = Some(revision);
                Some(revision == before.revision)
            },
        );
        let during = roots.authority_snapshot().unwrap();
        drop(held);
        let after = roots.authority_snapshot().unwrap();
        drop(policy);
        drop(roots);
        drop(dir);
        assert_eq!(before.enrolling, 0);
        assert_eq!(during.enrolling, 1);
        assert_eq!(after.enrolling, 0);
        assert_ne!(
            polled_revision.unwrap(),
            before.revision,
            "actual registry changed inside fresh kernel walk"
        );
        assert_eq!(
            authority,
            Authority::Human,
            "exact kernel-confirmed daemon birth cannot become an unrelated new child"
        );
    }
}

#[cfg(test)]
mod enrollment_wait_controls {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn pending() -> Authority {
        Authority::Unknown {
            reason: "spawn_enrollment_in_progress",
        }
    }
    #[tokio::test]
    async fn s1_revalidation_kernel_walk_finishing_after_deadline_cannot_grant_human() {
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let got = await_enrollment_authority("session.start", &mut rx, || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Authority::Unknown {
                    reason: "registry_revision_changed",
                }
            } else {
                // Controls an actually synchronous ancestry probe crossing the
                // deadline; no provider or business operation is invoked.
                std::thread::sleep(ENROLLMENT_AUTH_LIMIT);
                Authority::Human
            }
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn s1_revision_churn_revalidates_same_request_and_never_retries_effects() {
        for final_authority in [
            Authority::Human,
            Authority::Managed {
                session: "native".into(),
            },
            Authority::Unknown {
                reason: "unqualified_or_changed_lineage",
            },
        ] {
            let (_tx, mut rx) = tokio::sync::watch::channel(false);
            let calls = AtomicUsize::new(0);
            let got = await_enrollment_authority("session.start", &mut rx, || {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Authority::Unknown {
                        reason: "registry_revision_changed",
                    }
                } else {
                    final_authority.clone()
                }
            })
            .await;
            assert_eq!(got.is_ok(), matches!(final_authority, Authority::Human));
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let start = tokio::time::Instant::now();
        let got = await_enrollment_authority("session.start", &mut rx, || Authority::Unknown {
            reason: "registry_revision_changed",
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(tokio::time::Instant::now() - start, ENROLLMENT_AUTH_LIMIT);
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let got = await_enrollment_authority("session.start", &mut rx, || {
            calls.fetch_add(1, Ordering::SeqCst);
            tx.send(true).unwrap();
            Authority::Unknown {
                reason: "registry_revision_changed",
            }
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn s1_enrollment_wait_settles_same_request_without_cached_human() {
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let got = await_enrollment_authority("message.post", &mut rx, || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending()
            } else {
                Authority::Human
            }
        })
        .await;
        assert!(got.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn s1_enrollment_wait_keeps_one_deadline_under_continuous_churn() {
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let start = tokio::time::Instant::now();
        let got = await_enrollment_authority("events.subscribe", &mut rx, || {
            calls.fetch_add(1, Ordering::SeqCst);
            pending()
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(tokio::time::Instant::now() - start, ENROLLMENT_AUTH_LIMIT);
        assert!(calls.load(Ordering::SeqCst) > 1);
    }
    #[tokio::test(start_paused = true)]
    async fn s1_enrollment_wait_drain_wins_before_pending_release() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let got = await_enrollment_authority("message.post", &mut rx, || {
            calls.fetch_add(1, Ordering::SeqCst);
            tx.send(true).unwrap();
            pending()
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn s1_enrollment_wait_never_promotes_managed_unavailable_or_legal_agent() {
        for (authority, method, ok) in [
            (
                Authority::Managed {
                    session: "owned".into(),
                },
                "message.post",
                false,
            ),
            (
                Authority::Unknown {
                    reason: "registry_unavailable",
                },
                "message.post",
                false,
            ),
            (pending(), "agent.attach", true),
            (pending(), "system.hello", true),
        ] {
            let (_tx, mut rx) = tokio::sync::watch::channel(false);
            let start = tokio::time::Instant::now();
            let got = await_enrollment_authority(method, &mut rx, || authority.clone()).await;
            assert_eq!(got.is_ok(), ok);
            assert_eq!(tokio::time::Instant::now(), start);
        }
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let calls = AtomicUsize::new(0);
        let got = await_enrollment_authority("message.post", &mut rx, || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending()
            } else {
                Authority::Managed {
                    session: "registered-real-root".into(),
                }
            }
        })
        .await;
        assert_eq!(got.unwrap_err().code, RpcError::FORBIDDEN);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

#[cfg(test)]
mod public_physical_controls {
    use super::*;
    use crewd_exec::registry::ProbeResult;
    use std::{
        io::{BufRead, BufReader},
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    #[test]
    fn public_native_runtime_requires_actual_enrolled_same_generation_helper_chain() {
        let directory = tempfile::tempdir().unwrap();
        let roots =
            Arc::new(ProcessRegistry::load(&directory.path().join("registry.json")).unwrap());
        let policy = PeerPolicy::new(Some(roots.clone())).unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command
            .args([
                "-c",
                r#"import os,time
pid=os.fork()
if pid==0:
 os.setsid()
 time.sleep(30)
else:
 print(pid,flush=True)
 time.sleep(30)
"#,
            ])
            .stdout(Stdio::piped());
        // SAFETY: setsid is async-signal-safe and only establishes this owned
        // helper's fresh group before exec, never another process's group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut helper = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(helper.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let native_pid = line.trim().parse::<i32>().unwrap();
        struct Cleanup {
            helper: std::process::Child,
            native: i32,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for pid in [self.native, self.helper.id() as i32] {
                    if let ProbeResult::Found(birth) = registry::probe(pid) {
                        registry::apply(&registry::decide(&birth, &registry::probe(pid)), &birth);
                    }
                }
                let _ = self.helper.wait();
            }
        }
        let owned = Cleanup {
            helper,
            native: native_pid,
        };
        assert!(
            policy
                .owned_runtime_birth("native-session", native_pid as u32)
                .is_none(),
            "unregistered same-UID child is never physical authority"
        );
        let generation = uuid::Uuid::new_v4();
        let nonce = [9; 16];
        let mut custody = crewd_exec::acp::custody::Custody::enroll_helper(
            roots.clone(),
            "native-session".into(),
            generation,
            nonce,
            owned.helper.id() as i32,
        )
        .unwrap();
        custody
            .enroll_native(&crewd_exec::acp::birth::BirthReport {
                pid: native_pid,
                epoch: 1,
                nonce,
            })
            .unwrap();
        let birth = policy
            .owned_runtime_birth("native-session", native_pid as u32)
            .unwrap();
        assert_eq!(birth.pid, native_pid);
        assert!(
            policy
                .owned_runtime_birth("foreign-session", native_pid as u32)
                .is_none()
        );
        let key = roots
            .try_entries()
            .unwrap()
            .into_iter()
            .find(|(_, identity)| identity == custody.helper_identity())
            .expect("actual enrolled helper registration")
            .0;
        roots.forget(&key).unwrap();
        assert!(
            policy
                .owned_runtime_birth("native-session", native_pid as u32)
                .is_none(),
            "restrictive retained history cannot replace current same-generation helper registration"
        );
        drop(owned);
    }
}

#[cfg(test)]
mod host_admin_controls {
    use super::*;

    // Only this in-module seam supplies synthetic uid0; production's sole
    // constructor reads actual peer_cred. This is not real Linux root proof.
    fn synthetic_permit(policy: Arc<PeerPolicy>, uid: u32) -> RootAdminPermit {
        let ParentProbe::Found { identity, .. } = registry::parent(std::process::id() as i32)
        else {
            panic!("current birth required")
        };
        RootAdminPermit(ConnectionPeer {
            policy,
            identity: Some(identity),
            kernel_uid: uid,
            connection_scope: crate::connection_scope::ConnectionScope::new(),
        })
    }
    #[tokio::test]
    async fn host_admin_real_ordinary_socket_peer_never_becomes_root() {
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "ordinary-user test context required"
        );
        let policy = Arc::new(PeerPolicy::new(None).unwrap());
        let (stream, _other) = UnixStream::pair().unwrap();
        assert!(policy.accept_root_admin(&stream).is_err());
        assert!(
            synthetic_permit(policy, unsafe { libc::geteuid() })
                .check()
                .is_err()
        );
    }
    #[tokio::test]
    async fn host_admin_original_enrollment_lookup_is_readonly_when_identity_absent() {
        let dir = tempfile::tempdir().unwrap();
        let db =
            crewd_store::Db::spawn(crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap())
                .unwrap();
        let state = Arc::new(crate::AppState::new(db.clone()));
        let (_drain, rx) = tokio::sync::watch::channel(false);
        let manager = crate::remote::Manager::new(&state, dir.path().to_owned(), rx)
            .await
            .unwrap();
        let permit = synthetic_permit(Arc::new(PeerPolicy::new(None).unwrap()), 0);
        let query = crewd_proto::HostEnrollmentLookupParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            enrollment_id: uuid::Uuid::new_v4().to_string(),
        };
        let found = manager
            .admin_enrollment_lookup(permit.clone(), query.clone())
            .await
            .unwrap();
        assert_eq!(found, serde_json::json!({"state":"notFound"}));
        let identity = db
            .call(|s| crewd_store::remote::identity(s.conn()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            identity.is_none(),
            "read-only original lookup created server identity"
        );
        assert!(
            !dir.path().join("remote").exists(),
            "read-only lookup created private identity directory"
        );
        permit.close();
        assert!(
            manager
                .admin_enrollment_lookup(permit, query)
                .await
                .is_err()
        );
        drop(manager);
        drop(state);
        db.shutdown();
    }
    #[tokio::test]
    async fn host_admin_configuration_lookup_returns_original_witness_not_fresh_status() {
        let dir = tempfile::tempdir().unwrap();
        let db =
            crewd_store::Db::spawn(crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap())
                .unwrap();
        let state = Arc::new(crate::AppState::new(db.clone()));
        let (_drain, rx) = tokio::sync::watch::channel(false);
        let manager = crate::remote::Manager::new(&state, dir.path().to_owned(), rx)
            .await
            .unwrap();
        let permit = synthetic_permit(Arc::new(PeerPolicy::new(None).unwrap()), 0);
        let first = crewd_proto::RemoteConfigureParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: manager.status().await.unwrap().configuration.revision,
            enabled: false,
            listen_host: "127.0.0.1".into(),
            port: 49381,
        };
        let absent = manager
            .admin_configuration_lookup(permit.clone(), first.clone())
            .await
            .unwrap();
        assert_eq!(absent["state"], "notFound");
        let original = manager
            .configure_with_admin(first.clone(), Some(permit.clone()))
            .await
            .unwrap();
        let second = crewd_proto::RemoteConfigureParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: original["revision"].as_str().unwrap().into(),
            enabled: false,
            listen_host: "127.0.0.1".into(),
            port: 49382,
        };
        manager
            .configure_with_admin(second, Some(permit.clone()))
            .await
            .unwrap();
        let recovered = manager
            .admin_configuration_lookup(permit.clone(), first.clone())
            .await
            .unwrap();
        assert_eq!(recovered["state"], "recorded");
        assert_eq!(recovered["configuration"], original);
        assert_ne!(
            recovered["configuration"]["revision"],
            manager.status().await.unwrap().configuration.revision
        );
        let mut altered = first.clone();
        altered.port = 49383;
        assert!(
            manager
                .admin_configuration_lookup(permit.clone(), altered)
                .await
                .is_err()
        );
        permit.close();
        assert!(
            manager
                .admin_configuration_lookup(permit, first)
                .await
                .is_err()
        );
        drop(manager);
        drop(state);
        db.shutdown();
    }
    #[test]
    fn host_admin_sealed_permit_rechecks_connection_birth_and_managed_floor() {
        let dir = tempfile::tempdir().unwrap();
        let roots = Arc::new(ProcessRegistry::load(&dir.path().join("registry.json")).unwrap());
        let policy = Arc::new(PeerPolicy::new(Some(roots.clone())).unwrap());
        let permit = synthetic_permit(policy.clone(), 0);
        assert!(permit.check().is_ok());
        let enrollment = roots.begin_enrollment().unwrap();
        assert!(
            permit.check().is_err(),
            "spawn enrollment revokes pending management authority"
        );
        drop(enrollment);
        assert!(permit.check().is_ok());
        let mut reused = synthetic_permit(policy.clone(), 0);
        reused.0.identity.as_mut().unwrap().start_time += 1;
        assert!(
            reused.check().is_err(),
            "PID reuse is not the original root birth"
        );
        permit.close();
        assert!(
            permit.check().is_err(),
            "closed root connection cannot mutate"
        );
        let managed = synthetic_permit(policy, 0);
        let birth = managed.0.identity.clone().unwrap();
        roots.record("synthetic-managed-root-floor", birth).unwrap();
        assert!(
            managed.check().is_err(),
            "managed lineage cannot acquire root service authority"
        );
    }
}
