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

/// 直系子机组，按侧栏顺序（边上的 position，再按建组时间、id）。与 `Crew.child_ids` 同一处定义。
pub fn children(conn: &Connection, id: &str) -> Result<Vec<String>> {
    Ok(crewd_store::crews::children_of(conn, id)?)
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
            .query_row("SELECT org_root FROM crews WHERE id=?1", [id], |r| r.get(0))
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
        org_root: false,
        working_directory: Some(wd),
        title_source: n.title_source,
        created_at: now.clone(),
        updated_at: now.clone(),
        ..Default::default()
    };
    crewd_store::crews::upsert(tx.conn(), &crew, None)?;
    crewd_store::channels::ensure_lobby(tx.conn(), &crew.id, &now)?;
    match n.parent_id.as_deref() {
        // 有父的直接排在父下的末尾（adopt 做），不占顶层的次序。
        Some(p) => {
            tx.note(Change::CrewChanged {
                crew_id: crew.id.clone(),
            });
            adopt(tx, &crew.id, p)?;
        }
        None => {
            put_last(tx, &crew.id, None)?;
            tx.note(Change::CrewChanged {
                crew_id: crew.id.clone(),
            });
        }
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
        if crew.org_root {
            return Err(CollabError::Forbidden("内建机组不能改名".into()));
        }
        conn.execute("UPDATE crews SET title=?2 WHERE id=?1", params![id, t])?;
    }
    // 谁起的名与改不改名无关：只给 titleSource 也要落（不能悄悄丢）。
    if let Some(ts) = p.title_source {
        conn.execute(
            "UPDATE crews SET title_source=?2 WHERE id=?1",
            params![id, ts],
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
    let was_root = ps.is_empty();
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, parent],
    )?;
    if was_root {
        crate::channels::archive_human_of(tx, crew)?;
    }
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
    // 不存在的父 = NotFound（控制者裁定 e），与「存在但不是直系父」（Invalid）分开。
    get(tx.conn(), from)?;
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
    // 又成了根：原来就有的人类群解档（没有的仍然懒建）。
    if is_root(tx.conn(), crew)? && crate::channels::human_channel_exists(tx.conn(), crew)? {
        crate::channels::ensure_human_channel(tx, crew)?;
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
/// 删除级联（人显式的操作）：本机组的 session、群（含跨机组 topic 及其历史）一起删掉，每个群发
/// ChannelRemoved；它的 session 在别的机组群里的成员 / 群主身份先交出去（`release_sessions_of_doomed_crew`）。
pub fn delete(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    let crew = get(tx.conn(), id)?;
    if crew.org_root {
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
    let kids = children(tx.conn(), id)?;
    // 只有这一个父的子机组删后变成根：按原先的次序排到顶层末尾（C1）。
    let mut orphans = Vec::new();
    for k in &kids {
        if parents(tx.conn(), k)? == [id] {
            orphans.push(k.clone());
        }
    }
    let around: Vec<String> = parents(tx.conn(), id)?.into_iter().chain(kids).collect();
    // 级联之前：它的 session 退出别的机组的群、交出群主（终审 I4）。
    let cascaded = crate::channels::release_sessions_of_doomed_crew(tx, id)?;
    tx.conn().execute("DELETE FROM crews WHERE id=?1", [id])?;
    for channel_id in cascaded {
        tx.note(Change::ChannelRemoved { channel_id });
    }
    for k in &orphans {
        put_last(tx, k, None)?;
        // 又成了根：原来就有的人类群解档（与 release 同一个做法；没有的仍然懒建）。
        if crate::channels::human_channel_exists(tx.conn(), k)? {
            crate::channels::ensure_human_channel(tx, k)?;
        }
    }
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
    point_captain_at(tx, crew, session)?;
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

/// 「成为机长」的公共一处（`set_captain` 与 `sessions::register(Role::Captain)` 共用）：机长指针指向 `session`，
/// 大厅发 ChannelChanged（群主随机长算）；根机组且人类群已存在时，把新机长的游标钉到人类群末尾（不凭空多出一屏历史未读，裁定 9）。
/// 调用方负责 session 行的 role 与分机号。
pub(crate) fn point_captain_at(tx: &mut Tx<'_>, crew: &str, session: &str) -> Result<()> {
    tx.conn().execute(
        "UPDATE crews SET captain_session_id=?2 WHERE id=?1",
        params![crew, session],
    )?;
    // 大厅的群主是算出来的（= 机长）：指针一动，大厅就变了（终审 M1）。
    tx.note(Change::ChannelChanged {
        channel_id: crewd_store::channels::lobby_id(crew),
    });
    if is_root(tx.conn(), crew)? {
        let human = crewd_store::channels::human_id(crew);
        let exists = crate::channels::human_channel_exists(tx.conn(), crew)?;
        if exists {
            crate::cursors::pin_to_end(tx, session, &human)?;
            tx.note(Change::ChannelChanged { channel_id: human });
        }
    }
    Ok(())
}

pub fn detail(tx: &mut Tx<'_>, id: &str) -> Result<crewd_proto::CrewDetail> {
    let crew = get(tx.conn(), id)?;
    Ok(crewd_proto::CrewDetail {
        root_id: root_of(tx.conn(), id)?,
        children: children(tx.conn(), id)?,
        channels: crate::channels::list_for_crew(tx, id)?,
        sessions: crate::sessions::list(tx.conn(), Some(id))?,
        crew,
    })
}

/// 同级：`parent` 的直系子（按它们在这个父下的边次序）；`None` = 顶层的可见根机组（没有父边、非内建；
/// 内建机组不参与排序，客户端给的顶层名单里也没有它），按 `crews.position`。
fn siblings(conn: &Connection, parent: Option<&str>) -> Result<Vec<String>> {
    match parent {
        Some(p) => children(conn, p),
        None => {
            let mut out = Vec::new();
            for c in list(conn)? {
                if !c.org_root && is_root(conn, &c.id)? {
                    out.push(c.id);
                }
            }
            Ok(out)
        }
    }
}

/// 把 `crew` 放到 `parent` 这一层（`None` = 顶层）的末尾（C1：新建 / 收编 / 转挂进来的排最后）。
/// `Some(p)` 读写 (crew, p) 这条边的 position，只和 p 的其它边比；多父机组在每个父下各有自己的次序，互不影响。
/// 调用时边必须已经存在。`None` 写 `crews.position`（只管顶层）。
fn put_last(tx: &mut Tx<'_>, crew: &str, parent: Option<&str>) -> Result<()> {
    match parent {
        Some(p) => {
            let max: i64 = tx.conn().query_row(
                "SELECT COALESCE(MAX(position),0) FROM crew_parents WHERE parent_id=?1 AND crew_id<>?2",
                params![p, crew],
                |r| r.get(0),
            )?;
            let n = tx.conn().execute(
                "UPDATE crew_parents SET position=?3 WHERE crew_id=?1 AND parent_id=?2",
                params![crew, p, max + 1],
            )?;
            debug_assert_eq!(n, 1, "put_last 要求边已存在");
        }
        None => {
            let mut max = 0i64;
            for c in siblings(tx.conn(), None)? {
                if c != crew {
                    max = max.max(position_of(tx.conn(), &c)?);
                }
            }
            tx.conn().execute(
                "UPDATE crews SET position=?2 WHERE id=?1",
                params![crew, max + 1],
            )?;
        }
    }
    Ok(())
}

/// 人手动排侧栏（C1，只有客户端 `crew.reorder` 调）：`ordered` 必须恰好是这一层现有的全部机组（防客户端拿着旧列表覆盖）。
/// `Some(p)` 只改 p 名下各条边的次序（同一机组在别的父下的次序不动）并给 p 发一条 `CrewChanged`（它的 `childIds` 变了）；
/// `None` 改顶层的 `crews.position`。挪了位置的每个机组各发一条 `CrewChanged`。
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
    let mut moved = false;
    for (i, id) in ordered.iter().enumerate() {
        let pos = i as i64 + 1;
        let old: i64 = match parent {
            Some(p) => tx.conn().query_row(
                "SELECT position FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
                params![id, p],
                |r| r.get(0),
            )?,
            None => position_of(tx.conn(), id)?,
        };
        if old == pos {
            continue;
        }
        match parent {
            Some(p) => tx.conn().execute(
                "UPDATE crew_parents SET position=?3 WHERE crew_id=?1 AND parent_id=?2",
                params![id, p, pos],
            )?,
            None => tx
                .conn()
                .execute("UPDATE crews SET position=?2 WHERE id=?1", params![id, pos])?,
        };
        tx.note(Change::CrewChanged {
            crew_id: id.clone(),
        });
        moved = true;
    }
    if let (Some(p), true) = (parent, moved) {
        tx.note(Change::CrewChanged { crew_id: p.into() });
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
        // 控制者裁定 e：不存在的父 = NotFound（-32004），不是「不是直系子机组」（-32602）。
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "nope", None)),
            CollabError::NotFound(_)
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

    /// 终审 I4：删机组仍然级联（人显式的操作），但别的机组的群里不留幽灵成员 / 幽灵群主，
    /// 活下来的群发 ChannelChanged，被级联删掉的群发 ChannelRemoved。
    #[test]
    fn delete_clears_its_sessions_out_of_other_crews_channels_and_reports_every_channel() {
        use crate::channels::{self, NewChannel};
        use crewd_proto::{ChannelKind, MemberRef};
        let mut s = store();
        let as_ = |id: &str| crate::Sender::Session(id.into());
        let topic = |crew: &str, name: &str, members: Vec<MemberRef>| NewChannel {
            crew_id: crew.into(),
            kind: ChannelKind::Topic,
            name: name.into(),
            purpose: None,
            members,
        };
        // crew-a 的群：cap-x 只是成员（群主 w-a1，且有一条转给 cap-x 的待接受转让）。
        let member_only = ok(&mut s, |tx| {
            channels::create(
                tx,
                &as_("w-a1"),
                topic("crew-a", "甲", vec![MemberRef::session("cap-x")]),
            )
        });
        ok(&mut s, |tx| {
            channels::request_owner_transfer(
                tx,
                &as_("w-a1"),
                &member_only.id,
                &MemberRef::session("cap-x"),
            )
        });
        // crew-a 的群：cap-x 建的、是群主 → 交给 crew-a 此刻的机长。
        let owned = ok(&mut s, |tx| {
            channels::create(
                tx,
                &as_("cap-x"),
                topic("crew-a", "乙", vec![MemberRef::session("w-a2")]),
            )
        });
        // 删之前乙群里已有一条消息：机长入群的游标钉在它上面（不凭空多出一屏未读）。
        let before = ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                as_("w-a2"),
                crate::messages::NewPost {
                    channel_id: owned.id.clone(),
                    text: "删之前的一条".into(),
                    ..Default::default()
                },
            )
        });
        // crew-b 的群：cap-x 是群主，crew-b 没有机长 → 交给剩下的成员里最早入群的。
        let owned_no_captain = ok(&mut s, |tx| {
            channels::create(
                tx,
                &as_("cap-x"),
                topic("crew-b", "丙", vec![MemberRef::session("w-b1")]),
            )
        });
        // crew-b 的群：只有 cap-x 一个人，crew-b 没有机长 → 一个成员都不剩，群主留空。
        let owned_alone = ok(&mut s, |tx| {
            channels::create(tx, &as_("cap-x"), topic("crew-b", "戊", vec![]))
        });
        // crew-a 的私聊（w-a1 ↔ cap-x），群主被改成 cap-x（API 造不出，直接写库）：私聊不能加人，交给剩下那一方。
        let (dm, _) = ok(&mut s, |tx| {
            channels::ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("cap-x"),
            )
        });
        s.conn()
            .execute(
                "UPDATE channels SET owner_kind='session', owner_id='cap-x' WHERE id=?1",
                [&dm.id],
            )
            .unwrap();
        s.conn()
            .execute(
                "UPDATE crews SET captain_session_id=NULL WHERE id='crew-b'",
                [],
            )
            .unwrap();
        // crew-a 的人类群：cap-x 被人显式拉进来。
        ok(&mut s, |tx| channels::ensure_human_channel(tx, "crew-a"));
        ok(&mut s, |tx| {
            channels::add_members(
                tx,
                &crate::Sender::Human,
                "crew-a/human",
                &[MemberRef::session("cap-x")],
            )
        });
        // crew-x 自己的群：随机组级联删掉。
        let doomed = ok(&mut s, |tx| {
            channels::create(
                tx,
                &as_("cap-x"),
                topic("crew-x", "丁", vec![MemberRef::session("w-a1")]),
            )
        });

        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| delete(tx, "crew-x"));

        let ghost = MemberRef::session("cap-x");
        for id in [
            &member_only.id,
            &owned.id,
            &owned_no_captain.id,
            "crew-a/human",
        ] {
            assert!(
                !channels::members(s.conn(), id).unwrap().contains(&ghost),
                "{id} 里不留幽灵成员"
            );
            assert!(
                ch.contains(&Change::ChannelChanged {
                    channel_id: id.to_string()
                }),
                "{id} 活下来、成员变了，要发 ChannelChanged：{ch:?}"
            );
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) FROM channel_members WHERE member_id='cap-x' AND left_at IS NULL"
            ),
            0
        );
        let get = |id: &str| channels::get(s.conn(), id).unwrap();
        assert_eq!(
            get(&owned.id).owner,
            Some(MemberRef::session("cap-a")),
            "交给本群机组的机长"
        );
        assert_eq!(
            get(&owned_no_captain.id).owner,
            Some(MemberRef::session("w-b1")),
            "没机长就交给剩下的成员里最早入群的"
        );
        assert_eq!(get(&owned_alone.id).owner, None, "一个成员都不剩：群主留空");
        assert_eq!(
            get(&dm.id).owner,
            Some(MemberRef::session("w-a1")),
            "私聊交给剩下那一方"
        );
        assert_eq!(
            channels::members(s.conn(), &dm.id).unwrap(),
            [MemberRef::session("w-a1")],
            "私聊不因交接多出第三个人"
        );
        // 群主永远是成员（终审复审 A）。
        for id in [
            &member_only.id,
            &owned.id,
            &owned_no_captain.id,
            &owned_alone.id,
            &dm.id,
        ] {
            if let Some(o) = get(id).owner {
                assert!(
                    channels::members(s.conn(), id).unwrap().contains(&o),
                    "{id} 的群主 {o:?} 不在群里"
                );
            }
        }
        assert_eq!(
            crate::cursors::cursor(s.conn(), "cap-a", &owned.id).unwrap(),
            Some(before.message.seq),
            "接手的机长入群：游标钉在群的末尾"
        );
        assert_eq!(
            get(&member_only.id).owner,
            Some(MemberRef::session("w-a1")),
            "别人的群主不动"
        );
        assert_eq!(
            get(&member_only.id).pending_owner,
            None,
            "转给幽灵的待接受转让作废"
        );
        for gone in [doomed.id.as_str(), "crew-x/lobby"] {
            assert!(
                ch.contains(&Change::ChannelRemoved {
                    channel_id: gone.to_string()
                }),
                "{gone} 被级联删掉，要发 ChannelRemoved：{ch:?}"
            );
        }
        assert!(
            !ch.iter().any(|c| matches!(c, Change::ChannelRemoved { channel_id } if !channel_id.starts_with("crew-x/") && *channel_id != doomed.id)),
            "只报被删掉的群：{ch:?}"
        );
        // 机长在群里了，@机长 照常解析。
        let p = ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                as_("w-a2"),
                crate::messages::NewPost {
                    channel_id: owned.id.clone(),
                    text: "@机长 接一下".into(),
                    mentions: vec![crewd_proto::Mention {
                        kind: crewd_proto::MentionKind::Captain,
                        target_id: None,
                    }],
                    ..Default::default()
                },
            )
        });
        assert_eq!(p.message.mentions[0].target_id.as_deref(), Some("cap-a"));
    }

    /// 终审 M1：大厅的群主是算出来的（= 机长），换机长 = 大厅变了。
    #[test]
    fn changing_the_captain_notes_the_lobby_whose_owner_is_computed() {
        let mut s = store();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| set_captain(tx, "crew-a", "w-a1"));
        assert!(
            ch.contains(&Change::ChannelChanged {
                channel_id: "crew-a/lobby".into()
            }),
            "{ch:?}"
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
                .filter(|c| !c.org_root && is_root(s.conn(), &c.id).unwrap())
                .map(|c| c.id)
                .collect(),
        }
    }

    fn kid(s: &mut crewd_store::Store, title: &str, parent: &str) -> Crew {
        ok(s, |tx| {
            create(
                tx,
                NewCrew {
                    title: title.into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some(parent.into()),
                    ..Default::default()
                },
            )
        })
    }

    #[test]
    fn adopt_and_release_to_put_the_crew_after_the_new_siblings() {
        let mut s = store();
        let mk = |s: &mut crewd_store::Store, title: &str, parent: &str| {
            ok(s, |tx| {
                create(
                    tx,
                    NewCrew {
                        title: title.into(),
                        working_directory: "/tmp/example".into(),
                        parent_id: Some(parent.into()),
                        ..Default::default()
                    },
                )
            })
        };
        let c1 = mk(&mut s, "先建的", "crew-a");
        let c2 = mk(&mut s, "后建的", "crew-x");
        // c1 的 position 比 c2 小；收编到 crew-x 下后必须排在 c2 后面（新同级的末尾）。
        ok(&mut s, |tx| adopt(tx, &c1.id, "crew-x"));
        assert_eq!(order(&s, Some("crew-x")), [c2.id.clone(), c1.id.clone()]);
        let c3 = mk(&mut s, "再一个", "crew-b");
        ok(&mut s, |tx| release(tx, &c2.id, "crew-x", Some("crew-b")));
        assert_eq!(order(&s, Some("crew-b")), [c3.id.clone(), c2.id.clone()]);
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

    #[test]
    fn reordering_under_one_parent_leaves_the_order_under_another_parent_alone() {
        let mut s = store();
        let k1 = kid(&mut s, "甲1", "crew-a");
        let k2 = kid(&mut s, "甲2", "crew-a");
        let m1 = kid(&mut s, "丙1", "crew-x");
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x")); // crew-b 有两个父
        let under_a = vec!["crew-b".to_string(), k1.id.clone(), k2.id.clone()];
        assert_eq!(order(&s, Some("crew-a")), under_a);
        assert_eq!(
            order(&s, Some("crew-x")),
            vec![m1.id.clone(), "crew-b".to_string()]
        );
        let want_x = vec!["crew-b".to_string(), m1.id.clone()];
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            reorder(tx, Some("crew-x"), &want_x).map(|_| ())
        });
        assert_eq!(order(&s, Some("crew-x")), want_x);
        assert_eq!(
            order(&s, Some("crew-a")),
            under_a,
            "crew-a 下的顺序不受影响"
        );
        assert!(
            ch.contains(&Change::CrewChanged {
                crew_id: "crew-x".into()
            }),
            "父的 childIds 变了"
        );
        let want_a = vec![k2.id.clone(), k1.id.clone(), "crew-b".to_string()];
        ok(&mut s, |tx| reorder(tx, Some("crew-a"), &want_a));
        assert_eq!(order(&s, Some("crew-x")), want_x, "反过来也一样");
        assert_eq!(
            get(s.conn(), "crew-a").unwrap().child_ids,
            want_a,
            "线上带 childIds，按侧栏顺序"
        );
        assert_eq!(get(s.conn(), "crew-x").unwrap().child_ids, want_x);
    }

    #[test]
    fn adopting_under_a_busy_parent_does_not_reorder_the_other_parents() {
        let mut s = store();
        let ks: Vec<String> = ["a1", "a2", "a3"]
            .iter()
            .map(|t| kid(&mut s, t, "crew-a").id)
            .collect();
        let xs: Vec<String> = (1..=7)
            .map(|i| kid(&mut s, &format!("x{i}"), "crew-x").id)
            .collect();
        let under_a: Vec<String> = std::iter::once("crew-b".to_string()).chain(ks).collect();
        assert_eq!(order(&s, Some("crew-a")), under_a);
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(
            order(&s, Some("crew-a")),
            under_a,
            "收编进别的父不改 crew-a 下的顺序"
        );
        let mut want_x = xs;
        want_x.push("crew-b".to_string());
        assert_eq!(order(&s, Some("crew-x")), want_x, "排在新父的末尾");
    }

    #[test]
    fn the_top_level_order_leaves_out_builtin_crews() {
        let mut s = store();
        let roots = order(&s, None);
        assert!(!roots.contains(&"chief".to_string()));
        let mut rotated = roots.clone();
        rotated.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, None, &rotated));
        assert_eq!(order(&s, None), rotated);
        let mut with_chief = roots.clone();
        with_chief.push("chief".into());
        assert!(
            matches!(
                err(&mut s, |tx| reorder(tx, None, &with_chief)),
                CollabError::Invalid(_)
            ),
            "客户端只会给可见的顶层"
        );
    }

    #[test]
    fn delete_puts_orphaned_children_last_at_the_top_level() {
        let mut s = store();
        let solo = kid(&mut s, "只有一个父", "crew-a");
        let shared = kid(&mut s, "另有父", "crew-a");
        ok(&mut s, |tx| adopt(tx, &shared.id, "crew-x")); // 另有父 crew-x，不会变根
        let roots_before = order(&s, None);
        ok(&mut s, |tx| delete(tx, "crew-a"));
        let roots = order(&s, None);
        let mut want: Vec<String> = roots_before.into_iter().filter(|c| c != "crew-a").collect();
        want.extend(["crew-b".to_string(), solo.id.clone()]);
        assert_eq!(
            roots, want,
            "变成根的（按原先的次序）排在顶层末尾，原有的根不动"
        );
        assert!(!roots.contains(&shared.id));
        assert_eq!(edges(&s, &shared.id), ["crew-x"]);
    }

    #[test]
    fn a_failure_after_earlier_writes_rolls_all_of_them_back() {
        let mut s = store();
        let crews_before = count(&s, "SELECT COUNT(*) FROM crews");
        let chans_before = count(&s, "SELECT COUNT(*) FROM channels");
        // create 先发号、写机组、建大厅，最后挂到内建父下才失败。
        let e = err(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "半截".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("chief".into()),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(e, CollabError::Forbidden(_)));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM crews"), crews_before);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM channels"), chans_before);
        let next = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "接着建".into(),
                    working_directory: "/tmp/example".into(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(next.number, Some(4), "失败那次没有消耗机组号");
    }

    #[test]
    fn update_applies_title_source_even_without_a_title() {
        let mut s = store();
        let c = ok(&mut s, |tx| {
            update(
                tx,
                "crew-a",
                CrewPatch {
                    title_source: Some("captain".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(c.title_source.as_deref(), Some("captain"));
        assert_eq!(c.title, "机组甲", "标题不动");
        let c = ok(&mut s, |tx| {
            update(
                tx,
                "crew-a",
                CrewPatch {
                    title: Some("新名字".into()),
                    title_source: Some("human".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(
            (c.title.as_str(), c.title_source.as_deref()),
            ("新名字", Some("human"))
        );
        let c = ok(&mut s, |tx| {
            update(
                tx,
                "crew-a",
                CrewPatch {
                    title: Some("再改".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(
            c.title_source.as_deref(),
            Some("human"),
            "没给 titleSource = 不动"
        );
    }
}
