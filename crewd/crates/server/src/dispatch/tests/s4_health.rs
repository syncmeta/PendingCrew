//! S4 test-first: real FIFO Db and actual NDJSON connection/hello, not literal serve/kernel proof.
use crate::agent::testkit::Fixture;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);
struct Rig {
    fixture: Option<Fixture>,
    db: crewd_store::Db,
    writer: Option<OwnedWriteHalf>,
    reader: Option<BufReader<OwnedReadHalf>>,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(task) = self.task.as_ref() {
            task.abort();
        }
    }
}
impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let db = fixture.state.db.clone();
        let (client, server) = UnixStream::pair().unwrap();
        let state = Arc::clone(&fixture.state);
        let task = tokio::spawn(crate::conn::run(state, server));
        let (reader, writer) = client.into_split();
        Self {
            fixture: Some(fixture),
            db,
            writer: Some(writer),
            reader: Some(BufReader::new(reader)),
            task: Some(task),
        }
    }
    async fn hello(&mut self, id: u64) -> Result<Value, String> {
        let mut bytes =
            serde_json::to_vec(&json!({"id":id,"method":"system.hello","params":null})).unwrap();
        bytes.push(b'\n');
        timeout(LIMIT, self.writer.as_mut().unwrap().write_all(&bytes))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        let n = timeout(LIMIT, self.reader.as_mut().unwrap().read_line(&mut line))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("actual hello connection EOF".into());
        }
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
    async fn close(mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(mut writer) = self.writer.take() {
            match timeout(LIMIT, writer.shutdown()).await {
                Ok(Ok(())) => {}
                other => errors.push(format!("client EOF: {other:?}")),
            }
        }
        self.reader.take();
        let task = self.task.as_mut().unwrap();
        match timeout(LIMIT, &mut *task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(other) => errors.push(format!("actual connection join: {other:?}")),
            Err(error) => {
                errors.push(format!("connection deadline: {error}"));
                task.abort();
                errors.push(format!(
                    "owned abort fallback: {:?}",
                    timeout(LIMIT, &mut *task).await
                ));
            }
        }
        self.task.take();
        let db = self.db.clone();
        let joined = tokio::task::spawn_blocking(move || db.shutdown()).await;
        if let Err(error) = joined {
            errors.push(format!("DB shutdown join: {error}"));
        }
        self.fixture.take();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[tokio::test]
async fn s4_actual_hello_exposes_fifo_job_panic_without_db_backed_notice() {
    let mut rig = Rig::new().await;
    let prior = rig.hello(601).await;
    let fault = rig
        .db
        .call::<(), _>(|_| panic!("S4_SYNTHETIC_FIFO_JOB_PANIC"))
        .await;
    let after = rig.hello(602).await;
    let next_job = rig
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
        })
        .await;
    let cleanup = rig.close().await;
    assert_eq!(cleanup, Ok(()), "S4 normal connection/DB cleanup");
    assert!(
        matches!(fault, Err(crewd_store::StoreError::DbJobPanicked)),
        "actual FIFO panic control"
    );
    assert_eq!(
        next_job.unwrap().unwrap(),
        1,
        "accepted FIFO survives caught job panic"
    );
    let prior = prior.expect("setup hello response");
    let after = after.expect("post-fault actual hello response");
    assert_eq!(prior["id"], 601);
    assert_eq!(after["id"], 602);
    assert!(
        after.get("error").is_none(),
        "hello is not a DB write notice"
    );
    assert_eq!(
        after["result"]["daemonHealth"]["lastDbError"]["kind"], "db_job_panicked",
        "S4 actual wire latest FIFO fault"
    );
}

#[tokio::test]
async fn s4_actual_hello_exposes_closed_db_without_querying_that_db() {
    let mut rig = Rig::new().await;
    let before = rig.hello(611).await;
    let db = rig.db.clone();
    let shutdown = tokio::task::spawn_blocking(move || db.shutdown()).await;
    let unavailable = rig.db.call(|_| 1u8).await;
    let after = rig.hello(612).await;
    let cleanup = rig.close().await;
    assert_eq!(cleanup, Ok(()), "S4 normal connection/DB cleanup");
    assert!(shutdown.is_ok(), "actual owned DB thread shutdown");
    assert!(
        matches!(unavailable, Err(crewd_store::StoreError::DbClosed)),
        "actual failed FIFO access control"
    );
    assert!(before.expect("setup hello").get("error").is_none());
    let after = after.expect("closed DB actual hello response");
    assert_eq!(after["id"], 612);
    assert!(
        after.get("error").is_none(),
        "independent health must remain readable"
    );
    assert_eq!(
        after["result"]["daemonHealth"]["lastDbError"]["kind"], "db_closed",
        "S4 actual wire fault when DB unavailable"
    );
}

