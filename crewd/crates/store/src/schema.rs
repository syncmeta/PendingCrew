//! 表结构与迁移。版本号存在 SQLite 的 `PRAGMA user_version`。
use crate::{Result, StoreError};
use rusqlite::{Connection, params};

/// Knowledge files have an independent SQLite schema; personal draft restore writer barrier is Core21.
pub const SCHEMA_VERSION: i64 = 21;
pub const SUPPORTED_SCHEMA_VERSION: i64 = SCHEMA_VERSION;

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

/// v4（N1，spec §24）：删审批；内建机组标记改名为「管理机组」并保证至多一个；归档列。数据部分在 `backfill_v4`。
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
/// 机组树是森林：旧的顶层机组原样留在顶层（不挂到谁下面）；总机组（管理机组，PM 2026-10-07）不在树里——
/// 它的父边、子边全部去掉，原来只挂在它下面的子机组成为顶层、排在顶层末尾（保持它们在总机组下的相对次序）。
pub fn backfill_v4(conn: &Connection) -> Result<()> {
    // 1) 多个管理机组：留 created_at 最早的（再按 id），其余降为普通机组。
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
    let m = crate::crews::ensure_management(conn, &crate::now_ts())?;
    // 4) 只挂在总机组下的子机组成为顶层：排在现有顶层（没有任何父边的非管理机组）之后，
    //    次序 = 它们在总机组下的次序（边上的 position, created_at, id）。
    conn.execute(
        "WITH freed AS (
           SELECT c.id, ROW_NUMBER() OVER (ORDER BY p.position, c.created_at, c.id) AS k
             FROM crew_parents p JOIN crews c ON c.id=p.crew_id
            WHERE p.parent_id=?1 AND c.id<>?1
              AND NOT EXISTS (SELECT 1 FROM crew_parents q WHERE q.crew_id=c.id AND q.parent_id<>?1))
         UPDATE crews SET position =
           (SELECT COALESCE(MAX(COALESCE(t.position,0)),0) FROM crews t
             WHERE t.id<>?1 AND NOT EXISTS (SELECT 1 FROM crew_parents q WHERE q.crew_id=t.id))
           + (SELECT k FROM freed WHERE freed.id=crews.id)
          WHERE id IN (SELECT id FROM freed)",
        [&m],
    )?;
    // 5) 总机组不在树里：去掉它的父边与子边（多父的子机组只少掉指向总机组的那一条）。
    conn.execute(
        "DELETE FROM crew_parents WHERE crew_id=?1 OR parent_id=?1",
        [&m],
    )?;
    // 6) 号码 0（没有号、且 0 没人用时）；不隐藏（总机组不能被隐藏）；position 0（与新建的总机组一致，
    //    旧库里它可能带着全局排名）。
    conn.execute(
        "UPDATE crews SET number=0 WHERE id=?1 AND number IS NULL
            AND NOT EXISTS (SELECT 1 FROM crews WHERE number=0)",
        [&m],
    )?;
    conn.execute(
        "UPDATE crews SET hidden_at=NULL, position=0 WHERE id=?1",
        [&m],
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

// Global stable identity only: source data remains in its existing authoritative store.
fn migrate_v5(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE object_refs(
        number INTEGER PRIMARY KEY AUTOINCREMENT CHECK(number > 0),
        kind TEXT NOT NULL CHECK(kind IN ('requirement','plan','human_task','decision','lesson','risk','custom_row','table')),
        crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL CHECK(length(trim(source_id)) > 0),
        created_at TEXT NOT NULL,
        UNIQUE(kind,crew_id,source_id)
    ); CREATE INDEX object_refs_by_source_crew ON object_refs(crew_id,source_id); PRAGMA user_version = 5;")?;
    tx.commit()?;
    Ok(())
}

// Retains real new events; historical events are not reconstructed from snapshots.
fn migrate_v6(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE work_events(
        crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
        seq INTEGER NOT NULL CHECK(seq > 0),
        body TEXT NOT NULL CHECK(json_valid(body)),
        PRIMARY KEY(crew_id,seq)
    );",
    )?;
    crate::work_events::backfill_references(&tx)?;
    tx.execute_batch("PRAGMA user_version = 6;")?;
    tx.commit()?;
    Ok(())
}

fn migrate_v7(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE work_stop_checks(session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,epoch INTEGER NOT NULL CHECK(epoch>=0),state TEXT NOT NULL CHECK(json_valid(state)));")?;
    crate::work_stop_checks::backfill(&tx)?;
    tx.execute_batch("PRAGMA user_version=7;")?;
    tx.commit()?;
    Ok(())
}

fn migrate_v8(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE runner_usage(id INTEGER PRIMARY KEY AUTOINCREMENT,session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE CHECK(length(CAST(session_id AS BLOB)) BETWEEN 1 AND 4096),provider TEXT NOT NULL CHECK(length(provider)>0 AND length(CAST(provider AS BLOB))<=128),thread_id TEXT NOT NULL CHECK(length(thread_id)>0 AND length(CAST(thread_id AS BLOB))<=4096),turn_id_json TEXT NOT NULL CHECK(json_valid(turn_id_json) AND json_type(turn_id_json) IN ('text','null') AND length(CAST(turn_id_json AS BLOB))<=24578),token_usage_json TEXT NOT NULL CHECK(json_valid(token_usage_json) AND json_type(token_usage_json)='object' AND length(CAST(token_usage_json AS BLOB))<=65536),observed_at TEXT NOT NULL,UNIQUE(session_id,provider,thread_id,turn_id_json,token_usage_json)); CREATE INDEX runner_usage_by_session_id ON runner_usage(session_id,id); PRAGMA user_version=8;")?;
    tx.commit()?;
    Ok(())
}

// Historical fixture construction is intentionally independent of the current
// latest version. Only an empty caller-owned connection may use this path.
#[cfg(test)]
pub(crate) fn initialize_v8_fixture(conn: &Connection) -> Result<()> {
    initialize_legacy_fixture(conn, 8)
}

pub(crate) fn initialize_legacy_fixture(conn: &Connection, target: i64) -> Result<()> {
    if !(4..=9).contains(&target) {
        return Err(StoreError::Invalid(
            "historical fixture supports versions 4 through 9".into(),
        ));
    }
    let objects: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if objects != 0 || version != 0 {
        return Err(StoreError::Invalid(
            "historical fixture requires an empty database".into(),
        ));
    }
    conn.execute_batch(V1)?;
    conn.execute_batch(V2)?;
    migrate_v3(conn)?;
    migrate_v4(conn)?;
    if target >= 5 {
        migrate_v5(conn)?;
    }
    if target >= 6 {
        migrate_v6(conn)?;
    }
    if target >= 7 {
        migrate_v7(conn)?;
    }
    if target >= 8 {
        migrate_v8(conn)?;
    }
    if target >= 9 {
        migrate_v9(conn)?;
    }
    Ok(())
}

