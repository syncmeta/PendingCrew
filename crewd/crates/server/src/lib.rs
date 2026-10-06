//! crewd 服务端：Unix socket 上的 NDJSON RPC。
mod conn;
pub mod dispatch;
pub mod hub;
pub mod ledger;

use crewd_store::Store;
pub use hub::Hub;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::UnixListener;

pub struct AppState {
    pub store: tokio::sync::Mutex<Store>,
    pub hub: Hub,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self {
            store: tokio::sync::Mutex::new(store),
            hub: Hub::new(),
        }
    }
}

/// 建数据目录；新建的那一级权限 0700（库里是聊天记录，别让同机其他用户读）。已存在的不动。
pub fn ensure_data_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {
            // mode 会被 umask 削；显式再设一次。
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 独占数据目录：拿 `crewd.lock` 的排他锁。返回的文件句柄活着锁就在。
/// serve 与 import 共用，二者不能同时碰同一个库。
pub fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("crewd.lock"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("另一个 crewd 已在使用数据目录 {}", data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    ensure_data_dir(&data_dir)?;
    let lock = lock_data_dir(&data_dir)?;
    // 拿到锁 = 没有别的实例在跑，残留的 socket 文件一定是上次崩溃留下的。
    let sock = data_dir.join("crewd.sock");
    let _ = std::fs::remove_file(&sock);

    let store = Store::open(&data_dir.join("crewd.db"))?;
    let state = Arc::new(AppState::new(store));
    let listener = UnixListener::bind(&sock)?;

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        eprintln!("crewd: accept 失败: {e}");
                        // 句柄耗尽（EMFILE/ENFILE）时别空转。
                        if matches!(e.raw_os_error(), Some(23) | Some(24)) {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        continue;
                    }
                };
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = conn::run(st, stream).await {
                        eprintln!("crewd: 连接异常结束: {e}");
                    }
                });
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
    drop(lock);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn fresh_data_dir_is_0700_and_existing_dir_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let d = root.path().join("a/b");
        ensure_data_dir(&d).unwrap();
        let mode = std::fs::metadata(&d).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        ensure_data_dir(&d).unwrap();

        let existing = root.path().join("shared");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_data_dir(&existing).unwrap();
        let mode = std::fs::metadata(&existing).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn second_lock_on_same_dir_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let first = lock_data_dir(root.path()).unwrap();
        let err = lock_data_dir(root.path()).unwrap_err();
        assert!(err.to_string().contains("另一个 crewd"), "{err}");
        drop(first);
        lock_data_dir(root.path()).unwrap();
    }
}
