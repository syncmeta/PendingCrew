//! 孤儿进程登记与回收（Swift `SessionOrphanReaper` 的移植）。
//!
//! crewd 崩了 / 被杀之后，它起的 agent 进程可能还活着。重启时按登记表回收——
//! 但 **pid 会复用**：只有「登记的启动时刻」与「这个 pid 现在的启动时刻」完全一致才动手，
//! 宁可留一个孤儿，也不误杀一个无辜进程。
//!
//! 启动时刻：macOS 取 `proc_pidinfo(PROC_PIDTBSDINFO)` 的秒 + 微秒（与 sysctl
//! `KERN_PROC` 的 `p_starttime` 是同一个内核字段；libc 没给 macOS 导出 `kinfo_proc`），
//! Linux 取 `/proc/<pid>/stat` 第 22 字段（开机以来的时钟滴答）。两者都折成一个 `u64`，
//! 只在同一台机器上比较。
use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: i32,
    pub pgid: i32,
    /// 这把锁：macOS = 微秒时间戳，Linux = 开机以来的时钟滴答。
    pub start_time: u64,
    /// Linux：开机标识（`/proc/sys/kernel/random/boot_id`）。`start_time` 是开机以来的滴答，
    /// 换一次开机就可能碰巧相同，必须连开机标识一起对。macOS 的启动时刻是绝对时间，留空。
    pub boot_id: String,
    /// 进程短名。只给人看，不参与判定（会截断、会重名）。
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeResult {
    Found(ProcessIdentity),
    Missing,
    /// 系统调用答不上来（如权限）——不是「不存在」。
    Unreadable(i32),
}

/// 读 `pid` 当前的身份。
///
/// 固有的竞态：从这里读到身份到后面 `apply` 发信号之间，进程可能退出、pid 可能被复用
/// （窗口是微秒级，且要求内核在这段时间里恰好把同一个 pid 分给别人）。没有 pidfd 之类的
/// 句柄就无法消除；我们靠「启动时刻 + 进程组 + 开机标识」三者全对才动手把它压到最小。
pub fn probe(pid: i32) -> ProbeResult {
    if pid <= 0 {
        return ProbeResult::Missing;
    }
    os_probe(pid)
}

#[cfg(target_os = "macos")]
fn os_probe(pid: i32) -> ProbeResult {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: 缓冲区大小与 flavor 对应的结构体一致；内核只写满 `size` 字节。
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return if errno == libc::ESRCH || n == 0 && errno == 0 {
            ProbeResult::Missing
        } else {
            ProbeResult::Unreadable(errno)
        };
    }
    // SAFETY: 上面确认内核写满了整个结构体。
    let info = unsafe { info.assume_init() };
    let comm: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    ProbeResult::Found(ProcessIdentity {
        pid,
        pgid: info.pbi_pgid as i32,
        start_time: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        boot_id: String::new(),
        command: String::from_utf8_lossy(&comm).into_owned(),
    })
}

#[cfg(not(target_os = "macos"))]
fn os_probe(pid: i32) -> ProbeResult {
    let boot_id = match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(id) => id.trim().to_string(),
        Err(e) => return ProbeResult::Unreadable(e.raw_os_error().unwrap_or(0)),
    };
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(content) => match parse_proc_stat(&content) {
            Some((pgid, start_time, command)) => ProbeResult::Found(ProcessIdentity {
                pid,
                pgid,
                start_time,
                boot_id,
                command,
            }),
            None => ProbeResult::Unreadable(0),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => ProbeResult::Missing,
        Err(e) => ProbeResult::Unreadable(e.raw_os_error().unwrap_or(0)),
    }
}

/// 解析 Linux `/proc/<pid>/stat`：返回（第 5 字段 pgrp，第 22 字段 starttime，comm）。
/// comm 里可能有空格和括号，所以按**最后一个** `)` 切。所有平台都编译，便于单测。
pub fn parse_proc_stat(content: &str) -> Option<(i32, u64, String)> {
    let open = content.find('(')?;
    let close = content.rfind(')')?;
    let command = content.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = content.get(close + 1..)?.split_whitespace().collect();
    // rest[0] 是第 3 字段（state），所以第 N 字段在 rest[N - 3]。
    let pgid = rest.get(5 - 3)?.parse().ok()?;
    let start = rest.get(22 - 3)?.parse().ok()?;
    Some((pgid, start, command))
}

/// 一条登记的处置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrphanDecision {
    AlreadyGone,
    /// 启动时刻对得上：确认是我们的孤儿，杀进程组。
    Reap,
    /// pid 被复用了：绝不动手。
    PidReused(ProcessIdentity),
    /// 读不到：不动手，留给人。
    Unreadable(i32),
}