// The transaction owns both organization conversion and its version marker;
// a failed conversion stays v8.
fn migrate_v9(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    crate::work_organization::migrate_in_tx(&tx, &crate::ts_from_ms(crate::now_ms()))?;
    tx.execute_batch("PRAGMA user_version=9;")?;
    tx.commit()?;
    Ok(())
}

// Additive document/classification and channel-reference storage shares the protected chain.
pub(crate) fn migrate_v10(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(V10)?;
    tx.commit()?;
    Ok(())
}
const V10: &str = r#"-- Candidate schema10. Apply only via normal protected schema migration transaction.
CREATE TABLE documents(
 id TEXT PRIMARY KEY, crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 title TEXT NOT NULL CHECK(length(CAST(title AS BLOB)) BETWEEN 1 AND 512), body TEXT NOT NULL,
 body_length INTEGER NOT NULL CHECK(body_length>=0 AND body_length<=1048576 AND body_length=length(CAST(body AS BLOB))),
 revision INTEGER NOT NULL CHECK(revision>0),
 created_by TEXT NOT NULL CHECK(json_valid(created_by)), updated_by TEXT NOT NULL CHECK(json_valid(updated_by)),
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
);
CREATE INDEX documents_crew_cursor ON documents(crew_id,id);
CREATE TABLE document_requests(
 request_id TEXT PRIMARY KEY, document_id TEXT NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
 payload_digest BLOB NOT NULL CHECK(length(payload_digest)=32)
);
CREATE TABLE document_events(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(revision>0), actor TEXT NOT NULL CHECK(json_valid(actor)),
 kind TEXT NOT NULL CHECK(kind IN ('created','updated','deleted')),
 title TEXT NOT NULL, body TEXT, at TEXT NOT NULL,
 UNIQUE(document_id,revision)
);
CREATE TABLE view_folder_scopes(
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 view TEXT NOT NULL CHECK(view IN ('messages','todo','documents','progress')),
 revision INTEGER NOT NULL CHECK(revision>=0), PRIMARY KEY(crew_id,view)
);
CREATE TABLE view_folders(
 id TEXT PRIMARY KEY, crew_id TEXT NOT NULL, view TEXT NOT NULL,
 name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256), parent_id TEXT REFERENCES view_folders(id), position INTEGER NOT NULL CHECK(position>=0),
 FOREIGN KEY(crew_id,view) REFERENCES view_folder_scopes(crew_id,view) ON DELETE CASCADE
);
CREATE INDEX view_folders_scope ON view_folders(crew_id,view,parent_id,position,id);
CREATE TABLE view_folder_requests(
 request_id TEXT PRIMARY KEY, crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 folder_id TEXT NOT NULL, payload_digest BLOB NOT NULL CHECK(length(payload_digest)=32)
 -- folder_id intentionally not cascade-deleted: a repeated create after folder deletion
 -- confirms the original operation; it must not allocate a replacement folder.
);
CREATE TABLE view_folder_assignments(
 crew_id TEXT NOT NULL, view TEXT NOT NULL,
 object_kind TEXT NOT NULL CHECK(object_kind IN ('channel','requirement','work','humanTask','document')),
 object_id TEXT NOT NULL, folder_id TEXT NOT NULL REFERENCES view_folders(id) ON DELETE CASCADE,
 PRIMARY KEY(crew_id,view,object_kind,object_id),
 FOREIGN KEY(crew_id,view) REFERENCES view_folder_scopes(crew_id,view) ON DELETE CASCADE
);
CREATE INDEX view_folder_object_refs ON view_folder_assignments(object_kind,object_id,crew_id);
CREATE TABLE view_folder_events(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 crew_id TEXT NOT NULL, view TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
 actor TEXT NOT NULL CHECK(json_valid(actor)), body TEXT NOT NULL CHECK(json_valid(body)), at TEXT NOT NULL,
 UNIQUE(crew_id,view,revision),
 FOREIGN KEY(crew_id,view) REFERENCES view_folder_scopes(crew_id,view) ON DELETE CASCADE
);
CREATE TABLE channel_work_scopes(
 channel_id TEXT PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(revision>=0)
);
CREATE TABLE channel_work_links(
 channel_id TEXT NOT NULL REFERENCES channel_work_scopes(channel_id) ON DELETE CASCADE,
 object_kind TEXT NOT NULL CHECK(object_kind IN ('requirement','work','humanTask')),
 object_id TEXT NOT NULL,
 source_crew_id TEXT NOT NULL REFERENCES crews(id),
 PRIMARY KEY(channel_id,object_kind,object_id)
);
CREATE INDEX channel_work_source_refs ON channel_work_links(source_crew_id,channel_id);
CREATE TABLE channel_work_events(
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 channel_id TEXT NOT NULL REFERENCES channel_work_scopes(channel_id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(revision>0),actor TEXT NOT NULL CHECK(json_valid(actor)),
 body TEXT NOT NULL CHECK(json_valid(body)),at TEXT NOT NULL,UNIQUE(channel_id,revision)
);
CREATE TABLE work_creation_requests(
 request_id TEXT PRIMARY KEY, crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 work_id TEXT NOT NULL UNIQUE, creator_key TEXT NOT NULL,
 source_channel_id TEXT, payload_digest BLOB NOT NULL CHECK(length(payload_digest)=32)
);
PRAGMA user_version=10;
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_guarded(conn, None)
}

