//! 机组：根、机长、DAG、建 / 改 / 删、编号。移植 `LocalCrewStore.swift:356-526`、`CrewDirectory.swift:8-13`。
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::Crew;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewCrew {
    pub title: String,
    pub working_directory: String,
    pub parent_id: Option<String>,
    pub title_source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrewPatch {
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub working_directory: Option<String>,
    pub hidden: Option<bool>,
    /// "" = 清掉。
    pub status_line: Option<String>,
    pub policy: Option<serde_json::Value>,
}

fn not_found(id: &str) -> CollabError {
    CollabError::NotFound(format!("没有机组 {id}"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Crew> {
    crewd_store::crews::get(conn, id)?.ok_or_else(|| not_found(id))
}

/// 侧栏顺序（C1）：position，再按建组时间、id。store 层仍按号排（M1 的测试依赖它）。
pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    let mut crews = crewd_store::crews::list(conn)?;
    crews.sort_by(|a, b| {
        a.position
            .cmp(&b.position)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(crews)
}

pub fn parents(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT p.parent_id FROM crew_parents p JOIN crews c ON c.id=p.parent_id
          WHERE p.crew_id=?1 ORDER BY c.created_at, c.id",
    )?;
    Ok(st
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

pub fn children(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT p.crew_id FROM crew_parents p JOIN crews c ON c.id=p.crew_id
          WHERE p.parent_id=?1 ORDER BY COALESCE(c.position,0), c.created_at, c.id",
    )?;
    Ok(st
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

pub fn is_root(conn: &Connection, id: &str) -> Result<bool> {
    Ok(parents(conn, id)?.is_empty())
}

fn descendants(conn: &Connection, id: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let mut queue = VecDeque::from([id.to_string()]);
    while let Some(c) = queue.pop_front() {
        for k in children(conn, &c)? {
            if out.insert(k.clone()) {
                queue.push_back(k);
            }
        }
    }
    Ok(out)
}

fn created_at(conn: &Connection, id: &str) -> Result<String> {
    Ok(
        conn.query_row("SELECT created_at FROM crews WHERE id=?1", [id], |r| {
            r.get(0)
        })?,
    )
}

/// 没有父边的祖先里 created_at 最早的（§13 Q1）。自己就是根 → 自己。脏数据成环时有 visited 守卫、退回自己。
pub fn root_of(conn: &Connection, id: &str) -> Result<String> {
    get(conn, id)?;
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([id.to_string()]);
    let mut roots: Vec<(String, String)> = Vec::new();
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c.clone()) {
            continue;
        }
        let ps = parents(conn, &c)?;
        if ps.is_empty() {
            roots.push((created_at(conn, &c)?, c));
        } else {
            queue.extend(ps);
        }
    }
    Ok(roots
        .into_iter()
        .min()
        .map(|(_, r)| r)
        .unwrap_or_else(|| id.to_string()))
}

pub fn captain_of(conn: &Connection, crew: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT captain_session_id FROM crews WHERE id=?1",
            [crew],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}

/// 分机号分配（终身不变、不回收，分机 1 = 机长不占号）：取机组的 `next_ext` 并加一。
/// `sessions::register`（worker 入组）与 `set_captain`（旧机长降级）共用这一处。
pub(crate) fn alloc_ext(conn: &Connection, crew: &str) -> Result<i64> {
    let ext: i64 = conn.query_row("SELECT next_ext FROM crews WHERE id=?1", [crew], |r| {
        r.get(0)
    })?;
    conn.execute("UPDATE crews SET next_ext=next_ext+1 WHERE id=?1", [crew])?;
    Ok(ext)
}

/// 机组此刻的侧栏次序（缺省 0）。
fn position_of(conn: &Connection, id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(position,0) FROM crews WHERE id=?1",
        [id],
        |r| r.get(0),
    )?)
}

fn refuse_builtin(conn: &Connection, ids: &[&str]) -> Result<()> {
    for id in ids {
        let builtin: Option<bool> = conn
            .query_row("SELECT builtin FROM crews WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        if builtin == Some(true) {
            return Err(CollabError::Forbidden(format!(
                "内建机组 {id} 不参与收编 / 摘出"
            )));
        }
    }
    Ok(())
}

fn next_crew_number(tx: &mut Tx<'_>) -> Result<i64> {
    let conn = tx.conn();
    let stored = crewd_store::meta_get(conn, "nextCrewNumber")?.and_then(|s| s.parse::<i64>().ok());
    let max: i64 = conn.query_row("SELECT COALESCE(MAX(number),0) FROM crews", [], |r| {
        r.get(0)
    })?;
    let n = stored.unwrap_or(0).max(max + 1);
    crewd_store::meta_set(conn, "nextCrewNumber", &(n + 1).to_string())?;
    Ok(n)
}

pub fn create(tx: &mut Tx<'_>, n: NewCrew) -> Result<Crew> {
    let title = n.title.trim().to_string();
    if title.is_empty() {
        return Err(CollabError::Invalid("机组名不能为空".into()));
    }
    let wd = n.working_directory.trim().to_string();
    if wd.is_empty() {
        return Err(CollabError::Invalid("工作目录不能为空".into()));
    }
    let number = next_crew_number(tx)?;
    let now = tx.now_ts();
    let crew = Crew {
        id: format!("crew-{}", uuid::Uuid::new_v4().simple()),
        title,
        number: Some(number),
        builtin: false,
        working_directory: Some(wd),
        title_source: n.title_source,
        created_at: now.clone(),
        updated_at: now.clone(),
        ..Default::default()
    };
    crewd_store::crews::upsert(tx.conn(), &crew, None)?;
    crewd_store::channels::ensure_lobby(tx.conn(), &crew.id, &now)?;
    put_last(tx, &crew.id, None)?;
    tx.note(Change::CrewChanged {
        crew_id: crew.id.clone(),
    });
    if let Some(p) = n.parent_id.as_deref() {
        adopt(tx, &crew.id, p)?;
    }
    get(tx.conn(), &crew.id)
}

pub fn update(tx: &mut Tx<'_>, id: &str, p: CrewPatch) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    let conn = tx.conn();
    let now = tx.now_ts();
    if let Some(t) = p.title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(CollabError::Invalid("机组名不能为空".into()));
        }
        if crew.builtin {
            return Err(CollabError::Forbidden("内建机组不能改名".into()));
        }
        conn.execute(
            "UPDATE crews SET title=?2, title_source=COALESCE(?3, title_source) WHERE id=?1",
            params![id, t, p.title_source],
        )?;
    }
    if let Some(wd) = p.working_directory {
        if wd.trim().is_empty() {
            return Err(CollabError::Invalid("工作目录不能为空".into()));
        }
        conn.execute(
            "UPDATE crews SET working_directory=?2 WHERE id=?1",
            params![id, wd.trim()],
        )?;
    }
    if let Some(h) = p.hidden {
        conn.execute(
            "UPDATE crews SET hidden_at=?2 WHERE id=?1",
            params![id, h.then(|| now.clone())],
        )?;
    }
    if let Some(line) = p.status_line {
        let line = line.trim().to_string();
        conn.execute(
            "UPDATE crews SET status_line=?2 WHERE id=?1",
            params![id, (!line.is_empty()).then_some(line)],
        )?;
    }
    if let Some(policy) = p.policy {
        conn.execute(
            "UPDATE crews SET policy=?2 WHERE id=?1",
            params![id, policy.to_string()],
        )?;
    }
    conn.execute(
        "UPDATE crews SET updated_at=?2 WHERE id=?1",
        params![id, now],
    )?;
    tx.note(Change::CrewChanged { crew_id: id.into() });
    get(tx.conn(), id)
}

/// 把 `crew`（整棵子树）挂到 `parent` 名下。父边 additive（DAG 多父保留），已是其子 → 不动。
pub fn adopt(tx: &mut Tx<'_>, crew: &str, parent: &str) -> Result<Crew> {
    refuse_builtin(tx.conn(), &[crew, parent])?;
    get(tx.conn(), crew)?;
    get(tx.conn(), parent)?;
    if crew == parent {
        return Err(CollabError::Invalid("机组不能挂在自己下面".into()));
    }
    if descendants(tx.conn(), crew)?.contains(parent) {
        return Err(CollabError::Invalid(format!(
            "{parent} 是 {crew} 的下级，挂上去会成环"
        )));
    }
    let ps = parents(tx.conn(), crew)?;
    if ps.iter().any(|p| p == parent) {
        return get(tx.conn(), crew);
    }
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, parent],
    )?;
    put_last(tx, crew, Some(parent))?;
    tx.note(Change::CrewChanged {
        crew_id: crew.into(),
    });
    tx.note(Change::CrewChanged {
        crew_id: parent.into(),
    });
    get(tx.conn(), crew)
}

