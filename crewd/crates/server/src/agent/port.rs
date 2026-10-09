//! 唯一直接调 M3a（crewd-collab / crewd-store / SessionControl / TokenTable）的文件。
//! 名字取自 M3a 计划的冻结接口块；M3a 落地时与这里不同，只改这个文件。
//!
//! 约定：读 = `st.db.call(|s| collab::X(s.conn(), ..))`；写 = `st.collab(|tx| collab::X(tx, ..))`
//! （同一事务里跑并把 `Change` 发到 Hub）。视图里的名字是原值：给 agent 看之前由调用方过
//! `render::safe_name`（群 / 发言者标签用本文件的 `display_label` / `sender_label`，它们已经过了）。
//!
//! 可见性：「读得到哪些群」只有一条规则，在 collab（B15：`channels::readable_by` / `readable_channels`，
//! 含总机组读全组织）∪ 自己的收听；`readable_channels` / `message` / `messages_as` / `search` /
//! `watched_channels` 都经本文件的 `can_read`。@ 只决定叫醒谁，不藏消息（§24.2），所以读到的群里每条都看得见。
//! `messages_raw` 是**不带视角**的原始读（系统 / 测试用，名字里带 `_raw` 就是怕误用）。
//! `crew_channels` 是结构性列表（`list_for_crew` 的口径，比可读集合宽），不是可读性判定。
//!
//! 授权：M3a 对机组 DAG（`attach_parent` / `detach_parent` / `create_crew`）与会话控制（`start_session` /
//! `stop_session` / `nudge` / `set_profile` / `handoff_captain` ...）**不做**「谁能调」的授权——`by` 参数在
//! M3a 里是被忽略的。这些操作的授权在 T15 的工具层，判据是 collab 的 `crews::acts_as_captain_of`（经本文件的
//! `acts_as_captain_of`；总机组的机长在每个机组都按机长算，C2），工具层不另写一份。
use std::fmt;

use crewd_collab::{Change, CollabError, Locale, Sender, Tx};
pub use crewd_proto::ChannelKind;
use crewd_proto::{Mention, Message};
use rusqlite::Connection;
use tokio::sync::broadcast;

use crate::AppState;
use crate::agent::{Identity, Role};
use crate::control::ControlError;

pub type PortResult<T> = Result<T, PortError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    NotFound(Reason),
    /// M3a 按规则拒绝（不是成员、已归档、成环、没权限、冲突……）：给 agent 的说明，原样给。
    Refused(Reason),
    Internal(String),
}

/// 拒绝的说明：collab 的规则拒绝带稳定码（`Refusal`），到了知道 agent 语言的地方（`tools::Ctx::fail`）
/// 才按语言渲染；其余来源（会话控制、本文件）是已写好的一句话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Rule(crewd_collab::refusal::Refusal),
    Text(String),
}

impl Reason {
    /// 按 agent 的语言渲染（`Text` 原样）。
    pub fn text(&self, l: Locale) -> String {
        match self {
            Self::Rule(r) => r.text(l),
            Self::Text(s) => s.clone(),
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rule(r) => write!(f, "{r}"),
            Self::Text(s) => f.write_str(s),
        }
    }
}

