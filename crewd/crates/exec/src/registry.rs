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
        command: String::from_utf8_lossy(&comm).into_owned(),
    })
}

#[cfg(not(target_os = "macos"))]
fn os_probe(pid: i32) -> ProbeResult {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(content) => match parse_proc_stat(&content) {
            Some((pgid, start_time, command)) => ProbeResult::Found(ProcessIdentity {
                pid,
                pgid,
                start_time,
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
        ProbeResult::Found(now) if now.start_time == recorded.start_time => OrphanDecision::Reap,
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

/// 执行处置：只有 `Reap` 发信号（先 killpg，失败再单杀）。返回是否发了信号。
pub fn apply(decision: &OrphanDecision, recorded: &ProcessIdentity) -> bool {
    if *decision != OrphanDecision::Reap {
        return false;
    }
    let kill = rustix::process::Signal::KILL;
    let group = rustix::process::Pid::from_raw(if recorded.pgid > 0 {
        recorded.pgid
    } else {
        recorded.pid
    });
    let sent = group.is_some_and(|g| rustix::process::kill_process_group(g, kill).is_ok());
    if !sent && let Some(p) = rustix::process::Pid::from_raw(recorded.pid) {
        let _ = rustix::process::kill_process(p, kill);
    }
    true
}

/// 「如果我下一秒崩了，回来之后该去看哪些 pid」。JSON 文件，0600，整份原子替换。
#[derive(Debug)]
pub struct ProcessRegistry {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, ProcessIdentity>>,
}

impl ProcessRegistry {
    /// 读登记表。文件不存在 = 空表；存在但解析不了 = 错误（不能当空表，否则孤儿就漏了）。
    pub fn load(path: &Path) -> io::Result<Self> {
        let entries = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries: Mutex::new(entries),
        })
    }

    pub fn record(&self, session_id: &str, identity: ProcessIdentity) -> io::Result<()> {
        let mut entries = self.entries.lock().expect("登记表锁中毒");
        entries.insert(session_id.to_string(), identity);
        self.persist(&entries)
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
        std::fs::rename(&tmp, &self.path)
    }
}

/// 一条回收结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanReport {
    pub session_id: String,
    pub decision: OrphanDecision,
    pub message: String,
}

/// 启动时调用（必须持有数据目录锁，确认没有别的 crewd 活着）：逐条判定、执行、清账。
/// 读不到身份的那条留在表里，下次再看。
pub fn reap_orphans(registry: &ProcessRegistry) -> io::Result<Vec<OrphanReport>> {
    let mut reports = Vec::new();
    for (session_id, recorded) in registry.entries() {
        let decision = decide(&recorded, &probe(recorded.pid));
        apply(&decision, &recorded);
        if !matches!(decision, OrphanDecision::Unreadable(_)) {
            registry.forget(&session_id)?;
        }
        reports.push(OrphanReport {
            message: describe(&session_id, &recorded, &decision),
            session_id,
            decision,
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
            command: "claude".to_string(),
        }
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
        assert!(!apply(&OrphanDecision::AlreadyGone, &rec));
        assert!(!apply(
            &OrphanDecision::PidReused(identity(i32::MAX, 2)),
            &rec
        ));
        assert!(!apply(&OrphanDecision::Unreadable(1), &rec));
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
        std::fs::write(&path, b"{not json").unwrap();
        assert!(
            ProcessRegistry::load(&path).is_err(),
            "坏掉的登记表不能当空表"
        );
    }
}