/// 把 `from` 的直系子 `crew` 摘出；`to` 有值 = 先挂新边（过 adopt 的全部检查）再摘旧边。
pub fn release(tx: &mut Tx<'_>, crew: &str, from: &str, to: Option<&str>) -> Result<Crew> {
    let mut ids = vec![crew, from];
    ids.extend(to);
    refuse_builtin(tx.conn(), &ids)?;
    get(tx.conn(), crew)?;
    if !parents(tx.conn(), crew)?.iter().any(|p| p == from) {
        return Err(CollabError::Invalid(format!(
            "{crew} 不是 {from} 的直系子机组"
        )));
    }
    if to == Some(from) {
        return get(tx.conn(), crew);
    }
    if let Some(t) = to {
        adopt(tx, crew, t)?;
    }
    tx.conn().execute(
        "DELETE FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
        params![crew, from],
    )?;
    if to.is_none() && is_root(tx.conn(), crew)? {
        put_last(tx, crew, None)?;
    }
    tx.note(Change::CrewChanged {
        crew_id: crew.into(),
    });
    tx.note(Change::CrewChanged {
        crew_id: from.into(),
    });
    get(tx.conn(), crew)
}

/// 只能删非内建、且没有 desired=running session 的机组（spec §22.2）。子机组的父边随之消失。
pub fn delete(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    let crew = get(tx.conn(), id)?;
    if crew.builtin {
        return Err(CollabError::Forbidden("内建机组不能删".into()));
    }
    let running: i64 = tx.conn().query_row(
        "SELECT COUNT(*) FROM sessions WHERE crew_id=?1 AND desired='running'",
        [id],
        |r| r.get(0),
    )?;
    if running > 0 {
        return Err(CollabError::Conflict(format!(
            "机组 {id} 还有 {running} 个在跑的 session，先停掉"
        )));
    }
    let around: Vec<String> = parents(tx.conn(), id)?
        .into_iter()
        .chain(children(tx.conn(), id)?)
        .collect();
    tx.conn().execute("DELETE FROM crews WHERE id=?1", [id])?;
    tx.note(Change::CrewRemoved { crew_id: id.into() });
    for c in around {
        tx.note(Change::CrewChanged { crew_id: c });
    }
    Ok(())
}

