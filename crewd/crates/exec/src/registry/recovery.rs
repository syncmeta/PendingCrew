//! Kernel observations for bounded startup recovery. Signal success is not settlement.
//! New source preparation: callers must retain an obligation on every Unknown.
use super::{ProbeResult, ProcessIdentity};
use serde::Serialize;
use std::io;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupObservation {
    Absent,
    Present(Vec<ProcessIdentity>),
    Unknown(i32),
}

/// A real boot identity, never a generated UUID or an empty default.
pub fn boot_id() -> io::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes = [0u8; 128];
        let mut len = bytes.len();
        // SAFETY: fixed writable buffer and exact NUL-terminated sysctl key;
        // this is a read-only query (newp=NULL, newlen=0).
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.bootsessionuuid".as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if len == 0 || len > bytes.len() {
            return Err(io::Error::other("boot identity size unknown"));
        }
        let end = bytes[..len].iter().position(|b| *b == 0).unwrap_or(len);
        let value = std::str::from_utf8(&bytes[..end])
            .map_err(|_| io::Error::other("boot identity encoding unknown"))?;
        uuid::Uuid::parse_str(value)
            .map_err(|_| io::Error::other("boot identity not native UUID"))?;
        Ok(value.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let value = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        let value = value.trim();
        uuid::Uuid::parse_str(value)
            .map_err(|_| io::Error::other("boot identity not native UUID"))?;
        Ok(value.to_owned())
    }
}

