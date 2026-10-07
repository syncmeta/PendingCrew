//! crewd 服务端：Unix socket 上的 NDJSON RPC。
pub mod agent;
pub(crate) mod conn;
pub mod control;
pub mod dispatch;
pub mod hub;
pub mod ledger;
pub mod orchestra;
pub mod token;

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
    pub sessions: Arc<dyn control::SessionControl>,
    pub tokens: token::TokenTable,
    /// M3b 的 agent 面运行时（配置、attach 表）。M3a 只放占位，M3b 换掉 `agent/runtime.rs` 的内容（G9）。
    pub agent: agent::AgentRuntime,
    /// M3c 的编排句柄：AppState 先建、编排后起（编排的 Port 要持有 AppState），所以晚到、放 OnceLock。
    pub orchestra: std::sync::OnceLock<orchestra::Handle>,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            hub: Hub::new(),
            sessions: Arc::new(control::NoSessions),
            tokens: token::TokenTable::default(),
            agent: agent::AgentRuntime::default(),
            orchestra: std::sync::OnceLock::new(),
        }
    }

    /// M3c 在 serve 启动段换上真实现。
    pub fn with_sessions(mut self, s: Arc<dyn control::SessionControl>) -> Self {
        self.sessions = s;
        self
    }

    /// 写协作数据的唯一入口：库线程上跑 `collab::run`，提交后在同一线程上 `hub.emit`（事件顺序 = 提交顺序）。
    pub async fn collab<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut crewd_collab::Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
    ) -> crewd_collab::Result<T> {
        let hub = self.hub.clone();
        self.db
            .call(move |s| {
                // 在库线程上取时间：库线程串行执行，时间戳顺序 = 提交顺序（排队期间不会被后来者超过）。
                let now = crewd_store::now_ms();
                let (v, changes) = crewd_collab::run(s, now, f)?;
                hub.emit(s.conn(), &changes);
                Ok(v)
            })
            .await
            .map_err(crewd_collab::CollabError::from)?
    }
}

impl AppState {
    /// 和 `collab` 一样，但先拍 pre-delete 快照（N4 / S22）：快照与事务在**同一个**库线程任务里，中间插不进
    /// 别的写；快照拍不成就返回存储错误，`f` 不跑。给永久删除用。
    /// `check` 是只读的前置条件，先于快照跑：删不了（没有、不准、冲突）就不拍整库快照（复审 2 轮 1）。
    pub async fn collab_after_snapshot<T: Send + 'static>(
        &self,
        what: String,
        check: impl FnOnce(&rusqlite::Connection) -> crewd_collab::Result<()> + Send + 'static,
        f: impl FnOnce(&mut crewd_collab::Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
    ) -> crewd_collab::Result<T> {
        let hub = self.hub.clone();
        self.db
            .call(move |s| {
                let (v, _snapshot) = s.snapshot_then(&what, check, |s| {
                    let now = crewd_store::now_ms();
                    let (v, changes) = crewd_collab::run(s, now, f)?;
                    hub.emit(s.conn(), &changes);
                    Ok::<T, crewd_collab::CollabError>(v)
                })?;
                Ok(v)
            })
            .await
            .map_err(crewd_collab::CollabError::from)?
    }
}

