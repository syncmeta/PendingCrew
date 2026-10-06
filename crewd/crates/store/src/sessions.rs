use crate::{Result, StoreError};
use crewd_proto::{Desired, Role, Session};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

const COLS: &str = "id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at,\
title,brief,desired,ext,worktree,was_working,last_active_at,last_output_at,last_progress_at,awaiting_question,resume_failed_at";

fn bad_vocab(idx: usize, what: &str, s: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        format!("未知 {what} {s}").into(),
    )
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let role: String = r.get(2)?;
    let desired: String = r.get(12)?;
    Ok(Session {
        id: r.get(0)?,
        crew_id: r.get(1)?,
        role: Role::parse(&role).ok_or_else(|| bad_vocab(2, "role", &role))?,
        executor: r.get(3)?,
        agent_session_id: r.get(4)?,
        display_name: r.get(5)?,
        working_directory: r.get(6)?,
        model: r.get(7)?,
        effort: r.get(8)?,
        updated_at: r.get(9)?,
        title: r.get(10)?,
        brief: r.get(11)?,
        desired: Desired::parse(&desired).ok_or_else(|| bad_vocab(12, "desired", &desired))?,
        ext: r.get(13)?,
        worktree: r.get(14)?,
        was_working: r.get(15)?,
        last_active_at: r.get(16)?,
        last_output_at: r.get(17)?,
        last_progress_at: r.get(18)?,
        awaiting_question: r.get(19)?,
        resume_failed_at: r.get(20)?,
    })
}

pub fn upsert(conn: &Connection, s: &Session, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions(id,crew_id,role,executor,agent_session_id,display_name,working_directory,model,effort,updated_at,extra,
                              title,brief,desired,ext,worktree,was_working,last_active_at,last_output_at,last_progress_at,
                              awaiting_question,resume_failed_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)
         ON CONFLICT(id) DO UPDATE SET crew_id=excluded.crew_id, role=excluded.role, executor=excluded.executor,
           agent_session_id=excluded.agent_session_id, display_name=excluded.display_name,
           working_directory=excluded.working_directory, model=excluded.model, effort=excluded.effort,
           updated_at=excluded.updated_at, extra=excluded.extra,
           title=excluded.title, brief=excluded.brief, desired=excluded.desired, ext=excluded.ext,
           worktree=excluded.worktree, was_working=excluded.was_working, last_active_at=excluded.last_active_at,
           last_output_at=excluded.last_output_at, last_progress_at=excluded.last_progress_at,
           awaiting_question=excluded.awaiting_question, resume_failed_at=excluded.resume_failed_at",
        params![
            s.id,
            s.crew_id,
            s.role.as_str(),
            s.executor,
            s.agent_session_id,
            s.display_name,
            s.working_directory,
            s.model,
            s.effort,
            s.updated_at,
            extra.map(|v| v.to_string()),
            s.title,
            s.brief,
            s.desired.as_str(),
            s.ext,
            s.worktree,
            s.was_working,
            s.last_active_at,
            s.last_output_at,
            s.last_progress_at,
            s.awaiting_question,
            s.resume_failed_at,
        ],
    )?;
    Ok(())
}

pub fn list_for_crew(conn: &Connection, crew_id: &str) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM sessions WHERE crew_id=?1 ORDER BY id"
    ))?;
    let rows = stmt
        .query_map([crew_id], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Session>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM sessions WHERE id=?1"),
            [id],
            row,
        )
        .optional()?)
}

pub fn list_all(conn: &Connection) -> Result<Vec<Session>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM sessions ORDER BY crew_id, id"))?;
    Ok(st
        .query_map([], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
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

/// 这个 session 的执行器类型（`claude` / `codex` / `acp:<id>` …）。session 不存在是错误。
pub fn executor(conn: &Connection, session_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT executor FROM sessions WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("没有 session {session_id}")))
}
