//! 已读游标（spec §22.2：红标与未读都按游标派生，不建表）。reader = "human" 或 session id。
use crate::{Result, Tx};
use rusqlite::{Connection, OptionalExtension, params};

/// 人类的游标名（Mac / iOS 共用一个）。
pub const HUMAN: &str = "human";

pub fn cursor(conn: &Connection, reader: &str, channel: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT seq FROM read_cursors WHERE reader=?1 AND channel_id=?2",
            params![reader, channel],
            |r| r.get(0),
        )
        .optional()?)
}

/// 推进游标（只进不退）。
pub fn advance(tx: &mut Tx<'_>, reader: &str, channel: &str, seq: i64) -> Result<()> {
    tx.conn().execute(
        "INSERT INTO read_cursors(reader,channel_id,seq,updated_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(reader,channel_id) DO UPDATE SET seq=MAX(seq, excluded.seq), updated_at=excluded.updated_at",
        params![reader, channel, seq, tx.now_ts()],
    )?;
    Ok(())
}

/// 入群点：把游标钉到群的当前末尾（裁定 9）。返回钉到的 seq。
pub fn pin_to_end(tx: &mut Tx<'_>, reader: &str, channel: &str) -> Result<i64> {
    let end = crewd_store::messages::max_seq(tx.conn(), channel)?;
    advance(tx, reader, channel, end)?;
    Ok(end)
}
