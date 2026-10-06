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

/// 人类在某群的红标（spec §22.2：按游标派生）。人自己发的不算；缺游标 = 0（新群一开始是空的）。
pub fn badge(conn: &Connection, channel: &str) -> Result<crewd_proto::Badge> {
    let after = cursor(conn, HUMAN, channel)?.unwrap_or(0);
    let (unread, mentions): (i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(EXISTS(SELECT 1 FROM json_each(m.mentions) j
                                     WHERE json_extract(j.value,'$.kind')='human')),0)
           FROM messages m
          WHERE m.channel_id=?1 AND m.seq>?2 AND m.sender_kind<>'human'",
        params![channel, after],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(crewd_proto::Badge {
        channel_id: channel.into(),
        unread_for_human: unread,
        human_mentions: mentions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{msg, store};
    use crewd_proto::{Mention, MentionKind, SenderKind};

    #[test]
    fn badge_counts_unread_and_human_mentions_after_the_human_cursor() {
        let s = store();
        msg(
            &s,
            "crew-a/lobby",
            "m1",
            SenderKind::Session,
            Some("w-a1"),
            vec![],
        );
        msg(
            &s,
            "crew-a/lobby",
            "m2",
            SenderKind::Session,
            Some("w-a1"),
            vec![Mention {
                kind: MentionKind::Human,
                target_id: None,
            }],
        );
        let b = badge(s.conn(), "crew-a/lobby").unwrap();
        assert_eq!((b.unread_for_human, b.human_mentions), (2, 1));
    }
}
