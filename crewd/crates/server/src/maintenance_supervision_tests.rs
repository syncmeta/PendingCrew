//! Supplemental owning callers: actual serve-created stdworker, no provider or manual Ticker.
use super::*;
use crewd_store::backup::{Policy, TickerTestControl, TickerTestFault};
use std::time::Duration;
use tokio::sync::oneshot;
const WAIT: Duration = Duration::from_secs(5);
struct CloseGate {
    entered: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
}
struct Rig {
    dir: Option<tempfile::TempDir>,
    data: PathBuf,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    stop: Option<oneshot::Sender<()>>,
    state: Option<Arc<AppState>>,
    control: Option<TickerTestControl>,
    closed: Option<oneshot::Receiver<()>>,
    gate: Option<CloseGate>,
    completed: Option<bool>,
    task_joined: bool,
    setup: bool,
}
struct Cleanup {
    completed: Option<bool>,
    task_joined: bool,
    worker_joined: Option<bool>,
    db_closed: bool,
    socket_absent: bool,
    lock_released: bool,
    storage_closed: bool,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(gate) = self.gate.as_mut()
            && let Some(release) = gate.release.take()
        {
            let _ = release.send(());
        }
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl Rig {
    async fn new(policy: Policy, closing_gate: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        ensure_data_dir(&data).unwrap();
        let store = crewd_store::Store::open_data_dir(&data).unwrap();
        crewd_store::crews::upsert(
            store.conn(),
            &crewd_proto::Crew {
                id: "s4-supplement-crew".into(),
                title: "synthetic".into(),
                created_at: "2026-10-01T00:00:00.000Z".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        drop(store);
        let (ready_tx, ready_rx) = oneshot::channel();
        let (backup_tx, backup_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (closed_tx, closed_rx) = oneshot::channel();
        let mut opts = ServeOptions {
            test_backup_policy: Some(policy),
            test_ready: Some(Box::new(move |state, _| {
                let _ = ready_tx.send(state);
            })),
            test_backup_ready: Some(Box::new(move |control| {
                let _ = backup_tx.send(control);
            })),
            test_storage_closed: Some(Box::new(move |_| {
                let _ = closed_tx.send(());
            })),
            ..Default::default()
        };
        let gate = if closing_gate {
            let (entered_tx, entered) = oneshot::channel();
            let (release, release_rx) = oneshot::channel();
            opts.test_storage_closing = Some((entered_tx, release_rx));
            Some(CloseGate {
                entered,
                release: Some(release),
            })
        } else {
            None
        };
        let running_data = data.clone();
        let task = tokio::spawn(serve_with(running_data, opts, async {
            let _ = stop_rx.await;
        }));
        let state = tokio::time::timeout(WAIT, ready_rx)
            .await
            .ok()
            .and_then(Result::ok);
        let control = tokio::time::timeout(WAIT, backup_rx)
            .await
            .ok()
            .and_then(Result::ok);
        let setup = state.is_some() && control.is_some();
        Self {
            dir: Some(dir),
            data,
            task: Some(task),
            stop: Some(stop_tx),
            state,
            control,
            closed: Some(closed_rx),
            gate,
            completed: None,
            task_joined: false,
            setup,
        }
    }
    async fn first_tick(&mut self) -> bool {
        match self.control.as_mut() {
            Some(c) => matches!(tokio::time::timeout(WAIT, c.first_tick()).await, Ok(Ok(()))),
            None => false,
        }
    }
    fn request_stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
    async fn fault(&mut self, panic: bool) -> (bool, Option<TickerTestFault>) {
        let Some(c) = self.control.as_mut() else {
            return (false, None);
        };
        let sent = if panic {
            c.panic_worker()
        } else {
            c.return_worker()
        }
        .is_ok();
        let entry = tokio::time::timeout(WAIT, c.fault_entered())
            .await
            .ok()
            .and_then(Result::ok);
        (sent, entry)
    }
    async fn early(&mut self) -> Option<bool> {
        let Some(task) = self.task.as_mut() else {
            return self.completed;
        };
        match tokio::time::timeout(Duration::from_secs(2), task).await {
            Ok(Ok(result)) => {
                self.completed = Some(result.is_err());
                self.task_joined = true;
                self.task.take();
                self.completed
            }
            Ok(Err(_)) => {
                self.task.take();
                self.completed = None;
                self.task_joined = false;
                None
            }
            Err(_) => None,
        }
    }
    async fn hello(&self, id: u64) -> Result<serde_json::Value, String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let stream = tokio::net::UnixStream::connect(self.data.join("crewd.sock"))
            .await
            .map_err(|e| e.to_string())?;
        let (read, mut write) = stream.into_split();
        write
            .write_all(
                format!("{{\"id\":{id},\"method\":\"system.hello\",\"params\":null}}\n").as_bytes(),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut read = BufReader::new(read);
        let mut line = String::new();
        let n = tokio::time::timeout(WAIT, read.read_line(&mut line))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("actual hello EOF".into());
        }
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
    async fn close(mut self) -> Cleanup {
        self.request_stop();
        if let Some(g) = self.gate.as_mut()
            && let Some(tx) = g.release.take()
        {
            let _ = tx.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(WAIT, &mut task).await {
                Ok(Ok(result)) => {
                    self.completed = Some(result.is_err());
                    self.task_joined = true;
                }
                _ => {
                    task.abort();
                    let _ = task.await;
                    self.task_joined = false;
                }
            }
        }
        let worker_joined = match self.control.as_mut() {
            Some(c) => tokio::time::timeout(WAIT, c.joined())
                .await
                .ok()
                .and_then(Result::ok),
            None => None,
        };
        let storage_closed = match self.closed.take() {
            Some(rx) => matches!(tokio::time::timeout(WAIT, rx).await, Ok(Ok(()))),
            None => false,
        };
        let db_closed = match self.state.take() {
            Some(st) => matches!(
                st.db.call(|_| ()).await,
                Err(crewd_store::StoreError::DbClosed)
            ),
            None => false,
        };
        let store = crewd_store::Store::open_data_dir(&self.data).unwrap();
        let clean = crewd_store::meta_get(store.conn(), "lifecycle.clean").unwrap();
        assert_eq!(
            clean.as_deref() == Some("true"),
            self.completed == Some(false)
        );
        drop(store);
        let socket_absent = !self.data.join("crewd.sock").exists();
        let lock_released = lock_data_dir(&self.data).is_ok();
        let result = Cleanup {
            completed: self.completed,
            task_joined: self.task_joined,
            worker_joined,
            db_closed,
            socket_absent,
            lock_released,
            storage_closed,
        };
        self.control.take();
        self.dir.take();
        result
    }
}
fn cleaned(c: &Cleanup) {
    assert!(
        c.task_joined
            && c.worker_joined.is_some()
            && c.storage_closed
            && c.db_closed
            && c.socket_absent
            && c.lock_released,
        "actual joined storage/socket/lock cleanup before target"
    );
}
#[tokio::test]
async fn s4_actual_serve_ticker_panic_is_automatic_orderly_fatal() {
    let mut rig = Rig::new(Policy::default(), false).await;
    let setup = rig.setup;
    let tick = rig.first_tick().await;
    let (sent, entered) = rig.fault(true).await;
    let early = rig.early().await;
    let cleanup = rig.close().await;
    cleaned(&cleanup);
    assert!(setup && tick && sent);
    assert_eq!(entered, Some(TickerTestFault::PanicEntered));
    assert_eq!(cleanup.worker_joined, Some(false));
    assert_eq!(
        early,
        Some(true),
        "real worker panic must stop serve before external fallback"
    );
}
#[tokio::test]
async fn s4_actual_serve_ticker_unexpected_return_is_automatic_orderly_fatal() {
    let mut rig = Rig::new(Policy::default(), false).await;
    let setup = rig.setup;
    let tick = rig.first_tick().await;
    let (sent, entered) = rig.fault(false).await;
    let early = rig.early().await;
    let cleanup = rig.close().await;
    cleaned(&cleanup);
    assert!(setup && tick && sent);
    assert_eq!(entered, Some(TickerTestFault::ReturnEntered));
    assert_eq!(cleanup.worker_joined, Some(true));
    assert_eq!(
        early,
        Some(true),
        "unexpected actual return must stop serve before external fallback"
    );
}
#[tokio::test]
async fn s4_actual_serve_ticker_closing_panic_retains_fatal_after_all_cleanup() {
    let mut rig = Rig::new(Policy::default(), true).await;
    let setup = rig.setup;
    let tick = rig.first_tick().await;
    rig.request_stop();
    let reached = match rig.gate.as_mut() {
        Some(g) => matches!(tokio::time::timeout(WAIT, &mut g.entered).await, Ok(Ok(()))),
        None => false,
    };
    let (sent, entered) = rig.fault(true).await;
    if let Some(g) = rig.gate.as_mut()
        && let Some(tx) = g.release.take()
    {
        let _ = tx.send(());
    }
    let _ = rig.early().await;
    let cleanup = rig.close().await;
    cleaned(&cleanup);
    assert!(setup && tick && reached && sent);
    assert_eq!(entered, Some(TickerTestFault::PanicEntered));
    assert_eq!(cleanup.worker_joined, Some(false));
    assert_eq!(
        cleanup.completed,
        Some(true),
        "normal shutdown winner cannot erase actual ticker join panic"
    );
}
#[tokio::test]
async fn s4_actual_serve_ticker_requested_stop_remains_success() {
    let mut rig = Rig::new(Policy::default(), false).await;
    let setup = rig.setup;
    let tick = rig.first_tick().await;
    let cleanup = rig.close().await;
    cleaned(&cleanup);
    assert!(setup && tick);
    assert_eq!(cleanup.worker_joined, Some(true));
    assert_eq!(cleanup.completed, Some(false));
}
#[tokio::test]
async fn s4_actual_serve_created_ticker_health_reaches_actual_hello() {
    let mut rig = Rig::new(
        Policy {
            reserve_bytes: u64::MAX,
            ..Default::default()
        },
        false,
    )
    .await;
    let setup = rig.setup;
    let tick = rig.first_tick().await;
    let conn = crewd_store::backup::open_read_only(&rig.data.join(crewd_store::DB_FILE));
    let cause = conn.and_then(|conn| {
        crewd_store::backup::vacuum_into_reserving(
            &conn,
            &rig.data.join("backups/s4-actual-factory-control.db"),
            u64::MAX,
        )
    });
    let observed = rig.state.as_ref().and_then(|st| st.db.health().snapshot());
    let wire = rig.hello(752).await;
    let later = rig.state.as_ref().and_then(|st| st.db.health().snapshot());
    let cleanup = rig.close().await;
    cleaned(&cleanup);
    assert!(setup && tick);
    assert_eq!(cleanup.worker_joined, Some(true));
    assert_eq!(cleanup.completed, Some(false));
    assert!(
        matches!(cause, Err(crewd_store::StoreError::Snapshot { ref source, .. }) if matches!(source.as_ref(), crewd_store::StoreError::NoSpace { needed, available } if needed > available))
    );
    assert_eq!(observed, later);
    assert_eq!(
        observed.as_ref().map(|f| f.sequence),
        Some(1),
        "ACTUAL serve-created shared health, no manual Ticker substitute"
    );
    let fault = observed.unwrap();
    assert_eq!(
        fault.stage,
        crewd_store::db_health::DbFaultStage::Maintenance
    );
    assert_eq!(
        fault.kind,
        crewd_store::db_health::DbFaultKind::Storage(crewd_store::StorageKind::DiskFull)
    );
    let wire = wire.unwrap();
    assert_eq!(wire["id"], 752);
    assert!(wire.get("error").is_none());
    assert_eq!(
        wire["result"]["daemonHealth"]["lastDbError"]["kind"],
        "storage"
    );
    assert_eq!(wire["result"]["daemonHealth"]["lastDbError"]["sequence"], 1);
    assert_eq!(
        wire["result"]["daemonHealth"]["lastDbError"]["atMs"],
        fault.at_ms
    );
    assert_eq!(
        wire["result"]["daemonHealth"]["lastDbError"]["stage"],
        "maintenance"
    );
    assert_eq!(
        wire["result"]["daemonHealth"]["lastDbError"]["storageKind"],
        "disk_full"
    );
}
