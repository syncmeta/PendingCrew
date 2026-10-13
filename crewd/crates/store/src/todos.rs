use crate::{Result, json_col};
use crewd_proto::{Ledger, Todo, TodoResponse};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

/// 原子写入：todo 与其 responses 要么全进，要么全不进（自带 savepoint，可嵌在外层事务里）。
/// 失败时回滚到 savepoint 并返回原始错误。
pub fn insert(conn: &Connection, t: &Todo, extra: Option<&Value>) -> Result<()> {
    conn.execute_batch("SAVEPOINT todo_insert")?;
    match insert_rows(conn, t, extra) {
        Ok(()) => {
            conn.execute_batch("RELEASE todo_insert")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO todo_insert; RELEASE todo_insert");
            Err(e)
        }
    }
}

fn insert_rows(conn: &Connection, t: &Todo, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO todos(id,crew_id,ledger,number,text,status,created_at,updated_at,deleted_at,attachments,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            t.id,
            t.crew_id,
            t.ledger.as_str(),
            t.number,
            t.text,
            t.status,
            t.created_at,
            t.updated_at,
            t.deleted_at,
            serde_json::to_string(&t.attachments)?,
            extra.map(|v| v.to_string()),
        ],
    )?;
    for (ord, r) in t.responses.iter().enumerate() {
        conn.execute(
            "INSERT INTO todo_responses(id,todo_id,ord,session_id,sender_name,text,status,attachments,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                r.id,
                t.id,
                ord as i64,
                r.session_id,
                r.sender_name,
                r.text,
                r.status,
                serde_json::to_string(&r.attachments)?,
                r.created_at,
            ],
        )?;
    }
    crate::work_organization::register_raw_entry(conn, t)?;
    Ok(())
}

pub fn list(conn: &Connection, crew_id: &str, ledger: Ledger) -> Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id,crew_id,number,text,status,created_at,updated_at,deleted_at,attachments
         FROM todos WHERE crew_id=?1 AND ledger=?2 ORDER BY number",
    )?;
    let mut todos = stmt
        .query_map(params![crew_id, ledger.as_str()], |r| {
            Ok(Todo {
                requirement_id: None,
                id: r.get(0)?,
                crew_id: r.get(1)?,
                ledger,
                number: r.get(2)?,
                text: r.get(3)?,
                status: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                deleted_at: r.get(7)?,
                attachments: json_col(r, 8)?,
                responses: vec![],
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ledger == Ledger::Agent {
        let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='organization_work_book')",[],|r|r.get(0))?;
        if active {
            let state = crate::work_organization::load(conn)?.ok_or_else(|| {
                crate::StoreError::Invalid("raw requirements have no organization authority".into())
            })?;
            crate::work_organization::verify_references(conn, &state)?;
            let book = crewd_work::WorkBook::restore(state, crewd_work::Config::default())
                .map_err(|v| crate::StoreError::Invalid(v.to_string()))?;
            for todo in &mut todos {
                let (core,public):(String,i64)=conn.query_row("SELECT core_id,number FROM object_refs WHERE raw_todo_id=?1 AND crew_id=?2 AND kind='requirement'",params![todo.id,todo.crew_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
                let core = core.parse().map_err(|_| {
                    crate::StoreError::Invalid("raw requirement core identity is invalid".into())
                })?;
                let view = book.entry_view(core).ok_or_else(|| {
                    crate::StoreError::Invalid("raw requirement entry authority is missing".into())
                })?;
                todo.requirement_id = Some(format!("#{public}"));
                todo.status = match view.status {
                    crewd_work::EntryStatus::Unclaimed => "unclaimed",
                    crewd_work::EntryStatus::InProgress => "in_progress",
                    crewd_work::EntryStatus::Done => "done",
                    crewd_work::EntryStatus::NeedsRuling => "needs_ruling",
                    crewd_work::EntryStatus::Closed => "closed",
                }
                .into();
            }
        }
    }
    let mut rstmt = conn.prepare(
        "SELECT id,session_id,sender_name,text,status,attachments,created_at
         FROM todo_responses WHERE todo_id=?1 ORDER BY ord",
    )?;
    for t in &mut todos {
        t.responses = rstmt
            .query_map([&t.id], |r| {
                Ok(TodoResponse {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    sender_name: r.get(2)?,
                    text: r.get(3)?,
                    status: r.get(4)?,
                    attachments: json_col(r, 5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(todos)
}

/// Original source facts only; callers derive lifecycle from the organization authority.
pub fn get_raw(conn: &Connection, id: &str) -> Result<Option<Todo>> {
    let mut todo=conn.query_row("SELECT id,crew_id,ledger,number,text,status,created_at,updated_at,deleted_at,attachments FROM todos WHERE id=?1",[id],|r| {
        let ledger:String=r.get(2)?;
        Ok(Todo { requirement_id:None,id:r.get(0)?,crew_id:r.get(1)?,ledger:match ledger.as_str() { "agent"=>Ledger::Agent,"human"=>Ledger::Human,_=>return Err(rusqlite::Error::FromSqlConversionFailure(2,rusqlite::types::Type::Text,"invalid raw requirement ledger".into())) },number:r.get(3)?,text:r.get(4)?,status:r.get(5)?,created_at:r.get(6)?,updated_at:r.get(7)?,deleted_at:r.get(8)?,attachments:json_col(r,9)?,responses:vec![] })
    }).optional()?;
    if let Some(t) = &mut todo {
        let mut q=conn.prepare("SELECT id,session_id,sender_name,text,status,attachments,created_at FROM todo_responses WHERE todo_id=?1 ORDER BY ord")?;
        t.responses = q
            .query_map([id], |r| {
                Ok(TodoResponse {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    sender_name: r.get(2)?,
                    text: r.get(3)?,
                    status: r.get(4)?,
                    attachments: json_col(r, 5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(todo)
}
