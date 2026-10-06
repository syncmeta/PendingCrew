//! claude 会话号账本的 SQLite 实现（`crewd_exec::resume::AgentIdLedger`）。
//! 每次读写各开一条连接：一次启动只读写一两回，不值得跟 RPC 抢 `AppState` 那把锁。
use std::io;
use std::path::PathBuf;

use crewd_exec::resume::AgentIdLedger;
use crewd_store::{Store, sessions};

pub struct StoreLedger {
    db: PathBuf,
    session_id: String,
}

impl StoreLedger {
    /// `db` = `<数据目录>/crewd.db`；`session_id` = crewd 的 session id（行必须已存在）。
    pub fn new(db: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            db,
            session_id: session_id.into(),
        }
    }
}

impl AgentIdLedger for StoreLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        let store = Store::open(&self.db).map_err(io::Error::other)?;
        sessions::agent_session_id(store.conn(), &self.session_id).map_err(io::Error::other)
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        let store = Store::open(&self.db).map_err(io::Error::other)?;
        sessions::set_agent_session_id(store.conn(), &self.session_id, agent_session_id)
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Session};
    use crewd_store::crews;

    fn seeded(dir: &std::path::Path) -> PathBuf {
        let db = dir.join("crewd.db");
        let s = Store::open(&db).unwrap();
        let crew = Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        crews::upsert(s.conn(), &crew, None).unwrap();
        let ses = Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: "worker".into(),
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
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
        assert!(ledger.recorded().is_err());
        assert!(ledger.record("x").is_err());
    }
}
