//! 机组：森林（顶层机组 + DAG）、总机组（管理机组，不在树里）、机长、建 / 改 / 归档、编号。移植
//! `LocalCrewStore.swift:356-526`、`CrewDirectory.swift:8-13`。「归档」是库里的叫法；客户端与人看到的叫
//! 「移除」（`crew.remove`，隐藏并冻结、可恢复）/「恢复」（`crew.restore`）；「永久删除」（`crew.purge`）
//! 才真的删（PM 2026-10-07）。
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::Crew;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    CollabError::NotFound(Refusal::new(Code::CrewNotFound).arg("crew", id))
}

pub fn get(conn: &Connection, id: &str) -> Result<Crew> {
    crewd_store::crews::get(conn, id)?.ok_or_else(|| not_found(id))
}

/// `crew.list` 的次序 = 侧栏：总机组（管理机组，不在树里）单独排第一；然后机组森林的先序遍历——顶层机组按
/// `crews.position`（人排的，`reorder` 不带 parent 时改它；再按建组时间、id），每层按父的 `child_ids`（边上的次序）；
/// 多父的机组排在第一次遇到的位置、只出现一次；从顶层走不到的（旧数据里成环的）按建组时间、id 排在最后。
/// 这只是一个稳定的扁平次序；客户端建树用 `parent_ids` / `child_ids`。store 层仍按号排（M1 的测试依赖它）。
/// 归档的机组（N1 裁定 13）默认不列，`include_archived` 才列；`child_ids` 照实含归档的子机组（边还在，解档原地回来），
/// 客户端遇到列表里没有的 id 跳过即可。
pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<Crew>> {
    let mut rest: BTreeMap<String, Crew> = crewd_store::crews::list(conn)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let mut out: Vec<Crew> = Vec::with_capacity(rest.len());
    if let Some(m) = management(conn)?
        && let Some(c) = rest.remove(&m)
    {
        out.push(c);
    }
    // 先序：栈里倒着压，弹出来就是顶层次序 / child_ids 的次序。
    let mut stack: Vec<String> = top_level(conn)?.into_iter().rev().collect();
    while let Some(id) = stack.pop() {
        let Some(c) = rest.remove(&id) else {
            continue; // 已经出现过（多父）
        };
        stack.extend(c.child_ids.iter().rev().cloned());
        out.push(c);
    }
    let mut cut_off: Vec<Crew> = rest.into_values().collect();
    cut_off.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out.extend(cut_off);
    if !include_archived {
        out.retain(|c| c.removed_at.is_none());
    }
    Ok(out)
}

/// 顶层机组（没有父边的非管理机组，含归档的），按侧栏次序：`crews.position`，再按建组时间、id。
pub fn top_level(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT id FROM crews c WHERE management=0
            AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)
          ORDER BY COALESCE(position,0), created_at, id",
    )?;
    Ok(st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
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

pub fn management(conn: &Connection) -> Result<Option<String>> {
    Ok(crewd_store::crews::management(conn)?)
}

/// 一次直查（preflight D9）：`home_of` / `is_home` 在循环里调它，不走 `get`（那会把全部机组连同父子边读一遍）。
pub fn is_management(conn: &Connection, id: &str) -> Result<bool> {
    conn.query_row("SELECT management FROM crews WHERE id=?1", [id], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| not_found(id))
}

/// `session` 是总机组此刻的机长？权限上它当作每个机组的机长（PM 2026-10-07：「就像所有机组的根机组一样的权限」——
/// 只是权限，不是树）。总机组的其它 session 读全组织、可以往任何群发言，但在别的机组里只算普通 session。
/// 一次直查；session 不存在 = false（同 `captain_of(..) == Some(s)` 的口径）。
pub fn is_management_captain(conn: &Connection, session: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM crews WHERE management=1 AND captain_session_id=?1",
        [session],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// `session` 在机组 `crew` 上有机长的权限：是它此刻的机长，或总机组的机长（`is_management_captain`；
/// PM 2026-10-07：总机组的机长像所有机组的根机组一样，在每个机组都按机长算）。总机组的其他 session
/// 不算（它们只多出「读全组织、往任何活群发言」）。collab 里「这个机组的机长才能做」的检查都走这里；
/// collab 之外（M3b 的机长工具、`crew_structure` 等）也必须用它，不要只比 `Crew.captain_session_id`。
/// 一两次直查；机组或 session 不存在 = false（不是错误）。
pub fn acts_as_captain_of(conn: &Connection, session: &str, crew: &str) -> Result<bool> {
    Ok(
        captain_of(conn, crew)?.as_deref() == Some(session)
            || is_management_captain(conn, session)?,
    )
}

/// `session` 属于总机组（管理机组）？读全组织、往任何群发言（dm 除外）的依据。一次直查；session 不存在 = false。
pub fn is_management_session(conn: &Connection, session: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sessions s JOIN crews c ON c.id=s.crew_id
          WHERE s.id=?1 AND c.management=1",
        [session],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 机组已归档（N1 裁定 13）？一次直查；不存在 = NotFound。
pub fn is_archived(conn: &Connection, id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT archived_at IS NOT NULL FROM crews WHERE id=?1",
        [id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| not_found(id))
}

/// 归档的机组只读（N1 裁定 13、preflight M4）：结构操作、改名、换机长、起 / 注册 session、建群、动它的群、
/// 往它的人类群发言一律拒，指路先解档。读（列表、详情、读消息）照常。
pub(crate) fn refuse_archived(conn: &Connection, id: &str) -> Result<()> {
    if is_archived(conn, id)? {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewRemoved).arg("crew", id),
        ));
    }
    Ok(())
}

/// 有自己人类群的机组：总机组（它自己的管理用人类群）；顶层机组（没有父边）。
pub fn is_home(conn: &Connection, id: &str) -> Result<bool> {
    if is_management(conn, id)? {
        return Ok(true);
    }
    Ok(parents(conn, id)?.is_empty())
}

/// 人类群归属（N1 裁定 5）：自己是 home → 自己；否则往上走，遇到的顶层祖先里 created_at 最早的（再按 id）。
/// 每个祖先几次直查（O(祖先数)，不读全部机组）；不存在的机组由 `is_home` → `is_management` 报 NotFound。
/// 走不到顶层（旧数据里成环）→ 退回自己（全函数；拒绝指路时由 `home_pointer` 按数据不一致报）。
pub fn home_of(conn: &Connection, id: &str) -> Result<String> {
    if is_home(conn, id)? {
        return Ok(id.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from(parents(conn, id)?);
    let mut homes: Vec<(String, String)> = Vec::new();
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c.clone()) {
            continue;
        }
        if is_home(conn, &c)? {
            homes.push((created_at(conn, &c)?, c));
        } else {
            queue.extend(parents(conn, &c)?);
        }
    }
    Ok(homes
        .into_iter()
        .min()
        .map(|(_, h)| h)
        .unwrap_or_else(|| id.to_string()))
}

/// 给「`crew` 不是 home、没有人类群」的拒绝指路用的 home。树规则（无环）下非 home 机组往上一定走得到
/// 顶层机组；走不到（旧数据里成环）时 `home_of` 退回自己——指回自己没有意义，按内部数据不一致报。
pub(crate) fn home_pointer(conn: &Connection, crew: &str) -> Result<String> {
    let home = home_of(conn, crew)?;
    if home == crew {
        return Err(CollabError::Store(crewd_store::StoreError::Invalid(
            format!("机组 {crew} 不是 home，往上也走不到顶层机组（上级成环）：数据不一致"),
        )));
    }
    Ok(home)
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

/// 总机组（管理机组）不能隐藏、归档、永久删除。被操作的机组不存在 = NotFound。
fn refuse_management(conn: &Connection, id: &str) -> Result<()> {
    if is_management(conn, id)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::CrewManagementFixed,
        )));
    }
    Ok(())
}

/// 总机组不在机组树里（PM 2026-10-07）：结构操作涉及的任何一个机组是总机组 → Invalid
/// （它不能有上级、下级，别的机组也不能挂到它下面）。不存在的机组 = NotFound。
fn refuse_management_in_tree(conn: &Connection, ids: &[&str]) -> Result<()> {
    for id in ids {
        if is_management(conn, id)? {
            return Err(CollabError::Invalid(Refusal::new(
                Code::CrewManagementNotInTree,
            )));
        }
    }
    Ok(())
}

/// 空库里第一个机组顺带懒建总机组（N1 裁定 3）；懒建出来的总机组连同它的大厅和别的建机组一样发变更。
fn ensure_management(tx: &mut Tx<'_>) -> Result<()> {
    if management(tx.conn())?.is_some() {
        return Ok(());
    }
    let now = tx.now_ts();
    let m = crewd_store::crews::ensure_management(tx.conn(), &now)?;
    tx.note(Change::CrewChanged { crew_id: m.clone() });
    tx.note(Change::ChannelChanged {
        channel_id: crewd_store::channels::lobby_id(&m),
    });
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
    // The parent first: under the management crew (which has no working directory) the reason
    // is that it is outside the tree, not the empty directory a caller inherited from it.
    if let Some(p) = n.parent_id.as_deref() {
        refuse_management_in_tree(tx.conn(), &[p])?;
        refuse_archived(tx.conn(), p)?;
    }
    let title = n.title.trim().to_string();
    if title.is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::CrewTitleEmpty)));
    }
    let wd = n.working_directory.trim().to_string();
    if wd.is_empty() {
        return Err(CollabError::Invalid(Refusal::new(
            Code::CrewWorkingDirEmpty,
        )));
    }
    let number = next_crew_number(tx)?;
    let now = tx.now_ts();
    let crew = Crew {
        id: format!("crew-{}", uuid::Uuid::new_v4().simple()),
        title,
        number: Some(number),
        management: false,
        working_directory: Some(wd),
        title_source: n.title_source,
        created_at: now.clone(),
        updated_at: now.clone(),
        ..Default::default()
    };
    crewd_store::crews::upsert(tx.conn(), &crew, None)?;
    crewd_store::channels::ensure_lobby(tx.conn(), &crew.id, &now)?;
    tx.note(Change::CrewChanged {
        crew_id: crew.id.clone(),
    });
    // 大厅是新建的群：和别的建群一样发 ChannelChanged。
    tx.note(Change::ChannelChanged {
        channel_id: crewd_store::channels::lobby_id(&crew.id),
    });
    match n.parent_id.as_deref() {
        // 有父：排在父下的末尾（adopt 做）。
        Some(p) => {
            adopt(tx, &crew.id, p)?;
        }
        // 没给父：顶层的末尾。
        None => put_last_top(tx, &crew.id)?,
    }
    // 空库里第一个机组顺带建出总机组（N1 裁定 3）。
    ensure_management(tx)?;
    get(tx.conn(), &crew.id)
}

/// A new top-level crew with `child` under it (M3b `crew_structure` create_parent). `child` is
/// checked first (exists, not the management crew, not removed), so its refusal is the reason
/// given, not the empty directory the new crew would have inherited from the management crew.
/// The caller's transaction holds both steps: a refusal leaves nothing behind.
pub fn create_above(tx: &mut Tx<'_>, n: NewCrew, child: &str) -> Result<Crew> {
    get(tx.conn(), child)?;
    refuse_management_in_tree(tx.conn(), &[child])?;
    refuse_archived(tx.conn(), child)?;
    let parent = create(
        tx,
        NewCrew {
            parent_id: None,
            ..n
        },
    )?;
    adopt(tx, child, &parent.id)?;
    get(tx.conn(), &parent.id)
}

