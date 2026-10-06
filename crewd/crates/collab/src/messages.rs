//! 发消息（spec §22.2「群规则」、裁定 10–14）、列表、搜索。
use crate::{Change, CollabError, Result, Sender, Tx, channels, crews, cursors, sessions};
use crewd_proto::{
    Attachment, ChannelKind, MemberRef, Mention, MentionKind, Message, Role, SenderKind,
};
use rusqlite::Connection;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewPost {
    pub channel_id: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub mentions: Vec<Mention>,
    pub attachments: Vec<Attachment>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub message: Message,
    /// mentions 含 human、群没被注视、且不是根机组人类群：M3b 回执要明说「人类未注视此群」（§17）。
    pub unwatched_human_mention: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub query: String,
    pub crew_id: Option<String>,
    pub channel_ids: Option<Vec<String>>,
    /// RFC 3339，含。
    pub after: Option<String>,
    pub before: Option<String>,
    pub limit: u32,
}

pub fn get(conn: &Connection, id: &str) -> Result<Message> {
    crewd_store::messages::get(conn, id)?
        .ok_or_else(|| CollabError::NotFound(format!("没有消息 {id}")))
}

pub fn list(
    conn: &Connection,
    channel: &str,
    after: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<Message>> {
    channels::get(conn, channel)?;
    Ok(crewd_store::messages::list(
        conn,
        channel,
        after.unwrap_or(0),
        before,
        limit.clamp(1, 1000),
    )?)
}

/// RFC 3339（任意时区偏移）→ 库里存的 `…sssZ` 形式，才能按字符串比大小。
fn normalize_ts(s: &str) -> Result<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(s.trim())
        .map_err(|e| CollabError::Invalid(format!("时间 {s:?} 不是 RFC 3339：{e}")))?;
    Ok(crewd_store::ts_from_ms(dt.timestamp_millis()))
}

pub fn search(conn: &Connection, q: &Search) -> Result<Vec<Message>> {
    // 两边在 SQL 里用同一个 lower() 折叠（只折 ASCII）：「Über」这类非 ASCII 的原样匹配不能被一边折一边不折弄丢。
    let needle = q.query.trim().to_string();
    if needle.is_empty() {
        return Err(CollabError::Invalid("搜索词不能为空".into()));
    }
    let mut sql = String::from(
        "SELECT m.id FROM messages m JOIN channels ch ON ch.id=m.channel_id WHERE instr(lower(m.text), lower(?1)) > 0",
    );
    let mut args: Vec<rusqlite::types::Value> = vec![needle.into()];
    if let Some(c) = &q.crew_id {
        args.push(c.clone().into());
        sql += &format!(" AND ch.crew_id=?{}", args.len());
    }
    if let Some(ids) = &q.channel_ids {
        let mut ph = Vec::new();
        for id in ids {
            args.push(id.clone().into());
            ph.push(format!("?{}", args.len()));
        }
        if ph.is_empty() {
            return Ok(Vec::new());
        }
        sql += &format!(" AND m.channel_id IN ({})", ph.join(","));
    }
    if let Some(a) = &q.after {
        args.push(normalize_ts(a)?.into());
        sql += &format!(" AND m.created_at>=?{}", args.len());
    }
    if let Some(b) = &q.before {
        args.push(normalize_ts(b)?.into());
        sql += &format!(" AND m.created_at<?{}", args.len());
    }
    args.push(i64::from(q.limit.clamp(1, 500)).into());
    sql += &format!(" ORDER BY m.seq DESC LIMIT ?{}", args.len());
    let mut st = conn.prepare(&sql)?;
    let ids: Vec<String> = st
        .query_map(rusqlite::params_from_iter(args), |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|id| get(conn, id)).collect()
}

pub fn post(tx: &mut Tx<'_>, sender: Sender, p: NewPost) -> Result<Posted> {
    post_with(tx, sender, p, false)
}

