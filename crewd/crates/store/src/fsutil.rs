//! 旁路文件的崩溃安全写（S31）：同目录临时文件（create_new、指定权限）→ 写 → `sync_all`
//! （Apple 上是 F_FULLFSYNC）→ rename → fsync 父目录。任何时刻读到的要么是旧文件，要么是完整的新文件；
//! 改名这一步本身也落了盘（只 fsync 文件、不 fsync 目录，断电后可能连名字带内容一起回到旧的）。
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// 把目录项的变更（建、删、改名）落盘。
pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// 原子地把 `bytes` 写到 `path`（权限 `mode`，比如 0o600）。失败时不留临时文件，原文件不动。
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let dir = parent_of(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "路径没有文件名"))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return result;
    }
    fsync_dir(dir)
}

/// 改名后 fsync 目标所在目录（rename 已由调用方做完时用）。
pub fn rename_durably(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)?;
    fsync_dir(parent_of(to)?)?;
    let from_dir = parent_of(from)?;
    if from_dir != parent_of(to)? {
        fsync_dir(from_dir)?;
    }
    Ok(())
}

/// 库文件不存在就先以 0600 建一个空文件（SQLite 把 0 字节文件当新库，之后建的 -wal / -shm 沿用库文件的权限）；
/// 已有的库、-wal、-shm 去掉组和其他人的权限位（只收紧，不放宽：只读的库仍是只读）。
pub fn restrict_db_files(db: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(db)
    {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let mut side = db.as_os_str().to_owned();
    side.push("-wal");
    let wal = std::path::PathBuf::from(side);
    let mut side = db.as_os_str().to_owned();
    side.push("-shm");
    let shm = std::path::PathBuf::from(side);
    for p in [db, wal.as_path(), shm.as_path()] {
        if let Ok(m) = std::fs::metadata(p) {
            let mode = m.permissions().mode() & 0o7777;
            if mode & 0o077 != 0 {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode & !0o077))?;
            }
        }
    }
    Ok(())
}

fn parent_of(path: &Path) -> io::Result<&Path> {
    match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(p) => Ok(p),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} 没有父目录", path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn writes_replaces_and_sets_the_mode_without_leftovers() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db.id");
        atomic_write(&p, b"one", 0o600).unwrap();
        atomic_write(&p, b"two", 0o600).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        assert_eq!(names(d.path()), ["crewd.db.id"]);
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        atomic_write(&p, b"old", 0o600).unwrap();
        // 目录只读：临时文件建不出来。
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let r = atomic_write(&p, b"new", 0o600);
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(r.is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"old");
        assert_eq!(names(d.path()), ["f"]);
    }

    #[test]
    fn rename_durably_moves_the_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"x").unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        rename_durably(&d.path().join("a"), &d.path().join("sub/b")).unwrap();
        assert_eq!(std::fs::read(d.path().join("sub/b")).unwrap(), b"x");
        assert!(!d.path().join("a").exists());
    }
}
