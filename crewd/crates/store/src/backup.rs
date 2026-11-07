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
    snapshot_steps(conn, dest, reserve, None, false).map_err(|e| StoreError::Snapshot {
        dest: dest.to_path_buf(),
        source: Box::new(e),
    })
}

fn snapshot_steps(
    conn: &Connection,
    dest: &Path,
    reserve: u64,
    restore_floor: Option<i64>,
    qualify_empty_knowledge: bool,
) -> Result<()> {
    snapshot_steps_with_floors(
        conn,
        dest,
        reserve,
        restore_floor,
        None,
        None,
        qualify_empty_knowledge,
    )
}

fn snapshot_steps_with_floors(
    conn: &Connection,
    dest: &Path,
    reserve: u64,
    restore_floor: Option<i64>,
    field_floor: Option<i64>,
    option_floor: Option<i64>,
    qualify_empty_knowledge: bool,
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
        if restore_floor.is_some()
            || field_floor.is_some()
            || option_floor.is_some()
            || qualify_empty_knowledge
        {
            // Never install an old active grant even if the process crashes just
            // after rename. Transform only this private staging copy, not the backup.
            let mut copy =
                Connection::open_with_flags(&tmp, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            crate::search_context::ensure_registered(&copy)?;
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
            clear_empty_knowledge_cut(&tx)?;
            if let Some(floor) = restore_floor {
                crate::remote::invalidate_for_work_replacement(
                    &tx,
                    "work_restore",
                    &crate::now_ts(),
                )?;
                crate::native_execution::invalidate_unknown(&tx)?;
                crate::work_followups::restored(&tx)?;
                crate::command_receipts::restored(&tx, &crate::now_ts())?;
                crate::local_clients::restored(&tx)?;
                crate::judgment_state::restored(&tx)?;
                crate::personal_drafts::restored(&tx)?;
                crate::organization_search::restore(&tx)?;
                crate::notifications::restored(&tx)?;
                crate::codex_inputs::restored(&tx)?;
                crate::codex_controls::restored(&tx)?;
                if floor > 0 {
                    crate::object_refs::preserve_restore_floor(&tx, floor)?;
                }
            } else {
                record_empty_knowledge_cut(&tx)?;
            }
            if let Some(floor) = field_floor {
                crate::managed_field_allocator::preserve_restore_floor(&tx, floor)?;
            }
            if let Some(floor) = option_floor {
                crate::managed_tables_v3::preserve_option_restore_floor(&tx, floor)?;
            }
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
    crate::search_context::ensure_registered(&c)?;
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
    crate::search_context::ensure_registered(&c)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    durable(&c)?;
    c.execute_batch("PRAGMA query_only=ON")?;
    Ok(c)
}

/// restore 挪走旧库之前做检查点用的连接（READ_WRITE、不带 CREATE）。
pub(crate) fn open_for_checkpoint(db: &Path) -> Result<Connection> {
    require_file(db)?;
    let c = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::search_context::ensure_registered(&c)?;
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
    let data = db.parent().unwrap_or(Path::new("."));
    crate::version_guard::with_quiescent_copy(db, |c| {
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let qualifies = db.file_name().and_then(|n| n.to_str()) == Some(crate::DB_FILE)
            && version >= 23
            && empty_knowledge_root(data)?;
        if qualifies {
            snapshot_steps(c, dest, 0, None, true).map_err(|e| StoreError::Snapshot {
                dest: dest.to_path_buf(),
                source: Box::new(e),
            })
        } else {
            vacuum_into(c, dest)
        }
    })
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
    tick_inner(db, now_ms, policy, None)
}

fn tick_inner(
    db: &Path,
    now_ms: i64,
    policy: Policy,
    health: Option<&crate::db_health::DbHealth>,
) -> Result<TickReport> {
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
    // This is the result of the exact known SQLite Check operation, never arbitrary text classification.
    if !report.live_problems.is_empty()
        && let Some(health) = health
    {
        health.record(
            crate::db_health::DbFaultStage::Maintenance,
            crate::db_health::DbFaultKind::Integrity,
        );
    }
    if !backup_due {
        return Ok(report);
    }
    let dest = dir.join(auto_name(now_ms));
    match vacuum_into_reserving(&c, &dest, policy.reserve_bytes) {
        Ok(()) => report.backup = Some(dest),
        Err(e) => {
            if let Some(health) = health {
                health.observe(crate::db_health::DbFaultStage::Maintenance, &e);
            }
            report.backup_error = Some(e.to_string());
        }
    }
    if report.backup.is_some() && report.live_problems.is_empty() {
        report.removed = rotate(&dir, policy)?;
        report
            .removed
            .extend(expire_pre_delete(&dir, now_ms, policy)?);
        report
            .removed
            .extend(crate::knowledge_purge::expire_completed(
                &dir, now_ms, policy,
            )?);
    }
    Ok(report)
}

/// The same tick algorithm, observing typed failures into the daemon-owned shared history.
pub fn tick_with_health(
    db: &Path,
    now_ms: i64,
    policy: Policy,
    health: &crate::db_health::DbHealth,
) -> Result<TickReport> {
    let result = tick_inner(db, now_ms, policy, Some(health));
    // Early/check/rotation errors escape once; backup-error strings remain Ok and are not reclassified.
    if let Err(error) = &result {
        health.observe(crate::db_health::DbFaultStage::Maintenance, error);
    }
    result
}

/// serve 里的定时拍（D-8：serve 自己的 interval，不进编排 Core）。一条自己的线程：启动时先拍一次，
/// 之后每 `every` 看一次到没到点。`stop` 等正在进行的那次做完再返回。
/// Receipt from the actual worker stack. Final termination still requires the real thread join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickerExit {
    RequestedStop,
    Disconnected,
    UnexpectedReturn,
    Panicked,
}
#[derive(Debug, thiserror::Error)]
pub enum TickerStopError {
    #[error("backup worker panicked")]
    Panicked,
}
struct TickerExitGuard {
    sender: Option<tokio::sync::oneshot::Sender<TickerExit>>,
    reason: TickerExit,
}
impl Drop for TickerExitGuard {
    fn drop(&mut self) {
        let reason = if std::thread::panicking() {
            TickerExit::Panicked
        } else {
            self.reason
        };
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(reason);
        }
    }
}
enum TickerCommand {
    Stop,
    #[cfg(feature = "testkit")]
    Panic,
    #[cfg(feature = "testkit")]
    Return,
}
#[cfg(feature = "testkit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickerTestFault {
    PanicEntered,
    ReturnEntered,
}
/// Owned synthetic controls, never a daemon wire method or ordinary-build API.
#[cfg(feature = "testkit")]
pub struct TickerTestControl {
    sender: std::sync::mpsc::Sender<TickerCommand>,
    first_tick: Option<tokio::sync::oneshot::Receiver<()>>,
    fault_entered: Option<tokio::sync::oneshot::Receiver<TickerTestFault>>,
    joined: Option<tokio::sync::oneshot::Receiver<bool>>,
}
#[cfg(feature = "testkit")]
impl TickerTestControl {
    pub fn panic_worker(&self) -> std::result::Result<(), String> {
        self.sender
            .send(TickerCommand::Panic)
            .map_err(|_| "actual worker command channel closed".to_owned())
    }
    pub fn return_worker(&self) -> std::result::Result<(), String> {
        self.sender
            .send(TickerCommand::Return)
            .map_err(|_| "actual worker command channel closed".to_owned())
    }
    pub async fn first_tick(&mut self) -> std::result::Result<(), String> {
        self.first_tick
            .take()
            .ok_or_else(|| "first tick witness already taken".to_owned())?
            .await
            .map_err(|_| "actual worker ended before first tick".to_owned())
    }
    pub async fn fault_entered(&mut self) -> std::result::Result<TickerTestFault, String> {
        self.fault_entered
            .take()
            .ok_or_else(|| "fault entry witness already taken".to_owned())?
            .await
            .map_err(|_| "actual fault branch was not reached".to_owned())
    }
    pub async fn joined(&mut self) -> std::result::Result<bool, String> {
        self.joined
            .take()
            .ok_or_else(|| "join witness already taken".to_owned())?
            .await
            .map_err(|_| "real worker join was not reported".to_owned())
    }
}

