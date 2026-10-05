use crate::{Result, json_col};
use crewd_proto::{Ledger, Todo, TodoResponse};
use rusqlite::{Connection, params};
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
