//! 给 agent 看的渲染：注入块（hook / codex additionalContext）与唤醒正文（spec §22.2、§22.5）。
//! 文字只来自 prompts/render.*.md；本文件（测试以外）不含任何自然语言字面量（测试钉住）。
use crate::prompts::{fill, render_file};
use crate::wakes::{WakeKind, WakeRow};
use crate::{Locale, Result, asks, channels, crews, cursors, sessions};
use crewd_proto::{AskStateView, ChannelKind, Message, SenderKind};
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgLine {
    pub seq: i64,
    pub at: String,
    pub sender: String,
    pub text: String,
    /// 看得见但不是给我的：目标的显示名。
    pub directed_to: Option<Vec<String>>,
    /// 回复的是谁（显示名）。
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBlock {
    pub channel_id: String,
    pub kind: ChannelKind,
    pub name: String,
    pub crew_title: String,
    pub lines: Vec<MsgLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeBody {
    Messages(Vec<ChannelBlock>),
    Timer,
    AskAnswered {
        ask: String,
        question: String,
        answer: String,
        resume_note: Option<String>,
    },
    AskDefaulted {
        ask: String,
        question: String,
        answer: String,
        resume_note: Option<String>,
    },
    /// 审批有了结果（控制者裁定 C2；队列行 kind = ask_answer、source = `approval:<id>`）。
    Approval {
        tool: String,
        summary: String,
        allow: bool,
    },
    Restart,
    System,
    Checkpoint,
    StopCheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    /// 合并进这一项的队列行（消息类会把多行合成一项）。
    pub wake_ids: Vec<i64>,
    pub kind: WakeKind,
    pub note: Option<String>,
    pub body: WakeBody,
}

/// 一次出队的批。传进来的每一行恰好落在三处之一：某个 `items[..].wake_ids`、`unusable`、`deferred`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBatch {
    pub session_id: String,
    pub items: Vec<WakeItem>,
    /// 渲染不出来的行（消息已删、问已撤回……）与原因：M3c 记 `mark_dropped`。
    pub unusable: Vec<(i64, String)>,
    /// 消息行里、消息落在这批窗口之外的（R6：一个群一批最多带 `UNREAD_CAP` 条，取最旧的）：
    /// M3c **不动它们**（仍是 pending），投递成功并按 `cursor_targets` 推游标后，下一批紧接着带上。
    pub deferred: Vec<i64>,
}

impl WakeBatch {
    /// 投递成功后每个群的游标该推到哪：这批**实际带上**的最大 seq（R6；不是群末尾，也不是队列里最大的 seq）。
    /// 窗口是连续的（游标之后到这个 seq 为止它看得见的消息全在批里），推到这里不会越过任何没告知过的消息。
    pub fn cursor_targets(&self) -> Vec<(String, i64)> {
        let mut out: Vec<(String, i64)> = Vec::new();
        for it in &self.items {
            let WakeBody::Messages(blocks) = &it.body else {
                continue;
            };
            for b in blocks {
                if let Some(max) = b.lines.iter().map(|l| l.seq).max() {
                    out.push((b.channel_id.clone(), max));
                }
            }
        }
        out
    }
}

/// 队列行 source 的前缀（拼法见 `wakes::source`）。
const SRC_MESSAGE: &str = "msg:";
const SRC_ASK: &str = "ask:";
const SRC_APPROVAL: &str = "approval:";

fn sender_name(conn: &Connection, l: Locale, m: &Message) -> Result<String> {
    let f = render_file(l);
    Ok(match m.sender_kind {
        SenderKind::Human => f.get("sender.human").to_string(),
        SenderKind::System => f.get("sender.system").to_string(),
        SenderKind::Session | SenderKind::Captain => match (&m.sender_name, &m.sender_session_id) {
            (Some(n), _) => n.clone(),
            (None, Some(id)) => sessions::display_name(conn, id).unwrap_or_else(|_| id.clone()),
            (None, None) => String::new(),
        },
    })
}

/// 未读（或唤醒带的消息）→ 按群分块，标出「发给别人的」与回复对象。
/// `directed_to`：`None` = 不标；`Some([])` = 收窄型 mention 在场但说不出冲谁（仍标「不是发给你的」）；
/// `Some(names)` = 发给这些人的（`cursors::directed_targets`）。
pub fn blocks_for(
    conn: &Connection,
    session: &str,
    unread: &[(String, Vec<Message>)],
) -> Result<Vec<ChannelBlock>> {
    let l = crate::settings::locale(conn)?;
    let mut out = Vec::new();
    for (ch_id, msgs) in unread {
        if msgs.is_empty() {
            continue;
        }
        let ch = channels::get(conn, ch_id)?;
        let crew_title = crews::get(conn, &ch.crew_id)?.title;
        let mut lines = Vec::new();
        for m in msgs {
            let directed_to = cursors::directed_targets(m, session).map(|ids| {
                ids.iter()
                    .map(|i| sessions::display_name(conn, i).unwrap_or_else(|_| i.clone()))
                    .collect()
            });
            let reply_to = match &m.in_reply_to {
                None => None,
                Some(rid) => match crewd_store::messages::get(conn, rid)? {
                    Some(r) => Some(sender_name(conn, l, &r)?),
                    None => None,
                },
            };
            lines.push(MsgLine {
                seq: m.seq,
                at: m.created_at.clone(),
                sender: sender_name(conn, l, m)?,
                text: m.text.clone(),
                directed_to,
                reply_to,
            });
        }
        out.push(ChannelBlock {
            channel_id: ch.id,
            kind: ch.kind,
            name: ch.name,
            crew_title,
            lines,
        });
    }
    Ok(out)
}

fn block_text(l: Locale, b: &ChannelBlock) -> String {
    let f = render_file(l);
    let head = match b.kind {
        ChannelKind::Lobby => fill(f.get("inject.channel.lobby"), &[("crew", &b.crew_title)]),
        ChannelKind::Human => fill(f.get("inject.channel.human"), &[("crew", &b.crew_title)]),
        ChannelKind::Topic => fill(f.get("inject.channel.topic"), &[("name", &b.name)]),
        ChannelKind::Dm => f.get("inject.channel.dm").to_string(),
    };
    let mut out = head;
    for ln in &b.lines {
        out.push('\n');
        match &ln.directed_to {
            None => {}
            Some(t) if t.is_empty() => out.push_str(f.get("line.directed.unknown")),
            Some(t) => out.push_str(&fill(f.get("line.directed"), &[("targets", &t.join(", "))])),
        }
        if let Some(r) = &ln.reply_to {
            out.push_str(&fill(f.get("line.reply"), &[("reply", r)]));
        }
        out.push_str(&fill(
            f.get("line"),
            &[("at", &ln.at), ("sender", &ln.sender), ("text", &ln.text)],
        ));
    }
    out
}

/// 注入块。没有消息 = 空串（调用方据此不注入）。
///
/// 窗口规则（R6）由上游 `cursors::unread_for` 保证：每个群最旧的 `UNREAD_CAP` 条；注入成功后调用方只把游标推到
/// 各块里实际带上的最大 seq。
pub fn injection(l: Locale, blocks: &[ChannelBlock]) -> String {
    let blocks: Vec<&ChannelBlock> = blocks.iter().filter(|b| !b.lines.is_empty()).collect();
    if blocks.is_empty() {
        return String::new();
    }
    let mut out = render_file(l).get("inject.header").to_string();
    for b in blocks {
        out.push_str("\n\n");
        out.push_str(&block_text(l, b));
    }
    out
}

/// 一次投递的唤醒正文（M3c 合并同一档优先级后调用）。
pub fn wake_text(l: Locale, batch: &WakeBatch) -> String {
    let f = render_file(l);
    let mut out = f.get("wake.header").to_string();
    for it in &batch.items {
        out.push_str("\n\n");
        match &it.body {
            WakeBody::Messages(blocks) => {
                out.push_str(f.get("wake.message"));
                for b in blocks {
                    out.push_str("\n\n");
                    out.push_str(&block_text(l, b));
                }
            }
            WakeBody::AskAnswered {
                ask,
                question,
                answer,
                resume_note,
            }
            | WakeBody::AskDefaulted {
                ask,
                question,
                answer,
                resume_note,
            } => {
                let key = if matches!(it.body, WakeBody::AskAnswered { .. }) {
                    "wake.ask_answer"
                } else {
                    "wake.ask_defaulted"
                };
                out.push_str(&fill(
                    f.get(key),
                    &[("ask", ask), ("question", question), ("answer", answer)],
                ));
                if let Some(n) = resume_note {
                    out.push('\n');
                    out.push_str(&fill(f.get("wake.resume_note"), &[("note", n)]));
                }
            }
            WakeBody::Approval {
                tool,
                summary,
                allow,
            } => {
                let key = if *allow {
                    "wake.approval_allowed"
                } else {
                    "wake.approval_denied"
                };
                out.push_str(&fill(f.get(key), &[("tool", tool), ("summary", summary)]));
            }
            WakeBody::Timer => out.push_str(f.get("wake.timer")),
            WakeBody::Restart => out.push_str(f.get("wake.restart")),
            WakeBody::System => out.push_str(f.get("wake.system")),
            WakeBody::Checkpoint => out.push_str(f.get("wake.checkpoint")),
            WakeBody::StopCheck => out.push_str(f.get("wake.stop_check")),
        }
        if let Some(n) = &it.note {
            out.push('\n');
            out.push_str(&fill(f.get("wake.note"), &[("note", n)]));
        }
    }
    out
}

/// 一个群这批带哪些消息（R6 的窗口规则，与 `cursors::unread_for` 同一条）：从 min(游标, 第一条被叫的消息之前)
/// 开始按 seq 升序**翻页**扫（一整页都看不见时不停），收「它看得见、不是它自己发的」与「叫醒它的」消息，
/// 直到收满最旧的 `UNREAD_CAP` 条或扫过被叫的最大 seq。返回 (消息, 窗口末尾 seq)：被叫的消息 seq ≤ 末尾的都在里面，
/// 末尾之后的留到下一批。窗口连续，所以投递后把游标推到末尾不越过任何没告知过的消息。
fn batch_window(
    conn: &Connection,
    session: &str,
    ch: &str,
    queued: &[i64],
) -> Result<(Vec<Message>, i64)> {
    let (Some(&min), Some(&max)) = (queued.iter().min(), queued.iter().max()) else {
        return Ok((Vec::new(), 0));
    };
    let cap = cursors::UNREAD_CAP as usize;
    let mut after = cursors::cursor(conn, session, ch)?
        .unwrap_or(min - 1)
        .min(min - 1);
    let mut got: Vec<Message> = Vec::new();
    'scan: loop {
        let page = crewd_store::messages::list(conn, ch, after, None, cursors::UNREAD_CAP)?;
        let Some(last) = page.last().map(|m| m.seq) else {
            break;
        };
        let full = page.len() >= cap;
        for m in page {
            if m.seq > max {
                break 'scan;
            }
            let deliverable =
                m.sender_session_id.as_deref() != Some(session) && cursors::visible_to(&m, session);
            if deliverable || queued.contains(&m.seq) {
                got.push(m);
                if got.len() >= cap {
                    break 'scan;
                }
            }
        }
        if !full {
            break;
        }
        after = last;
    }
    let end = if got.len() >= cap {
        got.last().map(|m| m.seq).unwrap_or(max)
    } else {
        max
    };
    Ok((got, end))
}

