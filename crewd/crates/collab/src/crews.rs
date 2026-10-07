//! 机组：根、机长、DAG、建 / 改 / 删、编号。移植 `LocalCrewStore.swift:356-526`、`CrewDirectory.swift:8-13`。
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
    CollabError::NotFound(format!("没有机组 {id}"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Crew> {
    crewd_store::crews::get(conn, id)?.ok_or_else(|| not_found(id))
}

/// `crew.list` 的次序 = 侧栏的树：总机组（树根）第一，然后先序遍历，每层按父的 `child_ids`（边上的次序，
/// `reorder` 排的就是它）；多父的机组排在第一次遇到的位置、只出现一次；从根走不到的（旧数据里断开的）
/// 按建组时间、id 排在最后。不看 `crews.position`（只对总机组有意义，N1 裁定 7）。
/// 这只是一个稳定的扁平次序；客户端建树用 `parent_ids` / `child_ids`。store 层仍按号排（M1 的测试依赖它）。
pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    let mut rest: BTreeMap<String, Crew> = crewd_store::crews::list(conn)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let mut out: Vec<Crew> = Vec::with_capacity(rest.len());
    // 先序：栈里倒着压子机组，弹出来就是 child_ids 的次序。
    let mut stack: Vec<String> = org_root(conn)?.into_iter().collect();
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
    Ok(out)
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

pub fn org_root(conn: &Connection) -> Result<Option<String>> {
    Ok(crewd_store::crews::org_root(conn)?)
}

/// 一次直查（preflight D9）：`home_of` / `is_home` 在循环里调它，不走 `get`（那会把全部机组连同父子边读一遍）。
pub fn is_org_root(conn: &Connection, id: &str) -> Result<bool> {
    conn.query_row("SELECT org_root FROM crews WHERE id=?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| not_found(id))
}

/// 有自己人类群的机组（H2）：总机组；一级机组（父边恰好是 [总机组]）。没有父边的非根机组只会出现在
/// 旧形状 / 脏数据里，当作自己的 home（全函数，同旧 `root_of` 遇环退回自己）。
pub fn is_home(conn: &Connection, id: &str) -> Result<bool> {
    if is_org_root(conn, id)? {
        return Ok(true);
    }
    let ps = parents(conn, id)?;
    Ok(ps.is_empty() || (ps.len() == 1 && is_org_root(conn, &ps[0])?))
}

/// 人类群归属（N1 裁定 5）：自己是 home → 自己；否则往上走，遇到的 home 里 created_at 最早的（再按 id）。
/// 每个祖先几次直查（O(祖先数)，不读全部机组）；不存在的机组由 `is_home` → `is_org_root` 报 NotFound。
pub fn home_of(conn: &Connection, id: &str) -> Result<String> {
    if is_home(conn, id)? {
        return Ok(id.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from(parents(conn, id)?);
    let mut homes: Vec<(String, String)> = Vec::new();
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c.clone()) || is_org_root(conn, &c)? {
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

/// 给「`crew` 不是 home、没有人类群」的拒绝指路用的 home。树规则（I-root、无环）下非 home 机组往上一定走得到
/// 一级机组；走不到（旧数据里与总机组断开的环）时 `home_of` 退回自己——指回自己没有意义，按内部数据不一致报。
pub(crate) fn home_pointer(conn: &Connection, crew: &str) -> Result<String> {
    let home = home_of(conn, crew)?;
    if home == crew {
        return Err(CollabError::Store(crewd_store::StoreError::Invalid(
            format!(
                "机组 {crew} 不是 home，往上也走不到一级机组（上级成环或与总机组断开）：数据不一致"
            ),
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

/// 根的轻量规则（N1 裁定 6）：总机组不能有父（不能被收编、转挂、摘出），不能删。被操作的机组不存在 = NotFound。
fn refuse_org_root(conn: &Connection, id: &str) -> Result<()> {
    if is_org_root(conn, id)? {
        return Err(CollabError::Forbidden(
            "总机组是组织的根，不能被收编、摘出或删除".into(),
        ));
    }
    Ok(())
}

/// 把 `crew` 挂到总机组下、排在末尾（必要时懒建总机组），返回总机组 id。调用方保证 `crew` 此刻没有父边。
/// 懒建出来的总机组连同它的大厅和别的建群一样发 ChannelChanged。
fn hang_under_org_root(tx: &mut Tx<'_>, crew: &str) -> Result<String> {
    let now = tx.now_ts();
    let existed = org_root(tx.conn())?.is_some();
    let root = crewd_store::crews::ensure_org_root(tx.conn(), &now)?;
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, root],
    )?;
    put_last(tx, crew, &root)?;
    tx.note(Change::CrewChanged {
        crew_id: root.clone(),
    });
    if !existed {
        tx.note(Change::ChannelChanged {
            channel_id: crewd_store::channels::lobby_id(&root),
        });
    }
    Ok(root)
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
    tx.note(Change::CrewChanged {
        crew_id: crew.id.clone(),
    });
    // 大厅是新建的群：和别的建群一样发 ChannelChanged。
    tx.note(Change::ChannelChanged {
        channel_id: crewd_store::channels::lobby_id(&crew.id),
    });
    match n.parent_id.as_deref() {
        // 有（非根的）父：排在父下的末尾（adopt 做）。
        Some(p) if !is_org_root(tx.conn(), p)? => {
            adopt(tx, &crew.id, p)?;
        }
        // 没给父 / 给的就是总机组：挂在总机组下的末尾（空库里第一个机组顺带建出总机组，N1 裁定 3）。
        _ => {
            hang_under_org_root(tx, &crew.id)?;
        }
    }
    get(tx.conn(), &crew.id)
}

pub fn update(tx: &mut Tx<'_>, id: &str, p: CrewPatch) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    // 根的轻量规则（N1 裁定 6）：总机组不能被隐藏（改名等其余字段与普通机组一样）。
    if crew.org_root && p.hidden == Some(true) {
        return Err(CollabError::Forbidden("总机组是组织的根，不能隐藏".into()));
    }
    let conn = tx.conn();
    let now = tx.now_ts();
    if let Some(t) = p.title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(CollabError::Invalid("机组名不能为空".into()));
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
/// 不变式 I-root（N1 裁定 4）：指向总机组的边只能是唯一的父——挂上非根的父时顺手删掉根边；
/// `parent` 是总机组：已是一级 → 空操作，否则 `Invalid`（挪到一级要用 release 摘掉其它父）。
pub fn adopt(tx: &mut Tx<'_>, crew: &str, parent: &str) -> Result<Crew> {
    refuse_org_root(tx.conn(), crew)?;
    get(tx.conn(), crew)?;
    get(tx.conn(), parent)?;
    if is_org_root(tx.conn(), parent)? {
        if parents(tx.conn(), crew)? == [parent] {
            return get(tx.conn(), crew);
        }
        return Err(CollabError::Invalid(format!(
            "挪到一级：用 release 摘掉 {crew} 的其它上级"
        )));
    }
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
    let was_home = is_home(tx.conn(), crew)?;
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, parent],
    )?;
    // 有了别的父：指向总机组的边去掉（总机组的 childIds 变了）。
    if let Some(root) = org_root(tx.conn())? {
        let dropped = tx.conn().execute(
            "DELETE FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
            params![crew, root],
        )?;
        if dropped > 0 {
            tx.note(Change::CrewChanged { crew_id: root });
        }
    }
    // 不再是 home（总机组 / 一级机组）：它的人类群归档（消息保留）。
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

/// 把 `from` 的直系子 `crew` 摘出；`to` 有值 = 先挂新边（过 adopt 的全部检查）再摘旧边。
/// 摘掉的是最后一条父边 = 回到总机组下的末尾；`to` = 总机组 等同 `None`（N1 裁定 4）。
pub fn release(tx: &mut Tx<'_>, crew: &str, from: &str, to: Option<&str>) -> Result<Crew> {
    refuse_org_root(tx.conn(), crew)?;
    let to_root = match to {
        Some(t) => is_org_root(tx.conn(), t)?,
        None => false,
    };
    let to = if to_root { None } else { to };
    get(tx.conn(), crew)?;
    // 不存在的父 = NotFound（控制者裁定 e），与「存在但不是直系父」（Invalid）分开。
    get(tx.conn(), from)?;
    let ps = parents(tx.conn(), crew)?;
    if !ps.iter().any(|p| p == from) {
        return Err(CollabError::Invalid(format!(
            "{crew} 不是 {from} 的直系子机组"
        )));
    }
    // 一级机组从总机组「摘出」、落回总机组下 = 原地不动（不挪到末尾、不发变更；与 adopt 到总机组对称）。
    if to == Some(from) || (to.is_none() && is_org_root(tx.conn(), from)?) {
        return get(tx.conn(), crew);
    }
    // 明说要回到一级，但摘掉这个父之后还挂在别的上级下：回不到一级，拒并指路（不悄悄留在原处）。
    if to_root && ps.len() > 1 {
        return Err(CollabError::Invalid(format!(
            "挪到一级：先解除 {crew} 的其它上级（release），只剩 {from} 时再摘到总机组下"
        )));
    }
    if let Some(t) = to {
        adopt(tx, crew, t)?;
    }
    tx.conn().execute(
        "DELETE FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
        params![crew, from],
    )?;
    if parents(tx.conn(), crew)?.is_empty() {
        hang_under_org_root(tx, crew)?;
    }
    // 又成了 home：原来就有的人类群解档（没有的仍然懒建）。
    if is_home(tx.conn(), crew)? && crate::channels::human_channel_exists(tx.conn(), crew)? {
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

/// 只能删非总机组、且没有 desired=running session 的机组（spec §22.2）。子机组的父边随之消失；
/// 只剩这一个父的子机组回到总机组下（N1 裁定 4）。
/// 删除级联（人显式的操作）：本机组的 session、群（含跨机组 topic 及其历史）一起删掉，每个群发
/// ChannelRemoved；它的 session 在别的机组群里的成员 / 群主身份先交出去（`release_sessions_of_doomed_crew`）。
pub fn delete(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    refuse_org_root(tx.conn(), id)?;
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
    // 只有这一个父的子机组删后没有父了：按原先的次序排到总机组下的末尾（C1）。
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
        hang_under_org_root(tx, k)?;
        // 又成了 home：原来就有的人类群解档（与 release 同一个做法；没有的仍然懒建）。
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
/// 大厅发 ChannelChanged（群主随机长算）；home（总机组 / 一级机组）且人类群已存在时，把新机长的游标钉到人类群末尾（不凭空多出一屏历史未读，裁定 9）。
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

/// 人手动排侧栏（C1，只有客户端 `crew.reorder` 调）：`ordered` 必须恰好是 `parent` 名下现有的全部直系子
/// （防客户端拿着旧列表覆盖）。只改 `parent` 名下各条边的次序（同一机组在别的父下的次序不动）；顶层只剩总机组
/// （N1 裁定 7），一级机组的次序就是总机组名下的次序。挪了位置的每个机组各发一条 `CrewChanged`，
/// 有挪动时 `parent` 也发一条（它的 `childIds` 变了）。
pub fn reorder(tx: &mut Tx<'_>, parent: &str, ordered: &[String]) -> Result<Vec<Crew>> {
    get(tx.conn(), parent)?;
    let current = children(tx.conn(), parent)?;
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
        let old: i64 = tx.conn().query_row(
            "SELECT position FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
            params![id, parent],
            |r| r.get(0),
        )?;
        if old == pos {
            continue;
        }
        tx.conn().execute(
            "UPDATE crew_parents SET position=?3 WHERE crew_id=?1 AND parent_id=?2",
            params![id, parent, pos],
        )?;
        tx.note(Change::CrewChanged {
            crew_id: id.clone(),
        });
        moved = true;
    }
    if moved {
        tx.note(Change::CrewChanged {
            crew_id: parent.into(),
        });
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
    fn home_is_the_first_level_ancestor_and_the_org_root_is_its_own_home() {
        let s = store();
        assert_eq!(home_of(s.conn(), "chief").unwrap(), "chief");
        assert_eq!(home_of(s.conn(), "crew-a").unwrap(), "crew-a");
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert!(is_home(s.conn(), "chief").unwrap());
        assert!(is_home(s.conn(), "crew-a").unwrap());
        assert!(!is_home(s.conn(), "crew-b").unwrap());
        assert!(
            is_org_root(s.conn(), "chief").unwrap() && !is_org_root(s.conn(), "crew-a").unwrap()
        );
        // 多父：二级机组同时挂在两个一级机组下 → created_at 最早的一级祖先（§13 Q1 同口径）。
        crewd_store::crews::set_parents(
            s.conn(),
            "crew-b",
            &["crew-x".to_string(), "crew-a".to_string()],
        )
        .unwrap();
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
    }

    #[test]
    fn crew_detail_names_the_home_not_the_org_root() {
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
    fn adopt_refuses_cycles_self_and_the_org_root_and_is_idempotent() {
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
        // 一级机组再挂到总机组下 = 空操作（N1 裁定 4）。
        ok(&mut s, |tx| adopt(tx, "crew-a", "chief"));
        assert_eq!(edges(&s, "crew-a"), ["chief"]);
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
        assert!(
            matches!(
                err(&mut s, |tx| release(tx, "crew-b", "chief", None)),
                CollabError::Invalid(_)
            ),
            "crew-b 不是总机组的直系子"
        );
        // 控制者裁定 e：不存在的父 = NotFound（-32004），不是「不是直系子机组」（-32602）。
        assert!(matches!(
            err(&mut s, |tx| release(tx, "crew-b", "nope", None)),
            CollabError::NotFound(_)
        ));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", None));
        assert_eq!(
            edges(&s, "crew-b"),
            ["chief"],
            "摘掉最后一个父 = 回到总机组下"
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

    fn order(s: &crewd_store::Store, parent: &str) -> Vec<String> {
        children(s.conn(), parent).unwrap()
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
        let (got, ch) = crate::testkit::ok_at(&mut s, NOW, |tx| reorder(tx, "crew-a", &want));
        assert_eq!(got.into_iter().map(|c| c.id).collect::<Vec<_>>(), want);
        assert_eq!(order(&s, "crew-a"), want);
        assert!(ch.iter().all(|c| matches!(c, Change::CrewChanged { .. })) && !ch.is_empty());
        let mut partial = want.clone();
        partial.pop();
        assert!(
            matches!(
                err(&mut s, |tx| reorder(tx, "crew-a", &partial)),
                CollabError::Invalid(_)
            ),
            "必须给出这一层的全部机组"
        );
        let mut dup = want.clone();
        dup[0] = dup[1].clone();
        assert!(matches!(
            err(&mut s, |tx| reorder(tx, "crew-a", &dup)),
            CollabError::Invalid(_)
        ));
        // 转挂到别处 = 新同级的末尾。
        ok(&mut s, |tx| release(tx, &a.id, "crew-a", Some("crew-x")));
        assert_eq!(order(&s, "crew-x").last(), Some(&a.id));
        // 顶层也能排。
        let mut roots = order(&s, "chief");
        roots.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, "chief", &roots));
        assert_eq!(order(&s, "chief"), roots);
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
        let ((), ch) =
            crate::testkit::ok_at(&mut s, NOW, |tx| reorder(tx, "crew-x", &want_x).map(|_| ()));
        assert_eq!(order(&s, "crew-x"), want_x);
        assert_eq!(order(&s, "crew-a"), under_a, "crew-a 下的顺序不受影响");
        assert!(
            ch.contains(&Change::CrewChanged {
                crew_id: "crew-x".into()
            }),
            "父的 childIds 变了"
        );
        let want_a = vec![k2.id.clone(), k1.id.clone(), "crew-b".to_string()];
        ok(&mut s, |tx| reorder(tx, "crew-a", &want_a));
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
    fn the_first_level_order_is_the_org_roots_children_and_leaves_out_the_root() {
        let mut s = store();
        let roots = order(&s, "chief");
        assert_eq!(roots, ["crew-a", "crew-x"]);
        let mut rotated = roots.clone();
        rotated.rotate_left(1);
        ok(&mut s, |tx| reorder(tx, "chief", &rotated));
        assert_eq!(order(&s, "chief"), rotated);
        let mut with_chief = roots.clone();
        with_chief.push("chief".into());
        assert!(
            matches!(
                err(&mut s, |tx| reorder(tx, "chief", &with_chief)),
                CollabError::Invalid(_)
            ),
            "总机组自己不在它名下的名单里"
        );
    }

    #[test]
    fn delete_puts_orphaned_children_last_under_the_org_root() {
        let mut s = store();
        let solo = kid(&mut s, "只有一个父", "crew-a");
        let shared = kid(&mut s, "另有父", "crew-a");
        ok(&mut s, |tx| adopt(tx, &shared.id, "crew-x")); // 另有父 crew-x，不会变根
        let roots_before = order(&s, "chief");
        ok(&mut s, |tx| delete(tx, "crew-a"));
        let roots = order(&s, "chief");
        let mut want: Vec<String> = roots_before.into_iter().filter(|c| c != "crew-a").collect();
        want.extend(["crew-b".to_string(), solo.id.clone()]);
        assert_eq!(
            roots, want,
            "没了父的（按原先的次序）排在总机组下的末尾，原有的一级机组不动"
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

    fn new_crew(title: &str) -> NewCrew {
        NewCrew {
            title: title.into(),
            working_directory: "/tmp/example".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_new_crew_without_a_parent_lands_last_under_the_org_root() {
        let mut s = store();
        let c = ok(&mut s, |tx| create(tx, new_crew("新机组")));
        assert_eq!(parents(s.conn(), &c.id).unwrap(), ["chief"]);
        assert_eq!(children(s.conn(), "chief").unwrap().last(), Some(&c.id));
        assert!(is_home(s.conn(), &c.id).unwrap());
    }

    #[test]
    fn the_first_crew_in_an_empty_database_creates_the_org_root() {
        let mut s = crewd_store::Store::open_in_memory().unwrap();
        let (c, changes) = ok_at(&mut s, NOW, |tx| create(tx, new_crew("第一个")));
        let root = org_root(s.conn()).unwrap().unwrap();
        assert_eq!(root, crewd_store::crews::ORG_ROOT_ID);
        assert_eq!(
            parents(s.conn(), &c.id).unwrap(),
            std::slice::from_ref(&root)
        );
        assert!(crate::channels::get(s.conn(), &crewd_store::channels::lobby_id(&root)).is_ok());
        // 顺带建出的群（总机组的大厅、新机组的大厅）和别的建群一样发 ChannelChanged。
        for lobby in [
            crewd_store::channels::lobby_id(&root),
            crewd_store::channels::lobby_id(&c.id),
        ] {
            assert!(
                changes.contains(&Change::ChannelChanged {
                    channel_id: lobby.clone()
                }),
                "{lobby}: {changes:?}"
            );
        }
        assert!(changes.contains(&Change::CrewChanged { crew_id: root }));
    }

    /// 总机组已在：只有新机组的大厅是新建的群，总机组的大厅不发。
    #[test]
    fn a_new_crews_lobby_is_announced_and_an_existing_root_lobby_is_not() {
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
    }

    #[test]
    fn the_org_root_cannot_be_adopted_released_or_deleted() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| adopt(tx, "chief", "crew-a")),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| release(tx, "chief", "crew-a", None)),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| delete(tx, "chief")),
            CollabError::Forbidden(_)
        ));
    }

    #[test]
    fn adopting_a_first_level_crew_drops_its_root_edge_and_releasing_it_brings_it_back() {
        let mut s = store();
        ok(&mut s, |tx| {
            crate::channels::ensure_human_channel(tx, "crew-x")
        });
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-a"));
        assert_eq!(
            parents(s.conn(), "crew-x").unwrap(),
            ["crew-a"],
            "指向根的边只能是唯一的父"
        );
        assert_eq!(home_of(s.conn(), "crew-x").unwrap(), "crew-a");
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some()
        );
        ok(&mut s, |tx| release(tx, "crew-x", "crew-a", None));
        assert_eq!(
            parents(s.conn(), "crew-x").unwrap(),
            ["chief"],
            "摘出 = 回到总机组下"
        );
        assert!(
            crate::channels::get(s.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_none()
        );
    }

    #[test]
    fn adopting_under_the_org_root_is_a_noop_for_first_level_crews_and_refused_otherwise() {
        let mut s = store();
        ok(&mut s, |tx| adopt(tx, "crew-a", "chief"));
        assert_eq!(parents(s.conn(), "crew-a").unwrap(), ["chief"]);
        assert!(matches!(
            err(&mut s, |tx| adopt(tx, "crew-b", "chief")),
            CollabError::Invalid(_)
        ));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("chief")));
        assert_eq!(
            parents(s.conn(), "crew-b").unwrap(),
            ["chief"],
            "release to=根 ≡ to=None"
        );
    }

    #[test]
    fn deleting_a_parent_moves_its_only_child_under_the_org_root() {
        let mut s = store();
        ok(&mut s, |tx| delete(tx, "crew-a"));
        assert_eq!(parents(s.conn(), "crew-b").unwrap(), ["chief"]);
        assert!(is_home(s.conn(), "crew-b").unwrap());
    }

    #[test]
    fn reorder_takes_a_parent_and_the_org_root_is_renamable() {
        let mut s = store();
        let got = ok(&mut s, |tx| {
            reorder(tx, "chief", &["crew-x".to_string(), "crew-a".to_string()])
        });
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["crew-x", "crew-a"]
        );
        assert_eq!(children(s.conn(), "chief").unwrap(), ["crew-x", "crew-a"]);
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

    /// 不变式：任何结构操作之后，没有父边的只有总机组（`is_home` 的「无父 = home」防御分支走不到）。
    #[test]
    fn after_any_structural_op_the_only_parentless_crew_is_the_org_root() {
        let mut s = store();
        let parentless = |s: &crewd_store::Store| {
            count(
                s,
                "SELECT COUNT(*) FROM crews c WHERE NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)",
            )
        };
        assert_eq!(parentless(&s), 1);
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        ok(&mut s, |tx| release(tx, "crew-x", "crew-b", None));
        ok(&mut s, |tx| delete(tx, "crew-a"));
        ok(&mut s, |tx| create(tx, new_crew("再建一个")));
        assert_eq!(parentless(&s), 1);
    }

    /// `crew.list` 的次序：总机组在前（不看 `crews.position` / 建组时间），然后按树的先序、每层按父的 childIds。
    #[test]
    fn crew_list_puts_the_org_root_first_then_the_tree_in_sidebar_order() {
        let mut s = store();
        // 根建得最晚、position 最大：照样排第一；新机组的 position 是 0，也不能排到根前面。
        s.conn()
            .execute(
                "UPDATE crews SET position=99, created_at='2099-01-01T00:00:00.000Z' WHERE id='chief'",
                [],
            )
            .unwrap();
        let n = ok(&mut s, |tx| create(tx, new_crew("新机组")));
        let ids = |s: &crewd_store::Store| -> Vec<String> {
            list(s.conn()).unwrap().into_iter().map(|c| c.id).collect()
        };
        assert_eq!(
            ids(&s),
            ["chief", "crew-a", "crew-b", "crew-x", n.id.as_str()]
        );
        ok(&mut s, |tx| {
            reorder(
                tx,
                "chief",
                &[n.id.clone(), "crew-x".into(), "crew-a".into()],
            )
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

    /// 从总机组「摘出」一级机组（`to` 缺省或就是总机组）= 原地不动：不能把它挪到一级的末尾
    /// （只有 reorder 动次序），也不发变更——与 `adopt(一级机组, 总机组)` 对称。
    #[test]
    fn releasing_a_first_level_crew_from_the_org_root_is_a_no_op() {
        let mut s = store();
        ok(&mut s, |tx| {
            reorder(tx, "chief", &["crew-a".into(), "crew-x".into()])
        });
        for to in [None, Some("chief")] {
            let (c, changes) = ok_at(&mut s, NOW, |tx| release(tx, "crew-a", "chief", to));
            assert_eq!(c.parent_ids, ["chief"], "{to:?}");
            assert!(changes.is_empty(), "{to:?}: {changes:?}");
            assert_eq!(
                children(s.conn(), "chief").unwrap(),
                ["crew-a", "crew-x"],
                "{to:?}"
            );
        }
    }

    /// 多父的机组「摘到总机组下」：摘掉一个父之后它还挂在别的上级下，回不到一级——拒并指路，不悄悄留在原处。
    #[test]
    fn releasing_to_the_org_root_with_other_parents_left_is_refused() {
        let mut s = store();
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        let e = err(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("chief")));
        assert!(
            matches!(&e, CollabError::Invalid(m) if m.contains("其它上级")),
            "{e:?}"
        );
        let mut ps = parents(s.conn(), "crew-b").unwrap();
        ps.sort();
        assert_eq!(ps, ["crew-a", "crew-x"], "什么都没动");
        // 只剩这一个父时摘到总机组下照常。
        ok(&mut s, |tx| release(tx, "crew-b", "crew-x", None));
        let c = ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("chief")));
        assert_eq!(c.parent_ids, ["chief"]);
    }

    /// 根的轻量规则（N1 裁定 6）：总机组不能被隐藏；取消隐藏 / 别的机组照常。
    #[test]
    fn the_org_root_cannot_be_hidden() {
        let mut s = store();
        let hide = |h: bool| CrewPatch {
            hidden: Some(h),
            ..Default::default()
        };
        assert!(matches!(
            err(&mut s, |tx| update(tx, "chief", hide(true))),
            CollabError::Forbidden(_)
        ));
        assert!(get(s.conn(), "chief").unwrap().hidden_at.is_none());
        ok(&mut s, |tx| update(tx, "chief", hide(false)));
        let x = ok(&mut s, |tx| update(tx, "crew-x", hide(true)));
        assert!(x.hidden_at.is_some());
    }

    /// `home_of` 不再先 `get`（那会把全部机组连同父子边读一遍）：不存在的机组仍是 NotFound（由 `is_org_root` 报）。
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

    /// 单根下一个一级机组被收编、再多挂一个父、逐个摘掉：home 身份、人类群的归档 / 解档、机长「在」的群、
    /// 提问的转达对象都跟着树走；全程没有「既挂总机组又挂别人」的形状（I-root）。
    #[test]
    fn moving_a_first_level_crew_down_and_back_keeps_home_human_channel_and_relay_in_step() {
        let mut s = store();
        let two_parent_root_edges = |s: &crewd_store::Store| {
            count(
                s,
                "SELECT COUNT(*) FROM crew_parents p JOIN crews r ON r.id=p.parent_id AND r.org_root=1
                  WHERE (SELECT COUNT(*) FROM crew_parents q WHERE q.crew_id=p.crew_id) > 1",
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
        assert!(in_human(&s), "一级机组的机长在自己的人类群里");

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

        // 再多挂一个（非根的）父、摘掉一个：仍不是 home，人类群保持归档。
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        assert_eq!(edges(&s, "crew-x"), ["crew-a", "crew-b"]);
        ok(&mut s, |tx| release(tx, "crew-x", "crew-a", None));
        assert_eq!(edges(&s, "crew-x"), ["crew-b"]);
        assert!(!is_home(s.conn(), "crew-x").unwrap() && archived(&s));
        assert_eq!(two_parent_root_edges(&s), 0);

        // 摘掉最后一个父：回到总机组下，重新是 home，人类群解档，机长又在里面。
        ok(&mut s, |tx| release(tx, "crew-x", "crew-b", None));
        assert_eq!(edges(&s, "crew-x"), ["chief"]);
        assert!(is_home(s.conn(), "crew-x").unwrap() && !archived(&s) && in_human(&s));

        // 非 home 的二级机组收编到别处：从来没有人类群，也不会凭空建一个。
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(edges(&s, "crew-b"), ["crew-a", "crew-x"]);
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM channels WHERE id='crew-b/human'"),
            0
        );
        assert_eq!(two_parent_root_edges(&s), 0);
    }
}
