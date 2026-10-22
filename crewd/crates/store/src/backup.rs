//! 快照、备份与检查（N4：S20 / S21 / S25）。数据只在这台机器上，所以这里**只增不删库**：
//! 检查失败只报告；快照绝不覆盖已有文件；轮转按份数淘汰定时备份、按日子（30 天）淘汰删除前的快照；迁移前 / 手动的快照永远保留。
use crate::{Result, StorageKind, StoreError, fsutil};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 备份目录名（在数据目录里）。
pub const BACKUPS_DIR: &str = "backups";

/// 库文件旁边的备份目录：`<库所在目录>/backups`。
pub fn backups_dir_for(db: &Path) -> PathBuf {
    db.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUPS_DIR)
}

/// 备份目录里有没有库文件（`*.db`，不算临时文件）。目录不在 = 没有；读不了照实报错。
pub fn has_backups(dir: &Path) -> Result<bool> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    for e in rd {
        let name = e?.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".db") && !name.starts_with('.') {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 检查的强度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// `quick_check`：O(N)，不核索引内容。启动、定时备份之后跑。
    Quick,
    /// `integrity_check`：O(N log N)。迁移前跑。
    Integrity,
    /// `integrity_check` + `foreign_key_check`：按需（`crewd check`）。
    Full,
}

impl Check {
    pub fn name(self) -> &'static str {
        match self {
            Check::Quick => "quick_check",
            Check::Integrity => "integrity_check",
            Check::Full => "integrity_check + foreign_key_check",
        }
    }
}

/// 跑一次检查，返回发现的问题（空 = 通过）。SQLite 直接以「损坏 / 不是库」报错的，也记成问题而不是调用失败。
pub fn check(conn: &Connection, kind: Check) -> Result<Vec<String>> {
    let pragma = match kind {
        Check::Quick => "PRAGMA quick_check",
        Check::Integrity | Check::Full => "PRAGMA integrity_check",
    };
    let rows = (|| -> rusqlite::Result<Vec<String>> {
        let mut st = conn.prepare(pragma)?;
        st.query_map([], |r| r.get(0))?.collect()
    })();
    let mut problems: Vec<String> = match rows {
        Ok(rows) => rows.into_iter().filter(|l| l != "ok").collect(),
        Err(e) if StorageKind::of(&e) == Some(StorageKind::Corrupt) => vec![e.to_string()],
        Err(e) => return Err(e.into()),
    };
    if kind == Check::Full && problems.is_empty() {
        let mut st = conn.prepare("PRAGMA foreign_key_check")?;
        let fk = st.query_map([], |r| {
            Ok(format!(
                "外键不成立：{} rowid {:?} → {}",
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?
            ))
        })?;
        for line in fk {
            problems.push(line?);
        }
    }
    Ok(problems)
}

/// 检查不过就是 `StoreError::Integrity`（类别 = 损坏）。
pub fn ensure_ok(conn: &Connection, kind: Check) -> Result<()> {
    let problems = check(conn, kind)?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Integrity {
            check: kind.name(),
            problems,
        })
    }
}

/// 库现在占多少字节（页数 × 页大小）。
pub fn db_bytes(conn: &Connection) -> Result<u64> {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok((pages.max(0) as u64) * (size.max(0) as u64))
}

