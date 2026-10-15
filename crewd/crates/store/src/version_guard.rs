//! Refuse future/unknown databases without opening original SQLite files.
//! `open_data_dir` is called under crewd.lock. That cooperative authority lock
//! does not authorize unrelated processes to mutate these files concurrently.
//! SQLite format: https://sqlite.org/fileformat.html#wal_file_format (§4.1–4.5).
use crate::{Result, StoreError, schema::SCHEMA_VERSION};
use ring::digest::{Context, SHA256};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

fn unknown() -> StoreError {
    StoreError::Invalid("数据库/WAL 版本或稳定性无法确认；拒绝打开，原数据文件未触碰".into())
}
fn version(v: u32) -> Result<()> {
    if i64::from(v) > SCHEMA_VERSION {
        return Err(StoreError::Invalid(format!(
            "数据库版本 {v} 比本程序支持的 {SCHEMA_VERSION} 新，拒绝打开"
        )));
    }
    Ok(())
}
fn be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("fixed format field"))
}
fn header(bytes: &[u8]) -> Result<(u32, u32, usize)> {
    if bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
        return Err(unknown());
    }
    let raw = u16::from_be_bytes([bytes[16], bytes[17]]);
    let page = if raw == 1 { 65536 } else { usize::from(raw) };
    if !(512..=65536).contains(&page)
        || !page.is_power_of_two()
        || ![1, 2].contains(&bytes[18])
        || ![1, 2].contains(&bytes[19])
    {
        return Err(unknown());
    }
    Ok((be(&bytes[60..64]), be(&bytes[68..72]), page))
}
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified: (i64, i64),
    mode: u32,
    digest: Vec<u8>,
}
fn read_file(path: &Path) -> Result<Option<(File, Stamp)>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(unknown());
    }
    let mut digest = Context::new(&SHA256);
    let mut chunk = [0u8; 65536];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        digest.update(&chunk[..n]);
        size = size.checked_add(n as u64).ok_or_else(unknown)?;
    }
    let after = file.metadata()?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || size != before.len()
    {
        return Err(unknown());
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(Some((
        file,
        Stamp {
            dev: before.dev(),
            ino: before.ino(),
            size,
            modified: (before.mtime(), before.mtime_nsec()),
            mode: before.mode(),
            digest: digest.finish().as_ref().to_vec(),
        },
    )))
}
fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
pub(crate) struct Guard {
    files: Vec<(PathBuf, Option<Stamp>)>,
}
impl Guard {
    pub(crate) fn recheck(&self) -> Result<()> {
        for (path, expected) in &self.files {
            if read_file(path)?.map(|(_, stamp)| stamp) != *expected {
                return Err(unknown());
            }
        }
        Ok(())
    }
}
struct PrivateCopy(PathBuf);
impl PrivateCopy {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("pcw-version-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self(dir))
    }
    fn copy(&self, name: &str, source: &mut File) -> Result<PathBuf> {
        let path = self.0.join(name);
        let mut dest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        source.seek(SeekFrom::Start(0))?;
        std::io::copy(source, &mut dest)?;
        dest.flush()?;
        Ok(path)
    }
}
impl Drop for PrivateCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// The checksum order is selected by the WAL magic, not host endianness.
fn checksum(bytes: &[u8], little: bool, state: &mut [u32; 2]) {
    for pair in bytes.chunks_exact(8) {
        let word = |slice: &[u8]| {
            if little {
                u32::from_le_bytes(slice.try_into().unwrap())
            } else {
                be(slice)
            }
        };
        state[0] = state[0]
            .wrapping_add(word(&pair[..4]))
            .wrapping_add(state[1]);
        state[1] = state[1]
            .wrapping_add(word(&pair[4..]))
            .wrapping_add(state[0]);
    }
}
/// Calculate the effective page-1 fields at the LAST VALID COMMIT. Valid but
/// uncommitted frames, incomplete final frames and a first salt/checksum-invalid
/// frame are ignored by SQLite's reader cut. A malformed WAL HEADER is unknown;
/// a successful copy SQLite open alone is never our tail-format proof.
fn wal_cut(wal: &mut File, main: (u32, u32, usize)) -> Result<(u32, u32)> {
    let mut h = [0u8; 32];
    wal.read_exact(&mut h).map_err(|_| unknown())?;
    let magic = be(&h[..4]);
    if ![0x377f0682, 0x377f0683].contains(&magic)
        || be(&h[4..8]) != 3_007_000
        || be(&h[8..12]) as usize != main.2
    {
        return Err(unknown());
    }
    let little = magic == 0x377f0682;
    let mut sum = [0, 0];
    checksum(&h[..24], little, &mut sum);
    if sum != [be(&h[24..28]), be(&h[28..32])] {
        return Err(unknown());
    }
    let mut effective = (main.0, main.1);
    let mut pending = effective;
    let mut frame = vec![0u8; 24 + main.2];
    loop {
        let mut filled = 0;
        while filled < frame.len() {
            let n = wal.read(&mut frame[filled..])?;
            if n == 0 {
                return Ok(effective);
            } // clean EOF or truncated final frame
            filled += n;
        }
        if frame[8..16] != h[16..24] {
            return Ok(effective);
        }
        let mut next = sum;
        checksum(&frame[..8], little, &mut next);
        checksum(&frame[24..], little, &mut next);
        if next != [be(&frame[16..20]), be(&frame[20..24])] {
            return Ok(effective);
        }
        sum = next;
        let page = be(&frame[..4]);
        if page == 0 {
            return Err(unknown());
        }
        if page == 1 {
            let new = header(&frame[24..])?;
            if new.2 != main.2 {
                return Err(unknown());
            }
            pending = (new.0, new.1);
        }
        if be(&frame[4..8]) != 0 {
            effective = pending;
            version(effective.0)?;
        }
    }
}