/// 迁移。`backups` = 文件库的备份目录：给了、且库是非空的旧版本时，先过 `integrity_check`、再
/// `VACUUM INTO backups/crewd-v{旧}-pre-v{新}-{时间}.db`；任何一步不成就拒绝迁移，库原样不动（S20 / S25）。
pub fn migrate_guarded(conn: &Connection, backups: Option<&std::path::Path>) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v == 16 {
        return Err(StoreError::Invalid(
            "reserved/experimental Core16 is not the resource17 format".into(),
        ));
    }
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
    if v < 5 {
        migrate_v5(conn)?;
    }
    if v < 6 {
        migrate_v6(conn)?;
    }
    if v < 7 {
        migrate_v7(conn)?;
    }
    if v < 8 {
        migrate_v8(conn)?;
    }
    if v < 9 {
        migrate_v9(conn)?;
    }
    if v < 10 {
        migrate_v10(conn)?;
    }
    if v < 11 {
        migrate_v11(conn)?;
    }
    if v < 12 {
        migrate_v12(conn)?;
    }
    if v < 13 {
        migrate_v13(conn, v != 0)?;
    }
    if v < 14 {
        migrate_v14(conn)?;
    }
    if v < 15 {
        migrate_v15(conn)?;
    }
    if v < 17 {
        migrate_resource17(conn, 18)?;
    } else if v < 18 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch("PRAGMA user_version=18;")?;
        tx.commit()?;
    }
    if v < 19 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(crate::notifications::DDL)?;
        tx.execute_batch("PRAGMA user_version=19;")?;
        tx.commit()?;
    }
    if v < 20 {
        let tx=conn.unchecked_transaction()?;
        // No released19 writer knew native input bytes. Reject unshipped
        // experimental19 staging rather than silently importing its authority.
        let experimental:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name IN('codex_native_inputs','codex_input_requests'))",[],|r|r.get(0))?;
        if experimental{return Err(StoreError::Invalid("experimental pre20 Codex inputs are not a supported production authority".into()))}
        tx.execute_batch(crate::codex_inputs::DDL)?;
        tx.execute_batch("PRAGMA user_version=20;")?;
        tx.commit()?;
    }
    if v < 21 {
        let tx=conn.unchecked_transaction()?;
        // Old20 restore cannot invalidate the independent private Draft stamp.
        // Never inherit experimental pre21 membership authority on upgrade.
        crate::personal_drafts::initialize_new_format(&tx)?;
        tx.execute_batch("PRAGMA user_version=21;")?;
        tx.commit()?;
    }
    Ok(())
}

/// Resource scopes and Human source facts. Deliberately skips experimental Native16.
#[cfg(test)]
pub(crate) fn migrate_v17(conn: &Connection) -> Result<()> {
    migrate_resource17(conn, 17)
}

// Store18 adds only a writer barrier: old writers cannot ignore durable Codex Unknown/fork state.
// For pre-17 stores the resource rebuild and barrier commit in the same transaction.
fn migrate_resource17(conn: &Connection, target: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(r#"
ALTER TABLE view_folder_assignments RENAME TO view_folder_assignments_v15;
ALTER TABLE view_folder_events RENAME TO view_folder_events_v15;
ALTER TABLE view_folders RENAME TO view_folders_v15;
ALTER TABLE view_folder_scopes RENAME TO view_folder_scopes_v15;
CREATE TABLE view_folder_scopes(crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,channel_key TEXT NOT NULL DEFAULT '',view TEXT NOT NULL CHECK(view IN ('messages','todo','documents','progress')),revision INTEGER NOT NULL CHECK(revision>=0),PRIMARY KEY(crew_id,channel_key,view));
CREATE TABLE view_folders(id TEXT PRIMARY KEY,crew_id TEXT NOT NULL,channel_key TEXT NOT NULL DEFAULT '',view TEXT NOT NULL,name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256),parent_id TEXT REFERENCES view_folders(id),position INTEGER NOT NULL CHECK(position>=0),FOREIGN KEY(crew_id,channel_key,view) REFERENCES view_folder_scopes(crew_id,channel_key,view) ON DELETE CASCADE);
CREATE TABLE view_folder_assignments(crew_id TEXT NOT NULL,channel_key TEXT NOT NULL DEFAULT '',view TEXT NOT NULL,object_kind TEXT NOT NULL CHECK(object_kind IN ('channel','message','requirement','work','humanTask','document','knowledgeView')),object_id TEXT NOT NULL,knowledge_source TEXT CHECK(knowledge_source IS NULL OR json_valid(knowledge_source)),folder_id TEXT NOT NULL REFERENCES view_folders(id) ON DELETE CASCADE,PRIMARY KEY(crew_id,channel_key,view,object_kind,object_id),FOREIGN KEY(crew_id,channel_key,view) REFERENCES view_folder_scopes(crew_id,channel_key,view) ON DELETE CASCADE);
CREATE TABLE view_folder_events(seq INTEGER PRIMARY KEY AUTOINCREMENT,crew_id TEXT NOT NULL,channel_key TEXT NOT NULL DEFAULT '',view TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),actor TEXT NOT NULL CHECK(json_valid(actor)),body TEXT NOT NULL CHECK(json_valid(body)),at TEXT NOT NULL,UNIQUE(crew_id,channel_key,view,revision),FOREIGN KEY(crew_id,channel_key,view) REFERENCES view_folder_scopes(crew_id,channel_key,view) ON DELETE CASCADE);
INSERT INTO view_folder_scopes(crew_id,view,revision) SELECT crew_id,view,revision FROM view_folder_scopes_v15;
INSERT INTO view_folders(id,crew_id,view,name,parent_id,position) SELECT id,crew_id,view,name,parent_id,position FROM view_folders_v15;
INSERT INTO view_folder_assignments(crew_id,view,object_kind,object_id,folder_id) SELECT crew_id,view,object_kind,object_id,folder_id FROM view_folder_assignments_v15;
INSERT INTO view_folder_events(seq,crew_id,view,revision,actor,body,at) SELECT seq,crew_id,view,revision,actor,body,at FROM view_folder_events_v15;
DROP TABLE view_folder_assignments_v15;
DROP TABLE view_folder_events_v15;
DROP TABLE view_folders_v15;
DROP TABLE view_folder_scopes_v15;
CREATE INDEX view_folders_scope ON view_folders(crew_id,channel_key,view,parent_id,position,id);
CREATE INDEX view_folder_object_refs ON view_folder_assignments(object_kind,object_id,crew_id);
ALTER TABLE channel_work_links RENAME TO channel_work_links_v15;
CREATE TABLE channel_work_links(channel_id TEXT NOT NULL REFERENCES channel_work_scopes(channel_id) ON DELETE CASCADE,object_kind TEXT NOT NULL CHECK(object_kind IN ('requirement','work','humanTask')),object_id TEXT NOT NULL,source_crew_id TEXT NOT NULL REFERENCES crews(id),tags TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(tags) AND json_type(tags)='array' AND json_array_length(tags)<=16),PRIMARY KEY(channel_id,object_kind,object_id));
INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) SELECT channel_id,object_kind,object_id,source_crew_id FROM channel_work_links_v15;
DROP TABLE channel_work_links_v15;
CREATE INDEX channel_work_source_refs ON channel_work_links(source_crew_id,channel_id);
CREATE TABLE todo_attributes(object_kind TEXT NOT NULL CHECK(object_kind IN ('requirement','humanTask')),object_id TEXT NOT NULL,crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,revision INTEGER NOT NULL CHECK(revision>0),origin TEXT NOT NULL CHECK(origin IN ('requirement','agentAsk','humanTask')),owner TEXT CHECK(owner IS NULL OR json_valid(owner)),manual_state TEXT CHECK(manual_state IS NULL OR manual_state IN ('open','done')),PRIMARY KEY(object_kind,object_id));
CREATE TABLE todo_attribute_events(seq INTEGER PRIMARY KEY AUTOINCREMENT,object_kind TEXT NOT NULL,object_id TEXT NOT NULL,crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,revision INTEGER NOT NULL CHECK(revision>0),body TEXT NOT NULL CHECK(json_valid(body)),at TEXT NOT NULL,UNIQUE(object_kind,object_id,revision));
CREATE TABLE resource_mutation_requests(request_id TEXT PRIMARY KEY,operation TEXT NOT NULL CHECK(operation IN ('todo.assign','todo.setState','humanTask.create','channel.workTagsSet','channel.knowledgeLink','channel.knowledgeUnlink')),crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,channel_id TEXT NOT NULL,object_kind TEXT NOT NULL CHECK(object_kind IN ('requirement','humanTask','knowledgeView')),object_id TEXT NOT NULL,payload_digest BLOB NOT NULL CHECK(length(payload_digest)=32),result TEXT NOT NULL CHECK(json_valid(result)),at TEXT NOT NULL);
CREATE TABLE channel_knowledge_links(channel_id TEXT NOT NULL REFERENCES channel_work_scopes(channel_id) ON DELETE CASCADE,source_key TEXT NOT NULL,source_crew_id TEXT NOT NULL REFERENCES crews(id),source TEXT NOT NULL CHECK(json_valid(source)),PRIMARY KEY(channel_id,source_key));
CREATE INDEX channel_knowledge_source_refs ON channel_knowledge_links(source_crew_id,channel_id);
"#)?;
    tx.pragma_update(None, "user_version", target)?;
    tx.commit()?;
    Ok(())
}

