//! 人类待确认（H 号）：经 crewd-work 的工作账本发号（§19 Q1：工作账本是唯一发号处）。
use crate::messages;
use crate::refusal::{Code, Refusal};
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
    /// 「X 分钟没回复就按 A 办」（§6；到点后照常经过 runner 自己的权限确认，§13 Q2）。
    pub default: Option<AskDefault>,
    /// 回答后叫醒提问者时带上的「接着做什么」。
    pub resume_note: Option<String>,
}

fn ts(t: Ts) -> String {
    crewd_store::ts_from_ms(t.0)
}

// References witness identity; they never replace missing authoritative state.
fn verify_book_references(conn: &Connection, crew: &str, state: Option<&BookState>) -> Result<()> {
    let mut legacy = conn.prepare("SELECT ask FROM ask_meta WHERE crew_id=?1")?;
    let rows = legacy.query_map([crew], |r| r.get::<_, String>(0))?;
    for row in rows {
        let key = row?;
        if !state.is_some_and(|st| {
            key.parse::<AskId>()
                .is_ok_and(|id| st.asks.contains_key(&id))
        }) {
            return Err(crewd_store::StoreError::Invalid(
                "work book is missing a legacy question identity".into(),
            )
            .into());
        }
    }
    let mut statement = conn.prepare(
        "SELECT kind,source_id FROM object_refs WHERE crew_id=?1 AND substr(source_id,1,5)='work:'",
    )?;
    let rows = statement.query_map([crew], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (kind, source) = row?;
        let alias = source
            .strip_prefix("work:")
            .expect("SQL selected the exact namespace");
        let valid = state.is_some_and(|st| match kind.as_str() {
            "human_task" => alias
                .parse::<AskId>()
                .is_ok_and(|id| st.asks.contains_key(&id)),
            "plan" => alias
                .parse::<crewd_work::WorkId>()
                .is_ok_and(|id| st.items.contains_key(&id)),
            "requirement" => alias
                .parse::<crewd_work::EntryNo>()
                .is_ok_and(|id| st.entries.contains_key(&id)),
            _ => false,
        });
        if !valid {
            return Err(crewd_store::StoreError::Invalid(
                "work book is missing an issued object identity".into(),
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn load(conn: &Connection, crew: &str) -> Result<WorkBook> {
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM work_books WHERE crew_id=?1",
            [crew],
            |r| r.get(0),
        )
        .optional()?;
    match state {
        None => {
            verify_book_references(conn, crew, None)?;
            Ok(WorkBook::new(CrewId::new(crew), Config::default()))
        }
        Some(s) => {
            let st: BookState = serde_json::from_str(&s)?;
            if st.crew.as_str() != crew {
                return Err(crewd_store::StoreError::Invalid(
                    "work book crew identity mismatch".into(),
                )
                .into());
            }
            // 读回来不合法 = 服务端的错（同账本不变式，终审 M4）：映射成 -32603，细节只进日志，不给客户端。
            verify_book_references(conn, crew, Some(&st))?;
            WorkBook::restore(st, Config::default())
                .map_err(|v| CollabError::Work(Rejected::Invariant(v)))
        }
    }
}

/// 账本拒绝 → collab 的错误。号码格式对、账本里没有（H / W / #）= NotFound（R11：-32004，不是 -32602）；
/// 其余原样带着 `Rejected`（服务层按种类映射）。
fn rejected(crew: &str, r: Rejected) -> CollabError {
    match r {
        Rejected::UnknownAsk(id) => unknown_id(crew, id),
        Rejected::UnknownItem(id) => unknown_id(crew, id),
        Rejected::UnknownEntry(id) => unknown_id(crew, id),
        other => CollabError::Work(other),
    }
}

/// 在同一个事务里读整本账、跑一条命令、整份写回（M4a 的持久化边界）。M4b 的工作项命令也走这里。
/// 闭包返回账本自己的 `Result<R, Rejected>`；这里把 `Rejected` 映射成 collab 的错误（R5，见 `rejected`）。
/// 被拒 = 不写回（账本原样）；命令没改动账本（只读、到点的拍子没事可办）也不写回——不改 `updated_at`，
/// Full state, real events and stable references commit in the same transaction.
pub fn with_book<R>(
    tx: &mut Tx<'_>,
    crew: &str,
    f: impl FnOnce(&mut WorkBook, Ts) -> std::result::Result<R, Rejected>,
) -> Result<R> {
    crews::get(tx.conn(), crew)?;
    let mut book = load(tx.conn(), crew)?;
    let before = book.state().clone();
    let r = f(&mut book, Ts(tx.now_ms())).map_err(|e| rejected(crew, e))?;
    let after = book.state();
    if after.crew.as_str() != crew
        || after.last_ask < before.last_ask
        || after.last_item < before.last_item
        || after.last_seq < before.last_seq
        || before.items.keys().any(|id| !after.items.contains_key(id))
        || before.asks.keys().any(|id| !after.asks.contains_key(id))
        || before
            .entries
            .keys()
            .any(|id| !after.entries.contains_key(id))
    {
        return Err(crewd_store::StoreError::Invalid(
            "work command retired an issued identity".into(),
        )
        .into());
    }
    verify_book_references(tx.conn(), crew, Some(after))?;
    let events = book.take_log();
    if *book.state() == before {
        return Ok(r);
    }
    tx.conn().execute(
        "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,?3)
         ON CONFLICT(crew_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at",
        params![crew, serde_json::to_string(book.state())?, tx.now_ts()],
    )?;
    crewd_store::work_events::append(tx.conn(), crew, &events)?;
    // Identity metadata commits with the authoritative state, never as a second
    // copy of status. Legacy local aliases remain valid during staged M4b cutover.
    for event in events {
        use crewd_store::object_refs::{self, ObjectKind};
        let reference = match event.kind {
            EventKind::EntryRegistered { entry } => {
                Some((ObjectKind::Requirement, entry.to_string()))
            }
            EventKind::ItemCreated { id, .. } => Some((ObjectKind::Plan, id.to_string())),
            EventKind::AskOpened { ask, .. } => Some((ObjectKind::HumanTask, ask.to_string())),
            _ => None,
        };
        if let Some((kind, source)) = reference {
            object_refs::bind(
                tx.conn(),
                kind,
                crew,
                &format!("work:{source}"),
                &tx.now_ts(),
            )?;
        }
    }
    Ok(r)
}

fn actor_of(conn: &Connection, session: &str) -> Result<Actor> {
    Ok(match sessions::role_of(conn, session)? {
        crewd_proto::Role::Captain => Actor::captain(session),
        crewd_proto::Role::Worker => Actor::worker(session),
        crewd_proto::Role::Admin => Actor::admin(session),
    })
}

/// 格式对、账本里没有的 H / W / # 号（R11：NotFound）。
fn unknown_id(crew: &str, id: impl std::fmt::Display) -> CollabError {
    CollabError::NotFound(
        Refusal::new(Code::AskIdNotFound)
            .arg("crew", crew)
            .arg("id", id.to_string()),
    )
}

fn parse_ask(ask: &str) -> Result<AskId> {
    ask.parse().map_err(|e: crewd_work::ParseIdError| {
        CollabError::Invalid(Refusal::new(Code::AskBadId).arg("input", e.input))
    })
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

/// 这条消息是哪条问的公告（`ask_meta.message_id`）：(机组, H 号)。
pub(crate) fn by_message(conn: &Connection, message_id: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT crew_id, ask FROM ask_meta WHERE message_id=?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub fn get(conn: &Connection, crew: &str, ask: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let book = load(conn, crew)?;
    let a = book.ask(h).ok_or_else(|| unknown_id(crew, ask))?;
    view(conn, crew, a)
}

pub fn open(tx: &mut Tx<'_>, asker: &str, o: OpenAsk) -> Result<Ask> {
    let s = sessions::get(tx.conn(), asker)?;
    let crew = crews::get(tx.conn(), &s.crew_id)?;
    // 归档的机组只读（终审 5）：二级机组的 home 人类群还活着，不能靠公告发不进去碰巧挡住。
    crews::refuse_archived(tx.conn(), &crew.id)?;
    let actor = actor_of(tx.conn(), asker)?;
    let question = o.question.trim().to_string();
    let default = o.default.map(|d| DefaultOption {
        text: d.text,
        after_ms: d.after_minutes.saturating_mul(60_000),
    });
    let h = with_book(tx, &crew.id, |b, now| {
        b.open_ask(&actor, now, &question, default, None)
            .map(|a| a.value)
    })
    .map_err(|e| match e {
        // 账本的英文短句换成带码的拒绝（M3b T14，同 `withdraw_refused`）。
        CollabError::Work(Rejected::Blank("question")) => {
            CollabError::Invalid(Refusal::new(Code::AskQuestionEmpty))
        }
        e => e,
    })?;
    let home = crews::home_of(tx.conn(), &crew.id)?;
    let posted =
        messages::announce_to_human(tx, &home, Sender::Session(asker.into()), question, "ask")?;
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

/// session 还在、且没被标恢复失败：叫醒它它就能接着干。
fn available(conn: &Connection, id: &str) -> Result<bool> {
    Ok(matches!(crewd_store::sessions::get(conn, id)?, Some(s) if s.resume_failed_at.is_none()))
}

/// 直接开的问被答 / 按默认办了：叫醒能接着干的第一位（§22.4）。依次：提问者 → 问所在机组此刻的机长（转达）
/// → home（总机组 / 顶层机组）此刻的机长。三位都不能接（例如 home 机长自己提问、之后恢复失败）→ 不叫任何人：
/// 答复照样记在账上，人在 `ask.list` 里看得到，下次有人接手时读得到。
fn wake_asker(tx: &mut Tx<'_>, crew: &str, a: &Ask) -> Result<()> {
    let Some(asker) = &a.asked_by else {
        return Ok(());
    };
    let conn = tx.conn();
    let mut candidates = vec![asker.clone()];
    candidates.extend(crews::captain_of(conn, crew)?);
    candidates.extend(crews::captain_of(conn, &crews::home_of(conn, crew)?)?);
    let mut target = None;
    for c in candidates {
        if available(conn, &c)? {
            target = Some(c);
            break;
        }
    }
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

/// 一条问被答 / 按默认办之后的收尾（`answer` 与 `apply_due_defaults` 共用）：直接开的问叫醒提问者；
/// 记 `AskChanged` 与 `AskAnswered`（`cleared` = 账本事件里每条 `BlockerCleared` 的 (项, 负责人)，裁定 17）。
fn closed(
    tx: &mut Tx<'_>,
    crew: &str,
    h: AskId,
    standalone: bool,
    events: &[crewd_work::Event],
) -> Result<Ask> {
    let cleared: Vec<(String, Option<String>)> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::BlockerCleared { id, owner, .. } => {
                Some((id.to_string(), owner.as_ref().map(|o| o.to_string())))
            }
            _ => None,
        })
        .collect();
    let a = get(tx.conn(), crew, &h.to_string())?;
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

pub fn answer(tx: &mut Tx<'_>, crew: &str, ask: &str, text: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let (applied, standalone) = with_book(tx, crew, |b, now| {
        let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
        b.answer_ask(&Actor::Human, now, h, text)
            .map(|ap| (ap, standalone))
    })?;
    closed(tx, crew, h, standalone, &applied.events)
}

/// 撤回一条问。人随时可以；session 只能是提问者本人，或**问所在机组**此刻的机长，或总机组的机长（权限上当作
/// 每个机组的机长，PM 2026-10-07）——别的机组的机长（另一棵树的、子机组的）、总机组的普通 session 在这本账上
/// 只算普通 session（账本按提问者放行）。
pub fn withdraw(tx: &mut Tx<'_>, by: &Sender, crew: &str, ask: &str, reason: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let actor = match by {
        Sender::Human => Actor::Human,
        Sender::Session(s) if sessions::crew_of(tx.conn(), s)? == crew => actor_of(tx.conn(), s)?,
        Sender::Session(s) if crews::is_management_captain(tx.conn(), s)? => Actor::captain(s),
        Sender::Session(s) => Actor::worker(s),
        Sender::System => {
            return Err(CollabError::Forbidden(Refusal::new(
                Code::AskSystemCannotWithdraw,
            )));
        }
    };
    let canonical = h.to_string();
    with_book(tx, crew, |b, now| b.withdraw_ask(&actor, now, h, reason))
        .map_err(|e| withdraw_refused(e, &canonical))?;
    // 规范写法（"H1"），不是调用方原样给的（"h1"）：客户端按它对号（终审 M3）。
    tx.note(Change::AskChanged {
        crew_id: crew.into(),
        ask: h.to_string(),
    });
    get(tx.conn(), crew, ask)
}

/// 账本对撤回的拒绝 → 带码的规则拒绝（M3b T14：agent 按自己的语言看到「下一步」，不是账本的英文短句）。
/// RPC 码不变：Forbidden / Invalid / Conflict 与 `dispatch::collab_err` 对账本原拒绝的映射一致。
fn withdraw_refused(e: CollabError, ask: &str) -> CollabError {
    match e {
        CollabError::Work(Rejected::Forbidden(_)) => {
            CollabError::Forbidden(Refusal::new(Code::AskWithdrawForbidden).arg("ask", ask))
        }
        CollabError::Work(Rejected::Blank(_)) => {
            CollabError::Invalid(Refusal::new(Code::AskReasonEmpty))
        }
        CollabError::Work(Rejected::AskNotOpen(_)) => {
            CollabError::Conflict(Refusal::new(Code::AskNotOpen).arg("ask", ask))
        }
        CollabError::Work(Rejected::AskInUse(_)) => {
            CollabError::Conflict(Refusal::new(Code::AskInUse).arg("ask", ask))
        }
        e => e,
    }
}

/// 机组归档（`crews::archive` 同一事务里调，N1 裁定 13）：以人的身份撤回这个机组账本里全部还开着的问，理由取
/// render md 的 `notice.ask.crew_removed`（按 meta.locale）。挂在一个 home 人类群里的问只来自它自己和它的下级，
/// 而下级必须先归档，所以归档到 home 时那里的问都已撤回。解档不重开。还卡着活项的问账本不让撤 →
/// Conflict（归档整体失败，什么都不变）。每条撤回记 AskChanged。
pub(crate) fn withdraw_open_for_archive(tx: &mut Tx<'_>, crew: &str) -> Result<()> {
    let open: Vec<AskId> = load(tx.conn(), crew)?
        .state()
        .asks
        .iter()
        .filter(|(_, a)| a.state == AskState::Open)
        .map(|(h, _)| *h)
        .collect();
    if open.is_empty() {
        return Ok(());
    }
    let reason = crate::prompts::render_file(crate::settings::locale(tx.conn())?)
        .get("notice.ask.crew_removed")
        .trim()
        .to_string();
    for h in open {
        with_book(tx, crew, |b, now| {
            b.withdraw_ask(&Actor::Human, now, h, &reason)
        })
        .map_err(|e| match e {
            CollabError::Work(Rejected::AskInUse(h)) => CollabError::Conflict(
                Refusal::new(Code::AskRemoveBlocked)
                    .arg("crew", crew)
                    .arg("ask", h.to_string()),
            ),
            e => e,
        })?;
        tx.note(Change::AskChanged {
            crew_id: crew.into(),
            ask: h.to_string(),
        });
    }
    Ok(())
}

/// 默认方案到点的问按默认办（M3c 的定时拍按机组逐个调，控制者裁定 C7）。返回办了的 H 号。
pub fn apply_due_defaults(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<String>> {
    let done: Vec<(AskId, bool, Vec<crewd_work::Event>)> = with_book(tx, crew, |b, now| {
        let mut out = Vec::new();
        for h in b.defaults_due(now) {
            let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
            let applied = b.apply_default(&Actor::System, now, h)?;
            out.push((h, standalone, applied.events));
        }
        Ok(out)
    })?;
    let mut ids = Vec::new();
    for (h, standalone, events) in done {
        ids.push(closed(tx, crew, h, standalone, &events)?.ask);
    }
    Ok(ids)
}

/// 人的待确认清单（`ask.list`）：`home` 给了就只看 home 是它的机组（它的人类群里的问）。等得久的在前。
pub fn waits(conn: &Connection, now_ms: i64, home: Option<&str>) -> Result<Vec<HumanWait>> {
    let mut st = conn.prepare("SELECT crew_id FROM work_books ORDER BY crew_id")?;
    let crews_with_books: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for crew in crews_with_books {
        if let Some(h) = home
            && crews::home_of(conn, &crew)? != h
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

    #[test]
    fn a_second_level_ask_goes_to_its_top_level_human_channel_not_the_management_crew() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            open(
                tx,
                "w-b1",
                OpenAsk {
                    question: "用哪个分支？".into(),
                    ..Default::default()
                },
            )
        });
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(m.channel_id, "crew-a/human");
        assert!(!crate::channels::human_channel_exists(s.conn(), "chief").unwrap());
        assert_eq!(waits(s.conn(), NOW, Some("crew-a")).unwrap().len(), 1);
        assert!(
            waits(s.conn(), NOW, Some("chief")).unwrap().is_empty(),
            "按 home 过滤"
        );
    }

    fn q(text: &str) -> OpenAsk {
        OpenAsk {
            question: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn open_issues_h_numbers_from_the_crew_book_and_posts_to_the_home_human_channel() {
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
            "公告发到 crew-b 的 home（crew-a）的人类群"
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
        let channels_before = count(&s, "SELECT COUNT(*) FROM channels");
        let wakes_before = count(&s, "SELECT COUNT(*) FROM wakes");
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
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM channels"),
            channels_before,
            "懒建的人类群一起回滚"
        );
        assert_eq!(count(&s, "SELECT COUNT(*) FROM wakes"), wakes_before);
        s.conn().execute("DELETE FROM ask_meta", []).unwrap();
        assert_eq!(ok(&mut s, |tx| open(tx, "w-a1", q("这次能成？"))).ask, "H1");
    }

    /// 终审 5：提问者的机组已移除 → crew.removed（Conflict，指路先恢复）；不发号、不建账本、不发公告。
    /// 二级机组 crew-b 归档后它的 home（crew-a）的人类群还活着——拒绝必须来自提问者自己的机组，不能靠
    /// 「公告发不进归档的人类群」碰巧挡住（顶层机组 crew-x 那条路就是被它碰巧挡住的）。
    #[test]
    fn an_archived_crews_session_cannot_open_an_ask() {
        let mut s = store();
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        let messages_before = count(&s, "SELECT COUNT(*) FROM messages");
        for asker in ["cap-b", "w-b1", "cap-x"] {
            match err(&mut s, |tx| open(tx, asker, q("归档了还能问吗？"))) {
                CollabError::Conflict(r) => {
                    assert_eq!(r.code, Code::CrewRemoved, "{asker}: {r:?}")
                }
                e => panic!("{asker}: {e:?}"),
            }
        }
        assert_eq!(count(&s, "SELECT COUNT(*) FROM work_books"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM ask_meta"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM messages"), messages_before);
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
                        title: Some("提问者".into()),
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
            CollabError::Forbidden(r) if r.code == Code::AskWithdrawForbidden
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
    fn the_management_crews_own_asks_go_to_its_management_human_channel() {
        let mut s = store();
        let cap = ok(&mut s, |tx| {
            crate::sessions::register(
                tx,
                crate::sessions::NewSession {
                    crew_id: "chief".into(),
                    role: crewd_proto::Role::Captain,
                    executor: "claude".into(),
                    title: Some("总机长".into()),
                    ..Default::default()
                },
            )
        });
        let a = ok(&mut s, |tx| {
            open(
                tx,
                &cap.id,
                OpenAsk {
                    question: "全局要不要冻结发版？".into(),
                    ..Default::default()
                },
            )
        });
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(m.channel_id, "chief/human");
    }

    /// 总机组的机长权限上当作每个机组的机长（PM 2026-10-07）：能撤别的机组的问；总机组的普通 session 不能。
    #[test]
    fn the_management_captain_may_withdraw_any_crews_ask_and_its_workers_may_not() {
        let mut s = store();
        let boss = crate::testkit::org_captain(&mut s);
        let worker = crate::testkit::org_worker(&mut s);
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("谁能撤？")));
        assert!(matches!(
            err(&mut s, |tx| withdraw(
                tx,
                &Sender::Session(worker.clone()),
                "crew-a",
                &a.ask,
                "不是你的"
            )),
            CollabError::Forbidden(r) if r.code == Code::AskWithdrawForbidden
        ));
        let w = ok(&mut s, |tx| {
            withdraw(
                tx,
                &Sender::Session(boss.clone()),
                "crew-a",
                &a.ask,
                "管理层撤",
            )
        });
        assert_eq!(w.state, AskStateView::Withdrawn);
    }

    #[test]
    fn only_the_asker_or_the_ask_crews_captain_may_withdraw() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("谁能撤？")));
        for other in ["cap-x", "cap-b"] {
            assert!(
                matches!(
                    err(&mut s, |tx| withdraw(
                        tx,
                        &Sender::Session(other.into()),
                        "crew-a",
                        &a.ask,
                        "不是你的"
                    )),
                    CollabError::Forbidden(r) if r.code == Code::AskWithdrawForbidden
                ),
                "{other} 是别的机组的机长，不能撤 crew-a 的问"
            );
        }
        let w = ok(&mut s, |tx| {
            withdraw(
                tx,
                &Sender::Session("w-a1".into()),
                "crew-a",
                &a.ask,
                "自己撤",
            )
        });
        assert_eq!(w.state, AskStateView::Withdrawn);
        let b = ok(&mut s, |tx| open(tx, "w-a1", q("人能撤吗？")));
        let h = ok(&mut s, |tx| {
            withdraw(tx, &Sender::Human, "crew-a", &b.ask, "人撤")
        });
        assert_eq!(h.state, AskStateView::Withdrawn);
    }

    #[test]
    fn a_blank_question_is_refused_with_a_code() {
        let mut s = store();
        match err(&mut s, |tx| open(tx, "w-a1", q(" \n "))) {
            CollabError::Invalid(r) => assert_eq!(r, Refusal::new(Code::AskQuestionEmpty)),
            e => panic!("{e:?}"),
        }
    }

    /// 账本对撤回的四种拒绝都带码（M3b T14）：agent 拿到的是按语言渲染的「下一步」，不是账本的英文短句。
    #[test]
    fn withdraw_refusals_carry_codes() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("还要吗？")));
        let me = Sender::Session("w-a1".into());
        match err(&mut s, |tx| withdraw(tx, &me, "crew-a", "h1", "  ")) {
            CollabError::Invalid(r) => assert_eq!(r, Refusal::new(Code::AskReasonEmpty)),
            e => panic!("{e:?}"),
        }
        match err(&mut s, |tx| {
            withdraw(tx, &Sender::Session("w-a2".into()), "crew-a", "h1", "x")
        }) {
            CollabError::Forbidden(r) => assert_eq!(
                r,
                Refusal::new(Code::AskWithdrawForbidden).arg("ask", "H1"),
                "canonical id, not the caller's spelling"
            ),
            e => panic!("{e:?}"),
        }
        ok(&mut s, |tx| withdraw(tx, &me, "crew-a", &a.ask, "不用了"));
        match err(&mut s, |tx| withdraw(tx, &me, "crew-a", &a.ask, "再撤")) {
            CollabError::Conflict(r) => {
                assert_eq!(r, Refusal::new(Code::AskNotOpen).arg("ask", "H1"))
            }
            e => panic!("{e:?}"),
        }
        let h = blocked_item_with_default(&mut s);
        match err(&mut s, |tx| withdraw(tx, &me, "crew-a", &h, "不用了")) {
            CollabError::Conflict(r) => {
                assert_eq!(r, Refusal::new(Code::AskInUse).arg("ask", h.as_str()))
            }
            e => panic!("{e:?}"),
        }
    }

    /// crew-a 里 w-a1 名下的一项卡在一条带默认方案（30 分钟）的新问上，返回 H 号。
    fn blocked_item_with_default(s: &mut crewd_store::Store) -> String {
        ok(s, |tx| {
            with_book(tx, "crew-a", |b, now| {
                let w = b
                    .create_item(
                        &Actor::worker("w-a1"),
                        now,
                        crewd_work::NewItem::titled("示例项").owned_by("w-a1"),
                    )?
                    .value;
                let r = b.block(
                    &Actor::worker("w-a1"),
                    now,
                    w,
                    vec![crewd_work::BlockOn::NewAsk {
                        question: "用哪个格式？".into(),
                        default: Some(DefaultOption {
                            text: "旧格式".into(),
                            after_ms: 30 * MIN,
                        }),
                    }],
                    crewd_work::Continuation::Nothing,
                )?;
                Ok(r.value.asks[0].to_string())
            })
        })
    }

    #[test]
    fn a_due_default_reports_the_cleared_blockers_like_an_answer() {
        let mut s = store();
        let h = blocked_item_with_default(&mut s);
        let (done, ch) = ok_at(&mut s, NOW + 31 * MIN, |tx| {
            apply_due_defaults(tx, "crew-a")
        });
        assert_eq!(done, std::slice::from_ref(&h));
        assert!(
            ch.contains(&Change::AskAnswered {
                crew_id: "crew-a".into(),
                ask: h.clone(),
                asked_by: Some("w-a1".into()),
                cleared: vec![("W1".into(), Some("w-a1".into()))],
            }),
            "{ch:?}"
        );
    }

    #[test]
    fn a_tick_with_nothing_due_rewrites_and_creates_no_book() {
        let mut s = store();
        ok(&mut s, |tx| {
            open(
                tx,
                "w-a1",
                OpenAsk {
                    default: Some(AskDefault {
                        text: "按周三".into(),
                        after_minutes: 30,
                    }),
                    ..q("周几？")
                },
            )
        });
        let before: Vec<(String, String)> = {
            let mut st = s
                .conn()
                .prepare("SELECT crew_id, updated_at FROM work_books ORDER BY crew_id")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(before.len(), 1);
        let (_, ch) = ok_at(&mut s, NOW + 5 * MIN, |tx| {
            for crew in ["crew-a", "crew-b", "crew-x", "chief"] {
                assert!(apply_due_defaults(tx, crew)?.is_empty());
            }
            Ok(())
        });
        let after: Vec<(String, String)> = {
            let mut st = s
                .conn()
                .prepare("SELECT crew_id, updated_at FROM work_books ORDER BY crew_id")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(
            after, before,
            "没到点的拍子不改写账本、不给没账本的机组建账本"
        );
        assert!(ch.is_empty(), "{ch:?}");
    }

    #[test]
    fn an_unavailable_captain_asker_falls_back_to_the_home_captain() {
        let mut s = store();
        // cap-b 在 crew-b 提问后恢复失败：crew-b 的机长就是它自己 → 叫 home crew-a 的机长。
        let a = ok(&mut s, |tx| open(tx, "cap-b", q("要不要上线？")));
        ok(&mut s, |tx| {
            crate::sessions::set_resume_failed(tx, "cap-b", true)
        });
        ok(&mut s, |tx| answer(tx, "crew-b", &a.ask, "上"));
        assert!(
            crate::wakes::list(s.conn(), Some("cap-b"), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::wakes::list(s.conn(), Some("cap-a"), 10).unwrap()[0].source,
            "ask:crew-b/H1"
        );
    }

    #[test]
    fn an_unavailable_home_captain_asker_wakes_nobody() {
        let mut s = store();
        // cap-a 是 home 的机长：本机组机长与 home 机长都是它自己 → 没人可转达（答复仍在 ask.list 里可见）。
        let a = ok(&mut s, |tx| open(tx, "cap-a", q("要不要回滚？")));
        ok(&mut s, |tx| {
            crate::sessions::set_resume_failed(tx, "cap-a", true)
        });
        let (_, ch) = ok_at(&mut s, NOW + MIN, |tx| answer(tx, "crew-a", &a.ask, "回滚"));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM wakes"), 0);
        assert!(!ch.iter().any(|c| matches!(c, Change::WakeEnqueued { .. })));
    }

    /// 终审 M3：撤回时事件里的 H 号是规范写法（"H1"），不是调用方原样给的（"h1"）——客户端按它对号。
    #[test]
    fn withdraw_reports_the_canonical_h_number() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("还要吗？")));
        assert_eq!(a.ask, "H1");
        let (_, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            withdraw(tx, &Sender::Human, "crew-a", "h1", "不需要了")
        });
        let asks: Vec<&str> = ch
            .iter()
            .filter_map(|c| match c {
                Change::AskChanged { ask, .. } => Some(ask.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(asks, ["H1"], "{ch:?}");
    }

    /// 终审 M4：库里的账本读回来不合法 = 服务端的错（不变式），不是调用方的参数错（-32602 会把细节发给客户端）。
    #[test]
    fn a_corrupt_stored_book_is_an_invariant_error_not_invalid_params() {
        let mut s = store();
        ok(&mut s, |tx| open(tx, "w-a1", q("还要吗？")));
        let raw: String = s
            .conn()
            .query_row(
                "SELECT state FROM work_books WHERE crew_id='crew-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let key = ["lastAsk", "last_ask"]
            .into_iter()
            .find(|k| v.get(*k).is_some())
            .expect("账本状态里有 H 号计数器");
        v[key] = serde_json::json!(0);
        s.conn()
            .execute(
                "UPDATE work_books SET state=?1 WHERE crew_id='crew-a'",
                [v.to_string()],
            )
            .unwrap();
        let e = get(s.conn(), "crew-a", "H1").unwrap_err();
        assert!(
            matches!(e, CollabError::Work(Rejected::Invariant(_))),
            "{e:?}"
        );
    }
    #[test]
    fn new_asks_get_global_references_without_changing_local_aliases() {
        use crewd_store::object_refs::{self, ObjectKind};
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("synthetic A")));
        let b = ok(&mut s, |tx| open(tx, "w-b1", q("synthetic B")));
        assert_eq!(a.ask, "H1");
        assert_eq!(b.ask, "H1");
        let ra = object_refs::resolve(
            s.conn(),
            ObjectKind::HumanTask,
            &a.crew_id,
            &format!("work:{}", &a.ask),
        )
        .unwrap()
        .expect("new ask must have a durable global reference");
        let rb = object_refs::resolve(
            s.conn(),
            ObjectKind::HumanTask,
            &b.crew_id,
            &format!("work:{}", &b.ask),
        )
        .unwrap()
        .expect("other crew must have a durable global reference");
        assert_ne!(ra.number, rb.number);
        ok(&mut s, |tx| {
            answer(tx, &a.crew_id, &a.ask, "synthetic answer")
        });
        assert_eq!(
            object_refs::resolve(
                s.conn(),
                ObjectKind::HumanTask,
                &a.crew_id,
                &format!("work:{}", &a.ask)
            )
            .unwrap(),
            Some(ra)
        );
    }
    #[test]
    fn reference_write_failure_rolls_back_book_and_question_side_effects() {
        let mut s = store();
        let before: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        s.conn().execute_batch("CREATE TRIGGER reject_reference BEFORE INSERT ON object_refs BEGIN SELECT RAISE(ABORT,'synthetic reference fault'); END;").unwrap();
        let refused = err(&mut s, |tx| {
            open(tx, "w-a1", q("must not escape transaction"))
        });
        assert!(matches!(refused, CollabError::Store(_)));
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM work_books WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM object_refs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn a_book_stored_under_another_crew_is_refused_without_rebinding_or_resetting() {
        let mut s = store();
        let foreign =
            serde_json::to_string(WorkBook::new(CrewId::new("crew-b"), Config::default()).state())
                .unwrap();
        s.conn()
            .execute(
                "INSERT INTO work_books(crew_id,state,updated_at) VALUES('crew-a',?1,'t')",
                [&foreign],
            )
            .unwrap();
        let before: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        let refused = err(&mut s, |tx| {
            open(tx, "w-a1", q("wrong book must stay closed"))
        });
        assert!(matches!(refused, CollabError::Store(_)));
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT state FROM work_books WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            foreign
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM object_refs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
    }
    #[test]
    fn issued_work_references_prevent_a_missing_book_from_being_guessed_empty() {
        use crewd_store::object_refs::{self, ObjectKind};
        let mut s = store();
        let old = ok(&mut s, |tx| open(tx, "w-a1", q("original identity")));
        let reference = object_refs::resolve(
            s.conn(),
            ObjectKind::HumanTask,
            "crew-a",
            &format!("work:{}", &old.ask),
        )
        .unwrap()
        .unwrap();
        let before: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        s.conn()
            .execute("DELETE FROM work_books WHERE crew_id='crew-a'", [])
            .unwrap();
        s.conn()
            .execute("DELETE FROM ask_meta WHERE crew_id='crew-a'", [])
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| open(
                tx,
                "w-a1",
                q("must not reuse original alias")
            )),
            CollabError::Store(_)
        ));
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM work_books WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            object_refs::resolve(
                s.conn(),
                ObjectKind::HumanTask,
                "crew-a",
                &format!("work:{}", &old.ask)
            )
            .unwrap(),
            Some(reference)
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
    }
    #[test]
    fn replacing_a_book_cannot_retire_issued_identity_counters() {
        let mut s = store();
        ok(&mut s, |tx| open(tx, "w-a1", q("original")));
        let before: String = s
            .conn()
            .query_row(
                "SELECT state FROM work_books WHERE crew_id='crew-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| with_book(tx, "crew-a", |b, _| {
                *b = WorkBook::new(CrewId::new("crew-a"), Config::default());
                Ok(())
            })),
            CollabError::Store(_)
        ));
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT state FROM work_books WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            before
        );
    }
    #[test]
    fn legacy_question_metadata_also_prevents_empty_book_guessing() {
        let mut s = store();
        ok(&mut s, |tx| open(tx, "w-a1", q("legacy original")));
        s.conn()
            .execute("DELETE FROM object_refs WHERE crew_id='crew-a'", [])
            .unwrap();
        s.conn()
            .execute("DELETE FROM work_books WHERE crew_id='crew-a'", [])
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| open(tx, "w-a1", q("must stay missing"))),
            CollabError::Store(_)
        ));
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM work_books WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM ask_meta WHERE crew_id='crew-a'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
    #[test]
    fn successful_work_commands_persist_real_events_with_original_actor() {
        let mut s = store();
        ok(&mut s, |tx| open(tx, "w-a1", q("event witness")));
        let events = crewd_store::work_events::list(s.conn(), "crew-a", 0, 200).unwrap();
        assert_eq!(
            events.len(),
            1,
            "actual ask command must record its real event"
        );
        assert!(matches!(events[0].kind, EventKind::AskOpened { .. }));
        assert_eq!(events[0].by, Actor::worker("w-a1"));
        assert_eq!(events[0].at, Ts(crate::testkit::NOW));
    }
    #[test]
    fn history_failure_rolls_back_book_references_and_announcement() {
        let mut s = store();
        let before: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        s.conn().execute_batch("CREATE TRIGGER reject_work_event BEFORE INSERT ON work_events BEGIN SELECT RAISE(ABORT,'synthetic history fault'); END;").unwrap();
        assert!(matches!(
            err(&mut s, |tx| open(tx, "w-a1", q("must not commit"))),
            CollabError::Store(_)
        ));
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        for table in ["work_books", "work_events", "object_refs"] {
            assert_eq!(
                s.conn()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
}