/// 换机长（数据部分；进程与世界观由 M3c / M3b 处理）：旧机长降为 worker，没号就领下一个分机号。
pub fn set_captain(tx: &mut Tx<'_>, crew: &str, session: &str) -> Result<()> {
    get(tx.conn(), crew)?;
    let s = crate::sessions::get(tx.conn(), session)?;
    if s.crew_id != crew {
        return Err(CollabError::Invalid(format!(
            "{session} 不在机组 {crew} 里"
        )));
    }
    let old = captain_of(tx.conn(), crew)?;
    if old.as_deref() == Some(session) {
        return Ok(());
    }
    let conn = tx.conn();
    if let Some(o) = &old {
        let has_ext: Option<i64> =
            conn.query_row("SELECT ext FROM sessions WHERE id=?1", [o], |r| r.get(0))?;
        if has_ext.is_none() {
            let ext = alloc_ext(conn, crew)?;
            conn.execute("UPDATE sessions SET ext=?2 WHERE id=?1", params![o, ext])?;
        }
        conn.execute("UPDATE sessions SET role='worker' WHERE id=?1", [o])?;
    }
    conn.execute("UPDATE sessions SET role='captain' WHERE id=?1", [session])?;
    conn.execute(
        "UPDATE crews SET captain_session_id=?2 WHERE id=?1",
        params![crew, session],
    )?;
    if is_root(conn, crew)? {
        let human = crewd_store::channels::human_id(crew);
        let exists: bool =
            conn.query_row("SELECT COUNT(*) FROM channels WHERE id=?1", [&human], |r| {
                r.get::<_, i64>(0)
            })? > 0;
        if exists {
            cursors_pin(tx, session, &human)?;
            tx.note(Change::ChannelChanged { channel_id: human });
        }
    }
    tx.note(Change::CrewChanged {
        crew_id: crew.into(),
    });
    if let Some(o) = old {
        tx.note(Change::SessionChanged { session_id: o });
    }
    tx.note(Change::SessionChanged {
        session_id: session.into(),
    });
    Ok(())
}

