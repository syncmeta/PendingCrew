//! 人类待确认（H 号）：经 crewd-work 的工作账本发号（§19 Q1：工作账本是唯一发号处）。
use crate::messages;
use crate::wakes::{self, WakeKind, WakePriority, WakeRequest};
use crate::{Change, CollabError, Result, Sender, Tx, crews, sessions};
use crewd_proto::{Ask, AskDefault, AskStateView, HumanWait, WaitTierView};
use crewd_work::{
    Actor, AskId, AskState, BookState, Config, CrewId, DefaultOption, EventKind, HumanAsk,
    Rejected, Ts, WaitTier, WorkBook,
};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenAsk {
    pub question: String,
    /// 「X 分钟没回复就按 A 办」（§6；到点后照常走审批，§13 Q2）。
    pub default: Option<AskDefault>,
    /// 回答后叫醒提问者时带上的「接着做什么」。
    pub resume_note: Option<String>,
}

fn ts(t: Ts) -> String {
    crewd_store::ts_from_ms(t.0)
}

fn load(conn: &Connection, crew: &str) -> Result<WorkBook> {
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM work_books WHERE crew_id=?1",
            [crew],
            |r| r.get(0),
        )
        .optional()?;
    match state {
        None => Ok(WorkBook::new(CrewId::new(crew), Config::default())),
        Some(s) => {
            let st: BookState = serde_json::from_str(&s)?;
            WorkBook::restore(st, Config::default())
                .map_err(|v| CollabError::Invalid(format!("机组 {crew} 的工作账本读回不合法：{v}")))
        }
    }
}

/// 账本拒绝 → collab 的错误。号码格式对、账本里没有（H / W / #）= NotFound（R11：-32004，不是 -32602）；
/// 其余原样带着 `Rejected`（服务层按种类映射）。
fn rejected(crew: &str, r: Rejected) -> CollabError {
    match r {
        Rejected::UnknownAsk(h) => CollabError::NotFound(format!("机组 {crew} 没有 {h}")),
        Rejected::UnknownItem(w) => CollabError::NotFound(format!("机组 {crew} 没有 {w}")),
        Rejected::UnknownEntry(e) => CollabError::NotFound(format!("机组 {crew} 没有 {e}")),
        other => CollabError::Work(other),
    }
}