impl From<String> for Reason {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for Reason {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl PortError {
    /// 给 agent 看的说明（`Internal` 不该给 agent 看细节，调用方另行处理；这里只为完整）。
    pub fn text(&self, l: Locale) -> String {
        match self {
            Self::NotFound(r) | Self::Refused(r) => r.text(l),
            Self::Internal(s) => s.clone(),
        }
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(s) | Self::Refused(s) => write!(f, "{s}"),
            Self::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

impl std::error::Error for PortError {}

impl From<CollabError> for PortError {
    fn from(e: CollabError) -> Self {
        match e {
            CollabError::NotFound(r) => Self::NotFound(Reason::Rule(r)),
            CollabError::Invalid(r) | CollabError::Forbidden(r) | CollabError::Conflict(r) => {
                Self::Refused(Reason::Rule(r))
            }
            CollabError::Store(s) => Self::Internal(s.to_string()),
            // The ledger's own invariant broke: a server fault, same as `dispatch::collab_err`.
            CollabError::Work(crewd_work::Rejected::Invariant(v)) => {
                Self::Internal(format!("work book invariant violated: {v}"))
            }
            CollabError::Work(other) => Self::Refused(Reason::Text(other.to_string())),
        }
    }
}

impl From<crewd_store::StoreError> for PortError {
    fn from(e: crewd_store::StoreError) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<ControlError> for PortError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::NotFound(s) => Self::NotFound(Reason::Text(s)),
            other => Self::Refused(Reason::Text(other.to_string())),
        }
    }
}

pub fn now_ms() -> i64 {
    crewd_store::now_ms()
}

/// 读：在库线程上拿只读连接跑 `f`。
async fn read<T, F>(st: &AppState, f: F) -> PortResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> crewd_collab::Result<T> + Send + 'static,
{
    Ok(st.db.call(move |s| f(s.conn())).await??)
}

/// 写：一个事务，提交后事件进 Hub（`AppState::collab`）。
async fn write<T, F>(st: &AppState, f: F) -> PortResult<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
{
    Ok(st.collab(f).await?)
}

/// `NotFound` 当成「没有」。
fn opt<T>(r: crewd_collab::Result<T>) -> crewd_collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------------------------
// 视图类型
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberRef {
    Session(String),
    Human,
}

impl MemberRef {
    fn to_proto(&self) -> crewd_proto::MemberRef {
        match self {
            Self::Session(id) => crewd_proto::MemberRef::session(id.clone()),
            Self::Human => crewd_proto::MemberRef::Human,
        }
    }
    fn from_proto(m: &crewd_proto::MemberRef) -> Self {
        match m {
            crewd_proto::MemberRef::Session { id } => Self::Session(id.clone()),
            crewd_proto::MemberRef::Human => Self::Human,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelView {
    pub id: String,
    pub crew_id: String,
    pub crew_title: String,
    pub kind: ChannelKind,
    /// 大厅 / 人类群是空串（M3a 约定）；显示用 `display_label`。
    pub name: String,
    pub purpose: Option<String>,
    /// 群主的 session id（人类做群主、或机组没有机长时 None）。
    pub owner: Option<String>,
    /// **有效**成员（M3a `channels::members`：大厅含本机组全部 session，人类群含根机长）。
    pub members: Vec<MemberRef>,
    pub watched: bool,
    pub archived: bool,
    /// 只在 `my_channels` 里有意义：我在收听它（不一定是成员）。
    pub listening: bool,
}

impl ChannelView {
    /// 这个 session 是不是群的（有效）成员。大厅的成员已在 `members` 里按本机组展开，所以别的机组的大厅是 false。
    pub fn has_session(&self, s: &str) -> bool {
        self.members
            .iter()
            .any(|m| matches!(m, MemberRef::Session(x) if x == s))
    }

    fn from_summary(
        conn: &Connection,
        s: crewd_proto::ChannelSummary,
    ) -> crewd_collab::Result<Self> {
        let crew_title = crewd_collab::crews::get(conn, &s.channel.crew_id)?.title;
        Ok(Self {
            owner: match &s.channel.owner {
                Some(crewd_proto::MemberRef::Session { id }) => Some(id.clone()),
                _ => None,
            },
            members: s.members.iter().map(MemberRef::from_proto).collect(),
            watched: s.watched,
            archived: s.channel.archived_at.is_some(),
            listening: false,
            id: s.channel.id,
            crew_id: s.channel.crew_id,
            crew_title,
            kind: s.channel.kind,
            name: s.channel.name,
            purpose: s.channel.purpose,
        })
    }
}

fn channel_view(conn: &Connection, id: &str) -> crewd_collab::Result<ChannelView> {
    ChannelView::from_summary(conn, crewd_collab::channels::summary(conn, id)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewView {
    pub id: String,
    pub title: String,
    pub number: Option<i64>,
    /// 总机组（管理机组，不在机组树里）。
    pub management: bool,
    pub parent_ids: Vec<String>,
    pub child_ids: Vec<String>,
    pub working_directory: Option<String>,
    /// 此刻的机长（session id）。
    pub captain: Option<String>,
    pub captain_executor: Option<String>,
    pub policy: Option<String>,
    /// Removed by the human (`crew.remove`, collab's archive): frozen, refused by every write.
    pub removed: bool,
}

fn crew_view(conn: &Connection, id: &str) -> crewd_collab::Result<CrewView> {
    use crewd_collab::crews;
    let c = crews::get(conn, id)?;
    Ok(CrewView {
        parent_ids: crews::parents(conn, id)?,
        child_ids: crews::children(conn, id)?,
        captain: crews::captain_of(conn, id)?,
        policy: c.policy.map(|v| match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        }),
        removed: c.removed_at.is_some(),
        id: c.id,
        title: c.title,
        number: c.number,
        management: c.management,
        working_directory: c.working_directory,
        captain_executor: c.captain_executor,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: String,
    pub crew_id: String,
    pub role: Role,
    pub label: String,
    /// 通讯录里的号（"7-1"、"7-3"）；没有号的是 None。
    pub extension: Option<String>,
    pub executor: String,
}

/// session 以自己身份、或系统身份发言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostSender {
    Session(String),
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostReq {
    pub channel_id: String,
    pub sender: PostSender,
    pub text: String,
    pub mentions: Vec<Mention>,
    pub in_reply_to: Option<String>,
    pub category: Option<String>,
    /// M3a 的 `message.post` 暂不收附件（M3a 裁定 14）：只能是空的。
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub message: Message,
    /// §17：@人类 而人类没在注视这个群（M3a 算好的）。
    pub unwatched_human_mention: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgRef {
    Seq(i64),
    Id(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub number: String,
    pub name: String,
    pub crew_title: String,
    /// The session behind an extension row; `None` = a crew row.
    pub session_id: Option<String>,
    /// 组织路径（顶层机组在最左，" / " 分隔；顶层机组与总机组本身是空串）；每一段机组标题都过了 `safe_name`。
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrewPatch {
    pub title: Option<String>,
    pub working_directory: Option<String>,
    pub status_line: Option<String>,
    pub policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReq {
    pub crew_id: String,
    pub role: Role,
    pub executor: Option<String>,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// C8：true = 新建 git worktree；false = 共享机组目录。
    pub isolation: bool,
}

pub use crewd_collab::render::ChannelBlock;

// ---------------------------------------------------------------------------------------------
// 身份 / 设置 / 事件
// ---------------------------------------------------------------------------------------------

pub async fn identity(st: &AppState, session: &str) -> PortResult<Option<Identity>> {
    let id = session.to_string();
    read(st, move |conn| {
        let row = match opt(crewd_collab::sessions::get(conn, &id))? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(Identity {
            role: crewd_collab::sessions::role_of(conn, &id)?,
            label: crewd_collab::sessions::display_name(conn, &id)?,
            crew_id: row.crew_id,
            session_id: row.id,
        }))
    })
    .await
}

pub async fn sessions_of(st: &AppState, crew: &str) -> PortResult<Vec<SessionView>> {
    let crew = crew.to_string();
    read(st, move |conn| {
        let numbers: std::collections::HashMap<String, String> =
            crewd_collab::directory::list(conn, None)?
                .into_iter()
                .filter_map(|r| r.session_id.map(|s| (s, r.number)))
                .collect();
        let mut out = Vec::new();
        for s in crewd_collab::sessions::list(conn, Some(&crew))? {
            out.push(SessionView {
                role: crewd_collab::sessions::role_of(conn, &s.id)?,
                label: crewd_collab::sessions::display_name(conn, &s.id)?,
                extension: numbers.get(&s.id).cloned(),
                crew_id: s.crew_id,
                executor: s.executor,
                id: s.id,
            });
        }
        Ok(out)
    })
    .await
}

pub async fn locale(st: &AppState) -> PortResult<Locale> {
    read(st, crewd_collab::settings::locale).await
}

/// Joint launch/SessionStart contract: DB identity/role/locale + the existing typed control quota.
/// M3c launch callers must use this context, not the DB-only context_for. Provider feeding remains
/// M3c-owned; empty/missing tiers, unsupported source and failed read are distinct outcomes.
pub async fn world_context(
    st: &AppState,
    session: &str,
) -> PortResult<crewd_collab::worldview::WorldCtx> {
    use crewd_collab::worldview::QuotaTier;
    let session = session.to_string();
    let mut ctx = read(st, move |conn| {
        let loc = crewd_collab::settings::locale(conn)?;
        crewd_collab::worldview::context_for(conn, &session, loc)
    })
    .await?;
    let tiers = match tokio::time::timeout(std::time::Duration::from_secs(1), st.sessions.quota())
        .await
    {
        Ok(Ok(report)) => {
            let tier = |executor: &str| {
                report
                    .lines
                    .iter()
                    .filter(|l| l.executor == executor)
                    .filter_map(|l| l.plan.as_deref())
                    .find(|p| !p.trim().is_empty())
                    .map(|p| QuotaTier::Known(p.to_owned()))
                    .unwrap_or(QuotaTier::Unknown)
            };
            (tier("claude"), tier("codex"))
        }
        Ok(Err(ControlError::Unsupported(_))) => (QuotaTier::Unavailable, QuotaTier::Unavailable),
        Ok(Err(e)) => {
            eprintln!("crewd: world quota source failed: {e}");
            (QuotaTier::Failed, QuotaTier::Failed)
        }
        Err(_) => {
            eprintln!("crewd: world quota source deadline");
            (QuotaTier::Failed, QuotaTier::Failed)
        }
    };
    (ctx.quota_claude, ctx.quota_codex) = tiers;
    Ok(ctx)
}

/// Re-read world facts at the moment SessionStart fires, never the launch-time copy.
pub async fn live_world(st: &AppState, session: &str) -> PortResult<String> {
    Ok(crewd_collab::worldview::render_live(
        &world_context(st, session).await?,
    ))
}

pub fn verify_token(st: &AppState, session: &str, token: &str) -> bool {
    st.tokens.verify(session, token)
}

pub fn subscribe_collab(st: &AppState) -> broadcast::Receiver<Change> {
    st.hub.subscribe_collab()
}

// ---------------------------------------------------------------------------------------------
// 机组
// ---------------------------------------------------------------------------------------------

pub async fn crew(st: &AppState, id: &str) -> PortResult<Option<CrewView>> {
    let id = id.to_string();
    read(st, move |conn| opt(crew_view(conn, &id))).await
}

/// id → 号码（全数字，比 `Crew.number`；`"0"` = 总机组；撞号取最早建的，同 `directory::resolve`）→ 标题恰好一个。
/// 已移除的机组：按 id / 标题当没有；按号码 = Conflict `directory.crew_removed`（号还在、不回收，原样回给 agent）。
pub async fn resolve_crew(st: &AppState, arg: &str) -> PortResult<Option<CrewView>> {
    use crewd_collab::refusal::{Code, Refusal};
    use rusqlite::OptionalExtension;
    let arg = arg.to_string();
    read(st, move |conn| {
        let all = crewd_collab::crews::list(conn, false)?;
        if let Some(c) = all.iter().find(|c| c.id == arg) {
            return opt(crew_view(conn, &c.id));
        }
        if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_digit()) {
            let by_number: Option<String> = conn
                .query_row(
                    "SELECT id FROM crews WHERE CAST(number AS TEXT)=?1 ORDER BY created_at, id LIMIT 1",
                    [&arg],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = by_number {
                if crewd_collab::crews::is_archived(conn, &id)? {
                    return Err(CollabError::Conflict(
                        Refusal::new(Code::DirectoryCrewRemoved).arg("number", arg.clone()),
                    ));
                }
                return opt(crew_view(conn, &id));
            }
        }
        let mut hits = all.iter().filter(|c| c.title == arg);
        match (hits.next(), hits.next()) {
            (Some(c), None) => opt(crew_view(conn, &c.id)),
            _ => Ok(None),
        }
    })
    .await
}

/// C2: may `session` act as the captain of `crew`: its current captain, or the management crew's
/// captain (who counts as the captain of every crew). collab `crews::acts_as_captain_of`, the one
/// copy of this rule; unknown session / crew = false.
pub async fn acts_as_captain_of(st: &AppState, session: &str, crew: &str) -> PortResult<bool> {
    let (s, c) = (session.to_string(), crew.to_string());
    read(st, move |conn| {
        crewd_collab::crews::acts_as_captain_of(conn, &s, &c)
    })
    .await
}

pub async fn update_crew(st: &AppState, id: &str, p: CrewPatch) -> PortResult<()> {
    let id = id.to_string();
    write(st, move |tx| {
        crewd_collab::crews::update(
            tx,
            &id,
            crewd_collab::crews::CrewPatch {
                title: p.title,
                working_directory: p.working_directory,
                status_line: p.status_line,
                policy: p.policy.map(serde_json::Value::String),
                ..Default::default()
            },
        )
        .map(|_| ())
    })
    .await
}

/// Set the working directory of several crews in one transaction (`change_workdir`): all or none.
/// collab's `crews::update` refuses an empty path or a removed crew.
pub async fn set_working_directory(
    st: &AppState,
    crews: Vec<String>,
    path: String,
) -> PortResult<()> {
    write(st, move |tx| {
        for id in &crews {
            crewd_collab::crews::update(
                tx,
                id,
                crewd_collab::crews::CrewPatch {
                    working_directory: Some(path.clone()),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    })
    .await
}

/// `child` 加上 `parent` 做父（`crews::adopt`）。成环、总机组、已移除由 collab 判；谁能调（结构可达，R0）
/// 只在工具层 `crew_structure` 判，这里不收调用者。
pub async fn attach_parent(st: &AppState, child: &str, parent: &str) -> PortResult<()> {
    let (c, p) = (child.to_string(), parent.to_string());
    write(st, move |tx| {
        crewd_collab::crews::adopt(tx, &c, &p).map(|_| ())
    })
    .await
}

/// `child` 从 `parent` 下放出来（`crews::release`）；`to` = 改挂到哪个机组。谁能调同 `attach_parent`。
pub async fn detach_parent(
    st: &AppState,
    parent: &str,
    child: &str,
    to: Option<String>,
) -> PortResult<()> {
    let (p, c) = (parent.to_string(), child.to_string());
    write(st, move |tx| {
        crewd_collab::crews::release(tx, &c, &p, to.as_deref()).map(|_| ())
    })
    .await
}

/// A crew to create (`crew_structure`). collab's `crews::create` refuses an empty title or
/// directory and a parent outside the tree; the caller fills the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewCrewReq {
    pub title: String,
    pub working_directory: String,
    /// Created under this crew (appended at the end of its children), else top-level (at the end).
    pub parent: Option<String>,
    /// This crew becomes a child of the new one (`crews::create_above`; then `parent` is ignored).
    pub child: Option<String>,
    /// Posted as System into the new crew's lobby (why the crew exists, for the human).
    pub notice: Option<String>,
}

/// Create a crew, hang `child` under it and post `notice`: one transaction, so a refusal (e.g.
/// `crew.management_not_in_tree` for `child`) leaves nothing behind.
pub async fn create_crew(st: &AppState, r: NewCrewReq) -> PortResult<CrewView> {
    write(st, move |tx| {
        let n = crewd_collab::crews::NewCrew {
            title: r.title,
            working_directory: r.working_directory,
            parent_id: r.parent,
            title_source: None,
        };
        let c = match &r.child {
            Some(child) => crewd_collab::crews::create_above(tx, n, child)?,
            None => crewd_collab::crews::create(tx, n)?,
        };
        if let Some(text) = r.notice {
            crewd_collab::messages::post(
                tx,
                Sender::System,
                crewd_collab::messages::NewPost {
                    channel_id: crewd_store::channels::lobby_id(&c.id),
                    text,
                    in_reply_to: None,
                    mentions: vec![],
                    attachments: vec![],
                    category: None,
                },
            )?;
        }
        crew_view(tx.conn(), &c.id)
    })
    .await
}

/// The crew whose human channel `crew`'s asks go to (collab `crews::home_of`).
pub async fn home_of(st: &AppState, crew: &str) -> PortResult<String> {
    let crew = crew.to_string();
    read(st, move |conn| crewd_collab::crews::home_of(conn, &crew)).await
}

// ---------------------------------------------------------------------------------------------
// 群
// ---------------------------------------------------------------------------------------------

pub async fn channel(st: &AppState, id: &str) -> PortResult<Option<ChannelView>> {
    let id = id.to_string();
    read(st, move |conn| opt(channel_view(conn, &id))).await
}

/// 我在的群（大厅 / 显式加入 / 机长的人类群）∪ 我在收听的群（`listening = true`，不一定是成员）。
pub async fn my_channels(st: &AppState, session: &str) -> PortResult<Vec<ChannelView>> {
    let s = session.to_string();
    read(st, move |conn| {
        let listening = crewd_collab::listens::channels_of(conn, &s, now_ms())?;
        let mut out = Vec::new();
        for ch in crewd_collab::channels::channels_of_session(conn, &s)? {
            let mut v = channel_view(conn, &ch.id)?;
            v.listening = listening.contains(&ch.id);
            out.push(v);
        }
        for id in listening {
            if !out.iter().any(|v| v.id == id) {
                let mut v = channel_view(conn, &id)?;
                v.listening = true;
                out.push(v);
            }
        }
        Ok(out)
    })
    .await
}

fn views_of(tx: &mut Tx<'_>, crew: &str) -> crewd_collab::Result<Vec<ChannelView>> {
    let sums = crewd_collab::channels::list_for_crew(tx, crew)?;
    sums.into_iter()
        .map(|s| ChannelView::from_summary(tx.conn(), s))
        .collect()
}

/// `channels::list_for_crew`（顶层机组 / 总机组会顺手懒建人类群，所以走写事务）。结构性列表，**比可读集合宽**
/// （含「有本机组成员」的别组的群）：要判「读不读得到」用 `readable_channels`。
pub async fn crew_channels(st: &AppState, crew: &str) -> PortResult<Vec<ChannelView>> {
    let crew = crew.to_string();
    write(st, move |tx| views_of(tx, &crew)).await
}

pub async fn lobby_of(st: &AppState, crew: &str) -> PortResult<ChannelView> {
    let id = crewd_store::channels::lobby_id(crew);
    read(st, move |conn| channel_view(conn, &id)).await
}

/// `ensure_human_channel(home_of(crew))`：home = 顶层祖先（总机组 / 顶层机组是自己）；懒建，建一次。
pub async fn home_human_channel(st: &AppState, crew: &str) -> PortResult<ChannelView> {
    let crew = crew.to_string();
    write(st, move |tx| {
        let home = crewd_collab::crews::home_of(tx.conn(), &crew)?;
        let ch = crewd_collab::channels::ensure_human_channel(tx, &home)?;
        channel_view(tx.conn(), &ch.id)
    })
    .await
}

/// home（顶层祖先）的人类群，**不建**：还没建过就是 `None`。只读，给列表 / 回执用（建群只发生在往里发言、
/// `home_human_channel` 与 collab 自己的路径上）。
pub async fn home_human_channel_if_any(
    st: &AppState,
    crew: &str,
) -> PortResult<Option<ChannelView>> {
    let crew = crew.to_string();
    read(st, move |conn| {
        let home = crewd_collab::crews::home_of(conn, &crew)?;
        opt(channel_view(conn, &crewd_store::channels::human_id(&home)))
    })
    .await
}

/// 人此刻正在注视的群里 `me` 读得到的那些。只读：只看已存在的群，不替任何树懒建人类群、不发事件。
pub async fn watched_channels(st: &AppState, me: &Identity) -> PortResult<Vec<ChannelView>> {
    let s = me.session_id.clone();
    read(st, move |conn| {
        let ids: Vec<String> = {
            let mut q =
                conn.prepare("SELECT channel_id FROM channel_watch ORDER BY since, channel_id")?;
            q.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if opt(crewd_collab::channels::get(conn, &id))?.is_some() && can_read(conn, &s, &id)? {
                out.push(channel_view(conn, &id)?);
            }
        }
        Ok(out)
    })
    .await
}

pub async fn create_channel(
    st: &AppState,
    by: &Identity,
    kind: ChannelKind,
    name: String,
    purpose: Option<String>,
    members: Vec<MemberRef>,
) -> PortResult<ChannelView> {
    let (by_id, crew) = (by.session_id.clone(), by.crew_id.clone());
    write(st, move |tx| {
        let ch = crewd_collab::channels::create(
            tx,
            &Sender::Session(by_id),
            crewd_collab::channels::NewChannel {
                crew_id: crew,
                kind,
                name,
                purpose,
                members: members.iter().map(MemberRef::to_proto).collect(),
            },
        )?;
        channel_view(tx.conn(), &ch.id)
    })
    .await
}

/// 同一对 session 只有一个 dm；复用时不看机组（谁发起都拿到同一个），`by.crew_id` 只决定新建时归属。
pub async fn ensure_dm(
    st: &AppState,
    by: &Identity,
    other: &str,
) -> PortResult<(ChannelView, bool)> {
    let (me, other, crew) = (by.session_id.clone(), other.to_string(), by.crew_id.clone());
    write(st, move |tx| {
        let (ch, created) = crewd_collab::channels::ensure_dm(
            tx,
            &crew,
            &crewd_proto::MemberRef::session(me),
            &crewd_proto::MemberRef::session(other),
        )?;
        Ok((channel_view(tx.conn(), &ch.id)?, created))
    })
    .await
}

/// 权限由 M3a 按 `by` 判（群主 / 机长 / 人），这里不重复判。
pub async fn add_members(
    st: &AppState,
    by: &Identity,
    channel: &str,
    ms: Vec<MemberRef>,
) -> PortResult<ChannelView> {
    let (by_id, ch) = (by.session_id.clone(), channel.to_string());
    write(st, move |tx| {
        let ms: Vec<_> = ms.iter().map(MemberRef::to_proto).collect();
        crewd_collab::channels::add_members(tx, &Sender::Session(by_id), &ch, &ms)?;
        channel_view(tx.conn(), &ch)
    })
    .await
}

pub async fn remove_member(
    st: &AppState,
    by: &Identity,
    channel: &str,
    m: MemberRef,
) -> PortResult<()> {
    let (by_id, ch) = (by.session_id.clone(), channel.to_string());
    write(st, move |tx| {
        crewd_collab::channels::remove_member(tx, &Sender::Session(by_id), &ch, &m.to_proto())
            .map(|_| ())
    })
    .await
}

pub async fn archive_channel(st: &AppState, by: &Identity, channel: &str) -> PortResult<()> {
    let (by_id, ch) = (by.session_id.clone(), channel.to_string());
    write(st, move |tx| {
        crewd_collab::channels::update(
            tx,
            &Sender::Session(by_id),
            &ch,
            crewd_collab::channels::ChannelPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .map(|_| ())
    })
    .await
}

pub async fn request_owner_transfer(
    st: &AppState,
    by: &Identity,
    channel: &str,
    to: &MemberRef,
) -> PortResult<()> {
    let (by_id, ch, to) = (by.session_id.clone(), channel.to_string(), to.to_proto());
    write(st, move |tx| {
        crewd_collab::channels::request_owner_transfer(tx, &Sender::Session(by_id), &ch, &to)
            .map(|_| ())
    })
    .await
}

pub async fn accept_owner_transfer(st: &AppState, by: &Identity, channel: &str) -> PortResult<()> {
    let (by_id, ch) = (by.session_id.clone(), channel.to_string());
    write(st, move |tx| {
        crewd_collab::channels::accept_owner_transfer(tx, &Sender::Session(by_id), &ch).map(|_| ())
    })
    .await
}

/// 机长强制换群主（`by` 的权限由 M3a 判）。
pub async fn force_owner(
    st: &AppState,
    by: &Identity,
    channel: &str,
    to: &MemberRef,
) -> PortResult<()> {
    let (by_id, ch, to) = (by.session_id.clone(), channel.to_string(), to.to_proto());
    write(st, move |tx| {
        crewd_collab::channels::force_owner(tx, &Sender::Session(by_id), &ch, &to).map(|_| ())
    })
    .await
}

pub async fn set_listen(
    st: &AppState,
    session: &str,
    channel: &str,
    until_ms: Option<i64>,
    senders: Vec<String>,
) -> PortResult<()> {
    let (s, ch) = (session.to_string(), channel.to_string());
    // 门在 collab：`listens::set` 要求读得到、群没归档（拒 Forbidden `listen.not_readable`），这里不另做一道。
    write(st, move |tx| {
        crewd_collab::listens::set(tx, &s, &ch, until_ms, senders)
    })
    .await
}

/// 返回删掉的收听行数（0 = 本来就没在收听）。
pub async fn clear_listen(st: &AppState, session: &str, channel: &str) -> PortResult<usize> {
    let (s, ch) = (session.to_string(), channel.to_string());
    write(st, move |tx| {
        crewd_collab::listens::clear(tx, &s, Some(&ch))
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 消息
// ---------------------------------------------------------------------------------------------

pub async fn post(st: &AppState, r: PostReq) -> PortResult<Posted> {
    if !r.attachments.is_empty() {
        return Err(PortError::Refused(
            "attachments are not supported yet".into(),
        ));
    }
    let sender = match r.sender {
        PostSender::Session(id) => Sender::Session(id),
        PostSender::System => Sender::System,
    };
    let np = crewd_collab::messages::NewPost {
        channel_id: r.channel_id,
        text: r.text,
        in_reply_to: r.in_reply_to,
        mentions: r.mentions,
        attachments: vec![],
        category: r.category,
    };
    let p = write(st, move |tx| crewd_collab::messages::post(tx, sender, np)).await?;
    Ok(Posted {
        message: p.message,
        unwatched_human_mention: p.unwatched_human_mention,
    })
}

/// 一页消息（旧在前）。`before_seq` 不含；`None` = 最新的一页。
/// **不按视角过滤、不查群的可读性**：只给系统和测试用，绝不能把结果交给 agent。agent 要看的读一律走
/// `messages_as` / `search` / `message`（名字里的 `_raw` 就是让误用编译不过）。
pub async fn messages_raw(
    st: &AppState,
    channel: &str,
    before_seq: Option<i64>,
    limit: u32,
) -> PortResult<Vec<Message>> {
    let ch = channel.to_string();
    // M3a `list` 的 before = None 是游标之后最旧的一页；最新一页必须给 before（i64::MAX）。
    read(st, move |conn| {
        crewd_collab::messages::list(conn, &ch, None, Some(before_seq.unwrap_or(i64::MAX)), limit)
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 「读得到」：collab 的 B15 规则 ∪ 自己的收听（spec §24.1 B15、§26.1 Task 6）
// ---------------------------------------------------------------------------------------------

/// session S 读得到群 C ⇔ collab `channels::readable_by`（⓪ 总机组读全组织；① 成员；② 挂在 S 的机组下；
/// ③ S 的 home 的人类群）或 S 在收听 C（`listens::channels_of` 已只算读得到、没归档的收听行）。
/// `readable_channels` / `message` / `messages_as` / `search` / `watched_channels` 都经这一个函数。
fn can_read(conn: &Connection, session: &str, channel: &str) -> crewd_collab::Result<bool> {
    Ok(crewd_collab::listens::channels_of(conn, session, now_ms())?
        .iter()
        .any(|c| c == channel)
        || crewd_collab::channels::readable_by(conn, channel, session)?)
}

/// 这个 session 读得到的全部群 id（含已归档的）：只读，不建任何群。collab 的可读集合在前，收听接在后面。
pub async fn readable_channels(st: &AppState, me: &Identity) -> PortResult<Vec<String>> {
    let s = me.session_id.clone();
    read(st, move |conn| {
        let mut out = crewd_collab::channels::readable_channels(conn, &s)?;
        for id in crewd_collab::listens::channels_of(conn, &s, now_ms())? {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        Ok(out)
    })
    .await
}

/// `readable_channels` 的视图（`listening` 按此刻的收听行填上）：工具层按名字找群、列群都从这里取。
pub async fn readable_views(st: &AppState, me: &Identity) -> PortResult<Vec<ChannelView>> {
    let s = me.session_id.clone();
    read(st, move |conn| {
        let listening = crewd_collab::listens::channels_of(conn, &s, now_ms())?;
        let mut ids = crewd_collab::channels::readable_channels(conn, &s)?;
        for id in &listening {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.iter()
            .map(|id| {
                let mut v = channel_view(conn, id)?;
                v.listening = listening.contains(id);
                Ok(v)
            })
            .collect()
    })
    .await
}

/// 这个 session 此刻算数的收听（`listens::channels_of`：没过期、没归档、读得到）。
pub async fn listening_to(st: &AppState, session: &str) -> PortResult<Vec<String>> {
    let s = session.to_string();
    read(st, move |conn| {
        crewd_collab::listens::channels_of(conn, &s, now_ms())
    })
    .await
}

/// `messages_raw` 的 agent 视角：群必须读得到（读不到 = `NotFound`，不区分「没有」和「看不见」）。旧在前。
/// @ 不藏消息（§24.2），读得到的群里每条都给。
pub async fn messages_as(
    st: &AppState,
    viewer: &str,
    channel: &str,
    before_seq: Option<i64>,
    limit: u32,
) -> PortResult<Vec<Message>> {
    let (viewer, ch) = (viewer.to_string(), channel.to_string());
    read(st, move |conn| {
        let readable =
            opt(crewd_collab::channels::get(conn, &ch))?.is_some() && can_read(conn, &viewer, &ch)?;
        if !readable {
            return Err(CollabError::NotFound(
                crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::ChannelNotFound)
                    .arg("channel", ch.clone()),
            ));
        }
        crewd_collab::messages::list(
            conn,
            &ch,
            None,
            Some(before_seq.unwrap_or(i64::MAX)),
            limit.clamp(1, 1000),
        )
    })
    .await
}

/// 群里最新一条的 seq（只是个数，不含内容；系统 / 测试 / 「我发之前有没有新消息」的判断用）。
pub async fn latest_seq(st: &AppState, channel: &str) -> PortResult<Option<i64>> {
    let ch = channel.to_string();
    read(st, move |conn| {
        Ok(
            crewd_collab::messages::list(conn, &ch, None, Some(i64::MAX), 1)?
                .last()
                .map(|m| m.seq),
        )
    })
    .await
}

/// Automatic reply mentions from collab's canonical resolver; agent post quota uses them before
/// reserving. The persisted write resolves again (no cached membership or caller-provided flag).
pub async fn reply_mentions(
    st: &AppState,
    session: &str,
    channel: &str,
    reply: &str,
) -> PortResult<Vec<Mention>> {
    let (session, channel, reply) = (session.to_string(), channel.to_string(), reply.to_string());
    read(st, move |conn| {
        crewd_collab::messages::resolve_reply(conn, &Sender::Session(session), &channel, &reply)
            .map(|r| r.mentions)
    })
    .await
}

/// 按 seq（全库唯一）或 id 取一条消息。只给 `me` 读得到的群里的消息；否则 `None`
/// （不区分「不存在」与「看不见」，免得靠它探别的机组）。数据库错误照实往上抛，不当成「没有」。
pub async fn message(st: &AppState, me: &Identity, r: MsgRef) -> PortResult<Option<Message>> {
    let me = me.session_id.clone();
    read(st, move |conn| {
        let found = match r {
            MsgRef::Seq(n) => opt(crewd_collab::messages::get_by_seq(conn, n))?,
            MsgRef::Id(id) => opt(crewd_collab::messages::get(conn, &id))?,
        };
        let Some(m) = found else {
            return Ok(None);
        };
        Ok(can_read(conn, &me, &m.channel_id)?.then_some(m))
    })
    .await
}

/// 在 `channel_ids` 里搜；范围先收进 `viewer` 读得到的群（给了读不到的群 = 当没给）。
/// 新的在前；最多 `limit` 条（collab 的上限 500）。
pub async fn search(
    st: &AppState,
    viewer: &str,
    query: &str,
    channel_ids: Vec<String>,
    after: Option<String>,
    before: Option<String>,
    limit: u32,
) -> PortResult<Vec<Message>> {
    let (viewer, query) = (viewer.to_string(), query.to_string());
    read(st, move |conn| {
        let mut ids = Vec::new();
        for id in channel_ids {
            if opt(crewd_collab::channels::get(conn, &id))?.is_some()
                && can_read(conn, &viewer, &id)?
            {
                ids.push(id);
            }
        }
        crewd_collab::messages::search(
            conn,
            &crewd_collab::messages::Search {
                query,
                channel_ids: Some(ids),
                after,
                before,
                limit: limit.min(500),
                crew_id: None,
            },
        )
    })
    .await
}

/// The sender session of each message id (`None` = not a session, or no such message). Ids only,
/// no content: the injection digest uses it to keep replies to the reader unfolded.
pub async fn message_senders(
    st: &AppState,
    ids: Vec<String>,
) -> PortResult<std::collections::HashMap<String, Option<String>>> {
    read(st, move |conn| {
        let mut out = std::collections::HashMap::new();
        for id in ids {
            let s = opt(crewd_collab::messages::get(conn, &id))?.and_then(|m| m.sender_session_id);
            out.insert(id, s);
        }
        Ok(out)
    })
    .await
}

/// 群在列表 / 读取面上的显示名（M3a `channels::display_label`，已过 `safe_name`）。
/// dm 的对方已不在时是空串：调用方给一句 md 里的回退文字。dm 的修饰按**此刻**的机长指针。
pub async fn display_label(
    st: &AppState,
    viewer: &str,
    channel: &str,
    loc: Locale,
) -> PortResult<String> {
    let (viewer, ch) = (viewer.to_string(), channel.to_string());
    read(st, move |conn| {
        let c = crewd_collab::channels::get(conn, &ch)?;
        crewd_collab::channels::display_label(
            conn,
            &c,
            &crewd_proto::MemberRef::session(viewer),
            loc,
        )
    })
    .await
}

/// 发言者标签（M3a `render::sender_label`：与注入面逐字相同，带机长 / 分机号修饰，已过 `safe_name`）。
pub async fn sender_label(st: &AppState, m: &Message, loc: Locale) -> PortResult<String> {
    let m = m.clone();
    read(st, move |conn| {
        crewd_collab::render::sender_label(conn, &m, loc)
    })
    .await
}

/// 注入面：各群游标之后、对它可见、不是它自己发的消息（M3a 把缺的游标钉在末尾、本次不返回）。
pub async fn unread(st: &AppState, session: &str) -> PortResult<Vec<(String, Vec<Message>)>> {
    let s = session.to_string();
    write(st, move |tx| crewd_collab::cursors::unread_for(tx, &s)).await
}

pub async fn blocks(
    st: &AppState,
    session: &str,
    loc: Locale,
    unread: Vec<(String, Vec<Message>)>,
) -> PortResult<Vec<ChannelBlock>> {
    let s = session.to_string();
    read(st, move |conn| {
        crewd_collab::render::blocks_for(conn, &s, loc, &unread)
    })
    .await
}

/// The injection's header line (what `injection` starts with), for a text without any block.
pub fn injection_header(loc: Locale) -> String {
    crewd_collab::prompts::render_file(loc)
        .get("inject.header")
        .to_string()
}

pub fn injection(loc: Locale, blocks: &[ChannelBlock]) -> String {
    crewd_collab::render::injection(loc, blocks)
}

/// Several cursors in ONE write transaction: a failure part-way advances none of them.
pub async fn advance_all(
    st: &AppState,
    session: &str,
    targets: Vec<(String, i64)>,
) -> PortResult<()> {
    let s = session.to_string();
    write(st, move |tx| {
        for (ch, seq) in &targets {
            crewd_collab::cursors::advance(tx, &s, ch, *seq)?;
        }
        Ok(())
    })
    .await
}

/// 这个 session 在群里的已读游标（没有游标 = `None`）。
pub async fn cursor(st: &AppState, session: &str, channel: &str) -> PortResult<Option<i64>> {
    let (s, ch) = (session.to_string(), channel.to_string());
    read(st, move |conn| crewd_collab::cursors::cursor(conn, &s, &ch)).await
}

pub async fn advance(st: &AppState, session: &str, channel: &str, seq: i64) -> PortResult<()> {
    let (s, ch) = (session.to_string(), channel.to_string());
    write(st, move |tx| {
        crewd_collab::cursors::advance(tx, &s, &ch, seq)
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 通讯录
// ---------------------------------------------------------------------------------------------

pub async fn directory(st: &AppState, query: Option<String>) -> PortResult<Vec<DirEntry>> {
    read(st, move |conn| {
        crewd_collab::directory::list(conn, query.as_deref())?
            .into_iter()
            .map(|r| {
                Ok(DirEntry {
                    crew_title: crewd_collab::crews::get(conn, &r.crew_id)?.title,
                    number: r.number,
                    name: r.name,
                    session_id: r.session_id,
                    detail: r
                        .org_path
                        .split(" / ")
                        .map(crewd_collab::render::safe_name)
                        .collect::<Vec<_>>()
                        .join(" / "),
                })
            })
            .collect()
    })
    .await
}

/// 收件人 → session id（C3）：通讯录地址走 collab `directory::resolve`（`"0"` = 总机组机长，`"N"` / `"N-1"` = N 号机组
/// **此刻**的机长，`"N-E"` = 分机；拒绝原样带码回来：`directory.bad_address` / `no_such_crew` / `crew_removed` /
/// `no_such_extension`、`crew.no_captain`）。不像地址（不全是数字和 `-`）而又恰好是一个 session 的 id 时，就是那个 session——
/// 工具里每个 session 参数都同时收地址和 id。
pub async fn resolve_address(st: &AppState, arg: &str) -> PortResult<String> {
    let arg = arg.trim().to_string();
    read(st, move |conn| {
        let address_like = arg.bytes().all(|b| b.is_ascii_digit() || b == b'-');
        if !address_like && opt(crewd_collab::sessions::get(conn, &arg))?.is_some() {
            return Ok(arg);
        }
        crewd_collab::directory::resolve(conn, &arg)
    })
    .await
}

/// A member argument that names a session (C3): an address (only digits and `-`) goes through collab
/// `directory::resolve` (its refusals come back with their codes); anything else must be the id of
/// an existing session (`session.not_found`). Unlike `resolve_address` (post.to), a non-address
/// that is no session is "no such session", not "bad address".
pub async fn resolve_session(st: &AppState, arg: &str) -> PortResult<String> {
    let arg = arg.trim().to_string();
    read(st, move |conn| {
        if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return crewd_collab::directory::resolve(conn, &arg);
        }
        crewd_collab::sessions::get(conn, &arg).map(|s| s.id)
    })
    .await
}

/// Does `s` name the human (`human`, any ASCII case, or the human's sender label in either locale)?
pub fn names_the_human(s: &str) -> bool {
    let s = s.trim();
    s.eq_ignore_ascii_case("human")
        || [Locale::Zh, Locale::En]
            .iter()
            .any(|l| s == crewd_collab::prompts::render_file(*l).get("sender.human"))
}

/// How a member is called in a receipt: the human's sender label, or the session's display name
/// (through `safe_name`).
pub async fn member_label(st: &AppState, m: &MemberRef, loc: Locale) -> PortResult<String> {
    match m {
        MemberRef::Human => Ok(crewd_collab::prompts::render_file(loc)
            .get("sender.human")
            .to_string()),
        MemberRef::Session(id) => {
            let id = id.clone();
            read(st, move |conn| {
                Ok(crewd_collab::render::safe_name(
                    &crewd_collab::sessions::display_name(conn, &id)?,
                ))
            })
            .await
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 人类待确认 / Todo / 唤醒
// ---------------------------------------------------------------------------------------------

/// 一条刚开的问：H 号，和公告实际发进的群（collab `asks::open` 选的 home 人类群，同一事务里读回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub ask: String,
    pub channel: ChannelView,
}

/// 开一条人类待确认。`fallback` = (按它办的文字, 多少分钟没回复就按它办)。发到哪个群由 collab 定（home 人类群，
/// S68）；这里只把它读回来给回执用，不另算一遍。
pub async fn open_ask(
    st: &AppState,
    by: &Identity,
    question: &str,
    fallback: Option<(String, i64)>,
    resume_note: Option<String>,
) -> PortResult<Asked> {
    let (s, question) = (by.session_id.clone(), question.to_string());
    write(st, move |tx| {
        let a = crewd_collab::asks::open(
            tx,
            &s,
            crewd_collab::asks::OpenAsk {
                question,
                default: fallback.map(|(text, minutes)| crewd_proto::AskDefault {
                    text,
                    after_minutes: minutes,
                }),
                resume_note,
            },
        )?;
        // `asks::open` always records its announcement; a missing one is a broken invariant: a
        // server fault (`PortError::Internal`, logged with a number) that rolls the ask back, not
        // a receipt with no channel.
        let Some(m) = a.message_id.as_deref() else {
            return Err(CollabError::Store(crewd_store::StoreError::Invalid(
                format!("ask {} was opened without an announcement message", a.ask),
            )));
        };
        let posted = crewd_collab::messages::get(tx.conn(), m)?;
        Ok(Asked {
            ask: a.ask,
            channel: channel_view(tx.conn(), &posted.channel_id)?,
        })
    })
    .await
}

/// 撤回 `crew` 那本账里的一条问；返回规范写法的 H 号。谁能撤全由 collab 定（提问者 / 问所在机组的机长 /
/// 总机组的机长，`asks::withdraw`）。
pub async fn withdraw_ask(
    st: &AppState,
    by: &Identity,
    crew: &str,
    ask: &str,
    reason: &str,
) -> PortResult<String> {
    let (s, crew, ask, reason) = (
        by.session_id.clone(),
        crew.to_string(),
        ask.to_string(),
        reason.to_string(),
    );
    write(st, move |tx| {
        crewd_collab::asks::withdraw(tx, &Sender::Session(s), &crew, &ask, &reason).map(|a| a.ask)
    })
    .await
}

/// 给本机组的 Agent Todo `#number` 追加回应（G3），返回被回应那条的原文（同一事务里读的，S73）。
/// 没有这个号 = `Refused`（M3a 的 NotFound 对 agent 是「被拒」）。
pub async fn respond_todo(
    st: &AppState,
    by: &Identity,
    number: i64,
    text: &str,
) -> PortResult<String> {
    let (s, text) = (by.session_id.clone(), text.to_string());
    match write(st, move |tx| {
        crewd_collab::todos::respond(tx, &s, number, &text).map(|r| r.todo_text)
    })
    .await
    {
        Err(PortError::NotFound(m)) => Err(PortError::Refused(m)),
        other => other,
    }
}

/// 定时唤醒排上了没有。`Skipped` = 目标 session 的机组已移除（collab `wakes::enqueue` 不入队）：要报给调用方，
/// 不能当排上了（S85）。
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Scheduled,
    Skipped,
}

/// 给 `session` 排一个定时唤醒（`at_ms` 之后投）。每次调用一个新的去重键，同一时刻排两个也都入队。
pub async fn schedule_timer(
    st: &AppState,
    session: &str,
    at_ms: i64,
    note: &str,
) -> PortResult<Timer> {
    let (s, note) = (session.to_string(), note.to_string());
    let token = uuid::Uuid::new_v4().simple().to_string();
    write(st, move |tx| {
        crewd_collab::wakes::enqueue(
            tx,
            crewd_collab::wakes::WakeRequest {
                session: s,
                kind: crewd_collab::wakes::WakeKind::Timer,
                source: crewd_collab::wakes::source::timer(&token),
                priority: crewd_collab::wakes::WakePriority::System,
                not_before_ms: Some(at_ms),
                note: Some(note),
                origin: None,
            },
        )
        .map(|e| match e {
            crewd_collab::wakes::Enqueued::Skipped => Timer::Skipped,
            crewd_collab::wakes::Enqueued::New(_) | crewd_collab::wakes::Enqueued::Duplicate(_) => {
                Timer::Scheduled
            }
        })
    })
    .await
}

/// C3：记一次产出（一个真正干活的工具调用）。
pub async fn note_progress(st: &AppState, session: &str) -> PortResult<()> {
    let s = session.to_string();
    write(st, move |tx| {
        crewd_collab::sessions::note_progress(tx, &s, crewd_collab::ProgressKind::ToolCall)
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 会话控制（SessionControl）
// ---------------------------------------------------------------------------------------------

/// The title a session started through `start_session` will get: collab's own rule
/// (`sessions::resolve_title`, the one `register` applies), checked here so its refusals
/// (`session.title_too_long` / `title_or_brief_required` / `title_is_sender_label`) reach the agent
/// before anything is started.
pub fn resolve_title(title: Option<&str>, brief: Option<&str>) -> PortResult<String> {
    Ok(crewd_collab::sessions::resolve_title(title, brief)?)
}

/// `executor` 缺省 = 机组的 `captain_executor`，再缺省 "claude"。返回新 session 的 id。
pub async fn start_session(st: &AppState, r: StartReq) -> PortResult<String> {
    let executor = match r.executor {
        Some(e) => e,
        None => crew(st, &r.crew_id)
            .await?
            .and_then(|c| c.captain_executor)
            .unwrap_or_else(|| "claude".to_string()),
    };
    let req = crate::control::StartRequest {
        crew_id: r.crew_id,
        role: r.role,
        executor,
        brief: r.brief,
        title: r.title,
        model: r.model,
        effort: r.effort,
        // C8：true = M3c 新建 git worktree；false = 共享机组目录。
        isolation: r.isolation,
    };
    Ok(st.sessions.start(req).await?.id)
}

pub async fn stop_session(st: &AppState, session: &str, reason: &str) -> PortResult<()> {
    Ok(st
        .sessions
        .stop(session.to_string(), reason.to_string())
        .await?)
}

pub async fn nudge(st: &AppState, session: &str, input: &str) -> PortResult<()> {
    Ok(st
        .sessions
        .nudge(session.to_string(), input.to_string())
        .await?)
}

fn live_lines(l: &crewd_proto::SessionLive) -> Vec<String> {
    let mut out = vec![
        format!("session: {}", l.session_id),
        format!("running: {}", l.running),
        format!("working: {}", l.working),
    ];
    if let Some(h) = &l.health {
        out.push(
            format!("health: {} {}", h.kind, h.detail)
                .trim_end()
                .to_string(),
        );
    }
    if let Some(d) = &l.pending_decision {
        out.push(format!("pending_decision: {}", d.summary));
        for (i, o) in d.options.iter().enumerate() {
            out.push(format!("  option {}: {o}", i + 1));
        }
        out.push(format!("  numbered: {}", d.numbered));
    }
    if let Some(q) = &l.awaiting_question {
        out.push(format!("awaiting_question: {q}"));
    }
    out
}

/// `Inspection.live` + `screen` 渲染成文本（英文 key 的 `key: value` 行，屏幕文本原样跟在后面）。
pub async fn inspect(st: &AppState, session: &str) -> PortResult<String> {
    let i = st.sessions.inspect(session.to_string(), 40).await?;
    let mut out = live_lines(&i.live).join("\n");
    out.push_str("\nscreen:\n");
    out.push_str(&i.screen);
    Ok(out)
}

/// 只在**下一次**起这个 session 时生效（spec C5）。
pub async fn set_profile(
    st: &AppState,
    session: &str,
    model: Option<String>,
    effort: Option<String>,
) -> PortResult<()> {
    Ok(st
        .sessions
        .set_profile(session.to_string(), model, effort)
        .await?)
}

pub async fn quota(st: &AppState) -> PortResult<String> {
    let r = st.sessions.quota().await?;
    Ok(r.lines
        .iter()
        .map(|l| {
            let mut parts = vec![format!("executor: {}", l.executor)];
            if let Some(p) = &l.plan {
                parts.push(format!("plan: {p}"));
            }
            if let Some(u) = &l.limited_until {
                parts.push(format!("limited_until: {u}"));
            }
            if let Some(d) = &l.detail {
                parts.push(format!("detail: {d}"));
            }
            parts.join(", ")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// A session's live state as SessionControl reports it (a list row picks its words from it).
pub async fn live(st: &AppState, session: &str) -> PortResult<crewd_proto::SessionLive> {
    Ok(st.sessions.live(session.to_string()).await?)
}

pub async fn live_state(st: &AppState, session: &str) -> PortResult<String> {
    let l = st.sessions.live(session.to_string()).await?;
    Ok(live_lines(&l).join("\n"))
}

pub async fn handoff_captain(st: &AppState, crew: &str, session: &str) -> PortResult<()> {
    Ok(st
        .sessions
        .handoff_captain(crew.to_string(), session.to_string())
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Role;
    use crate::agent::testkit::Fixture;
    use crewd_proto::MentionKind;
    use serde_json::json;

    #[tokio::test]
    async fn identity_reads_role_and_label() {
        let f = Fixture::new().await;
        let cap = identity(&f.state, &f.captain).await.unwrap().unwrap();
        assert_eq!(cap.role, Role::Captain);
        assert_eq!(cap.crew_id, f.crew);
        let w = identity(&f.state, &f.worker).await.unwrap().unwrap();
        assert_eq!((w.role, w.label.as_str()), (Role::Worker, "示例 worker"));
        assert!(identity(&f.state, "nobody").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn lobby_is_mine_and_posts_become_unread_for_others() {
        let f = Fixture::new().await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap();
        assert_eq!(lobby.kind, ChannelKind::Lobby);
        assert!(
            my_channels(&f.state, &f.worker)
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == lobby.id)
        );
        // 夹具与生产一样在登记时把大厅游标钉在末尾：之后发的消息就是未读，不用先「预热」一次。
        let m = f
            .session_post(&f.captain, &lobby.id, "示例公告", vec![])
            .await;
        let u = unread(&f.state, &f.worker).await.unwrap();
        let (_, msgs) = u.iter().find(|(c, _)| *c == lobby.id).expect("大厅有未读");
        assert_eq!(msgs.last().unwrap().id, m.id);
        advance(&f.state, &f.worker, &lobby.id, m.seq)
            .await
            .unwrap();
        assert!(
            unread(&f.state, &f.worker)
                .await
                .unwrap()
                .iter()
                .all(|(_, v)| v.is_empty())
        );
        let me = f.me(&f.worker).await;
        assert_eq!(
            message(&f.state, &me, MsgRef::Seq(m.seq))
                .await
                .unwrap()
                .unwrap()
                .id,
            m.id
        );
        assert_eq!(
            messages_raw(&f.state, &lobby.id, None, 1).await.unwrap()[0].id,
            m.id,
            "None = 最新一页"
        );
    }

    #[tokio::test]
    async fn home_human_channel_is_created_lazily_once() {
        let f = Fixture::new().await;
        let a = home_human_channel(&f.state, &f.crew).await.unwrap();
        let b = home_human_channel(&f.state, &f.crew).await.unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.kind, ChannelKind::Human);
    }

    #[tokio::test]
    async fn tokens_are_per_session() {
        let f = Fixture::new().await;
        let t = f.token(&f.worker);
        assert!(verify_token(&f.state, &f.worker, &t));
        assert!(!verify_token(&f.state, &f.captain, &t));
        assert!(!verify_token(&f.state, &f.worker, "forged"));
    }

    fn to(id: &str) -> Mention {
        Mention {
            kind: MentionKind::Session,
            target_id: Some(id.into()),
        }
    }

    /// §24.2（N1 改了 Task 3 复审 NOTE 的期望）：@ 只决定叫醒谁，不藏消息——@ 了别人的消息，经读 / 搜 /
    /// 按号取，旁观者照样读得到。
    #[tokio::test]
    async fn mentions_only_wake_they_never_hide_a_message() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap().id;
        let open = f
            .session_post(&f.captain, &lobby, "示例全员通知", vec![])
            .await;
        let directed = f
            .session_post(&f.captain, &lobby, "示例给 w2 的活", vec![to("s-w2")])
            .await;
        let w1 = f.me(&f.worker).await;
        let both = [open.id.as_str(), directed.id.as_str()];

        for who in [f.worker.as_str(), "s-w2", f.captain.as_str()] {
            let seen = messages_as(&f.state, who, &lobby, None, 10).await.unwrap();
            assert_eq!(
                seen.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
                both,
                "{who}"
            );
            let hits = search(&f.state, who, "示例", vec![lobby.clone()], None, None, 10)
                .await
                .unwrap();
            assert_eq!(hits.len(), 2, "{who}");
        }
        assert!(
            message(&f.state, &w1, MsgRef::Seq(directed.seq))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            message(&f.state, &w1, MsgRef::Id(directed.id.clone()))
                .await
                .unwrap()
                .is_some()
        );
    }

    /// 一页 = 最新的 `limit` 条，旧在前（@ 了别人的也算在页里）。
    #[tokio::test]
    async fn a_page_is_the_latest_messages_old_first() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap().id;
        f.session_post(&f.captain, &lobby, "示例甲", vec![]).await;
        let second = f
            .session_post(&f.captain, &lobby, "示例乙", vec![to("s-w2")])
            .await;
        let third = f.session_post(&f.captain, &lobby, "示例丙", vec![]).await;
        let page = messages_as(&f.state, &f.worker, &lobby, None, 2)
            .await
            .unwrap();
        assert_eq!(
            page.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [second.id.as_str(), third.id.as_str()],
            "旧在前、最新的 2 条"
        );
    }

    /// 按 seq / id 取消息只在 me 读得到的群里找：别的树的大厅读不到。
    #[tokio::test]
    async fn message_by_seq_needs_a_readable_channel() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例另一组", None).await;
        f.add_session(&other, "s-far", Role::Worker, "外组").await;
        let far_lobby = lobby_of(&f.state, &other).await.unwrap().id;
        let far = f
            .session_post("s-far", &far_lobby, "示例外组消息", vec![])
            .await;
        let w = f.me(&f.worker).await;
        assert!(
            message(&f.state, &w, MsgRef::Seq(far.seq))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            message(&f.state, &w, MsgRef::Id(far.id.clone()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            message(&f.state, &w, MsgRef::Seq(i64::MAX))
                .await
                .unwrap()
                .is_none()
        );
        let own = lobby_of(&f.state, &f.crew).await.unwrap().id;
        let mine = f
            .session_post(&f.captain, &own, "示例本组消息", vec![])
            .await;
        assert!(
            message(&f.state, &w, MsgRef::Seq(mine.seq))
                .await
                .unwrap()
                .is_some()
        );
    }

    /// M3a `list` 的 `before = None` 是最旧的一页；port 的 `None` 必须是最新的一页。
    #[tokio::test]
    async fn none_means_the_latest_page_not_the_oldest() {
        let f = Fixture::new().await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap().id;
        for i in 0..3 {
            f.session_post(&f.captain, &lobby, &format!("示例 {i}"), vec![])
                .await;
        }
        let last = f.session_post(&f.captain, &lobby, "示例末条", vec![]).await;
        let page = messages_raw(&f.state, &lobby, None, 2).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[1].id, last.id, "旧在前，最后一条是最新的");
        assert_eq!(latest_seq(&f.state, &lobby).await.unwrap(), Some(last.seq));
        let older = messages_raw(&f.state, &lobby, Some(page[0].seq), 2)
            .await
            .unwrap();
        assert!(older.iter().all(|m| m.seq < page[0].seq), "before 不含");
    }

    /// B13：别的机组的大厅不是我的群（旧计划里 `has_session` 对任何大厅都回 true）。
    #[tokio::test]
    async fn has_session_checks_real_membership() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例另一组", None).await;
        f.add_session(&other, "s-far", Role::Worker, "外组").await;
        let mine = lobby_of(&f.state, &f.crew).await.unwrap();
        let theirs = lobby_of(&f.state, &other).await.unwrap();
        assert!(mine.has_session(&f.worker) && mine.has_session(&f.captain));
        assert!(!mine.has_session("s-far"));
        assert!(theirs.has_session("s-far"));
        assert!(!theirs.has_session(&f.worker), "别的机组的大厅里没有我");
    }

    #[tokio::test]
    async fn listening_shows_up_in_my_channels_without_membership() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        assert!(
            !my_channels(&f.state, &f.worker)
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == topic.id)
        );
        set_listen(&f.state, &f.worker, &topic.id, None, vec![])
            .await
            .unwrap();
        let mine = my_channels(&f.state, &f.worker).await.unwrap();
        let v = mine.iter().find(|c| c.id == topic.id).expect("在收听的群");
        assert!(v.listening && !v.has_session(&f.worker));
        assert_eq!(
            clear_listen(&f.state, &f.worker, &topic.id).await.unwrap(),
            1
        );
        assert_eq!(
            clear_listen(&f.state, &f.worker, &topic.id).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn labels_come_from_m3a() {
        let f = Fixture::new().await;
        let loc = locale(&f.state).await.unwrap();
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap();
        // 大厅 = 机组标题（过 safe_name）。
        assert_eq!(
            display_label(&f.state, &f.worker, &lobby.id, loc)
                .await
                .unwrap(),
            "示例机组"
        );
        // dm = 对方的标签；对方已不在（取不到）时 M3a 回空串，调用方给 md 里的回退。
        let w = f.me(&f.worker).await;
        let (dm, created) = ensure_dm(&f.state, &w, &f.captain).await.unwrap();
        assert!(created);
        let label = display_label(&f.state, &f.worker, &dm.id, loc)
            .await
            .unwrap();
        assert!(label.contains("示例机长"), "{label}");
        let m = f.session_post(&f.worker, &lobby.id, "示例", vec![]).await;
        assert!(
            sender_label(&f.state, &m, loc)
                .await
                .unwrap()
                .contains("示例 worker")
        );
    }

    #[tokio::test]
    async fn crews_resolve_by_id_number_and_unique_title() {
        let f = Fixture::new().await;
        let c = crew(&f.state, &f.crew).await.unwrap().unwrap();
        assert_eq!(c.title, "示例机组");
        assert_eq!(c.captain.as_deref(), Some("s-cap"));
        let number = c.number.unwrap().to_string();
        for arg in [f.crew.as_str(), number.as_str(), "示例机组"] {
            assert_eq!(
                resolve_crew(&f.state, arg).await.unwrap().unwrap().id,
                f.crew,
                "{arg}"
            );
        }
        assert!(resolve_crew(&f.state, "没有这个").await.unwrap().is_none());
        f.add_crew("示例机组", None).await;
        assert!(
            resolve_crew(&f.state, "示例机组").await.unwrap().is_none(),
            "重名不猜"
        );
    }

    #[tokio::test]
    async fn directory_numbers_resolve_to_targets() {
        let f = Fixture::new().await;
        let x = f.register(&f.crew, Role::Worker, "示例新人").await;
        let ext = sessions_of(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == x)
            .unwrap()
            .extension
            .expect("登记的 worker 有分机号");
        assert_eq!(resolve_session(&f.state, &ext).await.unwrap(), x);
        let n = crew(&f.state, &f.crew)
            .await
            .unwrap()
            .unwrap()
            .number
            .unwrap()
            .to_string();
        assert_eq!(
            resolve_session(&f.state, &n).await.unwrap(),
            f.captain,
            "a crew number is that crew's captain right now"
        );
        assert_eq!(
            resolve_session(&f.state, &f.worker).await.unwrap(),
            f.worker
        );
        let rule = |e: PortError| match e {
            PortError::NotFound(Reason::Rule(r)) | PortError::Refused(Reason::Rule(r)) => r.code,
            other => panic!("{other:?}"),
        };
        use crewd_collab::refusal::Code;
        assert_eq!(
            rule(resolve_session(&f.state, "9999").await.unwrap_err()),
            Code::DirectoryNoSuchCrew
        );
        assert_eq!(
            rule(resolve_session(&f.state, "s-nobody").await.unwrap_err()),
            Code::SessionNotFound,
            "not an address: must be a session id"
        );
        let rows = directory(&f.state, None).await.unwrap();
        assert_eq!(
            rows.iter()
                .find(|d| d.number == ext)
                .unwrap()
                .session_id
                .as_deref(),
            Some(x.as_str())
        );
        assert!(
            rows.iter()
                .find(|d| d.number == n)
                .unwrap()
                .session_id
                .is_none()
        );
        assert!(
            directory(&f.state, None)
                .await
                .unwrap()
                .iter()
                .any(|d| d.number == n && d.crew_title == "示例机组")
        );
    }

    #[tokio::test]
    async fn control_calls_go_through_the_fake_and_errors_map() {
        let f = Fixture::new().await;
        let id = start_session(
            &f.state,
            StartReq {
                crew_id: f.crew.clone(),
                role: Role::Worker,
                executor: None,
                brief: "示例任务".into(),
                title: None,
                model: None,
                effort: None,
                isolation: true,
            },
        )
        .await
        .unwrap();
        assert!(id.starts_with("fake-"));
        let calls = f.control_calls();
        assert!(
            calls[0].starts_with("Start(")
                && calls[0].contains("claude")
                && calls[0].contains("isolation: true"),
            "{calls:?}"
        );
        stop_session(&f.state, &f.worker, "示例原因").await.unwrap();
        assert!(
            inspect(&f.state, &f.worker)
                .await
                .unwrap()
                .contains("running: false")
        );
        assert!(
            live_state(&f.state, &f.worker)
                .await
                .unwrap()
                .contains("working: false")
        );
        assert_eq!(quota(&f.state).await.unwrap(), "");
        // Unsupported 一类映射成 Refused；NotFound 保持 NotFound。
        assert!(matches!(
            PortError::from(ControlError::NotFound("x".into())),
            PortError::NotFound(_)
        ));
        assert!(matches!(
            PortError::from(ControlError::Unsupported("x".into())),
            PortError::Refused(_)
        ));
        assert!(matches!(
            PortError::from(CollabError::Invalid(crewd_collab::refusal::Refusal::new(
                crewd_collab::refusal::Code::MessageEmpty,
            ))),
            PortError::Refused(_)
        ));
    }

    #[tokio::test]
    async fn timers_and_progress_and_todo_errors() {
        let f = Fixture::new().await;
        let w = f.me(&f.worker).await;
        let at = now_ms() + 60_000;
        for _ in 0..2 {
            assert_eq!(
                schedule_timer(&f.state, &f.worker, at, "示例备注")
                    .await
                    .unwrap(),
                Timer::Scheduled
            );
        }
        let wakes = f
            .state
            .db
            .call(|s| crewd_collab::wakes::list(s.conn(), Some("s-w1"), 10))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wakes.len(), 2, "同一时刻排两个也都入队");
        let mut rx = subscribe_collab(&f.state);
        note_progress(&f.state, &f.worker).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(Change::Progress { .. })));
        assert!(matches!(
            respond_todo(&f.state, &w, 42, "示例").await,
            Err(PortError::Refused(_))
        ));
    }

    /// 一条「读得到」的规则（§22.7 Q3：没有私有群）的每一款。
    struct World {
        f: Fixture,
        other: String,
    }

    async fn world() -> World {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let child = f.add_crew("示例子组", Some(&f.crew)).await;
        f.add_session(&child, "s-ch", Role::Worker, "子组 worker")
            .await;
        let other = f.add_crew("示例另一棵树", None).await;
        f.add_session(&other, "s-fcap", Role::Captain, "外组机长")
            .await;
        f.add_session(&other, "s-far", Role::Worker, "外组 worker")
            .await;
        f.add_session(&other, "s-far2", Role::Worker, "外组 worker 二")
            .await;
        World { f, other }
    }

    async fn can(f: &Fixture, who: &str, ch: &str) -> bool {
        let me = f.me(who).await;
        readable_channels(&f.state, &me)
            .await
            .unwrap()
            .iter()
            .any(|c| c == ch)
    }

    async fn sees(f: &Fixture, who: &str, seq: i64) -> bool {
        message(&f.state, &f.me(who).await, MsgRef::Seq(seq))
            .await
            .unwrap()
            .is_some()
    }

    /// ① 成员读得到；② 话题所属机组的人、以及话题里有自己机组的在群 session 的人读得到（B15：挂在本机组下的群，
    /// `channel.list(本机组)` 列得出的就读得到）；不相干的机组读不到。
    #[tokio::test]
    async fn readable_clause_member_and_channel_crew() {
        let w = world().await;
        let f = &w.f;
        let fcap = f.me("s-fcap").await;
        let topic = create_channel(
            &f.state,
            &fcap,
            ChannelKind::Topic,
            "示例外组话题".into(),
            None,
            vec![MemberRef::Session(f.worker.clone())],
        )
        .await
        .unwrap();
        assert!(can(f, &f.worker, &topic.id).await, "① 成员");
        assert!(can(f, "s-far2", &topic.id).await, "② 话题所属机组的人");
        assert!(
            can(f, "s-w2", &topic.id).await && can(f, &f.captain, &topic.id).await,
            "② 群里有本机组的在群 session：挂在本机组下（N1 改了 M3b 的期望）"
        );
        assert!(
            !can(f, "s-ch", &topic.id).await,
            "子机组不读上级机组名下的群（只看本机组）"
        );
        let m = f
            .session_post("s-fcap", &topic.id, "示例话题消息", vec![])
            .await;
        assert!(sees(f, "s-w1", m.seq).await);
        assert!(sees(f, "s-w2", m.seq).await);
        assert!(!sees(f, "s-ch", m.seq).await);
    }

    /// 私聊也是机组的群（B15：没有私有群）；跨机组的 dm 两边机组的人都读得到（② 群里有本机组的在群 session），
    /// 子机组读不到。
    #[tokio::test]
    async fn readable_clause_cross_crew_dm() {
        let w = world().await;
        let f = &w.f;
        let me = f.me(&f.worker).await;
        let (dm, _) = ensure_dm(&f.state, &me, "s-far").await.unwrap();
        assert!(
            can(f, &f.worker, &dm.id).await && can(f, "s-far", &dm.id).await,
            "(a) 两个成员"
        );
        assert!(
            can(f, "s-w2", &dm.id).await && can(f, &f.captain, &dm.id).await,
            "(c) dm 所属机组的人"
        );
        assert!(
            can(f, "s-far2", &dm.id).await && can(f, "s-fcap", &dm.id).await,
            "② 对方机组的人也读得到（N1 改了 M3b 的期望）"
        );
        assert!(
            !can(f, "s-ch", &dm.id).await,
            "子组 worker 不是 dm 两边机组的人"
        );
        let m = f.session_post(&f.worker, &dm.id, "示例私聊", vec![]).await;
        assert!(sees(f, "s-w2", m.seq).await);
        assert!(sees(f, "s-far2", m.seq).await);
        assert!(!sees(f, "s-ch", m.seq).await);
    }

    /// ③ 自己 home（顶层机组）的人类群：整棵树的任何 session 读得到（含子组 worker）；别的顶层机组的不行。
    #[tokio::test]
    async fn readable_clause_home_human_channel() {
        let w = world().await;
        let f = &w.f;
        let human = home_human_channel(&f.state, &f.crew).await.unwrap();
        let far_human = home_human_channel(&f.state, &w.other).await.unwrap();
        for who in ["s-w1", "s-w2", "s-ch", "s-cap"] {
            assert!(can(f, who, &human.id).await, "{who} 读得到本树的人类群");
            assert!(
                !can(f, who, &far_human.id).await,
                "{who} 读不到别的树的人类群"
            );
        }
        for who in ["s-fcap", "s-far"] {
            assert!(!can(f, who, &human.id).await, "{who} 读不到别的树的人类群");
        }
        let m = f.human_post(&human.id, "示例人类消息", vec![]).await;
        assert!(
            sees(f, "s-ch", m.seq).await,
            "子组 worker 读得到 home 的人类群"
        );
        assert!(!sees(f, "s-far", m.seq).await);
        assert_eq!(
            messages_as(&f.state, "s-ch", &human.id, None, 5)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            messages_as(&f.state, "s-far", &human.id, None, 5)
                .await
                .is_err(),
            "别的树不能翻页读"
        );
    }

    /// 收听的门在 collab（`listens::set` 拒 `listen.not_readable`）；读不到的群上遗留的收听行（裸 SQL 写入，模拟
    /// 旧数据）不让它变得读得到（§26.1：失去读权的收听行留着但不算）。
    #[tokio::test]
    async fn listening_clause_and_set_listen_gate() {
        let w = world().await;
        let f = &w.f;
        let far_lobby = lobby_of(&f.state, &w.other).await.unwrap().id;
        match set_listen(&f.state, "s-w2", &far_lobby, None, vec![]).await {
            Err(PortError::Refused(Reason::Rule(r))) => {
                assert_eq!(r.code, crewd_collab::refusal::Code::ListenNotReadable)
            }
            other => panic!("{other:?}"),
        }
        assert!(!can(f, "s-w2", &far_lobby).await);
        // 子组 worker 可以收听 home 的人类群（③）。
        let human = home_human_channel(&f.state, &f.crew).await.unwrap().id;
        set_listen(&f.state, "s-ch", &human, None, vec![])
            .await
            .unwrap();
        // 遗留的收听行（绕过门）：不算。
        let lobby = far_lobby.clone();
        f.state
            .db
            .call(move |s| {
                s.conn().execute(
                    "INSERT INTO listens(session_id,channel_id,until,senders,created_at)
                     VALUES('s-w2',?1,NULL,'[]','2026-10-07T00:00:00.000Z')",
                    [&lobby],
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert!(!can(f, "s-w2", &far_lobby).await, "遗留的收听行不给读权");
        let m = f
            .session_post("s-far", &far_lobby, "示例外组", vec![])
            .await;
        assert!(!sees(f, "s-w2", m.seq).await);
        assert!(!can(f, "s-w1", &far_lobby).await);
    }

    /// ⓪ 总机组（管理层）的 session 读得到全组织的群：别的顶层机组的大厅、人类群、私聊都读得到。
    #[tokio::test]
    async fn management_sessions_read_the_whole_organisation() {
        let w = world().await;
        let f = &w.f;
        let management = f.management_crew().await;
        f.add_session(&management, "s-mgr", Role::Worker, "管理层 worker")
            .await;
        assert_eq!(f.me("s-mgr").await.crew_id, management);
        let far_lobby = lobby_of(&f.state, &w.other).await.unwrap().id;
        let far_human = home_human_channel(&f.state, &w.other).await.unwrap().id;
        let far = f.me("s-far").await;
        let (dm, _) = ensure_dm(&f.state, &far, "s-far2").await.unwrap();
        for ch in [&far_lobby, &far_human, &dm.id] {
            assert!(can(f, "s-mgr", ch).await, "{ch}");
        }
        let m = f
            .session_post("s-far", &dm.id, "示例外组私聊", vec![])
            .await;
        assert!(sees(f, "s-mgr", m.seq).await);
        assert!(!sees(f, "s-w1", m.seq).await);
    }

    /// search 的范围被收进可读集合：给了读不到的群也搜不出来。
    #[tokio::test]
    async fn search_is_scoped_to_readable_channels() {
        let w = world().await;
        let f = &w.f;
        let far_lobby = lobby_of(&f.state, &w.other).await.unwrap().id;
        let own = lobby_of(&f.state, &f.crew).await.unwrap().id;
        f.session_post("s-far", &far_lobby, "示例暗号", vec![])
            .await;
        let mine = f.session_post(&f.captain, &own, "示例暗号", vec![]).await;
        let hits = search(
            &f.state,
            &f.worker,
            "示例暗号",
            vec![far_lobby, own],
            None,
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(
            hits.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [mine.id.as_str()]
        );
    }

    /// watched_channels 只读：不建、不发事件；只返回调用者读得到的。
    #[tokio::test]
    async fn watched_channels_is_read_only_and_scoped() {
        let w = world().await;
        let f = &w.f;
        let own = lobby_of(&f.state, &f.crew).await.unwrap().id;
        let far = lobby_of(&f.state, &w.other).await.unwrap().id;
        for id in [&own, &far] {
            f.rpc("channel.watch", json!({"channelId": id, "on": true}))
                .await;
        }
        let far_human = crewd_store::channels::human_id(&w.other);
        let mut rx = subscribe_collab(&f.state);
        let me = f.me(&f.worker).await;
        let got = watched_channels(&f.state, &me).await.unwrap();
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            [own.as_str()]
        );
        assert!(rx.try_recv().is_err(), "没有事件");
        assert!(
            channel(&f.state, &far_human).await.unwrap().is_none(),
            "没替别的树懒建人类群"
        );
    }

    #[tokio::test]
    async fn directory_detail_is_neutralised() {
        let f = Fixture::new().await;
        let parent = f.add_crew("示例[根]\u{202e}", None).await;
        f.add_crew("示例子组", Some(&parent)).await;
        let rows = directory(&f.state, None).await.unwrap();
        let child = rows.iter().find(|d| d.name == "示例子组").unwrap();
        assert_eq!(child.detail, "示例\u{300c}根\u{300d}");
    }

    #[test]
    fn work_book_invariants_are_internal_not_refusals() {
        use crewd_work::{Rejected, Violation};
        let e = PortError::from(CollabError::Work(Rejected::Invariant(Violation::Cycle(
            vec![],
        ))));
        assert!(matches!(e, PortError::Internal(_)), "{e:?}");
        let e = PortError::from(CollabError::Work(Rejected::OwnerRequired));
        assert!(matches!(e, PortError::Refused(_)), "{e:?}");
    }
}