// Append inside the already proposed dispatch::tests::s4_health module; same real Rig.
fn sql_trigger(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(f, Some(message))
        if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == "S4_H1_TRIGGER")
}
fn wire_fault(response: &Value, id: u64, kind: &str, storage: Option<&str>) {
    assert_eq!(response["id"], id);
    assert!(
        response.get("error").is_none(),
        "independent actual hello stays readable"
    );
    let fault = &response["result"]["daemonHealth"]["lastDbError"];
    assert_eq!(fault["kind"], kind, "S4 H1 actual wire typed cause");
    assert_eq!(fault["stage"], "job", "one original FIFO job stage");
    assert_eq!(
        fault["sequence"], 1,
        "one actual failure produces exactly one history entry"
    );
    if let Some(storage) = storage {
        assert_eq!(fault["storageKind"], storage);
    }
    let rendered = response.to_string();
    assert!(
        !rendered.contains("S4_H1_TRIGGER"),
        "raw SQL diagnostic stays off wire"
    );
}

#[tokio::test]
async fn s4_h1_actual_collab_sql_error_reaches_fifo_history_and_hello_once() {
    let mut rig = Rig::new().await;
    let state = Arc::clone(&rig.fixture.as_ref().unwrap().state);
    let before = rig.hello(621).await;
    let setup = rig.db.call(|s| s.conn().execute_batch(
        "CREATE TABLE s4_probe(v INTEGER); CREATE TRIGGER s4_reject BEFORE INSERT ON s4_probe BEGIN SELECT RAISE(ABORT,'S4_H1_TRIGGER'); END;"
    )).await;
    // Actual AppState Tx -> SQLite -> CollabError::Store. No handcrafted error or source chain.
    let result = state
        .collab(|tx| {
            tx.conn().execute("INSERT INTO s4_probe VALUES(1)", [])?;
            Ok(())
        })
        .await;
    let after = rig.hello(622).await;
    let rows = rig
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT COUNT(*) FROM s4_probe", [], |r| r.get::<_, i64>(0))
        })
        .await;
    let health = rig.db.health().snapshot();
    let healthy_job = rig
        .db
        .call(|s| s.conn().query_row("SELECT 42", [], |r| r.get::<_, i64>(0)))
        .await;
    let later = rig.db.health().snapshot();
    drop(state);
    let cleanup = rig.close().await;
    assert_eq!(
        cleanup,
        Ok(()),
        "S4 H1 normal socket/FIFO cleanup before semantics"
    );
    assert!(
        setup.is_ok_and(|r| r.is_ok()),
        "real trigger setup must succeed"
    );
    assert!(
        matches!(&result, Err(crewd_collab::CollabError::Store(crewd_store::StoreError::Sql(e))) if sql_trigger(e)),
        "actual original SQL1811 must survive unchanged"
    );
    assert_eq!(rows.unwrap().unwrap(), 0, "actual failing Tx rolled back");
    assert_eq!(healthy_job.unwrap().unwrap(), 42);
    assert_eq!(
        health, later,
        "successful work does not erase latest fault or add history"
    );
    let fault = health.expect("S4 H1 transparent Collab typed history");
    assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
    assert_eq!(fault.sequence, 1);
    assert!(before.expect("initial hello").get("error").is_none());
    wire_fault(&after.expect("actual post-failure hello"), 622, "sql", None);
}

#[tokio::test]
async fn s4_h1_direct_sql_result_uses_same_fifo_typed_history() {
    let mut rig = Rig::new().await;
    let setup = rig.db.call(|s| s.conn().execute_batch(
        "CREATE TABLE s4_probe(v INTEGER); CREATE TRIGGER s4_reject BEFORE INSERT ON s4_probe BEGIN SELECT RAISE(ABORT,'S4_H1_TRIGGER'); END;"
    )).await;
    let result = rig
        .db
        .call_result(|s| s.conn().execute("INSERT INTO s4_probe VALUES(1)", []))
        .await;
    let after = rig.hello(632).await;
    let rows = rig
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT COUNT(*) FROM s4_probe", [], |r| r.get::<_, i64>(0))
        })
        .await;
    let health = rig.db.health().snapshot();
    let cleanup = rig.close().await;
    assert_eq!(cleanup, Ok(()));
    assert!(setup.is_ok_and(|r| r.is_ok()));
    assert!(
        matches!(&result, Ok(Err(e)) if sql_trigger(e)),
        "actual raw rusqlite error preserved"
    );
    assert_eq!(rows.unwrap().unwrap(), 0);
    assert_eq!(health.as_ref().map(|f| f.sequence), Some(1));
    wire_fault(&after.expect("raw SQL hello"), 632, "sql", None);
}