pub struct Ticker {
    stop: std::sync::mpsc::Sender<TickerCommand>,
    handle: std::thread::JoinHandle<()>,
    exit: Option<tokio::sync::oneshot::Receiver<TickerExit>>,
    #[cfg(feature = "testkit")]
    control: Option<TickerTestControl>,
    #[cfg(feature = "testkit")]
    join_witness: Option<tokio::sync::oneshot::Sender<bool>>,
}

impl Ticker {
    pub fn take_exit_signal(&mut self) -> Option<tokio::sync::oneshot::Receiver<TickerExit>> {
        self.exit.take()
    }
    /// Daemon checked close; standalone stop() remains compatible and ignores this result.
    pub fn stop_checked(self) -> std::result::Result<(), TickerStopError> {
        self.stop_result().map_err(|_| TickerStopError::Panicked)
    }
    #[cfg(feature = "testkit")]
    pub fn take_test_control(&mut self) -> Option<TickerTestControl> {
        self.control.take()
    }

    /// Same worker with the daemon's sole DB-independent history.
    pub fn start_with_health(
        db: PathBuf,
        every: std::time::Duration,
        policy: Policy,
        health: crate::db_health::DbHealth,
    ) -> std::io::Result<Ticker> {
        Self::start_inner(db, every, policy, Some(health))
    }

    pub fn start(
        db: PathBuf,
        every: std::time::Duration,
        policy: Policy,
    ) -> std::io::Result<Ticker> {
        Self::start_inner(db, every, policy, None)
    }

