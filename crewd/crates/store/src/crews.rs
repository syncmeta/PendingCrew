use crate::Result;
use crewd_proto::Crew;
use rusqlite::{Connection, params};
use serde_json::Value;

pub fn upsert(conn: &Connection, c: &Crew, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO crews(id,title,number,builtin,working_directory,captain_agent_kind,hidden_at,created_at,updated_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, number=excluded.number, builtin=excluded.builtin,
           working_directory=excluded.working_directory, captain_agent_kind=excluded.captain_agent_kind,
           hidden_at=excluded.hidden_at, updated_at=excluded.updated_at, extra=excluded.extra",
        params![
            c.id,
            c.title,
            c.number,
            c.builtin,
            c.working_directory,
            c.captain_agent_kind,
            c.hidden_at,
            c.created_at,
            c.updated_at,
            extra.map(|v| v.to_string()),
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

/// 按编号升序（无编号的排最后），同号按创建时间。
pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    let mut stmt = conn.prepare(
        "SELECT id,title,number,builtin,working_directory,captain_agent_kind,hidden_at,created_at,updated_at
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
                captain_agent_kind: r.get(5)?,
                hidden_at: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
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
