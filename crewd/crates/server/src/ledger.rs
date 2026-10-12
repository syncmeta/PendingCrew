//! claude 会话号账本的 SQLite 实现（`crewd_exec::resume::AgentIdLedger`）。
//! serve 使用 DbLedger，与 RPC 共用同一 DB FIFO；独立命令保留每次开连接的 StoreLedger。
use std::io;
use std::path::PathBuf;

use crewd_exec::resume::{AgentIdLedger, check_agent_session_id};
use crewd_store::{Store, StoreError, sessions};

pub struct StoreLedger {
    db: PathBuf,
    session_id: String,
    expected_executor: Option<String>,
}

impl StoreLedger {
    /// `db` = `<数据目录>/crewd.db`；`session_id` = crewd 的 session id（行必须已存在）。
    pub fn new(db: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            db,
            session_id: session_id.into(),
            expected_executor: None,
        }
    }

    /// 只认这种执行器的 session：行里的 `executor` 不是它就报 `InvalidInput`，不读也不写。
    /// （拿 claude 的会话号去 `thread/resume` 必然失败，退回新 thread 时还会把 claude 的号覆盖掉。）
    pub fn expecting(mut self, executor: impl Into<String>) -> Self {
        self.expected_executor = Some(executor.into());
        self
    }
}

/// 缺行是 `NotFound`（调用方能和别的失败分开）；其余（含库忙）是 `Other`。
fn to_io(e: StoreError) -> io::Error {
    match e {
        StoreError::NotFound(m) => io::Error::new(io::ErrorKind::NotFound, m),
        other => io::Error::other(other),
    }
}

impl StoreLedger {
    /// 库不存在 = `NotFound`，绝不让 `Connection::open` 顺手建出一个空库。
    fn open(&self) -> io::Result<Store> {
        if !self.db.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("数据库不存在：{}", self.db.display()),
            ));
        }
        Store::open(&self.db).map_err(to_io)
    }

    fn check_executor(&self, store: &Store) -> io::Result<()> {
        let Some(want) = &self.expected_executor else {
            return Ok(());
        };
        let actual = sessions::executor(store.conn(), &self.session_id).map_err(to_io)?;
        if &actual != want {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "session {} 的执行器是 {actual}，不是 {want}",
                    self.session_id
                ),
            ));
        }
        Ok(())
    }
}

impl AgentIdLedger for StoreLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        let store = self.open()?;
        self.check_executor(&store)?;
        sessions::agent_session_id(store.conn(), &self.session_id).map_err(to_io)
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        check_agent_session_id(agent_session_id)?;
        let store = self.open()?;
        self.check_executor(&store)?;
        sessions::set_agent_session_id(store.conn(), &self.session_id, agent_session_id)
            .map_err(to_io)
    }
}

/// Serve ledger: the driver calls this from its blocking task, and the writes
/// join the server's DB FIFO instead of racing collab read-to-write transactions.
/// The legacy standalone StoreLedger remains available for non-serve callers.
pub struct DbLedger {
    db: crewd_store::Db,
    session_id: String,
}

impl DbLedger {
    pub fn new(db: crewd_store::Db, session_id: impl Into<String>) -> Self {
        Self {
            db,
            session_id: session_id.into(),
        }
    }
}

