//! 表结构与迁移。版本号存在 SQLite 的 `PRAGMA user_version`。
use crate::{Result, StoreError};
use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 3;

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

/// v2（M2b，spec §16）：执行器类型字段。`sessions.agent_kind` 改名 `executor`、
/// `crews.captain_agent_kind` 改名 `captain_executor`，词表改成 `claude` / `codex` /
/// `acp:<registry-id>`；旧数据里的 `terminal` / `unknown` 原样保留。
const V2: &str = r#"
BEGIN;
ALTER TABLE sessions RENAME COLUMN agent_kind TO executor;
UPDATE sessions SET executor='claude' WHERE executor='claude_code';
ALTER TABLE crews RENAME COLUMN captain_agent_kind TO captain_executor;
UPDATE crews SET captain_executor='claude' WHERE captain_executor='claude_code';
PRAGMA user_version = 2;
COMMIT;
"#;

/// v3（M3a，spec §22.2 + §22.7 拍板）：群、成员、注视、游标、收听、唤醒队列、熔断台账、审批、工作账本、
/// 问的附注、重启询问；sessions / crews 加列。不含 BEGIN / COMMIT：由 `migrate_v3` 包事务。
const V3_DDL: &str = r#"
CREATE TABLE channels(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('lobby','human','topic','dm')),
  name TEXT NOT NULL DEFAULT '',
  purpose TEXT,
  created_by TEXT NOT NULL,
  owner_kind TEXT CHECK(owner_kind IN ('session','human')),
  owner_id TEXT,
  pending_owner_kind TEXT CHECK(pending_owner_kind IN ('session','human')),
  pending_owner_id TEXT,
  pending_owner_by TEXT,
  pending_owner_at TEXT,
  created_at TEXT NOT NULL,
  archived_at TEXT,
  CHECK((kind IN ('lobby','human')) = (owner_kind IS NULL))
);
CREATE UNIQUE INDEX channels_one_lobby ON channels(crew_id) WHERE kind='lobby';
CREATE UNIQUE INDEX channels_one_human ON channels(crew_id) WHERE kind='human';
CREATE TABLE channel_members(
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  member_kind TEXT NOT NULL CHECK(member_kind IN ('session','human')),
  member_id TEXT NOT NULL,
  joined_at TEXT NOT NULL,
  left_at TEXT,
  PRIMARY KEY(channel_id, member_kind, member_id)
);
CREATE INDEX channel_members_member ON channel_members(member_kind, member_id);
CREATE TABLE channel_watch(
  channel_id TEXT PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
  since TEXT NOT NULL
);
CREATE TABLE messages_v3(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  sender_kind TEXT NOT NULL CHECK(sender_kind IN ('human','session','captain','system')),
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
CREATE TABLE read_cursors(
  reader TEXT NOT NULL,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(reader, channel_id)
);
CREATE TABLE listens(
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  until INTEGER,
  senders TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  PRIMARY KEY(session_id, channel_id)
);
CREATE TABLE wakes(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('message','timer','ask_answer','restart','system','checkpoint','stop_check')),
  source TEXT NOT NULL,
  origin TEXT,
  priority TEXT NOT NULL CHECK(priority IN ('human','system','auto')),
  not_before INTEGER NOT NULL,
  note TEXT,
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','inflight','delivered','dropped')),
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  settled_at TEXT,
  UNIQUE(session_id, kind, source)
);
CREATE INDEX wakes_due ON wakes(state, not_before);
CREATE TABLE wake_latches(
  scope TEXT NOT NULL CHECK(scope IN ('edge','session')),
  id TEXT NOT NULL,
  reason TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY(scope, id)
);
CREATE TABLE approvals(
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  tool_name TEXT NOT NULL,
  summary TEXT NOT NULL,
  tool_use_id TEXT,
  requested_at TEXT NOT NULL,
  decision TEXT CHECK(decision IN ('allow','deny')),
  decided_at TEXT,
  decided_by TEXT,
  consumed_at TEXT,
  CHECK(consumed_at IS NULL OR decision='allow')
);
CREATE INDEX approvals_open ON approvals(session_id, tool_name) WHERE consumed_at IS NULL;
CREATE TABLE work_books(
  crew_id TEXT PRIMARY KEY REFERENCES crews(id) ON DELETE CASCADE,
  state TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE ask_meta(
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  ask TEXT NOT NULL,
  resume_note TEXT,
  message_id TEXT,
  PRIMARY KEY(crew_id, ask)
);
CREATE TABLE restart_prompts(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at TEXT NOT NULL,
  session_ids TEXT NOT NULL,
  answer TEXT CHECK(answer IN ('resume','later')),
  answered_at TEXT
);
ALTER TABLE sessions ADD COLUMN title TEXT;
ALTER TABLE sessions ADD COLUMN brief TEXT;
ALTER TABLE sessions ADD COLUMN desired TEXT NOT NULL DEFAULT 'stopped' CHECK(desired IN ('running','stopped'));
ALTER TABLE sessions ADD COLUMN was_working INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_active_at TEXT;
ALTER TABLE sessions ADD COLUMN last_output_at TEXT;
ALTER TABLE sessions ADD COLUMN last_progress_at TEXT;
ALTER TABLE sessions ADD COLUMN awaiting_question TEXT;
ALTER TABLE sessions ADD COLUMN resume_failed_at TEXT;
-- crewd 为这个 session 新建的 git worktree 的绝对路径（控制者裁定 C8，`isolation: true` 才有）；NULL = 用机组共享目录。crewd 不删它。
ALTER TABLE sessions ADD COLUMN worktree TEXT;
ALTER TABLE sessions ADD COLUMN ext INTEGER;
ALTER TABLE crews ADD COLUMN title_source TEXT;
ALTER TABLE crews ADD COLUMN status_line TEXT;
ALTER TABLE crews ADD COLUMN policy TEXT;
ALTER TABLE crews ADD COLUMN captain_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
ALTER TABLE crews ADD COLUMN next_ext INTEGER NOT NULL DEFAULT 2;
-- 侧栏手动序（控制者裁定 C1）。`crews.position` 只管顶层（根机组之间）的次序；子机组在某个父下的次序在边上：
-- `crew_parents.position`（0 = 还没排，回填 / `crews::put_last` 给出 >= 1 的值）。多父机组在每个父下各有自己的次序。
ALTER TABLE crews ADD COLUMN position INTEGER;
ALTER TABLE crew_parents ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
"#;

/// 旧消息搬进各机组大厅（大厅先建，外键才成立）。sender 词表在这里归一（裁定 6）。
const V3_MOVE_MESSAGES: &str = r#"
INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
  SELECT id||'/lobby', id, 'lobby', '', 'system', created_at FROM crews;
INSERT INTO messages_v3(seq,id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
  SELECT seq, id, crew_id||'/lobby',
         CASE sender_kind WHEN 'user' THEN 'human' WHEN 'pendingcrew' THEN 'system' ELSE sender_kind END,
         sender_session_id, sender_name, text, category, in_reply_to, mentions, attachments, created_at, extra
  FROM messages ORDER BY seq;
DROP TABLE messages;
ALTER TABLE messages_v3 RENAME TO messages;
CREATE INDEX messages_channel_seq ON messages(channel_id, seq);
"#;

/// 一列 TEXT 的去重值（迁移前检查用）。
fn distinct_strings(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    Ok(conn
        .prepare(sql)?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// 迁移前的词表检查：有认不出的 sender / mention 词（裁定 6）或 session role（控制者裁定 R3）
/// 就整体中止，报出全部值。
fn check_v3_vocabulary(conn: &Connection) -> Result<()> {
    let senders = distinct_strings(
        conn,
        "SELECT DISTINCT sender_kind FROM messages
          WHERE sender_kind NOT IN ('user','human','session','captain','pendingcrew','system')
          ORDER BY 1",
    )?;
    let mentions = distinct_strings(
        conn,
        "SELECT DISTINCT COALESCE(json_extract(j.value,'$.kind'),'<缺 kind>')
           FROM messages m, json_each(m.mentions) j
          WHERE COALESCE(json_extract(j.value,'$.kind'),'') NOT IN ('session','captain','human','broadcast')
          ORDER BY 1",
    )?;
    let roles = distinct_strings(
        conn,
        "SELECT DISTINCT role FROM sessions
          WHERE role NOT IN ('captain','worker','admin')
          ORDER BY 1",
    )?;
    if senders.is_empty() && mentions.is_empty() && roles.is_empty() {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "v3 迁移中止（库保持 v2）：有认不出的值——消息 senderKind {senders:?} / mention kind {mentions:?} / \
         session role {roles:?}。先确认这些值的含义、在 schema.rs 的归一表里补上再升级。"
    )))
}

/// 外键检查的报告：每个 (表, 父表) 一段「表 → 父表：N 条，rowid a, b, …」，每对至多列 5 个 rowid，
/// 让人能按 rowid 找到那些行。没有不成立的外键 → None。
fn foreign_key_violations(conn: &Connection) -> Result<Option<String>> {
    const SHOWN: usize = 5;
    let mut st = conn.prepare(
        r#"SELECT "table", parent, rowid FROM pragma_foreign_key_check ORDER BY 1, 2, 3"#,
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
        ))
    })?;
    // (表, 父表, 总条数, 前几个 rowid)
    let mut groups: Vec<(String, String, usize, Vec<i64>)> = Vec::new();
    for row in rows {
        let (table, parent, rowid) = row?;
        match groups.last_mut() {
            Some(g) if g.0 == table && g.1 == parent => {
                g.2 += 1;
                if let (Some(id), true) = (rowid, g.3.len() < SHOWN) {
                    g.3.push(id);
                }
            }
            _ => groups.push((table, parent, 1, rowid.into_iter().collect())),
        }
    }
    if groups.is_empty() {
        return Ok(None);
    }
    let parts: Vec<String> = groups
        .into_iter()
        .map(|(table, parent, n, ids)| {
            let ids_text = if ids.is_empty() {
                "无 rowid".to_string()
            } else {
                let listed = ids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                if n > ids.len() {
                    format!("rowid {listed} …")
                } else {
                    format!("rowid {listed}")
                }
            };
            format!("{table} → {parent}：{n} 条，{ids_text}")
        })
        .collect();
    Ok(Some(parts.join("；")))
}

