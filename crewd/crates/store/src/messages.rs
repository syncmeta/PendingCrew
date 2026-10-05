use crate::{Result, json_col};
use crewd_proto::Message;
use rusqlite::{Connection, params};
use serde_json::Value;

/// 写入一条消息。id 已存在 → `Ok(None)`（幂等）；否则返回分配到的 seq。
pub fn insert(conn: &Connection, m: &Message, extra: Option<&Value>) -> Result<Option<i64>> {
    let n = conn.execute(
        "INSERT INTO messages(id,crew_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO NOTHING",
        params![
            m.id,
            m.crew_id,
            m.sender_kind,
            m.sender_session_id,
            m.sender_name,
            m.text,
            m.category,
            m.in_reply_to,
            serde_json::to_string(&m.mentions)?,
            serde_json::to_string(&m.attachments)?,
            m.created_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok(if n == 0 {
        None
    } else {
        Some(conn.last_insert_rowid())
    })
}

pub fn list(conn: &Connection, crew_id: &str, after_seq: i64, limit: u32) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT seq,id,crew_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at
         FROM messages WHERE crew_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![crew_id, after_seq, limit], |r| {
            Ok(Message {
                seq: r.get(0)?,
                id: r.get(1)?,
                crew_id: r.get(2)?,
                sender_kind: r.get(3)?,
                sender_session_id: r.get(4)?,
                sender_name: r.get(5)?,
                text: r.get(6)?,
                category: r.get(7)?,
                in_reply_to: r.get(8)?,
                mentions: json_col(r, 9)?,
                attachments: json_col(r, 10)?,
                created_at: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