/// collab 自己的公告（问、审批、重启询问）：跳过发言者成员检查，其余规则同 `post`。
// TEMP: first caller in Task 10 (asks / approvals / restart announcements)
#[allow(dead_code)]
pub(crate) fn post_internal(tx: &mut Tx<'_>, sender: Sender, p: NewPost) -> Result<Posted> {
    post_with(tx, sender, p, true)
}

/// 正文上限（字节）。
const MAX_BODY_BYTES: usize = 256 * 1024;
/// 只有 collab 自己的公告（`post_internal`）能用的 `category`。
const RESERVED_CATEGORIES: [&str; 3] = ["ask", "approval", "restart"];

/// 空白与零宽字符（U+200B/200C/200D/FEFF）：只含这些的正文等于空。
fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}')
}

fn push_mention(v: &mut Vec<Mention>, m: Mention) {
    if !v.contains(&m) {
        v.push(m);
    }
}

fn post_with(tx: &mut Tx<'_>, sender: Sender, p: NewPost, internal: bool) -> Result<Posted> {
    let conn = tx.conn();
    // R1：根机组人类群懒建；现在是根的机组，它的人类群发言前一律过一遍 ensure（幂等：不存在就建、已归档就解档、
    // 钉机长游标、记 ChannelChanged），同一个 Tx。非根机组没有人类群，指路到它的根。
    if let Some(crew) = p.channel_id.strip_suffix("/human") {
        crews::get(conn, crew)?;
        if crews::is_root(conn, crew)? {
            channels::ensure_human_channel(tx, crew)?;
        } else {
            return Err(CollabError::Invalid(format!(
                "{crew} 是非根机组，没有人类群，请发到 {}/human",
                crews::root_of(conn, crew)?
            )));
        }
    }
    let ch = channels::get(conn, &p.channel_id)?;
    if ch.archived_at.is_some() {
        return Err(CollabError::Invalid(format!(
            "群 {} 已归档，不收消息",
            ch.id
        )));
    }
    if p.text.trim_matches(is_blank).is_empty() {
        return Err(CollabError::Invalid("消息不能为空".into()));
    }
    if p.text.len() > MAX_BODY_BYTES {
        return Err(CollabError::Invalid(format!(
            "消息正文最长 {MAX_BODY_BYTES} 字节，这条有 {} 字节",
            p.text.len()
        )));
    }
    if !internal
        && let Some(c) = p.category.as_deref()
        && RESERVED_CATEGORIES.contains(&c)
    {
        return Err(CollabError::Invalid(format!(
            "分类 {c:?} 留给系统公告，不能由发言带上"
        )));
    }
    if !p.attachments.is_empty() {
        return Err(CollabError::Invalid("附件上传尚未实现".into()));
    }
    let is_root_human = ch.kind == ChannelKind::Human;
    // 发言者
    let (sender_kind, sender_session_id, sender_name) = match &sender {
        Sender::Human => (SenderKind::Human, None, None),
        Sender::System => (SenderKind::System, None, None),
        Sender::Session(id) => {
            let me = MemberRef::session(id.clone());
            if !internal && !channels::is_member(conn, &ch.id, &me)? {
                if is_root_human {
                    // 只有这棵树里的 session 能经发言入群；别的机组走 System（C7）。
                    if crews::root_of(conn, &sessions::crew_of(conn, id)?)? != ch.crew_id {
                        return Err(CollabError::Forbidden(format!(
                            "{id} 不属于 {} 这棵机组树，不能进它的人类群",
                            ch.crew_id
                        )));
                    }
                    // 往根机组人类群发言 = 显式入群（R14：与 add_members 同一个 join；人回话时它才在群里）。
                    channels::join(tx, &ch.id, &me)?;
                    tx.note(Change::ChannelChanged {
                        channel_id: ch.id.clone(),
                    });
                } else {
                    return Err(CollabError::Forbidden(format!(
                        "{id} 不是群 {} 的成员",
                        ch.id
                    )));
                }
            }
            let kind = if sessions::role_of(conn, id)? == Role::Captain {
                SenderKind::Captain
            } else {
                SenderKind::Session
            };
            (
                kind,
                Some(id.clone()),
                Some(sessions::display_name(conn, id)?),
            )
        }
    };
    // @ 解析（裁定 12）
    let mut mentions: Vec<Mention> = Vec::new();
    for m in p.mentions {
        let resolved = match m.kind {
            MentionKind::Captain => {
                let cap = crews::captain_of(conn, &ch.crew_id)?.ok_or_else(|| {
                    CollabError::Invalid(format!("机组 {} 现在没有机长", ch.crew_id))
                })?;
                Mention {
                    kind: MentionKind::Captain,
                    target_id: Some(cap),
                }
            }
            MentionKind::Session => {
                let t = m
                    .target_id
                    .clone()
                    .ok_or_else(|| CollabError::Invalid("@session 要带 targetId".into()))?;
                if !channels::is_member(conn, &ch.id, &MemberRef::session(t.clone()))? {
                    let names: Vec<String> = channels::members(conn, &ch.id)?
                        .into_iter()
                        .filter_map(|x| match x {
                            MemberRef::Session { id } => Some(id),
                            MemberRef::Human => None,
                        })
                        .collect();
                    return Err(CollabError::Invalid(format!(
                        "{t} 不是群 {} 的成员；成员有：{}",
                        ch.id,
                        names.join(", ")
                    )));
                }
                Mention {
                    kind: MentionKind::Session,
                    target_id: Some(t),
                }
            }
            MentionKind::Human | MentionKind::Broadcast => Mention {
                kind: m.kind,
                target_id: None,
            },
        };
        push_mention(&mut mentions, resolved);
    }
    if let Some(rid) = &p.in_reply_to {
        let replied = get(conn, rid)?;
        if replied.channel_id != ch.id {
            return Err(CollabError::Invalid("只能回复同一个群里的消息".into()));
        }
        push_mention(
            &mut mentions,
            Mention {
                kind: MentionKind::Broadcast,
                target_id: None,
            },
        );
        match (replied.sender_kind, replied.sender_session_id) {
            (SenderKind::Human, _) => push_mention(
                &mut mentions,
                Mention {
                    kind: MentionKind::Human,
                    target_id: None,
                },
            ),
            (SenderKind::Session | SenderKind::Captain, Some(sid))
                if sender_session_id.as_deref() != Some(sid.as_str()) =>
            {
                push_mention(
                    &mut mentions,
                    Mention {
                        kind: MentionKind::Session,
                        target_id: Some(sid),
                    },
                )
            }
            _ => {}
        }
    }
    let mut message = Message {
        seq: 0,
        id: uuid::Uuid::new_v4().to_string(),
        channel_id: ch.id.clone(),
        sender_kind,
        sender_session_id: sender_session_id.clone(),
        sender_name,
        text: p.text,
        category: p.category,
        in_reply_to: p.in_reply_to,
        mentions,
        attachments: vec![],
        created_at: tx.now_ts(),
    };
    message.seq = crewd_store::messages::insert(conn, &message, None)?
        .ok_or_else(|| CollabError::Conflict("新消息 id 冲突".into()))?;
    match &sender {
        Sender::Human => cursors::advance(tx, cursors::HUMAN, &ch.id, message.seq)?,
        Sender::Session(id) => sessions::note_output(tx, id)?,
        Sender::System => {}
    }
    let human_mention = message
        .mentions
        .iter()
        .any(|m| m.kind == MentionKind::Human);
    let unwatched_human_mention =
        human_mention && !is_root_human && !channels::is_watched(conn, &ch.id)?;
    tx.note(Change::MessageAppended {
        channel_id: ch.id.clone(),
        seq: message.seq,
        message_id: message.id.clone(),
        sender_session_id,
    });
    tx.note(Change::BadgeChanged { channel_id: ch.id });
    Ok(Posted {
        message,
        unwatched_human_mention,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, err, ok, ok_at, store};

    fn post_text(ch: &str, text: &str) -> NewPost {
        NewPost {
            channel_id: ch.into(),
            text: text.into(),
            ..Default::default()
        }
    }
    fn m(kind: MentionKind, target: Option<&str>) -> Mention {
        Mention {
            kind,
            target_id: target.map(Into::into),
        }
    }
    fn session(id: &str) -> Sender {
        Sender::Session(id.into())
    }

    #[test]
    fn a_session_posts_into_its_lobby_and_the_change_carries_the_sender() {
        let mut s = store();
        let (p, ch) = ok_at(&mut s, NOW, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/lobby", "收到，开工"))
        });
        assert_eq!(p.message.sender_kind, SenderKind::Session);
        assert_eq!(p.message.sender_name.as_deref(), Some("名字-w-a1"));
        assert!(ch.contains(&Change::MessageAppended {
            channel_id: "crew-a/lobby".into(),
            seq: p.message.seq,
            message_id: p.message.id.clone(),
            sender_session_id: Some("w-a1".into()),
        }));
        assert!(ch.contains(&Change::BadgeChanged {
            channel_id: "crew-a/lobby".into()
        }));
        let after = crate::sessions::get(s.conn(), "w-a1").unwrap();
        assert!(after.last_output_at.is_some(), "发言写 last_output_at");
        assert!(
            after.last_progress_at.is_none(),
            "发言不是产出（控制者裁定 C3）：不写 last_progress_at"
        );
        assert!(
            !ch.iter().any(|c| matches!(c, Change::Progress { .. })),
            "发言不发 Progress"
        );
        let cap = ok(&mut s, |tx| {
            post(tx, session("cap-a"), post_text("crew-a/lobby", "好"))
        });
        assert_eq!(cap.message.sender_kind, SenderKind::Captain);
    }

    #[test]
    fn only_members_may_post_except_into_the_root_human_channel() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-b1"),
                post_text("crew-a/lobby", "串门")
            )),
            CollabError::Forbidden(_)
        ));
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-a")
        });
        ok(&mut s, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/human", "要人拍板"))
        });
        assert!(
            crate::channels::is_member(s.conn(), "crew-a/human", &MemberRef::session("w-a1"))
                .unwrap(),
            "发言即入群"
        );
        ok(&mut s, |tx| {
            post(
                tx,
                Sender::Human,
                post_text("crew-b/lobby", "人可以在任何群说话"),
            )
        });
        assert!(
            !crate::channels::is_member(s.conn(), "crew-b/lobby", &MemberRef::session("w-a1"))
                .unwrap()
        );
        ok(&mut s, |tx| {
            post(tx, Sender::System, post_text("crew-x/lobby", "系统通知"))
        });
    }

    #[test]
    fn posting_into_a_not_yet_created_root_human_channel_creates_it_and_joins() {
        // R1：不预先建群。
        let mut s = store();
        assert!(!crate::channels::human_channel_exists(s.conn(), "crew-a").unwrap());
        let (p, ch) = ok_at(&mut s, NOW, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/human", "要人拍板"))
        });
        assert_eq!(p.message.channel_id, "crew-a/human");
        assert!(crate::channels::human_channel_exists(s.conn(), "crew-a").unwrap());
        assert!(ch.contains(&Change::ChannelChanged {
            channel_id: "crew-a/human".into()
        }));
        assert!(
            crate::channels::is_member(s.conn(), "crew-a/human", &MemberRef::session("w-a1"))
                .unwrap(),
            "发言即入群"
        );
        assert_eq!(
            crate::cursors::cursor(s.conn(), "w-a1", "crew-a/human").unwrap(),
            Some(0),
            "入群点钉在发言之前的末尾（自己的发言不是自己的未读）"
        );
        // 非根机组没有人类群：拒，且不留下群。
        assert!(
            crate::run(&mut s, NOW, |tx| post(
                tx,
                session("w-b1"),
                post_text("crew-b/human", "不该成功")
            ))
            .is_err()
        );
        assert!(!crate::channels::human_channel_exists(s.conn(), "crew-b").unwrap());
        // 不存在的机组：拒。
        assert!(
            crate::run(&mut s, NOW, |tx| post(
                tx,
                Sender::Human,
                post_text("nope/human", "?")
            ))
            .is_err()
        );
    }

    #[test]
    fn archived_channels_refuse_messages_and_blank_text_is_refused() {
        let mut s = store();
        let ch = ok(&mut s, |tx| {
            crate::channels::create(
                tx,
                &session("w-a1"),
                crate::channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "临时".into(),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            crate::channels::update(
                tx,
                &session("w-a1"),
                &ch.id,
                crate::channels::ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-a1"),
                post_text(&ch.id, "还在吗")
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-a1"),
                post_text("crew-a/lobby", "  \n")
            )),
            CollabError::Invalid(_)
        ));
        assert!(
            matches!(
                err(&mut s, |tx| post(
                    tx,
                    Sender::Human,
                    NewPost {
                        attachments: vec![Attachment {
                            id: "a".into(),
                            mime: "image/png".into(),
                            size: None,
                            path: "/etc/passwd".into(),
                            filename: None
                        }],
                        ..post_text("crew-a/lobby", "带附件")
                    }
                )),
                CollabError::Invalid(_)
            ),
            "裁定 14：附件上传未实现，不收客户端给的路径"
        );
    }

    #[test]
    fn captain_mention_is_resolved_at_send_time() {
        let mut s = store();
        let before = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    mentions: vec![m(MentionKind::Captain, None)],
                    ..post_text("crew-a/lobby", "@机长 看一下")
                },
            )
        });
        assert_eq!(
            before.message.mentions,
            [m(MentionKind::Captain, Some("cap-a"))]
        );
        ok(&mut s, |tx| crate::crews::set_captain(tx, "crew-a", "w-a2"));
        let after = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    mentions: vec![m(MentionKind::Captain, None)],
                    ..post_text("crew-a/lobby", "@机长 再看一下")
                },
            )
        });
        assert_eq!(
            after.message.mentions,
            [m(MentionKind::Captain, Some("w-a2"))]
        );
        let old = get(s.conn(), &before.message.id).unwrap();
        assert_eq!(
            old.mentions,
            [m(MentionKind::Captain, Some("cap-a"))],
            "换机长不改历史"
        );
    }

    #[test]
    fn session_mentions_must_target_members() {
        let mut s = store();
        let e = err(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    mentions: vec![m(MentionKind::Session, Some("w-b1"))],
                    ..post_text("crew-a/lobby", "@别的机组的人")
                },
            )
        });
        let CollabError::Invalid(why) = e else {
            panic!("{e:?}")
        };
        assert!(why.contains("w-a2"), "错误里列出成员：{why}");
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-a1"),
                NewPost {
                    mentions: vec![m(MentionKind::Session, None)],
                    ..post_text("crew-a/lobby", "@?")
                }
            )),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn reply_to_appends_broadcast_and_the_replied_sender_without_duplicates() {
        let mut s = store();
        let q = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a2"),
                post_text("crew-a/lobby", "这个怎么做？"),
            )
        });
        let r = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    in_reply_to: Some(q.message.id.clone()),
                    mentions: vec![m(MentionKind::Session, Some("w-a2"))],
                    ..post_text("crew-a/lobby", "这样做")
                },
            )
        });
        assert_eq!(
            r.message.mentions,
            [
                m(MentionKind::Session, Some("w-a2")),
                m(MentionKind::Broadcast, None)
            ]
        );
        let h = ok(&mut s, |tx| {
            post(tx, Sender::Human, post_text("crew-a/lobby", "人的问题"))
        });
        let r2 = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    in_reply_to: Some(h.message.id.clone()),
                    ..post_text("crew-a/lobby", "答人")
                },
            )
        });
        assert_eq!(
            r2.message.mentions,
            [m(MentionKind::Broadcast, None), m(MentionKind::Human, None)]
        );
        let other = ok(&mut s, |tx| {
            post(tx, session("cap-x"), post_text("crew-x/lobby", "别处"))
        });
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-a1"),
                NewPost {
                    in_reply_to: Some(other.message.id.clone()),
                    ..post_text("crew-a/lobby", "跨群回复")
                }
            )),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn human_mention_in_an_unwatched_channel_is_flagged() {
        let mut s = store();
        let hm = |text: &str| NewPost {
            mentions: vec![m(MentionKind::Human, None)],
            ..post_text("crew-a/lobby", text)
        };
        assert!(ok(&mut s, |tx| post(tx, session("w-a1"), hm("@人类 a"))).unwatched_human_mention);
        ok(&mut s, |tx| {
            crate::channels::watch(tx, "crew-a/lobby", true)
        });
        assert!(!ok(&mut s, |tx| post(tx, session("w-a1"), hm("@人类 b"))).unwatched_human_mention);
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-a")
        });
        let p = ok(&mut s, |tx| {
            post(
                tx,
                session("cap-a"),
                NewPost {
                    channel_id: "crew-a/human".into(),
                    ..hm("@人类 c")
                },
            )
        });
        assert!(!p.unwatched_human_mention, "根机组人类群本来就是找人的地方");
    }

    #[test]
    fn a_human_post_moves_the_human_cursor() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            post(tx, Sender::Human, post_text("crew-a/lobby", "人说话"))
        });
        assert_eq!(
            crate::cursors::cursor(s.conn(), "human", "crew-a/lobby").unwrap(),
            Some(p.message.seq)
        );
    }

    #[test]
    fn list_pages_and_search_filters() {
        let mut s = store();
        let seqs: Vec<i64> = (0..5)
            .map(|i| {
                ok(&mut s, |tx| {
                    post(
                        tx,
                        session("w-a1"),
                        post_text("crew-a/lobby", &format!("第{i}条 关键词")),
                    )
                })
                .message
                .seq
            })
            .collect();
        ok(&mut s, |tx| {
            post(
                tx,
                session("cap-x"),
                post_text("crew-x/lobby", "别处的关键词"),
            )
        });
        let page = list(s.conn(), "crew-a/lobby", None, Some(seqs[4]), 2).unwrap();
        assert_eq!(
            page.iter().map(|m| m.seq).collect::<Vec<_>>(),
            [seqs[2], seqs[3]]
        );
        let tail = list(s.conn(), "crew-a/lobby", Some(seqs[2]), None, 10).unwrap();
        assert_eq!(tail.len(), 2);
        let all = search(
            s.conn(),
            &Search {
                query: "关键词".into(),
                limit: 50,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].channel_id, "crew-x/lobby", "新的在前");
        let a = search(
            s.conn(),
            &Search {
                query: "关键词".into(),
                crew_id: Some("crew-a".into()),
                limit: 3,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.len(), 3);
        assert!(
            search(
                s.conn(),
                &Search {
                    query: " ".into(),
                    limit: 3,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    // ---- review fixes ----

    #[test]
    fn an_archived_human_channel_of_a_current_root_is_reopened_by_posting() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some()
        );
        // 现根被删：crew-x 重新成了根，它的人类群要一并解档（与 release 同一个做法）。
        ok(&mut s, |tx| crate::crews::delete(tx, "crew-a"));
        assert!(crate::crews::is_root(s.conn(), "crew-x").unwrap());
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_none(),
            "删父后孤儿根的人类群解档"
        );
        ok(&mut s, |tx| {
            post(tx, Sender::Human, post_text("crew-x/human", "人说话"))
        });
        ok(&mut s, |tx| {
            post(tx, session("cap-x"), post_text("crew-x/human", "机长说话"))
        });
    }

    #[test]
    fn posting_reopens_an_archived_human_channel_of_a_root_even_if_nothing_else_did() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        // 只在库里把它标成已归档（模拟任何漏掉解档的路径）。
        s.conn()
            .execute(
                "UPDATE channels SET archived_at='2026-09-01T00:00:00.000Z' WHERE id='crew-x/human'",
                [],
            )
            .unwrap();
        ok(&mut s, |tx| {
            post(tx, Sender::Human, post_text("crew-x/human", "还在吗"))
        });
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_none()
        );
    }

    #[test]
    fn a_non_root_crews_human_channel_is_refused_with_a_pointer_to_the_root() {
        let mut s = store();
        let e = err(&mut s, |tx| {
            post(tx, Sender::Human, post_text("crew-b/human", "发错地方"))
        });
        let CollabError::Invalid(why) = e else {
            panic!("{e:?}")
        };
        assert!(why.contains("crew-a/human"), "{why}");
        assert!(!crate::channels::human_channel_exists(s.conn(), "crew-b").unwrap());
    }

    #[test]
    fn search_matches_non_ascii_exactly() {
        let mut s = store();
        ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                post_text("crew-a/lobby", "Über 文档 Hello"),
            )
        });
        let q = |query: &str| {
            search(
                s.conn(),
                &Search {
                    query: query.into(),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap()
            .len()
        };
        assert_eq!(q("Über"), 1);
        assert_eq!(q("  文档 "), 1);
        assert_eq!(q("hello"), 1, "ASCII 仍不分大小写");
        assert_eq!(q("HELLO"), 1);
    }

    #[test]
    fn search_filters_by_channel_ids_and_time_with_any_offset() {
        let mut s = store();
        let late = NOW + 60 * MIN;
        ok_at(&mut s, NOW, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/lobby", "早 词"))
        });
        ok_at(&mut s, late, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/lobby", "晚 词"))
        });
        ok_at(&mut s, late, |tx| {
            post(tx, session("cap-x"), post_text("crew-x/lobby", "别处 词"))
        });
        let run = |q: Search| -> Vec<String> {
            search(
                s.conn(),
                &Search {
                    query: "词".into(),
                    limit: 50,
                    ..q
                },
            )
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect()
        };
        assert_eq!(
            run(Search {
                channel_ids: Some(vec!["crew-x/lobby".into()]),
                ..Default::default()
            }),
            ["别处 词"]
        );
        assert!(
            run(Search {
                channel_ids: Some(vec![]),
                ..Default::default()
            })
            .is_empty()
        );
        // 中点用 +08:00 写：字符串比较会错，必须解析后比。
        let mid = chrono::DateTime::from_timestamp_millis(NOW + 30 * MIN)
            .unwrap()
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
            .to_rfc3339();
        assert!(mid.ends_with("+08:00"), "{mid}");
        assert_eq!(
            run(Search {
                after: Some(mid.clone()),
                crew_id: Some("crew-a".into()),
                ..Default::default()
            }),
            ["晚 词"]
        );
        assert_eq!(
            run(Search {
                before: Some(mid),
                ..Default::default()
            }),
            ["早 词"]
        );
        // after 含、before 不含（正好落在整点上）。
        let at_late = crewd_store::ts_from_ms(late);
        assert_eq!(
            run(Search {
                after: Some(at_late.clone()),
                ..Default::default()
            })
            .len(),
            2
        );
        assert_eq!(
            run(Search {
                before: Some(at_late),
                ..Default::default()
            }),
            ["早 词"]
        );
        assert!(matches!(
            search(
                s.conn(),
                &Search {
                    query: "词".into(),
                    after: Some("昨天".into()),
                    limit: 5,
                    ..Default::default()
                }
            ),
            Err(CollabError::Invalid(_))
        ));
    }

    #[test]
    fn a_session_may_only_join_its_own_trees_root_human_channel() {
        let mut s = store();
        // 别棵树的 session：拒，且没入群。
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("cap-x"),
                post_text("crew-a/human", "越界")
            )),
            CollabError::Forbidden(_)
        ));
        assert!(
            !crate::channels::is_member(s.conn(), "crew-a/human", &MemberRef::session("cap-x"))
                .unwrap_or(false)
        );
        // 子机组的 session（根是 crew-a）可以。
        ok(&mut s, |tx| {
            post(
                tx,
                session("w-b1"),
                post_text("crew-a/human", "子机组要人拍板"),
            )
        });
        // 跨机组走 System（C7）。
        ok(&mut s, |tx| {
            post(tx, Sender::System, post_text("crew-a/human", "来自 crew-x"))
        });
    }

    #[test]
    fn replying_to_your_own_message_does_not_mention_yourself() {
        let mut s = store();
        let mine = ok(&mut s, |tx| {
            post(tx, session("w-a1"), post_text("crew-a/lobby", "第一句"))
        });
        let r = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    in_reply_to: Some(mine.message.id.clone()),
                    ..post_text("crew-a/lobby", "补一句")
                },
            )
        });
        assert_eq!(r.message.mentions, [m(MentionKind::Broadcast, None)]);
    }

    #[test]
    fn replying_to_a_system_message_only_broadcasts() {
        let mut s = store();
        let sys = ok(&mut s, |tx| {
            post(tx, Sender::System, post_text("crew-a/lobby", "系统公告"))
        });
        let r = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    in_reply_to: Some(sys.message.id.clone()),
                    ..post_text("crew-a/lobby", "收到")
                },
            )
        });
        assert_eq!(r.message.mentions, [m(MentionKind::Broadcast, None)]);
    }

    #[test]
    fn invisible_only_bodies_oversized_bodies_and_reserved_categories_are_refused() {
        let mut s = store();
        for blank in ["\u{200B}", " \u{200C}\u{200D}\n\u{FEFF} "] {
            assert!(
                matches!(
                    err(&mut s, |tx| post(
                        tx,
                        session("w-a1"),
                        post_text("crew-a/lobby", blank)
                    )),
                    CollabError::Invalid(_)
                ),
                "{blank:?}"
            );
        }
        ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                post_text("crew-a/lobby", &"a".repeat(256 * 1024)),
            )
        });
        let e = err(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                post_text("crew-a/lobby", &"a".repeat(256 * 1024 + 1)),
            )
        });
        let CollabError::Invalid(why) = e else {
            panic!("{e:?}")
        };
        assert!(why.contains("262144"), "错误里写明上限：{why}");
        for cat in ["ask", "approval", "restart"] {
            assert!(
                matches!(
                    err(&mut s, |tx| post(
                        tx,
                        Sender::Human,
                        NewPost {
                            category: Some(cat.into()),
                            ..post_text("crew-a/lobby", "冒充公告")
                        }
                    )),
                    CollabError::Invalid(_)
                ),
                "{cat}"
            );
        }
        ok(&mut s, |tx| {
            post(
                tx,
                Sender::Human,
                NewPost {
                    category: Some("note".into()),
                    ..post_text("crew-a/lobby", "普通分类")
                },
            )
        });
        // 公告路径可以用保留分类。
        let a = ok(&mut s, |tx| {
            post_internal(
                tx,
                Sender::System,
                NewPost {
                    category: Some("ask".into()),
                    ..post_text("crew-a/lobby", "公告")
                },
            )
        });
        assert_eq!(a.message.category.as_deref(), Some("ask"));
    }

    #[test]
    fn captain_mention_with_no_captain_is_refused() {
        let mut s = store();
        s.conn()
            .execute(
                "UPDATE crews SET captain_session_id=NULL WHERE id='crew-a'",
                [],
            )
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| post(
                tx,
                session("w-a1"),
                NewPost {
                    mentions: vec![m(MentionKind::Captain, None)],
                    ..post_text("crew-a/lobby", "@机长")
                }
            )),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn a_human_posting_into_a_session_dm_leaves_membership_alone() {
        let mut s = store();
        let (dm, _) = ok(&mut s, |tx| {
            crate::channels::ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("w-a2"),
            )
        });
        let before = crate::channels::members(s.conn(), &dm.id).unwrap();
        ok(&mut s, |tx| {
            post(tx, Sender::Human, post_text(&dm.id, "人插一句"))
        });
        assert_eq!(crate::channels::members(s.conn(), &dm.id).unwrap(), before);
        assert!(
            !crate::channels::is_member(s.conn(), &dm.id, &MemberRef::Human).unwrap(),
            "人没有因此入群"
        );
    }
}
