//! 表结构与迁移。版本号存在 SQLite 的 `PRAGMA user_version`。
use crate::{Result, StoreError};
use rusqlite::{Connection, params};

pub const SCHEMA_VERSION: i64 = 4;

pub(crate) const V1: &str = r#"
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
pub(crate) const V2: &str = r#"
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
  CHECK(kind NOT IN ('lobby','human') OR owner_kind IS NULL)
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
/// 机长指针之外还标着 role='captain' 的行（旧数据一个机组多条 captain 行）：降为 worker，没号的按
/// (机组, 更新时间, id) 领下一个分机号——同 `set_captain` 对旧机长的做法（终审 M2）。幂等：降过的不再命中。
fn demote_captains_off_the_pointer(conn: &Connection) -> Result<()> {
    let stale: Vec<(String, String, Option<i64>)> = {
        let mut st = conn.prepare(
            "SELECT s.id, s.crew_id, s.ext FROM sessions s JOIN crews c ON c.id=s.crew_id
              WHERE s.role='captain' AND c.captain_session_id IS NOT s.id
              ORDER BY s.crew_id, s.updated_at, s.id",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, crew, ext) in stale {
        if ext.is_none() {
            conn.execute(
                "UPDATE sessions SET ext=(SELECT next_ext FROM crews WHERE id=?2) WHERE id=?1",
                params![id, crew],
            )?;
            conn.execute("UPDATE crews SET next_ext=next_ext+1 WHERE id=?1", [&crew])?;
        }
        conn.execute("UPDATE sessions SET role='worker' WHERE id=?1", [&id])?;
    }
    Ok(())
}

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
    demote_captains_off_the_pointer(conn)?;
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

/// v4（N1，spec §24）：删审批；组织根标记改名并保证唯一；归档列。数据部分在 `backfill_v4`。
const V4_DDL: &str = r#"
DROP INDEX IF EXISTS approvals_open;
DROP TABLE IF EXISTS approvals;
ALTER TABLE crews RENAME COLUMN builtin TO management;
ALTER TABLE crews ADD COLUMN archived_at TEXT;
"#;

/// 一个事务：DDL → 回填 → 唯一索引 → 外键检查 → 版本号。任何一步失败整体回滚，库保持 v3；
/// 错误一律带上「v4 迁移失败，已回滚（库保持 v3）」——crewd 拒绝启动时人看到的就是这句加原因。
/// 只用传进来的连接。由 `migrate_guarded` 的版本链调用：文件库的迁移前快照（integrity_check +
/// `VACUUM INTO`，S20）在链的开头、任何写之前就已拍好；拍不成整条链不跑。
fn migrate_v4(conn: &Connection) -> Result<()> {
    // 事务在 `migrate_v4_in_tx` 返回时已经落定（提交或随 drop 回滚），这里只补上下文。
    migrate_v4_in_tx(conn)
        .map_err(|e| StoreError::Invalid(format!("v4 迁移失败，已回滚（库保持 v3）：{e}")))
}

fn migrate_v4_in_tx(conn: &Connection) -> Result<()> {
    // 迁移前快照已由 `migrate_guarded` 在链开头拍好（v3 及更早的写也都在它之后）。
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(V4_DDL)?;
    backfill_v4(&tx)?;
    tx.execute_batch(
        "CREATE UNIQUE INDEX crews_one_management ON crews(management) WHERE management=1;",
    )?;
    if let Some(report) = foreign_key_violations(&tx)? {
        return Err(StoreError::Invalid(format!("迁移后外键不成立：{report}")));
    }
    tx.execute_batch("PRAGMA user_version = 4")?;
    tx.commit()?;
    Ok(())
}

/// v4 的数据回填（幂等；迁移与旧 JSON 导入共用，导入在 `backfill_v3` 之后调）。
/// 空机组表时只做与机组无关的几步（总机组由第一次建机组时懒建，N1 裁定 3）。
pub fn backfill_v4(conn: &Connection) -> Result<()> {
    // 1) 多个组织根：留 created_at 最早的（再按 id），其余降为普通机组。
    conn.execute_batch(
        "UPDATE crews SET management=0 WHERE management=1 AND id <>
           (SELECT id FROM crews WHERE management=1 ORDER BY created_at, id LIMIT 1);",
    )?;
    // 2) 作废排队中的审批唤醒（审批已删除，N1 裁定 15）。
    conn.execute(
        "UPDATE wakes SET state='dropped', last_error='approvals removed (v4)', settled_at=?1
          WHERE source LIKE 'approval:%' AND state IN ('pending','inflight')",
        [crate::now_ts()],
    )?;
    // 3) 剥掉 broadcast mention（@ 只决定叫醒谁，可见范围 = 群成员，N1 裁定 8）。其余 mention 顺序不变；
    //    在 `crews == 0` 提前返回之前——空机组表的库也可能有消息。
    conn.execute_batch(
        "UPDATE messages SET mentions = (
           SELECT json_group_array(json(value)) FROM (
             SELECT value FROM json_each(messages.mentions)
              WHERE json_extract(value,'$.kind') <> 'broadcast' ORDER BY key))
          WHERE json_valid(mentions) AND json_type(mentions)='array'
            AND EXISTS (SELECT 1 FROM json_each(messages.mentions) j WHERE json_extract(j.value,'$.kind')='broadcast');",
    )?;
    let crews: i64 = conn.query_row("SELECT COUNT(*) FROM crews", [], |r| r.get(0))?;
    if crews == 0 {
        return Ok(());
    }
    let root = crate::crews::ensure_management(conn, &crate::now_ts())?;
    // 4) 根不挂在任何人下面；号码 0（没有号、且 0 没人用时）；侧栏次序恒为 1；不隐藏。
    conn.execute("DELETE FROM crew_parents WHERE crew_id=?1", [&root])?;
    conn.execute(
        "UPDATE crews SET number=0 WHERE id=?1 AND number IS NULL
            AND NOT EXISTS (SELECT 1 FROM crews WHERE number=0)",
        [&root],
    )?;
    conn.execute("UPDATE crews SET position=1 WHERE id=?1", [&root])?;
    // 根不能被隐藏（N1 裁定 6）：旧数据里藏着的露出来。
    conn.execute("UPDATE crews SET hidden_at=NULL WHERE id=?1", [&root])?;
    // 5) 没有父边的其它机组挂到根下：次序 = 旧顶层次序（position, created_at, id），接在根已有的子之后。
    conn.execute(
        "INSERT INTO crew_parents(crew_id,parent_id,position)
           SELECT c.id, ?1,
                  (SELECT COALESCE(MAX(position),0) FROM crew_parents WHERE parent_id=?1)
                  + ROW_NUMBER() OVER (ORDER BY COALESCE(c.position,0), c.created_at, c.id)
             FROM crews c
            WHERE c.id <> ?1
              AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)",
        [&root],
    )?;
    // 6) 不变式 I-root：指向根的边只能是唯一的父（N1 裁定 4）。
    conn.execute(
        "DELETE FROM crew_parents WHERE parent_id=?1
            AND crew_id IN (SELECT crew_id FROM crew_parents WHERE parent_id <> ?1)",
        [&root],
    )?;
    // 7) 显示名与标题都空的旧 session 补标题（N1 裁定 12）：brief 第一条非空行的前 18 个字符；
    //    没有 brief = 机组标题 + ' #' + 分机号（机长没号 = 机组标题）。标签绝不回落到原始 id。
    //    SQLite 的 substr 按字符算。内部空白不压缩（只管首尾，旧数据的近似即可）。
    conn.execute_batch(
        "UPDATE sessions SET title = COALESCE(
             NULLIF(substr(trim(substr(ltrim(brief, char(9)||char(10)||char(13)||' '), 1,
                    instr(ltrim(brief, char(9)||char(10)||char(13)||' ')||char(10), char(10)) - 1),
                    char(9)||char(13)||' '), 1, 18), ''),
             (SELECT c.title FROM crews c WHERE c.id=sessions.crew_id)
               || CASE WHEN ext IS NULL THEN '' ELSE ' #'||ext END)
          WHERE COALESCE(display_name,'')='' AND COALESCE(title,'')='';",
    )?;
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_guarded(conn, None)
}