pub fn update(tx: &mut Tx<'_>, id: &str, p: CrewPatch) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    refuse_archived(tx.conn(), id)?;
    // 总机组不能被隐藏（改名等其余字段与普通机组一样）。
    if crew.management && p.hidden == Some(true) {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::CrewManagementFixed,
        )));
    }
    let conn = tx.conn();
    let now = tx.now_ts();
    if let Some(t) = p.title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(CollabError::Invalid(Refusal::new(Code::CrewTitleEmpty)));
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
            return Err(CollabError::Invalid(Refusal::new(
                Code::CrewWorkingDirEmpty,
            )));
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
/// 总机组不在树里：`crew` 或 `parent` 是总机组 → Invalid（crew.management_not_in_tree）。
/// 顶层机组挂到别人下面 = 不再是 home：它的人类群归档（消息保留）。
pub fn adopt(tx: &mut Tx<'_>, crew: &str, parent: &str) -> Result<Crew> {
    refuse_management_in_tree(tx.conn(), &[crew, parent])?;
    get(tx.conn(), crew)?;
    get(tx.conn(), parent)?;
    refuse_archived(tx.conn(), crew)?;
    refuse_archived(tx.conn(), parent)?;
    if crew == parent {
        return Err(CollabError::Invalid(Refusal::new(
            Code::CrewCannotParentSelf,
        )));
    }
    if descendants(tx.conn(), crew)?.contains(parent) {
        return Err(CollabError::Invalid(
            Refusal::new(Code::CrewWouldCycle)
                .arg("parent", parent)
                .arg("crew", crew),
        ));
    }
    let ps = parents(tx.conn(), crew)?;
    if ps.iter().any(|p| p == parent) {
        return get(tx.conn(), crew);
    }
    let was_home = is_home(tx.conn(), crew)?;
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, parent],
    )?;
    // 不再是 home（顶层机组）：它的人类群归档（消息保留）。
    if was_home && !is_home(tx.conn(), crew)? {
        crate::channels::archive_human_of(tx, crew)?;
    }
    put_last(tx, crew, parent)?;
    tx.note(Change::CrewChanged {
        crew_id: crew.into(),
    });
    tx.note(Change::CrewChanged {
        crew_id: parent.into(),
    });
    get(tx.conn(), crew)
}

/// 把 `from` 的直系子 `crew` 摘出；`to` 是别的机组 = 先挂新边（过 adopt 的全部检查）再摘旧边。
/// 摘掉的是最后一条父边 = 成为顶层机组、排在顶层末尾；它原来就有的人类群解档（没有的仍然懒建）。
/// 总机组不在树里：三者任何一个是总机组 → Invalid。`to == from` = 原地不动。
pub fn release(tx: &mut Tx<'_>, crew: &str, from: &str, to: Option<&str>) -> Result<Crew> {
    let mut involved = vec![crew, from];
    involved.extend(to);
    refuse_management_in_tree(tx.conn(), &involved)?;
    get(tx.conn(), crew)?;
    refuse_archived(tx.conn(), crew)?;
    // 不存在的父 = NotFound（控制者裁定 e），与「存在但不是直系父」（Invalid）分开。
    get(tx.conn(), from)?;
    let ps = parents(tx.conn(), crew)?;
    if !ps.iter().any(|p| p == from) {
        return Err(CollabError::Invalid(
            Refusal::new(Code::CrewNotDirectChild)
                .arg("crew", crew)
                .arg("from", from),
        ));
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
    if parents(tx.conn(), crew)?.is_empty() {
        put_last_top(tx, crew)?;
        // 又成了 home：原来就有的人类群解档（没有的仍然懒建）。
        if crate::channels::human_channel_exists(tx.conn(), crew)? {
            crate::channels::ensure_human_channel(tx, crew)?;
        }
    }
    tx.note(Change::CrewChanged {
        crew_id: crew.into(),
    });
    tx.note(Change::CrewChanged {
        crew_id: from.into(),
    });
    get(tx.conn(), crew)
}

fn refuse_running(conn: &Connection, id: &str) -> Result<()> {
    let running: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sessions WHERE crew_id=?1 AND desired='running'",
        [id],
        |r| r.get(0),
    )?;
    if running > 0 {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewRunningSessions)
                .arg("crew", id)
                .arg("running", running.to_string()),
        ));
    }
    Ok(())
}

/// 移除机组 = 归档（N1 裁定 13；`crew.remove` 调它，可逆，不拍快照）：拒总机组（顶层机组照常可以归档）、拒还有 desired=running 的 session、
/// 拒还有**没归档的子机组**（先挪走或归档子机组，不级联）。写 `crews.archived_at`，把**归属这个机组**的群全部标归档
/// （每个一条 ChannelChanged；topic / dm 上待接受的群主转让随之作废，同 `channels::update` 归档）。
/// 消息、session 行、成员行、账本全部保留；父子边不动（解档原地回来）。还开着的问撤回（`asks::withdraw_open_for_archive`，
/// 解档不重开）。它的 session 排着队的唤醒（pending，含 requeue 回来的）作废（`wakes::drop_for_crew`，原因
/// "crew archived"，同一事务）；还有 inflight 的唤醒（正在投递）→ Conflict（等投递结束再归档），什么都不变。
/// 已归档 = 原样返回、不发变更。
pub fn archive(tx: &mut Tx<'_>, id: &str) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    refuse_management(tx.conn(), id)?;
    crate::cursor_reservations::guard_crew(tx.conn(), id)?;
    if crew.removed_at.is_some() {
        return Ok(crew);
    }
    refuse_running(tx.conn(), id)?;
    let mut live_kids = Vec::new();
    for k in children(tx.conn(), id)? {
        if !is_archived(tx.conn(), &k)? {
            live_kids.push(k);
        }
    }
    if !live_kids.is_empty() {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewLiveChildren)
                .arg("crew", id)
                .arg("children", live_kids.join(", ")),
        ));
    }
    // 还有唤醒正在投递（inflight）就拒：投递方随后还要 mark_delivered / requeue，作废了它们会撞上 dropped。
    let inflight = crate::wakes::count_inflight_for_crew(tx.conn(), id)?;
    if inflight > 0 {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewWakesInflight)
                .arg("crew", id)
                .arg("count", inflight.to_string()),
        ));
    }
    // 它还开着的问一并撤回（Task 9 复审裁定；解档不重开）。
    crate::asks::withdraw_open_for_archive(tx, id)?;
    // 它的 session 还排着队（pending）的唤醒一并作废（归档的 session 起不来）。
    crate::wakes::drop_for_crew(tx, id, "crew archived")?;
    let now = tx.now_ts();
    tx.conn().execute(
        "UPDATE crews SET archived_at=?2, updated_at=?2 WHERE id=?1",
        params![id, now],
    )?;
    let channels: Vec<String> = {
        let mut st = tx.conn().prepare(
            "SELECT id FROM channels WHERE crew_id=?1 AND archived_at IS NULL ORDER BY created_at, id",
        )?;
        st.query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.conn().execute(
        "UPDATE channels SET archived_at=?2, pending_owner_kind=NULL, pending_owner_id=NULL,
                pending_owner_by=NULL, pending_owner_at=NULL
          WHERE crew_id=?1 AND archived_at IS NULL",
        params![id, now],
    )?;
    for channel_id in channels {
        tx.note(Change::ChannelChanged { channel_id });
    }
    tx.note(Change::CrewChanged { crew_id: id.into() });
    get(tx.conn(), id)
}

/// 解档（`crew.restore`，人叫「恢复」）：清 `archived_at`；大厅解档；是 home 且人类群建过 → 人类群解档（`ensure_human_channel`，
/// 钉机长游标）。topic / dm 留归档（人逐个解档）。上级还归档着 → Conflict（先解档上级：归档的机组下面不挂活的）。
/// 没归档 = 原样返回、不发变更。
pub fn unarchive(tx: &mut Tx<'_>, id: &str) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    if crew.removed_at.is_none() {
        return Ok(crew);
    }
    let mut archived_parents = Vec::new();
    for p in parents(tx.conn(), id)? {
        if is_archived(tx.conn(), &p)? {
            archived_parents.push(p);
        }
    }
    if !archived_parents.is_empty() {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewParentRemoved)
                .arg("crew", id)
                .arg("parents", archived_parents.join(", ")),
        ));
    }
    let now = tx.now_ts();
    tx.conn().execute(
        "UPDATE crews SET archived_at=NULL, updated_at=?2 WHERE id=?1",
        params![id, now],
    )?;
    let lobby = crewd_store::channels::lobby_id(id);
    if tx.conn().execute(
        "UPDATE channels SET archived_at=NULL WHERE id=?1 AND archived_at IS NOT NULL",
        [&lobby],
    )? > 0
    {
        tx.note(Change::ChannelChanged { channel_id: lobby });
    }
    if is_home(tx.conn(), id)? && crate::channels::human_channel_exists(tx.conn(), id)? {
        crate::channels::ensure_human_channel(tx, id)?;
    }
    tx.note(Change::CrewChanged { crew_id: id.into() });
    get(tx.conn(), id)
}

/// `purge` 的前置条件（只读、便宜）：机组存在、不是总机组、**已归档**、没有 desired=running 的 session。
/// `crew.purge` 先跑它、再拍快照（N4：删不了就不拍整库快照），三步在同一个库线程任务里。
pub fn check_purgeable(conn: &Connection, id: &str) -> Result<()> {
    get(conn, id)?;
    refuse_management(conn, id)?;
    if !is_archived(conn, id)? {
        return Err(CollabError::Conflict(
            Refusal::new(Code::CrewNotRemoved).arg("crew", id),
        ));
    }
    refuse_running(conn, id)?;
    crate::cursor_reservations::guard_crew(conn, id)
}