fn migrate_v3(conn: &Connection) -> Result<()> {
    check_v3_vocabulary(conn)?;
    // 重建表期间关外键（SQLite 官方的改表步骤）；事务里不能改这个 pragma，所以在事务外开关。
    conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let high_water: i64 = tx.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='messages'),0)",
            [],
            |r| r.get(0),
        )?;
        tx.execute_batch(V3_DDL)?;
        tx.execute_batch(V3_MOVE_MESSAGES)?;
        // 重建后 sqlite_sequence 只记得现存最大 seq；恢复旧高水位（裁定 7）。
        let n = tx.execute(
            "UPDATE sqlite_sequence SET seq=MAX(seq, ?1) WHERE name='messages'",
            [high_water],
        )?;
        if n == 0 && high_water > 0 {
            tx.execute(
                "INSERT INTO sqlite_sequence(name, seq) VALUES('messages', ?1)",
                [high_water],
            )?;
        }
        backfill_v3(&tx)?;
        if let Some(report) = foreign_key_violations(&tx)? {
            return Err(StoreError::Invalid(format!(
                "v3 迁移后外键不成立，已回滚（库保持 v2）：{report}"
            )));
        }
        tx.execute_batch("PRAGMA user_version = 3")?;
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("PRAGMA foreign_keys=ON")?;
    result
}