/// `dir` 所在卷对本用户可用的字节数。
pub fn available_bytes(dir: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` 是以 NUL 结尾的路径，`st` 是可写的 statvfs。
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

/// 文件名里用的 UTC 时间戳（到毫秒，字典序 = 时间序）。
pub fn stamp_from_ms(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%S%3fZ")
        .to_string()
}

/// 建备份目录（0700，和数据目录一样）；已有的只收紧：去掉组和其他人的位（复审 M3）。
fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if dir.is_dir() {
        let mode = std::fs::metadata(dir)?.permissions().mode() & 0o7777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode & !0o077))?;
        }
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// 一致快照（`VACUUM INTO`）：先写同目录的临时文件 → F_FULLFSYNC → 改名 → fsync 目录。
/// 目标已存在就拒绝（绝不覆盖）；可用空间不够约 1.1 倍库大小就不动手。失败时不留半截文件。
/// 不能在事务里调（SQLite 的限制）。
pub fn vacuum_into(conn: &Connection, dest: &Path) -> Result<()> {
    vacuum_into_reserving(conn, dest, 0)
}

/// 同 `vacuum_into`，另外要求拍完之后还剩 `reserve` 字节可用空间。
pub fn vacuum_into_reserving(conn: &Connection, dest: &Path, reserve: u64) -> Result<()> {
    snapshot_steps(conn, dest, reserve, false).map_err(|e| StoreError::Snapshot {
        dest: dest.to_path_buf(),
        source: Box::new(e),
    })
}

fn snapshot_steps(
    conn: &Connection,
    dest: &Path,
    reserve: u64,
    invalidate_remote_trust: bool,
) -> Result<()> {
    let dir = dest
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = dest
        .file_name()
        .ok_or_else(|| StoreError::Invalid("快照路径没有文件名".into()))?
        .to_string_lossy()
        .into_owned();
    ensure_dir(dir)?;
    if dest.exists() {
        return Err(StoreError::Invalid(format!(
            "{} 已存在，快照绝不覆盖",
            dest.display()
        )));
    }
    let needed = (db_bytes(conn)? / 10 * 11 + (1 << 20)).saturating_add(reserve);
    let available = available_bytes(dir)?;
    if available < needed {
        return Err(StoreError::NoSpace { needed, available });
    }
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| -> Result<()> {
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| StoreError::Invalid(format!("路径不是 UTF-8：{}", tmp.display())))?;
        // 先以 0600 建好空文件（SQLite 允许 VACUUM INTO 写进一个空文件）：快照里是全部聊天记录，
        // 任何时刻都不以 umask 给的 0644 存在（复审 M3）。
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
        }
        conn.execute("VACUUM INTO ?1", [tmp_str])?;
        if invalidate_remote_trust {
            // Never install an old active grant even if the process crashes just
            // after rename. Transform only this private staging copy, not the backup.
            let mut copy =
                Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            let mode: String = copy.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
            if !mode.eq_ignore_ascii_case("delete") {
                return Err(StoreError::Invalid(
                    "restore staging journal mode unavailable".into(),
                ));
            }
            copy.execute_batch(if crate::FULLFSYNC {
                "PRAGMA synchronous=FULL; PRAGMA fullfsync=ON; PRAGMA foreign_keys=ON;"
            } else {
                "PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;"
            })?;
            let tx = copy.transaction()?;
            crate::remote::invalidate_for_work_replacement(&tx, "work_restore", &crate::now_ts())?;
            crate::native_execution::invalidate_unknown(&tx)?;
            crate::work_followups::restored(&tx)?;
            crate::command_receipts::restored(&tx, &crate::now_ts())?;
            crate::local_clients::restored(&tx)?;
            crate::judgment_state::restored(&tx)?;
            tx.commit()?;
        }
        // 拍出来的这份自己也要过 quick_check，坏的不改名成正式备份。
        {
            let copy = open_for_check(&tmp)?;
            ensure_ok(&copy, Check::Quick)?;
        }
        std::fs::File::open(&tmp)?.sync_all()?;
        fsutil::rename_durably(&tmp, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn require_file(db: &Path) -> Result<()> {
    if db.exists() {
        Ok(())
    } else {
        Err(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("库文件不存在：{}", db.display()),
        )))
    }
}

/// SQLite READ_ONLY second connection for online snapshots. It does not write
/// SQL rows, but SQLite may create/checkpoint/remove WAL/SHM sidecars.
/// 库不在就报错。**做检查别用它**，见 `open_for_check`。
pub fn open_read_only(db: &Path) -> Result<Connection> {
    require_file(db)?;
    let c = Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// 做 `quick_check` / `integrity_check` 用的第二连接：READ_WRITE 标志（不带 CREATE）+ `query_only`。
/// 不用 `SQLITE_OPEN_READ_ONLY`，是因为 SQLite 在只读打开的库上根本不解析 CHECK 约束
/// （`sqlite3AddCheckConstraint` 跳过只读 btree），检查就看不见违反 CHECK 的行（N4 实测）。
/// `query_only` 让它照样写不了（`VACUUM INTO` 也被挡，所以拍快照用 `open_read_only`）。
pub fn open_for_check(db: &Path) -> Result<Connection> {
    require_file(db)?;
    let c = Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    durable(&c)?;
    c.execute_batch("PRAGMA query_only=ON")?;
    Ok(c)
}

/// restore 挪走旧库之前做检查点用的连接（READ_WRITE、不带 CREATE）。
pub(crate) fn open_for_checkpoint(db: &Path) -> Result<Connection> {
    require_file(db)?;
    let c = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    durable(&c)?;
    Ok(c)
}

/// 能写库文件的第二连接也要和库线程同一档落盘（复审 2）：它若是最后一个关的，关的时候会把 WAL
/// 检查点进主库——那是唯一那份数据。
fn durable(c: &Connection) -> Result<()> {
    c.execute_batch(&format!(
        "PRAGMA synchronous=FULL; PRAGMA checkpoint_fullfsync=ON; PRAGMA fullfsync={};",
        if crate::FULLFSYNC { "ON" } else { "OFF" }
    ))?;
    Ok(())
}

/// 在线备份（S21 / S32）：自己开一条只读连接做 `VACUUM INTO`，不占库线程；快照后过 `quick_check`。
pub fn backup_db(db: &Path, dest: &Path) -> Result<()> {
    let c = open_read_only(db)?;
    vacuum_into(&c, dest)
}

/// Offline backup under the caller-held data-directory lock. Original DB,
/// identity and SQLite sidecars remain untouched; only a verified private copy
/// is opened by SQLite. Online backup retains its concurrent-writer contract.
pub fn backup_db_quiescent(db: &Path, dest: &Path) -> Result<()> {
    require_file(db)?;
    crate::version_guard::with_quiescent_copy(db, |c| vacuum_into(c, dest))
}

/// 定时备份的保留策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// 两份定时备份之间至少隔多久（毫秒）。
    pub min_interval_ms: i64,
    /// 每天留最新的一份，留最近这么多天。
    pub keep_daily: usize,
    /// 每个 ISO 周留最新的一份，留最近这么多周。
    pub keep_weekly: usize,
    /// 拍备份之后至少还要剩这么多可用空间（字节），不够就不拍。
    pub reserve_bytes: u64,
    /// 永久删除前的快照（`crewd-pre-delete-*`）留多久（毫秒，按名字里的时间算）；之后随轮转删掉。
    pub keep_pre_delete_ms: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            min_interval_ms: 24 * 3_600_000,
            keep_daily: 7,
            keep_weekly: 4,
            reserve_bytes: 0,
            keep_pre_delete_ms: 30 * 24 * 3_600_000,
        }
    }
}

const AUTO_PREFIX: &str = "crewd-auto-";

/// 定时备份的文件名（只有这一种会被轮转淘汰）。
pub fn auto_name(ms: i64) -> String {
    format!("{AUTO_PREFIX}{}.db", stamp_from_ms(ms))
}

/// 手动备份（`crewd backup`）的文件名：永不轮转。
pub fn manual_name(ms: i64) -> String {
    format!("crewd-manual-{}.db", stamp_from_ms(ms))
}

fn parse_auto(name: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let stamp = name.strip_prefix(AUTO_PREFIX)?.strip_suffix(".db")?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%3fZ")
        .ok()
        .map(|t| t.and_utc())
}

/// 备份目录里的定时备份，按时间从新到旧。
pub fn list_auto(dir: &Path) -> Result<Vec<(chrono::DateTime<chrono::Utc>, PathBuf)>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut v = vec![];
    for e in rd {
        let e = e?;
        if let Some(t) = parse_auto(&e.file_name().to_string_lossy()) {
            v.push((t, e.path()));
        }
    }
    v.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(v)
}

/// 最新一份定时备份已经比 `min_interval` 旧（或一份都没有）。
pub fn due(dir: &Path, now_ms: i64, policy: Policy) -> Result<bool> {
    Ok(match list_auto(dir)?.first() {
        None => true,
        Some((t, _)) => now_ms - t.timestamp_millis() >= policy.min_interval_ms,
    })
}

/// 轮转（只动 `crewd-auto-*`）：按「有备份的日子」数，每天最新一份留 `keep_daily` 天；比这些更早的，
/// 每个 ISO 周最新一份再留 `keep_weekly` 周；其余删掉。按份数不按日历：crewd 停了几个月，
/// 留下的旧备份也不会因为「太旧」被删光。返回删掉的文件。
pub fn rotate(dir: &Path, policy: Policy) -> Result<Vec<PathBuf>> {
    use chrono::Datelike;
    let all = list_auto(dir)?;
    let mut keep = std::collections::HashSet::new();
    let mut days = Vec::new();
    let mut weeks = Vec::new();
    for (t, p) in &all {
        let day = t.date_naive();
        if days.contains(&day) {
            continue;
        }
        if days.len() < policy.keep_daily {
            days.push(day);
            keep.insert(p.clone());
            continue;
        }
        let week = (t.iso_week().year(), t.iso_week().week());
        if weeks.len() < policy.keep_weekly && !weeks.contains(&week) {
            weeks.push(week);
            keep.insert(p.clone());
        }
    }
    let mut removed = vec![];
    for (_, p) in all {
        if !keep.contains(&p) {
            std::fs::remove_file(&p)?;
            removed.push(p);
        }
    }
    if !removed.is_empty() {
        fsutil::fsync_dir(dir)?;
    }
    Ok(removed)
}

const PRE_DELETE_PREFIX: &str = "crewd-pre-delete-";

/// `crewd-pre-delete-{对象}-{时间}.db` 里的时间（对象 id 里可以有 `-`，时间在最后一个 `-` 之后）。
fn parse_pre_delete(name: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let rest = name.strip_prefix(PRE_DELETE_PREFIX)?.strip_suffix(".db")?;
    let (_, stamp) = rest.rsplit_once('-')?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%3fZ")
        .ok()
        .map(|t| t.and_utc())
}

/// 永久删除前的快照过期（PM 2026-10-07：永久删除真的删，快照只是安全网）：名字里的时间比
/// `now_ms - keep_pre_delete_ms` 还早的删掉。只动 `crewd-pre-delete-*`；名字里看不出时间的不删。
/// 返回删掉的文件。
pub fn expire_pre_delete(dir: &Path, now_ms: i64, policy: Policy) -> Result<Vec<PathBuf>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut removed = vec![];
    for e in rd {
        let e = e?;
        let Some(t) = parse_pre_delete(&e.file_name().to_string_lossy()) else {
            continue;
        };
        if now_ms - t.timestamp_millis() > policy.keep_pre_delete_ms {
            std::fs::remove_file(e.path())?;
            removed.push(e.path());
        }
    }
    if !removed.is_empty() {
        fsutil::fsync_dir(dir)?;
    }
    Ok(removed)
}

/// 一次定时拍的结果。
#[derive(Debug, Default)]
pub struct TickReport {
    /// 在用的库上 `quick_check` 发现的问题（空 = 通过）。
    pub live_problems: Vec<String>,
    /// 这次拍下的备份（不到时间、库是空的就没有）。
    pub backup: Option<PathBuf>,
    /// 到点了却没拍成（库坏了、磁盘满……）。
    pub backup_error: Option<String>,
    /// 轮转删掉的旧定时备份与过期的 pre-delete 快照。
    pub removed: Vec<PathBuf>,
}

/// 定时拍一次（S21 / S25）：先对在用的库跑检查（到点要拍的那次是 `integrity_check`，平时 `quick_check`），到点了就备份（备份文件自己也过
/// `quick_check`），备份成功**且**在用的库没问题才轮转淘汰旧备份（含过期的 pre-delete 快照）。库是空的不拍。
pub fn tick(db: &Path, now_ms: i64, policy: Policy) -> Result<TickReport> {
    let dir = backups_dir_for(db);
    let c = open_read_only(db)?;
    let has_data: i64 = c.query_row(
        "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages)",
        [],
        |r| r.get(0),
    )?;
    let backup_due = has_data > 0 && due(&dir, now_ms, policy)?;
    // 每天拍的那一次跑完整的 integrity_check（367k 条合成消息约 0.3s），平时的整点只跑 quick_check。
    let kind = if backup_due {
        Check::Integrity
    } else {
        Check::Quick
    };
    let mut report = TickReport {
        live_problems: check(&open_for_check(db)?, kind)?,
        ..Default::default()
    };
    if !backup_due {
        return Ok(report);
    }
    let dest = dir.join(auto_name(now_ms));
    match vacuum_into_reserving(&c, &dest, policy.reserve_bytes) {
        Ok(()) => report.backup = Some(dest),
        Err(e) => report.backup_error = Some(e.to_string()),
    }
    if report.backup.is_some() && report.live_problems.is_empty() {
        report.removed = rotate(&dir, policy)?;
        report
            .removed
            .extend(expire_pre_delete(&dir, now_ms, policy)?);
    }
    Ok(report)
}

/// serve 里的定时拍（D-8：serve 自己的 interval，不进编排 Core）。一条自己的线程：启动时先拍一次，
/// 之后每 `every` 看一次到没到点。`stop` 等正在进行的那次做完再返回。
pub struct Ticker {
    stop: std::sync::mpsc::Sender<()>,
    handle: std::thread::JoinHandle<()>,
}

impl Ticker {
    pub fn start(
        db: PathBuf,
        every: std::time::Duration,
        policy: Policy,
    ) -> std::io::Result<Ticker> {
        // 先同步清掉死进程留下的临时文件（复审 2 轮 5：在本线程开拍之前，不和它抢）：backups/ 里的快照
        // 临时文件，和数据目录里中断的 restore 留下的 `.crewd.db.tmp-<pid>`。清不掉只记一笔，不挡启动。
        let data_dir = db
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        for dir in [backups_dir_for(&db), data_dir] {
            match sweep_stale_tmp(&dir) {
                Ok(gone) => {
                    for p in gone {
                        eprintln!("crewd: 清掉上次没做完的临时文件 {}", p.display());
                    }
                }
                Err(e) => eprintln!("crewd: 清理 {} 里的临时文件失败：{e}", dir.display()),
            }
        }
        let (stop, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::Builder::new()
            .name("crewd-backup".into())
            .spawn(move || {
                loop {
                    log_tick(tick(&db, crate::now_ms(), policy));
                    match rx.recv_timeout(every) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        _ => break,
                    }
                }
            })?;
        Ok(Ticker { stop, handle })
    }

    /// 停下并等线程退出（阻塞；async 里用 `spawn_blocking`）。
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.handle.join();
    }
}

fn log_tick(r: Result<TickReport>) {
    match r {
        Ok(r) => {
            if !r.live_problems.is_empty() {
                eprintln!(
                    "crewd: 数据库没通过 quick_check（不删、不替换；旧备份暂停淘汰）：{}",
                    r.live_problems
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("；")
                );
            }
            if let Some(p) = r.backup {
                eprintln!("crewd: 已备份到 {}", p.display());
            }
            if let Some(e) = r.backup_error {
                eprintln!("crewd: 定时备份没拍成（旧备份一份不删）：{e}");
            }
            for p in r.removed {
                eprintln!("crewd: 轮转删掉旧备份 {}", p.display());
            }
        }
        Err(e) => eprintln!("crewd: 定时备份失败：{e}"),
    }
}

/// 清掉 `dir` 里死进程留下的快照临时文件（`.{名字}.tmp-{pid}`）。pid 还活着的不动（可能是正在跑的
/// `crewd backup`）；自己进程的也算残留（Ticker 启动时本进程还没开始拍）。返回删掉的文件。
pub fn sweep_stale_tmp(dir: &Path) -> Result<Vec<PathBuf>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let me = std::process::id() as i32;
    let mut removed = vec![];
    for e in rd {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        // `.X.tmp-<pid>`，以及 SQLite 给它建的 `.X.tmp-<pid>-wal` / `-shm` / `-journal`。
        let Some(pid) = name
            .strip_prefix('.')
            .and_then(|n| n.rsplit_once(".tmp-"))
            .map(|(_, rest)| {
                ["-wal", "-shm", "-journal"]
                    .iter()
                    .find_map(|suf| rest.strip_suffix(suf))
                    .unwrap_or(rest)
            })
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == me || !process_alive(pid) {
            std::fs::remove_file(e.path())?;
            removed.push(e.path());
        }
    }
    if !removed.is_empty() {
        fsutil::fsync_dir(dir)?;
    }
    removed.sort();
    Ok(removed)
}

/// `kill(pid, 0)`：ESRCH = 没有这个进程；EPERM = 有，只是不是我们的。
fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: 信号 0 不发送任何信号，只做存在与权限检查。
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// 一次恢复的结果。
#[derive(Debug)]
pub struct RestoreReport {
    /// 原来的库被挪到了哪里（原来没有库就是 `None`）。
    pub replaced: Option<PathBuf>,
}

/// 从备份恢复（S21）。**调用方必须先拿到数据目录锁**（serve 在跑时不能恢复）。
/// 先核来源（是 crewd 的库、版本不比本程序新、`integrity_check` 通过、不是在用的库自己）；
/// 原来的库（连同 -wal / -shm）改名为 `crewd.db.replaced-{时间}`，永不覆盖；再把来源 `VACUUM INTO` 成新的
/// `crewd.db`，标识文件改成来源的 dbId。
pub fn restore(data_dir: &Path, from: &Path) -> Result<RestoreReport> {
    let db = data_dir.join(crate::DB_FILE);
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(from), std::fs::canonicalize(&db))
        && a == b
    {
        return Err(StoreError::Invalid("不能用在用的库恢复它自己".into()));
    }
    let src = open_read_only(from)?;
    let app: i32 = src.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = src.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if (app != 0 && app != crate::APPLICATION_ID)
        || version == 16
        || !(1..=crate::schema::SUPPORTED_SCHEMA_VERSION).contains(&version)
    {
        return Err(StoreError::Invalid(format!(
            "{} 不是本程序能用的 crewd 库（application_id={app}，user_version={version}）",
            from.display()
        )));
    }
    if crate::meta_get(&src, "knowledge.sqlite.v1")?.as_deref() == Some("1")
        || data_dir.join("knowledge").exists()
    {
        return Err(StoreError::Invalid("Core-only restore cannot replace a knowledge workspace; restore a knowledge snapshot under current ACL".into()));
    }
    // A standalone SQLite restore must never split the Core16/native authority.
    // Its owning paired controller validates/copies the complete bundle into a
    // fresh private stage before any original file is touched.
    let native_present = match std::fs::symlink_metadata(data_dir.join("native-data")) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if version == 16 || native_present {
        return Err(StoreError::Invalid(
            "paired Core16/native-data restore required; legacy standalone restore refused".into(),
        ));
    }
    if db.try_exists()?
        && crate::version_guard::with_quiescent_copy(&db, |c| {
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .map_err(StoreError::from)
        })? == 16
    {
        return Err(StoreError::Invalid(
            "pre16 backup cannot replace a native authority in place; use an isolated target"
                .into(),
        ));
    }
    ensure_ok(&open_for_check(from)?, Check::Integrity)?;
    let src_id = crate::meta_get(&src, "dbId")?;

    let side = |suffix: &str| data_dir.join(format!("{}{suffix}", crate::DB_FILE));
    let db_present = db.exists();
    if db_present {
        // 先把 WAL 折回主库，挪走的那份自成一体；做不成也照样连 -wal / -shm 一起挪（名字跟着改）。
        if let Ok(c) = open_for_checkpoint(&db) {
            let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
    }
    let aside = data_dir.join(format!(
        "{}.replaced-{}",
        crate::DB_FILE,
        stamp_from_ms(crate::now_ms())
    ));
    if aside.exists() {
        return Err(StoreError::Invalid(format!("{} 已存在", aside.display())));
    }
    // 不管库文件在不在，残留的 -wal / -shm 一律挪开（复审 2 轮 2）：库不在时崩溃留下的旧 WAL
    // 会被 SQLite 回放到恢复出来的新文件上，把它弄坏。
    for suffix in ["-wal", "-shm"] {
        if side(suffix).exists() {
            let mut to = aside.as_os_str().to_owned();
            to.push(suffix);
            fsutil::rename_durably(&side(suffix), Path::new(&to))?;
        }
    }
    let replaced = if db_present {
        fsutil::rename_durably(&db, &aside)?;
        Some(aside)
    } else {
        None
    };
    snapshot_steps(&src, &db, 0, true).map_err(|e| StoreError::Snapshot {
        dest: db.clone(),
        source: Box::new(e),
    })?;
    let id_path = data_dir.join(crate::ID_FILE);
    match src_id {
        Some(id) => fsutil::atomic_write(&id_path, format!("{id}\n").as_bytes(), 0o600)?,
        // 没有 dbId 的旧库：去掉标识文件，下次打开时认领。
        None => match std::fs::remove_file(&id_path) {
            Ok(()) => fsutil::fsync_dir(data_dir)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(RestoreReport { replaced })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma(c: &Connection, name: &str) -> i64 {
        c.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    }

    /// 复审 2：会写库文件的第二连接（检查连接关掉时如果它是最后一个，会把 WAL 检查点进唯一那份库；
    /// restore 前的检查点连接）也要 F_FULLFSYNC，不能退回编译期默认。
    #[test]
    fn second_connections_that_may_checkpoint_are_durable() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("crewd.db");
        drop(crate::Store::open(&db).unwrap());
        for c in [
            open_for_check(&db).unwrap(),
            open_for_checkpoint(&db).unwrap(),
        ] {
            assert_eq!(pragma(&c, "synchronous"), 2);
            assert_eq!(pragma(&c, "fullfsync"), crate::FULLFSYNC as i64);
            assert_eq!(pragma(&c, "checkpoint_fullfsync"), 1);
        }
        assert_eq!(pragma(&open_for_check(&db).unwrap(), "query_only"), 1);
    }
}

/// Staging primitive used only after the owning paired controller has verified
/// the complete Core/native bundle. It never replaces an existing database and
/// applies the ORIGINAL restore trust/receipt/epoch invalidation transaction.
#[cfg(any())]
pub fn native_restore_stage(from: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let parent = destination
        .parent()
        .ok_or_else(|| StoreError::Invalid("native restore stage parent missing".into()))?;
    let m = std::fs::symlink_metadata(parent)?;
    if !parent.is_absolute()
        || std::fs::canonicalize(parent)? != parent
        || !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o700
        || destination.file_name() != Some(std::ffi::OsStr::new(crate::DB_FILE))
        || match std::fs::symlink_metadata(destination) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        }
    {
        return Err(StoreError::Invalid(
            "fresh private native restore stage required".into(),
        ));
    }
    let source = open_read_only(from)?;
    let version: i64 = source.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 16 {
        return Err(StoreError::Invalid(
            "paired native restore stage requires Core16".into(),
        ));
    }
    crate::team_data::validate_active(&source)?;
    ensure_ok(&source, Check::Integrity)?;
    snapshot_steps(&source, destination, 0, true)
}

#[cfg(test)]
mod sqlite_knowledge_restore_guard_tests {
    use super::*;
    #[test]
    fn sqlite_core_only_restore_cannot_roll_back_acl_beside_knowledge_files() {
        let dir = tempfile::tempdir().unwrap();
        let data = std::fs::canonicalize(dir.path()).unwrap();
        let db = data.join(crate::DB_FILE);
        let backup = data.join("core-before.sqlite");
        let store = crate::Store::open(&db).unwrap();
        crate::meta_set(store.conn(), "fixture.currentAcl", "old").unwrap();
        drop(store);
        backup_db(&db, &backup).unwrap();
        let store = crate::Store::open(&db).unwrap();
        crate::meta_set(store.conn(), "fixture.currentAcl", "current").unwrap();
        drop(store);
        crate::team_data::private_dir(&data.join("knowledge")).unwrap();
        assert!(restore(&data, &backup).is_err());
        let store = crate::Store::open(&db).unwrap();
        assert_eq!(
            crate::meta_get(store.conn(), "fixture.currentAcl")
                .unwrap()
                .as_deref(),
            Some("current")
        );
        assert_eq!(
            store
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            15
        );
    }
}
