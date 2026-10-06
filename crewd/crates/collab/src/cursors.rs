//! 已读游标（spec §22.2：红标与未读都按游标派生，不建表）。reader = "human" 或 session id。
use crate::{Change, Result, Tx, channels, listens};
use crewd_proto::{Badge, MentionKind, Message};
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
pub fn badge(conn: &Connection, channel: &str) -> Result<Badge> {
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
    Ok(Badge {
        channel_id: channel.into(),
        unread_for_human: unread,
        human_mentions: mentions,
    })
}

/// 每个群一次最多带多少条未读（再多就是该 `read_channel` 主动拉的量了）。
pub const UNREAD_CAP: u32 = 200;

/// 这条消息对 `session` 可见吗（移植 `CrewWhiteboardVisibility.isVisible`）。
pub fn visible_to(m: &Message, session: &str) -> bool {
    if m.mentions.iter().any(|x| x.kind == MentionKind::Broadcast) {
        return true;
    }
    let narrowing: Vec<_> = m
        .mentions
        .iter()
        .filter(|x| matches!(x.kind, MentionKind::Session | MentionKind::Captain))
        .collect();
    if narrowing.is_empty() || m.sender_session_id.as_deref() == Some(session) {
        return true;
    }
    narrowing
        .iter()
        .any(|x| x.target_id.as_deref() == Some(session))
}

/// 看得见、但收窄型 mention 里没有它 → 返回那些目标（渲染时标「发给 X 的」）；否则 None。
pub fn directed_targets(m: &Message, session: &str) -> Option<Vec<String>> {
    let targets: Vec<String> = m
        .mentions
        .iter()
        .filter(|x| matches!(x.kind, MentionKind::Session | MentionKind::Captain))
        .filter_map(|x| x.target_id.clone())
        .collect();
    if targets.is_empty() || targets.iter().any(|t| t == session) {
        return None;
    }
    Some(targets)
}