/// v3 的数据回填（幂等；迁移与旧 JSON 导入共用）：每个机组有大厅；`title_source` / worker 分机号从 extra 取；
/// `next_ext`、机长指针、`meta.nextCrewNumber`；所有 session 在自己大厅、人类在每个群的游标钉到末尾
/// （已有游标不动）。不产生任何未读、红标、唤醒（spec §22.2）。
pub fn backfill_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
  SELECT id||'/lobby', id, 'lobby', '', 'system', created_at FROM crews;
UPDATE crews SET title_source = json_extract(extra,'$.titleSource')
 WHERE title_source IS NULL AND extra IS NOT NULL AND json_valid(extra)
   AND json_type(extra,'$.titleSource')='text';
UPDATE sessions SET ext = json_extract(extra,'$.member.extensionNumber')
 WHERE ext IS NULL AND role='worker' AND extra IS NOT NULL AND json_valid(extra)
   AND json_type(extra,'$.member.extensionNumber')='integer';
UPDATE crews SET next_ext = MAX(next_ext,
  COALESCE((SELECT MAX(ext)+1 FROM sessions s WHERE s.crew_id=crews.id), 2));
UPDATE crews SET captain_session_id =
  (SELECT id FROM sessions s WHERE s.crew_id=crews.id AND s.role='captain'
    ORDER BY s.updated_at DESC, s.id DESC LIMIT 1)
 WHERE captain_session_id IS NULL;
