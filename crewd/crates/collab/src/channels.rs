//! 群（spec §17、§22.2「群规则」、§22.7 Q2 群主）。成员判定、群主、人类群懒建都只在这里写一次。
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Locale, Result, Sender, Tx, crews, cursors, sessions};
use crewd_proto::{Channel, ChannelKind, ChannelSummary, MemberRef};
use crewd_store::channels::{human_id, lobby_id};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewChannel {
    pub crew_id: String,
    /// 只能是 Topic 或 Dm（大厅、人类群由 crewd 建）。
    pub kind: ChannelKind,
    pub name: String,
    pub purpose: Option<String>,
    /// 建群者自动入群，不用列。
    pub members: Vec<MemberRef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub archived: Option<bool>,
}

const COLS: &str = "id,crew_id,kind,name,purpose,created_by,owner_kind,owner_id,pending_owner_kind,pending_owner_id,created_at,archived_at";

struct Row {
    ch: Channel,
    owner: Option<MemberRef>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let kind: String = r.get(2)?;
    let member = |k: Option<String>, id: Option<String>| match (k, id) {
        (Some(k), Some(id)) => MemberRef::from_row(&k, &id),
        _ => None,
    };
    Ok(Row {
        owner: member(r.get(6)?, r.get(7)?),
        ch: Channel {
            id: r.get(0)?,
            crew_id: r.get(1)?,
            kind: ChannelKind::parse(&kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("channels.kind = {kind:?}").into(),
                )
            })?,
            name: r.get(3)?,
            purpose: r.get(4)?,
            created_by: r.get(5)?,
            owner: None,
            pending_owner: member(r.get(8)?, r.get(9)?),
            created_at: r.get(10)?,
            archived_at: r.get(11)?,
        },
    })
}

/// 取一个群。大厅 / 人类群的 `owner` 不是存的，而是**此刻机组的机长**（跟着机长走）。
///
/// 不变式「owner ∈ members」只对**活着的**群成立：人类群的成员 = home 机组的机长 + 发过言的 session；
/// 机组被收编后不再是 home，它已归档的人类群留作历史，`owner` 仍显示该机组此刻的机长（给人看的名字），但他不再是成员
/// （[`is_member`] 为假）、也不能往里发言（`messages::post` 拒绝）。别拿 owner 当成员 / 权限的依据。
pub fn get(conn: &Connection, id: &str) -> Result<Channel> {
    let r = conn
        .query_row(
            &format!("SELECT {COLS} FROM channels WHERE id=?1"),
            [id],
            row,
        )
        .optional()?
        .ok_or_else(|| {
            CollabError::NotFound(Refusal::new(Code::ChannelNotFound).arg("channel", id))
        })?;
    let mut ch = r.ch;
    ch.owner = match ch.kind {
        ChannelKind::Lobby | ChannelKind::Human => {
            crews::captain_of(conn, &ch.crew_id)?.map(MemberRef::session)
        }
        _ => r.owner,
    };
    Ok(ch)
}

pub fn owner_of(conn: &Connection, id: &str) -> Result<Option<MemberRef>> {
    Ok(get(conn, id)?.owner)
}

fn explicit(conn: &Connection, id: &str) -> Result<Vec<MemberRef>> {
    let mut st = conn.prepare(
        "SELECT member_kind, member_id FROM channel_members WHERE channel_id=?1 AND left_at IS NULL ORDER BY joined_at, member_id",
    )?;
    let raw = st
        .query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, i)| MemberRef::from_row(&k, &i))
        .collect())
}

/// 人类群的显式成员（同 `explicit` 的顺序），只留 home 仍是这个群的 session：人类群的权利从 `home_of` 推出，
/// 不认陈旧的行（机组被挪到别的一级机组下之后，它的 session 在旧 home 人类群里留下的行不再给它发言 / 注入 / 读的权利）。
/// 一次查询带出成员所在的机组，home 按不同的机组各算一次。session 行已不存在的成员行照旧保留（与 topic / dm 的显式行同一口径）。
fn homed_here(conn: &Connection, id: &str, home: &str) -> Result<Vec<MemberRef>> {
    let mut st = conn.prepare(
        "SELECT cm.member_kind, cm.member_id, s.crew_id FROM channel_members cm
           LEFT JOIN sessions s ON cm.member_kind='session' AND s.id=cm.member_id
          WHERE cm.channel_id=?1 AND cm.left_at IS NULL ORDER BY cm.joined_at, cm.member_id",
    )?;
    let raw = st
        .query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut homes: std::collections::BTreeMap<String, bool> = Default::default();
    let mut out = Vec::with_capacity(raw.len());
    for (k, i, crew) in raw {
        if let Some(crew) = crew {
            let here = match homes.get(&crew) {
                Some(h) => *h,
                None => {
                    let h = crews::home_of(conn, &crew)? == home;
                    homes.insert(crew, h);
                    h
                }
            };
            if !here {
                continue;
            }
        }
        out.extend(MemberRef::from_row(&k, &i));
    }
    Ok(out)
}

/// **有效**成员（冻结语义，C6）：大厅 = 人 + 本机组全部 session（隐式，不写 channel_members）；
/// 人类群 = 人 + home（总机组 / 一级机组）此刻的机长（隐式；机组已不是 home——被收编后归档的旧人类群——就没有）
/// 加上 home 是它的 session 的显式行（`homed_here`）；
/// topic / dm = 显式行。M3b 的注入面、M3c 的唤醒规划都只认它。
pub fn members(conn: &Connection, id: &str) -> Result<Vec<MemberRef>> {
    let ch = get(conn, id)?;
    let mut out = match ch.kind {
        ChannelKind::Lobby => {
            let mut v = vec![MemberRef::Human];
            v.extend(
                sessions::list(conn, Some(&ch.crew_id))?
                    .into_iter()
                    .map(|s| MemberRef::session(s.id)),
            );
            v
        }
        ChannelKind::Human => {
            let mut v = vec![MemberRef::Human];
            if crews::is_home(conn, &ch.crew_id)? {
                v.extend(crews::captain_of(conn, &ch.crew_id)?.map(MemberRef::session));
            }
            v.extend(homed_here(conn, id, &ch.crew_id)?);
            v
        }
        ChannelKind::Topic | ChannelKind::Dm => explicit(conn, id)?,
    };
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|m| seen.insert(m.clone()));
    Ok(out)
}

/// 群在列表 / 读取面上的显示名（M3b 的 read_channel / search 等用，**不含**注入头里的 LLM 文案）。
/// - dm：**对方**的标签（与发言者标签同规则：机长 / 分机号 / 人类，名字过 `safe_name`）；
///   `viewer` 不在 dm 里（旁观）时两边都列、用「, 」连。
/// - 其余：与注入头里的名字槽同值（大厅 / 人类群 = 机组标题，topic = 群名），都过 `safe_name`。
pub fn display_label(
    conn: &Connection,
    ch: &Channel,
    viewer: &MemberRef,
    l: Locale,
) -> Result<String> {
    use crate::render::{member_label, safe_name};
    match ch.kind {
        ChannelKind::Dm => {
            let all = members(conn, &ch.id)?;
            let others: Vec<&MemberRef> = all.iter().filter(|m| *m != viewer).collect();
            others
                .iter()
                .map(|m| member_label(conn, l, m))
                .collect::<Result<Vec<_>>>()
                .map(|v| v.join(", "))
        }
        ChannelKind::Lobby | ChannelKind::Human => {
            Ok(safe_name(&crews::get(conn, &ch.crew_id)?.title))
        }
        ChannelKind::Topic => Ok(safe_name(&ch.name)),
    }
}

pub fn is_member(conn: &Connection, id: &str, m: &MemberRef) -> Result<bool> {
    Ok(members(conn, id)?.contains(m))
}

pub fn is_watched(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM channel_watch WHERE channel_id=?1",
        [id],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// 读规则里「谁在读」的那一半：每个 session 只算一次（`home_of` 不按群重算，progress NOTE Task 6）。
struct Reader {
    crew: String,
    home: String,
    /// 总机组的 session（管理层）。
    manages_org: bool,
}

impl Reader {
    fn of(conn: &Connection, session: &str) -> Result<Self> {
        let crew = sessions::crew_of(conn, session)?;
        Ok(Self {
            manages_org: crews::is_org_root(conn, &crew)?,
            home: crews::home_of(conn, &crew)?,
            crew,
        })
    }

    fn can_read(&self, conn: &Connection, id: &str, crew: &str, kind: ChannelKind) -> Result<bool> {
        // ⓪ 总机组 = 管理层，读全组织（controller N1-R3，PM 2026-10-07）。
        if self.manages_org {
            return Ok(true);
        }
        // ③ 自己 home 的人类群；② 自己机组的人类群（被收编后归档的旧人类群仍归自己机组）。人类群不看成员行：
        // 它的有效成员也是从 home 推出的（`homed_here`），陈旧的行不给读权。
        if kind == ChannelKind::Human {
            return Ok(crew == self.home || crew == self.crew);
        }
        // ② 归属自己的机组（大厅的隐式成员 ① 也在这里）。
        if crew == self.crew {
            return Ok(true);
        }
        // ① 显式成员 + ② 群里有本机组的在群 session（自己也是本机组的人，所以 ① 包含在这一句里）。
        let crewmate_inside: i64 = conn.query_row(
            "SELECT COUNT(*) FROM channel_members cm JOIN sessions s ON cm.member_kind='session' AND cm.member_id=s.id
              WHERE cm.channel_id=?1 AND cm.left_at IS NULL AND s.crew_id=?2",
            params![id, self.crew],
            |r| r.get(0),
        )?;
        Ok(crewmate_inside > 0)
    }
}

/// B15（人 2026-10-07：没有私有群，dm 别人也读得到）+ N1 裁定 10 + controller N1-R3。读 = 主动拉取
/// （read_channel / search）；注入与唤醒只认 `members`。session S 读得到群 C ⇔
/// ⓪ S 在总机组（管理层，读全组织）；或 ① S 是 C 的有效成员；或 ② C 挂在 S 的机组下（归属 S 的机组，或 C 里有
/// S 机组的在群 session——`channel.list(S 的机组)` 列得出的群；**不含**子机组的群）；或 ③ C 是 S 的 home 的人类群。
/// 人类群只看 ③ 与「归属自己的机组」（从 `home_of` 推出，不认陈旧的成员行）。收听不在这里，调用方并上 `listens::channels_of`。
pub fn readable_by(conn: &Connection, channel: &str, session: &str) -> Result<bool> {
    let ch = get(conn, channel)?;
    Reader::of(conn, session)?.can_read(conn, &ch.id, &ch.crew_id, ch.kind)
}

/// 这个 session 有没有收听这个群的行（不看截止时间：collab 不读时钟；入队时收听有效就投，见 `render::build_batch`）。
pub(crate) fn has_listen_row(conn: &Connection, channel: &str, session: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM listens WHERE session_id=?1 AND channel_id=?2",
        params![session, channel],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// `readable_by` 为真的全部群（同一条规则；含已归档；按 `created_at, id`；不含收听）。
/// 候选是全表（M3 的量级可接受；量大再换成按机组 / 成员取候选）；读者的 home 只算一次。
pub fn readable_channels(conn: &Connection, session: &str) -> Result<Vec<String>> {
    let reader = Reader::of(conn, session)?;
    let mut st = conn.prepare("SELECT id, crew_id, kind FROM channels ORDER BY created_at, id")?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, crew, kind) in rows {
        let kind = ChannelKind::parse(&kind).ok_or_else(|| {
            CollabError::Store(crewd_store::StoreError::Invalid(format!(
                "群 {id} 的 kind = {kind:?}"
            )))
        })?;
        if reader.can_read(conn, &id, &crew, kind)? {
            out.push(id);
        }
    }
    Ok(out)
}

/// dm 里 `of` 的对方（M3b `post.to` / M3c dm 唤醒的缺省对象）。不是 dm、`of` 不在这个 dm 里、或对方已不在
/// （机组删除时交接后只剩一方）→ `None`。
pub fn dm_peer(conn: &Connection, channel: &str, of: &MemberRef) -> Result<Option<MemberRef>> {
    if get(conn, channel)?.kind != ChannelKind::Dm {
        return Ok(None);
    }
    let ms = explicit(conn, channel)?;
    if !ms.contains(of) {
        return Ok(None);
    }
    Ok(ms.into_iter().find(|m| m != of))
}

/// 群已归档（channel.archived；listens / messages / 本文件共用同一句）：与当前状态冲突（解档就行），
/// 一律 Conflict —— 往归档机组的大厅、人类群、topic 发言得到的是同一个答案。
pub(crate) fn archived(channel: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ChannelArchived).arg("channel", channel))
}