/// Jev uses existing JSON/meta rows, but its routing wakes require new consumers.
/// This format barrier rejects old Core14 before it can falsely ACK an ordinary
/// System wake. It installs no Native history table or producer capability.
pub(crate) fn migrate_v15(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("PRAGMA user_version=15;")?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('judgment.format.v1','1')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// The sole public receipt implementation is installed after schema13 in one
/// transaction. Neither this version nor the DDL advertises a native adapter.
pub(crate) fn migrate_v14(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(crate::command_receipts::V14_DDL)?;
    tx.execute_batch(crate::local_clients::V14_DDL)?;
    tx.execute_batch("PRAGMA user_version=14;")?;
    tx.commit()?;
    Ok(())
}

/// Deterministic followup bookkeeping is separate from the work authority.
pub(crate) fn migrate_v13(conn: &Connection, historical: bool) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(crate::work_followups::V13_DDL)?;
    if historical {
        crate::work_followups::restored(&tx)?;
    } else {
        crate::work_followups::finish_baseline(&tx)?;
    }
    tx.execute_batch("PRAGMA user_version=13;")?;
    tx.commit()?;
    Ok(())
}

/// Core receipt authority is installed atomically after remote schema11.
/// Version12 alone does not advertise a provider/native capability.
pub(crate) fn migrate_v12(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(crate::native_execution::V12_DDL)?;
    tx.execute_batch("PRAGMA user_version=12;")?;
    tx.commit()?;
    Ok(())
}