/// 永久删除（`crew.purge`，N4 的快照之后跑；只收已归档的机组，见 `check_purgeable`）。子机组的父边随之消失；
/// 只剩这一个父的子机组成为顶层、按原先的次序排在顶层末尾——它们也已归档（归档的机组下面不挂活的），人类群不顺手解档。
/// 删除级联：本机组的 session、群（含跨机组 topic 及其历史）一起删掉，每个群发 ChannelRemoved；它的 session 在
/// 别的机组群里的成员 / 群主身份先交出去（`release_sessions_of_doomed_crew`）。
pub fn purge(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    check_purgeable(tx.conn(), id)?;
    let kids = children(tx.conn(), id)?;
    // 只有这一个父的子机组删后没有父了：按原先的次序排到顶层末尾（C1）。
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
    // Message cascades have completed. Keep provenance for surviving foreign-channel history,
    // even when the source session was purged; only actual orphan messages lose their markers.
    crate::message_provenance::prune_orphans(tx)?;
    crate::cursor_reservations::prune_orphan_readers(tx)?;
    for channel_id in cascaded {
        tx.note(Change::ChannelRemoved { channel_id });
    }
    for k in &orphans {
        put_last_top(tx, k)?;
        // 又成了 home：原来就有的人类群解档（与 release 同一个做法；没有的仍然懒建）——归档的孤儿除外，它只读。
        if !is_archived(tx.conn(), k)? && crate::channels::human_channel_exists(tx.conn(), k)? {
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
    refuse_archived(tx.conn(), crew)?;
    let s = crate::sessions::get(tx.conn(), session)?;
    if s.crew_id != crew {
        return Err(CollabError::Invalid(
            Refusal::new(Code::CrewSessionNotInCrew)
                .arg("session", session)
                .arg("crew", crew),
        ));
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
/// 大厅发 ChannelChanged（群主随机长算）；home（总机组 / 顶层机组）且人类群已存在时，把新机长的游标钉到人类群末尾（不凭空多出一屏历史未读，裁定 9）。
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
    if is_home(tx.conn(), crew)? {
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
        home_id: home_of(tx.conn(), id)?,
        children: children(tx.conn(), id)?,
        channels: crate::channels::list_for_crew(tx, id)?,
        sessions: crate::sessions::list(tx.conn(), Some(id))?,
        crew,
    })
}

/// 把 `crew` 放到 `parent` 名下的末尾（C1：新建 / 收编 / 转挂进来的排最后）：读写 (crew, parent) 这条边的
/// position，只和 parent 的其它边比；多父机组在每个父下各有自己的次序，互不影响。调用时边必须已经存在。
fn put_last(tx: &mut Tx<'_>, crew: &str, parent: &str) -> Result<()> {
    let max: i64 = tx.conn().query_row(
        "SELECT COALESCE(MAX(position),0) FROM crew_parents WHERE parent_id=?1 AND crew_id<>?2",
        params![parent, crew],
        |r| r.get(0),
    )?;
    let n = tx.conn().execute(
        "UPDATE crew_parents SET position=?3 WHERE crew_id=?1 AND parent_id=?2",
        params![crew, parent, max + 1],
    )?;
    debug_assert_eq!(n, 1, "put_last 要求边已存在");
    Ok(())
}

/// 把 `crew` 放到顶层的末尾（新建的顶层机组、摘掉最后一个父的、purge 留下的孤儿）：写 `crews.position`，
/// 只和别的顶层机组比（总机组不参与顶层次序）。
fn put_last_top(tx: &mut Tx<'_>, crew: &str) -> Result<()> {
    let max: i64 = tx.conn().query_row(
        "SELECT COALESCE(MAX(COALESCE(position,0)),0) FROM crews c
          WHERE c.id<>?1 AND management=0
            AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)",
        [crew],
        |r| r.get(0),
    )?;
    tx.conn().execute(
        "UPDATE crews SET position=?2 WHERE id=?1",
        params![crew, max + 1],
    )?;
    Ok(())
}

/// 人手动排侧栏（C1，只有客户端 `crew.reorder` 调），一次排一层：`parent` = `Some(p)` 排 p 名下的直系子
/// （写边上的次序，读回来是 p 的 `childIds`；同一机组在别的父下的次序不动；p 是总机组 → Invalid，它没有子机组）；
/// `None` 排顶层机组（写 `crews.position`；总机组不在顶层次序里，名单里也不能有它）。
/// `ordered` 必须包含这一层现有的全部**活的**机组（防客户端拿着旧列表覆盖），不重复、不含不认识的；客户端从默认
/// 列表（不含归档）排序，所以不必带上归档的。没带的归档机组保持原来的相对次序，排在 `ordered` 后面（带了就按
/// 客户端给的次序）。返回最终的全序。挪了位置的每个机组各发一条 `CrewChanged`，有挪动时 `parent` 也发一条
/// （它的 `childIds` 变了）。p 已移除 → Conflict `crew.removed`（移除 = 冻结，次序也不能改）。
pub fn reorder(tx: &mut Tx<'_>, parent: Option<&str>, ordered: &[String]) -> Result<Vec<Crew>> {
    let current = match parent {
        Some(p) => {
            get(tx.conn(), p)?;
            refuse_management_in_tree(tx.conn(), &[p])?;
            // 移除 = 冻结：已移除的父名下的次序也不改。
            refuse_archived(tx.conn(), p)?;
            children(tx.conn(), p)?
        }
        None => top_level(tx.conn())?,
    };
    let want: BTreeSet<&String> = ordered.iter().collect();
    let have: BTreeSet<&String> = current.iter().collect();
    let mut live = BTreeSet::new();
    for id in &current {
        if !is_archived(tx.conn(), id)? {
            live.insert(id);
        }
    }
    // 重复 / 不认识的 / 缺了活的 → Invalid；归档的可带可不带。
    if want.len() != ordered.len() || !want.is_subset(&have) || !live.is_subset(&want) {
        let live_ids: Vec<&String> = live.into_iter().collect();
        return Err(CollabError::Invalid(
            Refusal::new(Code::CrewReorderMismatch).arg("current", format!("{live_ids:?}")),
        ));
    }
    let ordered: Vec<String> = ordered
        .iter()
        .cloned()
        .chain(current.iter().filter(|id| !want.contains(id)).cloned())
        .collect();
    let mut moved = false;
    for (i, id) in ordered.iter().enumerate() {
        let pos = i as i64 + 1;
        let old: i64 = match parent {
            Some(p) => tx.conn().query_row(
                "SELECT position FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
                params![id, p],
                |r| r.get(0),
            )?,
            None => tx.conn().query_row(
                "SELECT COALESCE(position,0) FROM crews WHERE id=?1",
                [id],
                |r| r.get(0),
            )?,
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
    use crate::testkit::{NOW, count, err, ok, ok_at, store};

    fn edges(s: &crewd_store::Store, crew: &str) -> Vec<String> {
        parents(s.conn(), crew).unwrap()
    }

    #[test]
    fn home_is_the_top_level_ancestor_and_the_management_crew_is_its_own_home() {
        let s = store();
        assert_eq!(home_of(s.conn(), "chief").unwrap(), "chief");
        assert_eq!(home_of(s.conn(), "crew-a").unwrap(), "crew-a");
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert!(is_home(s.conn(), "chief").unwrap());
        assert!(is_home(s.conn(), "crew-a").unwrap());
        assert!(!is_home(s.conn(), "crew-b").unwrap());
        assert!(
            is_management(s.conn(), "chief").unwrap()
                && !is_management(s.conn(), "crew-a").unwrap()
        );
        // 多父：二级机组同时挂在两个顶层机组下 → created_at 最早的顶层祖先（§13 Q1 同口径）。
        crewd_store::crews::set_parents(
            s.conn(),
            "crew-b",
            &["crew-x".to_string(), "crew-a".to_string()],
        )
        .unwrap();
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
    }

    #[test]
    fn crew_detail_names_the_top_level_home() {
        let mut s = store();
        let d = ok(&mut s, |tx| detail(tx, "crew-b"));
        assert_eq!(d.home_id, "crew-a");
    }

    #[test]
    fn home_of_is_the_earliest_home_among_ancestors() {
        let mut s = store();
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert_eq!(home_of(s.conn(), "crew-a").unwrap(), "crew-a");
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(
            home_of(s.conn(), "crew-b").unwrap(),
            "crew-a",
            "crew-a 建得更早"
        );
        s.conn()
            .execute(
                "UPDATE crews SET created_at='2026-08-15T00:00:00.000Z' WHERE id='crew-x'",
                [],
            )
            .unwrap();
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-x");
    }

    #[test]
    fn adopt_refuses_cycles_self_and_the_management_crew_and_is_idempotent() {
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
        // 总机组不在树里：两个方向都拒（crew.management_not_in_tree）。
        for (crew, parent) in [("chief", "crew-a"), ("crew-a", "chief")] {
            match err(&mut s, |tx| adopt(tx, crew, parent)) {
                CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewManagementNotInTree),
                e => panic!("{crew} → {parent}: {e:?}"),
            }
        }
        assert!(edges(&s, "crew-a").is_empty(), "crew-a 仍是顶层");
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
        // 三种「from 不是 crew 的直系父」：别的机组；反过来（from 是 crew 的子）；crew 本来就是顶层。
        for (crew, from, why) in [
            ("crew-b", "crew-x", "crew-b 不是 crew-x 的直系子"),
            ("crew-a", "crew-b", "crew-b 是 crew-a 的子，不是它的父"),
            ("crew-x", "crew-a", "crew-x 是顶层，没有父"),
        ] {
            match err(&mut s, |tx| release(tx, crew, from, None)) {
                CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewNotDirectChild, "{why}"),
                e => panic!("{why}: {e:?}"),
            }
        }
        assert_eq!(edges(&s, "crew-b"), ["crew-a"], "被拒的摘出什么都不动");
        assert!(edges(&s, "crew-a").is_empty() && edges(&s, "crew-x").is_empty());
        // 控制者裁定 e：不存在的父 = NotFound（-32004），不是「不是直系子机组」（-32602）。
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "nope", None)),
            CollabError::NotFound(_)
        ));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", None));
        assert!(
            edges(&s, "crew-b").is_empty(),
            "摘掉最后一个父 = 成为顶层机组"
        );
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-b");
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
        archive_subtree(&mut s, &a.id);
        ok(&mut s, |tx| purge(tx, &a.id));
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
    fn purge_refuses_the_management_crew_and_crews_with_running_sessions() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| purge(tx, "chief")),
            CollabError::Forbidden(_)
        ));
        ok(&mut s, |tx| archive(tx, "crew-x"));
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", [])
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| purge(tx, "crew-x")),
            CollabError::Conflict(_)
        ));
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='cap-x'", [])
            .unwrap();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| purge(tx, "crew-x"));
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
    fn purge_clears_its_sessions_out_of_other_crews_channels_and_reports_every_channel() {
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
        // crew-a 的人类群：cap-x 留着一条成员行（旧数据的形状：home 不是 crew-a，现在 add_members 拒、行也不算数），
        // 删机组时这条行照样要清掉。
        ok(&mut s, |tx| channels::ensure_human_channel(tx, "crew-a"));
        ok(&mut s, |tx| {
            channels::join(tx, "crew-a/human", &MemberRef::session("cap-x"))
        });
        // crew-x 自己的群：随机组级联删掉。
        let doomed = ok(&mut s, |tx| {
            channels::create(
                tx,
                &as_("cap-x"),
                topic("crew-x", "丁", vec![MemberRef::session("w-a1")]),
            )
        });

        ok(&mut s, |tx| archive(tx, "crew-x"));
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| purge(tx, "crew-x"));

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

    fn order(s: &crewd_store::Store, parent: &str) -> Vec<String> {
        children(s.conn(), parent).unwrap()
    }

    /// 顶层机组的侧栏次序（含归档的）。
    fn top(s: &crewd_store::Store) -> Vec<String> {
        top_level(s.conn()).unwrap()
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
        assert_eq!(order(&s, "crew-x"), [c2.id.clone(), c1.id.clone()]);
        let c3 = mk(&mut s, "再一个", "crew-b");
        ok(&mut s, |tx| release(tx, &c2.id, "crew-x", Some("crew-b")));
        assert_eq!(order(&s, "crew-b"), [c3.id.clone(), c2.id.clone()]);
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
        let kids = order(&s, "crew-a");
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
        assert_eq!(order(&s, "crew-a"), kids);
        let mut want = kids.clone();
        want.reverse();
        let (got, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| reorder(tx, Some("crew-a"), &want));
        assert_eq!(got.into_iter().map(|c| c.id).collect::<Vec<_>>(), want);
        assert_eq!(order(&s, "crew-a"), want);
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
        assert_eq!(order(&s, "crew-x").last(), Some(&a.id));
        // 顶层也能排。
        let mut roots = top(&s);
        roots.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, None, &roots));
        assert_eq!(top(&s), roots);
    }

    #[test]
    fn reordering_under_one_parent_leaves_the_order_under_another_parent_alone() {
        let mut s = store();
        let k1 = kid(&mut s, "甲1", "crew-a");
        let k2 = kid(&mut s, "甲2", "crew-a");
        let m1 = kid(&mut s, "丙1", "crew-x");
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x")); // crew-b 有两个父
        let under_a = vec!["crew-b".to_string(), k1.id.clone(), k2.id.clone()];
        assert_eq!(order(&s, "crew-a"), under_a);
        assert_eq!(
            order(&s, "crew-x"),
            vec![m1.id.clone(), "crew-b".to_string()]
        );
        let want_x = vec!["crew-b".to_string(), m1.id.clone()];
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| {
            reorder(tx, Some("crew-x"), &want_x).map(|_| ())
        });
        assert_eq!(order(&s, "crew-x"), want_x);
        assert_eq!(order(&s, "crew-a"), under_a, "crew-a 下的顺序不受影响");
        assert!(
            ch.contains(&Change::CrewChanged {
                crew_id: "crew-x".into()
            }),
            "父的 childIds 变了"
        );
        let want_a = vec![k2.id.clone(), k1.id.clone(), "crew-b".to_string()];
        ok(&mut s, |tx| reorder(tx, Some("crew-a"), &want_a));
        assert_eq!(order(&s, "crew-x"), want_x, "反过来也一样");
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
        assert_eq!(order(&s, "crew-a"), under_a);
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(
            order(&s, "crew-a"),
            under_a,
            "收编进别的父不改 crew-a 下的顺序"
        );
        let mut want_x = xs;
        want_x.push("crew-b".to_string());
        assert_eq!(order(&s, "crew-x"), want_x, "排在新父的末尾");
    }

    #[test]
    fn the_top_level_order_is_crews_position_and_leaves_out_the_management_crew() {
        let mut s = store();
        let roots = top(&s);
        assert_eq!(roots, ["crew-a", "crew-x"]);
        let mut rotated = roots.clone();
        rotated.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, None, &rotated));
        assert_eq!(top(&s), rotated);
        let mut with_chief = roots.clone();
        with_chief.push("chief".into());
        assert!(
            matches!(
                err(&mut s, |tx| reorder(tx, None, &with_chief)),
                CollabError::Invalid(_)
            ),
            "总机组不在顶层次序里"
        );
        // 总机组没有子机组：排它名下 = 它不在树里。
        match err(&mut s, |tx| reorder(tx, Some("chief"), &[])) {
            CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewManagementNotInTree),
            e => panic!("{e:?}"),
        }
        assert_eq!(
            get(s.conn(), "crew-x").unwrap().position,
            1,
            "顶层次序写在 crews.position"
        );
    }

    #[test]
    fn purge_puts_orphaned_children_last_at_the_top_level() {
        let mut s = store();
        let solo = kid(&mut s, "只有一个父", "crew-a");
        let shared = kid(&mut s, "另有父", "crew-a");
        ok(&mut s, |tx| adopt(tx, &shared.id, "crew-x")); // 另有父 crew-x，不会变顶层
        let roots_before = top(&s);
        archive_subtree(&mut s, "crew-a");
        ok(&mut s, |tx| purge(tx, "crew-a"));
        let roots = top(&s);
        let mut want: Vec<String> = roots_before.into_iter().filter(|c| c != "crew-a").collect();
        want.extend(["crew-b".to_string(), solo.id.clone()]);
        assert_eq!(
            roots, want,
            "没了父的（按原先的次序）排在顶层末尾，原有的顶层机组不动"
        );
        assert!(!roots.contains(&shared.id));
        assert_eq!(edges(&s, &shared.id), ["crew-x"]);
    }

    #[test]
    fn a_failure_after_earlier_writes_rolls_all_of_them_back() {
        let mut s = store();
        let crews_before = count(&s, "SELECT COUNT(*) FROM crews");
        let chans_before = count(&s, "SELECT COUNT(*) FROM channels");
        // create 先发号、写机组、建大厅，最后挂到不存在的父下才失败。
        let e = err(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "半截".into(),
                    working_directory: "/tmp/example".into(),
                    parent_id: Some("nope".into()),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(e, CollabError::NotFound(_)));
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

    /// A child of the management crew is refused for being outside the tree, before the empty
    /// directory a caller inherited from it (the management crew has none).
    #[test]
    fn create_under_the_management_crew_names_the_tree_before_the_directory() {
        let mut s = store();
        match err(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    title: "示例".into(),
                    working_directory: String::new(),
                    parent_id: Some("chief".into()),
                    title_source: None,
                },
            )
        }) {
            CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewManagementNotInTree),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn create_above_puts_the_child_under_a_new_top_level_crew_and_refuses_the_management_crew() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            create_above(tx, new_crew("示例上级"), "crew-a")
        });
        assert_eq!(edges(&s, "crew-a"), [p.id.as_str()]);
        assert!(edges(&s, &p.id).is_empty(), "the new crew is top-level");
        let before = list(s.conn(), true).unwrap().len();
        match err(&mut s, |tx| {
            create_above(
                tx,
                NewCrew {
                    title: "示例".into(),
                    ..Default::default()
                },
                "chief",
            )
        }) {
            CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewManagementNotInTree),
            e => panic!("{e:?}"),
        }
        assert_eq!(list(s.conn(), true).unwrap().len(), before);
    }

    fn new_crew(title: &str) -> NewCrew {
        NewCrew {
            title: title.into(),
            working_directory: "/tmp/example".into(),
            ..Default::default()
        }
    }

    /// 客户端从默认列表（不含归档）排序：只给活的那部分也算数。归档的子机组相对次序不变、排在活的后面；
    /// 缺了活的或多了不认识的仍然 Invalid。
    #[test]
    fn reorder_accepts_the_live_children_and_parks_archived_ones_after_them() {
        let mut s = store();
        ok(&mut s, |tx| {
            reorder(tx, None, &["crew-x".into(), "crew-a".into()])
        });
        let n = ok(&mut s, |tx| create(tx, new_crew("新顶层")));
        ok(&mut s, |tx| archive(tx, "crew-x"));
        ok(&mut s, |tx| archive(tx, &n.id));
        assert_eq!(top(&s), ["crew-x", "crew-a", n.id.as_str()], "归档不动次序");
        // 缺活的 / 多出不认识的：Invalid，什么都不变。
        assert!(matches!(
            err(&mut s, |tx| reorder(tx, None, &[])),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| reorder(
                tx,
                None,
                &["crew-a".into(), "nope".into()]
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| reorder(
                tx,
                None,
                &["crew-a".into(), "crew-a".into()]
            )),
            CollabError::Invalid(_)
        ));
        // 只给活的：活的在前，归档的保持原来的相对次序（crew-x 在 n 前）接在后面。
        let got = ok(&mut s, |tx| reorder(tx, None, &["crew-a".into()]));
        let want = ["crew-a", "crew-x", n.id.as_str()];
        assert_eq!(top(&s), want);
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            want,
            "返回最终全序"
        );
        // 再排一次也稳定（幂等）。
        ok(&mut s, |tx| reorder(tx, None, &["crew-a".into()]));
        assert_eq!(top(&s), want);
        // 全序（含归档）照旧收，按客户端给的次序。
        ok(&mut s, |tx| {
            reorder(tx, None, &[n.id.clone(), "crew-a".into(), "crew-x".into()])
        });
        assert_eq!(top(&s), [n.id.as_str(), "crew-a", "crew-x"]);
        // 混合：提到一部分归档的（crew-x，按给的次序放在前面）、没提到的（n、m）保持原来的相对次序接在后面。
        let m = ok(&mut s, |tx| create(tx, new_crew("又一个")));
        ok(&mut s, |tx| archive(tx, &m.id));
        assert_eq!(top(&s), [n.id.as_str(), "crew-a", "crew-x", m.id.as_str()]);
        let got = ok(&mut s, |tx| {
            reorder(tx, None, &["crew-x".into(), "crew-a".into()])
        });
        let want = ["crew-x", "crew-a", n.id.as_str(), m.id.as_str()];
        assert_eq!(top(&s), want);
        assert_eq!(got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), want);
    }

    /// 移除 = 冻结：已移除的父机组名下的子机组次序也不能改（`crew.removed`，什么都不变）；顶层、活的父照旧。
    #[test]
    fn reorder_under_a_removed_parent_is_refused_and_changes_nothing() {
        let mut s = store();
        let p = ok(&mut s, |tx| create(tx, new_crew("要移除的父")));
        let k1 = kid(&mut s, "子一", &p.id);
        let k2 = kid(&mut s, "子二", &p.id);
        ok(&mut s, |tx| archive(tx, &k1.id));
        ok(&mut s, |tx| archive(tx, &k2.id));
        ok(&mut s, |tx| archive(tx, &p.id));
        let before = order(&s, &p.id);
        assert_eq!(before, [k1.id.clone(), k2.id.clone()]);
        let flipped = vec![k2.id.clone(), k1.id.clone()];
        match err(&mut s, |tx| reorder(tx, Some(&p.id), &flipped)) {
            CollabError::Conflict(r) => assert_eq!(r.code, Code::CrewRemoved),
            other => panic!("want Conflict(crew.removed), got {other:?}"),
        }
        assert_eq!(order(&s, &p.id), before, "次序没变");
        // 顶层（没有父）不受影响。
        ok(&mut s, |tx| {
            reorder(tx, None, &["crew-x".into(), "crew-a".into()])
        });
    }

    fn wake_req(
        session: &str,
        kind: crate::wakes::WakeKind,
        source: String,
    ) -> crate::wakes::WakeRequest {
        crate::wakes::WakeRequest {
            session: session.into(),
            kind,
            source,
            priority: crate::wakes::WakePriority::Auto,
            not_before_ms: None,
            note: None,
            origin: None,
        }
    }

    fn enqueue_id(
        s: &mut crewd_store::Store,
        session: &str,
        kind: crate::wakes::WakeKind,
        source: String,
    ) -> i64 {
        match ok(s, |tx| {
            crate::wakes::enqueue(tx, wake_req(session, kind, source))
        }) {
            crate::wakes::Enqueued::New(id) => id,
            d => panic!("{d:?}"),
        }
    }

    fn wake_state(
        s: &crewd_store::Store,
        session: &str,
        id: i64,
    ) -> (crate::wakes::WakeState, Option<String>) {
        let r = crate::wakes::list(s.conn(), Some(session), 50)
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap();
        (r.state, r.last_error)
    }

    /// 还有唤醒正在投递（inflight）时归档被拒：否则 M3c 随后的 mark_delivered / requeue 会撞上 dropped。
    /// 拒绝时什么都不变（排队的唤醒没被作废、问没被撤、机组没归档）。
    #[test]
    fn archiving_is_refused_while_a_wake_is_inflight_and_changes_nothing() {
        use crate::wakes::{self, WakeKind, WakeState};
        let mut s = store();
        let pending = enqueue_id(
            &mut s,
            "cap-x",
            WakeKind::AskAnswer,
            wakes::source::ask("crew-x", "H1"),
        );
        let inflight = enqueue_id(
            &mut s,
            "cap-x",
            WakeKind::Message,
            wakes::source::message("m-1"),
        );
        ok(&mut s, |tx| wakes::claim(tx, &[inflight]).map(|_| ()));
        let e = err(&mut s, |tx| archive(tx, "crew-x"));
        match &e {
            CollabError::Conflict(r) => {
                assert_eq!(r.code, crate::refusal::Code::CrewWakesInflight, "{e:?}");
                assert_eq!(r.value("count"), Some("1"));
                assert_eq!(r.value("crew"), Some("crew-x"));
            }
            e => panic!("{e:?}"),
        }
        assert!(get(s.conn(), "crew-x").unwrap().removed_at.is_none());
        assert_eq!(wake_state(&s, "cap-x", pending), (WakeState::Pending, None));
        assert_eq!(
            wake_state(&s, "cap-x", inflight),
            (WakeState::Inflight, None)
        );
        // 投递结束后可以归档，M3c 的收尾（mark_delivered）不会撞上。
        ok(&mut s, |tx| wakes::mark_delivered(tx, &[inflight]));
        ok(&mut s, |tx| archive(tx, "crew-x"));
    }

    /// 归档把这个机组的 session 排着队的唤醒（pending，含 requeue 回来的）作废（原因 "crew archived"，
    /// 同一个事务）；已投递的、别的机组的唤醒不动。归档的 session 起不来，留着的唤醒只会白占队。
    #[test]
    fn archiving_drops_the_pending_wakes_of_the_crews_sessions_and_nothing_else() {
        use crate::wakes::{self, WakeKind, WakeState};
        let mut s = store();
        let answer = enqueue_id(
            &mut s,
            "cap-x",
            WakeKind::AskAnswer,
            wakes::source::ask("crew-x", "H1"),
        );
        let requeued = enqueue_id(
            &mut s,
            "cap-x",
            WakeKind::Message,
            wakes::source::message("m-1"),
        );
        ok(&mut s, |tx| wakes::claim(tx, &[requeued]).map(|_| ()));
        ok(&mut s, |tx| wakes::requeue(tx, &[requeued]));
        let delivered = enqueue_id(
            &mut s,
            "cap-x",
            WakeKind::Message,
            wakes::source::message("m-2"),
        );
        ok(&mut s, |tx| wakes::mark_delivered(tx, &[delivered]));
        let other = enqueue_id(
            &mut s,
            "cap-a",
            WakeKind::Message,
            wakes::source::message("m-3"),
        );
        ok(&mut s, |tx| archive(tx, "crew-x"));
        for w in [answer, requeued] {
            assert_eq!(
                wake_state(&s, "cap-x", w),
                (WakeState::Dropped, Some("crew archived".to_string())),
                "wake {w}"
            );
        }
        assert_eq!(
            wake_state(&s, "cap-x", delivered),
            (WakeState::Delivered, None)
        );
        assert_eq!(wake_state(&s, "cap-a", other), (WakeState::Pending, None));
    }

    #[test]
    fn a_new_crew_without_a_parent_lands_last_at_the_top_level() {
        let mut s = store();
        let before = top(&s);
        let c = ok(&mut s, |tx| create(tx, new_crew("新机组")));
        assert!(parents(s.conn(), &c.id).unwrap().is_empty());
        let mut want = before;
        want.push(c.id.clone());
        assert_eq!(top(&s), want, "顶层的末尾");
        assert!(is_home(s.conn(), &c.id).unwrap());
        assert!(
            get(s.conn(), "chief").unwrap().child_ids.is_empty(),
            "总机组没有子机组"
        );
    }

    #[test]
    fn the_first_crew_in_an_empty_database_creates_the_management_crew_outside_the_tree() {
        let mut s = crewd_store::Store::open_in_memory().unwrap();
        let (c, changes) = ok_at(&mut s, NOW, |tx| create(tx, new_crew("第一个")));
        let m = management(s.conn()).unwrap().unwrap();
        assert_eq!(m, crewd_store::crews::MANAGEMENT_ID);
        assert!(parents(s.conn(), &c.id).unwrap().is_empty(), "新机组在顶层");
        assert!(
            parents(s.conn(), &m).unwrap().is_empty() && children(s.conn(), &m).unwrap().is_empty()
        );
        assert_eq!(top(&s), std::slice::from_ref(&c.id), "总机组不在顶层次序里");
        assert!(crate::channels::get(s.conn(), &crewd_store::channels::lobby_id(&m)).is_ok());
        // 顺带建出的群（总机组的大厅、新机组的大厅）和别的建群一样发 ChannelChanged。
        for lobby in [
            crewd_store::channels::lobby_id(&m),
            crewd_store::channels::lobby_id(&c.id),
        ] {
            assert!(
                changes.contains(&Change::ChannelChanged {
                    channel_id: lobby.clone()
                }),
                "{lobby}: {changes:?}"
            );
        }
        assert!(changes.contains(&Change::CrewChanged { crew_id: m }));
    }

    /// 总机组已在：只有新机组的大厅是新建的群，总机组的大厅不发；总机组也没有变（不是谁的父）。
    #[test]
    fn a_new_crews_lobby_is_announced_and_an_existing_management_lobby_is_not() {
        let mut s = store();
        let (c, changes) = ok_at(&mut s, NOW, |tx| create(tx, new_crew("新机组")));
        let channel_changes: Vec<&Change> = changes
            .iter()
            .filter(|x| matches!(x, Change::ChannelChanged { .. }))
            .collect();
        assert_eq!(
            channel_changes,
            [&Change::ChannelChanged {
                channel_id: crewd_store::channels::lobby_id(&c.id)
            }]
        );
        assert!(
            !changes.contains(&Change::CrewChanged {
                crew_id: "chief".into()
            }),
            "{changes:?}"
        );
    }

    fn not_in_tree(e: CollabError) {
        match e {
            CollabError::Invalid(r) => assert_eq!(r.code, Code::CrewManagementNotInTree),
            e => panic!("{e:?}"),
        }
    }

    /// 总机组不在机组树里（PM 2026-10-07）：给它挂上级、往它下面挂机组、从它摘出 / 摘到它下面、在它下面建机组，
    /// 一律 crew.management_not_in_tree，什么都不变；不能归档 / 永久删除（crew.management_fixed）。
    #[test]
    fn the_management_crew_is_outside_the_tree_and_cannot_be_archived_or_purged() {
        let mut s = store();
        let edges_before = count(&s, "SELECT COUNT(*) FROM crew_parents");
        let crews_before = count(&s, "SELECT COUNT(*) FROM crews");
        not_in_tree(err(&mut s, |tx| adopt(tx, "chief", "crew-a")));
        not_in_tree(err(&mut s, |tx| adopt(tx, "crew-a", "chief")));
        not_in_tree(err(&mut s, |tx| release(tx, "chief", "crew-a", None)));
        not_in_tree(err(&mut s, |tx| release(tx, "crew-a", "chief", None)));
        not_in_tree(err(&mut s, |tx| {
            release(tx, "crew-b", "crew-a", Some("chief"))
        }));
        not_in_tree(err(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    parent_id: Some("chief".into()),
                    ..new_crew("挂在总机组下")
                },
            )
        }));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM crew_parents"), edges_before);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM crews"), crews_before);
        assert_eq!(edges(&s, "crew-b"), ["crew-a"]);
        for e in [
            err(&mut s, |tx| archive(tx, "chief")),
            err(&mut s, |tx| purge(tx, "chief")),
        ] {
            match e {
                CollabError::Forbidden(r) => assert_eq!(r.code, Code::CrewManagementFixed),
                e => panic!("{e:?}"),
            }
        }
    }

    #[test]
    fn adopting_a_top_level_crew_archives_its_human_channel_and_releasing_it_brings_it_back() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-a"));
        assert_eq!(parents(s.conn(), "crew-x").unwrap(), ["crew-a"]);
        assert_eq!(home_of(s.conn(), "crew-x").unwrap(), "crew-a");
        assert!(!top(&s).contains(&"crew-x".to_string()));
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some()
        );
        ok(&mut s, |tx| release(tx, "crew-x", "crew-a", None));
        assert!(
            parents(s.conn(), "crew-x").unwrap().is_empty(),
            "摘出 = 回到顶层"
        );
        assert_eq!(top(&s).last().map(String::as_str), Some("crew-x"));
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_none()
        );
    }

    /// 多父的机组摘掉一个父：还挂在别的上级下，不进顶层；摘掉最后一个才成为顶层机组、排在末尾。
    #[test]
    fn only_releasing_the_last_parent_makes_a_crew_top_level() {
        let mut s = store();
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        let before = top(&s);
        let (c, changes) = ok_at(&mut s, NOW, |tx| release(tx, "crew-b", "crew-a", None));
        assert_eq!(c.parent_ids, ["crew-x"]);
        assert_eq!(top(&s), before, "顶层不变");
        assert!(changes.contains(&Change::CrewChanged {
            crew_id: "crew-a".into()
        }));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-x", None));
        let mut want = before;
        want.push("crew-b".into());
        assert_eq!(top(&s), want);
        assert!(is_home(s.conn(), "crew-b").unwrap());
    }

    #[test]
    fn purging_a_parent_makes_its_only_child_top_level() {
        let mut s = store();
        archive_subtree(&mut s, "crew-a");
        ok(&mut s, |tx| purge(tx, "crew-a"));
        assert!(parents(s.conn(), "crew-b").unwrap().is_empty());
        assert_eq!(top(&s), ["crew-x", "crew-b"]);
        assert!(is_home(s.conn(), "crew-b").unwrap());
    }

    #[test]
    fn reorder_without_a_parent_orders_the_top_level_and_the_management_crew_is_renamable() {
        let mut s = store();
        let (got, changes) = ok_at(&mut s, NOW, |tx| {
            reorder(tx, None, &["crew-x".to_string(), "crew-a".to_string()])
        });
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["crew-x", "crew-a"]
        );
        assert_eq!(top(&s), ["crew-x", "crew-a"]);
        assert!(
            changes
                .iter()
                .all(|c| matches!(c, Change::CrewChanged { crew_id } if crew_id != "chief")),
            "{changes:?}"
        );
        ok(&mut s, |tx| {
            update(
                tx,
                "chief",
                CrewPatch {
                    title: Some("管理层".into()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(get(s.conn(), "chief").unwrap().title, "管理层");
    }

    /// 森林的不变式：任何结构操作（含被拒的）之后，总机组没有父边也没有子边；每个非管理机组往上走得到顶层机组
    /// （`home_pointer` 不报数据不一致）；没有环。
    #[test]
    fn after_any_structural_op_the_forest_invariants_hold() {
        let mut s = store();
        let check = |s: &crewd_store::Store, step: &str| {
            assert_eq!(
                count(
                    s,
                    "SELECT COUNT(*) FROM crew_parents p JOIN crews m ON m.management=1
                      AND (p.crew_id=m.id OR p.parent_id=m.id)"
                ),
                0,
                "{step}：总机组没有边"
            );
            for c in crewd_store::crews::list(s.conn()).unwrap() {
                if c.management {
                    continue;
                }
                let home = home_of(s.conn(), &c.id).unwrap();
                assert!(
                    parents(s.conn(), &home).unwrap().is_empty()
                        && !is_management(s.conn(), &home).unwrap(),
                    "{step}：{} 的 home {home} 是顶层机组",
                    c.id
                );
                assert!(
                    !descendants(s.conn(), &c.id).unwrap().contains(&c.id),
                    "{step}：{} 不在环里",
                    c.id
                );
            }
        };
        check(&s, "初始");
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        check(&s, "adopt");
        ok(&mut s, |tx| release(tx, "crew-x", "crew-b", None));
        check(&s, "release 到顶层");
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("crew-x")));
        check(&s, "release(to=crew-x)");
        let _ = err(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        check(&s, "被拒的成环");
        let _ = err(&mut s, |tx| adopt(tx, "crew-a", "chief"));
        check(&s, "被拒的挂到总机组下");
        let k = ok(&mut s, |tx| {
            create(
                tx,
                NewCrew {
                    parent_id: Some("crew-b".into()),
                    ..new_crew("孙机组")
                },
            )
        });
        check(&s, "create(parent=crew-b)");
        archive_subtree(&mut s, "crew-x");
        check(&s, "archive");
        ok(&mut s, |tx| purge(tx, "crew-x"));
        check(&s, "purge");
        assert!(parents(s.conn(), "crew-b").unwrap().is_empty());
        assert_eq!(edges(&s, &k.id), ["crew-b"]);
        ok(&mut s, |tx| create(tx, new_crew("再建一个")));
        check(&s, "create");
    }

    /// 号码：空库里第一次建机组，懒建出来的总机组是 0 号，第一个机组是 1 号（N1 裁定 2 / N1-R1）。
    #[test]
    fn in_an_empty_database_the_management_crew_is_number_zero_and_the_first_crew_number_one() {
        let mut s = crewd_store::Store::open_in_memory().unwrap();
        let c = ok(&mut s, |tx| create(tx, new_crew("第一个")));
        assert_eq!(c.number, Some(1));
        let m = get(s.conn(), crewd_store::crews::MANAGEMENT_ID).unwrap();
        assert!(m.management);
        assert_eq!(m.number, Some(0));
        let d = ok(&mut s, |tx| create(tx, new_crew("第二个")));
        assert_eq!(d.number, Some(2));
    }

    /// `crew.list` 的次序：总机组单独排第一（不看 `crews.position` / 建组时间），然后森林的先序——顶层按
    /// `crews.position`，每层按父的 childIds。
    #[test]
    fn crew_list_puts_the_management_crew_first_then_the_forest_in_sidebar_order() {
        let mut s = store();
        // 总机组建得最晚、position 最大：照样排第一。
        s.conn()
            .execute(
                "UPDATE crews SET position=99, created_at='2099-01-01T00:00:00.000Z' WHERE id='chief'",
                [],
            )
            .unwrap();
        let n = ok(&mut s, |tx| create(tx, new_crew("新机组")));
        let ids = |s: &crewd_store::Store| -> Vec<String> {
            list(s.conn(), true)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(
            ids(&s),
            ["chief", "crew-a", "crew-b", "crew-x", n.id.as_str()]
        );
        ok(&mut s, |tx| {
            reorder(tx, None, &[n.id.clone(), "crew-x".into(), "crew-a".into()])
        });
        assert_eq!(
            ids(&s),
            ["chief", n.id.as_str(), "crew-x", "crew-a", "crew-b"]
        );
        // 多父的机组排在第一次遇到的位置，只出现一次。
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(
            ids(&s),
            ["chief", n.id.as_str(), "crew-x", "crew-b", "crew-a"]
        );
    }

    /// 总机组不能被隐藏；取消隐藏 / 别的机组照常。
    #[test]
    fn the_management_crew_cannot_be_hidden() {
        let mut s = store();
        let hide = |h: bool| CrewPatch {
            hidden: Some(h),
            ..Default::default()
        };
        match err(&mut s, |tx| update(tx, "chief", hide(true))) {
            CollabError::Forbidden(r) => assert_eq!(r.code, Code::CrewManagementFixed),
            e => panic!("{e:?}"),
        }
        assert!(get(s.conn(), "chief").unwrap().hidden_at.is_none());
        ok(&mut s, |tx| update(tx, "chief", hide(false)));
        let x = ok(&mut s, |tx| update(tx, "crew-x", hide(true)));
        assert!(x.hidden_at.is_some());
    }

    /// `home_of` 不再先 `get`（那会把全部机组连同父子边读一遍）：不存在的机组仍是 NotFound（由 `is_management` 报）。
    #[test]
    fn home_of_an_unknown_crew_is_not_found() {
        let s = store();
        assert!(matches!(
            home_of(s.conn(), "nope").unwrap_err(),
            CollabError::NotFound(_)
        ));
        assert!(matches!(
            is_home(s.conn(), "nope").unwrap_err(),
            CollabError::NotFound(_)
        ));
    }

    /// 一个顶层机组被收编、再多挂一个父、逐个摘掉：home 身份、人类群的归档 / 解档、机长「在」的群、
    /// 提问的转达对象都跟着树走；全程总机组没有边。
    #[test]
    fn moving_a_top_level_crew_down_and_back_keeps_home_human_channel_and_relay_in_step() {
        let mut s = store();
        let management_edges = |s: &crewd_store::Store| {
            count(
                s,
                "SELECT COUNT(*) FROM crew_parents p JOIN crews m ON m.management=1
                  AND (p.crew_id=m.id OR p.parent_id=m.id)",
            )
        };
        let in_human = |s: &crewd_store::Store| {
            crate::channels::channels_of_session(s.conn(), "cap-x")
                .unwrap()
                .iter()
                .any(|c| c.id == "crew-x/human")
        };
        let archived = |s: &crewd_store::Store| {
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some()
        };
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        assert!(in_human(&s), "顶层机组的机长在自己的人类群里");

        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-a"));
        assert!(!is_home(s.conn(), "crew-x").unwrap() && archived(&s) && !in_human(&s));
        // 提问的机长恢复失败：转达给它此刻的 home（crew-a）的机长。
        let a = ok(&mut s, |tx| {
            crate::asks::open(
                tx,
                "cap-x",
                crate::asks::OpenAsk {
                    question: "示例问题".into(),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            crate::sessions::set_resume_failed(tx, "cap-x", true)
        });
        ok(&mut s, |tx| {
            crate::asks::answer(tx, "crew-x", &a.ask, "示例回答")
        });
        assert_eq!(
            crate::wakes::list(s.conn(), Some("cap-a"), 10).unwrap()[0].source,
            format!("ask:crew-x/{}", a.ask)
        );

        // 再多挂一个父、摘掉一个：仍不是 home，人类群保持归档。
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        assert_eq!(edges(&s, "crew-x"), ["crew-a", "crew-b"]);
        ok(&mut s, |tx| release(tx, "crew-x", "crew-a", None));
        assert_eq!(edges(&s, "crew-x"), ["crew-b"]);
        assert!(!is_home(s.conn(), "crew-x").unwrap() && archived(&s));
        assert_eq!(management_edges(&s), 0);

        // 摘掉最后一个父：回到顶层，重新是 home，人类群解档，机长又在里面。
        ok(&mut s, |tx| release(tx, "crew-x", "crew-b", None));
        assert!(edges(&s, "crew-x").is_empty());
        assert!(is_home(s.conn(), "crew-x").unwrap() && !archived(&s) && in_human(&s));

        // 非 home 的二级机组收编到别处：从来没有人类群，也不会凭空建一个。
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(edges(&s, "crew-b"), ["crew-a", "crew-x"]);
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM channels WHERE id='crew-b/human'"),
            0
        );
        assert_eq!(management_edges(&s), 0);
    }

    // ---- 删机组 = 归档（N1 裁定 13，Task 9）----

    /// 永久删除只收已归档的机组：子树先从叶子往上逐个归档（后序：子先于父）。
    fn archive_subtree(s: &mut crewd_store::Store, id: &str) {
        for k in children(s.conn(), id).unwrap() {
            archive_subtree(s, &k);
        }
        if get(s.conn(), id).unwrap().removed_at.is_none() {
            ok(s, |tx| archive(tx, id));
        }
    }

    #[test]
    fn archiving_a_crew_keeps_everything_and_unarchiving_brings_it_back() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                crate::Sender::Human,
                crate::messages::NewPost {
                    channel_id: "crew-x/lobby".into(),
                    text: "归档前的一句".into(),
                    ..Default::default()
                },
            )
        });
        let c = ok(&mut s, |tx| archive(tx, "crew-x"));
        assert!(c.removed_at.is_some());
        for ch in ["crew-x/lobby", "crew-x/human"] {
            assert!(
                crate::channels::get(s.conn(), ch)
                    .unwrap()
                    .archived_at
                    .is_some(),
                "{ch}"
            );
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) FROM messages WHERE channel_id='crew-x/lobby'"
            ),
            1,
            "消息保留"
        );
        assert!(
            crate::sessions::get(s.conn(), "cap-x").is_ok(),
            "session 行保留"
        );
        assert!(
            matches!(
                err(&mut s, |tx| crate::sessions::register(
                    tx,
                    crate::sessions::NewSession {
                        crew_id: "crew-x".into(),
                        role: crewd_proto::Role::Worker,
                        executor: "claude".into(),
                        title: Some("新人".into()),
                        ..Default::default()
                    }
                )),
                CollabError::Conflict(_)
            ),
            "归档的机组不收新 session"
        );
        let c = ok(&mut s, |tx| unarchive(tx, "crew-x"));
        assert!(c.removed_at.is_none());
        for ch in ["crew-x/lobby", "crew-x/human"] {
            assert!(
                crate::channels::get(s.conn(), ch)
                    .unwrap()
                    .archived_at
                    .is_none(),
                "{ch}"
            );
        }
    }

    #[test]
    fn archiving_refuses_the_management_crew_running_sessions_and_live_children() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| archive(tx, "chief")),
            CollabError::Forbidden(_)
        ));
        assert!(
            matches!(
                err(&mut s, |tx| archive(tx, "crew-a")),
                CollabError::Conflict(_)
            ),
            "crew-b 还在它下面"
        );
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", [])
            .unwrap();
        assert!(matches!(
            err(&mut s, |tx| archive(tx, "crew-x")),
            CollabError::Conflict(_)
        ));
        // 子机组归档了，父就能归档（归档的子不算「还在下面干活」）。
        ok(&mut s, |tx| archive(tx, "crew-b"));
        ok(&mut s, |tx| archive(tx, "crew-a"));
    }

    /// 归档只动归属这个机组的群：topic 一起归档（解档时留着，人逐个解档），别的机组的群不动；
    /// 每个被归档的群一条 ChannelChanged，机组一条 CrewChanged；再归档一次 = 原样返回、不发变更。
    #[test]
    fn archiving_notes_every_channel_it_archives_and_unarchiving_leaves_topics_archived() {
        use crate::channels::{self, NewChannel};
        use crewd_proto::{ChannelKind, MemberRef};
        let mut s = store();
        ok(&mut s, |tx| channels::ensure_human_channel(tx, "crew-x"));
        let topic = ok(&mut s, |tx| {
            channels::create(
                tx,
                &crate::Sender::Session("cap-x".into()),
                NewChannel {
                    crew_id: "crew-x".into(),
                    kind: ChannelKind::Topic,
                    name: "丙的群".into(),
                    purpose: None,
                    members: vec![MemberRef::session("w-a1")],
                },
            )
        });
        let elsewhere = ok(&mut s, |tx| {
            channels::create(
                tx,
                &crate::Sender::Session("w-a1".into()),
                NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "甲的群".into(),
                    purpose: None,
                    members: vec![MemberRef::session("cap-x")],
                },
            )
        });
        let (_, ch) = ok_at(&mut s, NOW, |tx| archive(tx, "crew-x"));
        for id in ["crew-x/lobby", "crew-x/human", topic.id.as_str()] {
            assert!(
                ch.contains(&Change::ChannelChanged {
                    channel_id: id.to_string()
                }),
                "{id}：{ch:?}"
            );
            assert!(channels::get(s.conn(), id).unwrap().archived_at.is_some());
        }
        assert!(ch.contains(&Change::CrewChanged {
            crew_id: "crew-x".into()
        }));
        assert!(
            channels::get(s.conn(), &elsewhere.id)
                .unwrap()
                .archived_at
                .is_none(),
            "别的机组的群不动"
        );
        let (again, ch) = ok_at(&mut s, NOW, |tx| archive(tx, "crew-x"));
        assert!(again.removed_at.is_some());
        assert!(ch.is_empty(), "已归档：原样返回 {ch:?}");
        let (_, ch) = ok_at(&mut s, NOW, |tx| unarchive(tx, "crew-x"));
        for id in ["crew-x/lobby", "crew-x/human"] {
            assert!(
                ch.contains(&Change::ChannelChanged {
                    channel_id: id.to_string()
                }),
                "{id}：{ch:?}"
            );
        }
        assert!(
            channels::get(s.conn(), &topic.id)
                .unwrap()
                .archived_at
                .is_some(),
            "topic 留归档，人逐个解档"
        );
        // 解档后 topic 可以由人解档（机组不再只读）。
        ok(&mut s, |tx| {
            channels::update(
                tx,
                &crate::Sender::Human,
                &topic.id,
                channels::ChannelPatch {
                    archived: Some(false),
                    ..Default::default()
                },
            )
        });
    }

    /// 往归档机组的任何群发言都是同一个答案（Task 9 复审）：Conflict + `channel.archived`，不论大厅、人类群还是
    /// topic（之前大厅 -32602、人类群 -32009）。
    #[test]
    fn posting_to_any_channel_of_an_archived_crew_gets_one_code() {
        use crate::channels::{self, NewChannel};
        use crate::refusal::Code;
        use crewd_proto::ChannelKind;
        let mut s = store();
        ok(&mut s, |tx| channels::ensure_human_channel(tx, "crew-x"));
        let topic = ok(&mut s, |tx| {
            channels::create(
                tx,
                &crate::Sender::Human,
                NewChannel {
                    crew_id: "crew-x".into(),
                    kind: ChannelKind::Topic,
                    name: "议题".into(),
                    purpose: None,
                    members: vec![],
                },
            )
        });
        ok(&mut s, |tx| archive(tx, "crew-x"));
        for ch in ["crew-x/lobby", "crew-x/human", topic.id.as_str()] {
            let e = err(&mut s, |tx| {
                crate::messages::post(
                    tx,
                    crate::Sender::Human,
                    crate::messages::NewPost {
                        channel_id: ch.into(),
                        text: "还在吗".into(),
                        ..Default::default()
                    },
                )
            });
            match &e {
                CollabError::Conflict(r) => assert_eq!(r.code, Code::ChannelArchived, "{ch}"),
                e => panic!("{ch}: {e:?}"),
            }
        }
    }

    /// 归档的机组只读（Task 9 / preflight M4）：结构操作、改名、换机长、起 session、建群、解档它的群、
    /// 往它的人类群发言一律 Conflict，指路先解档；channel.list 不会顺手把它的人类群解档。
    #[test]
    fn an_archived_crew_is_read_only_until_unarchived() {
        use crate::channels::{self, NewChannel};
        use crewd_proto::{ChannelKind, MemberRef};
        let mut s = store();
        ok(&mut s, |tx| channels::ensure_human_channel(tx, "crew-x"));
        ok(&mut s, |tx| archive(tx, "crew-x"));
        let conflict = |e: CollabError, what: &str| {
            assert!(matches!(e, CollabError::Conflict(_)), "{what}: {e:?}");
        };
        conflict(
            err(&mut s, |tx| {
                create(
                    tx,
                    NewCrew {
                        parent_id: Some("crew-x".into()),
                        ..new_crew("挂到归档的下面")
                    },
                )
            }),
            "create(parent=归档)",
        );
        conflict(
            err(&mut s, |tx| adopt(tx, "crew-b", "crew-x")),
            "adopt 到归档的下面",
        );
        conflict(
            err(&mut s, |tx| adopt(tx, "crew-x", "crew-a")),
            "adopt 归档的",
        );
        conflict(
            err(&mut s, |tx| release(tx, "crew-x", "crew-a", Some("crew-b"))),
            "release 归档的",
        );
        conflict(
            err(&mut s, |tx| {
                update(
                    tx,
                    "crew-x",
                    CrewPatch {
                        title: Some("改名".into()),
                        ..Default::default()
                    },
                )
            }),
            "update",
        );
        conflict(
            err(&mut s, |tx| set_captain(tx, "crew-x", "cap-x")),
            "set_captain",
        );
        conflict(
            err(&mut s, |tx| {
                channels::create(
                    tx,
                    &crate::Sender::Human,
                    NewChannel {
                        crew_id: "crew-x".into(),
                        kind: ChannelKind::Topic,
                        name: "新群".into(),
                        purpose: None,
                        members: vec![],
                    },
                )
            }),
            "channels::create",
        );
        conflict(
            err(&mut s, |tx| {
                channels::ensure_dm(
                    tx,
                    "crew-x",
                    &MemberRef::session("cap-x"),
                    &MemberRef::Human,
                )
            }),
            "ensure_dm",
        );
        conflict(
            err(&mut s, |tx| {
                channels::update(
                    tx,
                    &crate::Sender::Human,
                    "crew-x/lobby",
                    channels::ChannelPatch {
                        purpose: Some("改用途".into()),
                        ..Default::default()
                    },
                )
            }),
            "channels::update",
        );
        conflict(
            err(&mut s, |tx| channels::ensure_human_channel(tx, "crew-x")),
            "ensure_human_channel",
        );
        conflict(
            err(&mut s, |tx| {
                crate::messages::post(
                    tx,
                    crate::Sender::Human,
                    crate::messages::NewPost {
                        channel_id: "crew-x/human".into(),
                        text: "还在吗".into(),
                        ..Default::default()
                    },
                )
            }),
            "往归档机组的人类群发言",
        );
        conflict(
            err(&mut s, |tx| {
                crate::sessions::set_desired(tx, "cap-x", crewd_proto::Desired::Running)
            }),
            "起归档机组的 session",
        );
        // 停（desired=stopped）照样能写：只拦「跑起来」。
        ok(&mut s, |tx| {
            crate::sessions::set_desired(tx, "cap-x", crewd_proto::Desired::Stopped)
        });
        // 读照常：channel.list / crew.get 列得出它的群，但不顺手把人类群解档。
        let listed = ok(&mut s, |tx| channels::list_for_crew(tx, "crew-x"));
        assert!(listed.iter().any(|c| c.channel.id == "crew-x/human"));
        ok(&mut s, |tx| detail(tx, "crew-x"));
        assert!(
            channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some(),
            "列一下不等于解档"
        );
        ok(&mut s, |tx| unarchive(tx, "crew-x"));
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        ok(&mut s, |tx| set_captain(tx, "crew-x", "cap-x"));
    }

    #[test]
    fn unarchiving_under_an_archived_parent_is_refused() {
        let mut s = store();
        ok(&mut s, |tx| archive(tx, "crew-b"));
        ok(&mut s, |tx| archive(tx, "crew-a"));
        assert!(
            matches!(
                err(&mut s, |tx| unarchive(tx, "crew-b")),
                CollabError::Conflict(_)
            ),
            "上级 crew-a 还归档着"
        );
        ok(&mut s, |tx| unarchive(tx, "crew-a"));
        ok(&mut s, |tx| unarchive(tx, "crew-b"));
        // 没归档的机组解档 = 原样返回。
        let (c, ch) = ok_at(&mut s, NOW, |tx| unarchive(tx, "crew-b"));
        assert!(c.removed_at.is_none() && ch.is_empty(), "{ch:?}");
        assert!(matches!(
            err(&mut s, |tx| unarchive(tx, "nope")),
            CollabError::NotFound(_)
        ));
    }

    /// 永久删除只收已归档的机组（先 crew.remove 移除）；前置条件也挡总机组与在跑的 session。
    #[test]
    fn purge_only_takes_an_archived_crew() {
        let mut s = store();
        assert!(matches!(
            check_purgeable(s.conn(), "crew-x").unwrap_err(),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| purge(tx, "crew-x")),
            CollabError::Conflict(_)
        ));
        assert!(get(s.conn(), "crew-x").is_ok(), "没归档的不删");
        assert!(matches!(
            check_purgeable(s.conn(), "chief").unwrap_err(),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            check_purgeable(s.conn(), "nope").unwrap_err(),
            CollabError::NotFound(_)
        ));
        ok(&mut s, |tx| archive(tx, "crew-x"));
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", [])
            .unwrap();
        assert!(matches!(
            check_purgeable(s.conn(), "crew-x").unwrap_err(),
            CollabError::Conflict(_)
        ));
        s.conn()
            .execute("UPDATE sessions SET desired='stopped' WHERE id='cap-x'", [])
            .unwrap();
        check_purgeable(s.conn(), "crew-x").unwrap();
        ok(&mut s, |tx| purge(tx, "crew-x"));
        assert!(matches!(
            get(s.conn(), "crew-x").unwrap_err(),
            CollabError::NotFound(_)
        ));
    }

    /// 永久删除父机组：只剩它这一个父的（已归档的）子机组成为顶层，但仍是归档的——人类群不顺手解档。
    #[test]
    fn purging_a_parent_leaves_its_archived_orphans_archived() {
        let mut s = store();
        // crew-x 原是顶层机组、有人类群；收编到 crew-a 下 → 人类群随之归档。
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-a"));
        archive_subtree(&mut s, "crew-a");
        ok(&mut s, |tx| purge(tx, "crew-a"));
        for orphan in ["crew-b", "crew-x"] {
            assert!(edges(&s, orphan).is_empty(), "{orphan}");
            assert!(get(s.conn(), orphan).unwrap().removed_at.is_some());
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) FROM channels WHERE id='crew-x/human' AND archived_at IS NULL"
            ),
            0,
            "归档的孤儿：人类群不解档"
        );
    }

    /// 默认列表不含归档的机组（通讯录同口径），`include_archived` 才列出来。
    #[test]
    fn archived_crews_are_left_out_of_the_default_list() {
        let mut s = store();
        ok(&mut s, |tx| archive(tx, "crew-x"));
        let ids = |v: Vec<Crew>| v.into_iter().map(|c| c.id).collect::<Vec<_>>();
        assert!(!ids(list(s.conn(), false).unwrap()).contains(&"crew-x".to_string()));
        assert!(ids(list(s.conn(), true).unwrap()).contains(&"crew-x".to_string()));
    }

    fn open_ask(s: &mut crewd_store::Store, asker: &str, q: &str) -> String {
        ok(s, |tx| {
            crate::asks::open(
                tx,
                asker,
                crate::asks::OpenAsk {
                    question: q.into(),
                    ..Default::default()
                },
            )
        })
        .ask
    }

    /// 归档撤回这个机组账本里还开着的问（同一事务，理由取 render md 的 `notice.ask.crew_removed`，按 meta.locale）；
    /// 子机组先归档，所以挂在这个 home 人类群里的问（含子机组的）都已撤回。解档不重开。
    #[test]
    fn archiving_withdraws_open_asks_and_unarchiving_does_not_reopen_them() {
        use crewd_proto::AskStateView;
        let mut s = store();
        let hb = open_ask(&mut s, "w-b1", "乙的问");
        let ha = open_ask(&mut s, "cap-a", "甲的问");
        let state = |s: &crewd_store::Store, crew: &str, h: &str| {
            crate::asks::get(s.conn(), crew, h).unwrap()
        };
        let (_, ch) = ok_at(&mut s, NOW, |tx| archive(tx, "crew-b"));
        let b = state(&s, "crew-b", &hb);
        assert_eq!(b.state, AskStateView::Withdrawn);
        assert_eq!(b.withdraw_reason.as_deref(), Some("机组已被移除"));
        assert!(ch.contains(&Change::AskChanged {
            crew_id: "crew-b".into(),
            ask: hb.clone()
        }));
        assert_eq!(
            state(&s, "crew-a", &ha).state,
            AskStateView::Open,
            "别的机组的问不动"
        );
        ok(&mut s, |tx| {
            crate::settings::set_locale(tx, crate::Locale::En)
        });
        ok(&mut s, |tx| archive(tx, "crew-a"));
        let a = state(&s, "crew-a", &ha);
        assert_eq!(a.state, AskStateView::Withdrawn);
        assert_eq!(a.withdraw_reason.as_deref(), Some("The crew was removed"));
        assert!(
            crate::asks::waits(s.conn(), NOW, Some("crew-a"))
                .unwrap()
                .is_empty(),
            "人的待确认清单里没有了"
        );
        ok(&mut s, |tx| unarchive(tx, "crew-a"));
        ok(&mut s, |tx| unarchive(tx, "crew-b"));
        assert_eq!(state(&s, "crew-a", &ha).state, AskStateView::Withdrawn);
        assert_eq!(state(&s, "crew-b", &hb).state, AskStateView::Withdrawn);
    }

    /// 还卡着活项的问账本不让撤：归档整体拒（Conflict，指路先答掉或改道），什么都不变。
    #[test]
    fn archiving_refuses_while_an_open_ask_still_blocks_a_live_item() {
        use crewd_work::{Actor, BlockOn, Continuation, NewItem, SessionId};
        let mut s = store();
        let h = ok(&mut s, |tx| {
            crate::asks::with_book(tx, "crew-x", |b, now| {
                let w = b
                    .create_item(
                        &Actor::Human,
                        now,
                        NewItem {
                            owner: Some(SessionId::new("cap-x")),
                            ..NewItem::titled("示例项")
                        },
                    )?
                    .value;
                let r = b.block(
                    &Actor::Human,
                    now,
                    w,
                    vec![BlockOn::NewAsk {
                        question: "示例问题".into(),
                        default: None,
                    }],
                    Continuation::Nothing,
                )?;
                Ok(r.value.asks[0].to_string())
            })
        });
        match err(&mut s, |tx| archive(tx, "crew-x")) {
            CollabError::Conflict(r) => {
                assert_eq!(r.code, crate::refusal::Code::AskRemoveBlocked);
                assert_eq!(r.value("ask"), Some(h.as_str()), "{r}");
            }
            e => panic!("{e:?}"),
        }
        assert!(get(s.conn(), "crew-x").unwrap().removed_at.is_none());
        assert_eq!(
            crate::asks::get(s.conn(), "crew-x", &h).unwrap().state,
            crewd_proto::AskStateView::Open
        );
    }

    const PURGE_PROVENANCE_PREFIX: &str = "message.provenance.v1.";

    fn provenance_purge_snapshot(
        s: &crewd_store::Store,
    ) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
        let mut st = s
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let names = st
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let quoted = name.replace('"', "\"\"");
                let mut rows = s
                    .conn()
                    .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid"))
                    .unwrap();
                let columns = rows.column_count();
                let values = rows
                    .query_map([], |r| {
                        (0..columns)
                            .map(|i| r.get::<_, rusqlite::types::Value>(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                (name, values)
            })
            .collect()
    }

    // Synthetic cleanup fixture only: Stop does not currently relay into a DM. Manually seeding a
    // private marker here tests purge ownership/transaction semantics, not the trusted Stop caller.
    fn provenance_purge_fixture() -> (crewd_store::Store, String, String, String, String) {
        let mut s = store();
        let (doomed_dm, _) = ok(&mut s, |tx| {
            crate::channels::ensure_dm(
                tx,
                "crew-x",
                &crewd_proto::MemberRef::session("cap-x"),
                &crewd_proto::MemberRef::session("w-a1"),
            )
        });
        let (foreign_dm, _) = ok(&mut s, |tx| {
            crate::channels::ensure_dm(
                tx,
                "crew-a",
                &crewd_proto::MemberRef::session("w-a2"),
                &crewd_proto::MemberRef::session("cap-x"),
            )
        });
        // DM reuse is global by member pair, regardless of requested crew. Distinct pairs
        // are necessary to exercise both real ownership directions instead of the same DM.
        assert_ne!(doomed_dm.id, foreign_dm.id);
        assert_eq!(doomed_dm.crew_id, "crew-x");
        assert_eq!(foreign_dm.crew_id, "crew-a");
        let post = |tx: &mut Tx<'_>, channel: &str| {
            crate::messages::post(
                tx,
                crate::Sender::System,
                crate::messages::NewPost {
                    channel_id: channel.into(),
                    text: "synthetic retained question?".into(),
                    category: Some("question".into()),
                    ..Default::default()
                },
            )
        };
        let doomed = ok(&mut s, |tx| post(tx, &doomed_dm.id)).message;
        let foreign = ok(&mut s, |tx| post(tx, &foreign_dm.id)).message;
        for m in [&doomed, &foreign] {
            let envelope = serde_json::json!({
                "version": 1, "kind": "stop_relay", "messageId": m.id,
                "seq": m.seq, "sourceSession": "cap-x"
            });
            crewd_store::meta_set(
                s.conn(),
                &format!("{PURGE_PROVENANCE_PREFIX}{}", m.id),
                &envelope.to_string(),
            )
            .unwrap();
        }
        crewd_store::meta_set(s.conn(), "messageXprovenanceXv1.orphan", "untouched").unwrap();
        crewd_store::meta_set(s.conn(), "agent.turn.unrelated", "untouched").unwrap();
        ok(&mut s, |tx| archive(tx, "crew-x"));
        (s, doomed.id, foreign.id, doomed_dm.id, foreign_dm.id)
    }

    #[test]
    fn purge_cleans_doomed_dm_markers_but_keeps_foreign_history() {
        let (mut s, doomed, foreign, doomed_dm, foreign_dm) = provenance_purge_fixture();
        let foreign_before =
            crewd_store::meta_get(s.conn(), &format!("{PURGE_PROVENANCE_PREFIX}{foreign}"))
                .unwrap();
        let ((), changes) = ok_at(&mut s, NOW + 1, |tx| purge(tx, "crew-x"));
        assert!(crate::messages::get(s.conn(), &doomed).is_err());
        assert!(crate::channels::get(s.conn(), &doomed_dm).is_err());
        assert_eq!(
            crewd_store::meta_get(s.conn(), &format!("{PURGE_PROVENANCE_PREFIX}{doomed}")).unwrap(),
            None,
            "the actual channel/message cascade must not leave an orphan provenance marker"
        );
        assert_eq!(
            crate::messages::get(s.conn(), &foreign).unwrap().channel_id,
            foreign_dm
        );
        assert_eq!(
            crewd_store::meta_get(s.conn(), &format!("{PURGE_PROVENANCE_PREFIX}{foreign}"))
                .unwrap(),
            foreign_before,
            "foreign DM history retains provenance even after its source session is purged"
        );
        assert!(crate::sessions::get(s.conn(), "cap-x").is_err());
        let retained = crate::messages::get(s.conn(), &foreign).unwrap();
        assert_eq!(
            crate::message_provenance::read(s.conn(), &retained).unwrap(),
            crate::message_provenance::MessageProvenance::StopRelay {
                source_session: "cap-x".into()
            }
        );
        assert_eq!(
            crate::channels::members(s.conn(), &foreign_dm).unwrap(),
            [crewd_proto::MemberRef::session("w-a2")]
        );
        for key in ["messageXprovenanceXv1.orphan", "agent.turn.unrelated"] {
            assert_eq!(
                crewd_store::meta_get(s.conn(), key).unwrap().as_deref(),
                Some("untouched")
            );
        }
        assert!(changes.contains(&Change::CrewRemoved {
            crew_id: "crew-x".into()
        }));
        assert!(changes.contains(&Change::ChannelRemoved {
            channel_id: doomed_dm
        }));
    }

    #[test]
    fn provenance_cleanup_failure_rolls_back_actual_purge() {
        let (mut s, doomed, _, _, _) = provenance_purge_fixture();
        // This trigger is reached only when the real post-cascade orphan cleanup deletes meta.
        // Install it through a bound key table rather than interpolating a message id into SQL.
        s.conn()
            .execute_batch(
                "CREATE TEMP TABLE synthetic_failed_provenance_key(key TEXT PRIMARY KEY);
             CREATE TEMP TRIGGER fail_provenance_cleanup BEFORE DELETE ON meta
             WHEN OLD.key IN (SELECT key FROM synthetic_failed_provenance_key)
             BEGIN SELECT RAISE(ABORT,'synthetic provenance cleanup failure'); END;",
            )
            .unwrap();
        s.conn()
            .execute(
                "INSERT INTO synthetic_failed_provenance_key(key) VALUES(?1)",
                [format!("{PURGE_PROVENANCE_PREFIX}{doomed}")],
            )
            .unwrap();
        let before = provenance_purge_snapshot(&s);
        let failed = crate::run(&mut s, NOW + 1, |tx| purge(tx, "crew-x"));
        assert!(
            matches!(&failed, Err(CollabError::Store(_))),
            "actual purge must propagate the orphan-marker DELETE failure: {failed:?}"
        );
        assert_eq!(
            provenance_purge_snapshot(&s),
            before,
            "crew, sessions, messages, membership/owner changes and every marker must roll back"
        );
        // Err contains no committed Change list; a separate server contract must observe Hub.
        s.conn()
            .execute_batch("DROP TRIGGER fail_provenance_cleanup")
            .unwrap();
        ok(&mut s, |tx| purge(tx, "crew-x"));
        assert_eq!(
            crewd_store::meta_get(s.conn(), &format!("{PURGE_PROVENANCE_PREFIX}{doomed}")).unwrap(),
            None
        );
    }
}
