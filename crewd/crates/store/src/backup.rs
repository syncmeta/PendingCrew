//! 快照、备份与检查（N4：S20 / S21 / S25）。数据只在这台机器上，所以这里**只增不删库**：
//! 检查失败只报告；快照绝不覆盖已有文件；轮转只淘汰定时备份，迁移前 / 删除前 / 手动的快照永远保留。
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

/// 建备份目录（新建的那一级 0700，和数据目录一样）。
fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if dir.is_dir() {
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
    snapshot_steps(conn, dest).map_err(|e| StoreError::Snapshot {
        dest: dest.to_path_buf(),
        source: Box::new(e),
    })
}

fn snapshot_steps(conn: &Connection, dest: &Path) -> Result<()> {
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
    let needed = db_bytes(conn)? / 10 * 11 + (1 << 20);
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
        conn.execute("VACUUM INTO ?1", [tmp_str])?;
        // VACUUM INTO 按 umask 建文件（通常 0644）；快照里是全部聊天记录，收成 0600 再改名。
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
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
