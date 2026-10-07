//! 领域模型：客户端看到的机组、session、消息、Todo。字段名即线上 JSON（camelCase）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub server: String,
    pub version: String,
    pub api_version: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    /// 组织唯一的根（总机组，spec §24 H2）；至多一个。
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
    /// 只有总机组用（恒为 1）；子机组的次序看父的 `childIds`（多父机组在每个父下各有自己的次序）。
    #[serde(default)]
    pub position: i64,
    /// 归档时刻（删机组 = 归档，N1 裁定 13）；没归档时不出现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
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
    /// 这个机组的人类群在 `<homeId>/human`；总机组 / 一级机组 = 自己（多父归最早建的一级祖先，§13 Q1 同口径）。
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
    /// 上级的路径：一级机组在最左，" / " 分隔；总机组是所有人的祖先、不进路径（总机组与一级机组本身是空串）。
    pub org_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub kind: HealthKindView,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    pub summary: String,
    pub options: Vec<String>,
    /// 选项带编号（按编号答）；false = 光标式菜单（绝不编造编号，§15）。
    pub numbered: bool,
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
}

/// 空结果（`crew.purge` 等）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `crew.removed` 的数据。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewRef {
    pub crew_id: String,
}

/// `channel.removed` 的数据：群随机组删除被级联删掉了（删了的群读不到，发不出 `channel.changed`）。
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