pub fn decide(recorded: &ProcessIdentity, current: &ProbeResult) -> OrphanDecision {
    match current {
        ProbeResult::Missing => OrphanDecision::AlreadyGone,
        ProbeResult::Unreadable(errno) => OrphanDecision::Unreadable(*errno),
        // 启动时刻、进程组、开机标识三者全对才算「我们的孤儿」；任何一项对不上都当 pid 复用。
        ProbeResult::Found(now)
            if now.start_time == recorded.start_time
                && now.pgid == recorded.pgid
                && now.boot_id == recorded.boot_id =>
        {
            OrphanDecision::Reap
        }
        ProbeResult::Found(now) => OrphanDecision::PidReused(now.clone()),
    }
}

/// 给人看的一句话。`PidReused` / `Unreadable` 必须说清「我没动它」。
pub fn describe(session_id: &str, recorded: &ProcessIdentity, decision: &OrphanDecision) -> String {
    let pid = recorded.pid;
    match decision {
        OrphanDecision::AlreadyGone => {
            format!("「{session_id}」的进程（pid {pid}）已经不在了，无需处理。")
        }
        OrphanDecision::Reap => format!(
            "「{session_id}」的进程（pid {pid}）启动时刻与登记一致，已回收其进程组 {}。",
            recorded.pgid
        ),
        OrphanDecision::PidReused(now) => format!(
            "「{session_id}」登记的 pid {pid} 现在属于另一个进程（{}，启动时刻 {} ≠ 登记的 {}）——pid 被复用了，我没有动它。",
            now.command, now.start_time, recorded.start_time
        ),
        OrphanDecision::Unreadable(errno) => format!(
            "「{session_id}」登记的 pid {pid} 读不到身份（errno {errno}），我没有动它；要不要处理请人来定。"
        ),
    }
}

/// 要发信号的对象。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillTarget {
    Group(i32),
    Process(i32),
}

/// 执行处置的真实结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 不是 `Reap`，没发信号。
    NotAttempted,
    /// 信号发出了（或目标在发之前恰好已经没了）。
    Killed,
    /// 判定是 `Reap`，但目标太危险（见 `apply_with`），一个信号都没发。
    Refused(String),
    /// 进程组和单杀都失败了（errno）。
    Failed(i32),
}

/// 执行处置：只有 `Reap` 发信号（先 killpg，失败再单杀）。
pub fn apply(decision: &OrphanDecision, recorded: &ProcessIdentity) -> ApplyOutcome {
    let own_pid = std::process::id() as i32;
    let own_pgid = rustix::process::getpgrp().as_raw_nonzero().get();
    apply_with(decision, recorded, own_pid, own_pgid, &mut |target| {
        let kill = rustix::process::Signal::KILL;
        match target {
            KillTarget::Group(g) => rustix::process::Pid::from_raw(g)
                .ok_or(libc::EINVAL)
                .and_then(|g| {
                    rustix::process::kill_process_group(g, kill).map_err(|e| e.raw_os_error())
                }),
            KillTarget::Process(p) => rustix::process::Pid::from_raw(p)
                .ok_or(libc::EINVAL)
                .and_then(|p| rustix::process::kill_process(p, kill).map_err(|e| e.raw_os_error())),
        }
    })
}

/// 整组发信号的安全闸（回收孤儿与驱动层停会话共用）：pgid <= 1（`kill(-1, …)` 会杀掉当前用户的
/// 全部进程，0 / 1 同样危险）或就是 crewd 自己所在的组，一律不发。
pub fn group_signal_refused(pgid: i32, own_pgid: i32) -> bool {
    pgid <= 1 || pgid == own_pgid
}

/// `apply` 的可测内核：信号经 `kill` 闭包发出，测试里换成记录器，不会真发。
///
/// 安全闸：即使判定是 `Reap`，登记的 pgid <= 1（`kill(-1, …)` 会杀掉当前用户的全部进程，
/// pgid 0 / 1 同样危险）、pgid 是 crewd 自己所在的组、或 pid 就是 crewd 自己，也一律
/// **什么都不做并报告**（`Refused`）——比单杀那个 pid 更保守：这种登记本身就不对劲，留给人看。
pub fn apply_with(
    decision: &OrphanDecision,
    recorded: &ProcessIdentity,
    own_pid: i32,
    own_pgid: i32,
    kill: &mut dyn FnMut(KillTarget) -> Result<(), i32>,
) -> ApplyOutcome {
    if *decision != OrphanDecision::Reap {
        return ApplyOutcome::NotAttempted;
    }
    if recorded.pid <= 1 || recorded.pid == own_pid {
        return ApplyOutcome::Refused(format!("pid {} 不能回收", recorded.pid));
    }
    if group_signal_refused(recorded.pgid, own_pgid) {
        return ApplyOutcome::Refused(format!(
            "进程组 {} 不能整组回收（<=1 或就是 crewd 自己那一组）",
            recorded.pgid
        ));
    }
    // ESRCH = 目标在这一瞬间已经没了，等同成功。
    let done = |r: &Result<(), i32>| matches!(r, Ok(()) | Err(libc::ESRCH));
    let group = kill(KillTarget::Group(recorded.pgid));
    if done(&group) {
        return ApplyOutcome::Killed;
    }
    let single = kill(KillTarget::Process(recorded.pid));
    if done(&single) {
        return ApplyOutcome::Killed;
    }
    ApplyOutcome::Failed(single.err().unwrap_or(0))
}