fn cursors_pin(tx: &mut Tx<'_>, reader: &str, channel: &str) -> Result<()> {
    crate::cursors::pin_to_end(tx, reader, channel).map(|_| ())
}

/// 同级：`parent` 的直系子；`None` = 全部根机组（没有父边的）。按 position。
fn siblings(conn: &Connection, parent: Option<&str>) -> Result<Vec<String>> {
    match parent {
        Some(p) => children(conn, p),
        None => {
            let mut out = Vec::new();
            for c in list(conn)? {
                if is_root(conn, &c.id)? {
                    out.push(c.id);
                }
            }
            Ok(out)
        }
    }
}

/// 把 `crew` 放到 `parent` 这一层（`None` = 顶层）的末尾（C1：新建 / 收编 / 转挂进来的排最后）。
/// 多父机组只有一个 position，在每个父下都按它排（少见；人再拖一次即可）。
fn put_last(tx: &mut Tx<'_>, crew: &str, parent: Option<&str>) -> Result<()> {
    let mut max = 0i64;
    for c in siblings(tx.conn(), parent)? {
        if c != crew {
            max = max.max(position_of(tx.conn(), &c)?);
        }
    }
    tx.conn().execute(
        "UPDATE crews SET position=?2 WHERE id=?1",
        params![crew, max + 1],
    )?;
    Ok(())
}