#[tokio::test]
async fn s4_h1_real_filesystem_io_failure_is_typed_not_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = Rig::new().await;
    let blocker = dir.path().join("regular-file");
    let setup = std::fs::write(&blocker, b"S4 synthetic owned file");
    let dest = blocker.join("cannot-be-a-child");
    let result = rig
        .db
        .call_result(move |_| -> crewd_store::Result<()> {
            // Actual filesystem failure ENOTDIR, no permission changes or injected Error value.
            crewd_store::fsutil::atomic_write(&dest, b"synthetic", 0o600)?;
            Ok(())
        })
        .await;
    let after = rig.hello(642).await;
    let health = rig.db.health().snapshot();
    let cleanup = rig.close().await;
    drop(dir);
    assert_eq!(cleanup, Ok(()));
    assert!(setup.is_ok());
    assert!(
        matches!(&result, Ok(Err(crewd_store::StoreError::Io(e))) if e.raw_os_error() == Some(libc::ENOTDIR)),
        "real owned filesystem error control"
    );
    assert_eq!(
        health.as_ref().map(|f| &f.kind),
        Some(&crewd_store::db_health::DbFaultKind::Storage(
            crewd_store::StorageKind::Io
        ))
    );
    wire_fault(
        &after.expect("filesystem failure hello"),
        642,
        "storage",
        Some("io"),
    );
}

#[tokio::test]
async fn s4_h1_real_snapshot_space_preflight_preserves_nested_nospace() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = Rig::new().await;
    let dest = dir.path().join("never-created.db");
    let target = dest.clone();
    let result = rig
        .db
        .call_result(move |s| {
            crewd_store::backup::vacuum_into_reserving(s.conn(), &target, u64::MAX)
        })
        .await;
    let after = rig.hello(652).await;
    let output_absent = !dest.exists();
    let health = rig.db.health().snapshot();
    let cleanup = rig.close().await;
    drop(dir);
    assert_eq!(cleanup, Ok(()));
    assert!(
        matches!(&result, Ok(Err(crewd_store::StoreError::Snapshot { source, .. })) if matches!(source.as_ref(), crewd_store::StoreError::NoSpace { needed, available } if *needed == u64::MAX && available < needed)),
        "actual preflight no-space, not filling the user's disk"
    );
    assert!(output_absent, "failed preflight creates no snapshot");
    assert_eq!(
        health.as_ref().map(|f| &f.kind),
        Some(&crewd_store::db_health::DbFaultKind::Storage(
            crewd_store::StorageKind::DiskFull
        ))
    );
    wire_fault(
        &after.expect("space preflight hello"),
        652,
        "storage",
        Some("disk_full"),
    );
}

#[tokio::test]
async fn s4_h1_real_business_refusal_does_not_create_db_fault() {
    let mut rig = Rig::new().await;
    let state = Arc::clone(&rig.fixture.as_ref().unwrap().state);
    let before = rig.db.health().snapshot();
    let result = state
        .collab(|tx| crewd_collab::crews::detail(tx, "s4-definitely-missing-crew"))
        .await;
    let after_health = rig.db.health().snapshot();
    let after = rig.hello(662).await;
    drop(state);
    let cleanup = rig.close().await;
    assert_eq!(cleanup, Ok(()));
    assert!(
        matches!(result, Err(crewd_collab::CollabError::NotFound(_))),
        "real business refusal control"
    );
    assert_eq!(
        before, after_health,
        "ordinary refusal adds no outage history"
    );
    let after = after.expect("business refusal hello");
    assert!(after.get("error").is_none());
    assert!(after["result"]["daemonHealth"]["lastDbError"].is_null());
}

#[tokio::test]
async fn s4_h1_actual_collab_snapshot_storage_error_is_not_lost_by_transparency() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = Rig::new().await;
    let state = Arc::clone(&rig.fixture.as_ref().unwrap().state);
    let dest = dir.path().join("never-created.db");
    let target = dest.clone();
    let result = state
        .collab(move |tx| {
            crewd_store::backup::vacuum_into_reserving(tx.conn(), &target, u64::MAX)?;
            Ok(())
        })
        .await;
    let after = rig.hello(672).await;
    let output_absent = !dest.exists();
    let health = rig.db.health().snapshot();
    drop(state);
    let cleanup = rig.close().await;
    drop(dir);
    assert_eq!(cleanup, Ok(()));
    assert!(
        matches!(&result, Err(crewd_collab::CollabError::Store(crewd_store::StoreError::Snapshot { source, .. })) if matches!(source.as_ref(), crewd_store::StoreError::NoSpace { needed, available } if *needed == u64::MAX && available < needed)),
        "actual Collab transparent Snapshot/NoSpace preflight result"
    );
    assert!(output_absent);
    assert_eq!(
        health.as_ref().map(|f| &f.kind),
        Some(&crewd_store::db_health::DbFaultKind::Storage(
            crewd_store::StorageKind::DiskFull
        ))
    );
    wire_fault(
        &after.expect("Collab space preflight hello"),
        672,
        "storage",
        Some("disk_full"),
    );
}