/// 迁移。`backups` = 文件库的备份目录：给了、且库是非空的旧版本时，先过 `integrity_check`、再
/// `VACUUM INTO backups/crewd-v{旧}-pre-v{新}-{时间}.db`；任何一步不成就拒绝迁移，库原样不动（S20 / S25）。
pub fn migrate_guarded(conn: &Connection, backups: Option<&std::path::Path>) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(StoreError::Invalid(format!(
            "数据库版本 {v} 比本程序支持的 {SCHEMA_VERSION} 新，拒绝打开"
        )));
    }
    if let Some(dir) = backups
        && (1..SCHEMA_VERSION).contains(&v)
        && has_data(conn)?
    {
        protect_before_migrating(conn, v, dir).map_err(|e| StoreError::MigrationRefused {
            from: v,
            source: Box::new(e),
        })?;
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
    if v < 4 {
        migrate_v4(conn)?;
    }
    Ok(())
}

/// 有没有值得保的数据（v1 起就有这两张表）。
fn has_data(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages)",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn protect_before_migrating(conn: &Connection, v: i64, dir: &std::path::Path) -> Result<()> {
    crate::backup::ensure_ok(conn, crate::backup::Check::Integrity)?;
    let name = format!(
        "crewd-v{v}-pre-v{SCHEMA_VERSION}-{}.db",
        crate::backup::stamp_from_ms(crate::now_ms())
    );
    crate::backup::vacuum_into(conn, &dir.join(name))
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
                // v4：库里有机组而没有内建机组 → 建出总机组（N1 裁定 2），其余四个挂到它下面。
                (crate::crews::MANAGEMENT_ID.to_string(), None),
            ]
        );
        migrate(&conn).unwrap();
    }

    use crate::{archive, channels, messages};

    /// v2 库里的合成数据（真实用户升级前的样子）。
    const V2_SEED: &str = r#"
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
"#;

    /// 一个停在 v2 的库。
    fn v2_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        seed_v2(&conn);
        conn
    }

    fn seed_v2(conn: &Connection) {
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute_batch(V2_SEED).unwrap();
    }

    /// 同一份 v2 数据，落在文件里（快照只对文件库做）。
    fn v2_file(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        let _: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        seed_v2(&conn);
    }

    fn snapshots(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return vec![];
        };
        let mut v: Vec<_> = rd.map(|e| e.unwrap().path()).collect();
        v.sort();
        v
    }

    /// S20：非空的旧版本库，迁移前先在 backups/ 里留一份迁移前的完整快照（旧版本号、旧数据、能通过 quick_check）。
    #[test]
    fn migrating_a_non_empty_file_db_snapshots_it_first() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        v2_file(&p);
        let s = crate::Store::open(&p).unwrap();
        assert_eq!(one::<i64>(s.conn(), "PRAGMA user_version"), SCHEMA_VERSION);
        let snaps = snapshots(&d.path().join("backups"));
        assert_eq!(snaps.len(), 1, "{snaps:?}");
        let name = snaps[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(&format!("crewd-v2-pre-v{SCHEMA_VERSION}-")) && name.ends_with(".db"),
            "{name}"
        );
        let snap = Connection::open(&snaps[0]).unwrap();
        assert_eq!(one::<i64>(&snap, "PRAGMA user_version"), 2);
        assert_eq!(one::<i64>(&snap, "SELECT COUNT(*) FROM messages"), 4);
        assert_eq!(one::<String>(&snap, "PRAGMA quick_check"), "ok");
        // 已经是新版本的库再打开，不再拍。
        drop(s);
        drop(crate::Store::open(&p).unwrap());
        assert_eq!(snapshots(&d.path().join("backups")).len(), 1);
    }

    /// S20：快照拍不出来（这里是 backups/ 只读）就不迁：库保持 v2、一个字节都不动。
    #[test]
    fn a_failed_snapshot_refuses_the_migration_and_leaves_the_db_alone() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        v2_file(&p);
        let b = d.path().join("backups");
        std::fs::create_dir(&b).unwrap();
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o500)).unwrap();
        let r = crate::Store::open(&p);
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o700)).unwrap();
        let err = r.err().expect("快照失败必须拒绝迁移");
        assert!(err.to_string().contains("拒绝迁移"), "{err}");
        let c = Connection::open(&p).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM messages"), 4);
        assert!(snapshots(&b).is_empty(), "不留半截快照");
    }

    /// S25：迁移前先过 integrity_check；没过就不迁、不快照、不删，原样留给人。
    #[test]
    fn a_corrupt_old_db_is_not_migrated() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        v2_file(&p);
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(
                "CREATE TABLE filler(id INTEGER PRIMARY KEY, t TEXT);
                 CREATE INDEX filler_t ON filler(t);
                 WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<5000)
                 INSERT INTO filler(t) SELECT hex(randomblob(50)) FROM n;
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        }
        crate::testing::scribble_near_end(&p);
        let err = crate::Store::open(&p).err().expect("坏库不能迁");
        assert_eq!(
            err.storage_kind(),
            Some(crate::StorageKind::Corrupt),
            "{err}"
        );
        assert!(err.to_string().contains("拒绝迁移"), "{err}");
        assert!(p.exists(), "绝不删");
        assert!(snapshots(&d.path().join("backups")).is_empty());
    }

    /// S25：坏处只在索引里、拷贝照样能成的库，也要被迁移前的 integrity_check 拦住（不靠快照失败顶替）。
    #[test]
    fn an_old_db_that_copies_fine_but_fails_integrity_check_is_not_migrated() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        v2_file(&p);
        crate::testing::corrupt_an_index(&Connection::open(&p).unwrap());
        let err = crate::Store::open(&p).err().expect("没过检查的库不能迁");
        assert!(
            matches!(&err, StoreError::MigrationRefused { source, .. }
                if matches!(**source, StoreError::Integrity { .. })),
            "{err:?}"
        );
        assert_eq!(
            one::<i64>(&Connection::open(&p).unwrap(), "PRAGMA user_version"),
            2
        );
        assert!(snapshots(&d.path().join("backups")).is_empty());
    }

    /// 空的旧库和全新的库都没有要保的东西：不拍快照，也不建 backups/。
    #[test]
    fn empty_or_fresh_dbs_are_not_snapshotted() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(V1).unwrap();
            c.execute_batch(V2).unwrap();
        }
        drop(crate::Store::open(&p).unwrap());
        drop(crate::Store::open(&d.path().join("fresh.db")).unwrap());
        assert!(!d.path().join("backups").exists());
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
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), SCHEMA_VERSION);
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
            7,
            "max(ext)+1 = 6，再被降级的 captain-old 领走 6（终审 M2）"
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

    /// 终审 M2：机长指针之外的 role='captain' 行降为 worker 并领分机号（同 set_captain 对旧机长的做法）；幂等。
    #[test]
    fn migration_demotes_captain_rows_off_the_pointer_to_numbered_workers_once() {
        let c = v2_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<String>(&c, "SELECT role FROM sessions WHERE id='captain-old'"),
            "worker"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT ext FROM sessions WHERE id='captain-old'"),
            6,
            "接着 worker-a1 的 5 往下发"
        );
        assert_eq!(
            one::<String>(&c, "SELECT role FROM sessions WHERE id='captain-new'"),
            "captain"
        );
        assert_eq!(
            one::<Option<i64>>(&c, "SELECT ext FROM sessions WHERE id='captain-new'"),
            None,
            "机长不占号"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-a'"),
            7
        );
        backfill_v3(&c).unwrap();
        assert_eq!(
            one::<i64>(&c, "SELECT ext FROM sessions WHERE id='captain-old'"),
            6,
            "再跑一遍不重发号"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-a'"),
            7
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sessions s JOIN crews c ON c.id=s.crew_id
                  WHERE s.role='captain' AND c.captain_session_id IS NOT s.id"
            ),
            0
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
        assert_eq!(one::<i64>(&ok, "PRAGMA user_version"), SCHEMA_VERSION);
    }

    /// 迁移前的词表检查（SQL 里的字面量）接受 proto 封闭词表里的每一个值：sender / mention / role 都不漂。
    #[test]
    fn migration_vocabulary_check_accepts_every_proto_value() {
        use crewd_proto::{MentionKind, Role, SenderKind};
        let c = v2_db();
        for (i, k) in SenderKind::ALL.iter().enumerate() {
            c.execute(
                "INSERT INTO messages(id,crew_id,sender_kind,text,created_at) VALUES(?1,'crew-a',?2,'示例','2026-09-03T00:00:00.000Z')",
                [format!("vs{i}"), k.as_str().to_string()],
            )
            .unwrap();
        }
        for (i, k) in MentionKind::ALL.iter().enumerate() {
            c.execute(
                "INSERT INTO messages(id,crew_id,sender_kind,text,mentions,created_at) VALUES(?1,'crew-a','human','示例',?2,'2026-09-03T00:00:00.000Z')",
                [format!("vm{i}"), format!(r#"[{{"kind":"{}"}}]"#, k.as_str())],
            )
            .unwrap();
        }
        for (i, r) in Role::ALL.iter().enumerate() {
            c.execute(
                "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES(?1,'crew-b',?2,'claude','2026-09-03T00:00:00.000Z')",
                [format!("vr{i}"), r.as_str().to_string()],
            )
            .unwrap();
        }
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), SCHEMA_VERSION);
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

    /// 停在 v3 的库：内建 chief、两个旧根（crew-x 人排在 crew-a 前面）、子机组 crew-b（也挂着 chief）；
    /// crew-a 有人类群；一条排队中的审批唤醒。
    fn v3_db() -> Connection {
        let c = v2_db();
        migrate_v3(&c).unwrap();
        c.execute_batch(V3_EXTRA).unwrap();
        c
    }

    const V3_EXTRA: &str = r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at) VALUES
  ('crew-x','另一个根',3,0,'2026-09-03T00:00:00.000Z','2026-09-03T00:00:00.000Z');
INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES
  ('crew-x/lobby','crew-x','lobby','','system','2026-09-03T00:00:00.000Z'),
  ('crew-a/human','crew-a','human','','system','2026-09-01T00:00:00.000Z');
UPDATE crews SET position = CASE id WHEN 'crew-x' THEN 1 WHEN 'crew-a' THEN 2 ELSE 9 END;
-- 旧数据里「既挂在 chief 下、又有别的父」的机组：v4 后根边必须去掉（不变式 I-root）。
INSERT INTO crew_parents(crew_id,parent_id,position) VALUES('crew-b','chief',0);
INSERT INTO wakes(session_id,kind,source,priority,not_before,state,attempts,created_at) VALUES
  ('worker-a1','ask_answer','approval:ap-1','human',0,'pending',0,'2026-09-04T00:00:00.000Z');
"#;

    /// 同一份 v3 数据落在文件里（WAL）。`stamped` = N4 之后的 v3 库：带 application_id / dbId，
    /// 旁边有 `crewd.db.id`（main 上的 crewd 跑过的库就是这样）；否则是 N4 之前、没有身份的 v3 库。
    fn v3_data_dir(dir: &std::path::Path, stamped: bool) {
        let p = dir.join(crate::DB_FILE);
        v2_file(&p);
        let c = Connection::open(&p).unwrap();
        migrate_v3(&c).unwrap();
        c.execute_batch(V3_EXTRA).unwrap();
        if stamped {
            crate::mark_identity(&c).unwrap();
            let _: i64 = c
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
                .unwrap();
            let id = crate::meta_get(&c, "dbId").unwrap().unwrap();
            let rec = crate::IdRecord {
                id,
                ino: Some(crate::inode_of(&p).unwrap()),
            };
            std::fs::write(dir.join(crate::ID_FILE), rec.render()).unwrap();
        }
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
    }

    /// N1 × N4：v3 → v4 走 `open_data_dir` 的守卫链——迁移前先在 backups/ 拍一份 v3 快照（审批表、
    /// builtin 列都还在），再在一个事务里升到 v4。没身份的旧库与 N4 之后带身份的库各一遍。
    #[test]
    fn a_v3_data_dir_is_snapshotted_then_migrated_to_v4_through_the_guarded_open() {
        for stamped in [false, true] {
            let d = tempfile::tempdir().unwrap();
            v3_data_dir(d.path(), stamped);
            let s = crate::Store::open_data_dir(d.path()).unwrap();
            assert_eq!(
                one::<i64>(s.conn(), "PRAGMA user_version"),
                4,
                "stamped={stamped}"
            );
            assert_eq!(
                one::<i64>(s.conn(), "SELECT COUNT(*) FROM crews WHERE management=1"),
                1
            );
            assert_eq!(
                one::<i64>(
                    s.conn(),
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='approvals'"
                ),
                0
            );
            assert_eq!(
                one::<String>(
                    s.conn(),
                    "SELECT state FROM wakes WHERE source='approval:ap-1'"
                ),
                "dropped"
            );
            let snaps = snapshots(&d.path().join(crate::backup::BACKUPS_DIR));
            assert_eq!(snaps.len(), 1, "stamped={stamped}: {snaps:?}");
            let name = snaps[0].file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("crewd-v3-pre-v4-"), "{name}");
            let snap = Connection::open(&snaps[0]).unwrap();
            assert_eq!(one::<i64>(&snap, "PRAGMA user_version"), 3);
            assert_eq!(
                one::<i64>(&snap, "SELECT COUNT(*) FROM approvals"),
                0,
                "快照是 v3 的形状：审批表还在"
            );
            assert_eq!(
                one::<i64>(&snap, "SELECT COUNT(*) FROM crews WHERE builtin=1"),
                1
            );
            assert_eq!(
                one::<String>(
                    &snap,
                    "SELECT state FROM wakes WHERE source='approval:ap-1'"
                ),
                "pending",
                "快照拍在任何 v4 写之前"
            );
            assert_eq!(one::<String>(&snap, "PRAGMA quick_check"), "ok");
            assert!(d.path().join(crate::ID_FILE).exists());
            // 再开一次：已是 v4，不再拍。
            drop(s);
            drop(crate::Store::open_data_dir(d.path()).unwrap());
            assert_eq!(
                snapshots(&d.path().join(crate::backup::BACKUPS_DIR)).len(),
                1
            );
        }
    }

    fn rows(c: &Connection, sql: &str) -> Vec<String> {
        let mut st = c.prepare(sql).unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn v4_gives_every_unnamed_session_a_title() {
        let c = v3_db();
        c.execute_batch(
            "UPDATE sessions SET display_name=NULL, title=NULL;
             UPDATE sessions SET brief='修首页滚动' WHERE id='worker-a2';
             UPDATE sessions SET brief=char(10)||'  一二三四五六七八九十甲乙丙丁戊己庚辛壬癸  '||char(10)||'第二行' WHERE id='worker-b1';",
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sessions WHERE COALESCE(display_name,'')='' AND COALESCE(title,'')=''"
            ),
            0,
            "标签绝不回落到原始 id"
        );
        assert_eq!(
            one::<String>(&c, "SELECT title FROM sessions WHERE id='worker-a2'"),
            "修首页滚动"
        );
        assert_eq!(
            one::<String>(&c, "SELECT title FROM sessions WHERE id='worker-a1'"),
            "示例机组 #5"
        );
        assert_eq!(
            one::<String>(&c, "SELECT title FROM sessions WHERE id='worker-b1'"),
            "一二三四五六七八九十甲乙丙丁戊己庚辛",
            "跳过空行、去首尾空白、截 18 字"
        );
    }

    #[test]
    fn v4_strips_broadcast_mentions_and_keeps_the_rest_in_order() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM messages m, json_each(m.mentions) j WHERE json_extract(j.value,'$.kind')='broadcast'"
            ),
            0
        );
        assert_eq!(
            one::<String>(&c, "SELECT mentions FROM messages WHERE id='m4'"),
            r#"[{"kind":"human"}]"#
        );
        // 读得回来：剥掉之后，旧消息照常反序列化（broadcast 已不在词表里）。
        let m4 = crate::messages::get(&c, "m4").unwrap().unwrap();
        assert_eq!(m4.mentions.len(), 1);
        assert_eq!(m4.mentions[0].kind, crewd_proto::MentionKind::Human);
    }

    /// 空机组表的库也可能有消息：剥 broadcast 在 `crews == 0` 提前返回之前。
    #[test]
    fn v4_strips_broadcast_even_without_crews() {
        let c = v3_db();
        c.execute_batch(
            "INSERT INTO messages(id,channel_id,sender_kind,text,mentions,created_at)
               VALUES('lonely','crew-a/lobby','human','示例消息','[{\"kind\":\"broadcast\"},{\"kind\":\"human\"}]','2026-09-05T00:00:00.000Z');",
        )
        .unwrap();
        c.execute_batch(V4_DDL).unwrap();
        c.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM crews;")
            .unwrap();
        backfill_v4(&c).unwrap();
        assert_eq!(
            one::<String>(&c, "SELECT mentions FROM messages WHERE id='lonely'"),
            r#"[{"kind":"human"}]"#
        );
    }

    #[test]
    fn v3_roots_are_reparented_under_the_one_org_root_in_their_sidebar_order() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 4);
        assert_eq!(
            rows(&c, "SELECT id FROM crews WHERE management=1"),
            ["chief"]
        );
        assert_eq!(
            rows(
                &c,
                "SELECT id FROM crews c WHERE NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)"
            ),
            ["chief"],
            "只剩一个没有父边的：总机组"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT crew_id FROM crew_parents WHERE parent_id='chief' ORDER BY position"
            ),
            ["crew-x", "crew-a"],
            "人排过的顶层次序原样搬到总机组下"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT parent_id FROM crew_parents WHERE crew_id='crew-b'"
            ),
            ["crew-a"],
            "子机组不动"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT number FROM crews WHERE id='chief'"),
            0
        );
    }

    #[test]
    fn a_first_level_crews_human_channel_survives_the_migration() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<String>>(
                &c,
                "SELECT archived_at FROM channels WHERE id='crew-a/human'"
            ),
            None,
            "一级机组保留自己的人类群（H2）"
        );
        // 预检 D3：基线上也绿，所以再钉住「挂到总机组下之后，群仍归属它、id 不变」。
        assert_eq!(
            one::<String>(&c, "SELECT crew_id FROM channels WHERE id='crew-a/human'"),
            "crew-a"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT parent_id FROM crew_parents WHERE crew_id='crew-a'"
            ),
            ["chief"]
        );
    }

    #[test]
    fn a_v3_database_without_a_builtin_crew_gets_one_org_root() {
        let c = v3_db();
        // 预检 D1：外键开着删，级联带走 chief 的群、游标与 crew-b → chief 的边（关着删会留下悬空行，
        // 迁移的外键检查必然中止）。
        c.execute_batch("DELETE FROM crews WHERE id='chief';")
            .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            rows(&c, "SELECT id FROM crews WHERE management=1"),
            [crate::crews::MANAGEMENT_ID]
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM channels WHERE id='pendingcrew-chief/lobby'"
            ),
            1,
            "新建的总机组也有大厅"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM crew_parents WHERE parent_id='pendingcrew-chief'"
            ),
            2
        );
    }

    #[test]
    fn extra_builtin_crews_are_demoted_and_the_earliest_stays_the_root() {
        let c = v3_db();
        c.execute("UPDATE crews SET builtin=1 WHERE id='crew-x'", [])
            .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            rows(&c, "SELECT id FROM crews WHERE management=1"),
            ["chief"]
        );
        assert_eq!(
            rows(
                &c,
                "SELECT parent_id FROM crew_parents WHERE crew_id='crew-x'"
            ),
            ["chief"]
        );
    }

    #[test]
    fn at_most_one_org_root_is_enforced_by_the_schema() {
        let c = v3_db();
        migrate(&c).unwrap();
        let err = c
            .execute("UPDATE crews SET management=1 WHERE id='crew-x'", [])
            .unwrap_err();
        // 预检 D3：基线上这句因「没有 management 列」也报错；要的是唯一索引挡住的那一种。
        assert!(crate::is_unique_violation(&err), "{err}");
    }

    #[test]
    fn v4_drops_the_approvals_table_and_voids_queued_approval_wakes() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='approvals'"
            ),
            0
        );
        assert_eq!(
            one::<String>(&c, "SELECT state FROM wakes WHERE source='approval:ap-1'"),
            "dropped"
        );
    }

    #[test]
    fn fresh_database_is_v4_and_has_no_org_root_until_a_crew_exists() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 4);
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM crews"),
            0,
            "导入要求空库：迁移不能凭空建总机组"
        );
        // 被取代的 v3 版本测试里的两句（没有可填的大厅）并到这里。
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM channels"), 0);
        assert_eq!(channels::lobby_id("x"), "x/lobby");
    }

    /// 迁移在一个事务里：v4 中途失败（这里是外键检查不过）→ 整体回滚，库保持 v3，列名、审批表、唤醒都没被动过。
    #[test]
    fn a_failed_v4_migration_rolls_back_and_keeps_v3() {
        let c = v3_db();
        // 合成的孤儿行：关外键才写得进去。
        c.execute_batch(
            "PRAGMA foreign_keys=OFF;
             INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('orphan-a','ghost-crew','worker','claude','2026-09-03T00:00:00.000Z');
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(
            err.contains("库保持 v3") && err.contains("sessions → crews"),
            "{err}"
        );
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM pragma_table_info('crews') WHERE name='builtin'"
            ),
            1,
            "列名没改"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='approvals'"
            ),
            1
        );
        assert_eq!(
            one::<String>(&c, "SELECT state FROM wakes WHERE source='approval:ap-1'"),
            "pending"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM crew_parents WHERE parent_id='chief'"
            ),
            1,
            "没有挂到总机组下的边（只有夹具里 crew-b 那条）"
        );
    }

    /// 任何一步失败（不只外键检查）都带上同一句上下文：人看得出是 v4 迁移、库没被动过。
    #[test]
    fn every_v4_failure_says_it_rolled_back_and_kept_v3() {
        let c = v3_db();
        // 合成的冲突：v4 要加的列已经在了 → DDL 那一步就失败。
        c.execute_batch("ALTER TABLE crews ADD COLUMN archived_at TEXT;")
            .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(
            err.starts_with("v4 迁移失败，已回滚（库保持 v3）") && err.contains("archived_at"),
            "{err}"
        );
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='approvals'"
            ),
            1
        );
    }

    /// 审批唤醒：排队中的（pending / inflight）作废并写明原因与结算时间；已结算的、非审批的一概不动。
    #[test]
    fn v4_voids_pending_and_inflight_approval_wakes_only() {
        let c = v3_db();
        c.execute_batch(
            r#"
INSERT INTO wakes(session_id,kind,source,priority,not_before,state,attempts,created_at) VALUES
  ('worker-a1','message','approval:ap-2','human',0,'inflight',1,'2026-09-04T00:00:00.000Z'),
  ('worker-a2','ask_answer','approval:ap-3','human',0,'delivered',1,'2026-09-04T00:00:00.000Z'),
  ('worker-a1','message','msg:m1','auto',0,'pending',0,'2026-09-04T00:00:00.000Z'),
  ('worker-a2','message','msg:m2','auto',0,'inflight',1,'2026-09-04T00:00:00.000Z');
"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        let row = |source: &str| -> String {
            one(
                &c,
                &format!(
                    "SELECT state||'|'||COALESCE(last_error,'')||'|'||(settled_at IS NOT NULL) FROM wakes WHERE source='{source}'"
                ),
            )
        };
        assert_eq!(row("approval:ap-1"), "dropped|approvals removed (v4)|1");
        assert_eq!(row("approval:ap-2"), "dropped|approvals removed (v4)|1");
        assert_eq!(row("approval:ap-3"), "delivered||0", "已结算的不改");
        assert_eq!(row("msg:m1"), "pending||0", "非审批的不动");
        assert_eq!(row("msg:m2"), "inflight||0");
    }

    /// 根下已经有子（旧数据里只挂着内建机组的）：挂过来的旧根接在它们后面，原有的次序不动。
    #[test]
    fn reparented_roots_go_after_the_org_roots_existing_children() {
        let c = v3_db();
        c.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,position) VALUES
  ('crew-c','原本挂在总机组下',4,0,'2026-09-04T00:00:00.000Z','2026-09-04T00:00:00.000Z',9);
