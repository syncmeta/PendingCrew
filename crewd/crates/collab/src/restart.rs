//! 重启后问人是否恢复上次在跑的 session（§22.7 Q1）。这里只记账与公告；恢复由 M3c 订阅 `RestartAnswered` 后做。
use crate::messages;
use crate::{
    Change, CollabError, Result, Sender, Tx, crews, hibernate, prompts, sessions, settings,
};
use crewd_proto::{RestartAnswer, RestartPrompt, RestartReason};
use rusqlite::{Connection, OptionalExtension, params};

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RestartPrompt> {
    let ids: String = r.get(2)?;
    let answer: Option<String> = r.get(3)?;
    Ok(RestartPrompt {
        id: r.get(0)?,
        created_at: r.get(1)?,
        session_ids: serde_json::from_str(&ids).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?,
        answer: answer.as_deref().and_then(RestartAnswer::parse),
        answered_at: r.get(4)?,
        reason: RestartReason::Restart,
        interrupted: Vec::new(),
    })
}

/// 休眠记录；坏了（读不出来）只记日志、当没有（评审 m3：不能因为它把重启询问的读写弄坏，按普通重启问）。
fn record_or_none(conn: &Connection) -> Option<hibernate::Record> {
    hibernate::get(conn).unwrap_or_else(|e| {
        eprintln!("crewd: 协作：休眠记录读不出来，按普通重启算：{e}");
        None
    })
}

/// 休眠记录（[`hibernate`]，C26）指着这条询问 → 标成 `hibernate`、带被打断的名单（只列这条询问里的）。
fn with_reason(conn: &Connection, mut p: RestartPrompt) -> Result<RestartPrompt> {
    if let Some(r) = record_or_none(conn)
        && r.prompt_id == Some(p.id)
    {
        p.reason = RestartReason::Hibernate;
        p.interrupted = r
            .sessions
            .iter()
            .filter(|h| h.interrupted() && p.session_ids.contains(&h.session_id))
            .map(|h| h.session_id.clone())
            .collect();
    }
    Ok(p)
}

const COLS: &str = "id,created_at,session_ids,answer,answered_at";

pub fn get(conn: &Connection, id: i64) -> Result<RestartPrompt> {
    let p = conn
        .query_row(
            &format!("SELECT {COLS} FROM restart_prompts WHERE id=?1"),
            [id],
            row,
        )
        .optional()?
        .ok_or_else(|| CollabError::NotFound(format!("没有重启询问 {id}")))?;
    with_reason(conn, p)
}

/// 还要人拍板的那一条（没答，或答了「先不」）。
pub fn pending(conn: &Connection) -> Result<Option<RestartPrompt>> {
    let p = conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM restart_prompts WHERE answer IS NULL OR answer='later' ORDER BY id DESC LIMIT 1"
            ),
            [],
            row,
        )
        .optional()?;
    p.map(|p| with_reason(conn, p)).transpose()
}

/// crewd 启动时（M3c）给出上次在跑的 session：去重、保序；空 = 不问。新的一次重启取代所有还没定下来的询问。
pub fn open(tx: &mut Tx<'_>, session_ids: Vec<String>) -> Result<Option<RestartPrompt>> {
    open_with(tx, session_ids, RestartReason::Restart)
}

