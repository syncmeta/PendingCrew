use crate::{Result, json_col};
use crewd_proto::{Message, SenderKind};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

/// v3 的 sender 词表（裁定 6）。旧词 `user` / `pendingcrew` 归一；认不出 → None。
pub fn normalize_sender_kind(k: &str) -> Option<&'static str> {
    match k {
        "human" | "user" => Some("human"),
        "session" => Some("session"),
        "captain" => Some("captain"),
        "system" | "pendingcrew" => Some("system"),
        _ => None,
    }
}

/// v3 的 mention 词表（裁定 6）。导入器拿它判「认不出」；迁移里的检查是同一张表的 SQL 写法。
pub const MENTION_KINDS: &[&str] = &["session", "captain", "human", "broadcast"];

const COLS: &str = "seq,id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let sender_kind: String = r.get(3)?;
    let sender_kind = SenderKind::parse(&sender_kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("未知 senderKind {sender_kind}").into(),
        )
    })?;
    Ok(Message {
        seq: r.get(0)?,
        id: r.get(1)?,
        channel_id: r.get(2)?,
        sender_kind,
        sender_session_id: r.get(4)?,
        sender_name: r.get(5)?,
        text: r.get(6)?,
        category: r.get(7)?,
        in_reply_to: r.get(8)?,
        mentions: json_col(r, 9)?,
        attachments: json_col(r, 10)?,
        created_at: r.get(11)?,
    })
}

/// 写入一条消息。id 已存在 → `Ok(None)`（幂等）；否则返回分配到的 seq。
pub fn insert(conn: &Connection, m: &Message, extra: Option<&Value>) -> Result<Option<i64>> {
    let n = conn.execute(
        "INSERT INTO messages(id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO NOTHING",
        params![
            m.id,
            m.channel_id,
            m.sender_kind.as_str(),
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
    Ok((n > 0).then(|| conn.last_insert_rowid()))
}

/// `after_seq < seq`（且 `seq < before_seq`，若给了）的消息，按 seq 升序。
/// 给了 `before_seq` 时取紧挨着它的**最后** `limit` 条（往上翻页），仍按升序返回。
pub fn list(
    conn: &Connection,
    channel_id: &str,
    after_seq: i64,
    before_seq: Option<i64>,
    limit: u32,
) -> Result<Vec<Message>> {
    let mut out = match before_seq {
        None => {
            let mut st = conn.prepare(&format!(
                "SELECT {COLS} FROM messages WHERE channel_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3"
            ))?;
            st.query_map(params![channel_id, after_seq, limit], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
        Some(before) => {
            let mut st = conn.prepare(&format!(
                "SELECT {COLS} FROM messages WHERE channel_id=?1 AND seq>?2 AND seq<?3 ORDER BY seq DESC LIMIT ?4"
            ))?;
            st.query_map(params![channel_id, after_seq, before, limit], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    if before_seq.is_some() {
        out.reverse();
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Message>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM messages WHERE id=?1"),
            [id],
            row,
        )
        .optional()?)
}

/// 群里最大的 seq；空群 0。
pub fn max_seq(conn: &Connection, channel_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM messages WHERE channel_id=?1",
        [channel_id],
        |r| r.get(0),
    )?)
}