/// 注入面：该 session 所在的群 + 正在收听的群里，游标之后、对它可见、不是它自己发的消息。
/// 只读不推进游标（注入成功后由调用方 `advance`）；缺游标行的群先钉末尾、本次不返回。
///
/// 窗口规则（裁定 R6）：每个群取游标之后**最旧**的至多 `UNREAD_CAP` 条（按 seq 升序截断，绝不丢旧留新）。
/// 调用方只能把游标推进到**实际带上**的那批里最大的 seq——不能推到群末尾，否则没带上的消息永远丢了；
/// 下一次调用自然从紧接其后的消息开始。
pub fn unread_for(tx: &mut Tx<'_>, session: &str) -> Result<Vec<(String, Vec<Message>)>> {
    let mut ids: Vec<String> = channels::channels_of_session(tx.conn(), session)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    for c in listens::channels_of(tx.conn(), session, tx.now_ms())? {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    let mut out = Vec::new();
    for ch in ids {
        let Some(after) = cursor(tx.conn(), session, &ch)? else {
            pin_to_end(tx, session, &ch)?;
            continue;
        };
        let msgs: Vec<Message> =
            crewd_store::messages::list(tx.conn(), &ch, after, None, UNREAD_CAP)?
                .into_iter()
                .filter(|m| {
                    m.sender_session_id.as_deref() != Some(session) && visible_to(m, session)
                })
                .collect();
        if !msgs.is_empty() {
            out.push((ch, msgs));
        }
    }
    Ok(out)
}

/// 人类看过某群到 `seq`（`read.mark`）。Mac / iOS 共用一个人类游标。
pub fn mark_read(tx: &mut Tx<'_>, channel: &str, seq: i64) -> Result<Badge> {
    channels::get(tx.conn(), channel)?;
    advance(tx, HUMAN, channel, seq)?;
    tx.note(Change::BadgeChanged {
        channel_id: channel.into(),
    });
    badge(tx.conn(), channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sender;
    use crate::messages::{NewPost, post};
    use crate::testkit::{MIN, NOW, msg, ok, ok_at, store};
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

    fn say(
        s: &mut crewd_store::Store,
        who: &str,
        ch: &str,
        mentions: Vec<Mention>,
    ) -> crewd_proto::Message {
        ok(s, |tx| {
            post(
                tx,
                Sender::Session(who.into()),
                NewPost {
                    channel_id: ch.into(),
                    text: format!("{who} 说"),
                    mentions,
                    ..Default::default()
                },
            )
        })
        .message
    }
    fn at(kind: MentionKind, t: Option<&str>) -> Mention {
        Mention {
            kind,
            target_id: t.map(Into::into),
        }
    }
    fn unread_ids(s: &mut crewd_store::Store, who: &str) -> Vec<String> {
        ok(s, |tx| unread_for(tx, who))
            .into_iter()
            .flat_map(|(_, ms)| ms.into_iter().map(|m| m.id))
            .collect()
    }

    #[test]
    fn missing_cursor_pins_to_end_instead_of_dumping_history() {
        let mut s = store();
        say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        s.conn()
            .execute("DELETE FROM read_cursors WHERE reader='w-a2'", [])
            .unwrap();
        assert!(unread_ids(&mut s, "w-a2").is_empty(), "缺游标不能当 0");
        assert!(
            cursor(s.conn(), "w-a2", "crew-a/lobby").unwrap().is_some(),
            "顺手钉上"
        );
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        assert_eq!(unread_ids(&mut s, "w-a2"), [m.id]);
    }

    #[test]
    fn own_messages_are_never_unread_and_advance_is_monotonic() {
        let mut s = store();
        say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        assert!(unread_ids(&mut s, "w-a1").is_empty());
        let m = say(&mut s, "w-a2", "crew-a/lobby", vec![]);
        ok(&mut s, |tx| advance(tx, "w-a1", "crew-a/lobby", m.seq));
        ok(&mut s, |tx| advance(tx, "w-a1", "crew-a/lobby", 1));
        assert_eq!(
            cursor(s.conn(), "w-a1", "crew-a/lobby").unwrap(),
            Some(m.seq),
            "只进不退"
        );
    }

    #[test]
    fn directed_messages_are_hidden_from_others_but_human_and_broadcast_are_not() {
        let mut s = store();
        let to_a2 = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Session, Some("w-a2"))],
        );
        let to_human = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Human, None)],
        );
        let widened = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![
                at(MentionKind::Broadcast, None),
                at(MentionKind::Session, Some("w-a2")),
            ],
        );
        assert_eq!(
            unread_ids(&mut s, "w-a2"),
            [to_a2.id.clone(), to_human.id.clone(), widened.id.clone()]
        );
        assert_eq!(
            unread_ids(&mut s, "cap-a"),
            [to_human.id.clone(), widened.id.clone()]
        );
        assert!(visible_to(&to_a2, "w-a1"), "自己发的永远可见");
        assert_eq!(
            directed_targets(&widened, "cap-a"),
            Some(vec!["w-a2".to_string()]),
            "看得见但不是给我的 → 要标注"
        );
        assert_eq!(directed_targets(&widened, "w-a2"), None);
        assert_eq!(
            directed_targets(&to_human, "cap-a"),
            None,
            "只 @ 人类 = 广播，不标"
        );
    }

    #[test]
    fn visibility_follows_the_stored_target_not_the_current_captain() {
        let mut s = store();
        let m = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Captain, None)],
        );
        ok(&mut s, |tx| crate::crews::set_captain(tx, "crew-a", "w-a2"));
        assert!(visible_to(&m, "cap-a"), "发送时的机长是 cap-a");
        assert!(!visible_to(&m, "w-a2"), "后来的机长不是那条消息的对象");
    }

    #[test]
    fn listened_channels_join_the_injection_surface_until_they_expire() {
        let mut s = store();
        let m = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        assert!(unread_ids(&mut s, "w-a1").is_empty());
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + 30 * MIN), vec![])
        });
        let m2 = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        assert_eq!(
            unread_ids(&mut s, "w-a1"),
            [m2.id],
            "开始收听时从末尾开始，{m:?} 不算"
        );
        let (got, _) = ok_at(&mut s, NOW + 31 * MIN, |tx| unread_for(tx, "w-a1"));
        assert!(got.is_empty(), "收听过期");
    }

    #[test]
    fn mark_read_moves_the_human_cursor_and_clears_the_badge() {
        let mut s = store();
        let m = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Human, None)],
        );
        let (b, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", m.seq));
        assert_eq!((b.unread_for_human, b.human_mentions), (0, 0));
        assert_eq!(
            ch,
            [crate::Change::BadgeChanged {
                channel_id: "crew-a/lobby".into()
            }]
        );
    }

    /// R6：窗口规则——取最旧的 UNREAD_CAP 条，游标只推到实际带上的最大 seq；下一批紧接其后。
    #[test]
    fn more_than_the_cap_returns_the_oldest_and_the_next_call_starts_right_after() {
        let mut s = store();
        let total = UNREAD_CAP as usize + 5;
        let ids: Vec<String> = (0..total)
            .map(|_| say(&mut s, "w-a1", "crew-a/lobby", vec![]).id)
            .collect();
        let first = ok(&mut s, |tx| unread_for(tx, "w-a2"));
        assert_eq!(first.len(), 1);
        let got: Vec<String> = first[0].1.iter().map(|m| m.id.clone()).collect();
        assert_eq!(got.len(), UNREAD_CAP as usize);
        assert_eq!(
            got,
            ids[..UNREAD_CAP as usize],
            "拿的是最旧的 200 条，不是最新的"
        );
        let last = first[0].1.last().unwrap().seq;
        ok(&mut s, |tx| advance(tx, "w-a2", "crew-a/lobby", last));
        assert_eq!(
            unread_ids(&mut s, "w-a2"),
            ids[UNREAD_CAP as usize..],
            "下一批紧接在已交付的之后"
        );
    }
}