/// 同 [`open`]，说明为什么问（C26）：`Hibernate` = 上次是人让 crewd 休眠的——公告换成「上次休眠了 N 个」，
/// 休眠记录（[`hibernate`]）记下这条询问的 id（没有记录就补一份空的），之后读这条询问都标成 `hibernate`。
pub fn open_with(
    tx: &mut Tx<'_>,
    session_ids: Vec<String>,
    reason: RestartReason,
) -> Result<Option<RestartPrompt>> {
    let mut ids: Vec<String> = Vec::new();
    for id in session_ids {
        sessions::get(tx.conn(), &id)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(None);
    }
    // 新的一次重启取代所有还没定下来的（没答的、答了「先不」的）：旧名单作废，不会在答完新的之后又冒出来。
    let superseded: Vec<i64> = {
        let mut st = tx
            .conn()
            .prepare("SELECT id FROM restart_prompts WHERE answer IS NULL OR answer='later'")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.conn().execute(
        "DELETE FROM restart_prompts WHERE answer IS NULL OR answer='later'",
        [],
    )?;
    for old in superseded {
        tx.note(Change::RestartChanged { id: old });
    }
    tx.conn().execute(
        "INSERT INTO restart_prompts(created_at,session_ids) VALUES(?1,?2)",
        params![tx.now_ts(), serde_json::to_string(&ids)?],
    )?;
    let id = tx.conn().last_insert_rowid();
    // 休眠停下、还没恢复过的（公告里「上次休眠了 N 个」只数它们，评审 m4）。
    let mut hibernated: Vec<String> = Vec::new();
    if reason == RestartReason::Hibernate {
        let mut rec = record_or_none(tx.conn()).unwrap_or_else(|| hibernate::Record {
            at_ms: tx.now_ms(),
            ..Default::default()
        });
        rec.prompt_id = Some(id);
        hibernate::put(tx, &rec)?;
        hibernated = rec
            .sessions
            .iter()
            .filter(|h| !h.resumed)
            .map(|h| h.session_id.clone())
            .collect();
    }
    // 每个涉及的根机组人类群各一条，只列那棵树里的 session。
    let f = prompts::render_file(settings::locale(tx.conn())?);
    let mut by_root: Vec<(String, Vec<String>)> = Vec::new();
    for sid in &ids {
        let root = crews::root_of(tx.conn(), &sessions::crew_of(tx.conn(), sid)?)?;
        match by_root.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(sid.clone()),
            None => by_root.push((root, vec![sid.clone()])),
        }
    }
    for (root, sids) in by_root {
        // 这棵树里有休眠停下的 → 「上次休眠了 N 个」（N 只数它们）；没有 → 普通公告。
        let n = sids.iter().filter(|s| hibernated.contains(s)).count();
        let (head, count) = if n > 0 {
            ("notice.restart.hibernate", n)
        } else {
            ("notice.restart", sids.len())
        };
        let mut text = prompts::fill(f.get(head), &[("count", &count.to_string())]);
        for sid in &sids {
            text.push('\n');
            text.push_str(&prompts::fill(
                f.get("notice.restart.item"),
                // 名字同注入面一样过 `safe_name`（终审复审 5）。
                &[(
                    "name",
                    &crate::render::safe_name(&sessions::display_name(tx.conn(), sid)?),
                )],
            ));
        }
        messages::announce_to_human(tx, &root, Sender::System, text, "restart")?;
    }
    tx.note(Change::RestartChanged { id });
    Ok(Some(get(tx.conn(), id)?))
}