-- Swift 时代的 captain mention 没有 target：一次性落成该群所属机组此刻的机长指针（之后可见性只认存下来的
-- target，不随机长变动）。机组没有机长就保持原样；已有 target 的、别的 mention 不动；幂等（落定后不再命中）。
UPDATE messages SET mentions = (
  SELECT json_group_array(
           CASE WHEN json_extract(j.value,'$.kind')='captain'
                 AND COALESCE(json_extract(j.value,'$.targetId'),'')=''
                THEN json_set(json(j.value),'$.targetId',
                       (SELECT cr.captain_session_id FROM channels ch JOIN crews cr ON cr.id=ch.crew_id
                         WHERE ch.id=messages.channel_id))
                ELSE json(j.value) END)
    FROM (SELECT value FROM json_each(messages.mentions) ORDER BY key) j)
 WHERE CASE WHEN json_valid(mentions) AND json_type(mentions)='array' THEN
         EXISTS (SELECT 1 FROM channels ch JOIN crews cr ON cr.id=ch.crew_id
                  WHERE ch.id=messages.channel_id AND cr.captain_session_id IS NOT NULL)
         AND EXISTS (SELECT 1 FROM json_each(messages.mentions) j
                      WHERE json_extract(j.value,'$.kind')='captain'
                        AND COALESCE(json_extract(j.value,'$.targetId'),'')='')
       ELSE 0 END;
-- 侧栏手动序的初值（控制者裁定 C1）：按建组先后的全局名次（只有顶层的相对次序有意义）。
UPDATE crews SET position =
  (SELECT COUNT(*) FROM crews c2 WHERE c2.created_at < crews.created_at
      OR (c2.created_at = crews.created_at AND c2.id < crews.id)) + 1
 WHERE position IS NULL;
-- 每个父下子机组的初值：按子的建组先后（再按 id）排 1..n，每个父各排各的。只填还是 0 的边：人排过的不重算（幂等）。
UPDATE crew_parents SET position =
  (SELECT COUNT(*) FROM crew_parents e2
     JOIN crews c2 ON c2.id = e2.crew_id
     JOIN crews c ON c.id = crew_parents.crew_id
    WHERE e2.parent_id = crew_parents.parent_id
      AND (c2.created_at < c.created_at OR (c2.created_at = c.created_at AND c2.id < c.id))) + 1
 WHERE position = 0;
INSERT OR IGNORE INTO read_cursors(reader,channel_id,seq,updated_at)
  SELECT s.id, s.crew_id||'/lobby',
         COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby'),0),
         strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM sessions s;
INSERT OR IGNORE INTO read_cursors(reader,channel_id,seq,updated_at)
  SELECT 'human', ch.id,
         COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=ch.id),0),
         strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM channels ch;
"#,
    )?;
    let max_no: i64 = conn.query_row("SELECT COALESCE(MAX(number),0) FROM crews", [], |r| {
        r.get(0)
    })?;
    let archived = crate::archive::get(conn, "local-crews.json")?
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("nextCrewNumber").and_then(|n| n.as_i64()))
        .unwrap_or(0);
    let current = crate::meta_get(conn, "nextCrewNumber")?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let next = current.max(archived).max(max_no + 1);
    crate::meta_set(conn, "nextCrewNumber", &next.to_string())?;
    Ok(())
}

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
    if v < 2 {
        conn.execute_batch(V2)?;
    }
    if v < 3 {
        migrate_v3(conn)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_database_is_migrated_to_executor_vocabulary() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(
            "INSERT INTO crews(id,title,captain_agent_kind,created_at,updated_at) VALUES
               ('c','t','claude_code','x','x'),
               ('c2','t','codex','x','x'),
               ('c3','t',NULL,'x','x'),
               ('c4','t','unknown','x','x');
             INSERT INTO sessions(id,crew_id,role,agent_kind,updated_at) VALUES
               ('a','c','worker','claude_code','x'),
               ('b','c','worker','codex','x'),
               ('d','c','worker','unknown','x');",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let mut stmt = conn
            .prepare("SELECT id, executor FROM sessions ORDER BY id")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("a".to_string(), "claude".to_string()),
                ("b".to_string(), "codex".to_string()),
                ("d".to_string(), "unknown".to_string()),
            ]
        );
        let mut stmt = conn
            .prepare("SELECT id, captain_executor FROM crews ORDER BY id")
            .unwrap();
        let crews: Vec<(String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            crews,
            [
                ("c".to_string(), Some("claude".to_string())),
                ("c2".to_string(), Some("codex".to_string())),
                ("c3".to_string(), None),
                ("c4".to_string(), Some("unknown".to_string())),
            ]
        );
        migrate(&conn).unwrap();
    }

    use crate::{archive, channels, messages};

    /// 一个停在 v2 的库（真实用户升级前的样子）。
    fn v2_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,extra) VALUES
  ('crew-a','示例机组',1,0,'2026-09-01T00:00:00.000Z','2026-09-01T00:00:00.000Z','{"titleSource":"human"}'),
  ('crew-b','子机组',2,0,'2026-09-02T00:00:00.000Z','2026-09-02T00:00:00.000Z',NULL),
  ('chief','总机组',NULL,1,'2026-08-01T00:00:00.000Z','2026-08-01T00:00:00.000Z',NULL);