    fn start_inner(
        db: PathBuf,
        every: std::time::Duration,
        policy: Policy,
        health: Option<crate::db_health::DbHealth>,
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
                Err(e) => {
                    if let Some(health) = &health {
                        health.observe(crate::db_health::DbFaultStage::Maintenance, &e);
                    }
                    eprintln!("crewd: 清理 {} 里的临时文件失败：{e}", dir.display());
                }
            }
        }
        let (stop, rx) = std::sync::mpsc::channel::<TickerCommand>();
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
        #[cfg(feature = "testkit")]
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        #[cfg(feature = "testkit")]
        let (fault_tx, fault_rx) = tokio::sync::oneshot::channel();
        #[cfg(feature = "testkit")]
        let (join_tx, join_rx) = tokio::sync::oneshot::channel();
        let spawn_health = health.clone();
        let handle = std::thread::Builder::new()
            .name("crewd-backup".into())
            .spawn(move || {
                // Named guard is held on this actual worker stack, never the launcher/observer.
                let mut exit = TickerExitGuard {
                    sender: Some(exit_tx),
                    reason: TickerExit::UnexpectedReturn,
                };
                #[cfg(feature = "testkit")]
                let mut first_tx = Some(first_tx);
                #[cfg(feature = "testkit")]
                let mut fault_tx = Some(fault_tx);
                loop {
                    let result = match &health {
                        Some(health) => tick_with_health(&db, crate::now_ms(), policy, health),
                        None => tick(&db, crate::now_ms(), policy),
                    };
                    log_tick(result);
                    #[cfg(feature = "testkit")]
                    if let Some(tx) = first_tx.take() {
                        let _ = tx.send(());
                    }
                    match rx.recv_timeout(every) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Ok(TickerCommand::Stop) => {
                            exit.reason = TickerExit::RequestedStop;
                            break;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            exit.reason = TickerExit::Disconnected;
                            break;
                        }
                        #[cfg(feature = "testkit")]
                        Ok(TickerCommand::Panic) => {
                            if let Some(tx) = fault_tx.take() {
                                let _ = tx.send(TickerTestFault::PanicEntered);
                            }
                            panic!("S4_OWNED_ACTUAL_BACKUP_WORKER_PANIC");
                        }
                        #[cfg(feature = "testkit")]
                        Ok(TickerCommand::Return) => {
                            if let Some(tx) = fault_tx.take() {
                                let _ = tx.send(TickerTestFault::ReturnEntered);
                            }
                            return;
                        }
                    }
                }
            });
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                if let Some(health) = &spawn_health {
                    health.observe(crate::db_health::DbFaultStage::Maintenance, &error);
                }
                return Err(error);
            }
        };
        #[cfg(feature = "testkit")]
        let control = TickerTestControl {
            sender: stop.clone(),
            first_tick: Some(first_rx),
            fault_entered: Some(fault_rx),
            joined: Some(join_rx),
        };
        Ok(Ticker {
            stop,
            handle,
            exit: Some(exit_rx),
            #[cfg(feature = "testkit")]
            control: Some(control),
            #[cfg(feature = "testkit")]
            join_witness: Some(join_tx),
        })
    }

    /// 停下并等线程退出（阻塞；async 里用 `spawn_blocking`）。
    pub fn stop(self) {
        let _ = self.stop_result();
    }

    // Same real join; only tests in this module inspect its result. Serve semantics unchanged.
    fn stop_result(self) -> std::thread::Result<()> {
        let _ = self.stop.send(TickerCommand::Stop);
        let result = self.handle.join();
        #[cfg(feature = "testkit")]
        if let Some(witness) = self.join_witness {
            let _ = witness.send(result.is_ok());
        }
        result
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

const EMPTY_KNOWLEDGE_CUT: &str = "backup.emptyKnowledgeCut.v1";

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EmptyKnowledgeCut {
    database: String,
    restore_epoch: String,
    floor: i64,
}

fn empty_knowledge_root(data: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let data = std::fs::canonicalize(data)?;
    if crate::knowledge_purge::has_recovery_material(&data)? {
        return Ok(false);
    }
    let root = data.join("knowledge");
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_dir()
        || metadata.mode() & 0o7777 != 0o700
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Ok(false);
    }
    Ok(std::fs::read_dir(root)?.next().transpose()?.is_none())
}