/// Kernel point-in-time group existence. EPERM/unknown is never absence.
fn group_exists(pgid: i32) -> io::Result<bool> {
    if pgid <= 1 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: signal zero does not send a signal to any member.
    let result = unsafe { libc::kill(-pgid, 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn unknown(error: io::Error) -> GroupObservation {
    GroupObservation::Unknown(error.raw_os_error().unwrap_or(0))
}

#[cfg(target_os = "macos")]
fn mac_pid_count(n: i32, capacity: usize, errno: i32) -> Result<usize, i32> {
    // A zero even with errno==0 is not an authoritative group absence.
    if n <= 0 {
        return Err(errno);
    }
    if n as usize >= capacity {
        return Err(libc::EOVERFLOW);
    }
    Ok(n as usize)
}

// Only a kernel ESRCH permits excluding a disappeared enumerated PID.
#[cfg(any(target_os = "linux", test))]
fn census_candidate(pgid: i32, observed: Result<i32, i32>) -> Result<bool, i32> {
    match observed {
        Ok(actual) => Ok(actual == pgid),
        Err(libc::ESRCH) => Ok(false),
        Err(errno) => Err(errno),
    }
}

#[cfg(any(target_os = "linux", test))]
fn kernel_pgid(pid: i32) -> Result<i32, i32> {
    // SAFETY: read-only kernel process-group lookup; capture errno immediately.
    let pgid = unsafe { libc::getpgid(pid) };
    if pgid < 0 {
        Err(io::Error::last_os_error().raw_os_error().unwrap_or(0))
    } else {
        Ok(pgid)
    }
}

#[cfg(any(target_os = "linux", test))]
std::thread_local! {
    // Pure memory: no stderr/pipe I/O can block the global recovery deadline.
    static CENSUS_NOTES: std::cell::RefCell<Vec<(i32, &'static str, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(any(target_os = "linux", test))]
fn census_unknown(pid: i32, stage: &'static str, errno: i32) -> GroupObservation {
    CENSUS_NOTES.with(|notes| {
        let mut notes = notes.borrow_mut();
        if notes.len() < 16 {
            notes.push((pid, stage, errno));
        }
    });
    GroupObservation::Unknown(errno)
}

#[cfg(any(target_os = "linux", test))]
fn qualified_proc_view() -> io::Result<()> {
    // Supported scope: procfs exposes the caller's active PID namespace,
    // procfs is mounted at /proc with no hidepid/restrictive view or per-PID
    // overlays. This is not a proof for arbitrary virtualized proc views.
    let pid = std::process::id();
    let self_link = std::fs::read_link("/proc/self")?;
    if self_link
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        != Some(pid)
    {
        return Err(io::Error::from_raw_os_error(libc::EXDEV));
    }
    // NSpid starts in the namespace of this procfs mount. One entry equal
    // to getpid excludes an ancestor-mounted view of this caller.
    let status = std::fs::read_to_string("/proc/self/status")?;
    let ns_pids = status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOTSUP))?;
    let fields: Vec<_> = ns_pids.split_whitespace().collect();
    if fields != [pid.to_string()] {
        return Err(io::Error::from_raw_os_error(libc::EXDEV));
    }
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
    qualified_proc_mounts(&mounts).map_err(io::Error::from_raw_os_error)
}

#[cfg(any(target_os = "linux", test))]
fn qualified_proc_mounts(mounts: &str) -> Result<(), i32> {
    let mut qualified = false;
    for line in mounts.lines() {
        let (left, right) = line.split_once(" - ").ok_or(libc::EINVAL)?;
        let fields: Vec<_> = left.split_whitespace().collect();
        let fs: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 6 || fs.len() < 3 {
            return Err(libc::EINVAL);
        }
        if let Some(suffix) = fields[4].strip_prefix("/proc/") {
            let first = suffix.split('/').next().unwrap_or("");
            let boot_path = "/proc/sys/kernel/random/boot_id";
            if first.parse::<u32>().is_ok()
                || matches!(first, "self" | "thread-self")
                || boot_path.starts_with(&format!("{}/", fields[4]))
                || fields[4] == boot_path
            {
                return Err(libc::ENOTSUP);
            }
        }
        if fields[4] != "/proc" {
            continue;
        }
        if qualified || fields[3] != "/" || fs[0] != "proc" {
            return Err(libc::ENOTSUP);
        }
        for option in fields[5].split(',').chain(fs[2].split(',')) {
            if option.starts_with("hidepid=") && option != "hidepid=0" && option != "hidepid=off" {
                return Err(libc::EACCES);
            }
            if option.starts_with("subset=") {
                return Err(libc::ENOTSUP);
            }
        }
        qualified = true;
    }
    if qualified {
        Ok(())
    } else {
        Err(libc::ENOTSUP)
    }
}

/// A complete bounded group census, with boot before/after and kernel identity
/// for every returned member. Races/truncation/permission preserve Unknown.
pub fn observe_group(pgid: i32) -> GroupObservation {
    observe_group_until(pgid, Instant::now() + Duration::from_secs(2))
}

pub fn observe_group_until(pgid: i32, deadline: Instant) -> GroupObservation {
    if Instant::now() >= deadline {
        return GroupObservation::Unknown(libc::ETIMEDOUT);
    }
    let before = match boot_id() {
        Ok(v) => v,
        Err(e) => return unknown(e),
    };
    match group_exists(pgid) {
        Ok(false) => {
            return match boot_id() {
                Ok(after) if before == after => GroupObservation::Absent,
                _ => GroupObservation::Unknown(0),
            };
        }
        Err(e) => return unknown(e),
        Ok(true) => (),
    }
    let mut members = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let mut pids = vec![0i32; 16384];
        let size = std::mem::size_of_val(pids.as_slice());
        // SAFETY: clear this thread's errno, then capture it immediately after
        // libproc. Its wrapper converts a failed proc_info (-1) into count 0.
        let (n, errno) = unsafe {
            *libc::__error() = 0;
            let n = libc::proc_listpgrppids(pgid, pids.as_mut_ptr().cast(), size as i32);
            (n, *libc::__error())
        };
        let count = match mac_pid_count(n, pids.len(), errno) {
            Ok(count) => count,
            Err(errno) => return GroupObservation::Unknown(errno),
        };
        for pid in pids.into_iter().take(count).filter(|p| *p > 0) {
            if Instant::now() >= deadline {
                return GroupObservation::Unknown(libc::ETIMEDOUT);
            }
            match super::probe(pid) {
                ProbeResult::Found(identity)
                    if identity.pgid == pgid && identity.boot_id == before =>
                {
                    members.push(identity)
                }
                _ => return GroupObservation::Unknown(0),
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = qualified_proc_view() {
            return census_unknown(0, "proc-view", error.raw_os_error().unwrap_or(0));
        }
        let entries = match std::fs::read_dir("/proc") {
            Ok(v) => v,
            Err(e) => return census_unknown(0, "proc-enumerate", e.raw_os_error().unwrap_or(0)),
        };
        let mut seen = 0usize;
        for entry in entries {
            if Instant::now() >= deadline {
                return GroupObservation::Unknown(libc::ETIMEDOUT);
            }
            let entry = match entry {
                Ok(v) => v,
                Err(e) => {
                    return census_unknown(0, "proc-enumerate", e.raw_os_error().unwrap_or(0));
                }
            };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            seen += 1;
            if seen > 16384 {
                return census_unknown(0, "proc-capacity", libc::EOVERFLOW);
            }
            match census_candidate(pgid, kernel_pgid(pid)) {
                Ok(false) => continue,
                Err(errno) => return census_unknown(pid, "getpgid", errno),
                Ok(true) => (),
            }
            match super::probe(pid) {
                ProbeResult::Found(identity)
                    if identity.pgid == pgid && identity.boot_id == before =>
                {
                    // A target member must stay readable and in this group.
                    // ESRCH here is a changed target cut, not an unrelated
                    // tombstone: this observation remains Unknown.
                    match kernel_pgid(pid) {
                        Ok(actual) if actual == pgid => members.push(identity),
                        Ok(_) => return census_unknown(pid, "target-changed", 0),
                        Err(errno) => return census_unknown(pid, "target-recheck", errno),
                    }
                }
                ProbeResult::Unreadable(errno) => {
                    return census_unknown(pid, "target-identity", errno);
                }
                ProbeResult::Missing => return census_unknown(pid, "target-missing", libc::ESRCH),
                ProbeResult::Found(_) => return census_unknown(pid, "target-identity-changed", 0),
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = qualified_proc_view() {
        return census_unknown(0, "proc-view-recheck", error.raw_os_error().unwrap_or(0));
    }
    members.sort_by_key(|member| member.pid);
    members.dedup_by_key(|member| member.pid);
    if members.is_empty() {
        return GroupObservation::Unknown(0);
    }
    match (boot_id(), group_exists(pgid)) {
        (Ok(after), Ok(true)) if after == before => GroupObservation::Present(members),
        // Membership changed during this cut, so a later attempt must take a
        // new complete observation; this cut cannot claim physical absence.
        _ => GroupObservation::Unknown(0),
    }
}

/// Actual effective UID bracketed by two exact kernel-birth reads. A JSON
/// label or registry file owner is not a substitute for this observation.
pub fn owned_current(identity: &ProcessIdentity) -> io::Result<bool> {
    // SAFETY: current effective UID is a read-only kernel observation.
    owned_for_uid(identity, unsafe { libc::geteuid() })
}

fn owned_for_uid(identity: &ProcessIdentity, expected: u32) -> io::Result<bool> {
    if !matches!(super::probe(identity.pid), ProbeResult::Found(ref now) if same_identity(now, identity))
    {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    let uid = {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: matching fixed BSD process-info buffer and flavor.
        let n = unsafe {
            libc::proc_pidinfo(
                identity.pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if n != size {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: exact-size initialized result checked above.
        unsafe { info.assume_init() }.pbi_uid
    };
    #[cfg(not(target_os = "macos"))]
    let uid = {
        let status = std::fs::read_to_string(format!("/proc/{}/status", identity.pid))?;
        status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|value| value.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| io::Error::other("effective UID unavailable"))?
    };
    Ok(uid == expected
        && matches!(super::probe(identity.pid), ProbeResult::Found(ref now) if same_identity(now, identity)))
}

/// A leader's UID does not authorize every member of its process group.
/// Observe a complete actual group, then validate every captured member's
/// current birth/boot/PGID/effective UID. Any incomplete observation is Unknown.
fn signal_group_owned(
    original: &ProcessIdentity,
    deadline: Instant,
    expected_uid: u32,
) -> io::Result<bool> {
    let GroupObservation::Present(members) = observe_group_until(original.pgid, deadline) else {
        return Ok(false);
    };
    if !members.iter().any(|member| same_identity(member, original)) {
        return Ok(false);
    }
    let all_owned = member_scope(&members, &mut |member| owned_for_uid(member, expected_uid))?;
    Ok(all_owned && Instant::now() < deadline && owned_for_uid(original, expected_uid)?)
}

fn member_scope(
    members: &[ProcessIdentity],
    check: &mut dyn FnMut(&ProcessIdentity) -> io::Result<bool>,
) -> io::Result<bool> {
    if members.is_empty() {
        return Ok(false);
    }
    for member in members {
        if !check(member)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReport {
    pub key: String,
    pub original: ProcessIdentity,
    pub confirmed_gone: bool,
    /// Only the signal syscall result; never an execution/completion receipt.
    pub signal_sent: bool,
    pub detail: String,
}

fn birth_gone(original: &ProcessIdentity, current: &ProbeResult) -> Option<bool> {
    match current {
        ProbeResult::Missing => Some(true),
        ProbeResult::Unreadable(_) => None,
        ProbeResult::Found(now) => {
            // Legacy Mac boot is deliberately not filled with today's value.
            // A live matching absolute birth remains unconfirmed ownership.
            let same_birth = now.start_time == original.start_time
                && (original.boot_id.is_empty() || now.boot_id == original.boot_id);
            Some(!same_birth)
        }
    }
}

fn confirmed_gone(original: &ProcessIdentity, deadline: Instant) -> bool {
    if birth_gone(original, &super::probe(original.pid)) != Some(true) {
        return false;
    }
    if !matches!(
        observe_group_until(original.pgid, deadline),
        GroupObservation::Absent
    ) {
        return false;
    }
    birth_gone(original, &super::probe(original.pid)) == Some(true) && Instant::now() < deadline
}

/// Normal startup may signal a proven original birth once. Reconcile is
/// observation-only. The caller supplies the actual held Store preflight
/// recheck, and must refuse ordinary admission while entries remain.
pub fn recover_bounded(
    registry: &super::ProcessRegistry,
    allow_signal: bool,
    verify_store: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<RecoveryReport>> {
    // The production expectation is never caller JSON or a CLI flag.
    // SAFETY: actual daemon UID, observation only.
    recover_for_uid(registry, allow_signal, verify_store, unsafe {
        libc::geteuid()
    })
}

fn recover_for_uid(
    registry: &super::ProcessRegistry,
    allow_signal: bool,
    verify_store: &dyn Fn() -> io::Result<()>,
    expected_uid: u32,
) -> io::Result<Vec<RecoveryReport>> {
    #[cfg(any(target_os = "linux", test))]
    CENSUS_NOTES.with(|notes| notes.borrow_mut().clear());
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut reports = Vec::new();
    let mut pending = Vec::new();
    for (key, original) in registry.try_entries()? {
        let mut report = RecoveryReport {
            key,
            original,
            confirmed_gone: false,
            signal_sent: false,
            detail: "original birth/group disappearance not confirmed".into(),
        };
        if Instant::now() >= deadline {
            report.detail = "global recovery deadline expired; obligation retained".into();
        } else if confirmed_gone(&report.original, deadline) {
            verify_store()?;
            registry.forget(&report.key)?;
            report.confirmed_gone = true;
            report.detail =
                "kernel original birth gone and recorded group absent; retirement persisted".into();
        } else if allow_signal && !report.original.boot_id.is_empty() {
            let current = super::probe(report.original.pid);
            if matches!(
                super::decide(&report.original, &current),
                super::OrphanDecision::Reap
            ) && signal_group_owned(&report.original, deadline, expected_uid)?
                && Instant::now() < deadline
            {
                verify_store()?;
                // Immediately repeat complete group/member custody and leader
                // identity/UID after the Store recheck.
                // There is no cross-platform atomic birth-qualified killpg;
                // this preserves the documented residual kernel TOCTOU limit.
                if signal_group_owned(&report.original, deadline, expected_uid)? {
                    match super::apply(&super::OrphanDecision::Reap, &report.original) {
                        super::ApplyOutcome::SignalSent => {
                            report.signal_sent = true;
                            report.detail =
                                "signal syscall accepted; physical settlement pending".into();
                        }
                        outcome => {
                            report.detail =
                                format!("signal not confirmed: {outcome:?}; obligation retained")
                        }
                    }
                }
            } else {
                report.detail =
                    "current kernel birth/group/boot/UID not proven; zero signal".into();
            }
        } else if report.original.boot_id.is_empty() {
            report.detail = "legacy empty boot identity unresolved; zero signal".into();
        }
        if !report.confirmed_gone {
            pending.push(reports.len());
        }
        reports.push(report);
    }
    while !pending.is_empty() && Instant::now() < deadline {
        let mut unresolved = Vec::new();
        for index in pending {
            if Instant::now() >= deadline {
                unresolved.push(index);
                continue;
            }
            let report = &mut reports[index];
            if confirmed_gone(&report.original, deadline) {
                verify_store()?;
                registry.forget(&report.key)?;
                report.confirmed_gone = true;
                report.detail =
                    "kernel original birth gone and recorded group absent; retirement persisted"
                        .into();
            } else {
                unresolved.push(index);
            }
        }
        pending = unresolved;
        if !pending.is_empty() {
            std::thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    for index in pending {
        reports[index]
            .detail
            .push_str("; finite deadline ended with obligation retained");
    }
    #[cfg(any(target_os = "linux", test))]
    CENSUS_NOTES.with(|notes| {
        let notes = notes.borrow();
        if let Some(report) = reports.iter_mut().find(|r| !r.confirmed_gone) {
            for (pid, stage, errno) in notes.iter() {
                use std::fmt::Write;
                let _ = write!(
                    report.detail,
                    "; census pid={pid} stage={stage} errno={errno}"
                );
            }
        }
    });
    Ok(reports)
}

fn same_identity(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    struct OwnedChild {
        child: Child,
        original: ProcessIdentity,
    }
    impl OwnedChild {
        fn new() -> Self {
            let child = Command::new("/bin/sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .unwrap();
            let ProbeResult::Found(original) = super::super::probe(child.id() as i32) else {
                panic!("own child kernel birth not visible")
            };
            Self { child, original }
        }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let now = super::super::probe(self.original.pid);
            if matches!(
                super::super::decide(&self.original, &now),
                super::super::OrphanDecision::Reap
            ) {
                let _ = super::super::apply(&super::super::OrphanDecision::Reap, &self.original);
            }
            let _ = self.child.wait();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn actual_mac_kernel_census_error_is_unknown_not_absence() {
        let child = OwnedChild::new();
        // Real proc_info copyout EFAULT into this test's own PROT_NONE page.
        // NULL buffer is a valid size query, so it is not an error control.
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(page, libc::MAP_FAILED);
        let (n, errno) = unsafe {
            *libc::__error() = 0;
            let n = libc::proc_listpgrppids(child.original.pgid, page, 4096);
            let errno = *libc::__error();
            assert_eq!(libc::munmap(page, 4096), 0);
            (n, errno)
        };
        assert_eq!(
            n, 0,
            "libproc wrapper converts actual kernel failure to zero"
        );
        assert_ne!(errno, 0, "actual invalid call must expose errno");
        assert_eq!(mac_pid_count(n, 16384, errno), Err(errno));
        assert_eq!(
            mac_pid_count(0, 16384, 0),
            Err(0),
            "no stale errno and no empty-group guess"
        );
        assert_eq!(mac_pid_count(16384, 16384, 0), Err(libc::EOVERFLOW));
        assert!(
            matches!(super::super::probe(child.original.pid),ProbeResult::Found(ref now) if same_identity(now,&child.original))
        );
    }

    #[test]
    fn real_target_neighbor_and_reaped_tombstone_classification() {
        let mut target = OwnedChild::new();
        let mut neighbor = OwnedChild::new();
        assert_eq!(
            census_candidate(target.original.pgid, kernel_pgid(target.original.pid)),
            Ok(true)
        );
        assert_eq!(
            census_candidate(target.original.pgid, kernel_pgid(neighbor.original.pid)),
            Ok(false)
        );
        neighbor.child.kill().unwrap();
        neighbor.child.wait().unwrap();
        assert_eq!(kernel_pgid(neighbor.original.pid), Err(libc::ESRCH));
        assert_eq!(
            census_candidate(target.original.pgid, kernel_pgid(neighbor.original.pid)),
            Ok(false)
        );
        assert!(target.child.try_wait().unwrap().is_none());
        target.child.kill().unwrap();
        target.child.wait().unwrap();
        assert_eq!(kernel_pgid(target.original.pid), Err(libc::ESRCH));
        assert_eq!(
            census_candidate(target.original.pgid, Err(libc::EPERM)),
            Err(libc::EPERM)
        );
        assert_eq!(
            census_candidate(target.original.pgid, Err(libc::EACCES)),
            Err(libc::EACCES)
        );
        assert_eq!(
            census_candidate(target.original.pgid, Err(libc::ENOENT)),
            Err(libc::ENOENT)
        );
    }

    #[test]
    fn census_diagnostics_are_bounded_memory_only() {
        CENSUS_NOTES.with(|notes| notes.borrow_mut().clear());
        for pid in 1..=100 {
            assert_eq!(
                census_unknown(pid, "controlled-error", libc::EPERM),
                GroupObservation::Unknown(libc::EPERM)
            );
        }
        CENSUS_NOTES.with(|notes| assert_eq!(notes.borrow().len(), 16));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn linux_view_guard_does_not_admit_missing_procfs() {
        assert!(qualified_proc_view().is_err());
    }

    #[test]
    fn restricted_or_incomplete_proc_mounts_are_unknown() {
        let normal = "1 0 0:1 / /proc rw,nosuid - proc proc rw";
        assert_eq!(qualified_proc_mounts(normal), Ok(()));
        assert_eq!(
            qualified_proc_mounts(&format!(
                "{normal}\n2 1 0:2 / /proc/sys/fs/binfmt_misc rw - binfmt_misc binfmt_misc rw"
            )),
            Ok(())
        );
        assert_eq!(
            qualified_proc_mounts(&format!(
                "{normal}\n2 1 0:2 / /proc/sys ro - tmpfs tmpfs ro"
            )),
            Err(libc::ENOTSUP)
        );
        assert_eq!(
            qualified_proc_mounts(&normal.replace("proc rw", "proc rw,hidepid=2")),
            Err(libc::EACCES)
        );
        assert_eq!(qualified_proc_mounts(""), Err(libc::ENOTSUP));
        assert_eq!(
            qualified_proc_mounts(&format!(
                "{normal}\n2 1 0:2 / /proc/123 ro - tmpfs tmpfs ro"
            )),
            Err(libc::ENOTSUP)
        );
        assert_eq!(
            qualified_proc_mounts(&normal.replace("proc rw", "proc rw,subset=pid")),
            Err(libc::ENOTSUP)
        );
    }

    #[test]
    fn actual_single_member_census_preserves_real_birth_and_boot() {
        let child = OwnedChild::new();
        let observation = observe_group(child.original.pgid);
        let GroupObservation::Present(members) = observation else {
            CENSUS_NOTES.with(|notes| panic!("actual complete single-member group was not observed: {observation:?}; bounded diagnostics={:?}", notes.borrow()));
            unreachable!()
        };
        assert_eq!(members.len(), 1);
        assert!(same_identity(&members[0], &child.original));
        assert!(owned_current(&members[0]).unwrap());
    }

    #[test]
    fn all_members_must_pass_scope_and_unreadable_is_not_owned() {
        let a = ProcessIdentity {
            pid: 101,
            pgid: 101,
            start_time: 1,
            boot_id: "component-boot".into(),
            command: String::new(),
        };
        let b = ProcessIdentity {
            pid: 102,
            ..a.clone()
        };
        let mut visited = Vec::new();
        assert!(
            !member_scope(&[a.clone(), b.clone()], &mut |member| {
                visited.push(member.pid);
                Ok(member.pid == a.pid)
            })
            .unwrap()
        );
        assert_eq!(
            visited,
            vec![a.pid, b.pid],
            "a valid leader cannot skip a foreign member"
        );
        assert!(
            member_scope(&[a.clone(), b], &mut |member| if member.pid == a.pid {
                Ok(true)
            } else {
                Err(io::Error::from_raw_os_error(libc::EPERM))
            })
            .is_err()
        );
        assert!(!member_scope(&[], &mut |_| Ok(true)).unwrap());
    }

    #[test]
    fn actual_legacy_boot_and_wrong_boot_are_retained_without_signal() {
        for legacy in [true, false] {
            let mut child = OwnedChild::new();
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("registry.json");
            let registry = super::super::ProcessRegistry::load(&path).unwrap();
            let mut unconfirmed = child.original.clone();
            unconfirmed.boot_id = if legacy {
                String::new()
            } else {
                "wrong-boot".into()
            };
            registry.record("old", unconfirmed.clone()).unwrap();
            let before = std::fs::read(&path).unwrap();
            let start = Instant::now();
            let reports = recover_bounded(&registry, true, &|| Ok(())).unwrap();
            assert!(start.elapsed() < Duration::from_secs(3), "global bound");
            assert!(!reports[0].signal_sent);
            assert!(!reports[0].confirmed_gone);
            assert!(child.child.try_wait().unwrap().is_none());
            assert_eq!(
                registry.try_entries().unwrap(),
                vec![("old".into(), unconfirmed)]
            );
            assert_eq!(
                before,
                std::fs::read(&path).unwrap(),
                "unknown keeps original bytes"
            );
        }
    }

    #[test]
    fn actual_signal_is_not_settlement_until_parent_reaps_then_reconcile_clears() {
        let mut child = OwnedChild::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        registry.record("own", child.original.clone()).unwrap();
        let reports = recover_bounded(&registry, true, &|| Ok(())).unwrap();
        assert!(reports[0].signal_sent, "actual original group signal");
        child.child.wait().unwrap();
        let reports = recover_bounded(&registry, false, &|| Ok(())).unwrap();
        if !reports.is_empty() {
            assert!(reports[0].confirmed_gone);
            assert!(!reports[0].signal_sent);
        }
        assert!(registry.try_entries().unwrap().is_empty());
        assert!(
            super::super::ProcessRegistry::load(&path)
                .unwrap()
                .try_entries()
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            observe_group(child.original.pgid),
            GroupObservation::Absent
        ));
    }

    #[test]
    fn store_preflight_failure_prevents_signal_and_registry_retirement() {
        let mut child = OwnedChild::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        registry.record("own", child.original.clone()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let result = recover_bounded(&registry, true, &|| {
            Err(io::Error::other("version guard changed"))
        });
        assert!(result.is_err());
        assert!(child.child.try_wait().unwrap().is_none());
        assert_eq!(before, std::fs::read(&path).unwrap());
        assert_eq!(registry.try_entries().unwrap().len(), 1);
    }
    #[test]
    fn actual_neighbor_and_foreign_birth_are_never_signalled_or_forgotten() {
        let mut own = OwnedChild::new();
        let mut neighbor = OwnedChild::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        registry.record("own", own.original.clone()).unwrap();
        let mut foreign = neighbor.original.clone();
        foreign.start_time += 1;
        registry.record("foreign", foreign.clone()).unwrap();
        let reports = recover_bounded(&registry, true, &|| Ok(())).unwrap();
        assert!(reports.iter().find(|r| r.key == "own").unwrap().signal_sent);
        assert!(
            !reports
                .iter()
                .find(|r| r.key == "foreign")
                .unwrap()
                .signal_sent
        );
        assert!(neighbor.child.try_wait().unwrap().is_none());
        own.child.wait().unwrap();
        let _ = recover_bounded(&registry, false, &|| Ok(())).unwrap();
        assert_eq!(
            registry.try_entries().unwrap(),
            vec![("foreign".into(), foreign)]
        );
        assert!(neighbor.child.try_wait().unwrap().is_none());
    }

    #[test]
    fn actual_leader_gone_live_descendant_group_retains_unknown_without_signal() {
        use std::io::{BufRead, Write};
        let leader = Command::new("/bin/sh")
            .args(["-c", "read start; sleep 30 & echo $!; read release; exit 0"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut leader = super::super::TestOwnedGroup::new(leader);
        let ProbeResult::Found(original) = super::super::probe(leader.id() as i32) else {
            panic!("leader birth")
        };
        let mut input = leader.stdin.take().unwrap();
        input.write_all(b"start\n").unwrap();
        let mut reader = std::io::BufReader::new(leader.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let pid = line.trim().parse::<i32>().unwrap();
        let ProbeResult::Found(descendant) = super::super::probe(pid) else {
            panic!("own descendant birth")
        };
        assert_eq!(descendant.pgid, original.pgid);
        struct DescendantCleanup(ProcessIdentity);
        impl Drop for DescendantCleanup {
            fn drop(&mut self) {
                if let Ok(true) = owned_current(&self.0) {
                    let _ = super::super::apply(&super::super::OrphanDecision::Reap, &self.0);
                }
            }
        }
        let _cleanup = DescendantCleanup(descendant.clone());
        let GroupObservation::Present(members) = observe_group(original.pgid) else {
            panic!("actual complete leader+descendant census unavailable")
        };
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|m| same_identity(m, &original)));
        assert!(members.iter().any(|m| same_identity(m, &descendant)));
        input.write_all(b"release\n").unwrap();
        drop(input);
        assert!(leader.wait().unwrap().success());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        registry.record("lost-leader", original.clone()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let reports = recover_bounded(&registry, true, &|| Ok(())).unwrap();
        assert!(!reports[0].signal_sent);
        assert!(!reports[0].confirmed_gone);
        assert_eq!(before, std::fs::read(&path).unwrap());
        assert!(
            matches!(super::super::probe(pid),ProbeResult::Found(ref now) if same_identity(now,&descendant))
        );
        // Cleanup authority comes from this test's deliberately launched child
        // and its captured kernel birth, never the dead registry leader's PGID.
        assert!(owned_current(&descendant).unwrap());
        assert_eq!(
            super::super::apply(&super::super::OrphanDecision::Reap, &descendant),
            super::super::ApplyOutcome::SignalSent
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(
            observe_group_until(original.pgid, deadline),
            GroupObservation::Absent
        ) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            observe_group(original.pgid),
            GroupObservation::Absent
        ));
        let settled = recover_bounded(&registry, false, &|| Ok(())).unwrap();
        assert!(settled[0].confirmed_gone);
        assert!(!settled[0].signal_sent);
        assert!(registry.try_entries().unwrap().is_empty());
    }

    #[test]
    fn failed_registry_retirement_preserves_original_memory_and_disk_obligation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        let original = ProcessIdentity {
            pid: i32::MAX,
            pgid: i32::MAX,
            start_time: 1,
            boot_id: String::new(),
            command: "missing".into(),
        };
        registry.record("gone", original.clone()).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        assert!(recover_bounded(&registry, false, &|| Ok(())).is_err());
        assert_eq!(
            registry.try_entries().unwrap(),
            vec![("gone".into(), original)]
        );
        assert_eq!(before, std::fs::read(path).unwrap());
    }

    #[test]
    fn actual_wrong_process_group_is_unresolved_zero_signal() {
        let mut child = OwnedChild::new();
        let temp = tempfile::tempdir().unwrap();
        let registry =
            super::super::ProcessRegistry::load(&temp.path().join("registry.json")).unwrap();
        let mut wrong = child.original.clone();
        // SAFETY: observation only, never signal own process group.
        wrong.pgid = unsafe { libc::getpgrp() };
        registry.record("wrong-group", wrong.clone()).unwrap();
        let reports = recover_bounded(&registry, true, &|| Ok(())).unwrap();
        assert!(!reports[0].signal_sent);
        assert!(!reports[0].confirmed_gone);
        assert!(child.child.try_wait().unwrap().is_none());
        assert_eq!(
            registry.try_entries().unwrap(),
            vec![("wrong-group".into(), wrong)]
        );
    }

    #[test]
    fn kernel_uid_read_with_wrong_expected_policy_is_zero_signal_and_retained() {
        let mut child = OwnedChild::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("registry.json");
        let registry = super::super::ProcessRegistry::load(&path).unwrap();
        registry.record("own", child.original.clone()).unwrap();
        let before = std::fs::read(&path).unwrap();
        // A controlled internal policy seam, not a foreign-user OS action and
        // not a selectable product CLI/RPC UID. The actual kernel read is real.
        let expected = unsafe { libc::geteuid() }.checked_add(1).unwrap();
        assert!(!owned_for_uid(&child.original, expected).unwrap());
        let reports = recover_for_uid(&registry, true, &|| Ok(()), expected).unwrap();
        assert!(!reports[0].signal_sent);
        assert!(!reports[0].confirmed_gone);
        assert!(child.child.try_wait().unwrap().is_none());
        assert_eq!(before, std::fs::read(path).unwrap());
    }
}
