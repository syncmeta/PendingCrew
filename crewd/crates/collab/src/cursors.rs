//! 已读游标（spec §22.2：红标与未读都按游标派生，不建表）。reader = "human" 或 session id。
use crate::{Change, Result, Tx, channels, listens};
use crewd_proto::{Badge, Message};
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

/// 推进游标（只进不退；夹在 0..=群当前末尾之内——远未来的 seq 不能吞掉之后才到的消息）。
pub fn advance(tx: &mut Tx<'_>, reader: &str, channel: &str, seq: i64) -> Result<()> {
    let seq = seq.clamp(0, crewd_store::messages::max_seq(tx.conn(), channel)?);
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

/// 注入面：该 session 所在的群 + 正在收听的群里，游标之后、不是它自己发的消息（@ 只决定叫醒谁，不收窄
/// 可见范围——群里的每条消息群成员都看得见，N1 裁定 8）。
/// 缺游标行的群先钉末尾、本次不返回。除此之外只读，唯一的写：游标之后**开头**那一段自己发的消息
/// 被游标越过——那段里没有任何东西可投递，跳过永远安全，否则一整个窗口的自己发的消息会把后面的消息永远堵住。
///
/// 窗口规则（裁定 R6）：每个群在游标之后按 seq 升序翻页扫描，收满最旧的 `UNREAD_CAP` 条可投递消息
/// 或扫到群末尾为止（绝不丢旧留新）。调用方只能把游标推进到**实际带上**的那批里最大的 seq——
/// 不能推到群末尾，否则没带上的消息永远丢了；下一次调用自然从紧接其后的消息开始。
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
        let Some(mut after) = cursor(tx.conn(), session, &ch)? else {
            pin_to_end(tx, session, &ch)?;
            continue;
        };
        let mut got: Vec<Message> = Vec::new();
        let mut leading_end: Option<i64> = None; // 开头那段自己发的消息的最后一个 seq
        'scan: loop {
            let page = crewd_store::messages::list(tx.conn(), &ch, after, None, UNREAD_CAP)?;
            let Some(last) = page.last().map(|m| m.seq) else {
                break;
            };
            let full = page.len() >= UNREAD_CAP as usize;
            for m in page {
                if m.sender_session_id.as_deref() != Some(session) {
                    got.push(m);
                    if got.len() >= UNREAD_CAP as usize {
                        break 'scan;
                    }
                } else if got.is_empty() {
                    leading_end = Some(m.seq);
                }
            }
            if !full {
                break;
            }
            after = last;
        }
        if let Some(seq) = leading_end {
            advance(tx, session, &ch, seq)?;
        }
        if !got.is_empty() {
            out.push((ch, got));
        }
    }
    Ok(out)
}