/// 在同一个事务里读整本账、跑一条命令、整份写回（M4a 的持久化边界）。M4b 的工作项命令也走这里。
/// 闭包返回账本自己的 `Result<R, Rejected>`；这里把 `Rejected` 映射成 collab 的错误（R5，见 `rejected`）。
/// 被拒 = 不写回（账本原样）。事件日志此时丢弃（M4b 加 v4 的事件表再落）。
pub fn with_book<R>(
    tx: &mut Tx<'_>,
    crew: &str,
    f: impl FnOnce(&mut WorkBook, Ts) -> std::result::Result<R, Rejected>,
) -> Result<R> {
    crews::get(tx.conn(), crew)?;
    let mut book = load(tx.conn(), crew)?;
    let r = f(&mut book, Ts(tx.now_ms())).map_err(|e| rejected(crew, e))?;
    let _ = book.take_log();
    tx.conn().execute(
        "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,?3)
         ON CONFLICT(crew_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at",
        params![crew, serde_json::to_string(book.state())?, tx.now_ts()],
    )?;
    Ok(r)
}

fn actor_of(conn: &Connection, session: &str) -> Result<Actor> {
    Ok(match sessions::role_of(conn, session)? {
        crewd_proto::Role::Captain => Actor::captain(session),
        crewd_proto::Role::Worker => Actor::worker(session),
        crewd_proto::Role::Admin => Actor::admin(session),
    })
}

fn parse_ask(ask: &str) -> Result<AskId> {
    ask.parse()
        .map_err(|e: crewd_work::ParseIdError| CollabError::Invalid(e.to_string()))
}

fn view(conn: &Connection, crew: &str, a: &HumanAsk) -> Result<Ask> {
    let meta: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT resume_note, message_id FROM ask_meta WHERE crew_id=?1 AND ask=?2",
            params![crew, a.id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (resume_note, message_id) = meta.unwrap_or((None, None));
    let (state, answer, closed_at, withdraw_reason) = match &a.state {
        AskState::Open => (AskStateView::Open, None, None, None),
        AskState::Answered { text, at } => (
            AskStateView::Answered,
            Some(text.clone()),
            Some(ts(*at)),
            None,
        ),
        AskState::Defaulted { at } => (
            AskStateView::Defaulted,
            a.default.as_ref().map(|d| d.text.clone()),
            Some(ts(*at)),
            None,
        ),
        AskState::Withdrawn { reason, at } => (
            AskStateView::Withdrawn,
            None,
            Some(ts(*at)),
            Some(reason.clone()),
        ),
    };
    Ok(Ask {
        crew_id: crew.into(),
        ask: a.id.to_string(),
        question: a.question.clone(),
        asked_by: a.asked_by.session().map(|s| s.to_string()),
        asked_at: ts(a.asked_at),
        state,
        answer,
        closed_at,
        withdraw_reason,
        default: a.default.as_ref().map(|d| AskDefault {
            text: d.text.clone(),
            after_minutes: d.after_ms / 60_000,
        }),
        item: a.item.map(|w| w.to_string()),
        resume_note,
        message_id,
    })
}

pub fn get(conn: &Connection, crew: &str, ask: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let book = load(conn, crew)?;
    let a = book
        .ask(h)
        .ok_or_else(|| CollabError::NotFound(format!("机组 {crew} 没有 {ask}")))?;
    view(conn, crew, a)
}

pub fn open(tx: &mut Tx<'_>, asker: &str, o: OpenAsk) -> Result<Ask> {
    let s = sessions::get(tx.conn(), asker)?;
    let crew = crews::get(tx.conn(), &s.crew_id)?;
    if crew.builtin {
        return Err(CollabError::Forbidden(
            "内建总机组不提问（同 McpServer.swift:745-753）".into(),
        ));
    }
    let actor = actor_of(tx.conn(), asker)?;
    let question = o.question.trim().to_string();
    let default = o.default.map(|d| DefaultOption {
        text: d.text,
        after_ms: d.after_minutes.saturating_mul(60_000),
    });
    let h = with_book(tx, &crew.id, |b, now| {
        b.open_ask(&actor, now, &question, default, None)
            .map(|a| a.value)
    })?;
    let root = crews::root_of(tx.conn(), &crew.id)?;
    let posted =
        messages::announce_to_human(tx, &root, Sender::Session(asker.into()), question, "ask")?;
    tx.conn().execute(
        "INSERT INTO ask_meta(crew_id,ask,resume_note,message_id) VALUES(?1,?2,?3,?4)",
        params![crew.id, h.to_string(), o.resume_note, posted.message.id],
    )?;
    tx.note(Change::AskChanged {
        crew_id: crew.id.clone(),
        ask: h.to_string(),
    });
    get(tx.conn(), &crew.id, &h.to_string())
}

/// 直接开的问被答 / 按默认办了：叫醒提问者；它已无法恢复（或已不在）→ 叫它机组的机长转达（§22.4）。
fn wake_asker(tx: &mut Tx<'_>, crew: &str, a: &Ask) -> Result<()> {
    let Some(asker) = &a.asked_by else {
        return Ok(());
    };
    let target = match crewd_store::sessions::get(tx.conn(), asker)? {
        Some(s) if s.resume_failed_at.is_none() => Some(s.id),
        _ => crews::captain_of(tx.conn(), crew)?.filter(|c| c != asker),
    };
    if let Some(t) = target {
        wakes::enqueue(
            tx,
            WakeRequest {
                session: t,
                kind: WakeKind::AskAnswer,
                source: wakes::source::ask(crew, &a.ask),
                priority: WakePriority::Human,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )?;
    }
    Ok(())
}

pub fn answer(tx: &mut Tx<'_>, crew: &str, ask: &str, text: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let (applied, standalone) = with_book(tx, crew, |b, now| {
        let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
        b.answer_ask(&Actor::Human, now, h, text)
            .map(|ap| (ap, standalone))
    })?;
    let cleared: Vec<(String, Option<String>)> = applied
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::BlockerCleared { id, owner, .. } => {
                Some((id.to_string(), owner.as_ref().map(|o| o.to_string())))
            }
            _ => None,
        })
        .collect();
    let a = get(tx.conn(), crew, ask)?;
    if standalone {
        wake_asker(tx, crew, &a)?;
    }
    tx.note(Change::AskChanged {
        crew_id: crew.into(),
        ask: a.ask.clone(),
    });
    tx.note(Change::AskAnswered {
        crew_id: crew.into(),
        ask: a.ask.clone(),
        asked_by: a.asked_by.clone(),
        cleared,
    });
    Ok(a)
}

