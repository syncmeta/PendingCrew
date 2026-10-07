use crate::Result;
use crewd_proto::Crew;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

/// 总机组（管理机组：独立于机组树、对全部机组有权限的管理层，PM 2026-10-07）的固定 id：与 Swift 的内建机组同 id，
/// 导入的旧数据天然对上（N1 裁定 2）。
pub const MANAGEMENT_ID: &str = "pendingcrew-chief";

/// 新建总机组时的默认标题（数据默认值，人可以改名；不是给 LLM 的提示词）。
fn default_management_title(conn: &Connection) -> Result<&'static str> {
    Ok(match crate::meta_get(conn, "locale")?.as_deref() {
        Some("en") => "Headquarters",
        _ => "总机组",
    })
}

pub fn management(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT id FROM crews WHERE management=1", [], |r| r.get(0))
        .optional()?)
}

/// 总机组：有就返回；没有就建（号码 0——没被别的机组占着时——、大厅），不加任何父边或子边（它不在机组树里）。
/// `position` 记 0：顶层次序不含总机组（`crew.list` 把它单独排在最前）。
/// 只有 store 的回填与 collab 的 `crews::create`（空库里第一个机组顺带懒建，N1 裁定 3）调它。
pub fn ensure_management(conn: &Connection, now_ts: &str) -> Result<String> {
    if let Some(id) = management(conn)? {
        return Ok(id);
    }
    let zero_taken: i64 = conn.query_row("SELECT COUNT(*) FROM crews WHERE number=0", [], |r| {
        r.get(0)
    })?;
    conn.execute(
        "INSERT INTO crews(id,title,number,management,created_at,updated_at,position) VALUES(?1,?2,?3,1,?4,?4,0)",
        params![
            MANAGEMENT_ID,
            default_management_title(conn)?,
            (zero_taken == 0).then_some(0i64),
            now_ts
        ],
    )?;
    crate::channels::ensure_lobby(conn, MANAGEMENT_ID, now_ts)?;
    Ok(MANAGEMENT_ID.to_string())
}

pub fn upsert(conn: &Connection, c: &Crew, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO crews(id,title,number,management,working_directory,captain_executor,hidden_at,created_at,updated_at,extra,
                           title_source,status_line,policy,captain_session_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, number=excluded.number, management=excluded.management,
           working_directory=excluded.working_directory, captain_executor=excluded.captain_executor,
           hidden_at=excluded.hidden_at, updated_at=excluded.updated_at, extra=excluded.extra,
           title_source=excluded.title_source, status_line=excluded.status_line, policy=excluded.policy,
           captain_session_id=excluded.captain_session_id",
        params![
            c.id,
            c.title,
            c.number,
            c.management,
            c.working_directory,
            c.captain_executor,
            c.hidden_at,
            c.created_at,
            c.updated_at,
            extra.map(|v| v.to_string()),
            c.title_source,
            c.status_line,
            c.policy.as_ref().map(serde_json::to_string).transpose()?,
            c.captain_session_id,
        ],
    )?;
    Ok(())
}

pub fn set_parents(conn: &Connection, crew_id: &str, parents: &[String]) -> Result<()> {
    conn.execute("DELETE FROM crew_parents WHERE crew_id=?1", [crew_id])?;
    for p in parents {
        conn.execute(
            "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
            params![crew_id, p],
        )?;
    }
    Ok(())
}

pub fn exists(conn: &Connection, id: &str) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM crews WHERE id=?1", [id], |r| r.get(0))?;
    Ok(n > 0)
}

/// 按编号升序（无编号的排最后），同号按创建时间。侧栏顺序（`position`）由 collab 在内存里排，这里不管。
pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    let mut stmt = conn.prepare(
        "SELECT id,title,number,management,working_directory,captain_executor,hidden_at,created_at,updated_at,
                title_source,status_line,policy,captain_session_id,COALESCE(position,0),archived_at
         FROM crews ORDER BY number IS NULL, number, created_at, id",
    )?;
    let mut crews = stmt
        .query_map([], |r| {
            Ok(Crew {
                id: r.get(0)?,
                title: r.get(1)?,
                number: r.get(2)?,
                management: r.get(3)?,
                parent_ids: vec![],
                child_ids: vec![],
                working_directory: r.get(4)?,
                captain_executor: r.get(5)?,
                hidden_at: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
                title_source: r.get(9)?,
                status_line: r.get(10)?,
                policy: policy_col(r, 11)?,
                captain_session_id: r.get(12)?,
                position: r.get(13)?,
                removed_at: r.get(14)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut pstmt =
        conn.prepare("SELECT parent_id FROM crew_parents WHERE crew_id=?1 ORDER BY parent_id")?;
    for c in &mut crews {
        c.parent_ids = pstmt
            .query_map([&c.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        c.child_ids = children_of(conn, &c.id)?;
    }
    Ok(crews)
}

/// `parent` 的直系子机组，按侧栏顺序：边上的 `position`，再按建组时间、id（控制者裁定 C1）。
/// `Crew.child_ids` 与 collab 的 `crews::children` 共用这一处。
pub fn children_of(conn: &Connection, parent: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT p.crew_id FROM crew_parents p JOIN crews c ON c.id=p.crew_id
          WHERE p.parent_id=?1 ORDER BY p.position, c.created_at, c.id",
    )?;
    Ok(st
        .query_map([parent], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Crew>> {
    Ok(list(conn)?.into_iter().find(|c| c.id == id))
}

/// `policy` 列是可空的 JSON 文本；坏 JSON 转成列转换错误。
fn policy_col(r: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<Value>> {
    let s: Option<String> = r.get(idx)?;
    s.map(|s| {
        serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
    })
    .transpose()
}
