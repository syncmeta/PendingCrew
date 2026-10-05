//! crewd 的持久化层。crewd 是数据库的唯一写者；其他进程一律经 crewd 的 API。
pub mod crews;
pub mod schema;
pub mod sessions;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // journal_mode 会返回一行结果，所以用 query_row 而不是 execute。
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let v = f(&tx)?;
        tx.commit()?;
        Ok(v)
    }

    /// 没有任何机组、也没有任何消息。导入只往空库里写。
    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages)",
            [],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// 只认 UNIQUE（2067）与 PRIMARY KEY（1555）两种扩展码；
/// 外键、NOT NULL、CHECK 等其他约束失败不算“已存在”。
pub fn is_unique_violation(e: &rusqlite::Error) -> bool {
    use rusqlite::ffi::{SQLITE_CONSTRAINT_PRIMARYKEY, SQLITE_CONSTRAINT_UNIQUE};
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == SQLITE_CONSTRAINT_UNIQUE
                || f.extended_code == SQLITE_CONSTRAINT_PRIMARYKEY
    )
}

/// 把一列 JSON 文本解成类型；失败转成 rusqlite 的列转换错误，便于在 query_map 里用 `?`。
#[allow(dead_code)] // used from Task 3
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    idx: usize,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Session};
    use serde_json::json;

    fn crew(id: &str, n: Option<i64>) -> Crew {
        Crew {
            id: id.into(),
            title: format!("机组 {id}"),
            number: n,
            builtin: false,
            parent_ids: vec![],
            working_directory: Some("/tmp/w".into()),
            captain_agent_kind: Some("claude_code".into()),
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn fresh_store_is_empty_and_versioned() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.is_empty().unwrap());
        let v: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn reopening_file_store_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let s = Store::open(&p).unwrap();
            crews::upsert(s.conn(), &crew("a", Some(1)), None).unwrap();
        }
        let s = Store::open(&p).unwrap();
        assert_eq!(crews::list(s.conn()).unwrap().len(), 1);
        assert!(!s.is_empty().unwrap());
    }

    #[test]
    fn newer_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let c = rusqlite::Connection::open(&p).unwrap();
            c.execute_batch("PRAGMA user_version = 99").unwrap();
        }
        let err = Store::open(&p).err().unwrap();
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn meta_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap(), None);
        meta_set(s.conn(), "k", "v1").unwrap();
        meta_set(s.conn(), "k", "v2").unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn crews_list_includes_parents_and_extra_is_kept() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(
            s.conn(),
            &crew("a", Some(2)),
            Some(&json!({"runtimeLocation":"local_host"})),
        )
        .unwrap();
        crews::upsert(s.conn(), &crew("b", Some(1)), None).unwrap();
        crews::set_parents(s.conn(), "b", &["a".to_string()]).unwrap();
        let list = crews::list(s.conn()).unwrap();
        assert_eq!(
            list.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(list[0].parent_ids, vec!["a".to_string()]);
        let extra: String = s
            .conn()
            .query_row("SELECT extra FROM crews WHERE id='a'", [], |r| r.get(0))
            .unwrap();
        assert!(extra.contains("local_host"));
        assert!(crews::exists(s.conn(), "a").unwrap());
        assert!(!crews::exists(s.conn(), "zz").unwrap());
    }

    #[test]
    fn with_tx_rolls_back_on_error() {
        let mut s = Store::open_in_memory().unwrap();
        let r: Result<()> = s.with_tx(|tx| {
            crews::upsert(tx, &crew("a", None), None)?;
            Err(StoreError::Invalid("boom".into()))
        });
        assert!(r.is_err());
        assert!(s.is_empty().unwrap());
    }

    #[test]
    fn sessions_upsert_and_list() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();
        let mut ses = Session {
            id: "captain-1".into(),
            crew_id: "a".into(),
            role: "captain".into(),
            agent_kind: "claude_code".into(),
            agent_session_id: Some("uuid-1".into()),
            display_name: Some("机长".into()),
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        ses.display_name = Some("新机长".into());
        sessions::upsert(s.conn(), &ses, None).unwrap();
        let list = sessions::list_for_crew(s.conn(), "a").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name.as_deref(), Some("新机长"));
    }

    #[test]
    fn is_unique_violation_only_matches_unique_and_primary_key() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();

        // PRIMARY KEY 冲突
        let pk = s
            .conn()
            .execute(
                "INSERT INTO crews(id,title,builtin,created_at,updated_at) VALUES('a','t',0,'x','x')",
                [],
            )
            .unwrap_err();
        assert!(is_unique_violation(&pk));

        // UNIQUE 冲突（messages.id 是 UNIQUE 列）
        let ins = "INSERT INTO messages(id,crew_id,sender_kind,text,created_at) VALUES('m1','a','human','t','x')";
        s.conn().execute(ins, []).unwrap();
        let uq = s.conn().execute(ins, []).unwrap_err();
        assert!(is_unique_violation(&uq));

        // FOREIGN KEY 失败不算
        let fk = match crews::set_parents(s.conn(), "a", &["nope".to_string()]) {
            Err(StoreError::Sql(e)) => e,
            other => panic!("expected sql error, got {other:?}"),
        };
        assert!(!is_unique_violation(&fk));

        // NOT NULL 失败不算
        let nn = s
            .conn()
            .execute("INSERT INTO crews(id) VALUES('z')", [])
            .unwrap_err();
        assert!(!is_unique_violation(&nn));
    }

    #[test]
    fn now_ts_is_utc_millis() {
        let t = now_ts();
        assert_eq!(t.len(), "2026-10-05T00:00:00.000Z".len());
        assert!(t.ends_with('Z'));
    }
}