/// 人答重启询问（客户端 `restart.answer`）。「先不」之后还能答「恢复」；答过「恢复」就定了。
/// 只有最新那条还没定下来的（`pending`）能答。
pub fn answer(tx: &mut Tx<'_>, id: i64, a: RestartAnswer) -> Result<RestartPrompt> {
    let p = get(tx.conn(), id)?;
    match (p.answer, a) {
        (Some(RestartAnswer::Resume), _) => {
            return Err(CollabError::Conflict("已经答过「恢复」".into()));
        }
        (Some(RestartAnswer::Later), RestartAnswer::Later) => return Ok(p),
        _ => {}
    }
    if pending(tx.conn())?.map(|x| x.id) != Some(id) {
        return Err(CollabError::Conflict(format!(
            "重启询问 {id} 已被更新的一条取代"
        )));
    }
    tx.conn().execute(
        "UPDATE restart_prompts SET answer=?2, answered_at=?3 WHERE id=?1",
        params![id, a.as_str(), tx.now_ts()],
    )?;
    tx.note(Change::RestartChanged { id });
    tx.note(Change::RestartAnswered {
        id,
        answer: a,
        session_ids: p.session_ids.clone(),
    });
    get(tx.conn(), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, ok_at, store};
    use crewd_proto::RestartReason;

    #[test]
    fn open_announces_per_root_and_nothing_to_resume_means_no_prompt() {
        let mut s = store();
        assert_eq!(ok(&mut s, |tx| open(tx, vec![])), None);
        let p = ok(&mut s, |tx| {
            open(
                tx,
                vec!["w-a1".into(), "w-b1".into(), "cap-x".into(), "w-a1".into()],
            )
        })
        .unwrap();
        assert_eq!(p.session_ids, ["w-a1", "w-b1", "cap-x"], "去重、保持顺序");
        let a = crate::messages::list(s.conn(), "crew-a/human", None, None, 10).unwrap();
        assert_eq!(a.len(), 1, "crew-a 与 crew-b 同根，一条公告");
        assert!(a[0].text.contains("名字-w-a1") && a[0].text.contains("名字-w-b1"));
        assert_eq!(
            crate::messages::list(s.conn(), "crew-x/human", None, None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(pending(s.conn()).unwrap().unwrap().id, p.id);
        assert!(matches!(
            err(&mut s, |tx| open(tx, vec!["ghost".into()])),
            CollabError::NotFound(_)
        ));
    }

    #[test]
    fn later_can_become_resume_but_not_the_other_way() {
        let mut s = store();
        let p = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        let (l, ch) = ok_at(&mut s, NOW, |tx| answer(tx, p.id, RestartAnswer::Later));
        assert_eq!(l.answer, Some(RestartAnswer::Later));
        assert!(ch.contains(&Change::RestartAnswered {
            id: p.id,
            answer: RestartAnswer::Later,
            session_ids: vec!["w-a1".into()]
        }));
        assert_eq!(
            pending(s.conn()).unwrap().map(|x| x.id),
            Some(p.id),
            "「先不」之后人仍可整批恢复"
        );
        let r = ok(&mut s, |tx| answer(tx, p.id, RestartAnswer::Resume));
        assert_eq!(r.answer, Some(RestartAnswer::Resume));
        assert!(pending(s.conn()).unwrap().is_none());
        assert!(matches!(
            err(&mut s, |tx| answer(tx, p.id, RestartAnswer::Later)),
            CollabError::Conflict(_)
        ));
    }

    #[test]
    fn a_new_restart_replaces_an_unanswered_prompt() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        let b = ok(&mut s, |tx| open(tx, vec!["w-a2".into()])).unwrap();
        assert!(matches!(
            err(&mut s, |tx| get(tx.conn(), a.id)),
            CollabError::NotFound(_)
        ));
        assert_eq!(pending(s.conn()).unwrap().unwrap().id, b.id);
    }

    #[test]
    fn a_new_restart_supersedes_a_later_prompt_too() {
        let mut s = store();
        let p1 = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        ok(&mut s, |tx| answer(tx, p1.id, RestartAnswer::Later));
        let (p2, ch) = ok_at(&mut s, NOW, |tx| open(tx, vec!["w-a2".into()]));
        let p2 = p2.unwrap();
        assert!(
            ch.contains(&Change::RestartChanged { id: p1.id }),
            "被取代的那条也发变更：{ch:?}"
        );
        assert!(ch.contains(&Change::RestartChanged { id: p2.id }));
        ok(&mut s, |tx| answer(tx, p2.id, RestartAnswer::Resume));
        assert!(pending(s.conn()).unwrap().is_none(), "旧的「先不」不会复活");
        assert!(matches!(
            err(&mut s, |tx| answer(tx, p1.id, RestartAnswer::Resume)),
            CollabError::NotFound(_) | CollabError::Conflict(_)
        ));
    }

    fn hibernated(
        id: &str,
        was_working: bool,
        outcome: crate::hibernate::Outcome,
    ) -> crate::hibernate::Hibernated {
        crate::hibernate::Hibernated {
            session_id: id.into(),
            was_working,
            outcome,
            ..Default::default()
        }
    }

    /// C26：休眠之后的那次询问——公告说「上次休眠了 N 个」（N 只数休眠停下的，评审 m4；名单里没有休眠的那棵树发普通
    /// 公告），载荷 `reason = hibernate`、带被打断的名单（评审 I3：休眠中途崩了、还停在「等安全点」的也算被打断），
    /// 与崩溃重启分得开；之后一次普通重启开的询问又是 `restart`。
    #[test]
    fn a_prompt_after_hibernate_says_so_in_the_notice_and_the_payload() {
        use crate::hibernate::Outcome;
        let mut s = store();
        let rec = crate::hibernate::Record {
            at_ms: NOW,
            sessions: vec![
                hibernated("w-a1", true, Outcome::Interrupted),
                hibernated("w-a2", true, Outcome::Finished),
                hibernated("w-b1", true, Outcome::Pending),
            ],
            ..Default::default()
        };
        ok(&mut s, |tx| crate::hibernate::put(tx, &rec));
        let ids: Vec<String> = ["w-a1", "w-a2", "w-b1", "cap-a", "cap-x"]
            .map(String::from)
            .to_vec();
        let p = ok(&mut s, |tx| open_with(tx, ids, RestartReason::Hibernate)).unwrap();
        assert_eq!(p.reason, RestartReason::Hibernate);
        assert_eq!(
            p.interrupted,
            ["w-a1", "w-b1"],
            "还停在「等安全点」的（休眠中途崩了）也算被打断"
        );
        assert_eq!(
            crate::hibernate::get(s.conn()).unwrap().unwrap().prompt_id,
            Some(p.id),
            "记录记下了是哪条询问"
        );
        let f = prompts::render_file(crate::Locale::Zh);
        let head = prompts::fill(f.get("notice.restart.hibernate"), &[("count", "3")]);
        let a = crate::messages::list(s.conn(), "crew-a/human", None, None, 10).unwrap();
        assert!(
            a[0].text.starts_with(&head),
            "只数休眠停下的（cap-a 不算）：{}",
            a[0].text
        );
        let x = crate::messages::list(s.conn(), "crew-x/human", None, None, 10).unwrap();
        let plain1 = prompts::fill(f.get("notice.restart"), &[("count", "1")]);
        assert!(
            x[0].text.starts_with(&plain1),
            "那棵树里没有休眠停下的：普通公告：{}",
            x[0].text
        );
        assert_eq!(
            pending(s.conn()).unwrap().unwrap(),
            p,
            "pending 也带着休眠的标记"
        );
        assert_eq!(
            ok(&mut s, |tx| answer(tx, p.id, RestartAnswer::Later)).reason,
            RestartReason::Hibernate,
            "答过之后照样是休眠的那条"
        );
        let q = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        assert_eq!(q.reason, RestartReason::Restart, "普通重启：不是休眠");
        assert!(q.interrupted.is_empty());
        let a = crate::messages::list(s.conn(), "crew-a/human", None, None, 10).unwrap();
        assert!(a.last().unwrap().text.starts_with(&plain1));
    }

    /// 评审 m3：休眠记录坏了（读不出来）不能把重启询问弄坏——按普通重启算、记一行日志；get / pending / answer /
    /// 开新询问（含说是休眠的）都照常。
    #[test]
    fn a_corrupt_hibernate_record_degrades_to_a_plain_restart() {
        let mut s = store();
        crewd_store::meta_set(s.conn(), "hibernate", "{坏").unwrap();
        let p = ok(&mut s, |tx| {
            open_with(tx, vec!["w-a1".into()], RestartReason::Hibernate)
        })
        .unwrap();
        assert_eq!(
            get(s.conn(), p.id).unwrap().reason,
            RestartReason::Hibernate,
            "坏记录被一份新的盖掉"
        );
        crewd_store::meta_set(s.conn(), "hibernate", "{坏").unwrap();
        assert_eq!(get(s.conn(), p.id).unwrap().reason, RestartReason::Restart);
        assert_eq!(pending(s.conn()).unwrap().unwrap().id, p.id);
        let r = ok(&mut s, |tx| answer(tx, p.id, RestartAnswer::Resume));
        assert_eq!(r.reason, RestartReason::Restart);
    }

    #[test]
    fn only_the_newest_unresolved_prompt_can_be_answered() {
        let mut s = store();
        // 绕过 open 造出两条都没决的旧数据（open 自己不会留下这种状态）。
        s.conn().execute("INSERT INTO restart_prompts(created_at,session_ids,answer) VALUES('t','[\"w-a1\"]','later')", []).unwrap();
        let old = s.conn().last_insert_rowid();
        s.conn()
            .execute(
                "INSERT INTO restart_prompts(created_at,session_ids) VALUES('t','[\"w-a2\"]')",
                [],
            )
            .unwrap();
        let new = s.conn().last_insert_rowid();
        assert!(matches!(
            err(&mut s, |tx| answer(tx, old, RestartAnswer::Resume)),
            CollabError::Conflict(_)
        ));
        assert_eq!(
            ok(&mut s, |tx| answer(tx, new, RestartAnswer::Resume)).answer,
            Some(RestartAnswer::Resume)
        );
    }
}
