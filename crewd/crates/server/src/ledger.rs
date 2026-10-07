//! claude 会话号账本的 SQLite 实现（`crewd_exec::resume::AgentIdLedger`）。
//! 每次读写各开一条连接：一次启动只读写一两回，不值得跟 RPC 抢 `AppState` 那把锁。
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
}