/// 人类看过某群到 `seq`（`read.mark`；夹在群末尾之内，回退 / 空操作不产生变更）。Mac / iOS 共用一个人类游标。
pub fn mark_read(tx: &mut Tx<'_>, channel: &str, seq: i64) -> Result<Badge> {
    channels::get(tx.conn(), channel)?;
    let target = seq.clamp(0, crewd_store::messages::max_seq(tx.conn(), channel)?);
    // 回退 / 原地不动：什么都不改、不发变更（缺游标 = 0）。
    if target > cursor(tx.conn(), HUMAN, channel)?.unwrap_or(0) {
        advance(tx, HUMAN, channel, target)?;
        tx.note(Change::BadgeChanged {
            channel_id: channel.into(),
        });
    }
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
    fn a_mention_wakes_but_never_hides_everyone_in_the_channel_sees_it() {
        let mut s = store();
        let to_a2 = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Session, Some("w-a2"))],
        );
        let to_cap = say(
            &mut s,
            "w-a1",
            "crew-a/lobby",
            vec![at(MentionKind::Captain, None)],
        );
        assert_eq!(
            unread_ids(&mut s, "w-a2"),
            [to_a2.id.clone(), to_cap.id.clone()]
        );
        assert_eq!(
            unread_ids(&mut s, "cap-a"),
            [to_a2.id.clone(), to_cap.id.clone()],
            "@ 别人的也看得见"
        );
        assert!(
            unread_ids(&mut s, "w-a1").is_empty(),
            "自己发的不是自己的未读"
        );
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

    /// 复核 1（a）：前 200 条是自己发的，第 201 条才是别人发的。
    #[test]
    fn a_window_of_own_messages_does_not_stall_the_unread_surface() {
        let mut s = store();
        for _ in 0..UNREAD_CAP {
            say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        }
        let m = say(&mut s, "w-a2", "crew-a/lobby", vec![]);
        assert_eq!(unread_ids(&mut s, "w-a1"), [m.id]);
    }

    /// 复核 1（b）：开头一段自己发的消息，游标被安全地推过去；全程没有可投递的就推到末尾。
    #[test]
    fn leading_run_of_undeliverable_messages_advances_the_cursor() {
        let mut s = store();
        let mut last_own = 0;
        for _ in 0..3 {
            last_own = say(&mut s, "cap-a", "crew-a/lobby", vec![]).seq;
        }
        assert!(unread_ids(&mut s, "cap-a").is_empty());
        assert_eq!(
            cursor(s.conn(), "cap-a", "crew-a/lobby").unwrap(),
            Some(last_own),
            "没有可投递的 = 推到扫过的末尾"
        );
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        say(&mut s, "cap-a", "crew-a/lobby", vec![]);
        let m_after = say(&mut s, "w-a2", "crew-a/lobby", vec![]);
        // 可投递消息之后的自己发的消息不能被越过：游标只推过「开头那段」。
        assert_eq!(unread_ids(&mut s, "cap-a"), [m.id.clone(), m_after.id]);
        assert_eq!(
            cursor(s.conn(), "cap-a", "crew-a/lobby").unwrap(),
            Some(last_own),
            "遇到第一条可投递的就停；调用方自己 advance"
        );
    }

    /// 复核 2：收听过期后再收听，不能把期间的历史倒出来。
    #[test]
    fn relistening_after_expiry_starts_from_the_end() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + MIN), vec![])
        });
        for _ in 0..3 {
            say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        }
        ok_at(&mut s, NOW + 2 * MIN, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + 30 * MIN), vec![])
        });
        assert!(unread_ids(&mut s, "w-a1").is_empty(), "过期期间的不算");
        let m = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        assert_eq!(unread_ids(&mut s, "w-a1"), [m.id]);
    }

    /// 复核 2：clear 之后再 set，同样从末尾开始。
    #[test]
    fn relistening_after_clear_starts_from_the_end() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", None, vec![])
        });
        ok(&mut s, |tx| crate::listens::clear(tx, "w-a1", None));
        for _ in 0..3 {
            say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        }
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", None, vec![])
        });
        assert!(unread_ids(&mut s, "w-a1").is_empty());
    }

    /// 复核 2：续期一个仍然有效的收听不动游标——没读的还在。
    #[test]
    fn extending_an_active_listen_keeps_the_unread() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + 10 * MIN), vec![])
        });
        let m = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + 30 * MIN), vec![])
        });
        assert_eq!(unread_ids(&mut s, "w-a1"), [m.id]);
    }

    /// 复核 2：在自己本来就「在」的群（成员 / 本机组大厅）上收听，也不动游标。
    #[test]
    fn listening_on_a_channel_you_are_already_in_keeps_the_cursor() {
        let mut s = store();
        let m = say(&mut s, "w-a2", "crew-a/lobby", vec![]);
        ok(&mut s, |tx| {
            crate::listens::set(tx, "w-a1", "crew-a/lobby", None, vec![])
        });
        assert_eq!(unread_ids(&mut s, "w-a1"), [m.id]);
    }

    /// 复核 3：旧数据里没有 target 的 captain mention 在数据层一次性落成机长指针（`backfill_v3`），
    /// 读时不再解析；落定之后机长换人，存下来的 target 仍是当时的机长。
    #[test]
    fn legacy_captain_mention_is_resolved_once_in_the_data_and_stays_put() {
        let mut s = store();
        msg(
            &s,
            "crew-a/lobby",
            "legacy-1",
            SenderKind::Session,
            Some("w-a1"),
            vec![at(MentionKind::Captain, None)],
        );
        crewd_store::schema::backfill_v3(s.conn()).unwrap();
        ok(&mut s, |tx| crate::crews::set_captain(tx, "crew-a", "w-a2"));
        let stored = crewd_store::messages::get(s.conn(), "legacy-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.mentions,
            [at(MentionKind::Captain, Some("cap-a"))],
            "落定的是当时的机长"
        );
    }

    /// 复核 5：advance 夹在群末尾之内；mark_read 同；回退 / 空操作不产生变更。
    #[test]
    fn advance_and_mark_read_clamp_to_the_channel_end() {
        let mut s = store();
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        ok(&mut s, |tx| advance(tx, "w-a2", "crew-a/lobby", 1_000_000));
        assert_eq!(
            cursor(s.conn(), "w-a2", "crew-a/lobby").unwrap(),
            Some(m.seq)
        );
        let (b, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", i64::MAX));
        assert_eq!(b.unread_for_human, 0);
        assert_eq!(
            cursor(s.conn(), HUMAN, "crew-a/lobby").unwrap(),
            Some(m.seq)
        );
        assert_eq!(ch.len(), 1);
        // 之后新来一条：被夹掉的「远未来」不会吞掉它。
        let m2 = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        assert_eq!(badge(s.conn(), "crew-a/lobby").unwrap().unread_for_human, 1);
        assert!(m2.seq > m.seq);
    }

    #[test]
    fn mark_read_backwards_or_noop_changes_nothing_and_emits_nothing() {
        let mut s = store();
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        ok(&mut s, |tx| mark_read(tx, "crew-a/lobby", m.seq));
        let (_, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", 1));
        assert!(ch.is_empty(), "回退不发 BadgeChanged");
        let (_, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", m.seq));
        assert!(ch.is_empty(), "原地不动不发 BadgeChanged");
        let (_, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", i64::MAX));
        assert!(ch.is_empty(), "已在末尾时远未来的 seq 也是空操作");
        assert_eq!(
            cursor(s.conn(), HUMAN, "crew-a/lobby").unwrap(),
            Some(m.seq)
        );
    }
}
