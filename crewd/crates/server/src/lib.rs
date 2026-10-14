//! crewd 服务端：Unix socket 上的 NDJSON RPC。
pub mod agent;
pub mod build_info;
pub mod causal_fence;
pub(crate) mod conn;
pub mod control;
pub mod dispatch;
pub mod hub;
pub mod ledger;
pub mod native_receipts;
pub mod orchestra;
pub(crate) mod peer;
pub(crate) mod remote;
pub mod token;
pub(crate) mod wake_adapter;

use crewd_exec::registry::{ApplyOutcome, OrphanDecision, OrphanReport, ProcessRegistry};
use crewd_store::{Db, Store};
pub use hub::Hub;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::UnixListener;

#[cfg(test)]
struct DbWriteBarrier {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

pub struct AppState {
    pub db: Db,
    pub hub: Hub,
    pub sessions: Arc<dyn control::SessionControl>,
    pub tokens: token::TokenTable,
    pub causal_fence: Arc<causal_fence::CausalFence>,
    /// M3b 的 agent 面运行时（配置、attach 表）。M3a 只放占位，M3b 换掉 `agent/runtime.rs` 的内容（G9）。
    pub agent: agent::AgentRuntime,
    /// M3c 的编排句柄：AppState 先建、编排后起（编排的 Port 要持有 AppState），所以晚到、放 OnceLock。
    pub orchestra: std::sync::OnceLock<orchestra::Handle>,
    pub(crate) peer_policy: std::sync::OnceLock<Arc<peer::PeerPolicy>>,
    pub(crate) remote: std::sync::OnceLock<Arc<remote::Manager>>,
    pub(crate) runtime_binding_id: String,
    #[cfg(any(test, feature = "testkit"))]
    s5_request_fault: Arc<std::sync::Mutex<Option<u64>>>,
    #[cfg(test)]
    s5_after_route_fault: std::sync::Mutex<Option<u64>>,
    #[cfg(test)]
    test_db_write_barrier: Arc<std::sync::Mutex<Option<DbWriteBarrier>>>,
    #[cfg(test)]
    pub(crate) s72_prediction_tests: wake_adapter::PredictionTestControls,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            hub: Hub::new(),
            sessions: Arc::new(control::NoSessions),
            tokens: token::TokenTable::default(),
            causal_fence: Arc::new(causal_fence::CausalFence::default()),
            agent: agent::AgentRuntime::default(),
            orchestra: std::sync::OnceLock::new(),
            peer_policy: std::sync::OnceLock::new(),
            remote: std::sync::OnceLock::new(),
            runtime_binding_id: uuid::Uuid::new_v4().to_string(),
            #[cfg(any(test, feature = "testkit"))]
            s5_request_fault: Default::default(),
            #[cfg(test)]
            s5_after_route_fault: Default::default(),
            #[cfg(test)]
            test_db_write_barrier: Default::default(),
            #[cfg(test)]
            s72_prediction_tests: Default::default(),
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
        let context = crewd_collab::TxContext::new(self.causal_fence.clone());
        #[cfg(test)]
        let barrier = self.test_db_write_barrier.clone();
        self.db
            .call_result(move |s| {
                #[cfg(test)]
                if let Some(b) = barrier.lock().unwrap().take() {
                    let _ = b.entered.send(());
                    b.release.recv().expect("test must release DB write");
                }
                // 在库线程上取时间：库线程串行执行，时间戳顺序 = 提交顺序（排队期间不会被后来者超过）。
                let now = crewd_store::now_ms();
                let (v, changes) = crewd_collab::run_with_context(s, now, context, f)?;
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
        let context = crewd_collab::TxContext::new(self.causal_fence.clone());
        self.db
            .call_result(move |s| {
                let (v, _snapshot) = s.snapshot_then(&what, check, |s| {
                    let now = crewd_store::now_ms();
                    let (v, changes) = crewd_collab::run_with_context(s, now, context, f)?;
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

#[cfg(test)]
type ServeReady = Box<dyn FnOnce(Arc<AppState>, Option<tokio::task::AbortHandle>) + Send>;
#[cfg(test)]
type ServeStorageClosed = Box<dyn FnOnce(PathBuf) + Send>;

/// Owned synthetic request-fault control; no identity or authorization input.
/// Absent from ordinary non-testkit builds and never a wire method.
#[cfg(any(test, feature = "testkit"))]
#[derive(Clone)]
pub struct S5RequestFaultControl {
    selected: Arc<std::sync::Mutex<Option<u64>>>,
}
#[cfg(any(test, feature = "testkit"))]
impl S5RequestFaultControl {
    /// Arms the existing single-use before-route fault for one exact request ID.
    pub fn arm(&self, id: u64) -> Result<(), String> {
        let mut selected = self
            .selected
            .lock()
            .map_err(|_| "owned S5 fault lock poisoned".to_owned())?;
        if selected.is_some() {
            return Err("owned S5 fault already armed".into());
        }
        *selected = Some(id);
        Ok(())
    }
    /// Observes the same slot consumed by production request_step's testkit seam.
    pub fn armed(&self) -> Result<Option<u64>, String> {
        self.selected
            .lock()
            .map(|selected| *selected)
            .map_err(|_| "owned S5 fault lock poisoned".to_owned())
    }
}

/// Optional orchestration. Library callers retain the no-session default.
#[cfg(feature = "testkit")]
pub type TestRegistryReady = Box<dyn FnOnce(Arc<ProcessRegistry>) -> std::io::Result<()> + Send>;
#[derive(Default)]
pub struct ServeOptions {
    pub orchestra: Option<OrchestraServe>,
    #[cfg(feature = "testkit")]
    pub test_registry_ready: Option<TestRegistryReady>,
    /// Read-only test witness; not a wire method or caller authority input.
    #[cfg(any(test, feature = "testkit"))]
    pub test_subscriptions_ready: Option<Box<dyn FnOnce(hub::SubscriptionCreations) + Send>>,
    /// Shares only the existing fault slot after bind; never exports AppState or peer policy.
    #[cfg(any(test, feature = "testkit"))]
    pub test_s5_fault_ready: Option<Box<dyn FnOnce(S5RequestFaultControl) + Send>>,
    #[cfg(test)]
    test_ready: Option<ServeReady>,
    #[cfg(test)]
    test_storage_closed: Option<ServeStorageClosed>,
    #[cfg(test)]
    test_bridge_fault: Option<tokio::sync::oneshot::Receiver<()>>,
}

pub struct OrchestraServe {
    pub codex: orchestra::exec_codex::CodexProgram,
    pub codex_env: std::collections::BTreeMap<String, String>,
    pub claude: orchestra::exec_claude::ClaudeProgram,
    pub claude_env: std::collections::BTreeMap<String, String>,
    pub helper: Option<PathBuf>,
    pub claude_projects_dir: Option<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
    pub child_path: Option<String>,
    pub config: orchestra::OrchestraConfig,
    /// Also bounds final response draining; stalled clients cannot hold shutdown.
    pub shutdown_limit: std::time::Duration,
}

impl Default for OrchestraServe {
    fn default() -> Self {
        Self {
            codex: orchestra::exec_codex::CodexProgram::default(),
            codex_env: Default::default(),
            claude: orchestra::exec_claude::ClaudeProgram::default(),
            claude_env: Default::default(),
            helper: None,
            claude_projects_dir: None,
            runtime_dir: None,
            child_path: None,
            config: Default::default(),
            shutdown_limit: std::time::Duration::from_secs(10),
        }
    }
}

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    serve_with(data_dir, ServeOptions::default(), shutdown).await
}

pub async fn serve_with(
    data_dir: PathBuf,
    opts: ServeOptions,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    serve_with_hooks_inner(data_dir, opts, None, shutdown).await
}

/// Synthetic library lifecycle instrumentation; ordinary CLI defaults unchanged.
#[cfg(feature = "testkit")]
pub async fn serve_with_test_hooks(
    data_dir: PathBuf,
    opts: ServeOptions,
    hooks: orchestra::hooks::Hooks,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    serve_with_hooks_inner(data_dir, opts, Some(hooks), shutdown).await
}

async fn serve_with_hooks_inner(
    data_dir: PathBuf,
    mut opts: ServeOptions,
    hooks: Option<orchestra::hooks::Hooks>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    // A missing explicit program fails before binding or creating storage.
    if let Some(o) = &mut opts.orchestra {
        if let orchestra::exec_codex::CodexProgram::Path(p) = &mut o.codex {
            *p = std::fs::canonicalize(&*p)?;
        }
        if let orchestra::exec_claude::ClaudeProgram::Path(p) = &mut o.claude {
            *p = std::fs::canonicalize(&*p)?;
        }
        if o.helper.is_none() {
            o.helper = Some(std::env::current_exe()?);
        }
    }
    let sock = data_dir.join("crewd.sock");
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    #[cfg(target_os = "macos")]
    let path_capacity = 104;
    #[cfg(not(target_os = "macos"))]
    let path_capacity = 108;
    let socket_bytes = sock.as_os_str().as_bytes();
    if socket_bytes.contains(&0) {
        return Err(anyhow::anyhow!("local socket path contains NUL"));
    }
    if socket_bytes
        .len()
        .checked_add(1)
        .is_none_or(|n| n > path_capacity)
    {
        return Err(anyhow::anyhow!(
            "local socket path exceeds platform byte limit"
        ));
    }
    ensure_data_dir(&data_dir)?;
    let lock = lock_data_dir(&data_dir)?;
    let _ = build_info::exe();
    let (registry, reports) = reap_orphans(&data_dir)?;
    log_reap_reports(&reports);
    #[cfg(feature = "testkit")]
    if let Some(ready) = opts.test_registry_ready.take() {
        ready(registry.clone())?;
    }
    let peer_policy = Arc::new(peer::PeerPolicy::new(Some(registry.clone()))?);
    let _ = std::fs::remove_file(&sock);
    if let Some(o) = &opts.orchestra {
        ensure_data_dir(o.runtime_dir.as_deref().unwrap_or(&data_dir.join("run")))?;
    }
    let mut store = Store::open_data_dir(&data_dir)?;
    // Registry recovery and the data-directory lock precede this transaction.
    // Stored birth/session receipts are observations, never fresh authority.
    {
        let tx = store.conn().unchecked_transaction()?;
        crewd_store::native_execution::invalidate_unknown(&tx)?;
        tx.commit()?;
    }
    // Establish the historical recovery baseline before exposing public writes.
    // A genuinely new fact created after readiness must not be mistaken for history.
    crewd_collab::run(
        &mut store,
        crewd_store::now_ms(),
        crewd_collab::work_followups::baseline,
    )?;
    let db = Db::spawn(store)?;
    let hooks = hooks.unwrap_or_else(|| orchestra::hooks::Hooks {
        extras: Arc::new(orchestra::work_context::WorkContext(db.clone())),
        idle: Arc::new(orchestra::work_idle::WorkIdle(db.clone())),
        ..Default::default()
    });
    let backups = match crewd_store::backup::Ticker::start(
        data_dir.join(crewd_store::DB_FILE),
        std::time::Duration::from_secs(3600),
        crewd_store::backup::Policy::default(),
    ) {
        Ok(t) => t,
        Err(e) => {
            tokio::task::spawn_blocking(move || db.shutdown()).await?;
            return Err(e.into());
        }
    };
    let limit = opts
        .orchestra
        .as_ref()
        .map_or(std::time::Duration::from_secs(10), |o| o.shutdown_limit);
    let cell = Arc::new(std::sync::OnceLock::new());
    let state = if opts.orchestra.is_some() {
        Arc::new(AppState::new(db.clone()).with_sessions(Arc::new(
            orchestra::control::OrchestraControl::new(cell.clone(), db.clone()),
        )))
    } else {
        Arc::new(AppState::new(db.clone()))
    };
    let _ = state.peer_policy.set(peer_policy);
    let mut handle = None;
    let mut actor = None;
    let mut forwarder = None;
    if let Some(o) = opts.orchestra {
        let rx = state.hub.subscribe_collab(); // subscribe before boot
        let codex =
            orchestra::exec_codex::CodexExecutor::new(orchestra::exec_codex::CodexExecutorConfig {
                program: o.codex,
                data_dir: data_dir.clone(),
                helper: o.helper.clone().expect("captured helper"),
                registry: Some(registry.clone()),
                extra_env: o.codex_env,
                runtime_dir: o
                    .runtime_dir
                    .clone()
                    .unwrap_or_else(|| data_dir.join("run")),
                child_path: o.child_path.clone(),
            });
        let executor = orchestra::exec_claude::ClaudeExecutor::new(
            orchestra::exec_claude::ClaudeExecutorConfig {
                program: o.claude,
                data_dir: data_dir.clone(),
                helper: o.helper.expect("captured helper"),
                registry: Some(registry.clone()),
                extra_env: o.claude_env,
                projects_dir: o.claude_projects_dir,
                runtime_dir: o.runtime_dir,
                child_path: o.child_path,
            },
        );
        match orchestra::spawn(
            Arc::new(orchestra::db::DbPort::new(
                state.clone(),
                data_dir.join(crewd_store::DB_FILE),
            )),
            orchestra::exec::Executors::new()
                .with("claude", Arc::new(executor))
                .with("codex", Arc::new(codex)),
            hooks,
            Arc::new(orchestra::clock::SystemClock),
            o.config,
        )
        .await
        {
            Ok((h, a)) => {
                let _ = cell.set(h.clone());
                let _ = state.orchestra.set(h.clone());
                #[cfg(test)]
                let bridge = orchestra::db::forward_collab_with_fault(
                    rx,
                    h.clone(),
                    opts.test_bridge_fault.take(),
                );
                #[cfg(not(test))]
                let bridge = orchestra::db::forward_collab(rx, h.clone());
                forwarder = Some(bridge);
                handle = Some(h);
                actor = Some(a);
            }
            Err(e) => {
                drop(state);
                close_storage(backups, db).await?;
                return Err(e);
            }
        }
    }
    let abort = actor.as_ref().map(orchestra::Actor::abort_handle);
    let mut done = Box::pin(async move {
        match actor {
            Some(a) => a.finished().await,
            None => std::future::pending().await,
        }
    });
    let (_no_exit, no_exit) = tokio::sync::watch::channel(false);
    let mut exit = handle
        .as_ref()
        .map_or(no_exit, orchestra::Handle::exit_requested);
    let listener = UnixListener::bind(&sock).and_then(|listener| {
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    });
    let mut fatal = None;
    let mut ended = false;
    let mut reason = "stop";
    let (drain_tx, drain_rx) = tokio::sync::watch::channel(false);
    let mut conns = tokio::task::JoinSet::new();
    match listener {
        Err(e) => fatal = Some(anyhow::Error::from(e)),
        Ok(listener) => {
            match remote::Manager::new(&state, data_dir.clone(), drain_rx.clone()).await {
                Ok(remote) => {
                    let _ = state.remote.set(remote.clone());
                    remote.start_persisted().await;
                }
                Err(error) => {
                    let _ = dispatch::log_internal(error);
                }
            }
            #[cfg(any(test, feature = "testkit"))]
            if let Some(ready) = opts.test_s5_fault_ready.take() {
                ready(S5RequestFaultControl {
                    selected: state.s5_request_fault.clone(),
                });
            }
            #[cfg(any(test, feature = "testkit"))]
            if let Some(ready) = opts.test_subscriptions_ready.take() {
                ready(state.hub.subscription_creations());
            }
            #[cfg(test)]
            if let Some(ready) = opts.test_ready.take() {
                ready(state.clone(), abort.clone());
            }
            tokio::pin!(shutdown);
            loop {
                tokio::select! {
                    _ = &mut shutdown => break,
                    changed = exit.changed(), if handle.is_some() => {
                        match changed {
                            Ok(()) if *exit.borrow() => { reason = "hibernate"; break; }
                            Ok(()) => {},
                            Err(_) => {
                                if *exit.borrow() { reason = "hibernate"; }
                                else { fatal = Some(orchestra::ActorDied("exit watch closed without hibernate".into()).into()); }
                                break;
                            }
                        }
                    }
                    result = &mut done, if handle.is_some() => {
                        ended = true;
                        match result {
                            Err(e) => fatal = Some(e.into()),
                            Ok(()) if *exit.borrow() => reason = "hibernate",
                            Ok(()) => fatal = Some(orchestra::ActorDied("actor ended without hibernate".into()).into()),
                        }
                        break;
                    }
                    bridge_result = async { forwarder.as_mut().expect("guarded bridge").await }, if forwarder.is_some() => {
                        // Either panic/cancellation or an unexpected normal end loses routing.
                        // Retain no raw panic payload on the public error path.
                        forwarder.take();
                        let _ = bridge_result;
                        fatal = Some(anyhow::anyhow!("critical collab bridge ended unexpectedly"));
                        break;
                    }
                    _ = conns.join_next(), if !conns.is_empty() => {},
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let st = state.clone(); let drain = drain_rx.clone();
                                conns.spawn(async move {
                                    if let Err(e) = conn::run_draining(st, stream, drain).await { eprintln!("crewd: connection ended: {e}"); }
                                });
                            }
                            Err(e) => {
                                eprintln!("crewd: accept failed: {e}");
                                if matches!(e.raw_os_error(), Some(23) | Some(24)) { tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
                            }
                        }
                    }
                }
            }
            drop(listener); // stop intake before actor/storage cleanup
        }
    }
    state.hub.publish(crewd_proto::Event {
        event: crewd_proto::events::SYSTEM_SHUTDOWN.into(),
        data: serde_json::json!({"reason":reason}),
    });
    let _ = drain_tx.send(true); // stop reads; in-flight dispatch and writer still finish
    if let Some(h) = &handle
        && !ended
    {
        if !h.shutdown(limit).await {
            abort.as_ref().expect("actor abort").abort();
        }
        if let Err(e) = done.await {
            fatal.get_or_insert(e.into());
        }
    }
    if tokio::time::timeout(limit, async { while conns.join_next().await.is_some() {} })
        .await
        .is_err()
    {
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
    if let Some(remote) = state.remote.get() {
        remote.shutdown(limit).await;
    }
    if let Some(f) = forwarder {
        f.abort();
        let _ = f.await;
    }
    drop(handle);
    drop(cell);
    drop(state);
    close_storage(backups, db).await?;
    #[cfg(test)]
    if let Some(closed) = opts.test_storage_closed {
        closed(data_dir.clone());
    }
    let _ = std::fs::remove_file(&sock);
    drop(lock);
    match fatal {
        Some(e) => Err(e),
        None => Ok(()),
    }
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
                Err(crewd_collab::CollabError::Invalid(
                    crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::MessageEmpty),
                ))
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
    #[tokio::test]
    async fn actual_daemon_restart_revokes_native_receipts_before_socket_ready() {
        use crewd_store::native_execution as n;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let id = || uuid::Uuid::new_v4().to_string();
        let runtime = n::Runtime {
            id: id(),
            session: "restart-native".into(),
            generation: id(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "synthetic-kernel-fixture".into(),
        };
        let request = id();
        {
            let store = Store::open_data_dir(&data).unwrap();
            store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('restart-crew','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('restart-native','restart-crew','worker','claude','now');").unwrap();
            let tx = store.conn().unchecked_transaction().unwrap();
            n::register(&tx, &runtime).unwrap();
            n::prepare(&tx, &runtime.id, &request, b"restart never replay", "now").unwrap();
            tx.commit().unwrap();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve(data.clone(), async {
            let _ = rx.await;
        }));
        let stream = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(s) = tokio::net::UnixStream::connect(data.join("crewd.sock")).await {
                    break s;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let conn = rusqlite::Connection::open(data.join(crewd_store::DB_FILE)).unwrap();
        let receipt = n::get(&conn, &request).unwrap().unwrap();
        assert_eq!(receipt.phase, n::Phase::Unknown);
        let state: (i64, i64) = conn
            .query_row(
                "SELECT revoked,manual FROM native_execution_runtimes WHERE runtime_id=?1",
                [&runtime.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (1, 1));
        drop(conn);
        drop(stream);
        tx.send(()).unwrap();
        task.await.unwrap().unwrap();
        let store = Store::open_data_dir(&data).unwrap();
        let dbtx = store.conn().unchecked_transaction().unwrap();
        assert!(
            n::accept_content(
                &dbtx,
                &runtime.id,
                &request,
                receipt.revision,
                &id(),
                b"restart never replay",
                "later"
            )
            .is_err()
        );
        assert!(n::revoke_before_raw(&dbtx, &runtime.id, 1, "later").is_err());
        assert_eq!(n::invalidate_unknown(&dbtx).unwrap(), 0);
        dbtx.commit().unwrap();
    }
    #[tokio::test]
    async fn t11_shutdown_closes_idle_client_connections() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve(data.clone(), async {
            let _ = rx.await;
        }));
        let stream = loop {
            if let Ok(s) = tokio::net::UnixStream::connect(data.join("crewd.sock")).await {
                break s;
            }
            tokio::task::yield_now().await;
        };
        let (rd, mut wr) = stream.into_split();
        wr.write_all(b"{\"id\":1,\"method\":\"system.hello\"}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(rd).lines();
        assert!(lines.next_line().await.unwrap().unwrap().contains("result"));
        tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let eof =
            tokio::time::timeout(std::time::Duration::from_millis(500), lines.next_line()).await;
        assert!(
            matches!(eof, Ok(Ok(None))),
            "serve must own and close idle connection tasks: {eof:?}"
        );
    }

    fn t11_options(data: &Path) -> ServeOptions {
        ServeOptions {
            orchestra: Some(OrchestraServe {
                claude: orchestra::exec_claude::ClaudeProgram::Path("/usr/bin/false".into()),
                claude_projects_dir: Some(data.join("synthetic-projects")),
                runtime_dir: Some(data.join("synthetic-runtime")),
                config: orchestra::OrchestraConfig::for_tests(),
                shutdown_limit: std::time::Duration::from_millis(500),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn collab_bridge_panic_shuts_down_storage_socket_and_lock() {
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (fault_tx, fault_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let mut opts = t11_options(&data);
        opts.test_bridge_fault = Some(fault_rx);
        opts.test_ready = Some(Box::new(move |state, _| {
            ready_tx.send(state).ok();
        }));
        let mut task = tokio::spawn(serve_with(data.clone(), opts, async {
            let _ = stop_rx.await;
        }));
        let ready = tokio::time::timeout(Duration::from_secs(5), ready_rx).await;
        let state = match ready {
            Ok(Ok(state)) => Some(state),
            _ => None,
        };
        let signalled = fault_tx.send(()).is_ok();
        let early = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
        let fatal = matches!(&early, Ok(Ok(Err(_))));
        // Old implementation keeps serving: ask it to stop, then assert the observed failure.
        let _ = stop_tx.send(());
        let cleaned = if early.is_err() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(_) => true,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    false
                }
            }
        } else {
            true
        };
        let db_closed = match state {
            Some(state) => state.db.call(|_| ()).await.is_err(),
            None => false,
        };
        let socket_absent = !data.join("crewd.sock").exists();
        let lock_released = lock_data_dir(&data).is_ok();
        assert!(
            cleaned && db_closed && socket_absent && lock_released,
            "owned serve must close storage/socket/lock before the main oracle"
        );
        assert!(
            signalled && fatal,
            "actual critical Hub pump panic must trigger orderly serve failure"
        );
    }

    #[tokio::test]
    async fn an_actor_panic_makes_serve_exit_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut opts = t11_options(&data);
        opts.test_ready = Some(Box::new(move |state, _| {
            tx.send(state).ok();
        }));
        let task = tokio::spawn(serve_with(data.clone(), opts, std::future::pending()));
        let state = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();
        state.orchestra.get().unwrap().panic_for_serve_test();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err(), "actor panic must fail the server");
        assert!(!data.join("crewd.sock").exists());
        assert!(
            state.db.call(|_| ()).await.is_err(),
            "DB must be closed even with surviving clones"
        );
        lock_data_dir(&data).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborting_the_actor_mid_db_call_still_shuts_the_db_down_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut opts = t11_options(&data);
        opts.test_ready = Some(Box::new(move |state, abort| {
            tx.send((state, abort.unwrap())).ok();
        }));
        let task = tokio::spawn(serve_with(data.clone(), opts, std::future::pending()));
        let (state, abort) = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *state.test_db_write_barrier.lock().unwrap() = Some(DbWriteBarrier {
            entered: entered_tx,
            release: release_rx,
        });
        let handle = state.orchestra.get().unwrap().clone();
        let rpc = tokio::spawn(async move {
            handle
                .set_settings(orchestra::port::SettingsUpdate {
                    lang: Some(crewd_wake::text::Lang::En),
                    max_live: Some(Some(7)),
                    ..Default::default()
                })
                .await
        });
        tokio::task::spawn_blocking(move || {
            entered_rx.recv_timeout(std::time::Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        // The actual actor write is now inside the DB thread. Cancellation of
        // its awaiting future must not cancel this already queued transaction.
        abort.abort();
        release_tx.send(()).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        let _ = rpc.await;
        assert!(state.db.call(|_| ()).await.is_err());
        let store = Store::open_data_dir(&data).unwrap();
        assert_eq!(
            crewd_collab::settings::locale(store.conn()).unwrap(),
            crewd_collab::Locale::En
        );
        assert_eq!(
            crewd_collab::settings::max_live_sessions(store.conn()).unwrap(),
            Some(7)
        );
        let integrity: String = store
            .conn()
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert!(!data.join("crewd.sock").exists());
        lock_data_dir(&data).unwrap();
    }

    #[tokio::test]
    async fn shutdown_order_closes_storage_before_the_socket_and_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let db_observer = Arc::new(std::sync::Mutex::new(None::<Db>));
        let mut opts = t11_options(&data);
        let observer = db_observer.clone();
        opts.test_ready = Some(Box::new(move |state, _| {
            *observer.lock().unwrap() = Some(state.db.clone());
            tx.send(()).ok();
        }));
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = observed.clone();
        opts.test_storage_closed = Some(Box::new(move |data| {
            let db = db_observer.lock().unwrap().take().unwrap();
            let mut call = Box::pin(db.call(|_| ()));
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(matches!(
                call.as_mut().poll(&mut context),
                std::task::Poll::Ready(Err(_))
            ));
            assert!(
                data.join("crewd.sock").exists(),
                "socket cleanup follows storage close"
            );
            assert!(
                lock_data_dir(&data).is_err(),
                "lock must still be held at storage close"
            );
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        let task = tokio::spawn(serve_with(data.clone(), opts, async {
            let _ = stop_rx.await;
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();
        stop_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!data.join("crewd.sock").exists());
        lock_data_dir(&data).unwrap();
    }

    #[tokio::test]
    async fn missing_explicit_program_fails_before_storage_or_binding() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let mut opts = t11_options(&data);
        opts.orchestra.as_mut().unwrap().claude =
            orchestra::exec_claude::ClaudeProgram::Path(dir.path().join("does-not-exist"));
        assert!(
            serve_with(data.clone(), opts, std::future::pending())
                .await
                .is_err()
        );
        assert!(
            !data.exists(),
            "missing program must fail before any storage is created"
        );
    }

    #[tokio::test]
    async fn bind_failure_closes_storage_then_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        ensure_data_dir(&data).unwrap();
        // A directory cannot be removed as a stale socket or bound as one.
        std::fs::create_dir(data.join("crewd.sock")).unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = observed.clone();
        let mut opts = t11_options(&data);
        opts.test_storage_closed = Some(Box::new(move |data| {
            assert!(data.join("crewd.sock").is_dir());
            assert!(lock_data_dir(&data).is_err());
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            serve_with(data.clone(), opts, std::future::pending()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
        lock_data_dir(&data).unwrap();
        let store = Store::open_data_dir(&data).unwrap();
        let integrity: String = store
            .conn()
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[tokio::test]
    async fn shutdown_subscription_flushes_final_event_before_eof() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let opts = ServeOptions {
            test_ready: Some(Box::new(move |_, _| {
                ready_tx.send(()).ok();
            })),
            ..Default::default()
        };
        let task = tokio::spawn(serve_with(data.clone(), opts, async {
            let _ = stop_rx.await;
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx)
            .await
            .unwrap()
            .unwrap();
        let stream = tokio::net::UnixStream::connect(data.join("crewd.sock"))
            .await
            .unwrap();
        let (rd, mut wr) = stream.into_split();
        wr.write_all(
            b"{\"id\":1,\"method\":\"events.subscribe\",\"params\":{\"topics\":[\"system\"]}}\n",
        )
        .await
        .unwrap();
        let mut lines = BufReader::new(rd).lines();
        let ack: serde_json::Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(ack["id"], 1);
        assert!(ack.get("result").is_some());
        stop_tx.send(()).unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("shutdown event before EOF");
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["event"], crewd_proto::events::SYSTEM_SHUTDOWN);
        assert_eq!(event["data"]["reason"], "stop");
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
