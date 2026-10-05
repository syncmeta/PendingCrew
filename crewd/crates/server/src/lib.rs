//! crewd 服务端：Unix socket 上的 NDJSON RPC。
mod conn;
pub mod dispatch;
pub mod hub;

use crewd_store::Store;
pub use hub::Hub;
use std::future::Future;
use std::path::PathBuf;
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

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("crewd.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("另一个 crewd 已在使用数据目录 {}", data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
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