fn observed_empty_knowledge_cut(c: &Connection) -> Result<EmptyKnowledgeCut> {
    let database = crate::meta_get(c, "dbId")?
        .filter(|id| !id.is_empty())
        .ok_or_else(|| StoreError::Invalid("offline backup database identity missing".into()))?;
    Ok(EmptyKnowledgeCut {
        database,
        restore_epoch: crate::personal_drafts::epoch(c)?,
        floor: crate::object_refs::allocator_floor(c)?,
    })
}
fn record_empty_knowledge_cut(c: &Connection) -> Result<()> {
    crate::meta_set(
        c,
        EMPTY_KNOWLEDGE_CUT,
        &serde_json::to_string(&observed_empty_knowledge_cut(c)?)?,
    )
}
fn qualified_empty_knowledge_cut(c: &Connection, source: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Ok(false);
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = source.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::symlink_metadata(Path::new(&name)) {
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    let Some(raw) = crate::meta_get(c, EMPTY_KNOWLEDGE_CUT)? else {
        return Ok(false);
    };
    if raw.len() > 1024 {
        return Ok(false);
    }
    let Ok(captured) = serde_json::from_str::<EmptyKnowledgeCut>(&raw) else {
        return Ok(false);
    };
    Ok(captured == observed_empty_knowledge_cut(c)?)
}

pub(crate) fn clear_empty_knowledge_cut(c: &Connection) -> Result<()> {
    // A live Core must never retain a snapshot-only attestation.
    if crate::meta_get(c, EMPTY_KNOWLEDGE_CUT)?.is_some() {
        c.execute("DELETE FROM meta WHERE key=?1", [EMPTY_KNOWLEDGE_CUT])?;
    }
    Ok(())
}

// Local monotone proof survives interruption between archiving the old
// database and publishing its verified replacement. It contains one integer,
// no crew identity, receipt, or content from the discarded generation.
const RESTORE_ALLOCATOR_WITNESS: &str = ".object-refs-restore-floor.v1";

pub(crate) fn restore_allocator_floor(data_dir: &Path) -> Result<i64> {
    let path = data_dir.join(RESTORE_ALLOCATOR_WITNESS);
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(StoreError::Invalid(
            "unqualified object reference restore floor".into(),
        ));
    }
    let mut raw = String::new();
    file.take(24).read_to_string(&mut raw)?;
    crate::object_refs::parse_floor(raw.strip_suffix('\n').unwrap_or(&raw))
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
    // Qualify the supported H33 format on a private cold/WAL copy before a
    // read-only SQLite open can create sidecars or any target path is touched.
    crate::version_guard::preflight(from)?.recheck()?;
    let src = open_read_only(from)?;
    let app: i32 = src.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = src.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 33 {
        src.pragma_update(None, "foreign_keys", true)?;
        crate::admin_ask_groups::validate_schema(&src)?;
    }
    if (app != 0 && app != crate::APPLICATION_ID)
        || version == 16
        || !(1..=crate::schema::SUPPORTED_SCHEMA_VERSION).contains(&version)
    {
        return Err(StoreError::Invalid(format!(
            "{} 不是本程序能用的 crewd 库（application_id={app}，user_version={version}）",
            from.display()
        )));
    }
    let source_enabled = crate::meta_get(&src, "knowledge.sqlite.v1")?.as_deref() == Some("1");
    let target_has_root = match std::fs::symlink_metadata(data_dir.join("knowledge")) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if !empty_knowledge_root(data_dir)?
        || ((source_enabled || target_has_root) && !qualified_empty_knowledge_cut(&src, from)?)
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
    ensure_ok(&open_for_check(from)?, Check::Integrity)?;
    let src_id = crate::meta_get(&src, "dbId")?;
    let floor_witness = data_dir.join(RESTORE_ALLOCATOR_WITNESS);
    let recorded = restore_allocator_floor(data_dir)?;
    let source_floor = crate::object_refs::allocator_floor(&src)?;
    let recorded_field_floor = crate::managed_field_allocator::witness_floor(data_dir)?;
    let source_field_floor = crate::managed_field_allocator::allocator_floor(&src)?;
    let recorded_option_floor = crate::managed_tables_v3::option_witness_floor(data_dir)?;
    let source_option_floor = crate::managed_tables_v3::option_allocator_floor(&src)?;
    let (live_id, live_floor, live_field_floor, live_option_floor) = if db.try_exists()? {
        crate::version_guard::with_quiescent_copy(&db, |current| {
            Ok((
                crate::meta_get(current, "dbId")?,
                crate::object_refs::allocator_floor(current)?,
                crate::managed_field_allocator::allocator_floor(current)?,
                crate::managed_tables_v3::option_allocator_floor(current)?,
            ))
        })?
    } else {
        // A failed cut may already have moved the predecessor. Its identity
        // record remains until a replacement has been durably published.
        let recorded = crate::read_id_file(&data_dir.join(crate::ID_FILE))?.map(|id| id.id);
        (recorded, 0, 0, 0)
    };
    if target_has_root && (src_id.is_none() || live_id != src_id) {
        return Err(StoreError::Invalid(
            "offline empty knowledge cut belongs to another Core authority".into(),
        ));
    }
    let floor = recorded.max(source_floor).max(live_floor);
    let field_floor = recorded_field_floor
        .max(source_field_floor)
        .max(live_field_floor);
    let option_floor = recorded_option_floor
        .max(source_option_floor)
        .max(live_option_floor);

    let aside = data_dir.join(format!(
        "{}.replaced-{}",
        crate::DB_FILE,
        stamp_from_ms(crate::now_ms())
    ));
    if aside.exists() {
        return Err(StoreError::Invalid(format!("{} 已存在", aside.display())));
    }
    let stage = data_dir.join(format!(".core-restore-{}.db", uuid::Uuid::new_v4()));
    let cut = (|| -> Result<Option<PathBuf>> {
        // Finish and verify the entire replacement before changing any active
        // path. Its allocator floor and restore stamp share one SQLite commit.
        snapshot_steps_with_floors(
            &src,
            &stage,
            0,
            Some(floor),
            Some(field_floor),
            Some(option_floor),
            false,
        )
        .map_err(|e| StoreError::Snapshot {
            dest: db.clone(),
            source: Box::new(e),
        })?;
        if floor > recorded {
            fsutil::atomic_write(&floor_witness, format!("{floor}\n").as_bytes(), 0o600)?;
        }
        crate::managed_field_allocator::persist_witness(data_dir, field_floor)?;
        crate::managed_tables_v3::persist_option_witness(data_dir, option_floor)?;
        let db_present = db.exists();
        if db_present {
            // Preserve the previous physical authority and any committed WAL.
            if let Ok(c) = open_for_checkpoint(&db) {
                let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            }
        }
        for suffix in ["-wal", "-shm"] {
            let side = data_dir.join(format!("{}{suffix}", crate::DB_FILE));
            if side.exists() {
                let mut to = aside.as_os_str().to_owned();
                to.push(suffix);
                fsutil::rename_durably(&side, Path::new(&to))?;
            }
        }
        let replaced = if db_present {
            fsutil::rename_durably(&db, &aside)?;
            Some(aside)
        } else {
            None
        };
        fsutil::rename_durably(&stage, &db)?;
        Ok(replaced)
    })();
    if cut.is_err() && stage.exists() {
        let _ = std::fs::remove_file(&stage);
    }
    let replaced = cut?;
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

    #[test]
    fn served_core_backup_attests_only_the_offline_empty_knowledge_cut() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().canonicalize().unwrap();
        let live = data.join(crate::DB_FILE);
        let online = data.join("unqualified-online.db");
        let qualified = data.join("qualified-offline.db");
        let dirty = data.join("unqualified-staging.db");
        let knowledge = data.join("knowledge");
        let s = crate::Store::open_data_dir(&data).unwrap();
        crate::meta_set(s.conn(), "knowledge.sqlite.v1", "1").unwrap();
        crate::meta_set(s.conn(), "fixture.cut", "old").unwrap();
        crate::team_data::private_dir(&knowledge).unwrap();
        drop(s);
        backup_db(&live, &online).unwrap();
        assert!(restore(&data, &online).is_err());
        backup_db_quiescent(&live, &qualified).unwrap();
        let original = std::fs::read(&qualified).unwrap();

        let pending = knowledge.join("unknown.stage");
        std::fs::write(&pending, b"private fixture").unwrap();
        backup_db_quiescent(&live, &dirty).unwrap();
        assert!(restore(&data, &qualified).is_err());
        assert_eq!(std::fs::read(&pending).unwrap(), b"private fixture");
        std::fs::remove_file(&pending).unwrap();
        assert!(restore(&data, &dirty).is_err());
        let s = crate::Store::open_data_dir(&data).unwrap();
        crate::meta_set(s.conn(), "fixture.cut", "current").unwrap();
        drop(s);

        restore(&data, &qualified).unwrap();
        assert_eq!(std::fs::read(&qualified).unwrap(), original);
        let s = crate::Store::open_data_dir(&data).unwrap();
        assert_eq!(
            crate::meta_get(s.conn(), "fixture.cut").unwrap().as_deref(),
            Some("old")
        );
        assert!(
            crate::meta_get(s.conn(), EMPTY_KNOWLEDGE_CUT)
                .unwrap()
                .is_none()
        );
        assert!(std::fs::read_dir(&knowledge).unwrap().next().is_none());
    }

    #[test]
    fn restore_floor_survives_an_interrupted_publish_with_no_active_database() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join(crate::DB_FILE);
        let source = dir.path().join("old-backup.db");
        let store = crate::Store::open_data_dir(dir.path()).unwrap();
        vacuum_into(store.conn(), &source).unwrap();
        drop(store);
        let held = dir.path().join("predecessor.db");
        std::fs::rename(&active, &held).unwrap();
        fsutil::atomic_write(&dir.path().join(RESTORE_ALLOCATOR_WITNESS), b"73\n", 0o600).unwrap();

        let restored = restore(dir.path(), &source).unwrap();
        assert!(restored.replaced.is_none());
        let store = crate::Store::open_data_dir(dir.path()).unwrap();
        assert_eq!(
            crate::object_refs::allocator_floor(store.conn()).unwrap(),
            73
        );
        assert!(held.is_file());
    }

    #[test]
    fn corrupt_restore_floor_refuses_without_moving_active_database() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join(crate::DB_FILE);
        let source = dir.path().join("old-backup.db");
        let store = crate::Store::open_data_dir(dir.path()).unwrap();
        vacuum_into(store.conn(), &source).unwrap();
        drop(store);
        let before = std::fs::read(&active).unwrap();
        fsutil::atomic_write(&dir.path().join(RESTORE_ALLOCATOR_WITNESS), b"bad\n", 0o600).unwrap();
        assert!(restore(dir.path(), &source).is_err());
        assert_eq!(std::fs::read(&active).unwrap(), before);
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|item| {
            item.unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".replaced-")
        }));
    }

    fn s4_maintenance_fixture(data: bool) -> (tempfile::TempDir, PathBuf, crate::Db) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::DB_FILE);
        let store = crate::Store::open_data_dir(dir.path()).unwrap();
        if data {
            crate::crews::upsert(
                store.conn(),
                &crewd_proto::Crew {
                    id: "s4-maintenance-crew".into(),
                    title: "synthetic".into(),
                    created_at: "2026-10-01T00:00:00.000Z".into(),
                    updated_at: "2026-10-01T00:00:00.000Z".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        let db = crate::Db::spawn(store).unwrap();
        (dir, path, db)
    }

    #[test]
    fn s4_maintenance_ticker_real_reserve_preflight_records_once_before_string() {
        let (dir, path, db) = s4_maintenance_fixture(true);
        let health = db.health();
        let conn = open_read_only(&path).unwrap();
        let cause = vacuum_into_reserving(&conn, &dir.path().join("backups/control.db"), u64::MAX);
        drop(conn);
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path,
            std::time::Duration::from_secs(3600),
            Policy {
                reserve_bytes: u64::MAX,
                ..Policy::default()
            },
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result(); // first actual tick runs even when stop was already sent
        let observed = health.snapshot();
        let recovered =
            db.call_blocking(|s| s.conn().query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
        let later = health.snapshot();
        let backups = list_auto(&dir.path().join("backups"));
        db.shutdown();
        drop(dir);
        assert!(
            joined.is_ok(),
            "actual worker join before all health oracles"
        );
        assert!(matches!(cause, Err(StoreError::Snapshot { ref source, .. })
            if matches!(source.as_ref(), StoreError::NoSpace { needed, available } if needed > available)));
        assert!(
            backups.unwrap().is_empty(),
            "real preflight makes no auto snapshot"
        );
        assert!(before.is_none());
        assert_eq!(recovered.unwrap().unwrap(), 1);
        assert_eq!(
            observed, later,
            "successful FIFO read does not erase maintenance history"
        );
        // First intended observation; later maintenance/kind assertions are NOTREACHED on RED.
        assert_eq!(observed.as_ref().map(|fault| fault.sequence), Some(1));
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crate::db_health::DbFaultStage::Maintenance);
        assert_eq!(
            fault.kind,
            crate::db_health::DbFaultKind::Storage(StorageKind::DiskFull)
        );
    }

    #[test]
    fn s4_maintenance_ticker_real_integrity_result_records_known_check_once() {
        let (dir, path, db) = s4_maintenance_fixture(true);
        let health = db.health();
        let fresh = tick(&path, crate::now_ms(), Policy::default()).unwrap();
        let retained = fresh.backup.clone().unwrap();
        db.call_blocking(|s| s.conn().execute_batch("CREATE TABLE s4_maintenance_check(v INTEGER CHECK(v>=0)); PRAGMA ignore_check_constraints=ON; INSERT INTO s4_maintenance_check VALUES(-1); PRAGMA ignore_check_constraints=OFF")).unwrap().unwrap();
        let conn = open_for_check(&path).unwrap();
        let cause = check(&conn, Check::Quick);
        drop(conn);
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path,
            std::time::Duration::from_secs(3600),
            Policy::default(),
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result(); // first actual tick runs even when stop was already sent
        let observed = health.snapshot();
        let recovered =
            db.call_blocking(|s| s.conn().query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
        let later = health.snapshot();
        let backups = list_auto(&dir.path().join("backups"));
        let retained_present = retained.exists();
        db.shutdown();
        drop(dir);
        assert!(
            joined.is_ok(),
            "actual worker join before all health oracles"
        );
        assert!(
            cause.is_ok_and(|v| !v.is_empty()),
            "actual named CHECK result, not arbitrary text"
        );
        assert_eq!(backups.unwrap().len(), 1);
        assert!(
            retained_present,
            "known live problems never rotate original snapshot"
        );
        assert!(before.is_none());
        assert_eq!(recovered.unwrap().unwrap(), 1);
        assert_eq!(
            observed, later,
            "successful FIFO read does not erase maintenance history"
        );
        // First intended observation; later maintenance/kind assertions are NOTREACHED on RED.
        assert_eq!(observed.as_ref().map(|fault| fault.sequence), Some(1));
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crate::db_health::DbFaultStage::Maintenance);
        assert_eq!(fault.kind, crate::db_health::DbFaultKind::Integrity);
    }

    #[test]
    fn s4_maintenance_ticker_startup_real_enotdir_sweep_records_once() {
        let (dir, path, db) = s4_maintenance_fixture(false);
        let health = db.health();
        std::fs::write(dir.path().join("backups"), b"owned regular file").unwrap();
        let cause = sweep_stale_tmp(&dir.path().join("backups"));
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path,
            std::time::Duration::from_secs(3600),
            Policy::default(),
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result(); // first actual tick runs even when stop was already sent
        let observed = health.snapshot();
        let recovered =
            db.call_blocking(|s| s.conn().query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
        let later = health.snapshot();
        let backup_is_file = dir.path().join("backups").is_file();
        db.shutdown();
        drop(dir);
        assert!(
            joined.is_ok(),
            "actual worker join before all health oracles"
        );
        assert!(
            matches!(cause, Err(StoreError::Io(ref e)) if e.raw_os_error() == Some(libc::ENOTDIR))
        );
        assert!(
            backup_is_file,
            "empty DB tick does not replace the owned obstruction"
        );
        assert!(before.is_none());
        assert_eq!(recovered.unwrap().unwrap(), 1);
        assert_eq!(
            observed, later,
            "successful FIFO read does not erase maintenance history"
        );
        // First intended observation; later maintenance/kind assertions are NOTREACHED on RED.
        assert_eq!(observed.as_ref().map(|fault| fault.sequence), Some(1));
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crate::db_health::DbFaultStage::Maintenance);
        assert_eq!(
            fault.kind,
            crate::db_health::DbFaultKind::Storage(StorageKind::Io)
        );
    }

    #[test]
    fn s4_maintenance_ticker_real_missing_file_keeps_io_cause_and_does_not_create_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::DB_FILE);
        let db = crate::Db::spawn(crate::Store::open_in_memory().unwrap()).unwrap();
        let health = db.health();
        let cause = tick(&path, crate::now_ms(), Policy::default());
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path.clone(),
            std::time::Duration::from_secs(3600),
            Policy::default(),
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result();
        let observed = health.snapshot();
        let absent = !path.exists();
        let recovered =
            db.call_blocking(|s| s.conn().query_row("SELECT 1", [], |r| r.get::<_, i64>(0)));
        let later = health.snapshot();
        db.shutdown();
        drop(dir);
        assert!(joined.is_ok(), "actual worker join before history oracle");
        assert!(
            matches!(cause, Err(StoreError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound)
        );
        assert!(absent, "missing maintenance file must never be created");
        assert!(before.is_none());
        assert_eq!(recovered.unwrap().unwrap(), 1);
        assert_eq!(observed, later);
        assert_eq!(observed.as_ref().map(|fault| fault.sequence), Some(1));
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crate::db_health::DbFaultStage::Maintenance);
        assert_eq!(
            fault.kind,
            crate::db_health::DbFaultKind::Storage(StorageKind::Io)
        );
    }

    #[test]
    fn s4_maintenance_ticker_empty_keeps_no_fault_and_preserves_backups() {
        let (dir, path, db) = s4_maintenance_fixture(false);
        let health = db.health();
        let control = tick(&path, crate::now_ms(), Policy::default());
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path,
            std::time::Duration::from_secs(3600),
            Policy::default(),
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result();
        let observed = health.snapshot();
        let backups = list_auto(&dir.path().join("backups"));

        db.shutdown();
        drop(dir);
        assert!(
            joined.is_ok(),
            "actual normal worker join before positive oracles"
        );
        let control = control.unwrap();
        assert!(
            control.backup.is_none()
                && control.backup_error.is_none()
                && control.live_problems.is_empty()
        );
        assert!(backups.unwrap().is_empty());
        assert!(before.is_none());
        assert!(
            observed.is_none(),
            "empty/not-due is ordinary successful maintenance"
        );
    }

    #[test]
    fn s4_maintenance_ticker_not_due_keeps_no_fault_and_preserves_backups() {
        let (dir, path, db) = s4_maintenance_fixture(true);
        let health = db.health();
        let first = tick(&path, crate::now_ms(), Policy::default()).unwrap();
        let retained = first.backup.clone().unwrap();
        let original = std::fs::read(&retained).unwrap();
        let before = health.snapshot();
        let ticker = Ticker::start_with_health(
            path,
            std::time::Duration::from_secs(3600),
            Policy::default(),
            health.clone(),
        )
        .unwrap();
        let joined = ticker.stop_result();
        let observed = health.snapshot();
        let backups = list_auto(&dir.path().join("backups"));
        let retained_after = std::fs::read(&retained);
        db.shutdown();
        drop(dir);
        assert!(
            joined.is_ok(),
            "actual normal worker join before positive oracles"
        );
        assert_eq!(backups.unwrap().len(), 1);
        assert_eq!(retained_after.unwrap(), original);
        assert!(before.is_none());
        assert!(
            observed.is_none(),
            "empty/not-due is ordinary successful maintenance"
        );
    }

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
    snapshot_steps(&source, destination, 0, Some(0), false)
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
            crate::schema::SCHEMA_VERSION
        );
    }
}