/// `crew` 不是一级机组、没有自己的人类群：指路到它的 home 的人类群（messages / 本文件共用同一句）。
pub(crate) fn no_human_channel(conn: &Connection, crew: &str) -> Result<CollabError> {
    Ok(CollabError::Invalid(
        Refusal::new(Code::ChannelNoHumanChannel)
            .arg("crew", crew)
            .arg("home", crews::home_pointer(conn, crew)?),
    ))
}

fn sender_ref(by: &Sender) -> Result<MemberRef> {
    by.member()
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::ChannelSystemCannotAct)))
}

fn check_member_exists(conn: &Connection, m: &MemberRef) -> Result<()> {
    if let MemberRef::Session { id } = m {
        sessions::get(conn, id)?;
    }
    Ok(())
}

/// 显式入群的唯一一处（R14）：写成员行；入群的 session 的游标钉到群的当前末尾（裁定 9）。
/// 已经在群里的人再被加一次什么都不动（不跳过他没读的）；退群后重新入群 = 新的入群点，重新钉。
/// 人类的游标**不动**：它的意思是「人看过了」，拉人进群不等于看过，钉了会抹掉没看的 @人 红标（spec §17）。
/// `create` / `add_members` 用它；Task 8 的「session 在 home 人类群发言即显式入群」也必须用它。
pub(crate) fn join(tx: &mut Tx<'_>, id: &str, m: &MemberRef) -> Result<()> {
    let (k, mid) = m.to_row();
    let joined = tx.conn().execute(
        "INSERT INTO channel_members(channel_id,member_kind,member_id,joined_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(channel_id,member_kind,member_id) DO UPDATE SET left_at=NULL, joined_at=excluded.joined_at
           WHERE channel_members.left_at IS NOT NULL",
        params![id, k, mid, tx.now_ts()],
    )? > 0;
    if joined && let MemberRef::Session { id: reader } = m {
        cursors::pin_to_end(tx, reader, id)?;
    }
    Ok(())
}

/// 私聊归属的机组（R10）：发起者（`all[0]`）是 session → 它的机组；人发起 → 对方 session 的机组。
fn dm_crew(conn: &Connection, all: &[MemberRef]) -> Result<String> {
    let anchor = match &all[0] {
        MemberRef::Human => &all[1],
        s => s,
    };
    match anchor {
        MemberRef::Session { id } => sessions::crew_of(conn, id),
        MemberRef::Human => Err(CollabError::Invalid(Refusal::new(Code::ChannelDmOneHuman))),
    }
}

pub fn create(tx: &mut Tx<'_>, by: &Sender, n: NewChannel) -> Result<Channel> {
    let creator = sender_ref(by)?;
    check_member_exists(tx.conn(), &creator)?;
    crews::get(tx.conn(), &n.crew_id)?;
    crews::refuse_archived(tx.conn(), &n.crew_id)?;
    if !matches!(n.kind, ChannelKind::Topic | ChannelKind::Dm) {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelCreateKindFixed,
        )));
    }
    let mut all = vec![creator.clone()];
    for m in n.members {
        check_member_exists(tx.conn(), &m)?;
        if !all.contains(&m) {
            all.push(m);
        }
    }
    let name = n.name.trim().to_string();
    if n.kind == ChannelKind::Topic && name.is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::ChannelNameEmpty)));
    }
    crate::prompts::refuse_sender_label(Code::ChannelNameIsSenderLabel, &name)?;
    if n.kind == ChannelKind::Dm {
        let humans = all.iter().filter(|m| **m == MemberRef::Human).count();
        if all.len() != 2 || humans > 1 {
            return Err(CollabError::Invalid(Refusal::new(Code::ChannelDmNeedsTwo)));
        }
        let owner_crew = dm_crew(tx.conn(), &all)?;
        if owner_crew != n.crew_id {
            return Err(CollabError::Invalid(
                Refusal::new(Code::ChannelDmWrongCrew)
                    .arg("owner", owner_crew)
                    .arg("crew", n.crew_id.clone()),
            ));
        }
    }
    let id = format!("ch-{}", uuid::Uuid::new_v4().simple());
    let (ok_kind, ok_id) = creator.to_row();
    let created_by = match by {
        Sender::Session(s) => s.clone(),
        _ => "human".to_string(),
    };
    tx.conn().execute(
        "INSERT INTO channels(id,crew_id,kind,name,purpose,created_by,owner_kind,owner_id,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, n.crew_id, n.kind.as_str(), name, n.purpose, created_by, ok_kind, ok_id, tx.now_ts()],
    )?;
    for m in &all {
        join(tx, &id, m)?;
    }
    tx.note(Change::ChannelChanged {
        channel_id: id.clone(),
    });
    get(tx.conn(), &id)
}

/// 两人之间的 dm：已有（未归档、恰好这两人）就复用，否则以 `a` 的身份新建在 `crew` 下
/// （新建时 `crew` 必须是发起者所在机组，见 `create`，R10）。返回 (群, 是否新建)。
/// `crew` 只管新建：复用时不论传哪个机组都返回已有的那个（终审裁定，推翻 Task 7 修复 #4），
/// 否则对方从自己机组发起私聊会被拒、永远打不开这条 dm。
pub fn ensure_dm(
    tx: &mut Tx<'_>,
    crew: &str,
    a: &MemberRef,
    b: &MemberRef,
) -> Result<(Channel, bool)> {
    let mut st = tx.conn().prepare(
        "SELECT id FROM channels WHERE kind='dm' AND archived_at IS NULL ORDER BY created_at",
    )?;
    let dms: Vec<String> = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in dms {
        let mut m = explicit(tx.conn(), &id)?;
        m.sort();
        let mut want = vec![a.clone(), b.clone()];
        want.sort();
        if m == want {
            return Ok((get(tx.conn(), &id)?, false));
        }
    }
    let by = match a {
        MemberRef::Session { id } => Sender::Session(id.clone()),
        MemberRef::Human => Sender::Human,
    };
    let ch = create(
        tx,
        &by,
        NewChannel {
            crew_id: crew.into(),
            kind: ChannelKind::Dm,
            name: String::new(),
            purpose: None,
            members: vec![b.clone()],
        },
    )?;
    Ok((ch, true))
}

/// 能管这个群的：人、归属机组的机长、群主。
fn may_manage(conn: &Connection, by: &Sender, ch: &Channel) -> Result<bool> {
    Ok(match by {
        Sender::Human => true,
        Sender::System => false,
        Sender::Session(s) => {
            crews::captain_of(conn, &ch.crew_id)?.as_deref() == Some(s.as_str())
                || ch.owner == Some(MemberRef::session(s.clone()))
        }
    })
}

pub fn update(tx: &mut Tx<'_>, by: &Sender, id: &str, p: ChannelPatch) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    // 归档机组的群跟着只读（含解档：先解档机组）。
    crews::refuse_archived(tx.conn(), &ch.crew_id)?;
    if !may_manage(tx.conn(), by, &ch)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelManageForbidden,
        )));
    }
    let fixed = matches!(ch.kind, ChannelKind::Lobby | ChannelKind::Human);
    if let Some(name) = p.name {
        if fixed {
            return Err(CollabError::Invalid(Refusal::new(Code::ChannelFixedName)));
        }
        let name = name.trim().to_string();
        if name.is_empty() && ch.kind == ChannelKind::Topic {
            return Err(CollabError::Invalid(Refusal::new(Code::ChannelNameEmpty)));
        }
        crate::prompts::refuse_sender_label(Code::ChannelNameIsSenderLabel, &name)?;
        tx.conn()
            .execute("UPDATE channels SET name=?2 WHERE id=?1", params![id, name])?;
    }
    if let Some(purpose) = p.purpose {
        let purpose = purpose.trim().to_string();
        tx.conn().execute(
            "UPDATE channels SET purpose=?2 WHERE id=?1",
            params![id, (!purpose.is_empty()).then_some(purpose)],
        )?;
    }
    if let Some(a) = p.archived {
        if fixed {
            return Err(CollabError::Invalid(Refusal::new(
                Code::ChannelFixedNoArchive,
            )));
        }
        let at = a.then(|| tx.now_ts());
        tx.conn().execute(
            "UPDATE channels SET archived_at=?2 WHERE id=?1",
            params![id, at],
        )?;
        if a {
            set_pending(tx, id, None)?;
        }
    }
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