/// One logical runner can own a helper and several native incarnations.
/// Slots are core-generated, never a provider-supplied file path or team identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessParticipant {
    AcpHelper {
        generation: uuid::Uuid,
    },
    AcpNative {
        generation: uuid::Uuid,
        epoch: u64,
    },
    AcpBridge {
        generation: uuid::Uuid,
        epoch: u64,
        slot: u64,
    },
}
impl ProcessParticipant {
    pub(crate) fn key(self) -> io::Result<String> {
        match self {
            Self::AcpHelper { generation } if !generation.is_nil() => {
                Ok(format!("@acp/{generation}/helper"))
            }
            Self::AcpNative { generation, epoch } if !generation.is_nil() && epoch != 0 => {
                Ok(format!("@acp/{generation}/native/{epoch}"))
            }
            Self::AcpBridge {
                generation,
                epoch,
                slot,
            } if !generation.is_nil() && epoch != 0 && slot != 0 => {
                Ok(format!("@acp/{generation}/bridge/{epoch}/{slot}"))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid process participant",
            )),
        }
    }
}

/// 「如果我下一秒崩了，回来之后该去看哪些 pid」。JSON 文件，0600，整份原子替换。
#[derive(Debug)]
pub struct ProcessRegistry {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, ProcessIdentity>>,
    authority: Mutex<AuthoritySnapshot>,
    #[cfg(feature = "testkit")]
    test_enrollment_control: Mutex<Option<PathBuf>>,
}

