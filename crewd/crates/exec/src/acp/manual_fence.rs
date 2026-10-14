//! Kernel file-lock barrier shared by Core and its authenticated helper.
//! This is a byte-write fence, never actor authentication or a receipt ledger.
use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
const MAGIC: &[u8; 8] = b"PCWMAN14";
const SIZE: u64 = 25;
#[derive(Clone)]
pub struct ManualFence {
    path: PathBuf,
    device: u64,
    inode: u64,
    nonce: [u8; 16],
    owner: bool,
    file: Arc<Mutex<File>>,
}
pub struct ManualFenceGuard<'a>(MutexGuard<'a, File>);
impl Drop for ManualFenceGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard owns the independently opened locked descriptor.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl ManualFence {
    /// Core creates one initially closed fence per lease in its private run
    /// directory. Authority must commit before enable; files are never reused.
    pub fn create(path: &Path, nonce: [u8; 16]) -> io::Result<Self> {
        if nonce == [0; 16] || !path.is_absolute() {
            return Err(denied());
        }
        let parent = path.parent().ok_or_else(denied)?;
        let metadata = parent.symlink_metadata()?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || std::fs::canonicalize(parent)? != parent
        {
            return Err(denied());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let mut header = [0u8; SIZE as usize];
        header[..8].copy_from_slice(MAGIC);
        header[8..24].copy_from_slice(&nonce);
        header[24] = 0;
        file.write_all_at(&header, 0)?;
        file.sync_data()?;
        let metadata = file.metadata()?;
        Ok(Self {
            path: path.to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            nonce,
            owner: true,
            file: Arc::new(Mutex::new(file)),
        })
    }
    /// Helper adoption is exposed only through its verified CoreLedger. A
    /// nonce/path supplied by public JSON does not construct an adopted fence.
    pub(super) fn adopt(path: &Path, nonce: [u8; 16]) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        let fence = Self {
            path: path.to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            nonce,
            owner: false,
            file: Arc::new(Mutex::new(file)),
        };
        {
            let file = fence.file.lock().map_err(|_| denied())?;
            fence.validate(&file)?;
        }
        Ok(fence)
    }
    fn validate(&self, file: &File) -> io::Result<u8> {
        let metadata = file.metadata()?;
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_file()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.nlink() > 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.len() != SIZE
        {
            return Err(denied());
        }
        let mut bytes = [0; SIZE as usize];
        file.read_exact_at(&mut bytes, 0)?;
        if &bytes[..8] != MAGIC || bytes[8..24] != self.nonce || bytes[24] > 1 {
            return Err(denied());
        }
        Ok(bytes[24])
    }
    fn locked(&self, deadline: Instant) -> io::Result<ManualFenceGuard<'_>> {
        // Core and helper open independently once. Clones in one process
        // share a local mutex, never a second flock on a duplicated description.
        // The owning Core unlinks after authenticated helper adoption; neither
        // the native exec child nor public callers receive this CLOEXEC fd.
        let file = self.file.try_lock().map_err(|_| denied())?;
        loop {
            // SAFETY: live independently owned descriptor; nonblocking lock.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                let guard = ManualFenceGuard(file);
                self.validate(&guard.0)?;
                return Ok(guard);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) || Instant::now() >= deadline {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    /// Remove the discoverable path only after the authenticated helper has
    /// opened its read-only descriptor. The live private inode is retained by
    /// both owners until lease cleanup; no native child inherits the fd at exec.
    pub fn seal_after_helper_adopted(&self) -> io::Result<()> {
        if !self.owner {
            return Err(denied());
        }
        let current = std::fs::symlink_metadata(&self.path)?;
        if current.dev() != self.device
            || current.ino() != self.inode
            || !current.is_file()
            || current.nlink() != 1
        {
            return Err(denied());
        }
        std::fs::remove_file(&self.path)
    }
    /// Hold this guard across exactly one bounded nonblocking PTY syscall.
    pub fn admit_chunk(&self, deadline: Instant) -> io::Result<ManualFenceGuard<'_>> {
        let guard = self.locked(deadline)?;
        if self.validate(&guard.0)? != 1 || Instant::now() >= deadline {
            return Err(denied());
        }
        Ok(guard)
    }
    /// Must follow the durable manual revocation/admission ACK. This fence
    /// remains closed until the authenticated caller explicitly enables it.
    pub fn enable_after_commit(&self, deadline: Instant) -> io::Result<()> {
        self.set(1, deadline)
    }
    /// Success is the cross-process barrier: every previous chunk finished,
    /// and future chunks see closed. Failure is unknown, never revoke-ACK.
    pub fn revoke(&self, deadline: Instant) -> io::Result<()> {
        self.set(0, deadline)
    }
    fn set(&self, value: u8, deadline: Instant) -> io::Result<()> {
        if !self.owner {
            return Err(denied());
        }
        let guard = self.locked(deadline)?;
        guard.0.write_all_at(&[value], 24)?;
        guard.0.sync_data()
    }
}
fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "manual fence unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };
    #[test]
    fn helper_adoption_is_read_only_and_cannot_reenable_revoked_fence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap().join("fence");
        let core = ManualFence::create(&path, [4; 16]).unwrap();
        let helper = ManualFence::adopt(&path, [4; 16]).unwrap();
        assert!(
            helper
                .admit_chunk(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        core.enable_after_commit(Instant::now() + Duration::from_secs(1))
            .unwrap();
        drop(
            helper
                .admit_chunk(Instant::now() + Duration::from_secs(1))
                .unwrap(),
        );
        core.seal_after_helper_adopted().unwrap();
        assert!(!path.exists());
        core.revoke(Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert!(
            helper
                .enable_after_commit(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        assert!(
            helper
                .admit_chunk(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
    }
    #[test]
    fn actual_child_chunk_finishes_before_parent_revoke_ack_and_no_next_chunk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap().join("fence");
        let core = ManualFence::create(&path, [7; 16]).unwrap();
        core.enable_after_commit(Instant::now() + Duration::from_secs(2))
            .unwrap();
        let mut child = Command::new("/usr/bin/python3")
            .args([
                "-c",
                r#"import os,sys,fcntl,time
f=os.open(sys.argv[1],os.O_RDONLY)
fcntl.flock(f,fcntl.LOCK_EX)
assert os.pread(f,1,24)==b'\x01'
print('chunk-held',flush=True)
time.sleep(0.05)
fcntl.flock(f,fcntl.LOCK_UN)
sys.stdin.readline()
fcntl.flock(f,fcntl.LOCK_EX)
assert os.pread(f,1,24)==b'\x00'
print('next-chunk-denied',flush=True)
fcntl.flock(f,fcntl.LOCK_UN)
"#,
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "chunk-held\n");
        core.revoke(Instant::now() + Duration::from_secs(2))
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"revoke-ack\n")
            .unwrap();
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "next-chunk-denied\n");
        assert!(child.wait().unwrap().success());
    }
}
