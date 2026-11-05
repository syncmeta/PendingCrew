//! A content-free monotone field-number floor. Tables still start at p1;
//! only restored predecessors inherit the maximum ever issued in this authority.
use crate::{Result, StoreError};
use rusqlite::Connection;
use std::path::Path;

const META: &str = "managedFields.restoreFloor.v1";
const WITNESS: &str = ".managed-fields-restore-floor.v1";
fn bad() -> StoreError {
    StoreError::Invalid("invalid managed field allocator floor".into())
}
fn parse(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == raw)
        .ok_or_else(bad)
}
/// Also used by the v29 migration: retain p1..pn, raise only its next number.
pub fn carried_floor(c: &Connection) -> Result<i64> {
    crate::meta_get(c, META)?
        .as_deref()
        .map(parse)
        .transpose()
        .map(|n| n.unwrap_or(0))
}
/// Read both durable retired-table history and all currently allocated tables.
/// Inspect every SQLite type before MAX so a smaller corrupt REAL cannot hide.
pub(crate) fn allocator_floor(c: &Connection) -> Result<i64> {
    let carry = carried_floor(c)?;
    if !crate::managed_tables_v2::enabled(c)? {
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        return if version < 30 { Ok(carry) } else { Err(bad()) };
    }
    let invalid: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM managed_table_attributes WHERE typeof(next_field_number) != 'integer' OR next_field_number <= 0)", [], |r| r.get(0))?;
    if invalid {
        return Err(bad());
    }
    let current: i64 = c.query_row(
        "SELECT COALESCE(MAX(next_field_number),0) FROM managed_table_attributes",
        [],
        |r| r.get(0),
    )?;
    Ok(current.max(carry))
}
/// Called in the SAME transaction that issues a field. Never removed on purge.
/// The stored number is the first unissued pN, not N+1: no SQL arithmetic.
pub fn remember_floor(c: &Connection, next: i64) -> Result<()> {
    if c.is_autocommit() || next <= 0 {
        return Err(bad());
    }
    let existing = carried_floor(c)?;
    if next > existing {
        crate::meta_set(c, META, &next.to_string())?;
    }
    Ok(())
}
/// Transform only the private restore stage, in its epoch/object-floor transaction.
pub(crate) fn preserve_restore_floor(c: &Connection, incoming: i64) -> Result<()> {
    if c.is_autocommit() || incoming < 0 {
        return Err(bad());
    }
    let floor = allocator_floor(c)?.max(incoming);
    if floor == 0 {
        return Ok(());
    }
    remember_floor(c, floor)?;
    if crate::managed_tables_v2::enabled(c)? {
        c.execute(
            "UPDATE managed_table_attributes SET next_field_number=?1 WHERE next_field_number < ?1",
            [floor],
        )?;
    }
    Ok(())
}
pub(crate) fn witness_floor(dir: &Path) -> Result<i64> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(WITNESS))
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let m = f.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o7777 != 0o600
        || m.uid() != unsafe { libc::geteuid() }
        || m.len() > 20
    {
        return Err(bad());
    }
    let mut raw = String::new();
    f.take(21).read_to_string(&mut raw)?;
    parse(raw.strip_suffix('\n').unwrap_or(&raw))
}
pub(crate) fn persist_witness(dir: &Path, floor: i64) -> Result<()> {
    if floor < 0 {
        return Err(bad());
    }
    let previous = witness_floor(dir)?;
    if floor > previous {
        crate::fsutil::atomic_write(&dir.join(WITNESS), format!("{floor}\n").as_bytes(), 0o600)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floor_is_transactional_strict_and_survives_retired_tables() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE managed_table_attributes(next_field_number INTEGER); PRAGMA user_version=30;").unwrap();
        assert!(remember_floor(&c, 9).is_err());
        {
            let tx = c.unchecked_transaction().unwrap();
            remember_floor(&tx, 9).unwrap();
        }
        assert_eq!(allocator_floor(&c).unwrap(), 0);
        let tx = c.unchecked_transaction().unwrap();
        remember_floor(&tx, 9).unwrap();
        remember_floor(&tx, 2).unwrap();
        tx.commit().unwrap();
        c.execute("INSERT INTO managed_table_attributes VALUES(2)", [])
            .unwrap();
        c.execute("DELETE FROM managed_table_attributes", [])
            .unwrap();
        assert_eq!(allocator_floor(&c).unwrap(), 9);
        c.execute("INSERT INTO managed_table_attributes VALUES(1.5)", [])
            .unwrap();
        assert!(allocator_floor(&c).is_err());
        c.execute("DELETE FROM managed_table_attributes", [])
            .unwrap();
        crate::meta_set(&c, META, "09").unwrap();
        assert!(allocator_floor(&c).is_err());
    }
    #[test]
    fn maximum_floor_stays_integer_without_overflow_arithmetic() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE managed_table_attributes(next_field_number INTEGER); PRAGMA user_version=30; INSERT INTO managed_table_attributes VALUES(2);").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        preserve_restore_floor(&tx, i64::MAX).unwrap();
        tx.commit().unwrap();
        let pair: (i64, String) = c
            .query_row(
                "SELECT next_field_number,typeof(next_field_number) FROM managed_table_attributes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pair, (i64::MAX, "integer".into()));
        assert_eq!(allocator_floor(&c).unwrap(), i64::MAX);
        assert!(pair.0.checked_add(1).is_none());
    }
    #[test]
    fn witness_is_monotone_and_rejects_malformed_or_linked_files() {
        let d = tempfile::tempdir().unwrap();
        persist_witness(d.path(), 37).unwrap();
        persist_witness(d.path(), 2).unwrap();
        assert_eq!(witness_floor(d.path()).unwrap(), 37);
        crate::fsutil::atomic_write(&d.path().join(WITNESS), b"037\n", 0o600).unwrap();
        assert!(witness_floor(d.path()).is_err());
        assert!(persist_witness(d.path(), 80).is_err());
        std::fs::remove_file(d.path().join(WITNESS)).unwrap();
        std::os::unix::fs::symlink("missing", d.path().join(WITNESS)).unwrap();
        assert!(witness_floor(d.path()).is_err());
    }
}