impl ProcessRegistry {
    /// 读登记表。文件不存在 = 空表；存在但解析不了 = 错误（不能当空表，否则孤儿就漏了）。
    pub fn load(path: &Path) -> io::Result<Self> {
        let entries = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "进程登记表 {} 坏了（{e}）。crewd 拒绝启动，而不是冒险按一张读不懂的表\
                         去杀进程（可能杀到不相干的进程）。处理办法：把这个文件挪到别处（比如改名加 \
                         .bad）再启动；上次留下的 claude 进程若还活着，需要人自己看一眼、手动结束。",
                        path.display()
                    ),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
            authority: Mutex::new(AuthoritySnapshot::default()),
            #[cfg(feature = "testkit")]
            test_enrollment_control: Mutex::new(None),
        })
    }

    pub fn record(&self, session_id: &str, identity: ProcessIdentity) -> io::Result<()> {
        // Runtime restrictive provenance is published before any fallible orphan-file IO.
        self.restrict_birth(session_id, identity.clone())?;
        let mut entries = self.entries.lock().expect("登记表锁中毒");
        entries.insert(session_id.to_string(), identity);
        self.persist(&entries)
    }

    /// The sole core writer records both process groups without overwriting one
    /// orphan obligation. Runtime authority retains the actual logical session.
    pub fn record_participant(
        &self,
        session_id: &str,
        participant: ProcessParticipant,
        identity: ProcessIdentity,
    ) -> io::Result<String> {
        crate::resume::check_agent_session_id(session_id)?;
        let key = participant.key()?;
        self.restrict_birth(session_id, identity.clone())?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| io::Error::other("process registry unavailable"))?;
        if let Some(existing) = entries.get(&key) {
            if existing != &identity {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "process participant already owned",
                ));
            }
        } else {
            entries.insert(key.clone(), identity);
        }
        self.persist(&entries)?;
        Ok(key)
    }

    /// Actual kernel birth only; this restrictive history is never removed by forget.
    pub fn restrict_birth(&self, session_id: &str, identity: ProcessIdentity) -> io::Result<()> {
        let mut authority = self
            .authority
            .lock()
            .map_err(|_| io::Error::other("authority registry unavailable"))?;
        authority.revision = authority
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("registry revision exhausted"))?;
        authority.spawned.push((session_id.to_string(), identity));
        Ok(())
    }

    pub fn forget(&self, session_id: &str) -> io::Result<()> {
        let mut entries = self.entries.lock().expect("登记表锁中毒");
        if entries.remove(session_id).is_none() {
            return Ok(());
        }
        self.persist(&entries)
    }

    pub fn entries(&self) -> Vec<(String, ProcessIdentity)> {
        let entries = self.entries.lock().expect("登记表锁中毒");
        entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn persist(&self, entries: &BTreeMap<String, ProcessIdentity>) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(entries).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        io::Write::write_all(&mut f, &bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        // 改名本身也要落盘：只 fsync 文件不 fsync 目录，断电后登记表可能回到旧版本（N4 / S31）。
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

/// 一条回收结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanReport {
    pub session_id: String,
    pub decision: OrphanDecision,
    pub outcome: ApplyOutcome,
    pub message: String,
}

/// 给人看的、带真实结果的一句话。
pub fn describe_outcome(
    session_id: &str,
    recorded: &ProcessIdentity,
    decision: &OrphanDecision,
    outcome: &ApplyOutcome,
) -> String {
    match outcome {
        ApplyOutcome::Refused(why) => format!(
            "「{session_id}」的进程（pid {}）判定应回收，但{why}，我没有动它；登记留着，请人来定。",
            recorded.pid
        ),
        ApplyOutcome::Failed(errno) => format!(
            "「{session_id}」的进程（pid {}）判定应回收，但发信号失败（errno {errno}），没有杀掉；登记留着，下次再试。",
            recorded.pid
        ),
        _ => describe(session_id, recorded, decision),
    }
}

/// 启动时调用（必须持有数据目录锁，确认没有别的 crewd 活着）：逐条判定、执行、清账。
/// 读不到身份的、拒绝执行的、杀失败的，都留在表里，下次再看。
pub fn reap_orphans(registry: &ProcessRegistry) -> io::Result<Vec<OrphanReport>> {
    reap_orphans_with(registry, &probe, &mut |rec, dec| apply(dec, rec))
}

/// `reap_orphans` 的可测内核：探测与执行都可换。
pub fn reap_orphans_with(
    registry: &ProcessRegistry,
    probe: &dyn Fn(i32) -> ProbeResult,
    apply: &mut dyn FnMut(&ProcessIdentity, &OrphanDecision) -> ApplyOutcome,
) -> io::Result<Vec<OrphanReport>> {
    let mut reports = Vec::new();
    for (session_id, recorded) in registry.entries() {
        let decision = decide(&recorded, &probe(recorded.pid));
        let outcome = apply(&recorded, &decision);
        let keep = matches!(decision, OrphanDecision::Unreadable(_))
            || matches!(outcome, ApplyOutcome::Refused(_) | ApplyOutcome::Failed(_));
        if !keep {
            registry.forget(&session_id)?;
        }
        reports.push(OrphanReport {
            message: describe_outcome(&session_id, &recorded, &decision, &outcome),
            session_id,
            decision,
            outcome,
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn identity(pid: i32, start: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            pgid: pid,
            start_time: start,
            boot_id: String::new(),
            command: "claude".to_string(),
        }
    }

    #[test]
    fn participants_preserve_both_orphan_groups_and_one_logical_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let registry = ProcessRegistry::load(&path).unwrap();
        let generation = uuid::Uuid::new_v4();
        registry.record("legacy", identity(10, 10)).unwrap();
        let helper = registry
            .record_participant(
                "worker",
                ProcessParticipant::AcpHelper { generation },
                identity(20, 20),
            )
            .unwrap();
        let native = registry
            .record_participant(
                "worker",
                ProcessParticipant::AcpNative {
                    generation,
                    epoch: 1,
                },
                identity(30, 30),
            )
            .unwrap();
        assert_ne!(helper, native);
        assert_eq!(registry.entries().len(), 3);
        let authority = registry.authority_snapshot().unwrap();
        assert_eq!(
            authority
                .spawned
                .iter()
                .filter(|(source, _)| source == "worker")
                .count(),
            2
        );
        assert!(
            registry
                .record_participant(
                    "worker",
                    ProcessParticipant::AcpNative {
                        generation,
                        epoch: 1
                    },
                    identity(31, 31)
                )
                .is_err()
        );
        assert_eq!(
            registry
                .entries()
                .iter()
                .find(|(key, _)| key == &native)
                .unwrap()
                .1
                .pid,
            30
        );
        registry.forget(&helper).unwrap();
        let reloaded = ProcessRegistry::load(&path).unwrap();
        assert_eq!(reloaded.entries().len(), 2);
        assert!(reloaded.entries().iter().any(|(key, _)| key == &native));
        assert!(reloaded.entries().iter().any(|(key, _)| key == "legacy"));
    }

    /// 记下每次「发信号」调用、从不真发的杀手。
    fn recorder(
        calls: &std::cell::RefCell<Vec<KillTarget>>,
    ) -> impl FnMut(KillTarget) -> Result<(), i32> + '_ {
        move |t| {
            calls.borrow_mut().push(t);
            Ok(())
        }
    }

    #[test]
    fn decision_also_requires_same_pgid_and_same_boot() {
        let mut rec = identity(4242, 5);
        rec.boot_id = "boot-a".to_string();
        let mut now = rec.clone();
        assert_eq!(
            decide(&rec, &ProbeResult::Found(now.clone())),
            OrphanDecision::Reap
        );
        now.pgid = 1;
        assert_eq!(
            decide(&rec, &ProbeResult::Found(now.clone())),
            OrphanDecision::PidReused(now.clone()),
            "进程组变了就不是我们起的那个"
        );
        let mut rebooted = rec.clone();
        rebooted.boot_id = "boot-b".to_string();
        assert_eq!(
            decide(&rec, &ProbeResult::Found(rebooted.clone())),
            OrphanDecision::PidReused(rebooted),
            "换了一次开机：启动时刻（滴答）可能碰巧相同，也不能动手"
        );
    }

    #[test]
    fn apply_refuses_dangerous_process_groups() {
        let own_pid = 5000;
        let own_pgid = 4999;
        for (pid, pgid) in [
            (4242, 1),
            (4242, 0),
            (4242, -1),
            (4242, own_pgid),
            (1, 1),
            (own_pid, 7777),
        ] {
            let rec = ProcessIdentity {
                pid,
                pgid,
                ..identity(pid, 1)
            };
            let calls = std::cell::RefCell::new(Vec::new());
            let outcome = apply_with(
                &OrphanDecision::Reap,
                &rec,
                own_pid,
                own_pgid,
                &mut recorder(&calls),
            );
            assert!(
                matches!(outcome, ApplyOutcome::Refused(_)),
                "pid {pid} pgid {pgid}: {outcome:?}"
            );
            assert!(
                calls.borrow().is_empty(),
                "pid {pid} pgid {pgid} 不能发任何信号"
            );
        }
    }

    #[test]
    fn apply_kills_the_group_and_reports_failure_honestly() {
        let rec = ProcessIdentity {
            pid: 4242,
            pgid: 4242,
            ..identity(4242, 1)
        };
        let calls = std::cell::RefCell::new(Vec::new());
        assert_eq!(
            apply_with(
                &OrphanDecision::Reap,
                &rec,
                5000,
                4999,
                &mut recorder(&calls)
            ),
            ApplyOutcome::Killed
        );
        assert_eq!(*calls.borrow(), vec![KillTarget::Group(4242)]);
        // 进程组杀失败 → 退而单杀；都失败 = Failed，不能谎称已回收。
        let mut seen = Vec::new();
        let outcome = apply_with(&OrphanDecision::Reap, &rec, 5000, 4999, &mut |t| {
            seen.push(t);
            Err(libc::EPERM)
        });
        assert_eq!(outcome, ApplyOutcome::Failed(libc::EPERM));
        assert_eq!(
            seen,
            vec![KillTarget::Group(4242), KillTarget::Process(4242)]
        );
        // ESRCH = 已经没了，算成了。
        let outcome = apply_with(&OrphanDecision::Reap, &rec, 5000, 4999, &mut |_| {
            Err(libc::ESRCH)
        });
        assert_eq!(outcome, ApplyOutcome::Killed);
        // 非 Reap 一律不动。
        let calls = std::cell::RefCell::new(Vec::new());
        assert_eq!(
            apply_with(
                &OrphanDecision::Unreadable(1),
                &rec,
                5000,
                4999,
                &mut recorder(&calls)
            ),
            ApplyOutcome::NotAttempted
        );
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn unreadable_failed_and_refused_entries_stay_in_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let reg = ProcessRegistry::load(&dir.path().join("registry.json")).unwrap();
        reg.record("unreadable", identity(100, 1)).unwrap();
        reg.record("failed", identity(101, 1)).unwrap();
        reg.record("refused", identity(102, 1)).unwrap();
        reg.record("killed", identity(103, 1)).unwrap();
        reg.record("gone", identity(104, 1)).unwrap();
        let reports = reap_orphans_with(
            &reg,
            &|pid| match pid {
                100 => ProbeResult::Unreadable(libc::EPERM),
                104 => ProbeResult::Missing,
                _ => ProbeResult::Found(identity(pid, 1)),
            },
            &mut |rec: &ProcessIdentity, decision: &OrphanDecision| match rec.pid {
                101 => ApplyOutcome::Failed(libc::EPERM),
                102 => ApplyOutcome::Refused("test".to_string()),
                _ if *decision == OrphanDecision::Reap => ApplyOutcome::Killed,
                _ => ApplyOutcome::NotAttempted,
            },
        )
        .unwrap();
        assert_eq!(reports.len(), 5);
        let kept: Vec<String> = reg.entries().into_iter().map(|(k, _)| k).collect();
        assert_eq!(kept, vec!["failed", "refused", "unreadable"]);
        let failed = reports.iter().find(|r| r.session_id == "failed").unwrap();
        assert!(!failed.message.contains("已回收"), "{}", failed.message);
        assert!(failed.message.contains("没有杀掉"), "{}", failed.message);
        let killed = reports.iter().find(|r| r.session_id == "killed").unwrap();
        assert!(killed.message.contains("已回收"), "{}", killed.message);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_unreadable_pid_is_kept_by_the_reaper() {
        // 非 root 的 macOS 上读不到 launchd(pid 1) 的身份（EPERM）。root 能读到：那时
        // 启动时刻对不上，判复用、不动手——这条断言只在读不到时才有意义，所以 root 下直接返回。
        if rustix::process::geteuid().is_root() {
            return;
        }
        let ProbeResult::Unreadable(_) = probe(1) else {
            panic!("非 root 下 probe(1) 应当 Unreadable：{:?}", probe(1));
        };
        let dir = tempfile::tempdir().unwrap();
        let reg = ProcessRegistry::load(&dir.path().join("registry.json")).unwrap();
        reg.record("launchd", identity(1, 1)).unwrap();
        let reports = reap_orphans(&reg).unwrap();
        assert!(matches!(reports[0].decision, OrphanDecision::Unreadable(_)));
        assert_eq!(reg.entries().len(), 1, "读不到身份的那条留着");
    }

    #[test]
    fn decision_requires_exact_start_time() {
        let rec = identity(4242, 1_700_000_000_123_456);
        assert_eq!(
            decide(&rec, &ProbeResult::Missing),
            OrphanDecision::AlreadyGone
        );
        assert_eq!(
            decide(&rec, &ProbeResult::Found(rec.clone())),
            OrphanDecision::Reap
        );
        let moved = identity(4242, 1_700_000_000_123_457);
        assert_eq!(
            decide(&rec, &ProbeResult::Found(moved.clone())),
            OrphanDecision::PidReused(moved),
            "只差一微秒也判复用"
        );
        assert_eq!(
            decide(&rec, &ProbeResult::Unreadable(1)),
            OrphanDecision::Unreadable(1)
        );
    }

    #[test]
    fn only_reap_sends_a_signal_and_reuse_says_hands_off() {
        let rec = identity(i32::MAX, 1);
        let none = ApplyOutcome::NotAttempted;
        assert_eq!(apply(&OrphanDecision::AlreadyGone, &rec), none);
        assert_eq!(
            apply(&OrphanDecision::PidReused(identity(i32::MAX, 2)), &rec),
            none
        );
        assert_eq!(apply(&OrphanDecision::Unreadable(1), &rec), none);
        let msg = describe(
            "w-1",
            &rec,
            &OrphanDecision::PidReused(identity(i32::MAX, 2)),
        );
        assert!(msg.contains("没有动它"), "{msg}");
    }

    #[test]
    fn proc_stat_parsing_survives_odd_command_names() {
        let mut fields = vec!["S".to_string(), "1".to_string(), "777".to_string()];
        fields.extend((6..=21).map(|n| n.to_string()));
        fields.push("987654".to_string());
        fields.extend((23..=52).map(|n| n.to_string()));
        let line = format!("4242 (we ird) (name) {}", fields.join(" "));
        assert_eq!(
            parse_proc_stat(&line),
            Some((777, 987654, "we ird) (name".to_string()))
        );
        assert_eq!(parse_proc_stat("garbage"), None);
        assert_eq!(parse_proc_stat("1 (x) S 1"), None);
    }

    #[test]
    fn probe_sees_this_process_and_misses_a_reaped_child() {
        let me = std::process::id() as i32;
        let ProbeResult::Found(id) = probe(me) else {
            panic!("读不到自己");
        };
        assert_eq!(id.pid, me);
        let pgid = rustix::process::getpgid(None)
            .unwrap()
            .as_raw_nonzero()
            .get();
        assert_eq!(id.pgid, pgid);
        assert!(id.start_time > 0);
        assert_eq!(probe(me), ProbeResult::Found(id), "同一进程读两次完全一致");

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert_eq!(probe(pid), ProbeResult::Missing);
        assert_eq!(probe(0), ProbeResult::Missing);
    }

    #[test]
    fn reaper_kills_a_live_orphan_group_but_not_a_reused_pid() {
        let dir = tempfile::tempdir().unwrap();
        let reg = ProcessRegistry::load(&dir.path().join("registry.json")).unwrap();
        // 一个自成进程组的「孤儿」。
        let mut orphan = Command::new("/bin/sh");
        orphan.arg("-c").arg("sleep 30");
        std::os::unix::process::CommandExt::process_group(&mut orphan, 0);
        let mut orphan = orphan.spawn().unwrap();
        let pid = orphan.id() as i32;
        let ProbeResult::Found(real) = probe(pid) else {
            panic!("刚起的进程读不到");
        };
        reg.record("live", real.clone()).unwrap();
        // 同一个 pid、但登记的启动时刻差一点：必须不动。它也自成进程组——万一判定写错了，
        // killpg 杀的是它自己那一组，而不是跑测试的这一组（会把 cargo test 连同终端一起带走）。
        let mut innocent = Command::new("/bin/sh");
        innocent.arg("-c").arg("sleep 30");
        std::os::unix::process::CommandExt::process_group(&mut innocent, 0);
        let mut innocent = innocent.spawn().unwrap();
        let ipid = innocent.id() as i32;
        let ProbeResult::Found(mut fake) = probe(ipid) else {
            panic!();
        };
        fake.start_time += 1;
        reg.record("reused", fake).unwrap();

        let reports = reap_orphans(&reg).unwrap();
        assert_eq!(reports.len(), 2);
        let status = orphan.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL)
        );
        assert!(
            innocent.try_wait().unwrap().is_none(),
            "复用的 pid 绝不能被杀"
        );
        assert!(reg.entries().is_empty(), "处理过的都清账");
        innocent.kill().unwrap();
        innocent.wait().unwrap();
    }

    #[test]
    fn registry_persists_privately_and_refuses_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let reg = ProcessRegistry::load(&path).unwrap();
        reg.record("a", identity(10, 1)).unwrap();
        reg.record("b", identity(11, 2)).unwrap();
        reg.forget("a").unwrap();
        reg.forget("never").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again = ProcessRegistry::load(&path).unwrap();
        assert_eq!(again.entries(), vec![("b".to_string(), identity(11, 2))]);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("startTime"), "camelCase：{raw}");
        assert!(raw.contains("bootId"), "camelCase：{raw}");
        std::fs::write(&path, b"{not json").unwrap();
        let err = ProcessRegistry::load(&path).expect_err("坏掉的登记表不能当空表");
        let msg = err.to_string();
        // 人看到这条报错要知道怎么办：挪开它再启动；以及为什么不自己处理掉。
        assert!(msg.contains(&path.display().to_string()), "{msg}");
        assert!(msg.contains("挪"), "要告诉人怎么办：{msg}");
        assert!(msg.contains("不相干的进程"), "要说清为什么拒绝启动：{msg}");
    }
}