INSERT INTO crew_parents(crew_id,parent_id) VALUES('crew-b','crew-a');
INSERT INTO sessions(id,crew_id,role,executor,updated_at,extra) VALUES
  ('captain-old','crew-a','captain','claude','2026-09-01T00:00:00.000Z',NULL),
  ('captain-new','crew-a','captain','claude','2026-09-05T00:00:00.000Z',NULL),
  ('worker-a1','crew-a','worker','claude','2026-09-01T00:00:00.000Z','{"member":{"extensionNumber":5}}'),
  ('worker-a2','crew-a','worker','codex','2026-09-01T00:00:00.000Z',NULL),
  ('worker-b1','crew-b','worker','claude','2026-09-02T00:00:00.000Z',NULL);
INSERT INTO messages(id,crew_id,sender_kind,sender_session_id,text,mentions,created_at) VALUES
  ('m1','crew-a','user',NULL,'人发的第一条','[]','2026-09-01T01:00:00.000Z'),
  ('m2','crew-a','session','worker-a1','收到','[{"kind":"session","targetId":"captain-new"}]','2026-09-01T01:01:00.000Z'),
  ('m3','crew-b','pendingcrew',NULL,'系统通知','[{"kind":"human"}]','2026-09-02T01:00:00.000Z'),
  ('m4','crew-a','captain','captain-new','@人类 请看','[{"kind":"human"},{"kind":"broadcast"}]','2026-09-01T02:00:00.000Z');
