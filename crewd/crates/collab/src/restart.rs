//! 重启后问人是否恢复上次在跑的 session（§22.7 Q1）。这里只记账与公告；恢复由 M3c 订阅 `RestartAnswered` 后做。
use crate::messages;
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Sender, Tx, crews, prompts, sessions, settings};
use crewd_proto::{RestartAnswer, RestartPrompt};
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
    })
}

const COLS: &str = "id,created_at,session_ids,answer,answered_at";

pub fn get(conn: &Connection, id: i64) -> Result<RestartPrompt> {
    conn.query_row(
        &format!("SELECT {COLS} FROM restart_prompts WHERE id=?1"),
        [id],
        row,
    )
    .optional()?
    .ok_or_else(|| {
        CollabError::NotFound(Refusal::new(Code::RestartNotFound).arg("id", id.to_string()))
    })
}

/// 还要人拍板的那一条（没答，或答了「先不」）。
pub fn pending(conn: &Connection) -> Result<Option<RestartPrompt>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM restart_prompts WHERE answer IS NULL OR answer='later' ORDER BY id DESC LIMIT 1"
            ),
            [],
            row,
        )
        .optional()?)
}

/// crewd 启动时（M3c）给出上次在跑的 session：去重、保序；空 = 不问。新的一次重启取代所有还没定下来的询问。
pub fn open(tx: &mut Tx<'_>, session_ids: Vec<String>) -> Result<Option<RestartPrompt>> {
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
    // 每个涉及的 home（总机组 / 顶层机组）人类群各一条，只列 home 是它的 session。
    let f = prompts::render_file(settings::locale(tx.conn())?);
    let mut by_home: Vec<(String, Vec<String>)> = Vec::new();
    for sid in &ids {
        let home = crews::home_of(tx.conn(), &sessions::crew_of(tx.conn(), sid)?)?;
        match by_home.iter_mut().find(|(h, _)| *h == home) {
            Some((_, v)) => v.push(sid.clone()),
            None => by_home.push((home, vec![sid.clone()])),
        }
    }
    for (home, sids) in by_home {
        let mut text = prompts::fill(
            f.get("notice.restart"),
            &[("count", &sids.len().to_string())],
        );
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
        messages::announce_to_human(tx, &home, Sender::System, text, "restart")?;
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
            return Err(CollabError::Conflict(Refusal::new(
                Code::RestartAlreadyResumed,
            )));
        }
        (Some(RestartAnswer::Later), RestartAnswer::Later) => return Ok(p),
        _ => {}
    }
    if pending(tx.conn())?.map(|x| x.id) != Some(id) {
        return Err(CollabError::Conflict(
            Refusal::new(Code::RestartSuperseded).arg("id", id.to_string()),
        ));
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

    #[test]
    fn a_restart_prompt_is_announced_in_each_top_level_human_channel() {
        let mut s = store();
        ok(&mut s, |tx| open(tx, vec!["w-b1".into(), "cap-x".into()]));
        let n = |ch: &str| {
            crate::messages::list(s.conn(), ch, None, None, 10)
                .map(|v| v.len())
                .unwrap_or(0)
        };
        assert_eq!((n("crew-a/human"), n("crew-x/human")), (1, 1));
        assert!(!crate::channels::human_channel_exists(s.conn(), "chief").unwrap());
    }

    #[test]
    fn open_announces_per_home_and_nothing_to_resume_means_no_prompt() {
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
        assert_eq!(a.len(), 1, "crew-a 与 crew-b 同一个 home，一条公告");
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