/// 队列行 → 可渲染的批（出队时现渲染，spec §22.4）。行的顺序即输出顺序（`due_for` 已按优先级排好）；
/// 消息类合成一项，放在第一条消息行出现的位置。渲染不了的行进 `unusable`。
///
/// 消息项里每个群带「它的游标之后、到批里最大 seq 为止」它看得见的全部消息（不止叫它的那几条）：M3c 投递成功后
/// 按 `cursor_targets` 推游标，中间没叫它的消息就随这次唤醒告知过了，不会被越过而丢（M3c 计划 spec 问题 12）。
/// 窗口规则同 `cursors::unread_for`（R6）：一个群一批最多带最旧的 `UNREAD_CAP` 条；被叫的消息落在窗口之外的
/// 那些行进 `deferred`（留在队列里，下一批带）。游标远远落后时，窗口可能整段都是比被叫消息更旧的未读——
/// 这时消息项的 `wake_ids` 可以是空的，它仍要投（投完游标前进，下一批就轮到被叫的那几条）。
pub fn build_batch(conn: &Connection, session: &str, rows: &[WakeRow]) -> Result<WakeBatch> {
    let mut items: Vec<WakeItem> = Vec::new();
    let mut unusable = Vec::new();
    let mut msg_slot: Option<usize> = None;
    // 消息行：(队列行 id, 群, seq)，按行的顺序。
    let mut queued: Vec<(i64, String, i64)> = Vec::new();
    for r in rows {
        match r.kind {
            WakeKind::Message => {
                let Some(id) = r.source.strip_prefix(SRC_MESSAGE) else {
                    unusable.push((r.id, format!("source {:?} is not a message", r.source)));
                    continue;
                };
                let Some(m) = crewd_store::messages::get(conn, id)? else {
                    unusable.push((r.id, format!("message {id} no longer exists")));
                    continue;
                };
                queued.push((r.id, m.channel_id, m.seq));
                if msg_slot.is_none() {
                    msg_slot = Some(items.len());
                    items.push(WakeItem {
                        wake_ids: Vec::new(),
                        kind: WakeKind::Message,
                        note: None,
                        body: WakeBody::Messages(Vec::new()),
                    });
                }
            }
            WakeKind::AskAnswer if r.source.starts_with(SRC_APPROVAL) => {
                let id = &r.source[SRC_APPROVAL.len()..];
                match crate::approvals::get(conn, id) {
                    Ok(a) if a.decision.is_some() => {
                        let allow = a.decision == Some(crewd_proto::Decision::Allow);
                        items.push(WakeItem {
                            wake_ids: vec![r.id],
                            kind: r.kind,
                            note: r.note.clone(),
                            body: WakeBody::Approval {
                                tool: a.tool_name,
                                summary: a.summary,
                                allow,
                            },
                        });
                    }
                    Ok(_) => unusable.push((r.id, format!("approval {id} has no decision"))),
                    Err(e) => unusable.push((r.id, e.to_string())),
                }
            }
            WakeKind::AskAnswer => {
                let parsed = r
                    .source
                    .strip_prefix(SRC_ASK)
                    .and_then(|s| s.rsplit_once('/'));
                let Some((crew, ask)) = parsed else {
                    unusable.push((r.id, format!("source {:?} is not an ask", r.source)));
                    continue;
                };
                let a = match asks::get(conn, crew, ask) {
                    Ok(a) => a,
                    Err(e) => {
                        unusable.push((r.id, e.to_string()));
                        continue;
                    }
                };
                let answer = a.answer.clone().unwrap_or_default();
                let body = match a.state {
                    AskStateView::Answered => WakeBody::AskAnswered {
                        ask: a.ask,
                        question: a.question,
                        answer,
                        resume_note: a.resume_note,
                    },
                    AskStateView::Defaulted => WakeBody::AskDefaulted {
                        ask: a.ask,
                        question: a.question,
                        answer,
                        resume_note: a.resume_note,
                    },
                    other => {
                        unusable
                            .push((r.id, format!("{} is {other}, no answer to deliver", a.ask)));
                        continue;
                    }
                };
                items.push(WakeItem {
                    wake_ids: vec![r.id],
                    kind: r.kind,
                    note: r.note.clone(),
                    body,
                });
            }
            WakeKind::Timer => items.push(WakeItem {
                wake_ids: vec![r.id],
                kind: r.kind,
                note: r.note.clone(),
                body: WakeBody::Timer,
            }),
            WakeKind::Restart => items.push(WakeItem {
                wake_ids: vec![r.id],
                kind: r.kind,
                note: r.note.clone(),
                body: WakeBody::Restart,
            }),
            WakeKind::System => items.push(WakeItem {
                wake_ids: vec![r.id],
                kind: r.kind,
                note: r.note.clone(),
                body: WakeBody::System,
            }),
            WakeKind::Checkpoint => items.push(WakeItem {
                wake_ids: vec![r.id],
                kind: r.kind,
                note: r.note.clone(),
                body: WakeBody::Checkpoint,
            }),
            WakeKind::StopCheck => items.push(WakeItem {
                wake_ids: vec![r.id],
                kind: r.kind,
                note: r.note.clone(),
                body: WakeBody::StopCheck,
            }),
        }
    }
    let mut deferred = Vec::new();
    if let Some(i) = msg_slot {
        // 群按第一条消息行出现的先后。
        let mut chans: Vec<String> = Vec::new();
        for (_, ch, _) in &queued {
            if !chans.contains(ch) {
                chans.push(ch.clone());
            }
        }
        let mut msgs: Vec<(String, Vec<Message>)> = Vec::new();
        let mut ends: Vec<(String, i64)> = Vec::new();
        for ch in chans {
            let seqs: Vec<i64> = queued
                .iter()
                .filter(|(_, c, _)| *c == ch)
                .map(|(_, _, s)| *s)
                .collect();
            let (window, end) = batch_window(conn, session, &ch, &seqs)?;
            ends.push((ch.clone(), end));
            msgs.push((ch, window));
        }
        for (row, ch, seq) in &queued {
            let end = ends
                .iter()
                .find(|(c, _)| c == ch)
                .map(|(_, e)| *e)
                .unwrap_or(0);
            if *seq <= end {
                items[i].wake_ids.push(*row);
            } else {
                deferred.push(*row);
            }
        }
        items[i].body = WakeBody::Messages(blocks_for(conn, session, &msgs)?);
    }
    Ok(WakeBatch {
        session_id: session.into(),
        items,
        unusable,
        deferred,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sender;
    use crate::messages::{NewPost, post};
    use crate::testkit::{NOW, msg, ok, store};
    use crate::wakes::{Enqueued, WakePriority, WakeRequest, enqueue, list};
    use crewd_proto::{Mention, MentionKind};

    fn say(
        s: &mut crewd_store::Store,
        who: Sender,
        ch: &str,
        text: &str,
        mentions: Vec<Mention>,
        reply: Option<String>,
    ) -> Message {
        ok(s, |tx| {
            post(
                tx,
                who,
                NewPost {
                    channel_id: ch.into(),
                    text: text.into(),
                    mentions,
                    in_reply_to: reply,
                    ..Default::default()
                },
            )
        })
        .message
    }

    #[test]
    fn injection_names_channels_senders_and_marks_messages_meant_for_others() {
        let mut s = store();
        let q = say(
            &mut s,
            Sender::Human,
            "crew-a/lobby",
            "今天发版吗？",
            vec![],
            None,
        );
        say(
            &mut s,
            Sender::Session("w-a1".into()),
            "crew-a/lobby",
            "我来查",
            vec![
                Mention {
                    kind: MentionKind::Broadcast,
                    target_id: None,
                },
                Mention {
                    kind: MentionKind::Session,
                    target_id: Some("w-a2".into()),
                },
            ],
            Some(q.id.clone()),
        );
        let unread = ok(&mut s, |tx| crate::cursors::unread_for(tx, "cap-a"));
        let blocks = blocks_for(s.conn(), "cap-a", &unread).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].crew_title, "机组甲");
        assert_eq!(blocks[0].lines[0].sender, "人类");
        assert_eq!(
            blocks[0].lines[1].directed_to,
            Some(vec!["名字-w-a2".to_string()])
        );
        assert_eq!(blocks[0].lines[1].reply_to.as_deref(), Some("人类"));
        let text = injection(Locale::Zh, &blocks);
        assert!(text.contains("【机组甲 · 大厅】"), "{text}");
        assert!(text.contains("人类：今天发版吗？"), "{text}");
        assert!(
            text.contains("（发给 名字-w-a2 的，不是派给你的）"),
            "{text}"
        );
        assert!(injection(Locale::En, &blocks).contains("[机组甲 · lobby]"));
        assert_eq!(injection(Locale::Zh, &[]), "", "没有未读 = 不注入");
    }

    #[test]
    fn build_batch_merges_messages_and_resolves_ask_answers() {
        let mut s = store();
        let m1 = say(
            &mut s,
            Sender::Session("w-a2".into()),
            "crew-a/lobby",
            "第一条",
            vec![],
            None,
        );
        say(
            &mut s,
            Sender::Human,
            "crew-a/lobby",
            "夹在中间、没叫它的一条",
            vec![],
            None,
        );
        let m2 = say(
            &mut s,
            Sender::Session("w-a2".into()),
            "crew-a/lobby",
            "第二条",
            vec![],
            None,
        );
        let a = ok(&mut s, |tx| {
            crate::asks::open(
                tx,
                "w-a1",
                crate::asks::OpenAsk {
                    question: "用哪家？".into(),
                    resume_note: Some("接着接入".into()),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            crate::asks::answer(tx, "crew-a", &a.ask, "用甲")
        });
        ok(&mut s, |tx| {
            for m in [&m1, &m2] {
                enqueue(
                    tx,
                    WakeRequest {
                        session: "w-a1".into(),
                        kind: WakeKind::Message,
                        source: crate::wakes::source::message(&m.id),
                        priority: WakePriority::Auto,
                        not_before_ms: None,
                        note: None,
                        origin: Some("w-a2".into()),
                    },
                )?;
            }
            let r: Result<Enqueued> = enqueue(
                tx,
                WakeRequest {
                    session: "w-a1".into(),
                    kind: WakeKind::Message,
                    source: crate::wakes::source::message("gone"),
                    priority: WakePriority::Auto,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
            );
            r
        });
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let batch = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(batch.unusable.len(), 1, "消息不存在的那一行");
        let kinds: Vec<WakeKind> = batch.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [WakeKind::AskAnswer, WakeKind::Message],
            "人回答的优先级高，排前面"
        );
        assert_eq!(batch.items[1].wake_ids.len(), 2, "两条消息合成一项");
        let text = wake_text(Locale::Zh, &batch);
        assert!(
            text.contains("答复：用甲") && text.contains("接续说明：接着接入"),
            "{text}"
        );
        assert!(text.contains("第一条") && text.contains("第二条"), "{text}");
        assert!(
            text.contains("夹在中间"),
            "游标之后到批里最大 seq 的可见消息都带上（M3c 推游标不丢消息）：{text}"
        );
        assert_eq!(list(s.conn(), Some("w-a1"), 10).unwrap().len(), 4);
    }

    #[test]
    fn approval_decision_wakes_with_retry_once_or_no() {
        let mut s = store();
        let a = match ok(&mut s, |tx| {
            crate::approvals::gate(
                tx,
                crate::approvals::NewApproval {
                    session_id: "w-a1".into(),
                    tool_name: "computer-use".into(),
                    summary: "点设置".into(),
                    tool_use_id: None,
                },
            )
        }) {
            crate::approvals::Gate::DenyFiled(a) => a,
            g => panic!("{g:?}"),
        };
        ok(&mut s, |tx| {
            crate::approvals::decide(tx, &a.id, true, "human")
        });
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let batch = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert!(batch.unusable.is_empty());
        let text = wake_text(Locale::Zh, &batch);
        assert!(
            text.contains("computer-use") && text.contains("只放行一次"),
            "{text}"
        );
    }

    #[test]
    fn render_code_has_no_prose() {
        let src = include_str!("render.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        let prose: Vec<char> = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .flat_map(|l| l.chars())
            .filter(|c| {
                ('\u{4e00}'..='\u{9fff}').contains(c)
                    || ('\u{3000}'..='\u{303f}').contains(c)
                    || ('\u{ff00}'..='\u{ffef}').contains(c)
            })
            .collect();
        assert!(
            prose.is_empty(),
            "render.rs 代码里出现了中文字面量（应放进 prompts/render.*.md）：{prose:?}"
        );
    }

    // ---- 以下为本任务补的测试（控制者对 Task 11 的追加要求：窗口翻页、R6 窗口、未知目标的标注、拒绝的审批） ----

    fn wake_msg(s: &mut crewd_store::Store, session: &str, message_id: &str) -> i64 {
        match ok(s, |tx| {
            enqueue(
                tx,
                WakeRequest {
                    session: session.into(),
                    kind: WakeKind::Message,
                    source: crate::wakes::source::message(message_id),
                    priority: WakePriority::Auto,
                    not_before_ms: None,
                    note: None,
                    origin: Some("w-a2".into()),
                },
            )
        }) {
            Enqueued::New(id) => id,
            d => panic!("{d:?}"),
        }
    }

    fn line_seqs(batch: &WakeBatch) -> Vec<i64> {
        batch
            .items
            .iter()
            .filter_map(|i| match &i.body {
                WakeBody::Messages(bs) => Some(
                    bs.iter()
                        .flat_map(|b| b.lines.iter().map(|l| l.seq))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 批的窗口要翻页：一整页都是看不见的消息时不能停（R6 + Task 9 的同一种「先截断再过滤」病）。
    /// 游标 → 看得见的 v1 → 250 条发给别人的 → 看得见的 v2 → 250 条发给别人的 → 叫醒它的 q。
    /// 只取前 200 条再过滤会丢 v2；取 q 之前最新的 200 条会丢 v1 与 v2。
    #[test]
    fn build_batch_window_pages_past_hidden_messages() {
        let mut s = store();
        use crewd_proto::SenderKind as K;
        let to_cap = || {
            vec![Mention {
                kind: MentionKind::Session,
                target_id: Some("cap-a".into()),
            }]
        };
        assert_eq!(
            crate::cursors::cursor(s.conn(), "w-a1", "crew-a/lobby").unwrap(),
            Some(0),
            "前提：w-a1 的游标钉在空大厅的末尾"
        );
        let v1 = msg(&s, "crew-a/lobby", "v1", K::Session, Some("w-a2"), vec![]);
        for i in 0..250 {
            msg(
                &s,
                "crew-a/lobby",
                &format!("h1-{i}"),
                K::Session,
                Some("w-a2"),
                to_cap(),
            );
        }
        let v2 = msg(&s, "crew-a/lobby", "v2", K::Session, Some("w-a2"), vec![]);
        for i in 0..250 {
            msg(
                &s,
                "crew-a/lobby",
                &format!("h2-{i}"),
                K::Session,
                Some("w-a2"),
                to_cap(),
            );
        }
        let q = msg(
            &s,
            "crew-a/lobby",
            "q",
            K::Session,
            Some("w-a2"),
            vec![Mention {
                kind: MentionKind::Session,
                target_id: Some("w-a1".into()),
            }],
        );
        let row = wake_msg(&mut s, "w-a1", "q");
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let batch = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(
            line_seqs(&batch),
            [v1, v2, q],
            "游标之后到 q 为止看得见的全带上，看不见的一条不带"
        );
        assert_eq!(batch.items[0].wake_ids, [row]);
        assert!(batch.deferred.is_empty());
        assert_eq!(batch.cursor_targets(), [("crew-a/lobby".to_string(), q)]);
    }

    /// R6：批与 `unread_for` 同一条窗口规则——取最旧的 `UNREAD_CAP` 条；窗口之外的消息行留在队列里
    /// （`deferred`），M3c 把游标推到这批实际带上的最大 seq 之后，下一批紧接着开始，一条不丢、一条不重。
    #[test]
    fn build_batch_takes_the_oldest_window_and_defers_rows_beyond_it() {
        let mut s = store();
        let mut seqs = Vec::new();
        for i in 0..250 {
            seqs.push(msg(
                &s,
                "crew-a/lobby",
                &format!("m{i}"),
                SenderKind::Session,
                Some("w-a2"),
                vec![],
            ));
        }
        let early = wake_msg(&mut s, "w-a1", "m9");
        let late = wake_msg(&mut s, "w-a1", "m249");
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let b1 = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(line_seqs(&b1), seqs[..200], "最旧的 200 条");
        assert_eq!(b1.items[0].wake_ids, [early], "窗口里的那一行随这批投");
        assert_eq!(b1.deferred, [late], "窗口外的那一行留到下一批");
        assert!(b1.unusable.is_empty());
        assert_eq!(
            b1.cursor_targets(),
            [("crew-a/lobby".to_string(), seqs[199])]
        );
        ok(&mut s, |tx| {
            crate::wakes::mark_delivered(tx, &b1.items[0].wake_ids)?;
            crate::cursors::advance(tx, "w-a1", "crew-a/lobby", seqs[199])
        });
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let b2 = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(line_seqs(&b2), seqs[200..], "下一批从紧接着的那条开始");
        assert_eq!(b2.items[0].wake_ids, [late]);
        assert!(b2.deferred.is_empty());
    }

    /// 收窄型 mention 在场、但一个目标都解不出（`directed_targets` = `Some([])`）：仍标「不是发给你的」（同 Swift `directedNote`）。
    #[test]
    fn a_directed_message_with_no_resolvable_target_is_still_marked() {
        let mut s = store();
        msg(
            &s,
            "crew-a/lobby",
            "u1",
            SenderKind::Session,
            Some("w-a2"),
            vec![
                Mention {
                    kind: MentionKind::Broadcast,
                    target_id: None,
                },
                Mention {
                    kind: MentionKind::Session,
                    target_id: None,
                },
            ],
        );
        let unread = ok(&mut s, |tx| crate::cursors::unread_for(tx, "cap-a"));
        let blocks = blocks_for(s.conn(), "cap-a", &unread).unwrap();
        assert_eq!(blocks[0].lines[0].directed_to, Some(vec![]));
        let zh = injection(Locale::Zh, &blocks);
        assert!(
            zh.contains("（不是发给你的）") && !zh.contains("发给  的"),
            "{zh}"
        );
        assert!(injection(Locale::En, &blocks).contains("(not addressed to you)"));
    }

    #[test]
    fn a_denied_approval_wakes_with_no() {
        let mut s = store();
        let a = match ok(&mut s, |tx| {
            crate::approvals::gate(
                tx,
                crate::approvals::NewApproval {
                    session_id: "w-a1".into(),
                    tool_name: "computer-use".into(),
                    summary: "点设置".into(),
                    tool_use_id: None,
                },
            )
        }) {
            crate::approvals::Gate::DenyFiled(a) => a,
            g => panic!("{g:?}"),
        };
        ok(&mut s, |tx| {
            crate::approvals::decide(tx, &a.id, false, "human")
        });
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let batch = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(batch.items.len(), 1);
        assert!(matches!(
            &batch.items[0].body,
            WakeBody::Approval { allow: false, .. }
        ));
        let text = wake_text(Locale::Zh, &batch);
        assert!(
            text.contains("人没有同意你用 computer-use（点设置）") && !text.contains("只放行一次"),
            "{text}"
        );
    }
}
