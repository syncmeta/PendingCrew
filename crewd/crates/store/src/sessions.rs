use crate::{Result, StoreError};
use crewd_proto::Session;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

pub fn upsert(conn: &Connection, s: &Session, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions(id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET crew_id=excluded.crew_id, role=excluded.role, executor=excluded.executor,
           agent_session_id=excluded.agent_session_id, display_name=excluded.display_name,
           working_directory=excluded.working_directory, model=excluded.model, effort=excluded.effort,
           updated_at=excluded.updated_at, extra=excluded.extra",
        params![
            s.id,
            s.crew_id,
            s.role,
            s.executor,
            s.agent_session_id,
            s.display_name,
            s.working_directory,
            s.model,
            s.effort,
            s.updated_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok(())
}

pub fn list_for_crew(conn: &Connection, crew_id: &str) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at
         FROM sessions WHERE crew_id=?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([crew_id], |r| {
            Ok(Session {
                id: r.get(0)?,
                crew_id: r.get(1)?,
                role: r.get(2)?,
                executor: r.get(3)?,
                agent_session_id: r.get(4)?,
                display_name: r.get(5)?,
                working_directory: r.get(6)?,
                model: r.get(7)?,
                effort: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 这个 session 记下的 agent 会话号（重启时 `--resume` 用）。session 不存在是错误。
pub fn agent_session_id(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT agent_session_id FROM sessions WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("没有 session {session_id}")))
}

/// 落账：起 agent 进程之前写下它将用的会话号。session 不存在是错误。
pub fn set_agent_session_id(
    conn: &Connection,
    session_id: &str,
    agent_session_id: &str,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE sessions SET agent_session_id=?2, updated_at=?3 WHERE id=?1",
        params![session_id, agent_session_id, crate::now_ts()],
    )?;
    if n == 0 {
        return Err(StoreError::NotFound(format!("没有 session {session_id}")));
    }
    Ok(())
}