/// 建数据目录，权限 0700（库里是聊天记录，别让同机其他用户读）。已存在的只收紧：去掉组和其他人的位
/// （PM 2026-10-07）。
pub fn ensure_data_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if dir.is_dir() {
        let mode = std::fs::metadata(dir)?.permissions().mode() & 0o7777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode & !0o077))?;
        }
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

    // 核对库的身份：该在的库不在 / 被换掉时拒绝启动，不新建空库（N4 / S24）。
    let store = Store::open_data_dir(&data_dir)?;
    let db = Db::spawn(store)?;
    // 定时备份（N4 / D-8：serve 自己的 interval，不进编排 Core）：启动时拍一次，之后每小时看一次到没到点。
    let backups = match crewd_store::backup::Ticker::start(
        data_dir.join(crewd_store::DB_FILE),
        std::time::Duration::from_secs(3600),
        crewd_store::backup::Policy::default(),
    ) {
        Ok(t) => t,
        Err(e) => {
            // 起不来也先关库再放锁（复审 2 轮 5）。
            let db = db.clone();
            tokio::task::spawn_blocking(move || db.shutdown()).await?;
            drop(lock);
            return Err(e.into());
        }
    };
    let state = Arc::new(AppState::new(db));
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            // 绑不上也要按次序收尾再放锁（复审 M7）。
            close_storage(backups, state.db.clone()).await?;
            drop(lock);
            return Err(e.into());
        }
    };

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
    // 放锁之前关库：已入队的写跑完，optimize + WAL 截断，库线程 join（N4 / S16）。
    close_storage(backups, state.db.clone()).await?;
    drop(lock);
    Ok(())
}

/// 放锁之前的存储收尾：先停 Ticker（等正在拍的那次做完），再关库（已入队的写跑完，optimize + WAL 截断，
/// 库线程 join）。N4 / S16。
async fn close_storage(backups: crewd_store::backup::Ticker, db: Db) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || {
        backups.stop();
        db.shutdown()
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::CollabEvent;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn collab_commits_then_publishes_client_and_internal_events() {
        use crewd_proto::{Crew, events};
        let store = Store::open_in_memory().unwrap();
        crewd_store::crews::upsert(
            store.conn(),
            &Crew {
                id: "crew-a".into(),
                title: "机组甲".into(),
                created_at: "t".into(),
                updated_at: "t".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        crewd_store::schema::backfill_v3(store.conn()).unwrap();
        let state = AppState::new(crewd_store::Db::spawn(store).unwrap());
        // 等事件一律带超时：发事件的路径被删掉时测试要红，不能挂死。
        async fn within<F: std::future::Future>(f: F) -> F::Output {
            tokio::time::timeout(std::time::Duration::from_secs(5), f)
                .await
                .expect("等事件超时")
        }
        let mut client = state.hub.subscribe();
        let mut internal = state.hub.subscribe_collab();
        let m = state
            .collab(|tx| {
                crewd_collab::messages::post(
                    tx,
                    crewd_collab::Sender::Human,
                    crewd_collab::messages::NewPost {
                        channel_id: "crew-a/lobby".into(),
                        text: "大家好".into(),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
        let e1 = within(client.recv()).await.unwrap();
        assert_eq!(e1.event, events::MESSAGE_APPENDED);
        assert_eq!(e1.data["seq"], m.message.seq);
        assert_eq!(
            within(client.recv()).await.unwrap().event,
            events::BADGE_CHANGED
        );
        assert!(matches!(
            within(internal.recv()).await.unwrap(),
            CollabEvent::MessageAppended { .. }
        ));

        // 回滚那一次之前另开一个内部订阅：上面那次成功调用留下的内部事件不会混进来。
        let mut internal_fresh = state.hub.subscribe_collab();

        let r: crewd_collab::Result<()> = state
            .collab(|tx| {
                crewd_collab::messages::post(
                    tx,
                    crewd_collab::Sender::Human,
                    crewd_collab::messages::NewPost {
                        channel_id: "crew-a/lobby".into(),
                        text: "会回滚".into(),
                        ..Default::default()
                    },
                )?;
                Err(crewd_collab::CollabError::Invalid("中途失败".into()))
            })
            .await;
        assert!(r.is_err());
        assert!(
            matches!(
                client.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "回滚 = 不发事件"
        );
        assert!(
            matches!(
                internal_fresh.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "回滚 = 内部通道也不发"
        );
    }

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
        assert!(r.into_result().is_ok());
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
        assert!(r.clone().into_result().is_ok(), "{r:?}");
        slow.await.unwrap().unwrap();
    }

    /// PM 2026-10-07：数据目录一律 0700；已有目录只收紧（去掉组和其他人的位），不放宽。
    #[test]
    fn fresh_data_dir_is_0700_and_an_existing_loose_dir_is_tightened() {
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
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
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