"#,
        )
        .unwrap();
        conn
    }

    fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn v2_database_migrates_to_v3_with_one_lobby_per_crew() {
        let c = v2_db();
        let seqs_before: Vec<i64> = {
            let mut st = c.prepare("SELECT seq FROM messages ORDER BY seq").unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        let lobbies: Vec<(String, String)> = {
            let mut st = c
                .prepare("SELECT id, crew_id FROM channels WHERE kind='lobby' ORDER BY id")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            lobbies,
            [
                ("chief/lobby".to_string(), "chief".to_string()),
                ("crew-a/lobby".to_string(), "crew-a".to_string()),
                ("crew-b/lobby".to_string(), "crew-b".to_string()),
            ]
        );
        let seqs_after: Vec<i64> = {
            let mut st = c.prepare("SELECT seq FROM messages ORDER BY seq").unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(seqs_before, seqs_after, "seq 原样搬");
        let rows: Vec<(String, String, String)> = {
            let mut st = c
                .prepare("SELECT id, channel_id, sender_kind FROM messages ORDER BY seq")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            rows,
            [
                ("m1".into(), "crew-a/lobby".into(), "human".into()),
                ("m2".into(), "crew-a/lobby".into(), "session".into()),
                ("m3".into(), "crew-b/lobby".into(), "system".into()),
                ("m4".into(), "crew-a/lobby".into(), "captain".into()),
            ]
        );
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
        assert_eq!(
            one::<i64>(&c, "PRAGMA foreign_keys"),
            1,
            "迁移后外键检查重新打开"
        );
    }

    #[test]
    fn migration_keeps_the_seq_high_water_mark() {
        let c = v2_db();
        c.execute("DELETE FROM messages WHERE id='m4'", []).unwrap();
        migrate(&c).unwrap();
        let m = crewd_proto::Message {
            id: "m5".into(),
            channel_id: "crew-a/lobby".into(),
            sender_kind: crewd_proto::SenderKind::Human,
            text: "迁移后的新消息".into(),
            created_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        let seq = messages::insert(&c, &m, None).unwrap().unwrap();
        assert_eq!(seq, 5, "删掉的 seq 4 不能被新消息复用");
    }

    #[test]
    fn migration_creates_no_unread_and_no_badges() {
        let c = v2_db();
        migrate(&c).unwrap();
        // 每个 session 在自己机组大厅的游标 = 该大厅最大 seq；人类在每个群的游标 = 该群最大 seq。
        let behind: i64 = one(
            &c,
            "SELECT COUNT(*) FROM sessions s
               LEFT JOIN read_cursors r ON r.reader=s.id AND r.channel_id=s.crew_id||'/lobby'
              WHERE r.seq IS NULL
                 OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby'),0)",
        );
        assert_eq!(behind, 0);
        let human_behind: i64 = one(
            &c,
            "SELECT COUNT(*) FROM channels ch
               LEFT JOIN read_cursors r ON r.reader='human' AND r.channel_id=ch.id
              WHERE r.seq IS NULL
                 OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=ch.id),0)",
        );
        assert_eq!(human_behind, 0);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM wakes"), 0);
    }

    #[test]
    fn migration_fills_captain_pointer_extensions_and_counters() {
        let c = v2_db();
        archive::put(
            &c,
            "local-crews.json",
            br#"{"version":1,"nextCrewNumber":9,"crews":[]}"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<String>(&c, "SELECT captain_session_id FROM crews WHERE id='crew-a'"),
            "captain-new",
            "多条 captain 行时取最近更新的"
        );
        assert_eq!(
            one::<Option<String>>(&c, "SELECT captain_session_id FROM crews WHERE id='crew-b'"),
            None
        );
        assert_eq!(
            one::<i64>(&c, "SELECT ext FROM sessions WHERE id='worker-a1'"),
            5
        );
        assert_eq!(
            one::<Option<i64>>(&c, "SELECT ext FROM sessions WHERE id='worker-a2'"),
            None,
            "旧数据没号的不编（同 Swift：通讯录里不放叫不出的号）"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-a'"),
            6
        );
        assert_eq!(
            one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-b'"),
            2
        );
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM crews WHERE position IS NULL"),
            0,
            "侧栏手动序的初值：每个机组都有 position（C1）"
        );
        assert!(
            one::<i64>(&c, "SELECT position FROM crews WHERE id='crew-a'")
                < one::<i64>(&c, "SELECT position FROM crews WHERE id='crew-b'"),
            "初值按建组先后"
        );
        assert_eq!(
            one::<String>(&c, "SELECT title_source FROM crews WHERE id='crew-a'"),
            "human"
        );
        assert_eq!(
            one::<String>(&c, "SELECT value FROM meta WHERE key='nextCrewNumber'"),
            "9",
            "归档里的计数器比 max(number)+1 大，取大的（号码不回收）"
        );
        assert_eq!(
            one::<String>(&c, "SELECT desired FROM sessions WHERE id='worker-a1'"),
            "stopped"
        );
    }

    #[test]
    fn unknown_sender_kind_aborts_migration_and_keeps_v2() {
        let c = v2_db();
        c.execute(
            "INSERT INTO messages(id,crew_id,sender_kind,text,created_at) VALUES('mx','crew-a','robot','?','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(err.contains("robot"), "{err}");
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='crew_id'"
            ),
            1,
            "messages 没被动过"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='channels'"
            ),
            0
        );
    }

    #[test]
    fn unknown_mention_kind_aborts_migration() {
        let c = v2_db();
        c.execute(
            "INSERT INTO messages(id,crew_id,sender_kind,text,mentions,created_at) VALUES('my','crew-a','session','?','[{\"kind\":\"everyone\"}]','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(err.contains("everyone"), "{err}");
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
    }

    /// 控制者裁定 R3：role 词表 = captain / worker / admin；别的值同 sender 一样中止、库保持 v2、报出值。
    #[test]
    fn unknown_role_aborts_migration_and_keeps_v2() {
        let c = v2_db();
        c.execute(
            "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('odd-a','crew-a','observer','claude','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(err.contains("observer"), "{err}");
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='channels'"
            ),
            0
        );
        // admin 是合法词（spec §8.1 行政处），不中止。
        let ok = v2_db();
        ok.execute(
            "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('admin-a','crew-a','admin','claude','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        migrate(&ok).unwrap();
        assert_eq!(one::<i64>(&ok, "PRAGMA user_version"), 3);
    }

    /// 外键不成立时中止的报错要让人找得到行：列出每个 (表, 父表) 及至多 5 个 rowid，库保持 v2。
    #[test]
    fn foreign_key_abort_lists_table_parent_pairs_and_rowids() {
        let c = v2_db();
        // 关外键才写得进孤儿行（合成数据）：7 条 session、1 条 todo 指向不存在的机组。
        c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        for i in 0..7 {
            c.execute(
                "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES(?1,'ghost-crew','worker','claude','2026-09-03T00:00:00.000Z')",
                [format!("orphan-{i}")],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO todos(id,crew_id,ledger,number,text,status,created_at) VALUES('t-orphan','ghost-crew','agent',1,'示例','open','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let session_rowids: Vec<i64> = {
            let mut st = c
                .prepare("SELECT rowid FROM sessions WHERE crew_id='ghost-crew' ORDER BY rowid")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let todo_rowid: i64 = one(&c, "SELECT rowid FROM todos WHERE id='t-orphan'");
        let err = migrate(&c).unwrap_err().to_string();
        let first5 = session_rowids[..5]
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            err.contains(&format!("sessions → crews：7 条，rowid {first5} …")),
            "{err}"
        );
        assert!(
            !err.contains(&format!("{}, {}", session_rowids[4], session_rowids[5])),
            "每对至多列 5 个 rowid：{err}"
        );
        assert!(
            err.contains(&format!("todos → crews：1 条，rowid {todo_rowid}；"))
                || err.ends_with(&format!("todos → crews：1 条，rowid {todo_rowid}")),
            "{err}"
        );
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2, "库保持 v2");
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='channels'"
            ),
            0
        );
    }

    #[test]
    fn migrate_and_backfill_are_idempotent() {
        let c = v2_db();
        migrate(&c).unwrap();
        let snapshot = |c: &Connection| -> (i64, i64, i64, String) {
            (
                one(c, "SELECT COUNT(*) FROM channels"),
                one(c, "SELECT COUNT(*) FROM read_cursors"),
                one(c, "SELECT SUM(next_ext) FROM crews"),
                one(c, "SELECT value FROM meta WHERE key='nextCrewNumber'"),
            )
        };
        let a = snapshot(&c);
        migrate(&c).unwrap();
        backfill_v3(&c).unwrap();
        assert_eq!(snapshot(&c), a);
    }

    #[test]
    fn fresh_database_is_v3_and_has_no_lobbies_to_fill() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM channels"), 0);
        assert_eq!(channels::lobby_id("x"), "x/lobby");
    }

    #[test]
    fn migration_orders_children_per_parent_by_creation_and_backfill_keeps_human_order() {
        let c = v2_db();
        c.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,extra) VALUES
  ('crew-c','后建的',3,0,'2026-09-03T00:00:00.000Z','2026-09-03T00:00:00.000Z',NULL),
  ('crew-d','中间建的',4,0,'2026-09-02T12:00:00.000Z','2026-09-02T12:00:00.000Z',NULL);