pub fn add_members(tx: &mut Tx<'_>, by: &Sender, id: &str, ms: &[MemberRef]) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    match ch.kind {
        ChannelKind::Lobby => {
            return Err(CollabError::Invalid(Refusal::new(
                Code::ChannelLobbyCannotAdd,
            )));
        }
        ChannelKind::Dm => {
            return Err(CollabError::Invalid(Refusal::new(
                Code::ChannelDmCannotGrow,
            )));
        }
        _ => {}
    }
    if ch.archived_at.is_some() {
        return Err(archived(id));
    }
    let me = sender_ref(by)?;
    if me != MemberRef::Human && !is_member(tx.conn(), id, &me)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelAddMembersMemberOnly,
        )));
    }
    for m in ms {
        check_member_exists(tx.conn(), m)?;
        // 人类群只收 home 是它的 session（与 `post` 的发言入群同一条；拉进来也不算数的行不写，免得静默失效）。
        if ch.kind == ChannelKind::Human
            && let MemberRef::Session { id: sid } = m
        {
            let home = crews::home_of(tx.conn(), &sessions::crew_of(tx.conn(), sid)?)?;
            if home != ch.crew_id {
                return Err(CollabError::Invalid(
                    Refusal::new(Code::ChannelHumanChannelOtherHome)
                        .arg("session", sid.clone())
                        .arg("home", home)
                        .arg("channel", id),
                ));
            }
        }
        join(tx, id, m)?;
    }
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

pub fn remove_member(tx: &mut Tx<'_>, by: &Sender, id: &str, m: &MemberRef) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    // 归档机组的群跟着只读（同 `update`）：名单不动，先解档机组。
    crews::refuse_archived(tx.conn(), &ch.crew_id)?;
    if ch.kind == ChannelKind::Lobby {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelLobbyCannotRemove,
        )));
    }
    if ch.kind == ChannelKind::Dm {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelDmCannotLeave,
        )));
    }
    let me = sender_ref(by)?;
    if me != *m && !may_manage(tx.conn(), by, &ch)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelLeaveForbidden,
        )));
    }
    if ch.owner.as_ref() == Some(m) && ch.kind == ChannelKind::Topic {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelOwnerMustTransfer,
        )));
    }
    let (k, mid) = m.to_row();
    let n = tx.conn().execute(
        "UPDATE channel_members SET left_at=?4 WHERE channel_id=?1 AND member_kind=?2 AND member_id=?3 AND left_at IS NULL",
        params![id, k, mid, tx.now_ts()],
    )?;
    if n == 0 {
        return Err(CollabError::NotFound(Refusal::new(
            Code::ChannelNotExplicitMember,
        )));
    }
    // 待接受转让的对象退群了：那条转让作废（否则它会一直挂着、还能被陈旧地接受）。
    if ch.pending_owner.as_ref() == Some(m) {
        set_pending(tx, id, None)?;
    }
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

pub fn watch(tx: &mut Tx<'_>, id: &str, on: bool) -> Result<ChannelSummary> {
    get(tx.conn(), id)?;
    if on {
        tx.conn().execute(
            "INSERT OR IGNORE INTO channel_watch(channel_id,since) VALUES(?1,?2)",
            params![id, tx.now_ts()],
        )?;
    } else {
        tx.conn()
            .execute("DELETE FROM channel_watch WHERE channel_id=?1", [id])?;
    }
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    summary(tx.conn(), id)
}

/// 这个机组的人类群已经建过了吗（含已归档的）？「人类群存在与否」的唯一一处判定（R14）：
/// `crews::{point_captain_at, release}` 都用它决定「只对已有的人类群动手，没有的仍然懒建」。
pub(crate) fn human_channel_exists(conn: &Connection, home: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM channels WHERE id=?1",
        [human_id(home)],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// home（总机组 / 一级机组）的人类群（懒建，spec §22.2；R1：`messages::post` 往还没建的人类群发言时也调
/// 这一个函数，不另写第二份）。总机组的是管理用人类群，同一条规则（N1-R2）。
/// 已归档（曾被收编到二级、后又回到一级）→ 解档。新建或解档时，当前机长的游标钉到群末尾
/// （与 `crews::point_captain_at` 一致：机长不会因为群刚出现 / 刚解档就凭空多出一屏历史未读，裁定 9）。
pub fn ensure_human_channel(tx: &mut Tx<'_>, home: &str) -> Result<Channel> {
    crews::get(tx.conn(), home)?;
    // 归档的机组只读：不建、不解档它的人类群（发言、列群都走这里，统一在这一处挡）。
    crews::refuse_archived(tx.conn(), home)?;
    if !crews::is_home(tx.conn(), home)? {
        return Err(no_human_channel(tx.conn(), home)?);
    }
    let id = human_id(home);
    let created = tx.conn().execute(
        "INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,?2,'human','','system',?3)",
        params![id, home, tx.now_ts()],
    )? > 0;
    let unarchived = tx.conn().execute(
        "UPDATE channels SET archived_at=NULL WHERE id=?1 AND archived_at IS NOT NULL",
        [&id],
    )? > 0;
    if created || unarchived {
        if let Some(cap) = crews::captain_of(tx.conn(), home)? {
            cursors::pin_to_end(tx, &cap, &id)?;
        }
        tx.note(Change::ChannelChanged {
            channel_id: id.clone(),
        });
    }
    get(tx.conn(), &id)
}

/// home 机组被收编到二级：它的人类群归档（消息保留，spec §22.2）。
pub(crate) fn archive_human_of(tx: &mut Tx<'_>, crew: &str) -> Result<()> {
    let id = human_id(crew);
    let at = tx.now_ts();
    if tx.conn().execute(
        "UPDATE channels SET archived_at=?2 WHERE id=?1 AND archived_at IS NULL",
        params![id, at],
    )? > 0
    {
        tx.note(Change::ChannelChanged { channel_id: id });
    }
    Ok(())
}

/// 一个 session 「在」的群：自己机组的大厅 + （是 home 机长时）人类群 + 显式加入且未归档的群。
pub fn channels_of_session(conn: &Connection, session: &str) -> Result<Vec<Channel>> {
    let s = sessions::get(conn, session)?;
    let mut out = vec![get(conn, &lobby_id(&s.crew_id))?];
    if crews::is_home(conn, &s.crew_id)?
        && crews::captain_of(conn, &s.crew_id)?.as_deref() == Some(session)
    {
        // 人类群是懒建的：还没有 = NotFound = 跳过；别的错误照实往上抛。
        match get(conn, &human_id(&s.crew_id)) {
            Ok(h) if h.archived_at.is_none() => out.push(h),
            Ok(_) | Err(CollabError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    let mut st = conn.prepare(
        "SELECT cm.channel_id FROM channel_members cm JOIN channels ch ON ch.id=cm.channel_id
          WHERE cm.member_kind='session' AND cm.member_id=?1 AND cm.left_at IS NULL AND ch.archived_at IS NULL
          ORDER BY ch.created_at, ch.id",
    )?;
    let ids: Vec<String> = st
        .query_map([session], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut home: Option<String> = None;
    for id in ids {
        if out.iter().any(|c| c.id == id) {
            continue;
        }
        let ch = get(conn, &id)?;
        // 人类群的显式行只在 home 对得上时算数（同 `members` 的 `homed_here`）；home 只算一次。
        if ch.kind == ChannelKind::Human {
            if home.is_none() {
                home = Some(crews::home_of(conn, &s.crew_id)?);
            }
            if home.as_deref() != Some(ch.crew_id.as_str()) {
                continue;
            }
        }
        out.push(ch);
    }
    Ok(out)
}

pub fn summary(conn: &Connection, id: &str) -> Result<ChannelSummary> {
    let channel = get(conn, id)?;
    let b = cursors::badge(conn, id)?;
    Ok(ChannelSummary {
        members: members(conn, id)?,
        watched: is_watched(conn, id)?,
        unread_for_human: b.unread_for_human,
        human_mentions: b.human_mentions,
        channel,
    })
}

/// `channel.list(crewId)`：归属该机组的群 ∪ 有成员 session 属于该机组的群（§17「同时显示在每个成员所在机组下」）。
/// home（总机组 / 一级机组）会顺手懒建人类群（归档的机组除外：只读，列出已有的群、不建不解档）。顺序：本机组自己的大厅、自己的人类群，其余（含别的机组的人类群）按
/// 大厅 / 人类群 / 其他、建群时间。
pub fn list_for_crew(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<ChannelSummary>> {
    crews::get(tx.conn(), crew)?;
    if crews::is_home(tx.conn(), crew)? && !crews::is_archived(tx.conn(), crew)? {
        ensure_human_channel(tx, crew)?;
    }
    // 成员行带进来的别人的人类群只在它是本机组 home 的人类群时才挂（陈旧的跨 home 行不算，与读规则一致）。
    let home = crews::home_of(tx.conn(), crew)?;
    let mut st = tx.conn().prepare(
        "SELECT id FROM channels ch WHERE ch.crew_id=?1
            OR (ch.id IN (SELECT cm.channel_id FROM channel_members cm JOIN sessions s
                            ON cm.member_kind='session' AND cm.member_id=s.id
                           WHERE s.crew_id=?1 AND cm.left_at IS NULL)
                AND NOT (ch.kind='human' AND ch.crew_id<>?4))
          ORDER BY CASE WHEN ch.id=?2 THEN 0 WHEN ch.id=?3 THEN 1 ELSE 2 END,
                   CASE kind WHEN 'lobby' THEN 0 WHEN 'human' THEN 1 ELSE 2 END, created_at, id",
    )?;
    let ids: Vec<String> = st
        .query_map(params![crew, lobby_id(crew), human_id(crew), home], |r| {
            r.get(0)
        })?
        .collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|id| summary(tx.conn(), id)).collect()
}

/// 群主转让只发生在活着的 topic / dm 上：大厅 / 人类群的群主跟着机长走，归档的群不再动群主。
fn require_owner_channel(ch: &Channel) -> Result<()> {
    if ch.archived_at.is_some() {
        return Err(archived(&ch.id));
    }
    if matches!(ch.kind, ChannelKind::Lobby | ChannelKind::Human) {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelOwnerFollowsCaptain,
        )));
    }
    Ok(())
}

fn set_pending(tx: &mut Tx<'_>, id: &str, to: Option<(&MemberRef, &str)>) -> Result<()> {
    let (k, i, by) = match to {
        Some((m, by)) => {
            let (k, i) = m.to_row();
            (Some(k), Some(i.to_string()), Some(by.to_string()))
        }
        None => (None, None, None),
    };
    let at = to.map(|_| tx.now_ts());
    tx.conn().execute(
        "UPDATE channels SET pending_owner_kind=?2, pending_owner_id=?3, pending_owner_by=?4, pending_owner_at=?5 WHERE id=?1",
        params![id, k, i, by, at],
    )?;
    Ok(())
}

fn set_owner(tx: &mut Tx<'_>, id: &str, to: &MemberRef) -> Result<Channel> {
    let (k, i) = to.to_row();
    tx.conn().execute(
        "UPDATE channels SET owner_kind=?2, owner_id=?3 WHERE id=?1",
        params![id, k, i],
    )?;
    set_pending(tx, id, None)?;
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

/// 机组 `crew` 要被永久删掉（`crews::purge` 在级联之前调，终审 I4）：它的 session 会随级联消失，
/// 先把它们从**别的机组**的群里摘干净——显式成员行记 `left_at`；它们当群主的 topic 交给群所属机组
/// 此刻的机长（机长随之入群），没有机长（或是 dm）交给剩下的成员里最早入群的，一个不剩群主留空；
/// 转给它们的待接受转让作废。每个受影响、活下来的群发 ChannelChanged。
/// 本机组自己的群随级联消失，这里不动，返回它们的 id（调用方在 DELETE 之后发 ChannelRemoved）。
pub(crate) fn release_sessions_of_doomed_crew(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<String>> {
    const DOOMED: &str = "SELECT id FROM sessions WHERE crew_id=?1";
    let ids = |tx: &Tx<'_>, sql: &str| -> Result<Vec<String>> {
        let mut st = tx.conn().prepare(sql)?;
        let v = st
            .query_map([crew], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    };
    let left = ids(
        tx,
        &format!(
            "SELECT DISTINCT channel_id FROM channel_members
             WHERE member_kind='session' AND left_at IS NULL AND member_id IN ({DOOMED})
               AND channel_id IN (SELECT id FROM channels WHERE crew_id<>?1)
             ORDER BY channel_id"
        ),
    )?;
    let now = tx.now_ts();
    tx.conn().execute(
        &format!(
            "UPDATE channel_members SET left_at=?2
             WHERE member_kind='session' AND left_at IS NULL AND member_id IN ({DOOMED})
               AND channel_id IN (SELECT id FROM channels WHERE crew_id<>?1)"
        ),
        params![crew, now],
    )?;
    for id in left {
        tx.note(Change::ChannelChanged { channel_id: id });
    }
    let owned = ids(
        tx,
        &format!(
            "SELECT id FROM channels WHERE crew_id<>?1 AND owner_kind='session' AND owner_id IN ({DOOMED})
             ORDER BY id"
        ),
    )?;
    // 群主永远是成员（终审复审 A）：topic 交给群所属机组此刻的机长并让它入群（游标钉末尾）；没有机长、或是 dm
    // （恰好两人，不能加人）就交给剩下的成员里最早入群的；一个都不剩才留空（存 NULL）。
    for id in owned {
        let ch = get(tx.conn(), &id)?;
        let captain = match ch.kind {
            ChannelKind::Topic => {
                crews::captain_of(tx.conn(), &ch.crew_id)?.map(MemberRef::session)
            }
            _ => None,
        };
        let to = match captain {
            Some(c) => {
                join(tx, &id, &c)?;
                Some(c)
            }
            None => explicit(tx.conn(), &id)?.into_iter().next(),
        };
        match to {
            Some(m) => {
                set_owner(tx, &id, &m)?;
            }
            None => {
                tx.conn().execute(
                    "UPDATE channels SET owner_kind=NULL, owner_id=NULL WHERE id=?1",
                    [&id],
                )?;
                set_pending(tx, &id, None)?;
                tx.note(Change::ChannelChanged { channel_id: id });
            }
        }
    }
    let pending = ids(
        tx,
        &format!(
            "SELECT id FROM channels WHERE crew_id<>?1 AND pending_owner_kind='session'
               AND pending_owner_id IN ({DOOMED}) ORDER BY id"
        ),
    )?;
    for id in pending {
        set_pending(tx, &id, None)?;
        tx.note(Change::ChannelChanged { channel_id: id });
    }
    ids(tx, "SELECT id FROM channels WHERE crew_id=?1 ORDER BY id")
}

/// 群主发起转让（§22.7 Q2：同工作项负责人，双方确认）。
pub fn request_owner_transfer(
    tx: &mut Tx<'_>,
    by: &Sender,
    id: &str,
    to: &MemberRef,
) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    let me = sender_ref(by)?;
    if ch.owner.as_ref() != Some(&me) {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelTransferOwnerOnly,
        )));
    }
    if *to == me {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelAlreadyOwner,
        )));
    }
    if !is_member(tx.conn(), id, to)? {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelNewOwnerMustBeMember,
        )));
    }
    let by_s = me.to_row().1.to_string();
    set_pending(tx, id, Some((to, &by_s)))?;
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