/// Persistent remote permission has its own public identity binding, separate
/// from dbId and private TLS key material. The listener remains disabled.
pub(crate) fn migrate_v11(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(r#"
CREATE TABLE remote_authority(
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  authority_id TEXT NOT NULL UNIQUE,
  db_id TEXT NOT NULL,
  certificate_der_base64 TEXT NOT NULL,
  spki_sha256 TEXT NOT NULL CHECK(length(spki_sha256)=64),
  created_at TEXT NOT NULL
);
CREATE TABLE remote_configuration(
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  revision INTEGER NOT NULL CHECK(revision>0),
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  listen_host TEXT NOT NULL,
  port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535)
);
INSERT INTO remote_configuration VALUES(1,1,0,'127.0.0.1',7423);
CREATE TABLE remote_invitations(
  id TEXT PRIMARY KEY,
  authority_id TEXT NOT NULL,
  challenge TEXT NOT NULL,
  envelope_json TEXT NOT NULL,
  expires_unix INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  consumed_at TEXT
);
CREATE TABLE remote_grants(
  id TEXT PRIMARY KEY,
  authority_id TEXT NOT NULL,
  device_label TEXT NOT NULL,
  client_spki_sha256 TEXT NOT NULL CHECK(length(client_spki_sha256)=64),
  client_certificate_der_base64 TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'owner_workspace' CHECK(scope='owner_workspace'),
  revision INTEGER NOT NULL CHECK(revision>0),
  created_at TEXT NOT NULL,
  revoked_at TEXT,
  revoke_note TEXT,
  CHECK((revoked_at IS NULL) = (revoke_note IS NULL))
);
CREATE UNIQUE INDEX remote_active_client_pin ON remote_grants(authority_id,client_spki_sha256) WHERE revoked_at IS NULL;
CREATE TABLE remote_requests(
  request_id TEXT PRIMARY KEY,
  authority_id TEXT NOT NULL,
  method TEXT NOT NULL,
  input_json TEXT NOT NULL,
  result_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE remote_audit(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  authority_id TEXT NOT NULL,
  grant_id TEXT,
  action TEXT NOT NULL,
  request_id TEXT,
  at TEXT NOT NULL,
  facts_json TEXT NOT NULL
);
PRAGMA user_version=11;
"#)?;
    tx.commit()?;
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
                // v4：库里有机组而没有内建机组 → 建出总机组（N1 裁定 2），其余四个仍是顶层机组（总机组不在树里）。
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

    /// 停在 v3 的库：内建 chief、两个顶层机组（crew-x 人排在 crew-a 前面）、子机组 crew-b（也挂着 chief）；
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
-- 旧数据里「既挂在 chief 下、又有别的父」的机组：v4 后指向总机组的边去掉（总机组不在树里），别的父留着。
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
    fn a_v3_data_dir_is_snapshotted_then_migrated_to_current_through_the_guarded_open() {
        for stamped in [false, true] {
            let d = tempfile::tempdir().unwrap();
            v3_data_dir(d.path(), stamped);
            let s = crate::Store::open_data_dir(d.path()).unwrap();
            assert_eq!(
                one::<i64>(s.conn(), "PRAGMA user_version"),
                SCHEMA_VERSION,
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
            assert!(
                name.starts_with(&format!("crewd-v3-pre-v{SCHEMA_VERSION}-")),
                "{name}"
            );
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
    fn v3_top_level_crews_stay_top_level_and_the_chief_becomes_the_management_crew_outside_the_tree()
     {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), SCHEMA_VERSION);
        assert_eq!(
            rows(&c, "SELECT id FROM crews WHERE management=1"),
            ["chief"]
        );
        assert_eq!(
            rows(
                &c,
                "SELECT crew_id||'>'||parent_id FROM crew_parents WHERE crew_id='chief' OR parent_id='chief'"
            ),
            Vec::<String>::new(),
            "总机组没有父边也没有子边"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT id FROM crews c WHERE management=0
                   AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)
                 ORDER BY position, created_at, id"
            ),
            ["crew-x", "crew-a"],
            "旧的顶层机组原样留在顶层，人排过的次序不变（不挂到总机组下）"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT parent_id FROM crew_parents WHERE crew_id='crew-b'"
            ),
            ["crew-a"],
            "多父的子机组只少掉指向总机组的那条边"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT number FROM crews WHERE id='chief'"),
            0
        );
        assert_eq!(
            one::<i64>(&c, "SELECT position FROM crews WHERE id='chief'"),
            0,
            "迁移来的总机组与新建的一样 position=0（旧库里它的 position 是别的值）"
        );
    }

    #[test]
    fn a_top_level_crews_human_channel_survives_the_migration() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<String>>(
                &c,
                "SELECT archived_at FROM channels WHERE id='crew-a/human'"
            ),
            None,
            "顶层机组保留自己的人类群"
        );
        assert_eq!(
            one::<String>(&c, "SELECT crew_id FROM channels WHERE id='crew-a/human'"),
            "crew-a"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM crew_parents WHERE crew_id='crew-a'"
            ),
            0,
            "仍是顶层"
        );
    }

    #[test]
    fn a_v3_database_without_a_builtin_crew_gets_one_management_crew_with_no_edges() {
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
                "SELECT COUNT(*) FROM crew_parents WHERE parent_id='pendingcrew-chief' OR crew_id='pendingcrew-chief'"
            ),
            0,
            "不挂任何机组"
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM crews c WHERE NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)"
            ),
            3,
            "crew-a、crew-x 与新建的总机组"
        );
    }

    #[test]
    fn extra_builtin_crews_are_demoted_and_the_earliest_stays_the_management_crew() {
        let c = v3_db();
        c.execute("UPDATE crews SET builtin=1 WHERE id='crew-x'", [])
            .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            rows(&c, "SELECT id FROM crews WHERE management=1"),
            ["chief"]
        );
        assert_eq!(
            one::<i64>(
                &c,
                "SELECT COUNT(*) FROM crew_parents WHERE crew_id='crew-x'"
            ),
            0,
            "降级的内建机组是普通的顶层机组"
        );
    }

    #[test]
    fn at_most_one_management_crew_is_enforced_by_the_schema() {
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
    fn fresh_database_is_current_and_has_no_management_crew_until_a_crew_exists() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), SCHEMA_VERSION);
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
            "夹具里 crew-b → chief 那条边没被摘掉"
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

    /// 旧数据里挂在内建机组下的子机组：只挂在它下面的成为顶层、排在现有顶层之后（保持在它下面的相对次序）；
    /// 还有别的父的只少掉那条边；内建机组自己挂在别人下面的边也去掉。
    #[test]
    fn the_chiefs_former_children_become_top_level_last_in_their_old_order() {
        let c = v3_db();
        c.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,position) VALUES
  ('crew-c','原本挂在总机组下',4,0,'2026-09-04T00:00:00.000Z','2026-09-04T00:00:00.000Z',1),
  ('crew-d','也挂在总机组下',5,0,'2026-09-05T00:00:00.000Z','2026-09-05T00:00:00.000Z',1);
INSERT INTO crew_parents(crew_id,parent_id,position) VALUES
  ('crew-d','chief',3),('crew-c','chief',7),('chief','crew-x',1);
