//! 表结构与迁移。版本号存在 SQLite 的 `PRAGMA user_version`。
use crate::{Result, StoreError};
use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 1;

const V1: &str = r#"
BEGIN;
CREATE TABLE meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE crews(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  number INTEGER,
  builtin INTEGER NOT NULL DEFAULT 0,
  working_directory TEXT,
  captain_agent_kind TEXT,
  hidden_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  extra TEXT
);
CREATE TABLE crew_parents(
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  parent_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  PRIMARY KEY(crew_id, parent_id)
);
CREATE TABLE sessions(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  agent_kind TEXT NOT NULL,
  agent_session_id TEXT,
  display_name TEXT,
  working_directory TEXT,
  model TEXT,
  effort TEXT,
  updated_at TEXT NOT NULL,
  extra TEXT
);
CREATE TABLE messages(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  sender_kind TEXT NOT NULL,
  sender_session_id TEXT,
  sender_name TEXT,
  text TEXT NOT NULL,
  category TEXT,
  in_reply_to TEXT,
  mentions TEXT NOT NULL DEFAULT '[]',
  attachments TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  extra TEXT
);
CREATE INDEX messages_crew_seq ON messages(crew_id, seq);
CREATE TABLE todos(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  ledger TEXT NOT NULL CHECK(ledger IN ('agent','human')),
  number INTEGER NOT NULL,
  text TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT,
  deleted_at TEXT,
  attachments TEXT NOT NULL DEFAULT '[]',
  extra TEXT,
  UNIQUE(crew_id, ledger, number)
);
CREATE TABLE todo_responses(
  id TEXT PRIMARY KEY,
  todo_id TEXT NOT NULL REFERENCES todos(id) ON DELETE CASCADE,
  ord INTEGER NOT NULL,
  session_id TEXT NOT NULL,
  sender_name TEXT,
  text TEXT NOT NULL,
  status TEXT,
  attachments TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL
);
CREATE INDEX todo_responses_todo ON todo_responses(todo_id, ord);
CREATE TABLE legacy_archive(
  path TEXT PRIMARY KEY,
  content BLOB NOT NULL
);
PRAGMA user_version = 1;
COMMIT;
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(StoreError::Invalid(format!(
            "数据库版本 {v} 比本程序支持的 {SCHEMA_VERSION} 新，拒绝打开"
        )));
    }
    if v < 1 {
        conn.execute_batch(V1)?;
    }
    Ok(())
}
