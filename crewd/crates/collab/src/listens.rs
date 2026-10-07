//! 收听（§22.7 Q2：session 可以收听一个群，持久化；收听者在该群有新消息时也会被叫醒——叫醒判定在 M3c）。
//! `until_ms = None` = 一直收听，直到 `clear`。`senders` 非空 = 只对这些发送者的消息叫醒（判定在 M3c）。
//!
//! 收听不扩大读范围（终审 I4；B15 / §24.5）：一条收听只在「群没归档、且收听者此刻读得到它」（`channels::readable_by`）
//! 时算数。`set` 读不到就拒（Forbidden `listen.not_readable`）；设下之后失去读权（被移出、机组挪走换了 home……）
//! 的行**留着但不算**——注入面（`channels_of`）、叫醒名单（`active_for_channel`）、批里的投递
//! （`channels::has_listen_row`）三处都用同一判据过滤；读权回来就又算数。与归档的群同一种处理（行留着，用时过滤），
//! 不在每条会丢读权的路径上各删一遍。
use crate::refusal::{Code, Refusal};
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
        return Err(channels::archived(channel));
    }
    if !channels::readable_by(tx.conn(), channel, session)? {
        return Err(CollabError::Forbidden(
            Refusal::new(Code::ListenNotReadable).arg("channel", channel),
        ));
    }
    if until_ms.is_some_and(|u| u <= tx.now_ms()) {
        return Err(CollabError::Invalid(Refusal::new(Code::ListenUntilInPast)));
    }
    // 这个群此刻是否已经在它的注入面上：有效的收听，或本来就在群里（成员 / 本机组大厅 / 机长的人类群）。
    // 不在 = 过期 / 清掉 / 从没收听过——从现在起重新收听，必须钉到末尾，不能把期间的历史倒出来。
    let on_surface = active_for_channel(tx.conn(), channel, tx.now_ms())?
        .iter()
        .any(|l| l.session == session)
        || channels::channels_of_session(tx.conn(), session)?
            .iter()
            .any(|c| c.id == channel);
    tx.conn().execute(
        "INSERT INTO listens(session_id,channel_id,until,senders,created_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(session_id,channel_id) DO UPDATE SET until=excluded.until, senders=excluded.senders",
        params![session, channel, until_ms, serde_json::to_string(&senders)?, tx.now_ts()],
    )?;
    if !on_surface || cursors::cursor(tx.conn(), session, channel)?.is_none() {
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

/// 这个群此刻算数的收听（没过期、群没归档、收听者读得到，见模块注释）。
pub fn active_for_channel(conn: &Connection, channel: &str, now_ms: i64) -> Result<Vec<Listen>> {
    let mut st = conn.prepare(
        "SELECT l.session_id, l.channel_id, l.until, l.senders FROM listens l JOIN channels ch ON ch.id=l.channel_id
          WHERE l.channel_id=?1 AND (l.until IS NULL OR l.until>?2) AND ch.archived_at IS NULL ORDER BY l.session_id",
    )?;
    let all = st
        .query_map(params![channel, now_ms], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(all.len());
    for l in all {
        if channels::readable_by(conn, channel, &l.session)? {
            out.push(l);
        }
    }
    Ok(out)
}

/// 这个 session 此刻算数的收听所在的群（同 `active_for_channel` 的判据）。
pub fn channels_of(conn: &Connection, session: &str, now_ms: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT l.channel_id FROM listens l JOIN channels ch ON ch.id=l.channel_id
          WHERE l.session_id=?1 AND (l.until IS NULL OR l.until>?2) AND ch.archived_at IS NULL
          ORDER BY l.channel_id",
    )?;
    let all = st
        .query_map(params![session, now_ms], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(all.len());
    for c in all {
        if channels::readable_by(conn, &c, session)? {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, org_worker, store};

    #[test]
    fn set_replaces_and_clear_removes() {
        let mut s = store();
        let o = org_worker(&mut s);
        ok(&mut s, |tx| {
            set(tx, &o, "crew-x/lobby", None, vec!["cap-x".into()])
        });
        ok(&mut s, |tx| {
            set(tx, &o, "crew-b/lobby", Some(NOW + MIN), vec![])
        });
        let l = active_for_channel(s.conn(), "crew-x/lobby", NOW).unwrap();
        assert_eq!(
            l,
            [Listen {
                session: o.clone(),
                channel: "crew-x/lobby".into(),
                until_ms: None,
                senders: vec!["cap-x".into()]
            }]
        );
        assert_eq!(
            channels_of(s.conn(), &o, NOW + 2 * MIN).unwrap(),
            ["crew-x/lobby"],
            "过期的不算"
        );
        assert_eq!(ok(&mut s, |tx| clear(tx, &o, Some("crew-x/lobby"))), 1);
        assert_eq!(ok(&mut s, |tx| clear(tx, &o, None)), 1);
        assert!(channels_of(s.conn(), &o, NOW).unwrap().is_empty());
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
                "crew-a/lobby",
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

    /// 终审 I4：收听不扩大读范围（B15 / §24.5）——只能收听读得到的群。别的 home 的人类群、兄弟机组的大厅都读不到
    /// → Forbidden `listen.not_readable`，什么都不写；自己 home 的人类群读得到 → 可以收听。
    #[test]
    fn listening_needs_read_access() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-a")
        });
        for ch in ["crew-x/human", "crew-x/lobby"] {
            match err(&mut s, |tx| set(tx, "w-a1", ch, None, vec![])) {
                CollabError::Forbidden(r) => {
                    assert_eq!(r.code, Code::ListenNotReadable, "{ch}: {r:?}");
                    assert_eq!(r.args, [("channel", ch.to_string())]);
                }
                e => panic!("{ch}: {e:?}"),
            }
        }
        assert_eq!(count(&s, "SELECT COUNT(*) FROM listens"), 0);
        ok(&mut s, |tx| set(tx, "w-b1", "crew-a/human", None, vec![]));
        assert_eq!(
            channels_of(s.conn(), "w-b1", NOW).unwrap(),
            ["crew-a/human"]
        );
    }

    /// 终审 I4 的另一半：收听只在读得到时算数。设下之后失去读权（这里：crew-b 从 crew-a 下挪到一级，crew-a 的
    /// 人类群不再是它的 home）→ 行还在，但注入面、叫醒名单都不认它（同归档的群：行留着，用时过滤）。
    #[test]
    fn a_listen_stops_counting_once_the_channel_is_no_longer_readable() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-a")
        });
        ok(&mut s, |tx| set(tx, "w-b1", "crew-a/human", None, vec![]));
        assert_eq!(
            active_for_channel(s.conn(), "crew-a/human", NOW)
                .unwrap()
                .len(),
            1
        );
        ok(&mut s, |tx| {
            crate::crews::release(tx, "crew-b", "crew-a", None)
        });
        assert!(!crate::channels::readable_by(s.conn(), "crew-a/human", "w-b1").unwrap());
        assert_eq!(count(&s, "SELECT COUNT(*) FROM listens"), 1, "行还在");
        assert!(
            active_for_channel(s.conn(), "crew-a/human", NOW)
                .unwrap()
                .is_empty()
        );
        assert!(channels_of(s.conn(), "w-b1", NOW).unwrap().is_empty());
    }

    /// 复核 6：已归档的群不算「有人在收听」。
    #[test]
    fn active_for_channel_skips_archived_channels() {
        let mut s = store();
        ok(&mut s, |tx| set(tx, "w-a1", "crew-a/lobby", None, vec![]));
        assert_eq!(
            active_for_channel(s.conn(), "crew-a/lobby", NOW)
                .unwrap()
                .len(),
            1
        );
        s.conn()
            .execute(
                "UPDATE channels SET archived_at='2026-09-01T00:00:00.000Z' WHERE id='crew-a/lobby'",
                [],
            )
            .unwrap();
        assert!(
            active_for_channel(s.conn(), "crew-a/lobby", NOW)
                .unwrap()
                .is_empty()
        );
        assert!(channels_of(s.conn(), "w-a1", NOW).unwrap().is_empty());
    }
}