/// Runtime-only restrictive kernel births, published before orphan-file persistence.
/// Orphan file contents do not confer request authority; retired spawned births
/// remain restricted for this daemon lifetime so forget does not elevate helpers.
#[derive(Clone, Debug, Default)]
pub struct AuthoritySnapshot {
    pub revision: u64,
    pub enrolling: u32,
    pub spawned: Vec<(String, ProcessIdentity)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentProbe {
    Found {
        identity: ProcessIdentity,
        parent: i32,
    },
    Missing,
    Unreadable(i32),
}
#[cfg(target_os = "macos")]
pub fn parent(pid: i32) -> ParentProbe {
    if pid <= 0 {
        return ParentProbe::Missing;
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return if code == libc::ESRCH {
            ParentProbe::Missing
        } else {
            ParentProbe::Unreadable(code)
        };
    }
    let info = unsafe { info.assume_init() };
    ParentProbe::Found {
        identity: ProcessIdentity {
            pid,
            pgid: info.pbi_pgid as i32,
            start_time: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            boot_id: String::new(),
            command: String::new(),
        },
        parent: info.pbi_ppid as i32,
    }
}
#[cfg(target_os = "linux")]
pub fn parent(pid: i32) -> ParentProbe {
    if pid <= 0 {
        return ParentProbe::Missing;
    }
    let body = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return ParentProbe::Missing,
        Err(e) => return ParentProbe::Unreadable(e.raw_os_error().unwrap_or(0)),
    };
    let Some(close) = body.rfind(')') else {
        return ParentProbe::Unreadable(0);
    };
    let fields: Vec<_> = body[close + 1..].split_whitespace().collect();
    let Some(ppid) = fields.get(1).and_then(|v| v.parse::<i32>().ok()) else {
        return ParentProbe::Unreadable(0);
    };
    let Some((pgid, start_time, command)) = parse_proc_stat(&body) else {
        return ParentProbe::Unreadable(0);
    };
    let boot_id = match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        Ok(v) => v.trim().to_string(),
        Err(e) => return ParentProbe::Unreadable(e.raw_os_error().unwrap_or(0)),
    };
    ParentProbe::Found {
        identity: ProcessIdentity {
            pid,
            pgid,
            start_time,
            boot_id,
            command,
        },
        parent: ppid,
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn parent(_: i32) -> ParentProbe {
    ParentProbe::Unreadable(libc::ENOSYS)
}
impl ProcessRegistry {
    pub fn authority_snapshot(&self) -> io::Result<AuthoritySnapshot> {
        self.authority
            .lock()
            .map(|s| s.clone())
            .map_err(|_| io::Error::other("authority registry unavailable"))
    }
}

pub struct Enrollment {
    registry: std::sync::Arc<ProcessRegistry>,
    // A spawned child is an obligation until successful registration or raw OS terminal proof.
    owned_unresolved: bool,
}
impl Enrollment {
    /// Call immediately after spawn succeeds, before the first await.
    pub(crate) fn arm_owned(&mut self) {
        self.owned_unresolved = true;
    }
    pub(crate) fn settle(&mut self) {
        self.owned_unresolved = false;
    }
}
impl ProcessRegistry {
    pub fn begin_enrollment(self: &std::sync::Arc<Self>) -> io::Result<Enrollment> {
        let mut s = self
            .authority
            .lock()
            .map_err(|_| io::Error::other("authority registry unavailable"))?;
        let enrolling = s
            .enrolling
            .checked_add(1)
            .ok_or_else(|| io::Error::other("enrollment count exhausted"))?;
        let revision = s
            .revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("registry revision exhausted"))?;
        s.enrolling = enrolling;
        s.revision = revision;
        Ok(Enrollment {
            registry: self.clone(),
            owned_unresolved: false,
        })
    }
}
impl Drop for Enrollment {
    fn drop(&mut self) {
        // Cancellation, unreadable birth, wait error and timeout cannot release this barrier.
        // It is deliberately retained for this daemon lifetime; time/count alone is no proof.
        if self.owned_unresolved {
            return;
        }
        if let Ok(mut s) = self.registry.authority.lock() {
            s.enrolling = s.enrolling.saturating_sub(1);
            // If exhausted retain enrolling>0 so no ordinary request can become Human.
            if let Some(next) = s.revision.checked_add(1) {
                s.revision = next;
            } else {
                s.enrolling = 1;
            }
        }
    }
}