pub fn accept_owner_transfer(tx: &mut Tx<'_>, by: &Sender, id: &str) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    let me = sender_ref(by)?;
    match &ch.pending_owner {
        Some(p) if *p == me => {
            if !is_member(tx.conn(), id, &me)? {
                return Err(CollabError::Invalid(Refusal::new(
                    Code::ChannelTransferTargetLeft,
                )));
            }
            set_owner(tx, id, &me)
        }
        Some(_) => Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelTransferNotForYou,
        ))),
        None => Err(CollabError::Invalid(Refusal::new(
            Code::ChannelNoPendingTransfer,
        ))),
    }
}

/// 撤回 / 拒绝：群主、被转让的人、机长、人。
pub fn withdraw_owner_transfer(tx: &mut Tx<'_>, by: &Sender, id: &str) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    if ch.pending_owner.is_none() {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelNoPendingTransfer,
        )));
    }
    let me = by.member();
    let allowed = may_manage(tx.conn(), by, &ch)? || me.as_ref() == ch.pending_owner.as_ref();
    if !allowed {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelTransferWithdrawForbidden,
        )));
    }
    set_pending(tx, id, None)?;
    tx.note(Change::ChannelChanged {
        channel_id: id.into(),
    });
    get(tx.conn(), id)
}

/// 机长（归属机组的）或人强制换群主。
pub fn force_owner(tx: &mut Tx<'_>, by: &Sender, id: &str, to: &MemberRef) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    let allowed = match by {
        Sender::Human => true,
        Sender::Session(s) => {
            crews::captain_of(tx.conn(), &ch.crew_id)?.as_deref() == Some(s.as_str())
        }
        Sender::System => false,
    };
    if !allowed {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ChannelForceOwnerForbidden,
        )));
    }
    if !is_member(tx.conn(), id, to)? {
        return Err(CollabError::Invalid(Refusal::new(
            Code::ChannelNewOwnerMustBeMember,
        )));
    }
    set_owner(tx, id, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, msg, ok, ok_at, store};
    use crewd_proto::SenderKind;

    fn s(id: &str) -> MemberRef {
        MemberRef::session(id)
    }
    fn topic(crew: &str, members: Vec<MemberRef>) -> NewChannel {
        NewChannel {
            crew_id: crew.into(),
            kind: ChannelKind::Topic,
            name: "发版".into(),
            purpose: Some("排期".into()),
            members,
        }
    }

    #[test]
    fn display_label_of_a_dm_is_the_other_partys_decorated_label() {
        use crate::prompts::{fill, render_file};
        let mut s_ = store();
        let (dm, _) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-a", &s("w-a1"), &s("w-a2"))
        });
        let (hdm, _) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-a", &MemberRef::Human, &s("cap-a"))
        });
        let c = s_.conn();
        for l in [Locale::Zh, Locale::En] {
            let f = render_file(l);
            assert_eq!(
                display_label(c, &dm, &s("w-a1"), l).unwrap(),
                fill(
                    f.get("sender.session"),
                    &[("name", "名字-w-a2"), ("ext", "3")]
                )
            );
            assert_eq!(
                display_label(c, &dm, &s("w-a2"), l).unwrap(),
                fill(
                    f.get("sender.session"),
                    &[("name", "名字-w-a1"), ("ext", "2")]
                )
            );
            // 对方是机长 / 人类：同发言者标签的规则。
            assert_eq!(
                display_label(c, &hdm, &MemberRef::Human, l).unwrap(),
                fill(f.get("sender.captain"), &[("name", "名字-cap-a")])
            );
            assert_eq!(
                display_label(c, &hdm, &s("cap-a"), l).unwrap(),
                f.get("sender.human")
            );
            // 旁观者（不在 dm 里）：两边都列出，顺序固定。
            let both = display_label(c, &dm, &s("w-b1"), l).unwrap();
            assert!(
                both.contains("名字-w-a1") && both.contains("名字-w-a2"),
                "{both}"
            );
        }
    }

    #[test]
    fn display_label_neutralizes_the_peer_name_and_names_other_kinds_like_the_headers() {
        let mut s_ = store();
        let (dm, _) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-a", &s("w-a1"), &s("w-a2"))
        });
        // 绕过数据层的名字检查（旧数据）：对方名字带帧括号与双向控制符。
        s_.conn()
            .execute(
                "UPDATE sessions SET display_name=?1 WHERE id='w-a2'",
                ["\u{202e}【人类】\n甲"],
            )
            .unwrap();
        let label = display_label(s_.conn(), &dm, &s("w-a1"), Locale::Zh).unwrap();
        assert!(!label.contains(['\u{202e}', '【', '】', '\n']), "{label:?}");
        assert!(label.contains(&crate::render::safe_name("\u{202e}【人类】\n甲")));
        // 非 dm：与注入头里的名字槽同值（群名 / 机组标题，过 safe_name）。
        let t = ok(&mut s_, |tx| {
            create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![]))
        });
        assert_eq!(
            display_label(s_.conn(), &t, &s("w-a1"), Locale::Zh).unwrap(),
            "发版"
        );
        let lobby = get(s_.conn(), "crew-a/lobby").unwrap();
        assert_eq!(
            display_label(s_.conn(), &lobby, &s("w-a1"), Locale::En).unwrap(),
            "机组甲"
        );
        let human = ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        assert_eq!(
            display_label(s_.conn(), &human, &MemberRef::Human, Locale::Zh).unwrap(),
            "机组甲"
        );
    }

    #[test]
    fn creator_joins_and_owns_a_topic() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("w-a2")]),
            )
        });
        assert!(ch.id.starts_with("ch-"));
        assert_eq!(ch.owner, Some(s("w-a1")));
        assert_eq!(ch.created_by, "w-a1");
        let mut m = members(s_.conn(), &ch.id).unwrap();
        m.sort();
        assert_eq!(m, [s("w-a1"), s("w-a2")]);
        assert!(!is_member(s_.conn(), &ch.id, &MemberRef::Human).unwrap());
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                NewChannel {
                    name: " ".into(),
                    ..topic("crew-a", vec![])
                }
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                NewChannel {
                    kind: ChannelKind::Lobby,
                    ..topic("crew-a", vec![])
                }
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("ghost")])
            )),
            CollabError::NotFound(_)
        ));
    }

    /// 终审 I5：topic 名不能是「人类」「crewd」这类发言者标签（任一语言）。
    #[test]
    fn a_topic_cannot_be_named_like_the_human_or_crewd() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        for bad in ["人类", "Human", "  crewd\n"] {
            assert!(
                matches!(
                    err(&mut s_, |tx| create(
                        tx,
                        &w1,
                        NewChannel {
                            name: bad.into(),
                            ..topic("crew-a", vec![])
                        }
                    )),
                    CollabError::Invalid(_)
                ),
                "{bad:?}"
            );
        }
        let ch = ok(&mut s_, |tx| create(tx, &w1, topic("crew-a", vec![])));
        assert!(matches!(
            err(&mut s_, |tx| update(
                tx,
                &w1,
                &ch.id,
                ChannelPatch {
                    name: Some("人类".into()),
                    ..Default::default()
                }
            )),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn dm_is_exactly_two_and_cannot_grow() {
        let mut s_ = store();
        let dm = NewChannel {
            kind: ChannelKind::Dm,
            name: String::new(),
            purpose: None,
            ..topic("crew-a", vec![s("w-b1")])
        };
        let ch = ok(&mut s_, |tx| {
            create(tx, &Sender::Session("w-a1".into()), dm.clone())
        });
        assert!(matches!(
            err(&mut s_, |tx| add_members(
                tx,
                &Sender::Session("w-a1".into()),
                &ch.id,
                &[s("w-a2")]
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                NewChannel {
                    members: vec![s("w-a2"), s("w-b1")],
                    ..dm.clone()
                }
            )),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                NewChannel {
                    members: vec![],
                    ..dm.clone()
                }
            )),
            CollabError::Invalid(_)
        ));
        let (again, created) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-a", &s("w-b1"), &s("w-a1"))
        });
        assert_eq!(
            (again.id.as_str(), created),
            (ch.id.as_str(), false),
            "同一对人复用同一个 dm"
        );
        let human_dm = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Human,
                NewChannel {
                    members: vec![s("w-a1")],
                    ..dm.clone()
                },
            )
        });
        assert_eq!(human_dm.owner, Some(MemberRef::Human));
    }

    #[test]
    fn lobby_and_human_channel_membership_is_implicit() {
        let mut s_ = store();
        let lobby = "crew-a/lobby";
        assert!(is_member(s_.conn(), lobby, &s("w-a1")).unwrap());
        assert!(is_member(s_.conn(), lobby, &MemberRef::Human).unwrap());
        assert!(
            !is_member(s_.conn(), lobby, &s("w-b1")).unwrap(),
            "子机组的人不在父机组大厅"
        );
        // `members` 返回**有效**成员（含隐式），M3c 的唤醒规划只在它里面选（C6，冻结）。
        let lm = members(s_.conn(), lobby).unwrap();
        assert!(
            lm.contains(&MemberRef::Human)
                && lm.contains(&s("cap-a"))
                && lm.contains(&s("w-a1"))
                && lm.contains(&s("w-a2")),
            "{lm:?}"
        );
        assert!(!lm.contains(&s("w-b1")));
        let h = ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        assert_eq!(
            members(s_.conn(), &h.id).unwrap(),
            [MemberRef::Human, s("cap-a")],
            "人类群：人 + home 机长（隐式）"
        );
        assert_eq!(h.id, "crew-a/human");
        assert_eq!(h.kind, ChannelKind::Human);
        assert!(
            is_member(s_.conn(), &h.id, &s("cap-a")).unwrap(),
            "home 机长隐式在人类群"
        );
        assert!(!is_member(s_.conn(), &h.id, &s("w-a1")).unwrap());
        assert!(
            matches!(
                err(&mut s_, |tx| ensure_human_channel(tx, "crew-b")),
                CollabError::Invalid(_)
            ),
            "只有 home（总机组 / 一级机组）有人类群"
        );
        assert_eq!(
            get(s_.conn(), lobby).unwrap().owner,
            Some(s("cap-a")),
            "大厅群主 = 机长"
        );
        ok(&mut s_, |tx| {
            crate::crews::set_captain(tx, "crew-a", "w-a1")
        });
        assert_eq!(
            get(s_.conn(), &h.id).unwrap().owner,
            Some(s("w-a1")),
            "跟着机长走"
        );
        assert!(is_member(s_.conn(), &h.id, &s("w-a1")).unwrap());
    }

    #[test]
    fn channel_list_shows_cross_crew_channels_under_every_members_crew() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("cap-x")]),
            )
        });
        let ids = |crew: &str, s_: &mut crewd_store::Store| -> Vec<String> {
            ok(s_, |tx| list_for_crew(tx, crew))
                .into_iter()
                .map(|c| c.channel.id)
                .collect()
        };
        assert_eq!(
            ids("crew-a", &mut s_),
            ["crew-a/lobby", "crew-a/human", ch.id.as_str()]
        );
        assert_eq!(
            ids("crew-x", &mut s_),
            ["crew-x/lobby", "crew-x/human", ch.id.as_str()],
            "跨机组成员所在的机组下也显示"
        );
        assert_eq!(
            ids("crew-b", &mut s_),
            ["crew-b/lobby"],
            "非 home 机组没有人类群"
        );
    }

    /// 归档机组的群只读：移出成员也拒（同 update / add_members 的守卫），成员名单不变；解档后照常。
    #[test]
    fn removing_a_member_from_an_archived_crews_channel_is_refused() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(tx, &Sender::Human, topic("crew-x", vec![s("cap-x")]))
        });
        ok(&mut s_, |tx| crate::crews::archive(tx, "crew-x"));
        let e = err(&mut s_, |tx| {
            remove_member(tx, &Sender::Human, &ch.id, &s("cap-x"))
        });
        match &e {
            CollabError::Conflict(r) => {
                assert_eq!(r.code, crate::refusal::Code::CrewArchived, "{e:?}")
            }
            e => panic!("{e:?}"),
        }
        assert!(
            is_member(s_.conn(), &ch.id, &s("cap-x")).unwrap(),
            "名单没变"
        );
        ok(&mut s_, |tx| crate::crews::unarchive(tx, "crew-x"));
        ok(&mut s_, |tx| {
            update(
                tx,
                &Sender::Human,
                &ch.id,
                ChannelPatch {
                    archived: Some(false),
                    ..Default::default()
                },
            )
        });
        ok(&mut s_, |tx| {
            remove_member(tx, &Sender::Human, &ch.id, &s("cap-x"))
        });
        assert!(!is_member(s_.conn(), &ch.id, &s("cap-x")).unwrap());
    }

    #[test]
    fn joining_a_topic_starts_at_the_end() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![]))
        });
        let seq = msg(&s_, &ch.id, "m1", SenderKind::Session, Some("w-a1"), vec![]);
        ok(&mut s_, |tx| {
            add_members(tx, &Sender::Session("w-a1".into()), &ch.id, &[s("w-a2")])
        });
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap(),
            Some(seq)
        );
        ok(&mut s_, |tx| {
            remove_member(tx, &Sender::Session("w-a2".into()), &ch.id, &s("w-a2"))
        });
        assert!(!is_member(s_.conn(), &ch.id, &s("w-a2")).unwrap());
        let seq2 = msg(&s_, &ch.id, "m2", SenderKind::Session, Some("w-a1"), vec![]);
        ok(&mut s_, |tx| {
            add_members(tx, &Sender::Session("w-a1".into()), &ch.id, &[s("w-a2")])
        });
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap(),
            Some(seq2),
            "重新入群也从末尾开始"
        );
    }

    #[test]
    fn only_owner_captain_or_human_may_archive_and_lobby_cannot_be_archived() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("w-a2")]),
            )
        });
        let arch = ChannelPatch {
            archived: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            err(&mut s_, |tx| update(
                tx,
                &Sender::Session("w-a2".into()),
                &ch.id,
                arch.clone()
            )),
            CollabError::Forbidden(_)
        ));
        ok(&mut s_, |tx| {
            update(tx, &Sender::Session("cap-a".into()), &ch.id, arch.clone())
        });
        assert!(get(s_.conn(), &ch.id).unwrap().archived_at.is_some());
        ok(&mut s_, |tx| {
            update(
                tx,
                &Sender::Human,
                &ch.id,
                ChannelPatch {
                    archived: Some(false),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(
            err(&mut s_, |tx| update(
                tx,
                &Sender::Human,
                "crew-a/lobby",
                arch.clone()
            )),
            CollabError::Invalid(_)
        ));
        assert!(
            matches!(
                err(&mut s_, |tx| remove_member(
                    tx,
                    &Sender::Session("w-a1".into()),
                    &ch.id,
                    &s("w-a1")
                )),
                CollabError::Invalid(_)
            ),
            "群主先转让再走"
        );
    }

    #[test]
    fn owner_transfer_needs_both_sides_or_force() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("w-a2"), s("cap-x")]),
            )
        });
        let w1 = Sender::Session("w-a1".into());
        let w2 = Sender::Session("w-a2".into());
        assert!(
            matches!(
                err(&mut s_, |tx| request_owner_transfer(
                    tx,
                    &w2,
                    &ch.id,
                    &s("w-a2")
                )),
                CollabError::Forbidden(_)
            ),
            "只有群主能发起"
        );
        assert!(
            matches!(
                err(&mut s_, |tx| request_owner_transfer(
                    tx,
                    &w1,
                    &ch.id,
                    &s("w-b1")
                )),
                CollabError::Invalid(_)
            ),
            "新群主必须是成员"
        );
        let c = ok(&mut s_, |tx| {
            request_owner_transfer(tx, &w1, &ch.id, &s("w-a2"))
        });
        assert_eq!(c.pending_owner, Some(s("w-a2")));
        assert!(matches!(
            err(&mut s_, |tx| accept_owner_transfer(
                tx,
                &Sender::Session("cap-x".into()),
                &ch.id
            )),
            CollabError::Forbidden(_)
        ));
        let c = ok(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id));
        assert_eq!((c.owner, c.pending_owner), (Some(s("w-a2")), None));
        assert!(
            matches!(
                err(&mut s_, |tx| force_owner(tx, &w1, &ch.id, &s("w-a1"))),
                CollabError::Forbidden(_)
            ),
            "worker 不能强制"
        );
        let c = ok(&mut s_, |tx| {
            force_owner(tx, &Sender::Session("cap-a".into()), &ch.id, &s("cap-x"))
        });
        assert_eq!(c.owner, Some(s("cap-x")), "归属机组的机长可强制");
        let c = ok(&mut s_, |tx| {
            force_owner(tx, &Sender::Human, &ch.id, &s("w-a1"))
        });
        assert_eq!(c.owner, Some(s("w-a1")));
        assert!(
            matches!(
                err(&mut s_, |tx| request_owner_transfer(
                    tx,
                    &Sender::Session("cap-a".into()),
                    "crew-a/lobby",
                    &s("w-a1")
                )),
                CollabError::Invalid(_)
            ),
            "大厅群主跟着机长走"
        );
    }

    #[test]
    fn adoption_archives_the_human_channel_and_release_brings_it_back() {
        let mut s_ = store();
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-x"));
        ok(&mut s_, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        assert!(
            get(s_.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_some()
        );
        ok(&mut s_, |tx| {
            crate::crews::release(tx, "crew-x", "crew-a", None)
        });
        let ids: Vec<String> = ok(&mut s_, |tx| list_for_crew(tx, "crew-x"))
            .into_iter()
            .map(|c| c.channel.id)
            .collect();
        assert!(ids.contains(&"crew-x/human".to_string()));
        assert!(
            get(s_.conn(), "crew-x/human")
                .unwrap()
                .archived_at
                .is_none(),
            "又成了 home → 解档"
        );
    }

    /// 不变式 owner ∈ members 只对活着的群成立：已归档（不再是 home）的人类群仍显示原机长为群主（历史），
    /// 但他不是成员、也发不了言。
    #[test]
    fn an_archived_human_channel_keeps_its_historical_owner_who_is_no_longer_a_member() {
        let mut s_ = store();
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-x"));
        assert_eq!(
            get(s_.conn(), "crew-x/human").unwrap().owner,
            Some(s("cap-x"))
        );
        assert!(is_member(s_.conn(), "crew-x/human", &s_ref("cap-x")).unwrap());
        ok(&mut s_, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        let h = get(s_.conn(), "crew-x/human").unwrap();
        assert!(h.archived_at.is_some());
        assert_eq!(h.owner, Some(s("cap-x")), "归档的群留着原群主（展示用）");
        assert!(
            !is_member(s_.conn(), "crew-x/human", &s_ref("cap-x")).unwrap(),
            "owner 不再是成员：不变式只管活着的群"
        );
        assert!(matches!(
            err(&mut s_, |tx| crate::messages::post(
                tx,
                Sender::Session("cap-x".into()),
                crate::messages::NewPost {
                    channel_id: "crew-x/human".into(),
                    text: "示例消息".into(),
                    ..Default::default()
                },
            )),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn summary_carries_watch_and_badges() {
        let mut s_ = store();
        msg(
            &s_,
            "crew-a/lobby",
            "m1",
            SenderKind::Session,
            Some("w-a1"),
            vec![crewd_proto::Mention {
                kind: crewd_proto::MentionKind::Human,
                target_id: None,
            }],
        );
        msg(&s_, "crew-a/lobby", "m2", SenderKind::Human, None, vec![]);
        let (sum, ch) = ok_at(&mut s_, NOW, |tx| watch(tx, "crew-a/lobby", true));
        assert!(sum.watched);
        assert_eq!(
            (sum.unread_for_human, sum.human_mentions),
            (1, 1),
            "人自己发的不算人的未读"
        );
        assert_eq!(
            ch,
            [Change::ChannelChanged {
                channel_id: "crew-a/lobby".into()
            }]
        );
        assert_eq!(count(&s_, "SELECT COUNT(*) FROM channel_watch"), 1);
        ok(&mut s_, |tx| watch(tx, "crew-a/lobby", false));
        assert!(!is_watched(s_.conn(), "crew-a/lobby").unwrap());
    }

    fn dm_to(crew: &str, other: MemberRef) -> NewChannel {
        NewChannel {
            crew_id: crew.into(),
            kind: ChannelKind::Dm,
            name: String::new(),
            purpose: None,
            members: vec![other],
        }
    }

    #[test]
    fn a_dm_belongs_to_the_initiators_crew_and_a_disagreeing_crew_is_invalid() {
        let mut s_ = store();
        let w_a1 = Sender::Session("w-a1".into());
        // session 发起：归发起者的机组（即使对方在别的机组）。
        let ch = ok(&mut s_, |tx| create(tx, &w_a1, dm_to("crew-a", s("w-b1"))));
        assert_eq!(ch.crew_id, "crew-a");
        // 调用方给的机组 id 与发起者的机组不一致 → Invalid。
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &w_a1, dm_to("crew-b", s("w-b1")))),
            CollabError::Invalid(_)
        ));
        // 人发起：归对方 session 的机组。
        let h = ok(&mut s_, |tx| {
            create(tx, &Sender::Human, dm_to("crew-b", s("w-b1")))
        });
        assert_eq!(h.crew_id, "crew-b");
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Human,
                dm_to("crew-a", s("cap-b"))
            )),
            CollabError::Invalid(_)
        ));
        // session → 人：仍归 session 的机组。
        let to_human = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a2".into()),
                dm_to("crew-a", MemberRef::Human),
            )
        });
        assert_eq!(to_human.crew_id, "crew-a");
        // ensure_dm 新建时同样的规则；复用已有的不看 crew 参数。
        assert!(matches!(
            err(&mut s_, |tx| ensure_dm(
                tx,
                "crew-a",
                &s("cap-b"),
                &s("w-a1")
            )),
            CollabError::Invalid(_)
        ));
        let (c, created) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-b", &s("cap-b"), &s("w-a1"))
        });
        assert!(created);
        assert_eq!(c.crew_id, "crew-b");
    }

    #[test]
    fn adding_an_existing_member_again_does_not_skip_their_unread() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let ch = ok(&mut s_, |tx| {
            create(tx, &w1, topic("crew-a", vec![s("w-a2")]))
        });
        let at_join = crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap();
        msg(&s_, &ch.id, "m1", SenderKind::Session, Some("w-a1"), vec![]);
        ok(&mut s_, |tx| add_members(tx, &w1, &ch.id, &[s("w-a2")]));
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap(),
            at_join,
            "已在群里的人再被加一次，游标不动"
        );
    }

    #[test]
    fn lazy_human_channel_pins_the_current_captain_and_unarchiving_repins() {
        let mut s_ = store();
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "cap-x", "crew-x/human").unwrap(),
            None
        );
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-x"));
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "cap-x", "crew-x/human").unwrap(),
            Some(0),
            "懒建时当前机长的游标就位"
        );
        // 再调一次什么都不动（不改游标、不发变更）。
        let seq = msg(&s_, "crew-x/human", "m1", SenderKind::Human, None, vec![]);
        let (_, changes) = ok_at(&mut s_, NOW, |tx| ensure_human_channel(tx, "crew-x"));
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "cap-x", "crew-x/human").unwrap(),
            Some(0)
        );
        // 被收编（归档）后又独立（解档）：机长游标钉到此刻的末尾，不倒出归档期间的历史。
        ok(&mut s_, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        ok(&mut s_, |tx| {
            crate::crews::release(tx, "crew-x", "crew-a", None)
        });
        assert_eq!(
            crate::cursors::cursor(s_.conn(), "cap-x", "crew-x/human").unwrap(),
            Some(seq)
        );
    }

    #[test]
    fn withdrawing_a_pending_transfer() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let w2 = Sender::Session("w-a2".into());
        let ch = ok(&mut s_, |tx| {
            create(tx, &w1, topic("crew-a", vec![s("w-a2"), s("cap-x")]))
        });
        assert!(matches!(
            err(&mut s_, |tx| withdraw_owner_transfer(tx, &w1, &ch.id)),
            CollabError::Invalid(_)
        ));
        ok(&mut s_, |tx| {
            request_owner_transfer(tx, &w1, &ch.id, &s("w-a2"))
        });
        assert!(matches!(
            err(&mut s_, |tx| withdraw_owner_transfer(
                tx,
                &Sender::Session("cap-x".into()),
                &ch.id
            )),
            CollabError::Forbidden(_)
        ));
        let c = ok(&mut s_, |tx| withdraw_owner_transfer(tx, &w2, &ch.id));
        assert_eq!((c.owner, c.pending_owner), (Some(s("w-a1")), None));
        ok(&mut s_, |tx| {
            request_owner_transfer(tx, &w1, &ch.id, &s("w-a2"))
        });
        let c = ok(&mut s_, |tx| withdraw_owner_transfer(tx, &w1, &ch.id));
        assert_eq!(c.pending_owner, None);
    }

    #[test]
    fn channels_of_session_lists_lobby_human_and_live_explicit_channels() {
        let mut s_ = store();
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        let ids = |s_: &crewd_store::Store, who: &str| -> Vec<String> {
            channels_of_session(s_.conn(), who)
                .unwrap()
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(ids(&s_, "cap-a"), ["crew-a/lobby", "crew-a/human"]);
        assert_eq!(ids(&s_, "w-a1"), ["crew-a/lobby"], "worker 不在人类群");
        let ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                topic("crew-a", vec![s("cap-b")]),
            )
        });
        assert_eq!(ids(&s_, "w-a1"), ["crew-a/lobby", ch.id.as_str()]);
        assert_eq!(ids(&s_, "cap-b"), ["crew-b/lobby", ch.id.as_str()]);
        ok(&mut s_, |tx| {
            update(
                tx,
                &Sender::Human,
                &ch.id,
                ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        assert_eq!(ids(&s_, "w-a1"), ["crew-a/lobby"], "归档的群不算");
    }

    #[test]
    fn a_stale_owner_transfer_cannot_be_accepted_after_the_target_left() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let w2 = Sender::Session("w-a2".into());
        let ch = ok(&mut s_, |tx| {
            create(tx, &w1, topic("crew-a", vec![s("w-a2")]))
        });
        ok(&mut s_, |tx| {
            request_owner_transfer(tx, &w1, &ch.id, &s("w-a2"))
        });
        ok(&mut s_, |tx| remove_member(tx, &w2, &ch.id, &s("w-a2")));
        assert_eq!(
            get(s_.conn(), &ch.id).unwrap().pending_owner,
            None,
            "待转让对象退群 → 待办清掉"
        );
        assert!(matches!(
            err(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id)),
            CollabError::Invalid(_)
        ));
        // 清掉之前已经写进去的陈旧待办（直接造数据）：接受时也要重查成员关系。
        s_.conn()
            .execute(
                "UPDATE channels SET pending_owner_kind='session', pending_owner_id='w-a2' WHERE id=?1",
                [&ch.id],
            )
            .unwrap();
        assert!(matches!(
            err(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id)),
            CollabError::Invalid(_)
        ));
        assert_eq!(get(s_.conn(), &ch.id).unwrap().owner, Some(s("w-a1")));
    }

    #[test]
    fn adding_the_human_does_not_move_the_human_cursor() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let ch = ok(&mut s_, |tx| create(tx, &w1, topic("crew-a", vec![])));
        msg(
            &s_,
            &ch.id,
            "m1",
            SenderKind::Session,
            Some("w-a1"),
            vec![crewd_proto::Mention {
                kind: crewd_proto::MentionKind::Human,
                target_id: None,
            }],
        );
        ok(&mut s_, |tx| {
            add_members(tx, &w1, &ch.id, &[MemberRef::Human])
        });
        let b = crate::cursors::badge(s_.conn(), &ch.id).unwrap();
        assert_eq!(
            (b.unread_for_human, b.human_mentions),
            (1, 1),
            "拉人进群不等于人看过了"
        );
        assert_eq!(
            crate::cursors::cursor(s_.conn(), crate::cursors::HUMAN, &ch.id).unwrap(),
            None
        );
    }

    #[test]
    fn a_dm_cannot_shrink() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let dm = ok(&mut s_, |tx| create(tx, &w1, dm_to("crew-a", s("w-a2"))));
        for who in [&w1, &Sender::Session("w-a2".into()), &Sender::Human] {
            assert!(matches!(
                err(&mut s_, |tx| remove_member(tx, who, &dm.id, &s("w-a2"))),
                CollabError::Invalid(_)
            ));
        }
        assert_eq!(members(s_.conn(), &dm.id).unwrap().len(), 2);
    }

    #[test]
    fn dm_reuse_returns_the_existing_dm_whatever_crew_is_passed() {
        // 终审裁定（推翻 Task 7 修复 #4）：crew 只管新建（R10）；复用不再核对。
        let mut s_ = store();
        let dm = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                dm_to("crew-a", s("w-b1")),
            )
        });
        // 对方从自己的机组发起：拿到同一个 dm，归属不变。
        let (again, created) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-b", &s("w-b1"), &s("w-a1"))
        });
        assert_eq!((again.id.as_str(), created), (dm.id.as_str(), false));
        assert_eq!(again.crew_id, "crew-a");
        let (same, created) = ok(&mut s_, |tx| {
            ensure_dm(tx, "crew-a", &s("w-a1"), &s("w-b1"))
        });
        assert_eq!((same.id, created), (dm.id, false));
    }

    #[test]
    fn owner_transfer_ops_reject_archived_channels_and_archiving_clears_a_pending_one() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let ch = ok(&mut s_, |tx| {
            create(tx, &w1, topic("crew-a", vec![s("w-a2")]))
        });
        ok(&mut s_, |tx| {
            request_owner_transfer(tx, &w1, &ch.id, &s("w-a2"))
        });
        let c = ok(&mut s_, |tx| {
            update(
                tx,
                &Sender::Human,
                &ch.id,
                ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        assert_eq!(c.pending_owner, None, "归档清掉待转让");
        let w2 = Sender::Session("w-a2".into());
        assert!(matches!(
            err(&mut s_, |tx| request_owner_transfer(
                tx,
                &w1,
                &ch.id,
                &s("w-a2")
            )),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id)),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| withdraw_owner_transfer(tx, &w1, &ch.id)),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| force_owner(
                tx,
                &Sender::Human,
                &ch.id,
                &s("w-a2")
            )),
            CollabError::Conflict(_)
        ));
        // 带着陈旧待办的已归档群（直接造数据）：accept / withdraw 也拒。
        s_.conn()
            .execute(
                "UPDATE channels SET pending_owner_kind='session', pending_owner_id='w-a2' WHERE id=?1",
                [&ch.id],
            )
            .unwrap();
        assert!(matches!(
            err(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id)),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| withdraw_owner_transfer(tx, &w1, &ch.id)),
            CollabError::Conflict(_)
        ));
    }

    #[test]
    fn creating_as_a_nonexistent_session_is_not_found_but_cross_crew_topics_are_fine() {
        let mut s_ = store();
        assert!(matches!(
            err(&mut s_, |tx| create(
                tx,
                &Sender::Session("ghost".into()),
                topic("crew-a", vec![])
            )),
            CollabError::NotFound(_)
        ));
        let ch = ok(&mut s_, |tx| {
            create(tx, &Sender::Session("w-b1".into()), topic("crew-a", vec![]))
        });
        assert_eq!(ch.crew_id, "crew-a");
    }

    #[test]
    fn the_org_root_is_a_home_and_keeps_its_management_human_channel() {
        let mut s_ = store();
        let h = ok(&mut s_, |tx| ensure_human_channel(tx, "chief"));
        assert_eq!(h.id, "chief/human");
        let ids: Vec<String> = ok(&mut s_, |tx| list_for_crew(tx, "chief"))
            .into_iter()
            .map(|c| c.channel.id)
            .collect();
        assert_eq!(ids, ["chief/lobby", "chief/human"]);
    }

    #[test]
    fn list_puts_the_crews_own_lobby_then_own_human_channel_first() {
        let mut s_ = store();
        // crew-a 的人类群里有 crew-x 的机长发过言（显式成员）→ crew-x 的列表里会多一个别人的人类群，
        // 而且它的 id 排在 crew-x 自己的人类群前面。
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        // 陈旧的跨 home 成员行（cap-x 的 home 是 crew-x）：crew-a/human 不挂到 crew-x 下（与读规则一致）。
        ok(&mut s_, |tx| join(tx, "crew-a/human", &s("cap-x")));
        let topic_ch = ok(&mut s_, |tx| {
            create(
                tx,
                &Sender::Session("cap-x".into()),
                topic("crew-x", vec![]),
            )
        });
        let ids = |s_: &mut crewd_store::Store, crew: &str| -> Vec<String> {
            ok(s_, |tx| list_for_crew(tx, crew))
                .into_iter()
                .map(|c| c.channel.id)
                .collect()
        };
        assert_eq!(
            ids(&mut s_, "crew-x"),
            ["crew-x/lobby", "crew-x/human", topic_ch.id.as_str()]
        );
        assert!(!readable_by(s_.conn(), "crew-a/human", "cap-x").unwrap());
        // home 对得上的成员行照常挂：w-b1（home = crew-a）在 crew-a/human 里 → crew-b 下列得出。
        ok(&mut s_, |tx| join(tx, "crew-a/human", &s("w-b1")));
        assert_eq!(ids(&mut s_, "crew-b"), ["crew-b/lobby", "crew-a/human"]);
    }

    /// I2：一级机组被收编之后，它自己的人类群（已归档）仍归它的机组，读得到（②）；机长不再隐式在群里
    /// （不再是 home），成员与读规则一致。
    #[test]
    fn an_adopted_crews_own_human_channel_stays_readable_but_has_no_implicit_captain() {
        let mut s = store();
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-x"));
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        assert!(is_member(s.conn(), "crew-x/human", &s_ref("cap-x")).unwrap());
        ok(&mut s, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        assert!(get(s.conn(), "crew-x/human").unwrap().archived_at.is_some());
        assert!(
            readable_by(s.conn(), "crew-x/human", "cap-x").unwrap(),
            "自己机组的群"
        );
        assert!(
            !is_member(s.conn(), "crew-x/human", &s_ref("cap-x")).unwrap(),
            "不是 home 了：没有隐式机长"
        );
        assert!(
            readable_by(s.conn(), "crew-a/human", "cap-x").unwrap(),
            "新 home 的人类群"
        );
        assert!(!readable_by(s.conn(), "crew-x/human", "w-a1").unwrap());
    }

    /// M5：跨机组的 topic（不是 dm）经「群里有本机组的在群 session」读得到；那个同事退群后就读不到了。
    #[test]
    fn a_cross_crew_topic_is_readable_through_a_crewmate_until_they_leave() {
        let mut s = store();
        let t = ok(&mut s, |tx| {
            create(
                tx,
                &Sender::Session("cap-x".into()),
                topic("crew-x", vec![s_ref("w-a1")]),
            )
        });
        assert!(readable_by(s.conn(), &t.id, "w-a2").unwrap());
        assert!(readable_channels(s.conn(), "w-a2").unwrap().contains(&t.id));
        ok(&mut s, |tx| {
            remove_member(tx, &Sender::Session("w-a1".into()), &t.id, &s_ref("w-a1"))
        });
        assert!(
            !readable_by(s.conn(), &t.id, "w-a2").unwrap(),
            "退群的同事不再给读权"
        );
        assert!(
            !readable_by(s.conn(), &t.id, "w-a1").unwrap(),
            "自己退了也读不到"
        );
        assert!(readable_by(s.conn(), &t.id, "cap-x").unwrap());
    }

    #[test]
    fn channels_of_session_propagates_non_notfound_errors_from_the_human_channel() {
        let mut s_ = store();
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        s_.conn()
            .execute_batch("PRAGMA ignore_check_constraints=1; UPDATE channels SET kind='bogus' WHERE id='crew-a/human'; PRAGMA ignore_check_constraints=0;")
            .unwrap();
        assert!(channels_of_session(s_.conn(), "cap-a").is_err());
        // 没建人类群（NotFound）仍然只是跳过。
        let fresh = store();
        assert_eq!(channels_of_session(fresh.conn(), "cap-a").unwrap().len(), 1);
    }

    #[test]
    fn forbidden_paths_and_archived_channels() {
        let mut s_ = store();
        let w1 = Sender::Session("w-a1".into());
        let ch = ok(&mut s_, |tx| {
            create(tx, &w1, topic("crew-a", vec![s("w-a2"), s("cap-x")]))
        });
        // 非管理者（既非群主、机长、人）移走别人。
        assert!(matches!(
            err(&mut s_, |tx| remove_member(
                tx,
                &Sender::Session("w-a2".into()),
                &ch.id,
                &s("cap-x")
            )),
            CollabError::Forbidden(_)
        ));
        // 非成员拉人。
        assert!(matches!(
            err(&mut s_, |tx| add_members(
                tx,
                &Sender::Session("w-b1".into()),
                &ch.id,
                &[s("w-b1")]
            )),
            CollabError::Forbidden(_)
        ));
        // 系统不能以成员身份做事。
        assert!(matches!(
            err(&mut s_, |tx| add_members(
                tx,
                &Sender::System,
                &ch.id,
                &[s("w-b1")]
            )),
            CollabError::Invalid(_)
        ));
        // 已归档的群不能再拉人。
        ok(&mut s_, |tx| {
            update(
                tx,
                &Sender::Human,
                &ch.id,
                ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(
            err(&mut s_, |tx| add_members(tx, &w1, &ch.id, &[s("w-b1")])),
            CollabError::Conflict(_)
        ));
    }

    #[test]
    fn crew_detail_collects_home_children_channels_and_sessions() {
        let mut s_ = store();
        let d = ok(&mut s_, |tx| crate::crews::detail(tx, "crew-b"));
        assert_eq!(d.home_id, "crew-a");
        assert_eq!(d.sessions.len(), 2);
        assert_eq!(d.channels.len(), 1);
        let d = ok(&mut s_, |tx| crate::crews::detail(tx, "crew-a"));
        assert_eq!(d.children, ["crew-b"]);
    }

    /// 树规则下一个非 home 机组往上一定走得到一级机组；走不到（旧数据里与总机组断开的环）时，
    /// `home_of` 退回自己——拒绝不能指回自己（「crew-b 没有人类群，人类群在 crew-b/human」），按内部数据不一致报。
    #[test]
    fn a_crew_cut_off_from_the_org_root_is_an_internal_error_not_a_pointer_to_itself() {
        let mut s_ = store();
        // 合成的脏形状：crew-b 与 crew-x 互为父，和总机组断开（结构操作造不出来，直接写库）。
        crewd_store::crews::set_parents(s_.conn(), "crew-b", &["crew-x".to_string()]).unwrap();
        crewd_store::crews::set_parents(s_.conn(), "crew-x", &["crew-b".to_string()]).unwrap();
        let e = err(&mut s_, |tx| ensure_human_channel(tx, "crew-b"));
        assert!(
            matches!(&e, CollabError::Store(_)) && !e.to_string().contains("crew-b/human"),
            "{e:?}"
        );
        let e = err(&mut s_, |tx| {
            crate::messages::post(
                tx,
                crate::Sender::Human,
                crate::messages::NewPost {
                    channel_id: "crew-b/human".into(),
                    text: "示例消息".into(),
                    ..Default::default()
                },
            )
        });
        assert!(
            matches!(&e, CollabError::Store(_)) && !e.to_string().contains("crew-b/human"),
            "{e:?}"
        );
    }

    fn s_ref(id: &str) -> MemberRef {
        MemberRef::session(id)
    }

    #[test]
    fn a_dm_between_two_sessions_is_readable_by_both_crews_but_injected_into_neither() {
        let mut s = store();
        let (dm, _) = ok(&mut s, |tx| {
            ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("cap-x"),
            )
        });
        let x2 = ok(&mut s, |tx| {
            crate::sessions::register(
                tx,
                crate::sessions::NewSession {
                    crew_id: "crew-x".into(),
                    role: crewd_proto::Role::Worker,
                    executor: "claude".into(),
                    title: Some("丙二".into()),
                    ..Default::default()
                },
            )
        })
        .id;
        assert!(
            readable_by(s.conn(), &dm.id, "w-a2").unwrap(),
            "发起方机组的同事读得到（B15）"
        );
        assert!(
            readable_by(s.conn(), &dm.id, &x2).unwrap(),
            "对方机组的同事也读得到（dm 显示在两边机组下）"
        );
        assert!(
            !readable_by(s.conn(), &dm.id, "w-b1").unwrap(),
            "子机组不因上级的 dm 读得到"
        );
        assert!(
            readable_channels(s.conn(), "w-a2")
                .unwrap()
                .contains(&dm.id)
        );
        assert!(
            !is_member(s.conn(), &dm.id, &MemberRef::session("w-a2")).unwrap(),
            "读得到 ≠ 成员：不注入、不叫醒"
        );
    }

    #[test]
    fn the_home_human_channel_is_readable_by_its_subtree_and_topics_by_the_owning_crew() {
        let mut s = store();
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        assert!(readable_by(s.conn(), "crew-a/human", "w-b1").unwrap());
        assert!(!readable_by(s.conn(), "crew-a/human", "cap-x").unwrap());
        let t = ok(&mut s, |tx| {
            create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![]))
        });
        assert!(
            readable_by(s.conn(), &t.id, "w-a2").unwrap(),
            "没有私有群：本机组的 topic 本机组都读得到"
        );
        assert!(!readable_by(s.conn(), &t.id, "cap-x").unwrap());
    }

    #[test]
    fn dm_peer_is_the_other_party_and_nothing_else() {
        let mut s = store();
        let (dm, _) = ok(&mut s, |tx| {
            ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("cap-x"),
            )
        });
        assert_eq!(
            dm_peer(s.conn(), &dm.id, &MemberRef::session("w-a1")).unwrap(),
            Some(MemberRef::session("cap-x"))
        );
        assert_eq!(
            dm_peer(s.conn(), &dm.id, &MemberRef::session("w-a2")).unwrap(),
            None,
            "旁观者没有对方"
        );
        assert_eq!(
            dm_peer(s.conn(), "crew-a/lobby", &MemberRef::session("w-a1")).unwrap(),
            None,
            "不是 dm"
        );
    }

    /// N1-R3（PM 2026-10-07）：总机组 = 管理层，它的 session 读得到全组织的群；读得到仍 ≠ 成员。
    #[test]
    fn the_org_roots_sessions_read_every_channel() {
        let mut s = store();
        let chief_cap = ok(&mut s, |tx| {
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
        })
        .id;
        let (dm, _) = ok(&mut s, |tx| {
            ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("cap-x"),
            )
        });
        let tb = ok(&mut s, |tx| {
            create(tx, &Sender::Session("w-b1".into()), topic("crew-b", vec![]))
        });
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        for id in [
            "crew-x/lobby",
            dm.id.as_str(),
            tb.id.as_str(),
            "crew-a/human",
        ] {
            assert!(
                readable_by(s.conn(), id, &chief_cap).unwrap(),
                "{id}：总机组读全组织"
            );
            assert!(
                !is_member(s.conn(), id, &MemberRef::session(chief_cap.clone())).unwrap(),
                "{id}：读得到 ≠ 成员"
            );
        }
        let all: Vec<String> = {
            let mut st = s
                .conn()
                .prepare("SELECT id FROM channels ORDER BY created_at, id")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(all.len(), 7, "四个大厅 + dm + topic + 人类群：{all:?}");
        assert_eq!(readable_channels(s.conn(), &chief_cap).unwrap(), all);
    }

    /// N1-R3：除总机组外，上级机组读不到子机组的群（只看自己机组下的 + 自己 home 的人类群）。
    #[test]
    fn a_parent_crew_does_not_read_its_child_crews_channels() {
        let mut s = store();
        let tb = ok(&mut s, |tx| {
            create(tx, &Sender::Session("w-b1".into()), topic("crew-b", vec![]))
        });
        assert!(readable_by(s.conn(), &tb.id, "cap-b").unwrap());
        assert!(!readable_by(s.conn(), &tb.id, "cap-a").unwrap());
        assert!(!readable_by(s.conn(), "crew-b/lobby", "cap-a").unwrap());
        let mine = readable_channels(s.conn(), "cap-a").unwrap();
        assert!(
            !mine.contains(&tb.id) && mine.contains(&"crew-a/lobby".to_string()),
            "{mine:?}"
        );
    }

    /// 人类群的读 / 发言 / 注入权从 `home_of` 推出，不认陈旧的成员行：机组被挪到另一个一级机组下之后，
    /// 它的 session 在旧 home 人类群里发过言留下的那一行不再算数（progress NOTE Task 6）。
    #[test]
    fn a_re_homed_session_loses_its_old_homes_human_channel() {
        let mut s = store();
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-x"));
        let say = |text: &str| crate::messages::NewPost {
            channel_id: "crew-a/human".into(),
            text: text.into(),
            ..Default::default()
        };
        ok(&mut s, |tx| {
            crate::messages::post(tx, Sender::Session("w-b1".into()), say("示例消息"))
        });
        assert!(
            is_member(s.conn(), "crew-a/human", &s_ref("w-b1")).unwrap(),
            "发言即入群"
        );
        ok(&mut s, |tx| {
            crate::crews::release(tx, "crew-b", "crew-a", Some("crew-x"))
        });
        assert_eq!(crate::crews::home_of(s.conn(), "crew-b").unwrap(), "crew-x");
        assert!(
            !readable_by(s.conn(), "crew-a/human", "w-b1").unwrap(),
            "读权跟着 home 走"
        );
        assert!(readable_by(s.conn(), "crew-x/human", "w-b1").unwrap());
        assert!(
            !is_member(s.conn(), "crew-a/human", &s_ref("w-b1")).unwrap(),
            "不再注入、不能再被 @ 叫醒"
        );
        assert!(
            !channels_of_session(s.conn(), "w-b1")
                .unwrap()
                .iter()
                .any(|c| c.id == "crew-a/human")
        );
        assert!(matches!(
            err(&mut s, |tx| crate::messages::post(
                tx,
                Sender::Session("w-b1".into()),
                say("越界")
            )),
            CollabError::Forbidden(_)
        ));
    }

    #[test]
    fn only_sessions_homed_here_can_be_added_to_a_human_channel() {
        let mut s = store();
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        assert!(matches!(
            err(&mut s, |tx| add_members(
                tx,
                &Sender::Human,
                "crew-a/human",
                &[s_ref("cap-x")]
            )),
            CollabError::Invalid(_)
        ));
        assert!(!is_member(s.conn(), "crew-a/human", &s_ref("cap-x")).unwrap());
        ok(&mut s, |tx| {
            add_members(tx, &Sender::Human, "crew-a/human", &[s_ref("w-b1")])
        });
        assert!(is_member(s.conn(), "crew-a/human", &s_ref("w-b1")).unwrap());
    }
}