"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            rows(
                &c,
                "SELECT id FROM crews c WHERE management=0
                   AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)
                 ORDER BY position, created_at, id"
            ),
            ["crew-x", "crew-a", "crew-d", "crew-c"],
            "原来的顶层在前；总机组下的两个按边上的次序接在后面"
        );
        assert_eq!(
            rows(
                &c,
                "SELECT crew_id||'>'||parent_id FROM crew_parents WHERE crew_id='chief' OR parent_id='chief'"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            rows(
                &c,
                "SELECT parent_id FROM crew_parents WHERE crew_id='crew-b'"
            ),
            ["crew-a"]
        );
    }

    /// 号码 0 已被别的机组占着：总机组不抢（留空），别人的 0 号不动；新建总机组的那条路也一样。
    #[test]
    fn the_management_crew_does_not_take_number_zero_from_another_crew() {
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
    fn a_hidden_management_crew_is_unhidden_by_the_migration() {
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

#[cfg(test)]
mod organization_version_tests {
    use super::*;
    use crewd_work::{Actor, Config, CrewId, EntryNo, NewItem, Ts, WorkBook};
    use rusqlite::params;

    fn legacy_v8() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize_v8_fixture(&conn).unwrap();
        assert_eq!(version(&conn), 8);
        conn.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('a','A','t','t'),('b','B','t','t');").unwrap();
        for crew in ["a", "b"] {
            let mut book = WorkBook::new(CrewId::new(crew), Config::default());
            book.register_entry(Ts(1000), EntryNo(1)).unwrap();
            book.create_item(
                &Actor::Human,
                Ts(1000),
                NewItem::titled(crew).from_entry(EntryNo(1)),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,'t')",
                params![crew, serde_json::to_string(book.state()).unwrap()],
            )
            .unwrap();
            for (kind, source) in [
                (crate::object_refs::ObjectKind::Plan, "work:W1"),
                (crate::object_refs::ObjectKind::Requirement, "work:#1"),
            ] {
                crate::object_refs::bind(&conn, kind, crew, source, "t").unwrap();
            }
        }
        conn
    }
    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }
    fn issued(conn: &Connection) -> Vec<(i64, String, String, String)> {
        conn.prepare("SELECT number,kind,crew_id,source_id FROM object_refs ORDER BY number")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    #[test]
    fn staged_v9_commits_version_with_authority_and_retries_without_archive_replay() {
        let conn = legacy_v8();
        let before = issued(&conn);
        migrate_v9(&conn).unwrap();
        assert_eq!(version(&conn), 9);
        assert_eq!(issued(&conn), before);
        let state = crate::work_organization::load(&conn).unwrap().unwrap();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.entries.len(), 2);
        let mut book = WorkBook::restore(state, Config::default()).unwrap();
        let mut item = NewItem::titled("created after migration");
        item.crew_id = Some(CrewId::new("a"));
        let created = book.create_item(&Actor::Human, Ts(2000), item).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        crate::work_organization::bind_core(
            &tx,
            crate::object_refs::ObjectKind::Plan,
            "a",
            &created.value.to_string(),
            "new-authority",
        )
        .unwrap();
        crate::work_organization::save(&tx, book.state(), &created.events, "new-authority")
            .unwrap();
        tx.commit().unwrap();
        let state = book.state().clone();
        assert_eq!(state.items.len(), 3);
        // Legacy archive still has just its two original items.
        // The canonical map is an object; check each original state directly.
        let mut q = conn.prepare("SELECT state FROM work_books").unwrap();
        let originals: Vec<String> = q
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            originals
                .iter()
                .map(|raw| serde_json::from_str::<crewd_work::BookState>(raw)
                    .unwrap()
                    .items
                    .len())
                .sum::<usize>(),
            2
        );
        drop(q);
        // Retry the transaction, without replaying the immutable legacy states.
        migrate_v9(&conn).unwrap();
        let updated: String = conn
            .query_row("SELECT updated_at FROM organization_work_book", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(updated, "new-authority");
        assert_eq!(
            crate::work_organization::load(&conn).unwrap().unwrap(),
            state
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1
        );
    }
    #[test]
    fn staged_v9_mid_conversion_failure_keeps_version_and_issued_ids_then_retries() {
        let conn = legacy_v8();
        let before = issued(&conn);
        conn.execute_batch("CREATE TRIGGER reject_core BEFORE UPDATE ON object_refs BEGIN SELECT RAISE(ABORT,'injected identity refusal'); END;").unwrap();
        assert!(migrate_v9(&conn).is_err());
        assert_eq!(version(&conn), 8);
        assert_eq!(issued(&conn), before);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("DROP TRIGGER reject_core;").unwrap();
        migrate_v9(&conn).unwrap();
        assert_eq!(version(&conn), 9);
        assert_eq!(issued(&conn), before);
    }
    #[test]
    fn staged_v9_pre_conversion_snapshot_restores_v8_objects_and_version() {
        let conn = legacy_v8();
        let before = issued(&conn);
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("pre-v9.db");
        crate::backup::vacuum_into(&conn, &snapshot).unwrap();
        migrate_v9(&conn).unwrap();
        let old = Connection::open(&snapshot).unwrap();
        assert_eq!(version(&old), 8);
        assert_eq!(issued(&old), before);
        assert_eq!(
            old.query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let count: i64 = old
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        migrate_v9(&old).unwrap();
        assert_eq!(version(&old), 9);
        assert_eq!(issued(&old), before);
    }
    #[test]
    fn staged_v9_preserves_usage_and_pending_inflight_delivered_wake_receipts() {
        let conn = legacy_v8();
        conn.execute_batch("INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('s','b','worker','codex','t');").unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        crate::runner_usage::append(&tx, "s", crate::runner_usage::Snapshot {
            provider: "codex", thread_id: "thread-original", turn_id: None,
            token_usage_json: r#"{"total":{"totalTokens":9007199254740993},"unknown":{"future":"retained"}}"#,
        }, 1000).unwrap();
        for (state, attempts, settled) in [
            ("pending", 0, None),
            ("inflight", 1, None),
            ("delivered", 1, Some("original-receipt")),
        ] {
            tx.execute("INSERT INTO wakes(session_id,kind,source,priority,not_before,note,state,attempts,created_at,settled_at) VALUES('s','checkpoint',?1,'system',1000,?2,?3,?4,'original-time',?5)", params![format!("work-check:{state}"), r#"{"items":["W3"],"original":"receipt identity"}"#,state,attempts,settled]).unwrap();
        }
        tx.commit().unwrap();
        fn wakes(conn: &Connection) -> Vec<(i64, String, String, String, i64, Option<String>)> {
            conn.prepare("SELECT id,source,note,state,attempts,settled_at FROM wakes ORDER BY id")
                .unwrap()
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }
        let before_wakes = wakes(&conn);
        let before_usage =
            serde_json::to_value(crate::runner_usage::list(&conn, "s", 0, 50).unwrap()).unwrap();
        migrate_v9(&conn).unwrap();
        assert_eq!(wakes(&conn), before_wakes);
        assert_eq!(
            serde_json::to_value(crate::runner_usage::list(&conn, "s", 0, 50).unwrap()).unwrap(),
            before_usage
        );
        // B's published W3 maps to organization W2; existing wake notes must
        // retain the published identity rather than be rewritten to the core.
        assert_eq!(
            crate::work_organization::core_for_reference(
                &conn,
                crate::object_refs::ObjectKind::Plan,
                3
            )
            .unwrap()
            .as_deref(),
            Some("W2")
        );
        migrate_v9(&conn).unwrap();
        assert_eq!(wakes(&conn), before_wakes);
        assert_eq!(
            serde_json::to_value(crate::runner_usage::list(&conn, "s", 0, 50).unwrap()).unwrap(),
            before_usage
        );
    }
    #[test]
    fn public_v8_upgrade_backs_up_then_reopens_current_authority_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        let original = legacy_v8();
        let issued_before = issued(&original);
        crate::backup::vacuum_into(&original, &path).unwrap();
        drop(original);
        let store = crate::Store::open(&path).unwrap();
        assert_eq!(version(store.conn()), SCHEMA_VERSION);
        assert_eq!(issued(store.conn()), issued_before);
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("crewd-v8-pre-v{SCHEMA_VERSION}-"))
        );
        let backup = Connection::open(&backups[0]).unwrap();
        assert_eq!(version(&backup), 8);
        assert_eq!(issued(&backup), issued_before);
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let mut book = WorkBook::restore(
            crate::work_organization::load(store.conn())
                .unwrap()
                .unwrap(),
            Config::default(),
        )
        .unwrap();
        let mut item = NewItem::titled("only in current organization");
        item.crew_id = Some(CrewId::new("b"));
        let created = book.create_item(&Actor::Human, Ts(2000), item).unwrap();
        let tx = store.conn().unchecked_transaction().unwrap();
        crate::work_organization::bind_core(
            &tx,
            crate::object_refs::ObjectKind::Plan,
            "b",
            &created.value.to_string(),
            "t",
        )
        .unwrap();
        crate::work_organization::save(&tx, book.state(), &created.events, "t").unwrap();
        tx.commit().unwrap();
        let current = book.state().clone();
        drop(store);
        let reopened = crate::Store::open(&path).unwrap();
        assert_eq!(version(reopened.conn()), SCHEMA_VERSION);
        assert_eq!(
            crate::work_organization::load(reopened.conn())
                .unwrap()
                .unwrap(),
            current
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("backups"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            reopened
                .conn()
                .query_row("SELECT COUNT(*) FROM organization_work_events", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
    }
    #[test]
    fn public_v8_backup_refusal_leaves_original_version_and_work_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        let original = legacy_v8();
        let before = issued(&original);
        crate::backup::vacuum_into(&original, &path).unwrap();
        drop(original);
        std::fs::write(dir.path().join("backups"), "injected non-directory").unwrap();
        assert!(matches!(
            crate::Store::open(&path),
            Err(StoreError::MigrationRefused { from: 8, .. })
        ));
        let untouched = Connection::open(&path).unwrap();
        assert_eq!(version(&untouched), 8);
        assert_eq!(issued(&untouched), before);
        assert_eq!(
            untouched
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            untouched
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='organization_work_book'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        std::fs::remove_file(dir.path().join("backups")).unwrap();
        drop(untouched);
        let retried = crate::Store::open(&path).unwrap();
        assert_eq!(version(retried.conn()), SCHEMA_VERSION);
        assert_eq!(issued(retried.conn()), before);
    }
    fn exact_v11_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch("DROP TABLE local_client_trust; DROP TABLE public_command_receipts; DROP TABLE work_followup_facts; DELETE FROM meta WHERE key='work_followup_baseline_required'; DROP TABLE native_execution_inputs; DROP TABLE native_execution_receipts; DROP TABLE native_execution_runtimes; PRAGMA user_version=11;").unwrap();
        conn
    }
    #[test]
    fn formal_v11_to_v12_is_atomic_and_preserves_remote_configuration() {
        let conn = exact_v11_fixture();
        conn.execute(
            "UPDATE remote_configuration SET revision=8,enabled=1,port=8443",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        assert_eq!(
            conn.query_row(
                "SELECT revision,enabled,port FROM remote_configuration",
                [],
                |r| Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?
                ))
            )
            .unwrap(),
            (8, 1, 8443)
        );
        let tables:i64=conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'native_execution_%'", [], |r|r.get(0)).unwrap();
        assert_eq!(tables, 3);
        migrate(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
    }
    #[test]
    fn formal_v12_conflict_rolls_back_whole_ddl_and_keeps_v11() {
        let conn = exact_v11_fixture();
        conn.execute_batch("CREATE TABLE native_execution_receipts(marker TEXT);")
            .unwrap();
        assert!(migrate(&conn).is_err());
        assert_eq!(version(&conn), 11);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='native_execution_runtimes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        conn.execute_batch("DROP TABLE native_execution_receipts;")
            .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
    }
    #[test]
    fn formal_v11_guarded_open_keeps_exact_backup_before_v12() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("crewd.db");
        {
            let store = crate::Store::open(&path).unwrap();
            store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('v11-work','retained','now','now'); DROP TABLE local_client_trust; DROP TABLE public_command_receipts; DROP TABLE work_followup_facts; DELETE FROM meta WHERE key='work_followup_baseline_required'; DROP TABLE native_execution_inputs; DROP TABLE native_execution_receipts; DROP TABLE native_execution_runtimes; PRAGMA user_version=11;").unwrap();
        }
        let store = crate::Store::open(&path).unwrap();
        assert_eq!(version(store.conn()), SCHEMA_VERSION);
        assert_eq!(
            store
                .conn()
                .query_row("SELECT title FROM crews WHERE id='v11-work'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "retained"
        );
        let snapshots: Vec<_> = std::fs::read_dir(tmp.path().join("backups"))
            .unwrap()
            .map(|x| x.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("crewd-v11-pre-v{SCHEMA_VERSION}-"))
            })
            .collect();
        assert_eq!(snapshots.len(), 1);
        let backup = Connection::open(&snapshots[0]).unwrap();
        assert_eq!(version(&backup), 11);
    }
}