#[cfg(test)]
mod s1_enrollment_controls {
    use super::*;
    #[test]
    fn s1_unresolved_owned_enrollment_drop_remains_restrictive() {
        let d = tempfile::tempdir().unwrap();
        let r = std::sync::Arc::new(ProcessRegistry::load(&d.path().join("r.json")).unwrap());
        let mut guard = r.begin_enrollment().unwrap();
        guard.arm_owned();
        drop(guard);
        assert_eq!(r.authority_snapshot().unwrap().enrolling, 1);
        let ordinary = r.begin_enrollment().unwrap();
        drop(ordinary);
        assert_eq!(
            r.authority_snapshot().unwrap().enrolling,
            1,
            "other settled enrollment cannot release unresolved obligation"
        );
    }
    #[test]
    fn s1_explicit_settlement_preserves_original_balanced_enrollment() {
        let d = tempfile::tempdir().unwrap();
        let r = std::sync::Arc::new(ProcessRegistry::load(&d.path().join("r.json")).unwrap());
        let mut guard = r.begin_enrollment().unwrap();
        guard.arm_owned();
        guard.settle();
        drop(guard);
        assert_eq!(r.authority_snapshot().unwrap().enrolling, 0);
        drop(r.begin_enrollment().unwrap());
        assert_eq!(r.authority_snapshot().unwrap().enrolling, 0);
    }
}