/// 人手动排侧栏（C1，只有客户端 `crew.reorder` 调）：`ordered` 必须恰好是这一层现有的全部机组（防客户端拿着旧列表覆盖）。
/// 挪了位置的每个机组各发一条 `CrewChanged`。
pub fn reorder(tx: &mut Tx<'_>, parent: Option<&str>, ordered: &[String]) -> Result<Vec<Crew>> {
    if let Some(p) = parent {
        get(tx.conn(), p)?;
    }
    let current = siblings(tx.conn(), parent)?;
    let want: BTreeSet<&String> = ordered.iter().collect();
    let have: BTreeSet<&String> = current.iter().collect();
    if want.len() != ordered.len() || want != have {
        return Err(CollabError::Invalid(format!(
            "crewIds 必须恰好是这一层的全部机组（不重复）：现有 {current:?}"
        )));
    }
    for (i, id) in ordered.iter().enumerate() {
        let pos = i as i64 + 1;
        let old = position_of(tx.conn(), id)?;
        if old != pos {
            tx.conn()
                .execute("UPDATE crews SET position=?2 WHERE id=?1", params![id, pos])?;
            tx.note(Change::CrewChanged {
                crew_id: id.clone(),
            });
        }
    }
    ordered.iter().map(|id| get(tx.conn(), id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, ok, store};

    fn edges(s: &crewd_store::Store, crew: &str) -> Vec<String> {
        parents(s.conn(), crew).unwrap()
    }

    #[test]
    fn root_of_is_the_earliest_root_among_ancestors() {
        let mut s = store();
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert_eq!(root_of(s.conn(), "crew-a").unwrap(), "crew-a");
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(
            root_of(s.conn(), "crew-b").unwrap(),
            "crew-a",
            "crew-a 建得更早"
        );
        s.conn()
            .execute(
                "UPDATE crews SET created_at='2026-08-15T00:00:00.000Z' WHERE id='crew-x'",
                [],
            )
            .unwrap();
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-x");
    }

    #[test]
    fn adopt_refuses_cycles_self_and_builtin_and_is_idempotent() {
        let mut s = store();
        assert!(
            matches!(
                err(&mut s, |tx| adopt(tx, "crew-a", "crew-b")),
                CollabError::Invalid(_)
            ),
            "成环"
        );
        assert!(
            matches!(
                err(&mut s, |tx| adopt(tx, "crew-a", "crew-a")),
                CollabError::Invalid(_)
            ),
            "自挂"
        );
        assert!(matches!(
            err(&mut s, |tx| adopt(tx, "chief", "crew-a")),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| adopt(tx, "crew-a", "chief")),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| adopt(tx, "crew-a", "nope")),
            CollabError::NotFound(_)
        ));
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        assert_eq!(edges(&s, "crew-x"), ["crew-b"]);
    }

    #[test]
    fn release_only_from_a_direct_parent() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "crew-x", None)),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "chief", None)),
            CollabError::Forbidden(_)
        ));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", None));
        assert!(edges(&s, "crew-b").is_empty());
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-b");
    }

    #[test]
    fn release_to_moves_the_edge() {
        let mut s = store();
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("crew-x")));
        assert_eq!(edges(&s, "crew-b"), ["crew-x"]);
        ok(&mut s, |tx| release(tx, "crew-b", "crew-x", Some("crew-x")));
        assert_eq!(edges(&s, "crew-b"), ["crew-x"], "转挂给原父 = 不动");
    }

    #[test]
    fn release_to_failure_leaves_no_side_effect() {
        let mut s = store();
        let c = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "孙机组".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("crew-b".into()),
                    ..Default::default()
                },
            )
        });
        // crew-b 转挂到自己的子机组下面 = 成环：整体失败，旧边还在。
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "crew-a", Some(&c.id))),
            CollabError::Invalid(_)
        ));
        assert_eq!(edges(&s, "crew-b"), ["crew-a"]);
        assert_eq!(edges(&s, &c.id), ["crew-b"]);
    }

    #[test]
    fn create_numbers_never_repeat_and_each_crew_gets_a_lobby() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "新机组".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(a.number, Some(4));
        assert_eq!(
            count(
                &s,
                &format!("SELECT COUNT(*) FROM channels WHERE id='{}/lobby'", a.id)
            ),
            1
        );
        ok(&mut s, |tx| delete(tx, &a.id));
        let b = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "再建一个".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(b.number, Some(5), "删掉的 4 号不回收");
        assert!(matches!(
            err(&mut s, |tx| create(
                tx,
                NewCrew {
                    title: "  ".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                }
            )),
            CollabError::Invalid(_)
        ));
        let c = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "子".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("crew-x".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(edges(&s, &c.id), ["crew-x"]);
    }

    #[test]
    fn delete_refuses_builtin_and_crews_with_running_sessions() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| delete(tx, "chief")),
            CollabError::Forbidden(_)
        ));
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", [])
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| delete(tx, "crew-x")),
            CollabError::Conflict(_)
        ));
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='cap-x'", [])
            .unwrap();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| delete(tx, "crew-x"));
        assert!(ch.contains(&Change::CrewRemoved {
            crew_id: "crew-x".into()
        }));
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM sessions WHERE crew_id='crew-x'"),
            0
        );
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM channels WHERE crew_id='crew-x'"),
            0
        );
    }

    #[test]
    fn set_captain_swaps_roles_and_gives_the_old_captain_a_fresh_extension() {
        let mut s = store();
        ok(&mut s, |tx| set_captain(tx, "crew-a", "w-a1"));
        assert_eq!(
            captain_of(s.conn(), "crew-a").unwrap().as_deref(),
            Some("w-a1")
        );
        assert_eq!(
            count(&s, "SELECT ext FROM sessions WHERE id='cap-a'"),
            4,
            "旧机长拿下一个分机号"
        );
        assert_eq!(count(&s, "SELECT next_ext FROM crews WHERE id='crew-a'"), 5);
        assert_eq!(
            crate::sessions::role_of(s.conn(), "cap-a").unwrap(),
            crewd_proto::Role::Worker
        );
        assert_eq!(
            crate::sessions::role_of(s.conn(), "w-a1").unwrap(),
            crewd_proto::Role::Captain
        );
        assert!(
            matches!(
                err(&mut s, |tx| set_captain(tx, "crew-a", "w-b1")),
                CollabError::Invalid(_)
            ),
            "别的机组的人"
        );
    }

    #[test]
    fn set_captain_pins_the_new_captain_to_the_end_of_an_existing_human_channel() {
        let mut s = store();
        let human = crewd_store::channels::human_id("crew-a");
        s.conn()
            .execute(
                "INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,'crew-a','human','','system','2026-09-01T00:00:00.000Z')",
                [&human],
            )
            .unwrap();
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message {
                id: "m-h".into(),
                channel_id: human.clone(),
                text: "示例消息".into(),
                created_at: "2026-09-21T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let end = crewd_store::messages::max_seq(s.conn(), &human).unwrap();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| set_captain(tx, "crew-a", "w-a1"));
        assert_eq!(
            crate::cursors::cursor(s.conn(), "w-a1", &human).unwrap(),
            Some(end),
            "新机长不凭空多出人类群的历史未读"
        );
        assert!(ch.contains(&Change::ChannelChanged { channel_id: human }));
    }

    fn order(s: &crewd_store::Store, parent: Option<&str>) -> Vec<String> {
        match parent {
            Some(p) => children(s.conn(), p).unwrap(),
            None => list(s.conn())
                .unwrap()
                .into_iter()
                .filter(|c| is_root(s.conn(), &c.id).unwrap())
                .map(|c| c.id)
                .collect(),
        }
    }

    #[test]
    fn new_crews_go_last_among_their_siblings_and_only_reorder_moves_them() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "新子甲".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("crew-a".into()),
                    ..Default::default()
                },
            )
        });
        let b = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "新子乙".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("crew-a".into()),
                    ..Default::default()
                },
            )
        });
        let kids = order(&s, Some("crew-a"));
        assert_eq!(
            &kids[kids.len() - 2..],
            [a.id.clone(), b.id.clone()],
            "新建的排在同级末尾"
        );
        // 有新消息不改顺序（没有任何按动静排序的东西，C1）。
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message {
                id: "m-x".into(),
                channel_id: format!("{}/lobby", a.id),
                text: "有动静".into(),
                created_at: "2026-09-21T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(order(&s, Some("crew-a")), kids);
        let mut want = kids.clone();
        want.reverse();
        let (got, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| reorder(tx, Some("crew-a"), &want));
        assert_eq!(got.into_iter().map(|c| c.id).collect::<Vec<_>>(), want);
        assert_eq!(order(&s, Some("crew-a")), want);
        assert!(ch.iter().all(|c| matches!(c, Change::CrewChanged { .. })) && !ch.is_empty());
        let mut partial = want.clone();
        partial.pop();
        assert!(
            matches!(
                err(&mut s, |tx| reorder(tx, Some("crew-a"), &partial)),
                CollabError::Invalid(_)
            ),
            "必须给出这一层的全部机组"
        );
        let mut dup = want.clone();
        dup[0] = dup[1].clone();
        assert!(matches!(
            err(&mut s, |tx| reorder(tx, Some("crew-a"), &dup)),
            CollabError::Invalid(_)
        ));
        // 转挂到别处 = 新同级的末尾。
        ok(&mut s, |tx| release(tx, &a.id, "crew-a", Some("crew-x")));
        assert_eq!(order(&s, Some("crew-x")).last(), Some(&a.id));
        // 顶层也能排。
        let mut roots = order(&s, None);
        roots.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, None, &roots));
        assert_eq!(order(&s, None), roots);
    }
}
