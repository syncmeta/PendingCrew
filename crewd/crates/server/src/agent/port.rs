//! 唯一直接调 M3a（crewd-collab / crewd-store / SessionControl / TokenTable）的文件。
//! 名字取自 M3a 计划的冻结接口块；M3a 落地时与这里不同，只改这个文件。
//!
//! 约定：读 = `st.db.call(|s| collab::X(s.conn(), ..))`；写 = `st.collab(|tx| collab::X(tx, ..))`
//! （同一事务里跑并把 `Change` 发到 Hub）。视图里的名字是原值：给 agent 看之前由调用方过
//! `render::safe_name`（群 / 发言者标签用本文件的 `display_label` / `sender_label`，它们已经过了）。
//!
//! 可见性：`messages` / `latest_seq` 是**不带视角**的原始读（系统 / 测试用）；工具要给 agent 看的读一律走
//! `messages_as` / `search` / `message`，它们按 `cursors::visible_to` 过滤，收窄给别人的消息读不到。
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
    NotFound(String),
    /// M3a 按规则拒绝（不是成员、已归档、成环、没权限、冲突……）；文本是 M3a 写的说明，原样给 agent。
    Refused(String),
    Internal(String),
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
            CollabError::NotFound(s) => Self::NotFound(s),
            CollabError::Store(s) => Self::Internal(s.to_string()),
            other => Self::Refused(other.to_string()),
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
            ControlError::NotFound(s) => Self::NotFound(s),
            other => Self::Refused(other.to_string()),
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
    pub builtin: bool,
    pub parent_ids: Vec<String>,
    pub child_ids: Vec<String>,
    pub working_directory: Option<String>,
    /// 此刻的机长（session id）。
    pub captain: Option<String>,
    pub captain_executor: Option<String>,
    pub policy: Option<String>,
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
        id: c.id,
        title: c.title,
        number: c.number,
        builtin: c.builtin,
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
pub enum NumberTarget {
    Crew(String),
    Session { crew_id: String, session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub number: String,
    pub name: String,
    pub crew_title: String,
    /// 组织路径（根在最左，" / " 分隔；根机组本身是空串）。
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

pub use crewd_collab::approvals::Gate;
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
        let crew = crewd_collab::crews::get(conn, &row.crew_id)?;
        Ok(Some(Identity {
            role: crewd_collab::sessions::role_of(conn, &id)?,
            label: crewd_collab::sessions::display_name(conn, &id)?,
            builtin_crew: crew.builtin,
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

/// id → 号码（全数字，比 `Crew.number`）→ 标题恰好一个。
pub async fn resolve_crew(st: &AppState, arg: &str) -> PortResult<Option<CrewView>> {
    let arg = arg.to_string();
    read(st, move |conn| {
        let all = crewd_collab::crews::list(conn)?;
        if let Some(c) = all.iter().find(|c| c.id == arg) {
            return opt(crew_view(conn, &c.id));
        }
        if !arg.is_empty()
            && arg.chars().all(|c| c.is_ascii_digit())
            && let Some(c) = all
                .iter()
                .find(|c| c.number.is_some_and(|n| n.to_string() == arg))
        {
            return opt(crew_view(conn, &c.id));
        }
        let mut hits = all.iter().filter(|c| c.title == arg);
        match (hits.next(), hits.next()) {
            (Some(c), None) => opt(crew_view(conn, &c.id)),
            _ => Ok(None),
        }
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

/// `child` 加上 `parent` 做父（`crews::adopt`）。权限与成环由 M3a 判。
pub async fn attach_parent(
    st: &AppState,
    _by: &Identity,
    child: &str,
    parent: &str,
) -> PortResult<()> {
    let (c, p) = (child.to_string(), parent.to_string());
    write(st, move |tx| {
        crewd_collab::crews::adopt(tx, &c, &p).map(|_| ())
    })
    .await
}

/// `child` 从 `parent` 下放出来（`crews::release`）；`to` = 改挂到哪个机组。
pub async fn detach_parent(
    st: &AppState,
    _by: &Identity,
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

/// 缺省由调用方补齐：M3a 的 `crews::create` 拒空标题 / 空目录。
pub async fn create_crew(
    st: &AppState,
    _by: &Identity,
    title: String,
    workdir: String,
    parent: Option<String>,
) -> PortResult<CrewView> {
    write(st, move |tx| {
        let c = crewd_collab::crews::create(
            tx,
            crewd_collab::crews::NewCrew {
                title,
                working_directory: workdir,
                parent_id: parent,
                title_source: None,
            },
        )?;
        crew_view(tx.conn(), &c.id)
    })
    .await
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

/// `channels::list_for_crew`（根机组会顺手懒建人类群，所以走写事务）。
pub async fn crew_channels(st: &AppState, crew: &str) -> PortResult<Vec<ChannelView>> {
    let crew = crew.to_string();
    write(st, move |tx| views_of(tx, &crew)).await
}

pub async fn lobby_of(st: &AppState, crew: &str) -> PortResult<ChannelView> {
    let id = crewd_store::channels::lobby_id(crew);
    read(st, move |conn| channel_view(conn, &id)).await
}

/// `ensure_human_channel(root_of(crew))`：懒建，建一次。
pub async fn root_human_channel(st: &AppState, crew: &str) -> PortResult<ChannelView> {
    let crew = crew.to_string();
    write(st, move |tx| {
        let root = crewd_collab::crews::root_of(tx.conn(), &crew)?;
        let ch = crewd_collab::channels::ensure_human_channel(tx, &root)?;
        channel_view(tx.conn(), &ch.id)
    })
    .await
}

/// 人此刻正在注视的群（全部机组里 `watched` 的，去重）。
pub async fn watched_channels(st: &AppState) -> PortResult<Vec<ChannelView>> {
    write(st, |tx| {
        let mut out: Vec<ChannelView> = Vec::new();
        for c in crewd_collab::crews::list(tx.conn())? {
            for v in views_of(tx, &c.id)? {
                if v.watched && !out.iter().any(|x| x.id == v.id) {
                    out.push(v);
                }
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

/// 一页消息（旧在前）。`before_seq` 不含；`None` = 最新的一页。**不按视角过滤**（系统 / 测试用）；
/// 给 agent 看的读用 `messages_as`。
pub async fn messages(
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

/// `messages` 的 agent 视角：丢掉收窄给别人的消息（`cursors::visible_to`），并往前翻页补满 `limit` 条
/// （一页里被滤掉的不会让这一页变短）。旧在前。
pub async fn messages_as(
    st: &AppState,
    viewer: &str,
    channel: &str,
    before_seq: Option<i64>,
    limit: u32,
) -> PortResult<Vec<Message>> {
    let (viewer, ch) = (viewer.to_string(), channel.to_string());
    read(st, move |conn| {
        let want = limit.clamp(1, 1000) as usize;
        let mut before = before_seq.unwrap_or(i64::MAX);
        let mut out: Vec<Message> = Vec::new();
        loop {
            let page = crewd_collab::messages::list(conn, &ch, None, Some(before), want as u32)?;
            let Some(oldest) = page.first().map(|m| m.seq) else {
                break;
            };
            let full = page.len() >= want;
            let mut vis: Vec<Message> = page
                .into_iter()
                .filter(|m| crewd_collab::cursors::visible_to(m, &viewer))
                .collect();
            vis.append(&mut out);
            out = vis;
            if out.len() >= want {
                out.drain(..out.len() - want);
                break;
            }
            if !full {
                break;
            }
            before = oldest;
        }
        Ok(out)
    })
    .await
}

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

/// `me` 能读这个群吗：它所在机组的群（`list_for_crew` 的口径：归属本机组、或有本机组的成员）∪ 它在收听的群。
/// 与 `my_channels` / `crew_channels` 同一口径，只是只读、不建人类群。
fn can_read_channel(conn: &Connection, me: &Identity, channel: &str) -> crewd_collab::Result<bool> {
    let Some(ch) = opt(crewd_collab::channels::get(conn, channel))? else {
        return Ok(false);
    };
    if ch.crew_id == me.crew_id
        || crewd_collab::listens::channels_of(conn, &me.session_id, now_ms())?.contains(&ch.id)
    {
        return Ok(true);
    }
    for m in crewd_collab::channels::members(conn, &ch.id)? {
        if let crewd_proto::MemberRef::Session { id } = m
            && matches!(crewd_collab::sessions::crew_of(conn, &id), Ok(c) if c == me.crew_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 按 seq（全库唯一）或 id 取一条消息。只给 `me` 读得到的群里、对 `me` 可见的消息；否则 `None`
/// （不区分「不存在」与「看不见」，免得靠它探别的机组）。
pub async fn message(st: &AppState, me: &Identity, r: MsgRef) -> PortResult<Option<Message>> {
    let me = me.clone();
    read(st, move |conn| {
        let found = match r {
            MsgRef::Seq(n) => opt(crewd_collab::messages::get_by_seq(conn, n))?,
            MsgRef::Id(id) => opt(crewd_collab::messages::get(conn, &id))?,
        };
        Ok(found.filter(|m| {
            crewd_collab::cursors::visible_to(m, &me.session_id)
                && can_read_channel(conn, &me, &m.channel_id).unwrap_or(false)
        }))
    })
    .await
}

/// 在 `channel_ids` 里搜（调用方先按自己能读的群圈定）。丢掉收窄给别人的消息；新的在前；最多 `limit` 条。
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
        // 按可见性滤掉之后条数会少：先按 M3a 的上限（500）多取，再截到 limit。
        let hits = crewd_collab::messages::search(
            conn,
            &crewd_collab::messages::Search {
                query,
                channel_ids: Some(channel_ids),
                after,
                before,
                limit: 500,
                crew_id: None,
            },
        )?;
        Ok(hits
            .into_iter()
            .filter(|m| crewd_collab::cursors::visible_to(m, &viewer))
            .take(limit as usize)
            .collect())
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

pub fn injection(loc: Locale, blocks: &[ChannelBlock]) -> String {
    crewd_collab::render::injection(loc, blocks)
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
                    detail: r.org_path,
                })
            })
            .collect()
    })
    .await
}

/// 在通讯录里找 `number` 完全相等的那一行。
pub async fn resolve_number(st: &AppState, number: &str) -> PortResult<Option<NumberTarget>> {
    let number = number.to_string();
    read(st, move |conn| {
        Ok(crewd_collab::directory::list(conn, None)?
            .into_iter()
            .find(|r| r.number == number)
            .map(|r| match r.session_id {
                Some(session_id) => NumberTarget::Session {
                    crew_id: r.crew_id,
                    session_id,
                },
                None => NumberTarget::Crew(r.crew_id),
            }))
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 人类待确认 / Todo / 唤醒
// ---------------------------------------------------------------------------------------------

/// 开一条人类待确认，返回 "H<n>"。`fallback` = (按它办的文字, 多少分钟没回复就按它办)。
pub async fn open_ask(
    st: &AppState,
    by: &Identity,
    question: &str,
    fallback: Option<(String, i64)>,
    resume_note: Option<String>,
) -> PortResult<String> {
    let (s, question) = (by.session_id.clone(), question.to_string());
    write(st, move |tx| {
        crewd_collab::asks::open(
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
        )
        .map(|a| a.ask)
    })
    .await
}

pub async fn withdraw_ask(st: &AppState, by: &Identity, ask: &str, reason: &str) -> PortResult<()> {
    let (s, crew, ask, reason) = (
        by.session_id.clone(),
        by.crew_id.clone(),
        ask.to_string(),
        reason.to_string(),
    );
    write(st, move |tx| {
        crewd_collab::asks::withdraw(tx, &Sender::Session(s), &crew, &ask, &reason).map(|_| ())
    })
    .await
}

/// 给本机组的 Agent Todo `#number` 追加回应（G3）。没有这个号 = `Refused`（M3a 的 NotFound 对 agent 是「被拒」）。
pub async fn respond_todo(st: &AppState, by: &Identity, number: i64, text: &str) -> PortResult<()> {
    let (s, text) = (by.session_id.clone(), text.to_string());
    match write(st, move |tx| {
        crewd_collab::todos::respond(tx, &s, number, &text).map(|_| ())
    })
    .await
    {
        Err(PortError::NotFound(m)) => Err(PortError::Refused(m)),
        other => other,
    }
}

/// 给 `session` 排一个定时唤醒（`at_ms` 之后投）。每次调用一个新的去重键，同一时刻排两个也都入队。
pub async fn schedule_timer(
    st: &AppState,
    session: &str,
    at_ms: i64,
    note: &str,
) -> PortResult<()> {
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
        .map(|_| ())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// 审批
// ---------------------------------------------------------------------------------------------

/// C2：一次调用，不等。用票 / 只拒 / 开新请求（开新请求时 M3a 自己发人类群通知）。
pub async fn gate_approval(
    st: &AppState,
    by: &Identity,
    tool_name: &str,
    summary: &str,
    tool_use_id: Option<String>,
) -> PortResult<Gate> {
    let n = crewd_collab::approvals::NewApproval {
        session_id: by.session_id.clone(),
        tool_name: tool_name.to_string(),
        summary: summary.to_string(),
        tool_use_id,
    };
    write(st, move |tx| crewd_collab::approvals::gate(tx, n)).await
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

    #[tokio::test]
    async fn identity_reads_role_label_and_builtin() {
        let f = Fixture::new().await;
        let cap = identity(&f.state, &f.captain).await.unwrap().unwrap();
        assert_eq!(cap.role, Role::Captain);
        assert_eq!(cap.crew_id, f.crew);
        let w = identity(&f.state, &f.worker).await.unwrap().unwrap();
        assert_eq!((w.role, w.label.as_str()), (Role::Worker, "示例 worker"));
        assert!(!w.builtin_crew);
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
            messages(&f.state, &lobby.id, None, 1).await.unwrap()[0].id,
            m.id,
            "None = 最新一页"
        );
    }

    #[tokio::test]
    async fn root_human_channel_is_created_lazily_once() {
        let f = Fixture::new().await;
        let a = root_human_channel(&f.state, &f.crew).await.unwrap();
        let b = root_human_channel(&f.state, &f.crew).await.unwrap();
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

    /// Task 3 复审 NOTE：收窄给别人的消息，经读 / 搜 / 按号取都读不到；发的人和收的人读得到。
    #[tokio::test]
    async fn narrowed_messages_are_hidden_from_other_readers() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap().id;
        let open = f
            .session_post(&f.captain, &lobby, "示例全员通知", vec![])
            .await;
        let secret = f
            .session_post(&f.captain, &lobby, "示例悄悄话", vec![to("s-w2")])
            .await;
        let w1 = f.me(&f.worker).await;
        let w2 = f.me("s-w2").await;

        let seen_by_w1 = messages_as(&f.state, &f.worker, &lobby, None, 10)
            .await
            .unwrap();
        assert_eq!(
            seen_by_w1.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [open.id.as_str()]
        );
        let seen_by_w2 = messages_as(&f.state, "s-w2", &lobby, None, 10)
            .await
            .unwrap();
        assert_eq!(seen_by_w2.len(), 2);
        let seen_by_cap = messages_as(&f.state, &f.captain, &lobby, None, 10)
            .await
            .unwrap();
        assert_eq!(seen_by_cap.len(), 2, "发的人自己看得见");

        assert!(
            message(&f.state, &w1, MsgRef::Seq(secret.seq))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            message(&f.state, &w1, MsgRef::Id(secret.id.clone()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            message(&f.state, &w2, MsgRef::Seq(secret.seq))
                .await
                .unwrap()
                .is_some()
        );

        let hits = search(
            &f.state,
            &f.worker,
            "示例",
            vec![lobby.clone()],
            None,
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(
            hits.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [open.id.as_str()]
        );
        let hits = search(
            &f.state,
            "s-w2",
            "示例",
            vec![lobby.clone()],
            None,
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(hits.len(), 2);
    }

    /// 被滤掉的消息不让一页变短：往前翻页补满。
    #[tokio::test]
    async fn filtered_messages_do_not_shorten_a_page() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap().id;
        let first = f.session_post(&f.captain, &lobby, "示例甲", vec![]).await;
        let second = f.session_post(&f.captain, &lobby, "示例乙", vec![]).await;
        for i in 0..3 {
            f.session_post(&f.captain, &lobby, &format!("示例私 {i}"), vec![to("s-w2")])
                .await;
        }
        let page = messages_as(&f.state, &f.worker, &lobby, None, 2)
            .await
            .unwrap();
        assert_eq!(
            page.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [first.id.as_str(), second.id.as_str()],
            "旧在前、补满 2 条"
        );
    }

    /// 按 seq 取消息只在 me 读得到的群里找：别的机组的群、没加入的话题读不到。
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
        // 收听之后就读得到。
        set_listen(&f.state, &f.worker, &far_lobby, None, vec![])
            .await
            .unwrap();
        assert!(
            message(&f.state, &w, MsgRef::Seq(far.seq))
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
        let page = messages(&f.state, &lobby, None, 2).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[1].id, last.id, "旧在前，最后一条是最新的");
        assert_eq!(latest_seq(&f.state, &lobby).await.unwrap(), Some(last.seq));
        let older = messages(&f.state, &lobby, Some(page[0].seq), 2)
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
        match resolve_number(&f.state, &ext).await.unwrap() {
            Some(NumberTarget::Session {
                crew_id,
                session_id,
            }) => assert_eq!((crew_id, session_id), (f.crew.clone(), x)),
            other => panic!("{other:?}"),
        }
        let n = crew(&f.state, &f.crew)
            .await
            .unwrap()
            .unwrap()
            .number
            .unwrap()
            .to_string();
        assert_eq!(
            resolve_number(&f.state, &n).await.unwrap(),
            Some(NumberTarget::Crew(f.crew.clone()))
        );
        assert!(resolve_number(&f.state, "9999").await.unwrap().is_none());
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
            PortError::from(CollabError::Invalid("x".into())),
            PortError::Refused(_)
        ));
    }

    #[tokio::test]
    async fn timers_and_progress_and_todo_errors() {
        let f = Fixture::new().await;
        let w = f.me(&f.worker).await;
        let at = now_ms() + 60_000;
        schedule_timer(&f.state, &f.worker, at, "示例备注")
            .await
            .unwrap();
        schedule_timer(&f.state, &f.worker, at, "示例备注")
            .await
            .unwrap();
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
}