pub fn withdraw(tx: &mut Tx<'_>, by: &Sender, crew: &str, ask: &str, reason: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let actor = match by {
        Sender::Human => Actor::Human,
        Sender::Session(s) => actor_of(tx.conn(), s)?,
        Sender::System => return Err(CollabError::Forbidden("系统不撤回问".into())),
    };
    with_book(tx, crew, |b, now| b.withdraw_ask(&actor, now, h, reason))?;
    tx.note(Change::AskChanged {
        crew_id: crew.into(),
        ask: ask.into(),
    });
    get(tx.conn(), crew, ask)
}

/// 默认方案到点的问按默认办（M3c 的定时拍按机组逐个调，控制者裁定 C7）。返回办了的 H 号。
pub fn apply_due_defaults(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<String>> {
    let done: Vec<(AskId, bool)> = with_book(tx, crew, |b, now| {
        let mut out = Vec::new();
        for h in b.defaults_due(now) {
            let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
            b.apply_default(&Actor::System, now, h)?;
            out.push((h, standalone));
        }
        Ok(out)
    })?;
    let mut ids = Vec::new();
    for (h, standalone) in done {
        let a = get(tx.conn(), crew, &h.to_string())?;
        if standalone {
            wake_asker(tx, crew, &a)?;
        }
        tx.note(Change::AskChanged {
            crew_id: crew.into(),
            ask: a.ask.clone(),
        });
        ids.push(a.ask);
    }
    Ok(ids)
}

/// 人的待确认清单（`ask.list`）：`root` 给了就只看这棵树里的机组。等得久的在前。
pub fn waits(conn: &Connection, now_ms: i64, root: Option<&str>) -> Result<Vec<HumanWait>> {
    let mut st = conn.prepare("SELECT crew_id FROM work_books ORDER BY crew_id")?;
    let crews_with_books: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for crew in crews_with_books {
        if let Some(r) = root
            && crews::root_of(conn, &crew)? != r
        {
            continue;
        }
        let book = load(conn, &crew)?;
        for w in book.human_waits(Ts(now_ms)) {
            let a = book.ask(w.ask).expect("human_waits 只列账本里的问");
            out.push(HumanWait {
                ask: view(conn, &crew, a)?,
                items: w.items.iter().map(|i| i.to_string()).collect(),
                owners: w.owners.iter().map(|o| o.to_string()).collect(),
                waited_ms: w.waited_ms,
                tier: match w.tier {
                    WaitTier::Fresh => WaitTierView::Fresh,
                    WaitTier::Pinned => WaitTierView::Pinned,
                    WaitTier::StillWanted => WaitTierView::StillWanted,
                },
                default_due_at: w.default_due_at.map(ts),
            });
        }
    }
    out.sort_by(|a, b| {
        b.waited_ms
            .cmp(&a.waited_ms)
            .then(a.ask.crew_id.cmp(&b.ask.crew_id))
            .then(a.ask.ask.cmp(&b.ask.ask))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, ok_at, store};
    use crewd_proto::{Mention, MentionKind};

    fn q(text: &str) -> OpenAsk {
        OpenAsk {
            question: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn open_issues_h_numbers_from_the_crew_book_and_posts_to_the_root_human_channel() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            open(
                tx,
                "w-b1",
                OpenAsk {
                    resume_note: Some("拿到答复后继续发版".into()),
                    ..q("发版窗口定在周几？")
                },
            )
        });
        assert_eq!((a.crew_id.as_str(), a.ask.as_str()), ("crew-b", "H1"));
        assert_eq!(a.asked_by.as_deref(), Some("w-b1"));
        assert_eq!(a.state, AskStateView::Open);
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(
            m.channel_id, "crew-a/human",
            "公告发到 crew-b 的根（crew-a）的人类群"
        );
        assert_eq!(
            m.mentions,
            [Mention {
                kind: MentionKind::Human,
                target_id: None
            }]
        );
        let b = ok(&mut s, |tx| open(tx, "cap-b", q("第二个问题？")));
        assert_eq!(b.ask, "H2", "号连续");
        let last_ask = ok(&mut s, |tx| {
            with_book(tx, "crew-b", |book, _| Ok(book.state().last_ask))
        });
        assert_eq!(
            last_ask, 2,
            "与工作账本共用一个计数（M4b 的 block 开新问也走它）"
        );
        let c = ok(&mut s, |tx| open(tx, "w-a1", q("crew-a 的第一个问题？")));
        assert_eq!(c.ask, "H1", "H 号按机组");
    }

    #[test]
    fn failed_ask_consumes_no_h_number_and_emits_nothing() {
        let mut s = store();
        s.conn()
            .execute(
                "INSERT INTO ask_meta(crew_id,ask) VALUES('crew-a','H1')",
                [],
            )
            .unwrap();
        let e =
            crate::run(&mut s, NOW, |tx| open(tx, "w-a1", q("会在写附注时失败？"))).unwrap_err();
        assert!(matches!(e, CollabError::Store(_)), "{e:?}");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM work_books"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM messages"), 0);
        s.conn().execute("DELETE FROM ask_meta", []).unwrap();
        assert_eq!(ok(&mut s, |tx| open(tx, "w-a1", q("这次能成？"))).ask, "H1");
    }

    #[test]
    fn asks_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        {
            let mut s = crewd_store::Store::open(&path).unwrap();
            // 复用夹具的数据：把内存库的内容搬不过来，这里直接在文件库上造一个最小机组。
            crate::run(&mut s, NOW, |tx| {
                let c = crate::crews::create(
                    tx,
                    crate::crews::NewCrew {
                        title: "文件库机组".into(),
                        working_directory: "/tmp/example".into(),
                        ..Default::default()
                    },
                )?;
                let w = crate::sessions::register(
                    tx,
                    crate::sessions::NewSession {
                        crew_id: c.id.clone(),
                        executor: "claude".into(),
                        ..Default::default()
                    },
                )?;
                open(tx, &w.id, q("重开库之后还在吗？"))
            })
            .unwrap();
        }
        let s = crewd_store::Store::open(&path).unwrap();
        let waits = waits(s.conn(), NOW + MIN, None).unwrap();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].ask.question, "重开库之后还在吗？");
        assert_eq!(waits[0].waited_ms, MIN);
    }

    #[test]
    fn answering_wakes_the_asker_and_reports_the_answer() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            open(
                tx,
                "w-a1",
                OpenAsk {
                    resume_note: Some("继续".into()),
                    ..q("用哪家？")
                },
            )
        });
        let (got, ch) = ok_at(&mut s, NOW + MIN, |tx| answer(tx, "crew-a", &a.ask, "用甲"));
        assert_eq!(got.state, AskStateView::Answered);
        assert_eq!(got.answer.as_deref(), Some("用甲"));
        assert!(ch.contains(&Change::AskAnswered {
            crew_id: "crew-a".into(),
            ask: "H1".into(),
            asked_by: Some("w-a1".into()),
            cleared: vec![]
        }));
        let w = crate::wakes::list(s.conn(), Some("w-a1"), 10).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].kind, w[0].priority, w[0].source.as_str()),
            (WakeKind::AskAnswer, WakePriority::Human, "ask:crew-a/H1")
        );
        assert!(matches!(
            err(&mut s, |tx| answer(tx, "crew-a", "H1", "再答一次")),
            CollabError::Work(Rejected::AskNotOpen(_))
        ));
        assert!(matches!(
            err(&mut s, |tx| answer(tx, "crew-a", "X1", "x")),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn an_unknown_h_number_is_not_found() {
        // R11：格式对、账本里没有的 H 号 = NotFound（-32004），不是 Invalid / Work（-32602）。
        let mut s = store();
        ok(&mut s, |tx| open(tx, "w-a1", q("有一条")));
        assert!(matches!(
            err(&mut s, |tx| answer(tx, "crew-a", "H9", "x")),
            CollabError::NotFound(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| withdraw(
                tx,
                &Sender::Human,
                "crew-a",
                "H9",
                "不要了"
            )),
            CollabError::NotFound(_)
        ));
        assert!(matches!(
            get(s.conn(), "crew-a", "H9"),
            Err(CollabError::NotFound(_))
        ));
        assert!(
            matches!(
                err(&mut s, |tx| answer(tx, "crew-x", "H1", "x")),
                CollabError::NotFound(_)
            ),
            "没开过问的机组"
        );
    }

    #[test]
    fn an_unrecoverable_asker_is_relayed_through_its_captain() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("要不要回滚？")));
        ok(&mut s, |tx| {
            crate::sessions::set_resume_failed(tx, "w-a1", true)
        });
        ok(&mut s, |tx| answer(tx, "crew-a", &a.ask, "回滚"));
        assert!(
            crate::wakes::list(s.conn(), Some("w-a1"), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::wakes::list(s.conn(), Some("cap-a"), 10).unwrap()[0].source,
            "ask:crew-a/H1"
        );
    }

    #[test]
    fn withdraw_follows_the_book_permissions() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("还要吗？")));
        assert!(matches!(
            err(&mut s, |tx| withdraw(
                tx,
                &Sender::Session("w-a2".into()),
                "crew-a",
                &a.ask,
                "不是我的"
            )),
            CollabError::Work(Rejected::Forbidden(_))
        ));
        let w = ok(&mut s, |tx| {
            withdraw(
                tx,
                &Sender::Session("cap-a".into()),
                "crew-a",
                &a.ask,
                "不需要了",
            )
        });
        assert_eq!(
            (w.state, w.withdraw_reason.as_deref()),
            (AskStateView::Withdrawn, Some("不需要了"))
        );
        assert!(waits(s.conn(), NOW, None).unwrap().is_empty());
    }

    #[test]
    fn defaults_apply_when_due_and_wake_the_asker() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            open(
                tx,
                "w-a1",
                OpenAsk {
                    default: Some(AskDefault {
                        text: "按周三".into(),
                        after_minutes: 30,
                    }),
                    ..q("周几发版？")
                },
            )
        });
        assert!(
            ok(&mut s, |tx| apply_due_defaults(tx, "crew-a")).is_empty(),
            "没到点"
        );
        let (done, _) = ok_at(&mut s, NOW + 31 * MIN, |tx| {
            apply_due_defaults(tx, "crew-a")
        });
        assert_eq!(done, std::slice::from_ref(&a.ask));
        assert_eq!(
            get(s.conn(), "crew-a", &a.ask).unwrap().state,
            AskStateView::Defaulted
        );
        assert_eq!(
            crate::wakes::list(s.conn(), Some("w-a1"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn the_builtin_crew_does_not_ask() {
        let mut s = store();
        s.conn().execute("INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('chief-1','chief','captain','claude','t')", []).unwrap();
        assert!(matches!(
            err(&mut s, |tx| open(tx, "chief-1", q("?"))),
            CollabError::Forbidden(_)
        ));
    }
}