impl AgentIdLedger for DbLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        let session = self.session_id.clone();
        self.db
            .call_result_blocking(move |s| sessions::agent_session_id(s.conn(), &session))
            .map_err(to_io)?
            .map_err(to_io)
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        check_agent_session_id(agent_session_id)?;
        let session = self.session_id.clone();
        let id = agent_session_id.to_owned();
        self.db
            .call_result_blocking(move |s| sessions::set_agent_session_id(s.conn(), &session, &id))
            .map_err(to_io)?
            .map_err(to_io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Role, Session};
    use crewd_store::crews;

    fn seeded(dir: &std::path::Path) -> PathBuf {
        let db = dir.join("crewd.db");
        let s = Store::open(&db).unwrap();
        let crew = Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            management: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        crews::upsert(s.conn(), &crew, None).unwrap();
        let ses = Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: Role::Worker,
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        db
    }

    #[test]
    fn db_ledger_keeps_validation_missing_and_closed_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = seeded(dir.path());
        let db = crewd_store::Db::spawn(Store::open(&path).unwrap()).unwrap();
        let ledger = DbLedger::new(db.clone(), "w-1");
        assert_eq!(ledger.recorded().unwrap(), None);
        assert_eq!(
            ledger.record("bad/id").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(ledger.recorded().unwrap(), None);
        ledger.record("synthetic-id").unwrap();
        assert_eq!(ledger.recorded().unwrap().as_deref(), Some("synthetic-id"));
        let missing = DbLedger::new(db.clone(), "missing");
        assert_eq!(
            missing.recorded().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            missing.record("synthetic-id").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            db.health().snapshot().is_none(),
            "valid operations and invalid/missing-session business errors add no DB fault"
        );
        db.shutdown();
        assert_eq!(ledger.recorded().unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(
            ledger.record("synthetic-id").unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn records_and_reads_back_through_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        let ledger = StoreLedger::new(db.clone(), "w-1");
        assert_eq!(ledger.recorded().unwrap(), None);
        ledger.record("uuid-1").unwrap();
        assert_eq!(
            StoreLedger::new(db, "w-1").recorded().unwrap().as_deref(),
            Some("uuid-1"),
            "换一个实例（= 重启后）照样读得到"
        );
    }

    #[test]
    fn unknown_session_is_an_error_not_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        let ledger = StoreLedger::new(db, "ghost");
        assert_eq!(
            ledger.recorded().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            ledger.record("x").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn invalid_ids_are_rejected_and_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        let ledger = StoreLedger::new(db, "w-1");
        for bad in ["../../etc/x", "x\0", "", "-rf", "a/b"] {
            assert_eq!(
                ledger.record(bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
        assert_eq!(ledger.recorded().unwrap(), None);
    }

    #[test]
    fn missing_database_is_not_found_and_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nope.db");
        let ledger = StoreLedger::new(db.clone(), "w-1");
        assert_eq!(
            ledger.recorded().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            ledger.record("ok-1").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!db.exists(), "不能留下一个空库");
    }

    #[test]
    fn a_ledger_expecting_codex_refuses_a_claude_session_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        StoreLedger::new(db.clone(), "w-1")
            .record("claude-uuid")
            .unwrap();
        let codex = StoreLedger::new(db.clone(), "w-1").expecting("codex");
        let err = codex.recorded().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("claude"), "{err}");
        assert_eq!(
            codex.record("thr-1").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            StoreLedger::new(db.clone(), "w-1")
                .expecting("claude")
                .recorded()
                .unwrap()
                .as_deref(),
            Some("claude-uuid"),
            "claude 的会话号原样留着"
        );
    }

    #[test]
    fn s4_ledger_nested_recorded_sql_observes_once_before_io_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let path = seeded(dir.path());
        let db = crewd_store::Db::spawn(Store::open(&path).unwrap()).unwrap();
        let health = db.health();
        let ledger = DbLedger::new(db.clone(), "w-1");
        // Actual owned schema rename leaves the real ledger SELECT unable to find its table.
        let setup = db.call_blocking(|s| {
            s.conn()
                .execute_batch("ALTER TABLE sessions RENAME TO s4_ledger_held_sessions")
        });
        // Original raw owning query, not a handcrafted error value. Plain call must not observe nested results.
        let control = db.call_blocking(|s| sessions::agent_session_id(s.conn(), "w-1"));
        let before = health.snapshot();
        let failed = ledger.recorded();
        let observed = health.snapshot();
        let restored = db.call_blocking(|s| {
            s.conn()
                .execute_batch("ALTER TABLE s4_ledger_held_sessions RENAME TO sessions")
        });
        let recovered = ledger.recorded();
        let later = health.snapshot();
        drop(ledger);
        db.shutdown();
        drop(dir);
        assert!(
            setup.is_ok_and(|r| r.is_ok()) && restored.is_ok_and(|r| r.is_ok()),
            "actual owned schema setup/restore before semantics"
        );
        assert!(
            matches!(&control, Ok(Err(StoreError::Sql(rusqlite::Error::SqliteFailure(f, _)))) if f.extended_code == rusqlite::ffi::SQLITE_ERROR),
            "real owning SQLite missing-table control"
        );
        assert!(
            before.is_none(),
            "plain Db call does not observe its nested result"
        );
        assert!(
            matches!(&failed, Err(e) if e.kind() == io::ErrorKind::Other),
            "original ledger SQL-to-I/O vocabulary stays unchanged"
        );
        assert!(
            matches!(recovered, Ok(None)),
            "same FIFO ledger recovers after owned schema restore"
        );
        assert_eq!(observed, later, "successful ledger read retains history");
        // First intended missing-mechanism target on the structural health floor.
        assert_eq!(
            observed.as_ref().map(|f| f.sequence),
            Some(1),
            "one real ledger read SQL failure observed before I/O conversion"
        );
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    }

    #[test]
    fn s4_ledger_nested_record_sql_observes_once_and_preserves_prior_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = seeded(dir.path());
        let db = crewd_store::Db::spawn(Store::open(&path).unwrap()).unwrap();
        let health = db.health();
        let ledger = DbLedger::new(db.clone(), "w-1");
        let initial = ledger.record("s4-original-id");
        let setup = db.call_blocking(|s| s.conn().execute_batch(
            "CREATE TRIGGER s4_ledger_reject BEFORE UPDATE OF agent_session_id ON sessions BEGIN SELECT RAISE(ABORT,'S4_LEDGER_SQL_TRIGGER'); END"
        ));
        let control =
            db.call_blocking(|s| sessions::set_agent_session_id(s.conn(), "w-1", "s4-control-id"));
        let before = health.snapshot();
        let failed = ledger.record("s4-failed-id");
        let observed = health.snapshot();
        let unchanged = ledger.recorded();
        let restored =
            db.call_blocking(|s| s.conn().execute_batch("DROP TRIGGER s4_ledger_reject"));
        let next = ledger.record("s4-next-id");
        let recovered = ledger.recorded();
        let later = health.snapshot();
        drop(ledger);
        db.shutdown();
        drop(dir);
        assert!(
            initial.is_ok() && setup.is_ok_and(|r| r.is_ok()) && restored.is_ok_and(|r| r.is_ok()),
            "real owned trigger setup/removal before semantics"
        );
        assert!(
            matches!(&control, Ok(Err(StoreError::Sql(rusqlite::Error::SqliteFailure(f, Some(message))))) if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == "S4_LEDGER_SQL_TRIGGER"),
            "real SQLite1811 control, not fabricated error"
        );
        assert!(before.is_none());
        assert!(
            matches!(&failed, Err(e) if e.kind() == io::ErrorKind::Other),
            "original ledger I/O error preserved"
        );
        assert_eq!(
            unchanged.unwrap().as_deref(),
            Some("s4-original-id"),
            "failed ledger update cannot overwrite prior committed id"
        );
        assert!(next.is_ok());
        assert_eq!(
            recovered.unwrap().as_deref(),
            Some("s4-next-id"),
            "same FIFO ledger recovers once trigger removed"
        );
        assert_eq!(
            observed, later,
            "successful writes/read do not erase/add fault"
        );
        // First intended target. Later kind/stage are NOTREACHED if this fails.
        assert_eq!(
            observed.as_ref().map(|f| f.sequence),
            Some(1),
            "one actual ledger update SQL failure observed before I/O conversion"
        );
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    }
}
