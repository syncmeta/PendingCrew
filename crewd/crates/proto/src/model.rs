//! 领域模型：客户端看到的机组、session、消息、Todo。字段名即线上 JSON（camelCase）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `system.hello` 的结果（U1：版本、构建身份、能力）。新字段带 `default`：新客户端连老 crewd 也读得出来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub server: String,
    pub version: String,
    pub api_version: u32,
    /// 这个 crewd 还认的最老的客户端 API 版本（更老的客户端 hello 被拒：-32009 `system.api_incompatible`）。
    #[serde(default)]
    pub min_api_version: u32,
    #[serde(default)]
    pub build: BuildInfo,
    /// 这个 crewd 此刻开着的能力（客户端据此显示 / 隐藏功能）。没有会话管理（没配 agent 执行器）时是空的。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// S4: optional DB-independent fault history; old replies and no-fault replies omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_health: Option<DaemonHealth>,
}

/// Historical latest DB failure; absence does not assert that the daemon is healthy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHealth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_db_error: Option<DbFaultView>,
}

/// Payload-free projection of the store-owned fault snapshot. No SQL, paths or messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbFaultView {
    pub stage: DbFaultStageView,
    pub kind: DbFaultKindView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_kind: Option<DbStorageKindView>,
    /// Actual observation time, Unix epoch milliseconds; successful jobs do not erase it.
    pub at_ms: i64,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbFaultStageView {
    Enqueue,
    Receive,
    Job,
    Finish,
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbFaultKindView {
    DbClosed,
    DbJobPanicked,
    Sql,
    Storage,
    Integrity,
    Unclassified,
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbStorageKindView {
    DiskFull,
    Io,
    Corrupt,
    ReadOnly,
    CantOpen,
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

/// crewd 二进制的构建身份（U1 / S6）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// 构建时的 git 提交（12 位；拿不到 = `unknown`）。
    pub git_sha: String,
    /// 构建时工作区有改动。
    pub dirty: bool,
    /// 正在跑的可执行文件。
    pub exe: String,
    /// 那个文件此刻还是正在跑的这一个；false = 被更新换掉 / 删了，重启 crewd 才用上新版。
    pub exe_still_on_disk: bool,
}

/// `system.hello` 的参数（U1）。**故意不 `deny_unknown_fields`**：握手是以后的客户端加字段（能力协商等）的地方，
/// 老 crewd 必须收得下新客户端的 hello——这是契约里唯一不拒未知字段的参数。全可选：`null` 当 `{}`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("type" = ["object", "null"]))]
pub struct HelloParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientInfo>,
    /// 客户端说的 API 版本：比 `minApiVersion` 老 = 拒；比 crewd 新 = 收（客户端自己决定怎么降级）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<u32>,
    /// 客户端会的能力（留给以后协商；crewd 现在只记日志）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// 客户端是谁（crewd 每次 hello 记一行日志）。同样不拒未知字段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    /// 总机组（管理机组）：至多一个；不在机组树里（没有父、没有子），对全部机组有权限（PM 2026-10-07）。
    /// `crew.list` 把它排在最前。
    pub management: bool,
    pub parent_ids: Vec<String>,
    /// 这个机组的直系子机组，按侧栏顺序（每个父下各自一套次序，控制者裁定 C1；`crew.reorder {parentId}` 改它）。
    #[serde(default)]
    pub child_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 机长的执行器类型，词表同 `Session.executor`。
    pub captain_executor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// "human" | "captain" | "auto"（谁起的名；旧数据从 extra 回填）。
    pub title_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 机长写的一句状态（`set_crew_status`）。
    pub status_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 委派策略（M3b 的 read / update_delegation_policy），自由 JSON。
    pub policy: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 此刻的机长（裁定 8）。
    pub captain_session_id: Option<String>,
    /// 只表示顶层机组之间的侧栏次序（人排的；`crew.reorder` 不带 parentId 时改它，新的顶层机组排在末尾）。
    /// `parentIds` 非空时这个值没有意义（可能是变成子机组之前留下的旧值），客户端不要用它排序：
    /// 子机组在某个父下的次序看父的 `childIds`（多父机组在每个父下各有自己的次序）。总机组恒为 0，单独列出。
    #[serde(default)]
    pub position: i64,
    /// 移除时刻（`crew.remove`：隐藏并冻结、可恢复，N1 裁定 13 / PM 2026-10-07）；没移除时不出现。
    /// 库里的列仍叫 `crews.archived_at`（内部叫归档）；线上一律叫 removed，archived 只用于群。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub crew_id: String,
    pub role: Role,
    /// 执行器类型："claude" | "codex" | "acp:<registry-id>"；旧数据导入的还可能是 "terminal" | "unknown"。
    pub executor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    #[serde(default)]
    pub desired: Desired,
    /// 分机号（机长恒为 1，不存；没号的旧成员为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<i64>,
    /// crewd 为它新建的 git worktree 的绝对路径（控制者裁定 C8，`isolation: true`）；None = 共享机组目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default)]
    pub was_working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_at: Option<String>,
    /// 最近一次「产出」（工具调用 / 工作项变化；发言不算，控制者裁定 C3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_failed_at: Option<String>,
}

/// 模型类型对库里读出来的行宽松（多余的键忽略）；客户端参数用严格的 `MentionParam`。
// 不实现 `Default`：@ 谁必须由构造方写明（终审 9；守卫见 `MentionHasNoDefault`）。普通注释，不进契约。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    pub kind: MentionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

/// 模型类型对库里读出来的行宽松（多余的键忽略）；客户端参数用严格的 `AttachmentParam`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// 全库单调递增的序号；客户端用它增量拉取。
    pub seq: i64,
    pub id: String,
    /// 归属的群（spec §17）：大厅是 `<crewId>/lobby`。
    pub channel_id: String,
    pub sender_kind: SenderKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Ledger {
    #[default]
    Agent,
    Human,
}

impl Ledger {
    pub fn as_str(self) -> &'static str {
        match self {
            Ledger::Agent => "agent",
            Ledger::Human => "human",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoResponse {
    pub id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    pub crew_id: String,
    pub ledger: Ledger,
    /// 账内编号 #N；软删除的行也占号。
    pub number: i64,
    pub text: String,
    /// "pending" | "in_progress" | "completed" | "blocked_on_human" | "dropped"
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub responses: Vec<TodoResponse>,
}

/// 封闭词表：线上是固定字符串，`parse` 认不出就是 None（不猜）。
macro_rules! closed_vocab {
    ($name:ident { $($variant:ident = $s:literal),+ $(,)? } default $def:ident) => {
        closed_vocab!($name { $($variant = $s),+ });
        impl Default for $name {
            fn default() -> $name {
                $name::$def
            }
        }
    };
    ($name:ident { $($variant:ident = $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        pub enum $name {
            $(#[serde(rename = $s)] $variant,)+
        }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s,)+ }
            }
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

closed_vocab!(SenderKind { Human = "human", Session = "session", Captain = "captain", System = "system" } default Human);
// 没有缺省值（终审 9）：`Mention::default()` 曾静默等于 @人类——@ 谁必须写明。
closed_vocab!(MentionKind { Session = "session", Captain = "captain", Human = "human" });

/// 守卫（终审 9，只在 doctest 里存在）：`Mention` / `MentionKind` 没有 `Default`，下面两段编不过才对。
///
/// ```compile_fail
/// let _ = crewd_proto::Mention::default();
/// ```
/// ```compile_fail
/// let _ = crewd_proto::MentionKind::default();
/// ```
/// 对照（编得过）：同一个宏生成、带缺省值的词表。
/// ```
/// let _ = crewd_proto::SenderKind::default();
/// ```
#[cfg(doctest)]
pub struct MentionHasNoDefault;
closed_vocab!(Role { Captain = "captain", Worker = "worker", Admin = "admin" } default Worker);
closed_vocab!(Desired { Running = "running", Stopped = "stopped" } default Stopped);
closed_vocab!(ChannelKind { Lobby = "lobby", Human = "human", Topic = "topic", Dm = "dm" } default Topic);
closed_vocab!(AskStateView { Open = "open", Answered = "answered", Defaulted = "defaulted", Withdrawn = "withdrawn" } default Open);
closed_vocab!(WaitTierView { Fresh = "fresh", Pinned = "pinned", StillWanted = "still_wanted" } default Fresh);
closed_vocab!(DirectoryKind { Crew = "crew", Session = "session" } default Crew);
closed_vocab!(HealthKindView {
    AuthRequired = "auth_required", UsageLimit = "usage_limit", RateLimited = "rate_limited",
    LaunchFailed = "launch_failed", BriefUndelivered = "brief_undelivered", PromptUndelivered = "prompt_undelivered",
    TurnFailed = "turn_failed", VersionIncompatible = "version_incompatible",
} default AuthRequired);
closed_vocab!(RestartAnswer { Resume = "resume", Later = "later" } default Later);
// 重启询问为什么开（C26）：`restart` = crewd 上次没有经休眠就停了（崩了 / 被杀 / 普通停机）；`hibernate` = 人让它休眠的。
closed_vocab!(RestartReason { Restart = "restart", Hibernate = "hibernate" } default Restart);

/// 群成员 / 群主的引用。人类没有 id（全机只有一个「人」）。客户端参数用严格的 `MemberParam`。
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MemberRef {
    Session { id: String },
    Human,
}

impl MemberRef {
    pub fn session(id: impl Into<String>) -> MemberRef {
        MemberRef::Session { id: id.into() }
    }
    /// 库里的 (member_kind, member_id)；人类的 id 固定是 "human"。
    pub fn to_row(&self) -> (&'static str, &str) {
        match self {
            MemberRef::Session { id } => ("session", id.as_str()),
            MemberRef::Human => ("human", "human"),
        }
    }
    pub fn from_row(kind: &str, id: &str) -> Option<MemberRef> {
        match kind {
            "session" => Some(MemberRef::session(id)),
            "human" => Some(MemberRef::Human),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub crew_id: String,
    pub kind: ChannelKind,
    /// 大厅 / 人类群是空串（客户端按 kind 显示）。
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// "human" | "system" | session id。
    pub created_by: String,
    /// 群主（§22.7 Q2）。大厅 / 人类群 = 归属机组此刻的机长（现算；没有机长时 None）。topic / dm 存的是群主；
    /// 删机组时群主随之消失、群里又一个成员不剩，就没有群主（None），只能由机长 / 人 `force_owner`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MemberRef>,
    /// 待接受的群主转让对象。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_owner: Option<MemberRef>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSummary {
    pub channel: Channel,
    pub members: Vec<MemberRef>,
    /// 人类标了「我在注视」。
    pub watched: bool,
    pub unread_for_human: i64,
    /// 未读里 @人类 的条数（红色「叫你」标记，§17）。
    pub human_mentions: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Badge {
    pub channel_id: String,
    pub unread_for_human: i64,
    pub human_mentions: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewDetail {
    pub crew: Crew,
    /// 这个机组的人类群在 `<homeId>/human`；总机组 / 顶层机组 = 自己（多父归最早建的顶层祖先，§13 Q1 同口径）。
    pub home_id: String,
    pub children: Vec<String>,
    pub channels: Vec<ChannelSummary>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AskDefault {
    pub text: String,
    pub after_minutes: i64,
}

/// 一条人类待确认（H 号按机组发，§19 Q1）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub crew_id: String,
    /// "H3"
    pub ask: String,
    pub question: String,
    /// 提问的 session；None = 系统 / 人。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<String>,
    pub asked_at: String,
    pub state: AskStateView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// 回答 / 按默认办 / 撤回的时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdraw_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<AskDefault>,
    /// 关于哪个工作项（"W2"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_note: Option<String>,
    /// 人类群里那条公告。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HumanWait {
    pub ask: Ask,
    pub items: Vec<String>,
    pub owners: Vec<String>,
    pub waited_ms: i64,
    pub tier: WaitTierView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_due_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRow {
    /// "7" 或 "7-3"
    pub number: String,
    pub kind: DirectoryKind,
    pub name: String,
    pub crew_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 上级的路径：顶层机组在最左，" / " 分隔；总机组不在机组树里（总机组与顶层机组本身是空串）。
    pub org_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub kind: HealthKindView,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    #[serde(default)]
    pub fingerprint: String,
    /// Zero-based option index, never a menu digit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    #[serde(default)]
    pub kind: DecisionKindView,
    pub summary: String,
    pub options: Vec<String>,
    /// 选项带编号（按编号答）；false = 光标式菜单（绝不编造编号，§15）。
    pub numbered: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKindView {
    #[default]
    Terminal,
    RunnerRequest,
}

/// 会话的实时状态（`session.live` / `session.changed`，由 M3c 填）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionLive {
    pub session_id: String,
    pub running: bool,
    pub working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_decision: Option<DecisionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_question: Option<String>,
    /// 输入框里有一段人打的、还没发出的草稿，正挡着投递（M3c T10d）：草稿原文。不在 = 没有（或分不清，见下）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_draft: Option<String>,
    /// true = 有人在终端里敲过字、但分不清输入框里是草稿还是占位提示，投递照样挡着（T10d fix R3）。这时不给
    /// `inputDraft`：看得见的那行不一定是人的字。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub input_draft_unknown: bool,
}

/// 重启后「是否立即恢复这 N 个 session」（§22.7 Q1）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestartPrompt {
    pub id: i64,
    pub created_at: String,
    pub session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RestartAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// 为什么问（C26）：`hibernate` = 上次是人让 crewd 休眠的（客户端据此写「上次休眠了 N 个」）。
    #[serde(default)]
    pub reason: RestartReason,
    /// `reason = hibernate` 时：到了休眠期限还没做完、被打断的 session（恢复后要重跑被打断的命令）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupted: Vec<String>,
}

/// 空结果（`crew.purge` 等）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `crew.purged` 的数据。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewRef {
    pub crew_id: String,
}

/// `channel.purged` 的数据：群随机组的永久删除被级联删掉了（删了的群读不到，发不出 `channel.changed`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRef {
    pub channel_id: String,
}

/// `events.lagged`：漏掉的事件条数，客户端应全量重拉。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lagged {
    pub missed: u64,
}

/// `events.subscribe` 的回执。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subscribed {
    pub topics: Vec<String>,
}