INSERT INTO crew_parents(crew_id,parent_id,position) VALUES('crew-c','chief',5);
"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            rows(
                &c,
                "SELECT crew_id||'#'||position FROM crew_parents WHERE parent_id='chief' ORDER BY position"
            ),
            ["crew-c#5", "crew-x#6", "crew-a#7"]
        );
    }

    /// 号码 0 已被别的机组占着：总机组不抢（留空），别人的 0 号不动；新建总机组的那条路也一样。
    #[test]
    fn the_org_root_does_not_take_number_zero_from_another_crew() {
        let c = v3_db();
        c.execute("UPDATE crews SET number=0 WHERE id='crew-x'", [])
            .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<i64>>(&c, "SELECT number FROM crews WHERE id='chief'"),
            None
        );
        assert_eq!(
            one::<i64>(&c, "SELECT number FROM crews WHERE id='crew-x'"),
            0
        );

        let c = v3_db();
        c.execute_batch(
            "DELETE FROM crews WHERE id='chief'; UPDATE crews SET number=0 WHERE id='crew-x';",
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<i64>>(&c, "SELECT number FROM crews WHERE id='pendingcrew-chief'"),
            None
        );
        assert_eq!(
            one::<i64>(&c, "SELECT number FROM crews WHERE id='crew-x'"),
            0
        );
    }

    /// 总机组不能被隐藏（N1 裁定 6）：旧数据里藏着的，迁移后露出来。
    #[test]
    fn a_hidden_org_root_is_unhidden_by_the_migration() {
        let c = v3_db();
        c.execute(
            "UPDATE crews SET hidden_at='2026-09-05T00:00:00.000Z' WHERE id IN ('chief','crew-x')",
            [],
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<String>>(&c, "SELECT hidden_at FROM crews WHERE id='chief'"),
            None
        );
        assert_eq!(
            one::<Option<String>>(&c, "SELECT hidden_at FROM crews WHERE id='crew-x'").as_deref(),
            Some("2026-09-05T00:00:00.000Z"),
            "别的机组照旧"
        );
    }

    #[test]
    fn backfill_v4_is_idempotent() {
        let c = v3_db();
        migrate(&c).unwrap();
        let snap = |c: &Connection| -> (String, String, i64, String, String) {
            (
                one(
                    c,
                    "SELECT group_concat(crew_id||'>'||parent_id||'#'||position, ',') FROM (SELECT * FROM crew_parents ORDER BY crew_id, parent_id)",
                ),
                one(
                    c,
                    "SELECT group_concat(id, ',') FROM crews WHERE management=1",
                ),
                one(c, "SELECT COUNT(*) FROM channels"),
                one(
                    c,
                    "SELECT group_concat(id||':'||COALESCE(number,'')||':'||COALESCE(position,''), ',') FROM (SELECT * FROM crews ORDER BY id)",
                ),
                one(
                    c,
                    "SELECT group_concat(id||':'||state||':'||COALESCE(settled_at,''), ',') FROM (SELECT * FROM wakes ORDER BY id)",
                ),
            )
        };
        let a = snap(&c);
        backfill_v4(&c).unwrap();
        migrate(&c).unwrap();
        assert_eq!(snap(&c), a);
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
            serde_json::json!([{"kind":"human"},{"kind":"captain","targetId":"captain-new"}]),
            "落成机长指针，顺序不变"
        );
        assert_eq!(
            v(mentions("c2")),
            serde_json::json!([{"kind":"captain","targetId":"captain-old"}])
        );
        assert_eq!(
            v(mentions("c5")),
            serde_json::json!([{"kind":"captain","targetId":"captain-new"},{"kind":"captain","targetId":"captain-old"},{"kind":"human"}]),
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