// Exists only in explicitly feature-enabled synthetic builds.
#[cfg(feature = "testkit")]
impl ProcessRegistry {
    pub fn test_configure_enrollment_control(&self, path: PathBuf) -> io::Result<()> {
        let path = std::fs::canonicalize(path)?;
        if !path.is_dir() {
            return Err(io::Error::other(
                "synthetic control must be an existing directory",
            ));
        }
        *self
            .test_enrollment_control
            .lock()
            .map_err(|_| io::Error::other("synthetic control unavailable"))? = Some(path.clone());
        test_write_receipt(
            &path.join("configured.json"),
            &serde_json::json!({"daemonPid":std::process::id()}),
        )
    }
    // True driver Found branch before record, with the actual Enrollment still held.
    pub(crate) fn test_pause_before_record(&self, identity: &ProcessIdentity) -> io::Result<()> {
        let path = self
            .test_enrollment_control
            .lock()
            .map_err(|_| io::Error::other("synthetic control unavailable"))?
            .clone();
        let Some(path) = path else { return Ok(()) };
        let snapshot = self.authority_snapshot()?;
        test_write_receipt(
            &path.join("held-birth.json"),
            &serde_json::json!({
                "identity":identity,"enrolling":snapshot.enrolling,"revision":snapshot.revision,"daemonPid":std::process::id()
            }),
        )?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !path.join("release").is_file() {
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "synthetic enrollment release missing",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }
    // Passive parsed-request receipt: no changes to roots, enrollment, role, token or allowlist.
    pub fn test_observe_enrollment_pending(&self, peer: &ProcessIdentity, method: &str) {
        let Ok(control) = self.test_enrollment_control.lock() else {
            return;
        };
        let Some(path) = control.as_ref() else { return };
        // Method comes from the wire: encode it rather than allow path traversal.
        let tag = method
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let _ = test_write_receipt(
            &path.join(format!("pending-{}-{tag}.json", peer.pid)),
            &serde_json::json!({"identity":peer,"method":method,"currentParent":match parent(peer.pid) {
                ParentProbe::Found { identity, parent } if identity.pid==peer.pid && identity.start_time==peer.start_time && identity.boot_id==peer.boot_id => Some(parent),
                _ => None,
            }}),
        );
    }
}
#[cfg(feature = "testkit")]
fn test_write_receipt(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let tmp = path.with_extension("receipt.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    io::Write::write_all(&mut f, &bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}