#[cfg(test)]
mod managed_field_restore_tests {
    use super::*;
    use crewd_proto::{managed_objects as v1, managed_tables_v2 as v2};
    fn create(c: &Connection) -> i64 {
        c.execute("INSERT OR IGNORE INTO crews(id,title,created_at,updated_at) VALUES('field-floor-crew','A','t','t')",[]).unwrap();
        let tx = c.unchecked_transaction().unwrap();
        let t = crate::managed_objects::create_table_by(
            &tx,
            "field-floor-crew",
            "T",
            &[v1::Field {
                name: "title".into(),
                field_type: v1::FieldType::Text,
                nullable: false,
            }],
            &v2::Author::Human {},
            "t",
        )
        .unwrap();
        tx.commit().unwrap();
        crate::managed_tables_v2::number(&t.id, 'T').unwrap()
    }
    fn issue(c: &Connection, n: i64) -> String {
        let mut t = crate::managed_tables_v2::table(c, n).unwrap().unwrap();
        let expected = t.schema_revision.parse::<i64>().unwrap();
        let next = t.next_field_number.parse::<i64>().unwrap();
        let id = format!("p{next}");
        t.fields.push(v2::Field {
            id: id.clone(),
            name: format!("field{next}"),
            kind: v2::FieldKind::Text {},
            nullable: true,
            hidden: false,
        });
        t.schema_revision = (expected + 1).to_string();
        t.next_field_number = next.checked_add(1).unwrap().to_string();
        let tx = c.unchecked_transaction().unwrap();
        crate::managed_tables_v2::save_table(&tx, &t, expected).unwrap();
        tx.commit().unwrap();
        id
    }
    #[test]
    fn restore_keeps_issued_fields_after_table_cascade_but_new_table_starts_at_p1() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("old.db");
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        let n = create(s.conn());
        vacuum_into(s.conn(), &source).unwrap();
        let bytes = std::fs::read(&source).unwrap();
        assert_eq!(issue(s.conn(), n), "p2");
        // Exercise the real storage cascade: allocator metadata must outlive it.
        s.conn()
            .execute("DELETE FROM object_refs WHERE number=?1", [n])
            .unwrap();
        assert!(
            crate::managed_tables_v2::table(s.conn(), n)
                .unwrap()
                .is_none()
        );
        drop(s);
        restore(d.path(), &source).unwrap();
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        assert_eq!(issue(s.conn(), n), "p3");
        let fresh = create(s.conn());
        assert_eq!(
            crate::managed_tables_v2::table(s.conn(), fresh)
                .unwrap()
                .unwrap()
                .next_field_number,
            "2"
        );
        assert_eq!(std::fs::read(source).unwrap(), bytes);
    }
    #[test]
    fn staged_and_missing_active_publish_retry_preserve_field_floor() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("old.db");
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        let n = create(s.conn());
        vacuum_into(s.conn(), &source).unwrap();
        assert_eq!(issue(s.conn(), n), "p2");
        drop(s);
        let src = open_read_only(&source).unwrap();
        let stage = d.path().join("interrupted-stage.db");
        snapshot_steps_with_floors(&src, &stage, 0, Some(0), Some(41), None, false).unwrap();
        let staged = open_read_only(&stage).unwrap();
        assert_eq!(
            crate::managed_field_allocator::allocator_floor(&staged).unwrap(),
            41
        );
        drop(staged);
        // Exact cut after durable witness / before publish, with old active moved.
        crate::managed_field_allocator::persist_witness(d.path(), 41).unwrap();
        let held = d.path().join("held-active.db");
        std::fs::rename(d.path().join(crate::DB_FILE), &held).unwrap();
        restore(d.path(), &source).unwrap();
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        assert_eq!(issue(s.conn(), n), "p41");
        assert!(held.is_file());
        assert!(stage.is_file());
    }
    #[test]
    fn bad_field_witness_refuses_before_moving_live_or_source() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("old.db");
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        create(s.conn());
        vacuum_into(s.conn(), &source).unwrap();
        drop(s);
        let active = d.path().join(crate::DB_FILE);
        let before = std::fs::read(&active).unwrap();
        let backup = std::fs::read(&source).unwrap();
        fsutil::atomic_write(
            &d.path().join(".managed-fields-restore-floor.v1"),
            b"invalid\n",
            0o600,
        )
        .unwrap();
        assert!(restore(d.path(), &source).is_err());
        assert_eq!(std::fs::read(active).unwrap(), before);
        assert_eq!(std::fs::read(source).unwrap(), backup);
        assert!(!std::fs::read_dir(d.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".replaced-")
        }));
    }
    #[test]
    fn real_v29_restore_keeps_positional_fields_then_carries_only_next_number() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("v29.db");
        let old = Connection::open(&source).unwrap();
        crate::testing::historical_fixture(&old, 29).unwrap();
        old.execute(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES('old','Old','t','t')",
            [],
        )
        .unwrap();
        let tx = old.unchecked_transaction().unwrap();
        let t = crate::managed_objects::create_table(
            &tx,
            "old",
            "Original",
            &[
                v1::Field {
                    name: "name".into(),
                    field_type: v1::FieldType::Text,
                    nullable: false,
                },
                v1::Field {
                    name: "count".into(),
                    field_type: v1::FieldType::Integer,
                    nullable: true,
                },
            ],
            "t",
        )
        .unwrap();
        tx.commit().unwrap();
        drop(old);
        let bytes = std::fs::read(&source).unwrap();
        crate::managed_field_allocator::persist_witness(d.path(), 73).unwrap();
        restore(d.path(), &source).unwrap();
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        let n = crate::managed_tables_v2::number(&t.id, 'T').unwrap();
        let t = crate::managed_tables_v2::table(s.conn(), n)
            .unwrap()
            .unwrap();
        assert_eq!(
            t.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["p1", "p2"]
        );
        assert_eq!(t.next_field_number, "73");
        assert_eq!(t.schema_revision, "1");
        assert_eq!(issue(s.conn(), n), "p73");
        assert_eq!(std::fs::read(source).unwrap(), bytes);
    }
    #[test]
    fn normal_v29_migration_keeps_each_tables_original_local_next() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join(crate::DB_FILE);
        let c = Connection::open(&db).unwrap();
        crate::testing::historical_fixture(&c, 29).unwrap();
        c.execute(
            "INSERT INTO crews(id,title,created_at,updated_at) VALUES('old','Old','t','t')",
            [],
        )
        .unwrap();
        let tx = c.unchecked_transaction().unwrap();
        let mut ids = Vec::new();
        for count in [3, 1] {
            let fields = (0..count)
                .map(|i| v1::Field {
                    name: format!("field{i}"),
                    field_type: v1::FieldType::Text,
                    nullable: false,
                })
                .collect::<Vec<_>>();
            let t =
                crate::managed_objects::create_table(&tx, "old", "Original", &fields, "t").unwrap();
            ids.push((crate::managed_tables_v2::number(&t.id, 'T').unwrap(), count));
        }
        tx.commit().unwrap();
        drop(c);
        let s = crate::Store::open_data_dir(d.path()).unwrap();
        for (id, count) in ids {
            let t = crate::managed_tables_v2::table(s.conn(), id)
                .unwrap()
                .unwrap();
            assert_eq!(
                t.fields.iter().map(|f| f.id.clone()).collect::<Vec<_>>(),
                (1..=count).map(|i| format!("p{i}")).collect::<Vec<_>>()
            );
            assert_eq!(t.next_field_number, (count + 1).to_string());
        }
    }
}