pub(crate) fn preflight(path: &Path) -> Result<Guard> {
    let paths = [
        path.to_path_buf(),
        suffix(path, "-wal"),
        suffix(path, "-shm"),
        suffix(path, "-journal"),
        suffix(path, ".id"),
    ];
    let mut sources = Vec::new();
    let mut guard = Guard { files: Vec::new() };
    for path in paths {
        let read = read_file(&path)?;
        guard.files.push((
            path,
            read.as_ref().map(|(_, s)| Stamp {
                dev: s.dev,
                ino: s.ino,
                size: s.size,
                modified: s.modified,
                mode: s.mode,
                digest: s.digest.clone(),
            }),
        ));
        sources.push(read);
    }
    let wal_nonempty = sources[1].as_ref().is_some_and(|(_, s)| s.size > 0);
    let journal_nonempty = sources[3].as_ref().is_some_and(|(_, s)| s.size > 0);
    let Some((main, stamp)) = sources[0].as_mut() else {
        if wal_nonempty || journal_nonempty {
            return Err(unknown());
        }
        guard.recheck()?;
        return Ok(guard);
    };
    if stamp.size == 0 && !wal_nonempty && !journal_nonempty {
        guard.recheck()?;
        return Ok(guard);
    }
    let mut h = [0u8; 100];
    let corrupt_header = || {
        if wal_nonempty || journal_nonempty {
            // A recovery sidecar can change classification; without a verified
            // complete format/cut, retain the prior unknown fail-closed result.
            unknown()
        } else {
            StoreError::Integrity {
                check: "SQLite file header",
                problems: vec!["nonempty main file does not contain a valid SQLite header".into()],
            }
        }
    };
    main.read_exact(&mut h).map_err(|_| corrupt_header())?;
    let fields = header(&h).map_err(|_| corrupt_header())?;
    version(fields.0)?;
    // A hot rollback journal needs a separate verified recovery contract. Never
    // let an unclassified journal change the source during a version probe.
    if journal_nonempty {
        return Err(unknown());
    }
    if wal_nonempty {
        let expected = wal_cut(&mut sources[1].as_mut().unwrap().0, fields)?;
        version(expected.0)?;
        let copy = PrivateCopy::new()?;
        let db = copy.copy("probe.db", &mut sources[0].as_mut().unwrap().0)?;
        copy.copy("probe.db-wal", &mut sources[1].as_mut().unwrap().0)?;
        guard.recheck()?;
        // ONLY the private copy may create SHM/checkpoint/recover. The original
        // SHM is deliberately not copied: SQLite must reconstruct its own index.
        let conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let actual: (u32, u32) = conn.query_row(
            "SELECT * FROM pragma_user_version(), pragma_application_id()",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if actual != expected {
            return Err(unknown());
        }
        version(actual.0)?;
        let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(unknown());
        }
        drop(conn);
    }
    guard.recheck()?;
    Ok(guard)
}
/// Offline backup caller holds the data-directory lock. SQLite reads/recovery
/// touch only this private stable copy, including the last committed WAL cut.
pub(crate) fn with_quiescent_copy<T>(
    path: &Path,
    operation: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let guard = preflight(path)?;
    let copy = PrivateCopy::new()?;
    let mut main = read_file(path)?.ok_or_else(unknown)?.0;
    let db = copy.copy("probe.db", &mut main)?;
    if let Some((mut wal, stamp)) = read_file(&suffix(path, "-wal"))?
        && stamp.size > 0
    {
        copy.copy("probe.db-wal", &mut wal)?;
    }
    guard.recheck()?;
    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let value = operation(&conn)?;
    drop(conn);
    guard.recheck()?;
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    type FileCensus = Vec<(PathBuf, Option<(Stamp, Vec<u8>)>)>;
    fn census(path: &Path) -> FileCensus {
        [
            path.to_path_buf(),
            suffix(path, "-wal"),
            suffix(path, "-shm"),
            suffix(path, "-journal"),
            suffix(path, ".id"),
        ]
        .into_iter()
        .map(|p| {
            let value = read_file(&p).unwrap().map(|(_, stamp)| {
                let bytes = fs::read(&p).unwrap();
                (stamp, bytes)
            });
            (p, value)
        })
        .collect()
    }
    fn wal_database(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE proof(id TEXT PRIMARY KEY); INSERT INTO proof VALUES('stable'); PRAGMA user_version=14; PRAGMA application_id=1129477956; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        conn
    }
    #[test]
    fn quiescent_backup_preserves_original_files_and_committed_crash_wal() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let held = wal_database(&source);
        held.execute_batch("INSERT INTO proof VALUES('committed-in-wal');")
            .unwrap();
        let before = census(&source);
        let dest = root.path().join("backup.db");
        crate::backup::backup_db_quiescent(&source, &dest).unwrap();
        assert_eq!(before, census(&source));
        let copied = Connection::open(&dest).unwrap();
        let rows: Vec<String> = copied
            .prepare("SELECT id FROM proof ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, ["committed-in-wal", "stable"]);
        assert_eq!(
            copied
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            14
        );
        drop(copied);
        drop(held);
    }
    #[test]
    fn quiescent_backup_future_wal_and_unknown_journal_are_zero_effect_rejections() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.db");
        let held = wal_database(&source);
        held.execute_batch(&format!(
            "PRAGMA user_version={};",
            crate::schema::SCHEMA_VERSION + 1
        ))
        .unwrap();
        let before = census(&source);
        let dest = root.path().join("backup.db");
        assert!(crate::backup::backup_db_quiescent(&source, &dest).is_err());
        assert_eq!(before, census(&source));
        assert!(!dest.exists());
        drop(held);
        let held = Connection::open(&source).unwrap();
        held.execute_batch("PRAGMA user_version=14;").unwrap();
        drop(held);
        fs::write(suffix(&source, "-journal"), b"unknown-hot-journal").unwrap();
        let before = census(&source);
        assert!(crate::backup::backup_db_quiescent(&source, &dest).is_err());
        assert_eq!(before, census(&source));
        assert!(!dest.exists());
    }
    #[test]
    fn future_main_refuses_before_chmod_or_sidecar_touch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proof.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE proof(x); PRAGMA user_version={};",
            crate::schema::SCHEMA_VERSION + 1
        ))
        .unwrap();
        drop(conn);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = census(&path);
        assert!(crate::Store::open(&path).is_err());
        assert_eq!(before, census(&path));
    }
    #[test]
    fn future_only_committed_in_wal_refuses_original_bytes_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proof.db");
        let conn = wal_database(&path);
        conn.execute_batch(&format!(
            "PRAGMA user_version={};",
            crate::schema::SCHEMA_VERSION + 1
        ))
        .unwrap();
        let main = fs::read(&path).unwrap();
        assert_eq!(header(&main).unwrap().0, 14);
        let before = census(&path);
        assert!(crate::Store::open(&path).is_err());
        assert_eq!(before, census(&path));
        drop(conn);
    }
    #[test]
    fn bundled_sqlite_commit_cut_agrees_with_valid_and_truncated_crash_tail() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proof.db");
        let conn = wal_database(&path);
        conn.execute_batch("INSERT INTO proof VALUES('committed');")
            .unwrap();
        let before = census(&path);
        let guard = preflight(&path).unwrap();
        guard.recheck().unwrap();
        assert_eq!(before, census(&path));
        let wal = suffix(&path, "-wal");
        let actual = fs::read(&wal).unwrap();
        // Only a copied, quiescent fixture receives a simulated crash tail.
        let copied = root.path().join("crash.db");
        fs::copy(&path, &copied).unwrap();
        let mut truncated = actual.clone();
        truncated.extend_from_slice(&actual[32..53]);
        fs::write(suffix(&copied, "-wal"), &truncated).unwrap();
        let before = census(&copied);
        preflight(&copied).unwrap();
        assert_eq!(before, census(&copied));
        let reopened = Connection::open(&copied).unwrap();
        assert_eq!(
            reopened
                .query_row("SELECT COUNT(*) FROM proof", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(reopened);
        drop(conn);
    }
    #[test]
    fn invalid_salt_tail_and_uncommitted_frames_do_not_become_future_commit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proof.db");
        let conn = wal_database(&path);
        conn.execute_batch("INSERT INTO proof VALUES('committed');")
            .unwrap();
        let bytes = fs::read(suffix(&path, "-wal")).unwrap();
        let copied = root.path().join("tail.db");
        fs::copy(&path, &copied).unwrap();
        let mut stale = bytes.clone();
        let mut frame = bytes[32..].to_vec();
        frame[8] ^= 1;
        stale.extend(frame);
        fs::write(suffix(&copied, "-wal"), stale).unwrap();
        let before = census(&copied);
        preflight(&copied).unwrap();
        assert_eq!(before, census(&copied));
        // Strip only the final commit marker and recompute its real cumulative
        // checksum: complete uncommitted rows are not the committed read view.
        let mut uncommitted = bytes.clone();
        let page = be(&bytes[8..12]) as usize;
        let size = 24 + page;
        let last = 32 + ((bytes.len() - 32) / size - 1) * size;
        uncommitted[last + 4..last + 8].fill(0);
        let little = be(&bytes[..4]) == 0x377f0682;
        let mut sum = [0, 0];
        checksum(&uncommitted[..24], little, &mut sum);
        for off in (32..uncommitted.len()).step_by(size) {
            checksum(&uncommitted[off..off + 8], little, &mut sum);
            checksum(&uncommitted[off + 24..off + size], little, &mut sum);
            uncommitted[off + 16..off + 20].copy_from_slice(&sum[0].to_be_bytes());
            uncommitted[off + 20..off + 24].copy_from_slice(&sum[1].to_be_bytes());
        }
        fs::write(suffix(&copied, "-wal"), uncommitted).unwrap();
        let before = census(&copied);
        preflight(&copied).unwrap();
        assert_eq!(before, census(&copied));
        drop(conn);
    }
    #[test]
    fn unknown_header_and_changed_source_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proof.db");
        let conn = wal_database(&path);
        conn.execute_batch("INSERT INTO proof VALUES('committed');")
            .unwrap();
        let guard = preflight(&path).unwrap();
        conn.execute_batch("INSERT INTO proof VALUES('new');")
            .unwrap();
        assert!(guard.recheck().is_err());
        let copy = root.path().join("unknown.db");
        fs::copy(&path, &copy).unwrap();
        fs::write(suffix(&copy, "-wal"), b"unknown nonempty header").unwrap();
        let before = census(&copy);
        assert!(preflight(&copy).is_err());
        assert_eq!(before, census(&copy));
        drop(conn);
    }
}