#[cfg(test)]
mod judgment_format_tests {
    use super::*;
    #[test]
    fn v15_format_marker_failure_rolls_back_version_and_retry_installs_no_native_tables() {
        // A mechanical transaction stub, not evidence of a genuine old14 DB.
        // The formal migration gate uses the sealed real old Core14 binary.
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); PRAGMA user_version=14; CREATE TRIGGER deny_jev_marker BEFORE INSERT ON meta WHEN new.key='judgment.format.v1' BEGIN SELECT RAISE(ABORT,'injected marker fault'); END;").unwrap();
        assert!(migrate_v15(&c).is_err());
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            14
        );
        assert!(crate::meta_get(&c, "judgment.format.v1").unwrap().is_none());
        c.execute_batch("DROP TRIGGER deny_jev_marker").unwrap();
        migrate_v15(&c).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            crate::meta_get(&c, "judgment.format.v1")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert!(crate::judgment_state::format_available(&c).unwrap());
        crate::meta_set(&c, "judgment.format.v1", "unknown-version").unwrap();
        assert!(!crate::judgment_state::format_available(&c).unwrap());
        assert_eq!(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'native_history_%'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}

#[cfg(test)]
mod resource17_tests {
    use super::*;
    fn original15(c: &Connection) {
        c.execute_batch(V1).unwrap();
        c.execute_batch(V2).unwrap();
        migrate_v3(c).unwrap();
        migrate_v4(c).unwrap();
        migrate_v5(c).unwrap();
        migrate_v6(c).unwrap();
        migrate_v7(c).unwrap();
        migrate_v8(c).unwrap();
        migrate_v9(c).unwrap();
        migrate_v10(c).unwrap();
        migrate_v11(c).unwrap();
        migrate_v12(c).unwrap();
        migrate_v13(c, false).unwrap();
        migrate_v14(c).unwrap();
        migrate_v15(c).unwrap();
    }
    fn seed(c: &Connection) {
        c.execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('resource17-source','retained','now','now');INSERT INTO view_folder_scopes(crew_id,view,revision)VALUES('resource17-source','documents',1);INSERT INTO view_folders(id,crew_id,view,name,parent_id,position)VALUES('retained-folder','resource17-source','documents','retained',NULL,0);INSERT INTO view_folder_assignments(crew_id,view,object_kind,object_id,folder_id)VALUES('resource17-source','documents','document','retained-document','retained-folder');INSERT INTO view_folder_events(crew_id,view,revision,actor,body,at)VALUES('resource17-source','documents',1,'{}','{}','now');").unwrap();
    }
    #[test]
    fn resource17_original15_protected_upgrade_retains_original_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original15.db");
        let c = Connection::open(&path).unwrap();
        original15(&c);
        seed(&c);
        migrate_guarded(&c, Some(&dir.path().join("backups"))).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            18
        );
        assert_eq!(
            c.query_row(
                "SELECT channel_key FROM view_folder_scopes WHERE crew_id='resource17-source'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            ""
        );
        assert_eq!(c.query_row("SELECT count(*) FROM view_folder_assignments WHERE folder_id='retained-folder' AND channel_key=''",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        let backups = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(backups.len(), 1);
        let old = Connection::open(backups[0].path()).unwrap();
        assert_eq!(
            old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM meta WHERE key='teamData.format.v1'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn resource17_failure_rolls_back_all_scope_rebuilds() {
        let c = Connection::open_in_memory().unwrap();
        original15(&c);
        seed(&c);
        c.execute_batch("CREATE TABLE todo_attributes(injected_collision TEXT);")
            .unwrap();
        assert!(migrate_guarded(&c, None).is_err());
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            c.query_row(
                "SELECT name FROM view_folders WHERE id='retained-folder'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "retained"
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%_v15'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn notification19_current18_migration_keeps_original_rows_and_protected_backup() {
        fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
            c.query_row(sql, [], |r| r.get(0)).unwrap()
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current18.db");
        let c = Connection::open(&path).unwrap();
        original15(&c);
        seed(&c);
        migrate_resource17(&c, 18).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 18);
        let old = c
            .query_row(
                "SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_master ORDER BY name)",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        migrate_guarded(&c, Some(&dir.path().join("backups"))).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 19);
        assert_eq!(
            c.query_row(
                "SELECT name FROM view_folders WHERE id='retained-folder'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "retained"
        );
        assert!(!crate::notifications::settings(&c).unwrap().enabled);
        let paths = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|r| r.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let backup = Connection::open(&paths[0]).unwrap();
        assert_eq!(one::<i64>(&backup, "PRAGMA user_version"), 18);
        assert_eq!(old,backup.query_row("SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_master ORDER BY name)",[],|r|r.get::<_,String>(0)).unwrap());
        // A genuine protected transaction failure may not leave partial19 DDL or a bumped writer gate.
        let broken = Connection::open_in_memory().unwrap();
        original15(&broken);
        seed(&broken);
        migrate_resource17(&broken, 18).unwrap();
        broken
            .execute_batch("CREATE TABLE notification_outbox(retained TEXT)")
            .unwrap();
        assert!(migrate_guarded(&broken, None).is_err());
        assert_eq!(one::<i64>(&broken, "PRAGMA user_version"), 18);
        assert_eq!(
            one::<i64>(
                &broken,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='notification_settings'"
            ),
            0
        );
    }

    #[test]
    fn notification19_original17_protected_writer_barrier_preserves_original_schema_and_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original17.db");
        let c = Connection::open(&path).unwrap();
        original15(&c);
        seed(&c);
        migrate_v17(&c).unwrap();
        let schema: String = c
            .query_row(
                "SELECT group_concat(sql, ';') FROM (SELECT sql FROM sqlite_master WHERE name NOT LIKE 'notification_%' ORDER BY name)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        migrate_guarded(&c, Some(&dir.path().join("backups"))).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            19
        );
        let after: String = c
            .query_row(
                "SELECT group_concat(sql, ';') FROM (SELECT sql FROM sqlite_master WHERE name NOT LIKE 'notification_%' ORDER BY name)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(schema, after);
        assert_eq!(
            c.query_row(
                "SELECT name FROM view_folders WHERE id='retained-folder'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "retained"
        );
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        let old = Connection::open(&backups[0]).unwrap();
        assert_eq!(
            old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            17
        );
        assert_eq!(
            old.query_row(
                "SELECT name FROM view_folders WHERE id='retained-folder'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "retained"
        );
    }

    #[test]
    #[ignore = "requires the exact preserved normal Store18 writer supplied by the acceptance owner"]
    fn notification19_saved_writer18_refuses_before_modifying_store() {
        use std::{
            collections::BTreeMap,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let old = std::env::var_os("CREWD_TEST_OLD_WRITER")
            .expect("exact preserved writer path required");
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::open_data_dir(dir.path()).unwrap();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('retained19','writer barrier','now','now')").unwrap();
        assert_eq!(
            store
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            19
        );
        drop(store);
        let snapshot = || {
            std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    (e.file_name(), std::fs::read(e.path()).unwrap())
                })
                .collect::<BTreeMap<_, _>>()
        };
        let before = snapshot();
        let mut child = Command::new(old)
            .args(["--data-dir"])
            .arg(dir.path())
            .args(["serve", "--claude", "/usr/bin/false"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("saved old writer did not refuse within bound");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "saved Store18 writer status={} stderr={stderr}",
            output.status
        );
        assert!(!output.status.success());
        assert!(
            stderr.contains("19") && stderr.contains("18") && stderr.contains("拒绝"),
            "{stderr}"
        );
        let mut after = snapshot();
        assert_eq!(
            after.remove(std::ffi::OsStr::new("crewd.lock")),
            Some(Vec::new()),
            "ordinary serve may create only its empty standard lease file"
        );
        assert!(
            after == before,
            "actual fresh19 DB, identity and all SQLite sidecars must remain byte-identical"
        );
    }

    #[test]
    fn resource17_reserved16_refused_before_original_file_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reserved16.db");
        let c = Connection::open(&path).unwrap();
        original15(&c);
        seed(&c);
        c.execute_batch("PRAGMA user_version=16").unwrap();
        drop(c);
        let before = std::fs::read(&path).unwrap();
        assert!(crate::Store::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