INSERT INTO crew_parents(crew_id,parent_id) VALUES('crew-c','crew-a'),('crew-d','crew-a'),('crew-c','crew-b');
"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        let pos = |crew: &str, parent: &str| -> i64 {
            one(
                &c,
                &format!(
                    "SELECT position FROM crew_parents WHERE crew_id='{crew}' AND parent_id='{parent}'"
                ),
            )
        };
        assert_eq!(
            (
                pos("crew-b", "crew-a"),
                pos("crew-d", "crew-a"),
                pos("crew-c", "crew-a")
            ),
            (1, 2, 3),
            "每个父下按子的建组先后排"
        );
        assert_eq!(
            pos("crew-c", "crew-b"),
            1,
            "同一个子在另一个父下有自己的次序"
        );
        c.execute(
            "UPDATE crew_parents SET position=9 WHERE crew_id='crew-d' AND parent_id='crew-a'",
            [],
        )
        .unwrap();
        backfill_v3(&c).unwrap();
        assert_eq!(pos("crew-d", "crew-a"), 9, "回填幂等：人排过的不重算");
        assert_eq!(pos("crew-b", "crew-a"), 1);
    }

    /// 旧数据里没有 target 的 captain mention：迁移时一次性落成「该群所属机组机长指针」，之后不再随机长变动；
    /// 机组没有机长的保持原样；已有 target 的、别的 mention 的顺序与内容不动；重跑幂等。
    #[test]
    fn migration_resolves_untargeted_captain_mentions_once_and_is_idempotent() {
        let c = v2_db();
        c.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,extra) VALUES
  ('crew-n','无机长',9,0,'2026-09-04T00:00:00.000Z','2026-09-04T00:00:00.000Z',NULL);
