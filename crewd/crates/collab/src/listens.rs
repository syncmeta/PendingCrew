//! 收听（§22.7 Q2：session 可以收听一个群，持久化；收听者在该群有新消息时也会被叫醒——叫醒判定在 M3c）。
//! `until_ms = None` = 一直收听，直到 `clear`。`senders` 非空 = 只对这些发送者的消息叫醒（判定在 M3c）。
use crate::{CollabError, Result, Tx, channels, cursors, sessions};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    pub session: String,
    pub channel: String,
    pub until_ms: Option<i64>,
    pub senders: Vec<String>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Listen> {
    let senders: String = r.get(3)?;
    Ok(Listen {
        session: r.get(0)?,
        channel: r.get(1)?,
        until_ms: r.get(2)?,
        senders: serde_json::from_str(&senders).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}

pub fn set(
    tx: &mut Tx<'_>,
    session: &str,
    channel: &str,
    until_ms: Option<i64>,
    senders: Vec<String>,
) -> Result<()> {
    sessions::get(tx.conn(), session)?;
    let ch = channels::get(tx.conn(), channel)?;
    if ch.archived_at.is_some() {
        return Err(CollabError::Invalid("群已归档".into()));
    }
    if until_ms.is_some_and(|u| u <= tx.now_ms()) {
        return Err(CollabError::Invalid("收听截止时间必须晚于现在".into()));
    }
    tx.conn().execute(
        "INSERT INTO listens(session_id,channel_id,until,senders,created_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(session_id,channel_id) DO UPDATE SET until=excluded.until, senders=excluded.senders",
        params![session, channel, until_ms, serde_json::to_string(&senders)?, tx.now_ts()],
    )?;
    if cursors::cursor(tx.conn(), session, channel)?.is_none() {
        cursors::pin_to_end(tx, session, channel)?;
    }
    Ok(())
}

pub fn clear(tx: &mut Tx<'_>, session: &str, channel: Option<&str>) -> Result<usize> {
    Ok(match channel {
        Some(c) => tx.conn().execute(
            "DELETE FROM listens WHERE session_id=?1 AND channel_id=?2",
            params![session, c],
        )?,
        None => tx
            .conn()
            .execute("DELETE FROM listens WHERE session_id=?1", [session])?,
    })
}

pub fn active_for_channel(conn: &Connection, channel: &str, now_ms: i64) -> Result<Vec<Listen>> {
    let mut st = conn.prepare(
        "SELECT session_id, channel_id, until, senders FROM listens
          WHERE channel_id=?1 AND (until IS NULL OR until>?2) ORDER BY session_id",
    )?;
    Ok(st
        .query_map(params![channel, now_ms], row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn channels_of(conn: &Connection, session: &str, now_ms: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT l.channel_id FROM listens l JOIN channels ch ON ch.id=l.channel_id
          WHERE l.session_id=?1 AND (l.until IS NULL OR l.until>?2) AND ch.archived_at IS NULL
          ORDER BY l.channel_id",
    )?;
    Ok(st
        .query_map(params![session, now_ms], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, err, ok, store};

    #[test]
    fn set_replaces_and_clear_removes() {
        let mut s = store();
        ok(&mut s, |tx| {
            set(tx, "w-a1", "crew-x/lobby", None, vec!["cap-x".into()])
        });
        ok(&mut s, |tx| {
            set(tx, "w-a1", "crew-b/lobby", Some(NOW + MIN), vec![])
        });
        let l = active_for_channel(s.conn(), "crew-x/lobby", NOW).unwrap();
        assert_eq!(
            l,
            [Listen {
                session: "w-a1".into(),
                channel: "crew-x/lobby".into(),
                until_ms: None,
                senders: vec!["cap-x".into()]
            }]
        );
        assert_eq!(
            channels_of(s.conn(), "w-a1", NOW + 2 * MIN).unwrap(),
            ["crew-x/lobby"],
            "过期的不算"
        );
        assert_eq!(ok(&mut s, |tx| clear(tx, "w-a1", Some("crew-x/lobby"))), 1);
        assert_eq!(ok(&mut s, |tx| clear(tx, "w-a1", None)), 1);
        assert!(channels_of(s.conn(), "w-a1", NOW).unwrap().is_empty());
    }

    #[test]
    fn listening_needs_a_live_channel_and_a_future_deadline() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| set(tx, "w-a1", "nope", None, vec![])),
            CollabError::NotFound(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| set(
                tx,
                "w-a1",
                "crew-x/lobby",
                Some(NOW),
                vec![]
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| set(tx, "ghost", "crew-x/lobby", None, vec![])),
            CollabError::NotFound(_)
        ));
    }
}
