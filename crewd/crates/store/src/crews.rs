use crate::Result;
use crewd_proto::Crew;
use rusqlite::{Connection, params};
use serde_json::Value;

pub fn upsert(conn: &Connection, c: &Crew, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO crews(id,title,number,builtin,working_directory,captain_executor,hidden_at,created_at,updated_at,extra,
                           title_source,status_line,policy,captain_session_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, number=excluded.number, builtin=excluded.builtin,
           working_directory=excluded.working_directory, captain_executor=excluded.captain_executor,
           hidden_at=excluded.hidden_at, updated_at=excluded.updated_at, extra=excluded.extra,
           title_source=excluded.title_source, status_line=excluded.status_line, policy=excluded.policy,
           captain_session_id=excluded.captain_session_id",
        params![
            c.id,
            c.title,
            c.number,
            c.builtin,
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
        "SELECT id,title,number,builtin,working_directory,captain_executor,hidden_at,created_at,updated_at,
                title_source,status_line,policy,captain_session_id,COALESCE(position,0)
         FROM crews ORDER BY number IS NULL, number, created_at, id",
    )?;
    let mut crews = stmt
        .query_map([], |r| {
            Ok(Crew {
                id: r.get(0)?,
                title: r.get(1)?,
                number: r.get(2)?,
                builtin: r.get(3)?,
                parent_ids: vec![],
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
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut pstmt =
        conn.prepare("SELECT parent_id FROM crew_parents WHERE crew_id=?1 ORDER BY parent_id")?;
    for c in &mut crews {
        c.parent_ids = pstmt
            .query_map([&c.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
    }
    Ok(crews)
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