INSERT INTO messages(id,crew_id,sender_kind,sender_session_id,text,mentions,created_at) VALUES
  ('c1','crew-a','session','worker-a1','给机长','[{"kind":"human"},{"kind":"captain"},{"kind":"broadcast"}]','2026-09-01T03:00:00.000Z'),
  ('c2','crew-a','session','worker-a1','带 target','[{"kind":"captain","targetId":"captain-old"}]','2026-09-01T03:01:00.000Z'),
  ('c3','crew-b','session','worker-b1','机组乙没有机长','[{"kind":"captain"}]','2026-09-02T03:00:00.000Z'),
  ('c4','crew-n','user',NULL,'无机长机组','[{"kind":"captain"}]','2026-09-04T03:00:00.000Z'),
  ('c5','crew-a','session','worker-a1','混合','[{"kind":"broadcast"},{"kind":"captain"},{"kind":"captain","targetId":"captain-old"},{"kind":"human"}]','2026-09-01T03:02:00.000Z');
"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        let mentions = |id: &str| -> String {
            one(
                &c,
                &format!("SELECT mentions FROM messages WHERE id='{id}'"),
            )
        };
        let v = |s: String| serde_json::from_str::<serde_json::Value>(&s).unwrap();
        assert_eq!(
            v(mentions("c1")),
            serde_json::json!([{"kind":"human"},{"kind":"captain","targetId":"captain-new"},{"kind":"broadcast"}]),
            "落成机长指针，顺序不变"
        );
        assert_eq!(
            v(mentions("c2")),
            serde_json::json!([{"kind":"captain","targetId":"captain-old"}])
        );
        assert_eq!(
            v(mentions("c5")),
            serde_json::json!([{"kind":"broadcast"},{"kind":"captain","targetId":"captain-new"},{"kind":"captain","targetId":"captain-old"},{"kind":"human"}]),
            "同一条里只补没 target 的，已有的不动，顺序不变"
        );
        assert_eq!(
            v(mentions("c3")),
            serde_json::json!([{"kind":"captain"}]),
            "没有机长就保持原样"
        );
        assert_eq!(v(mentions("c4")), serde_json::json!([{"kind":"captain"}]));
        assert_eq!(
            v(mentions("m2")),
            serde_json::json!([{"kind":"session","targetId":"captain-new"}])
        );
        // 机长之后换人：已落定的 target 不跟着变；重跑回填也不改。
        c.execute(
            "UPDATE crews SET captain_session_id='worker-a1' WHERE id='crew-a'",
            [],
        )
        .unwrap();
        let before: Vec<String> = ["c1", "c2", "c3", "c4"]
            .iter()
            .map(|i| mentions(i))
            .collect();
        backfill_v3(&c).unwrap();
        let after: Vec<String> = ["c1", "c2", "c3", "c4"]
            .iter()
            .map(|i| mentions(i))
            .collect();
        assert_eq!(before, after, "幂等");
        // 给没机长的机组补上机长再回填：此后才落定（一次性）。
        c.execute(
            "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('cap-n','crew-n','captain','claude','2026-09-05T00:00:00.000Z')",
            [],
        )
        .unwrap();
        backfill_v3(&c).unwrap();
        assert_eq!(
            v(mentions("c4")),
            serde_json::json!([{"kind":"captain","targetId":"cap-n"}])
        );
    }
}
