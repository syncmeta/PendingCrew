//! crewd 服务端：Unix socket 上的 NDJSON RPC。
mod conn;
pub mod dispatch;
pub mod hub;
pub mod ledger;

use crewd_exec::registry::{ApplyOutcome, OrphanDecision, OrphanReport, ProcessRegistry};
use crewd_store::{Db, Store};
pub use hub::Hub;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::UnixListener;

pub struct AppState {
    pub db: Db,
    pub hub: Hub,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self {
            db,
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

/// 孤儿登记表的位置（执行器起的每个 agent 进程都登记在这里，退出时清账）。
pub const PROCESS_REGISTRY_FILE: &str = "exec-registry.json";

/// 读登记表并回收上次崩溃留下的孤儿。**必须先拿到数据目录锁**（确认没有别的 crewd 活着）。
/// 返回登记表（之后起的会话继续往里登记）和每一条的处置说明。
/// 拒绝执行 / 杀失败 / 读不到身份的条目仍留在登记表里（见 `crewd_exec::registry::reap_orphans`）。
pub fn reap_orphans(data_dir: &Path) -> anyhow::Result<(Arc<ProcessRegistry>, Vec<OrphanReport>)> {
    let registry = Arc::new(ProcessRegistry::load(
        &data_dir.join(PROCESS_REGISTRY_FILE),
    )?);
    let reports = crewd_exec::registry::reap_orphans(&registry)?;
    Ok((registry, reports))
}

/// 把回收结果逐条打到 stderr（`message` 已按真实结果写：没动手的、杀失败的都说清）；
/// 有登记留在表里就再补一行，别让「回收过了」盖住「还有一条没处理」。
pub fn log_reap_reports(reports: &[OrphanReport]) {
    for r in reports {
        eprintln!("crewd: {}", r.message);
    }
    let kept = reports
        .iter()
        .filter(|r| {
            matches!(r.decision, OrphanDecision::Unreadable(_))
                || matches!(
                    r.outcome,
                    ApplyOutcome::Refused(_) | ApplyOutcome::Failed(_)
                )
        })
        .count();
    if kept > 0 {
        eprintln!(
            "crewd: 有 {kept} 条孤儿登记没能处理，仍留在 {PROCESS_REGISTRY_FILE} 里，需要人来看。"
        );
    }
}

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    ensure_data_dir(&data_dir)?;
    let lock = lock_data_dir(&data_dir)?;
    // M3 起会话由 serve 拉起，登记表交给会话管理；M2b 先只在启动时回收。
    let (_registry, reports) = reap_orphans(&data_dir)?;
    log_reap_reports(&reports);
    // 拿到锁 = 没有别的实例在跑，残留的 socket 文件一定是上次崩溃留下的。
    let sock = data_dir.join("crewd.sock");
    let _ = std::fs::remove_file(&sock);

    let store = Store::open(&data_dir.join("crewd.db"))?;
    let db = Db::spawn(store)?;
    let state = Arc::new(AppState::new(db));
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

    #[tokio::test(flavor = "current_thread")]
    async fn hello_answers_while_a_slow_db_job_runs() {
        use crewd_proto::{Request, methods};
        let db = crewd_store::Db::spawn(Store::open_in_memory().unwrap()).unwrap();
        let state = Arc::new(AppState::new(db.clone()));
        let slow = tokio::spawn(async move {
            db.call(|_| std::thread::sleep(std::time::Duration::from_millis(500)))
                .await
        });
        // 计时必须在 yield 之前开始：若库任务被内联跑在 runtime 线程上，卡顿就发生在 yield 里。
        let t = std::time::Instant::now();
        tokio::task::yield_now().await;
        let req = Request {
            id: 1,
            method: methods::HELLO.into(),
            params: serde_json::Value::Null,
        };
        let r = dispatch::handle(&state, req).await;
        assert!(r.error.is_none());
        assert!(
            t.elapsed() < std::time::Duration::from_millis(150),
            "慢查询进行中 system.hello 也要马上回：{:?}",
            t.elapsed()
        );
        let req = Request {
            id: 2,
            method: methods::CREW_LIST.into(),
            params: serde_json::Value::Null,
        };
        let r = dispatch::handle(&state, req).await;
        assert!(r.error.is_none(), "{r:?}");
        slow.await.unwrap().unwrap();
    }

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
    fn startup_reaps_registry_entries_and_clears_them() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(PROCESS_REGISTRY_FILE);
        let reg = ProcessRegistry::load(&path).unwrap();
        // 不存在的 pid（超出 macOS / Linux 的 pid 上限）→ 只清账。别在这里起子进程：
        // fork 到 exec 之间子进程攥着同一个锁文件描述符，会让并行跑的加锁测试偶发失败。
        let pid = i32::MAX;
        reg.record(
            "gone",
            crewd_exec::registry::ProcessIdentity {
                pid,
                pgid: pid,
                start_time: 1,
                boot_id: String::new(),
                command: "claude".into(),
            },
        )
        .unwrap();
        drop(reg);
        let (registry, reports) = reap_orphans(root.path()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].decision,
            crewd_exec::registry::OrphanDecision::AlreadyGone
        );
        assert!(registry.entries().is_empty());
        assert!(
            ProcessRegistry::load(&path).unwrap().entries().is_empty(),
            "清账落盘"
        );
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
