//! crewd 线协议：客户端（Mac / iOS / CLI）与 crewd 之间的唯一契约。
//! 传输：每行一个 JSON（NDJSON）。客户端发 `Request`，服务端回 `ServerFrame`。
//!
//! **兼容规则**（S9 第 1 部分，U1）：
//! - 加可选字段、加方法、加事件、加枚举值 = 兼容：`API_VERSION` 不变。
//! - 删掉 / 改名 / 改含义（含把可选改成必填、收紧参数）= 不兼容：`API_VERSION` 加一，旧行为一直保留到
//!   `MIN_API_VERSION`；比它更老的客户端在 `system.hello` 就被拒（-32009 `system.api_incompatible`）。
//! - 服务端发给客户端的枚举，客户端必须容忍不认识的词（读成 unknown，不让整条解析失败）；M3c 的词表在 Rust 里就带一个
//!   不进契约的 `Unknown`（`orchestra`）。共享的 M3a 词表随 iOS 准备一起改。
//! - 客户端发给服务端的参数保持严格（`deny_unknown_fields`：拼错字段报错）；唯一的例外是 `system.hello` 的参数
//!   （握手是以后加字段、协商能力的地方）。
//! - 客户端在 hello 里报自己的名字 / 版本 / 构建与 `apiVersion`；crewd 回自己的 `apiVersion`、`minApiVersion`、构建身份与能力。
//! - v3 还没发布：v3 之内的改名不留别名。
pub mod commands;
pub mod judgments;
mod model;
pub mod notifications;
/// M3c（编排）的方法、事件与类型。
pub mod orchestra;
mod params;
pub mod work;
pub use work::*;
pub mod remote;
pub use remote::*;
pub mod enrollment;
pub use enrollment::*;
pub mod resources;
pub use resources::*;
pub mod knowledge_channels;
pub use knowledge_channels::*;
pub mod channel_work;
pub use channel_work::*;
pub mod todo_attributes;
pub use todo_attributes::*;
pub mod requirements;
pub use requirements::*;
pub mod work_patrol;
pub use work_patrol::*;
pub mod runner_usage;
pub use runner_usage::*;
/// 唤醒行的状态转移表（不是线协议的一部分；放这里是因为 collab 与 server 都依赖本 crate，C24）。
pub mod wake_lifecycle;

pub mod agent;
pub mod executors;
pub use model::*;
pub use params::*;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 2：`whiteboard.*` 删除、`Message.crewId` → `channelId`（裁定 18）。
/// v3：删审批、总机组 = 管理机组（不在机组树里）、@ 不再收窄（N1，spec §24）。
pub const API_VERSION: u32 = 3;
/// 这个 crewd 还认的最老的客户端 API 版本（U1；见模块文档的兼容规则）。
pub const MIN_API_VERSION: u32 = 3;

pub mod methods {
    pub const HELLO: &str = "system.hello";
    pub const REMOTE_STATUS: &str = "remote.status";
    pub const REMOTE_CONFIGURE: &str = "remote.configure";
    pub const REMOTE_ENROLLMENT_CREATE: &str = "remote.enrollmentCreate";
    pub const REMOTE_ENROLLMENT_LOOKUP: &str = "remote.enrollmentLookup";
    pub const REMOTE_INVITE: &str = "remote.invite";
    pub const REMOTE_ACCEPT: &str = "remote.accept";
    pub const REMOTE_GRANTS: &str = "remote.grants";
    pub const REMOTE_REVOKE: &str = "remote.revoke";
    pub const CREW_LIST: &str = "crew.list";
    pub const CREW_GET: &str = "crew.get";
    pub const CREW_CREATE: &str = "crew.create";
    pub const CREW_UPDATE: &str = "crew.update";
    pub const CREW_ADOPT: &str = "crew.adopt";
    pub const CREW_REORDER: &str = "crew.reorder";
    pub const CREW_RELEASE: &str = "crew.release";
    /// 移除机组（PM 2026-10-07）：隐藏并冻结（库里叫归档；线上 `Crew.removedAt`），不删任何东西、可恢复；
    /// 返回移除后的 `Crew`。
    pub const CREW_REMOVE: &str = "crew.remove";
    /// 恢复（`crew.remove` 的反操作），返回 `Crew`。
    pub const CREW_RESTORE: &str = "crew.restore";
    /// 永久删除：只收已移除的机组，真的删掉这个机组的一切；删之前拍一份 pre-delete 快照（N4），拍不成就不删；
    /// 快照只是安全网，30 天后随定时备份轮转删掉。
    pub const CREW_PURGE: &str = "crew.purge";
    pub const CHANNEL_LIST: &str = "channel.list";
    pub const CHANNEL_CREATE: &str = "channel.create";
    pub const CHANNEL_UPDATE: &str = "channel.update";
    pub const CHANNEL_ADD_MEMBERS: &str = "channel.addMembers";
    pub const CHANNEL_REMOVE_MEMBER: &str = "channel.removeMember";
    pub const CHANNEL_WATCH: &str = "channel.watch";
    pub const CHANNEL_SET_OWNER: &str = "channel.setOwner";
    pub const MESSAGE_LIST: &str = "message.list";
    pub const MESSAGE_POST: &str = "message.post";
    pub const MESSAGE_SEARCH: &str = "message.search";
    pub const READ_MARK: &str = "read.mark";
    pub const SESSION_USAGE: &str = "session.usage";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_GET: &str = "session.get";
    pub const TODO_ATTRIBUTES_GET: &str = "todo.attributesGet";
    pub const TODO_ASSIGN: &str = "todo.assign";
    pub const TODO_SET_STATE: &str = "todo.setState";
    pub const HUMAN_TASK_CREATE: &str = "humanTask.create";
    pub const CHANNEL_WORK_TAGS_SET: &str = "channel.workTagsSet";
    pub const RESOURCE_REQUEST_LOOKUP: &str = "resource.requestLookup";
    pub const DOCUMENT_CREATE: &str = "document.create";
    pub const DOCUMENT_GET: &str = "document.get";
    pub const DOCUMENT_LIST: &str = "document.list";
    pub const DOCUMENT_UPDATE: &str = "document.update";
    pub const DOCUMENT_DELETE: &str = "document.delete";
    pub const CHANNEL_KNOWLEDGE_LIST: &str = "channel.knowledgeList";
    pub const CHANNEL_KNOWLEDGE_LINK: &str = "channel.knowledgeLink";
    pub const CHANNEL_KNOWLEDGE_UNLINK: &str = "channel.knowledgeUnlink";
    pub const CHANNEL_WORK_LIST: &str = "channel.workList";
    pub const CHANNEL_WORK_LINK: &str = "channel.workLink";
    pub const CHANNEL_WORK_UNLINK: &str = "channel.workUnlink";
    pub const FOLDER_LIST: &str = "viewFolder.list";
    pub const FOLDER_CREATE: &str = "viewFolder.create";
    pub const FOLDER_UPDATE: &str = "viewFolder.update";
    pub const FOLDER_DELETE: &str = "viewFolder.delete";
    pub const FOLDER_ASSIGN: &str = "viewFolder.assign";
    pub const WORK_CREATE: &str = "work.create";
    pub const WORK_UPDATE: &str = "work.update";
    pub const WORK_BLOCK: &str = "work.block";
    pub const WORK_TRANSFER_REQUEST: &str = "work.transferRequest";
    pub const WORK_TRANSFER_ACCEPT: &str = "work.transferAccept";
    pub const WORK_TRANSFER_WITHDRAW: &str = "work.transferWithdraw";
    pub const WORK_ASSIGN: &str = "work.assign";
    pub const WORK_REASSIGN_LOST: &str = "work.reassignLost";
    pub const WORK_HISTORY: &str = "work.history";
    pub const WORK_PATROL: &str = "work.patrol";
    pub const WORK_LIST: &str = "work.list";
    pub const WORK_GET: &str = "work.get";
    pub const ASK_GET: &str = "ask.get";
    pub const ASK_LIST: &str = "ask.list";
    pub const ASK_ANSWER: &str = "ask.answer";
    pub const ASK_WITHDRAW: &str = "ask.withdraw";
    pub const RESTART_PENDING: &str = "restart.pending";
    pub const RESTART_ANSWER: &str = "restart.answer";
    pub const DIRECTORY_LIST: &str = "directory.list";
    pub const TODO_CREATE: &str = "todo.create";
    pub const TODO_GET: &str = "todo.get";
    pub const TODO_CLOSE: &str = "todo.close";
    pub const TODO_LIST: &str = "todo.list";
    pub const SUBSCRIBE: &str = "events.subscribe";
    // M3b / M3c 的方法常量写在各自的段落里，并加进 ALL（契约 json 由后合者重新生成）。

    /// 客户端可调的全部方法（契约测试与 `crewd rpc` 端到端都按它逐个核）。
    const CORE_METHODS: &[&str] = &[
        crate::judgments::SETTINGS_GET,
        crate::judgments::SETTINGS_SET,
        crate::judgments::INSPECT,
        crate::commands::LOCAL_CLIENT_TRUST_STATUS,
        crate::commands::COMMAND_SUBMIT,
        crate::commands::COMMAND_LOOKUP,
        crate::commands::COMMAND_LIST_UNKNOWN,
        crate::commands::SESSION_CONTROL_TARGET,
        crate::codex::INSPECT,
        crate::codex::HISTORY,
        crate::codex::MODELS,
        crate::codex::TOOLS,
        crate::commands::SESSION_TERMINAL_INSPECT,
        crate::commands::SESSION_TERMINAL_WATCH,
        HELLO,
        REMOTE_STATUS,
        REMOTE_CONFIGURE,
        REMOTE_ENROLLMENT_CREATE,
        REMOTE_ENROLLMENT_LOOKUP,
        REMOTE_INVITE,
        REMOTE_ACCEPT,
        REMOTE_GRANTS,
        REMOTE_REVOKE,
        CREW_LIST,
        CREW_GET,
        CREW_CREATE,
        CREW_UPDATE,
        CREW_ADOPT,
        CREW_REORDER,
        CREW_RELEASE,
        CREW_REMOVE,
        CREW_RESTORE,
        CREW_PURGE,
        CHANNEL_LIST,
        CHANNEL_CREATE,
        CHANNEL_UPDATE,
        CHANNEL_ADD_MEMBERS,
        CHANNEL_REMOVE_MEMBER,
        CHANNEL_WATCH,
        CHANNEL_SET_OWNER,
        MESSAGE_LIST,
        MESSAGE_POST,
        MESSAGE_SEARCH,
        READ_MARK,
        SESSION_USAGE,
        SESSION_LIST,
        SESSION_GET,
        TODO_ATTRIBUTES_GET,
        TODO_ASSIGN,
        TODO_SET_STATE,
        HUMAN_TASK_CREATE,
        CHANNEL_WORK_TAGS_SET,
        RESOURCE_REQUEST_LOOKUP,
        CHANNEL_KNOWLEDGE_LIST,
        CHANNEL_KNOWLEDGE_LINK,
        CHANNEL_KNOWLEDGE_UNLINK,
        DOCUMENT_CREATE,
        DOCUMENT_GET,
        DOCUMENT_LIST,
        DOCUMENT_UPDATE,
        DOCUMENT_DELETE,
        CHANNEL_WORK_LIST,
        CHANNEL_WORK_LINK,
        CHANNEL_WORK_UNLINK,
        FOLDER_LIST,
        FOLDER_CREATE,
        FOLDER_UPDATE,
        FOLDER_DELETE,
        FOLDER_ASSIGN,
        WORK_CREATE,
        WORK_UPDATE,
        WORK_BLOCK,
        WORK_TRANSFER_REQUEST,
        WORK_TRANSFER_ACCEPT,
        WORK_TRANSFER_WITHDRAW,
        WORK_ASSIGN,
        WORK_REASSIGN_LOST,
        WORK_HISTORY,
        WORK_PATROL,
        WORK_LIST,
        WORK_GET,
        ASK_GET,
        ASK_LIST,
        ASK_ANSWER,
        ASK_WITHDRAW,
        RESTART_PENDING,
        RESTART_ANSWER,
        DIRECTORY_LIST,
        TODO_CREATE,
        TODO_GET,
        TODO_CLOSE,
        TODO_LIST,
        SUBSCRIBE,
        // M3c（编排，`crate::orchestra::methods`）。
        crate::orchestra::methods::SESSION_START,
        crate::orchestra::methods::SESSION_STOP,
        crate::orchestra::methods::SESSION_RESUME,
        crate::orchestra::methods::SESSION_LIVE,
        crate::orchestra::methods::SESSION_INSPECT,
        crate::orchestra::methods::SESSION_ANSWER,
        crate::orchestra::methods::SESSION_RESPOND_REQUEST,
        crate::orchestra::methods::SESSION_INTERRUPT,
        crate::orchestra::methods::DECISION_LIST,
        crate::orchestra::methods::CREW_SET_CAPTAIN,
        crate::orchestra::methods::WAKE_LIST,
        crate::orchestra::methods::LOOP_LIST,
        crate::orchestra::methods::LOOP_RECOVER,
        crate::orchestra::methods::SETTINGS_GET,
        crate::orchestra::methods::SETTINGS_SET,
        crate::orchestra::methods::HIBERNATE,
        crate::orchestra::methods::MESSAGE_RECEIPTS,
    ];
    /// Compose extension registries rather than maintaining a second method list.
    pub static ALL: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        CORE_METHODS
            .iter()
            .copied()
            .chain(crate::team_data_api::METHODS.iter().copied())
            .chain(crate::lifecycle::METHODS.iter().copied())
            .chain(std::iter::once(crate::executors::EXECUTOR_LIST))
            .collect()
    });
}

pub mod events {
    pub const TODO_ATTRIBUTES_CHANGED: &str = "todo.attributesChanged";
    /// `events.subscribe` 接受的主题（= 事件名第一个点之前的部分）；其他一律 -32602。
    /// `session` 与 `wake`（排障用的 `wake.changed`，常量在 M3c 的 `proto::orchestra::events`）主题的事件由 M3c 发；
    /// 主题名在这里一次列全，M3c 不改 conn.rs / 这张表。
    pub const TOPICS: &[&str] = &[
        "message",
        "channel",
        "crew",
        "badge",
        "ask",
        "session",
        "restart",
        "wake",
        "system",
        "todo",
        "work",
        "documents",
        "viewFolders",
        "data",
    ];
    pub const SYSTEM_SHUTDOWN: &str = "system.shutdown";
    pub const MESSAGE_APPENDED: &str = "message.appended";
    pub const CHANNEL_CHANGED: &str = "channel.changed";
    /// 群随机组的永久删除（`crew.purge`）被级联删掉（`{channelId}`）。不叫 `*.removed`：
    /// 「移除」（`crew.remove`）是可逆的归档，发的是 `channel.changed`（PM 2026-10-07）。
    pub const CHANNEL_PURGED: &str = "channel.purged";
    pub const CREW_CHANGED: &str = "crew.changed";
    /// 机组被永久删除（`crew.purge`，`{crewId}`）；移除（归档）发的是 `crew.changed`。
    pub const CREW_PURGED: &str = "crew.purged";
    pub const BADGE_CHANGED: &str = "badge.changed";
    pub const WORK_CHANGED: &str = "work.changed";
    pub const CHANNEL_WORK_CHANGED: &str = "channel.workChanged";
    pub const DATA_CHANGED: &str = "data.changed";
    pub const DOCUMENT_CHANGED: &str = "document.changed";
    pub const FOLDER_CHANGED: &str = "viewFolder.changed";
    pub const ASK_CHANGED: &str = "ask.changed";
    /// `SessionLive`，M3c 发。
    pub const SESSION_CHANGED: &str = "session.changed";
    pub const RESTART_CHANGED: &str = "restart.changed";
    /// 订阅者处理太慢、漏掉了事件；`data.missed` 是漏掉的条数，客户端应全量重拉。
    pub const LAGGED: &str = "events.lagged";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    /// 客户端必须从 1 开始编号：0 保留给「请求无法解析」时服务端回的错误响应。
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    /// 规则拒绝的稳定码（N1 Task 10）：客户端按码本地化，`message` 只是中文缺省。只加不改——老客户端忽略它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<RpcErrorData>,
}

/// `RpcError.data`：规则拒绝带的机器可读部分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RpcErrorData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_todo_attributes: Option<Box<TodoAttributes>>,
    /// 拒绝码，如 `channel.dm_cannot_grow`（全集见 crewd-collab 的 `refusal::Code`）。
    pub refusal: String,
    /// `decision.changed`：此刻屏上的那个框（没有框了 = 不在）。客户端据它重新问人（M3c T10d）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Box<DecisionView>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_work: Option<Box<WorkConflictFacts>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_document: Option<Box<DocumentSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_link_revision: Option<Box<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_source_revision: Option<Box<String>>,
}

impl RpcError {
    pub const PARSE: i32 = -32700;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL: i32 = -32603;
    /// 规则不允许（不是参数写错：换个身份 / 时机就行）。
    pub const FORBIDDEN: i32 = -32003;
    /// 引用的东西不存在（机组、群、消息、session、H 号、重启询问……）。
    pub const NOT_FOUND: i32 = -32004;
    /// 与当前状态冲突（已经答过 / 撤回的问、还有在跑 session 的机组……）。
    pub const CONFLICT: i32 = -32009;
    /// 存储故障（磁盘满、I/O 错、库损坏、只读、打不开）：不是这次请求的错，重试多半没用，要人处理（N4 / S26）。
    pub const STORAGE: i32 = -32010;

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// 带上规则拒绝码（`data.refusal`）。
    pub fn with_refusal(mut self, code: &str) -> Self {
        self.data = Some(RpcErrorData {
            current_todo_attributes: None,
            refusal: code.to_string(),
            decision: None,
            current_work: None,
            current_document: None,
            current_tree_revision: None,
            current_link_revision: None,
            current_source_revision: None,
        });
        self
    }

    /// 附上此刻屏上的框（`data.decision`；先 [`Self::with_refusal`]）。
    pub fn with_decision(mut self, decision: DecisionView) -> Self {
        if let Some(d) = self.data.as_mut() {
            d.decision = Some(Box::new(decision));
        }
        self
    }
}

/// 对一条请求的回复：恰好带 `result` 或 `error` 之一（控制者裁定 g）。`"result": null` 是「成功、值为 null」
/// （例如 `restart.pending` 没有待答询问），与「没有 result」不同；两样都没有 / 都有的帧解析失败。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// 回复的内容：线上是 `"result": <值>` 或 `"error": {code, message}` 这一个键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// 成功；类型见方法表里这个方法的 `result`（可以是 null）。
    Result(Value),
    Error(RpcError),
}

/// 手写：派生的反序列化会把 `"result": null` 读成「没有 result」，也查不出「两样都没有 / 都有」。
impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        /// 键在 = Some（哪怕值是 null）；键不在 = None（`#[serde(default)]`）。
        fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
            Value::deserialize(d).map(Some)
        }
        #[derive(Deserialize)]
        struct Wire {
            id: u64,
            #[serde(default, deserialize_with = "present")]
            result: Option<Value>,
            #[serde(default)]
            error: Option<RpcError>,
        }
        let w = Wire::deserialize(d)?;
        let outcome = match (w.result, w.error) {
            (Some(v), None) => Outcome::Result(v),
            (None, Some(e)) => Outcome::Error(e),
            (None, None) => return Err(D::Error::custom("回复既没有 result 也没有 error")),
            (Some(_), Some(_)) => return Err(D::Error::custom("回复同时带了 result 和 error")),
        };
        Ok(Response { id: w.id, outcome })
    }
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            outcome: Outcome::Result(result),
        }
    }
    pub fn err(id: u64, error: RpcError) -> Self {
        Self {
            id,
            outcome: Outcome::Error(error),
        }
    }
    pub fn into_result(self) -> Result<Value, RpcError> {
        match self.outcome {
            Outcome::Result(v) => Ok(v),
            Outcome::Error(e) => Err(e),
        }
    }
}

/// 服务端推的事件。`data` 的类型由 `event` 决定（见 `event_payloads`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SystemShutdown {
    pub reason: ShutdownReason,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    Stop,
    Hibernate,
}

/// 每个事件名 → 它的 `data` 类型。契约的 `events` 表与 `Event` 的 schema 都从这一张表来。
fn event_payloads(g: &mut schemars::SchemaGenerator) -> Vec<(&'static str, Value)> {
    vec![
        (
            commands::TERMINAL_OUTPUT_EVENT,
            sub::<commands::TerminalWatchEvent>(g),
        ),
        (events::SYSTEM_SHUTDOWN, sub::<SystemShutdown>(g)),
        (events::MESSAGE_APPENDED, sub::<Message>(g)),
        (events::CHANNEL_CHANGED, sub::<ChannelSummary>(g)),
        (events::CHANNEL_PURGED, sub::<ChannelRef>(g)),
        (events::CREW_CHANGED, sub::<Crew>(g)),
        (events::CREW_PURGED, sub::<CrewRef>(g)),
        (events::BADGE_CHANGED, sub::<Badge>(g)),
        (
            events::TODO_ATTRIBUTES_CHANGED,
            sub::<TodoAttributesChanged>(g),
        ),
        (events::ASK_CHANGED, sub::<Ask>(g)),
        (events::WORK_CHANGED, sub::<WorkChanged>(g)),
        (events::CHANNEL_WORK_CHANGED, sub::<ChannelWorkChanged>(g)),
        (events::DATA_CHANGED, sub::<CrewRef>(g)),
        (events::DOCUMENT_CHANGED, sub::<DocumentChanged>(g)),
        (events::FOLDER_CHANGED, sub::<FolderChanged>(g)),
        (events::SESSION_CHANGED, sub::<SessionLive>(g)),
        (events::RESTART_CHANGED, sub::<RestartPrompt>(g)),
        (events::LAGGED, sub::<Lagged>(g)),
    ]
    .into_iter()
    .chain(orchestra::schema_entries(g).1)
    .collect()
}

/// 手写：按 `event` 区分的 oneOf（每个事件一支：`event` 是常量、`data` 指向负载类型），Swift 端可直接生成
/// 带关联值的枚举（Task 14 复审裁定 2）。
impl JsonSchema for Event {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Event".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let branches: Vec<Value> = event_payloads(g)
            .into_iter()
            .map(|(name, data)| {
                json!({
                    "type": "object",
                    "properties": {
                        "event": { "type": "string", "const": name },
                        "data": data,
                    },
                    "required": ["event", "data"],
                })
            })
            .collect();
        schemars::json_schema!({
            "description": "服务端推的事件；按 `event` 区分，`data` 的类型随之确定。",
            "oneOf": branches,
        })
    }
}

/// 服务端发的一行：回复或事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerFrame {
    Response(Response),
    Event(Event),
}

/// 手写：两者互斥（回复有 `id`、事件有 `event`），契约写成 oneOf（派生的 untagged 是 anyOf）。
impl JsonSchema for ServerFrame {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ServerFrame".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let response = sub::<Response>(g);
        let event = sub::<Event>(g);
        schemars::json_schema!({
            "description": "服务端发的一行：对某个请求的回复，或订阅的事件。",
            "oneOf": [response, event],
        })
    }
}

/// params 只能是对象（或 `null`，交给目标类型决定收不收）。数组必须在这里拦：serde 会把它按字段顺序读成结构体
/// （`["c"]` → `{crewId:"c"}`），契约里没有这种写法（控制者裁定 b）。
pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    if !(params.is_object() || params.is_null()) {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "参数无效: params 必须是对象或 null",
        ));
    }
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("参数无效: {e}")))
}

/// 参数全可选的方法：`null` 当 `{}`。
pub fn parse_params_or_default<T: DeserializeOwned + Default>(
    params: Value,
) -> Result<T, RpcError> {
    if params.is_null() {
        Ok(T::default())
    } else {
        parse_params(params)
    }
}

/// 无参数的方法：只收 `null` 或 `{}`。
pub fn parse_no_params(params: Value) -> Result<(), RpcError> {
    parse_params_or_default::<NoParams>(params).map(|_| ())
}

fn sub<T: JsonSchema>(g: &mut schemars::SchemaGenerator) -> Value {
    serde_json::to_value(g.subschema_for::<T>()).expect("schema 可序列化")
}

fn many(v: Value) -> Value {
    json!({ "type": "array", "items": v })
}

fn nullable(v: Value) -> Value {
    json!({ "anyOf": [v, { "type": "null" }] })
}

fn m(params: Value, result: Value) -> Value {
    json!({ "params": params, "result": result })
}

/// 整份 API 契约（`crewd schema` 打印它；仓库里提交一份，测试保证两者一致）。
/// 全部类型只在顶层 `$defs` 出现一次；方法 / 事件 / 帧都是 `$ref`，Swift 端可以直接据此生成代码。
/// 参数类型是 `object | null` 的方法 = 解析器也收 `null`（R11：契约与解析器一致，见 params.rs）。
pub fn api_schema() -> Value {
    // R8：`gen` 在 edition 2024 是保留字。
    let mut generator = schemars::SchemaGenerator::default();
    let g = &mut generator;
    let _ = sub::<notifications::NotificationOpenContext>(g);
    // json! 一次展开 32 个方法会撞宏递归上限：先列成表再收成对象（输出形状不变）。
    let methods: serde_json::Map<String, Value> = [
        (
            "notification.status",
            m(sub::<Empty>(g), sub::<notifications::NotificationStatus>(g)),
        ),
        (
            "notification.configure",
            m(
                sub::<notifications::NotificationConfigureParams>(g),
                sub::<notifications::NotificationRequestLookup>(g),
            ),
        ),
        (
            "notification.installationRegister",
            m(
                sub::<notifications::NotificationRegisterParams>(g),
                sub::<notifications::NotificationRequestLookup>(g),
            ),
        ),
        (
            "notification.installationRevoke",
            m(
                sub::<notifications::NotificationRevokeParams>(g),
                sub::<notifications::NotificationRequestLookup>(g),
            ),
        ),
        (
            "notification.installationGet",
            m(
                sub::<notifications::NotificationGetParams>(g),
                sub::<Option<notifications::NotificationInstallation>>(g),
            ),
        ),
        (
            "notification.requestLookup",
            m(
                sub::<notifications::NotificationLookupParams>(g),
                sub::<notifications::NotificationRequestLookup>(g),
            ),
        ),
        (
            "notification.deliveryGet",
            m(
                sub::<notifications::NotificationDeliveryGetParams>(g),
                sub::<Option<notifications::NotificationDelivery>>(g),
            ),
        ),
        (
            "notification.deliveryList",
            m(
                sub::<notifications::NotificationDeliveryListParams>(g),
                sub::<notifications::NotificationDeliveryPage>(g),
            ),
        ),
        (
            commands::LOCAL_CLIENT_TRUST_STATUS,
            m(
                sub::<commands::LocalClientTrustStatusParams>(g),
                sub::<commands::LocalClientTrustStatus>(g),
            ),
        ),
        (
            commands::COMMAND_SUBMIT,
            m(
                sub::<commands::CommandSubmitParams>(g),
                sub::<commands::PublicCommandReceipt>(g),
            ),
        ),
        (
            commands::COMMAND_LOOKUP,
            m(
                sub::<commands::CommandLookupParams>(g),
                sub::<Option<commands::PublicCommandReceipt>>(g),
            ),
        ),
        (
            commands::COMMAND_LIST_UNKNOWN,
            m(
                sub::<commands::CommandListUnknownParams>(g),
                sub::<commands::CommandUnknownPage>(g),
            ),
        ),
        (
            commands::SESSION_CONTROL_TARGET,
            m(
                sub::<commands::TerminalInspectParams>(g),
                sub::<commands::CommandRuntimeTarget>(g),
            ),
        ),
        (
            commands::SESSION_TERMINAL_INSPECT,
            m(
                sub::<commands::TerminalInspectParams>(g),
                sub::<commands::TerminalInspection>(g),
            ),
        ),
        (
            commands::SESSION_TERMINAL_WATCH,
            m(
                sub::<commands::TerminalWatchParams>(g),
                sub::<commands::TerminalWatchStarted>(g),
            ),
        ),
        (
            judgments::SETTINGS_GET,
            m(
                sub::<Option<judgments::JudgmentSettingsGet>>(g),
                sub::<judgments::JudgmentSettingsResponse>(g),
            ),
        ),
        (
            judgments::SETTINGS_SET,
            m(
                sub::<judgments::JudgmentSettingsSet>(g),
                sub::<judgments::JudgmentSettingsResult>(g),
            ),
        ),
        (
            judgments::INSPECT,
            m(
                sub::<judgments::JudgmentInspect>(g),
                sub::<judgments::JudgmentInspection>(g),
            ),
        ),
        (methods::HELLO, m(sub::<HelloParams>(g), sub::<Hello>(g))),
        (
            methods::REMOTE_STATUS,
            m(sub::<NoParams>(g), sub::<RemoteStatus>(g)),
        ),
        (
            methods::REMOTE_CONFIGURE,
            m(
                sub::<RemoteConfigureParams>(g),
                sub::<RemoteConfiguration>(g),
            ),
        ),
        (
            methods::REMOTE_ENROLLMENT_CREATE,
            m(
                sub::<HostEnrollmentCreateParams>(g),
                sub::<HostEnrollmentCreated>(g),
            ),
        ),
        (
            methods::REMOTE_ENROLLMENT_LOOKUP,
            m(
                sub::<HostEnrollmentLookupParams>(g),
                sub::<HostEnrollmentLookupResult>(g),
            ),
        ),
        (
            methods::REMOTE_INVITE,
            m(sub::<RemoteInviteParams>(g), sub::<RemoteSignedEnvelope>(g)),
        ),
        (
            methods::REMOTE_ACCEPT,
            m(sub::<RemoteAcceptParams>(g), sub::<RemoteAcceptResult>(g)),
        ),
        (
            methods::REMOTE_GRANTS,
            m(sub::<NoParams>(g), many(sub::<RemoteGrantSummary>(g))),
        ),
        (
            methods::REMOTE_REVOKE,
            m(sub::<RemoteRevokeParams>(g), sub::<RemoteGrantSummary>(g)),
        ),
        (
            methods::CREW_LIST,
            m(sub::<CrewListParams>(g), many(sub::<Crew>(g))),
        ),
        (
            methods::CREW_GET,
            m(sub::<CrewIdParams>(g), sub::<CrewDetail>(g)),
        ),
        (
            methods::CREW_CREATE,
            m(sub::<CrewCreateParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_UPDATE,
            m(sub::<CrewUpdateParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_ADOPT,
            m(sub::<CrewAdoptParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_REORDER,
            m(sub::<CrewReorderParams>(g), many(sub::<Crew>(g))),
        ),
        (
            methods::CREW_RELEASE,
            m(sub::<CrewReleaseParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_REMOVE,
            m(sub::<CrewIdParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_RESTORE,
            m(sub::<CrewIdParams>(g), sub::<Crew>(g)),
        ),
        (
            methods::CREW_PURGE,
            m(sub::<CrewIdParams>(g), sub::<Empty>(g)),
        ),
        (
            methods::CHANNEL_LIST,
            m(sub::<CrewIdParams>(g), many(sub::<ChannelSummary>(g))),
        ),
        (
            methods::CHANNEL_CREATE,
            m(sub::<ChannelCreateParams>(g), sub::<Channel>(g)),
        ),
        (
            methods::CHANNEL_UPDATE,
            m(sub::<ChannelUpdateParams>(g), sub::<Channel>(g)),
        ),
        (
            methods::CHANNEL_ADD_MEMBERS,
            m(sub::<ChannelMembersParams>(g), sub::<Channel>(g)),
        ),
        (
            methods::CHANNEL_REMOVE_MEMBER,
            m(sub::<ChannelMemberParams>(g), sub::<Channel>(g)),
        ),
        (
            methods::CHANNEL_WATCH,
            m(sub::<ChannelWatchParams>(g), sub::<ChannelSummary>(g)),
        ),
        (
            methods::CHANNEL_SET_OWNER,
            m(sub::<ChannelSetOwnerParams>(g), sub::<Channel>(g)),
        ),
        (
            methods::MESSAGE_LIST,
            m(sub::<MessageListParams>(g), many(sub::<Message>(g))),
        ),
        (
            methods::MESSAGE_POST,
            m(sub::<MessagePostParams>(g), sub::<Message>(g)),
        ),
        (
            methods::MESSAGE_SEARCH,
            m(sub::<MessageSearchParams>(g), many(sub::<Message>(g))),
        ),
        (
            methods::READ_MARK,
            m(sub::<ReadMarkParams>(g), sub::<ChannelSummary>(g)),
        ),
        (
            methods::SESSION_LIST,
            m(sub::<SessionListParams>(g), many(sub::<Session>(g))),
        ),
        (
            methods::SESSION_GET,
            m(sub::<SessionGetParams>(g), sub::<Session>(g)),
        ),
        (
            methods::TODO_ATTRIBUTES_GET,
            m(sub::<TodoAttributesGetParams>(g), sub::<TodoAttributes>(g)),
        ),
        (
            methods::TODO_ASSIGN,
            m(sub::<TodoAssignParams>(g), sub::<TodoAttributes>(g)),
        ),
        (
            methods::TODO_SET_STATE,
            m(sub::<TodoSetStateParams>(g), sub::<TodoAttributes>(g)),
        ),
        (
            methods::HUMAN_TASK_CREATE,
            m(
                sub::<HumanTaskCreateParams>(g),
                sub::<HumanTaskCreateResult>(g),
            ),
        ),
        (
            methods::CHANNEL_WORK_TAGS_SET,
            m(
                sub::<ChannelWorkTagsSetParams>(g),
                sub::<ChannelWorkTagsReceipt>(g),
            ),
        ),
        (
            methods::RESOURCE_REQUEST_LOOKUP,
            m(
                sub::<ResourceRequestLookupParams>(g),
                sub::<ResourceRequestLookupResult>(g),
            ),
        ),
        (
            methods::CHANNEL_KNOWLEDGE_LIST,
            m(
                sub::<ChannelKnowledgeListParams>(g),
                sub::<ChannelKnowledgePage>(g),
            ),
        ),
        (
            methods::CHANNEL_KNOWLEDGE_LINK,
            m(
                sub::<ChannelKnowledgeLinkParams>(g),
                sub::<ChannelKnowledgeReceipt>(g),
            ),
        ),
        (
            methods::CHANNEL_KNOWLEDGE_UNLINK,
            m(
                sub::<ChannelKnowledgeLinkParams>(g),
                sub::<ChannelKnowledgeReceipt>(g),
            ),
        ),
        (
            methods::DOCUMENT_CREATE,
            m(
                sub::<DocumentCreateParams>(g),
                sub::<DocumentCreateResult>(g),
            ),
        ),
        (
            methods::DOCUMENT_GET,
            m(sub::<DocumentGetParams>(g), sub::<DocumentDetail>(g)),
        ),
        (
            methods::DOCUMENT_LIST,
            m(sub::<DocumentListParams>(g), sub::<DocumentPage>(g)),
        ),
        (
            methods::DOCUMENT_UPDATE,
            m(sub::<DocumentUpdateParams>(g), sub::<DocumentSummary>(g)),
        ),
        (
            methods::DOCUMENT_DELETE,
            m(sub::<DocumentDeleteParams>(g), sub::<DocumentSummary>(g)),
        ),
        (
            methods::CHANNEL_WORK_LIST,
            m(sub::<ChannelWorkListParams>(g), sub::<ChannelWorkPage>(g)),
        ),
        (
            methods::CHANNEL_WORK_LINK,
            m(
                sub::<ChannelWorkLinkParams>(g),
                sub::<ChannelWorkLinkReceipt>(g),
            ),
        ),
        (
            methods::CHANNEL_WORK_UNLINK,
            m(
                sub::<ChannelWorkLinkParams>(g),
                sub::<ChannelWorkLinkReceipt>(g),
            ),
        ),
        (
            methods::FOLDER_LIST,
            m(sub::<FolderListParams>(g), sub::<FolderSnapshot>(g)),
        ),
        (
            methods::FOLDER_CREATE,
            m(sub::<FolderCreateParams>(g), sub::<FolderSnapshot>(g)),
        ),
        (
            methods::FOLDER_UPDATE,
            m(sub::<FolderUpdateParams>(g), sub::<FolderSnapshot>(g)),
        ),
        (
            methods::FOLDER_DELETE,
            m(sub::<FolderDeleteParams>(g), sub::<FolderSnapshot>(g)),
        ),
        (
            methods::FOLDER_ASSIGN,
            m(sub::<FolderAssignParams>(g), sub::<FolderSnapshot>(g)),
        ),
        (
            methods::WORK_CREATE,
            m(sub::<WorkCreateParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_UPDATE,
            m(sub::<WorkUpdateParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_BLOCK,
            m(sub::<WorkBlockParams>(g), sub::<WorkBlockReply>(g)),
        ),
        (
            methods::WORK_TRANSFER_REQUEST,
            m(sub::<WorkTransferRequestParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_TRANSFER_ACCEPT,
            m(sub::<WorkTransferResolveParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_TRANSFER_WITHDRAW,
            m(sub::<WorkTransferResolveParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_REASSIGN_LOST,
            m(sub::<WorkAssignParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_ASSIGN,
            m(sub::<WorkAssignParams>(g), sub::<WorkPlan>(g)),
        ),
        (
            methods::WORK_HISTORY,
            m(sub::<WorkHistoryParams>(g), sub::<WorkHistoryPage>(g)),
        ),
        (
            codex::INSPECT,
            m(
                sub::<orchestra::SessionIdParams>(g),
                sub::<codex::CodexSessionInspection>(g),
            ),
        ),
        (
            codex::HISTORY,
            m(
                sub::<codex::CodexHistoryParams>(g),
                sub::<codex::CodexHistoryPage>(g),
            ),
        ),
        (
            codex::MODELS,
            m(
                sub::<orchestra::SessionIdParams>(g),
                sub::<codex::CodexModels>(g),
            ),
        ),
        (
            codex::TOOLS,
            m(
                sub::<orchestra::SessionIdParams>(g),
                sub::<codex::CodexTools>(g),
            ),
        ),
        (
            methods::SESSION_USAGE,
            m(sub::<RunnerUsageParams>(g), sub::<RunnerUsagePage>(g)),
        ),
        (
            methods::WORK_PATROL,
            m(sub::<WorkPatrolParams>(g), sub::<WorkPatrolPage>(g)),
        ),
        (
            methods::WORK_LIST,
            m(sub::<WorkListParams>(g), sub::<WorkPage>(g)),
        ),
        (
            methods::WORK_GET,
            m(sub::<WorkGetParams>(g), sub::<WorkPlan>(g)),
        ),
        (methods::ASK_GET, m(sub::<AskGetParams>(g), sub::<Ask>(g))),
        (
            methods::ASK_LIST,
            m(sub::<AskListParams>(g), many(sub::<HumanWait>(g))),
        ),
        (
            methods::ASK_ANSWER,
            m(sub::<AskAnswerParams>(g), sub::<Ask>(g)),
        ),
        (
            methods::ASK_WITHDRAW,
            m(sub::<AskWithdrawParams>(g), sub::<Ask>(g)),
        ),
        (
            methods::RESTART_PENDING,
            m(sub::<NoParams>(g), nullable(sub::<RestartPrompt>(g))),
        ),
        (
            methods::RESTART_ANSWER,
            m(sub::<RestartAnswerParams>(g), sub::<RestartPrompt>(g)),
        ),
        (
            methods::DIRECTORY_LIST,
            m(sub::<DirectoryListParams>(g), many(sub::<DirectoryRow>(g))),
        ),
        (
            methods::TODO_CREATE,
            m(sub::<TodoCreateParams>(g), sub::<RequirementDetail>(g)),
        ),
        (
            methods::TODO_GET,
            m(sub::<TodoGetParams>(g), sub::<RequirementDetail>(g)),
        ),
        (
            methods::TODO_CLOSE,
            m(sub::<TodoCloseParams>(g), sub::<RequirementDetail>(g)),
        ),
        (
            methods::TODO_LIST,
            m(sub::<TodoListParams>(g), many(sub::<Todo>(g))),
        ),
        (
            methods::SUBSCRIBE,
            m(sub::<SubscribeParams>(g), sub::<Subscribed>(g)),
        ),
    ]
    .into_iter()
    .chain(orchestra::schema_entries(g).0)
    .chain(team_data_api::schema_entries(g))
    .chain(lifecycle::schema_entries(g))
    .chain(std::iter::once((
        executors::EXECUTOR_LIST,
        m(
            sub::<executors::ExecutorListParams>(g),
            sub::<executors::ExecutorListResult>(g),
        ),
    )))
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let events: serde_json::Map<String, Value> = event_payloads(g)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let frames = json!({
        "Request": sub::<Request>(g),
        "ServerFrame": sub::<ServerFrame>(g),
    });
    let agent = agent::schema(g);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "topics": events::TOPICS,
        "frames": frames,
        "methods": methods,
        "events": events,
        "agent": agent,
        "$defs": generator.take_definitions(true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::schema_for;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn health_kind_view_covers_every_exec_health_kind() {
        // crewd-exec 的 HealthKind 共 8 档（含 codex 的 TurnFailed / VersionIncompatible）；线上 snake_case。
        let want = [
            "auth_required",
            "usage_limit",
            "rate_limited",
            "launch_failed",
            "brief_undelivered",
            "prompt_undelivered",
            "turn_failed",
            "version_incompatible",
        ];
        let got: Vec<&str> = HealthKindView::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(got, want);
        for w in want {
            let k = HealthKindView::parse(w).unwrap();
            assert_eq!(serde_json::to_value(k).unwrap(), json!(w));
        }
    }

    /// U1：hello 的参数全可选、收 null、**收未知字段**（以后的客户端在握手里加字段）；结果带版本、构建、能力，
    /// 老 crewd 的 hello（没有新字段）新客户端也读得出来。
    #[test]
    fn hello_params_accept_null_and_unknown_fields_and_the_result_is_additive() {
        let p: HelloParams = parse_params_or_default(Value::Null).unwrap();
        assert_eq!(p, HelloParams::default());
        let p: HelloParams = parse_params(json!({
            "client": {"name": "合成客户端", "version": "1.0", "build": "b1", "later": true},
            "apiVersion": 4,
            "capabilities": ["x"],
            "negotiate": {"anything": 1}
        }))
        .unwrap();
        assert_eq!(p.api_version, Some(4));
        assert_eq!(p.client.unwrap().build.as_deref(), Some("b1"));
        let h = Hello {
            server: "crewd".into(),
            version: "0.1.0".into(),
            api_version: API_VERSION,
            min_api_version: MIN_API_VERSION,
            build: BuildInfo {
                git_sha: "0123456789ab".into(),
                dirty: false,
                exe: "/合成/crewd".into(),
                exe_still_on_disk: true,
            },
            capabilities: vec!["orchestra".into()],
            daemon_health: None,
            remote_binding: None,
            runtime_identity: None,
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["minApiVersion"], MIN_API_VERSION);
        assert_eq!(v["build"]["exeStillOnDisk"], true);
        assert_eq!(v["build"]["gitSha"], "0123456789ab");
        let old: Hello =
            serde_json::from_value(json!({"server":"crewd","version":"0.1.0","apiVersion":3}))
                .unwrap();
        assert_eq!((old.min_api_version, old.capabilities.len()), (0, 0));
        assert!(old.daemon_health.is_none());
        assert!(
            v.get("daemonHealth").is_none(),
            "empty additive field stays omitted"
        );
        let mut future = v.clone();
        future["daemonHealth"] = json!({"lastDbError":{
            "stage":"future_stage", "kind":"future_kind", "storageKind":"future_storage",
            "atMs":123, "sequence":7, "futureField":true
        },"futureField":true});
        let decoded: Hello = serde_json::from_value(future).unwrap();
        let fault = decoded.daemon_health.unwrap().last_db_error.unwrap();
        assert_eq!(fault.stage, DbFaultStageView::Unknown);
        assert_eq!(fault.kind, DbFaultKindView::Unknown);
        assert_eq!(fault.storage_kind, Some(DbStorageKindView::Unknown));
        assert_eq!((fault.at_ms, fault.sequence), (123, 7));
        const { assert!(MIN_API_VERSION <= API_VERSION) };
    }

    #[test]
    fn request_params_default_to_null() {
        let r: Request = serde_json::from_str(r#"{"id":7,"method":"system.hello"}"#).unwrap();
        assert_eq!(r.id, 7);
        assert_eq!(r.params, Value::Null);
    }

    #[test]
    fn server_frame_distinguishes_response_and_event() {
        let f: ServerFrame = serde_json::from_str(r#"{"id":1,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(f, ServerFrame::Response(_)));
        let f: ServerFrame =
            serde_json::from_str(r#"{"event":"message.appended","data":{}}"#).unwrap();
        assert!(matches!(f, ServerFrame::Event(_)));
    }

    #[test]
    fn response_omits_absent_fields() {
        let s = serde_json::to_value(Response::ok(3, json!(1))).unwrap();
        assert_eq!(s, json!({"id":3,"result":1}));
    }

    #[test]
    fn message_uses_camel_case_and_skips_none() {
        let m = Message {
            seq: 5,
            id: "m1".into(),
            channel_id: "local-a/lobby".into(),
            sender_kind: SenderKind::Human,
            sender_session_id: None,
            sender_name: None,
            text: "你好 👋".into(),
            category: None,
            in_reply_to: Some("m0".into()),
            mentions: vec![Mention {
                kind: MentionKind::Session,
                target_id: Some("worker-1".into()),
            }],
            attachments: vec![],
            created_at: "2026-10-05T00:00:00.000Z".into(),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["channelId"], "local-a/lobby");
        assert_eq!(v["inReplyTo"], "m0");
        assert_eq!(v["mentions"][0]["targetId"], "worker-1");
        assert!(v.get("senderName").is_none());
    }

    #[test]
    fn parse_params_reports_invalid_params() {
        let e = parse_params::<MessageListParams>(json!({"limit": 3})).unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
    }

    #[test]
    fn params_reject_unknown_fields_and_no_param_methods_accept_null_or_empty() {
        let e = parse_params::<ReadMarkParams>(json!({"channelId":"c","seq":1,"bogus":true}))
            .unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        assert!(parse_no_params(Value::Null).is_ok());
        assert!(parse_no_params(json!({})).is_ok());
        assert!(parse_no_params(json!({"x":1})).is_err());
        let p: SessionListParams = parse_params_or_default(Value::Null).unwrap();
        assert_eq!(p.crew_id, None);
    }

    /// 控制者裁定 b：params 只能是对象或 null。serde 会把数组按字段顺序读成结构体（`["c"]` → `{crewId:"c"}`），
    /// 所以这一条必须在解析前拦，不能指望字段类型不对。
    #[test]
    fn params_must_be_an_object_or_null() {
        for bad in [json!(["c"]), json!([]), json!(1), json!("c"), json!(true)] {
            assert_eq!(
                parse_params::<CrewIdParams>(bad.clone()).unwrap_err().code,
                RpcError::INVALID_PARAMS,
                "{bad}"
            );
            assert_eq!(
                parse_params_or_default::<SessionListParams>(bad.clone())
                    .unwrap_err()
                    .code,
                RpcError::INVALID_PARAMS,
                "{bad}"
            );
            assert!(parse_no_params(bad.clone()).is_err(), "{bad}");
        }
        assert!(parse_params::<CrewIdParams>(json!({"crewId":"c"})).is_ok());
    }

    /// 控制者裁定 c + 复审：参数里嵌套的值（@、成员、附件）拒未知字段，用的是参数专用类型
    /// （`MentionParam` / `MemberParam` / `AttachmentParam`）；共享的模型类型对库里读出来的行保持宽松。
    #[test]
    fn nested_param_values_deny_unknown_fields() {
        assert!(serde_json::from_value::<MentionParam>(json!({"kind":"human","bogus":1})).is_err());
        assert!(
            serde_json::from_value::<MemberParam>(json!({"kind":"session","id":"w","bogus":1}))
                .is_err()
        );
        assert!(serde_json::from_value::<MemberParam>(json!({"kind":"human","id":"x"})).is_err());
        assert!(
            serde_json::from_value::<AttachmentParam>(
                json!({"id":"a","mime":"x","path":"/p","width":3})
            )
            .is_err()
        );
        for bad in [
            json!({"channelId":"c","text":"t","mentions":[{"kind":"session","targetId":"w","label":"@w"}]}),
            json!({"channelId":"c","text":"t","attachments":[{"id":"a","mime":"x","path":"/p","width":3}]}),
        ] {
            assert_eq!(
                parse_params::<MessagePostParams>(bad.clone())
                    .unwrap_err()
                    .code,
                RpcError::INVALID_PARAMS,
                "{bad}"
            );
        }
        assert_eq!(
            parse_params::<ChannelMemberParams>(
                json!({"channelId":"c","member":{"kind":"human","id":"x"}})
            )
            .unwrap_err()
            .code,
            RpcError::INVALID_PARAMS
        );
        // 合法的照收，并转成模型类型。
        let p: MessagePostParams = parse_params(json!({
            "channelId":"c","text":"t","mentions":[{"kind":"session","targetId":"w"}]
        }))
        .unwrap();
        assert_eq!(
            Mention::from(p.mentions[0].clone()),
            Mention {
                kind: MentionKind::Session,
                target_id: Some("w".into())
            }
        );
        assert_eq!(
            MemberRef::from(
                serde_json::from_value::<MemberParam>(json!({"kind":"human"})).unwrap()
            ),
            MemberRef::Human
        );
        assert_eq!(
            MemberRef::from(
                serde_json::from_value::<MemberParam>(json!({"kind":"session","id":"w"})).unwrap()
            ),
            MemberRef::session("w")
        );
        // 模型类型宽松：库里存的行多一个键照样读（输出不带它）。
        let m: Mention = serde_json::from_value(json!({"kind":"human","label":"@人"})).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), json!({"kind":"human"}));
        assert!(
            serde_json::from_value::<Attachment>(
                json!({"id":"a","mime":"x","path":"/p","width":3})
            )
            .is_ok()
        );
        assert!(serde_json::from_value::<MemberRef>(json!({"kind":"human","x":1})).is_ok());
        // 契约里参数类型写成 additionalProperties: false。
        for (name, s) in [
            ("MentionParam", json!(schema_for!(MentionParam))),
            ("AttachmentParam", json!(schema_for!(AttachmentParam))),
        ] {
            assert_eq!(s["additionalProperties"], json!(false), "{name}: {s}");
        }
        let member = json!(schema_for!(MemberParam));
        let variants = member["oneOf"].as_array().expect("MemberParam 是 oneOf");
        assert_eq!(variants.len(), 2);
        for v in variants {
            assert_eq!(
                v["additionalProperties"],
                json!(false),
                "MemberParam: {member}"
            );
        }
    }

    /// 复审裁定 2–4：帧可直接生成代码。`Event` 是按 `event` 区分的 oneOf（每个事件一支，`data` 指向负载类型）；
    /// `ServerFrame` 是 oneOf [Response, Event]；订阅主题是封闭枚举；顶层声明 `$schema`。
    #[test]
    fn frames_are_discriminated_and_topics_are_an_enum() {
        let s = api_schema();
        assert_eq!(s["$schema"], "https://json-schema.org/draft/2020-12/schema");
        let defs = &s["$defs"];
        assert_eq!(
            defs["ServerFrame"]["oneOf"],
            json!([{"$ref":"#/$defs/Response"},{"$ref":"#/$defs/Event"}]),
            "{}",
            defs["ServerFrame"]
        );
        let branches = defs["Event"]["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("Event 不是 oneOf：{}", defs["Event"]));
        let table = s["events"].as_object().unwrap();
        assert_eq!(branches.len(), table.len());
        for b in branches {
            let name = b["properties"]["event"]["const"]
                .as_str()
                .unwrap_or_else(|| panic!("分支没有 event 常量：{b}"));
            assert_eq!(b["properties"]["data"], table[name], "{name}");
            assert_eq!(b["required"], json!(["event", "data"]), "{name}");
        }
        assert_eq!(
            defs["SubscribeParams"]["properties"]["topics"]["items"]["enum"],
            json!(events::TOPICS),
            "{}",
            defs["SubscribeParams"]
        );
    }

    /// 一个 schema 的形状：每个分支（oneOf；没有就是它自己）的 (kind 常量, 属性键集合, required 集合)。
    fn shape(s: &Value) -> Vec<(Option<String>, BTreeSet<String>, BTreeSet<String>)> {
        let keys = |v: &Value| -> BTreeSet<String> {
            v.as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default()
        };
        let strings = |v: &Value| -> BTreeSet<String> {
            v.as_array()
                .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                .unwrap_or_default()
        };
        let branches: Vec<&Value> = match s["oneOf"].as_array() {
            Some(a) => a.iter().collect(),
            None => vec![s],
        };
        let mut out: Vec<_> = branches
            .into_iter()
            .map(|b| {
                (
                    b["properties"]["kind"]["const"]
                        .as_str()
                        .map(str::to_string),
                    keys(&b["properties"]),
                    strings(&b["required"]),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// 严格的参数类型（`*Param`）与宽松的模型类型线上形状相同、不漂：满字段的模型值序列化后能被参数类型收下，
    /// `From` 回来逐字段相等；两边 schema 的属性键与 required 逐分支相同。
    #[test]
    fn model_and_param_types_do_not_drift() {
        fn round_trip<M, P>(m: M)
        where
            M: Serialize + PartialEq + std::fmt::Debug + From<P>,
            P: serde::de::DeserializeOwned,
        {
            let wire = serde_json::to_value(&m).unwrap();
            let p: P = serde_json::from_value(wire.clone())
                .unwrap_or_else(|e| panic!("参数类型不收模型的线上形状 {wire}：{e}"));
            assert_eq!(M::from(p), m, "{wire}");
        }
        round_trip::<Mention, MentionParam>(Mention {
            kind: MentionKind::Captain,
            target_id: Some("cap-a".into()),
        });
        round_trip::<Attachment, AttachmentParam>(Attachment {
            id: "att-a".into(),
            mime: "image/png".into(),
            size: Some(42),
            path: "/示例/附件.png".into(),
            filename: Some("附件.png".into()),
        });
        round_trip::<MemberRef, MemberParam>(MemberRef::session("w-a1"));
        round_trip::<MemberRef, MemberParam>(MemberRef::Human);
        for (name, model, param) in [
            (
                "Mention",
                json!(schema_for!(Mention)),
                json!(schema_for!(MentionParam)),
            ),
            (
                "Attachment",
                json!(schema_for!(Attachment)),
                json!(schema_for!(AttachmentParam)),
            ),
            (
                "MemberRef",
                json!(schema_for!(MemberRef)),
                json!(schema_for!(MemberParam)),
            ),
        ] {
            let (m, p) = (shape(&model), shape(&param));
            assert!(!m.is_empty() && !m[0].1.is_empty(), "{name}: {model}");
            assert_eq!(m, p, "{name}\n模型：{model}\n参数：{param}");
        }
        let member = shape(&json!(schema_for!(MemberRef)));
        assert_eq!(
            member.iter().map(|b| b.0.as_deref()).collect::<Vec<_>>(),
            [Some("human"), Some("session")],
            "MemberRef 两个分支，按 kind 常量对齐"
        );
    }

    #[test]
    fn ledger_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Ledger::Human).unwrap(), json!("human"));
        assert_eq!(Ledger::Agent.as_str(), "agent");
    }

    #[test]
    fn vocabularies_are_closed_enums_on_the_wire() {
        assert_eq!(
            serde_json::to_value(SenderKind::System).unwrap(),
            json!("system")
        );
        assert!(
            serde_json::from_value::<SenderKind>(json!("user")).is_err(),
            "旧词只在迁移 / 导入里归一"
        );
        assert_eq!(MentionKind::parse("broadcast"), None);
        assert_eq!(serde_json::to_value(ChannelKind::Dm).unwrap(), json!("dm"));
        assert_eq!(
            serde_json::to_value(MemberRef::Session { id: "w-1".into() }).unwrap(),
            json!({"kind":"session","id":"w-1"})
        );
        assert_eq!(
            serde_json::to_value(MemberRef::Human).unwrap(),
            json!({"kind":"human"})
        );
        assert_eq!(
            serde_json::to_value(WaitTierView::StillWanted).unwrap(),
            json!("still_wanted")
        );
        assert_eq!(
            serde_json::to_value(RestartAnswer::Later).unwrap(),
            json!("later")
        );
        assert_eq!(SenderKind::parse("captain"), Some(SenderKind::Captain));
        assert_eq!(Role::parse("captain"), Some(Role::Captain));
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("chief"), None);
    }

    #[test]
    fn session_and_crew_carry_v3_fields_with_defaults() {
        let s: Session = serde_json::from_value(json!({
            "id":"w-1","crewId":"c","role":"worker","executor":"claude","updatedAt":"t"
        }))
        .unwrap();
        assert_eq!(s.desired, Desired::Stopped);
        assert!(!s.was_working);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["desired"], "stopped");
        assert!(v.get("title").is_none(), "None 不上线");
        let c = Crew {
            id: "c".into(),
            captain_session_id: Some("cap".into()),
            child_ids: vec!["k1".into(), "k2".into()],
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&c).unwrap()["childIds"][1], "k2");
        assert_eq!(serde_json::to_value(&c).unwrap()["captainSessionId"], "cap");
        assert_eq!(serde_json::to_value(&c).unwrap()["position"], 0);
    }

    fn walk_refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get("$ref") {
                    out.push(r.clone());
                }
                m.values().for_each(|x| walk_refs(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk_refs(x, out)),
            _ => {}
        }
    }

    #[test]
    fn schema_lists_exactly_the_client_methods() {
        let s = api_schema();
        let mut listed: Vec<&str> = s["methods"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        listed.sort();
        let mut all: Vec<&str> = methods::ALL.to_vec();
        all.sort();
        assert_eq!(listed, all);
        assert_eq!(s["apiVersion"], 3);
    }

    /// Legacy PCW approval workflow stays removed. Native Codex policy/structured
    /// decisions are the explicitly requested session surface, not that workflow.
    #[test]
    fn there_is_no_legacy_approval_workflow() {
        let schema = api_schema();
        for method in schema["methods"].as_object().unwrap().keys() {
            assert!(
                !method.starts_with("approval."),
                "legacy approval RPC must stay absent"
            );
        }
        assert_eq!(API_VERSION, 3);
    }

    #[test]
    fn api3_start_without_options_preserves_the_original_command_envelope() {
        let original = json!({"operation":"session.start","params":{"crewId":"crew-original","role":"captain","executor":"codex","brief":"Original task exactly","title":null,"model":null,"effort":null,"isolation":false}});
        let command: commands::PublicCommand = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            original,
            "an absent additive field must not change the preexisting API3 envelope"
        );
        let mut configured = original;
        configured["params"]["codexOptions"] = json!({"fast":false});
        let command: commands::PublicCommand = serde_json::from_value(configured.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(command).unwrap(),
            configured,
            "actual options retain their exact command identity"
        );
    }

    #[test]
    fn every_ref_resolves_to_a_single_top_level_def() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        let mut refs = Vec::new();
        walk_refs(&s, &mut refs);
        assert!(!refs.is_empty());
        for r in refs {
            let name = r
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("不是顶层 $defs 引用：{r}"));
            assert!(defs.contains_key(name), "悬空引用 {r}");
        }
        for (name, d) in defs {
            assert!(d.get("$defs").is_none(), "{name} 自带了一份 $defs");
        }
    }

    // A nullable method may use a closed top-level object definition wrapped
    // in exactly one ref/null union. Keep checking the underlying object.
    fn params_contract_definition<'a>(schema: &'a Value, params: &Value) -> (&'a Value, bool) {
        let (reference, nullable) = if let Some(reference) = params["$ref"].as_str() {
            (reference, false)
        } else {
            assert_eq!(params.as_object().unwrap().len(), 1);
            let variants = params["anyOf"].as_array().expect("params ref/null union");
            assert_eq!(variants.len(), 2);
            assert_eq!(
                variants
                    .iter()
                    .filter(|v| **v == json!({"type":"null"}))
                    .count(),
                1
            );
            let reference = variants
                .iter()
                .find_map(|v| {
                    let fields = v.as_object()?;
                    if fields.len() == 1 {
                        v["$ref"].as_str()
                    } else {
                        None
                    }
                })
                .expect("one closed params reference beside null");
            (reference, true)
        };
        let name = reference
            .strip_prefix("#/$defs/")
            .expect("top-level params reference");
        (
            schema["$defs"]
                .get(name)
                .expect("resolved params definition"),
            nullable,
        )
    }

    #[test]
    fn every_params_type_denies_unknown_fields_and_vocabularies_are_enums() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        for (m, spec) in s["methods"].as_object().unwrap() {
            let (def, _) = params_contract_definition(&s, &spec["params"]);
            // 唯一的例外（U1）：握手的参数收未知字段——以后的客户端在这里加字段。
            let want = if m == methods::HELLO {
                Value::Null
            } else {
                json!(false)
            };
            assert_eq!(
                def["additionalProperties"], want,
                "{m} 的参数：除了 system.hello 都要 deny_unknown_fields"
            );
        }
        for e in [
            "SenderKind",
            "MentionKind",
            "Role",
            "ChannelKind",
            "Desired",
            "AskStateView",
            "RestartAnswer",
            "RestartReason",
        ] {
            assert!(
                defs[e].get("enum").is_some() || defs[e].get("oneOf").is_some(),
                "{e} 不是封闭枚举：{}",
                defs[e]
            );
        }
        for e in [
            events::MESSAGE_APPENDED,
            events::SESSION_CHANGED,
            events::LAGGED,
        ] {
            assert!(s["events"][e].is_object(), "缺事件 {e}");
        }
    }

    #[test]
    fn t10c_decision_wire_defaults_terminal_and_preserves_zero_based_selected() {
        let old: DecisionView = serde_json::from_value(
            json!({"summary":"Trust?","options":["Yes","No"],"numbered":false}),
        )
        .unwrap();
        assert_eq!(old.kind, DecisionKindView::Terminal);
        assert_eq!(old.selected, None);
        assert_eq!(old.fingerprint, "");
        let runner = DecisionView {
            fingerprint: "request:r1".into(),
            selected: Some(0),
            kind: DecisionKindView::RunnerRequest,
            ..old
        };
        let wire = serde_json::to_value(&runner).unwrap();
        assert_eq!(wire["kind"], "runner_request");
        assert_eq!(wire["selected"], 0);
        assert_eq!(
            serde_json::from_value::<DecisionView>(wire).unwrap(),
            runner
        );
        let zero: orchestra::SessionAnswerParams =
            serde_json::from_value(json!({"sessionId":"s","fingerprint":"f","choice":0})).unwrap();
        assert_eq!(zero.choice, 0);
        assert!(
            serde_json::from_value::<orchestra::SessionAnswerParams>(
                json!({"sessionId":"s","fingerprint":"f","choice":-1})
            )
            .is_err()
        );
    }

    /// R11：契约与解析器一致。解析器收 `null` 的方法（无参 / 全可选参数），参数类型在契约里写成 `object | null`；
    /// 其余的不收 null。`restart.pending` 的结果可以是 null。
    #[test]
    fn methods_whose_parser_accepts_null_say_so_in_the_contract() {
        let s = api_schema();
        let null_ok = [
            judgments::SETTINGS_GET,
            methods::REMOTE_STATUS,
            methods::REMOTE_GRANTS,
            methods::WORK_PATROL,
            methods::HELLO,
            methods::CREW_LIST,
            methods::RESTART_PENDING,
            methods::SESSION_LIST,
            methods::ASK_LIST,
            methods::DIRECTORY_LIST,
            orchestra::methods::WAKE_LIST,
            orchestra::methods::LOOP_LIST,
            orchestra::methods::DECISION_LIST,
            orchestra::methods::SETTINGS_GET,
            orchestra::methods::HIBERNATE,
        ];
        for (m, spec) in s["methods"].as_object().unwrap() {
            let (def, nullable) = params_contract_definition(&s, &spec["params"]);
            let takes_null = nullable
                || def["type"]
                    .as_array()
                    .is_some_and(|t| t.contains(&json!("null")));
            assert_eq!(takes_null, null_ok.contains(&m.as_str()), "{m}: 契约 {def}");
        }
        let pending = &s["methods"][methods::RESTART_PENDING]["result"];
        assert!(
            pending["anyOf"]
                .as_array()
                .is_some_and(|a| a.contains(&json!({"type":"null"}))),
            "{pending}"
        );
    }

    /// `crew.purged` / `channel.purged` / `events.lagged` / 订阅回执有了类型，线上形状与 Task 13 的手写 JSON 相同。
    #[test]
    fn small_event_and_ack_payloads_keep_their_wire_shape() {
        assert_eq!(
            serde_json::to_value(CrewRef {
                crew_id: "crew-a".into()
            })
            .unwrap(),
            json!({"crewId":"crew-a"})
        );
        assert_eq!(
            serde_json::to_value(ChannelRef {
                channel_id: "crew-a/lobby".into()
            })
            .unwrap(),
            json!({"channelId":"crew-a/lobby"})
        );
        assert_eq!(
            serde_json::to_value(Lagged { missed: 3 }).unwrap(),
            json!({"missed":3})
        );
        assert_eq!(
            serde_json::to_value(Subscribed {
                topics: vec!["message".into()]
            })
            .unwrap(),
            json!({"topics":["message"]})
        );
    }

    /// 控制者裁定 g：`"result": null`（成功、值是 null）与「没有 result」是两回事；一条回复恰好带 result 或 error 之一。
    #[test]
    fn a_null_result_is_not_an_absent_result() {
        let f: ServerFrame = serde_json::from_str(r#"{"id":4,"result":null}"#).unwrap();
        match f {
            ServerFrame::Response(r) => assert_eq!(r.into_result(), Ok(Value::Null)),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            serde_json::to_value(Response::ok(4, Value::Null)).unwrap(),
            json!({"id":4,"result":null})
        );
        let f: ServerFrame =
            serde_json::from_str(r#"{"id":5,"error":{"code":-32004,"message":"没有"}}"#).unwrap();
        match f {
            ServerFrame::Response(r) => assert_eq!(r.into_result().unwrap_err().code, -32004),
            other => panic!("{other:?}"),
        }
        assert!(
            serde_json::from_str::<ServerFrame>(r#"{"id":4}"#).is_err(),
            "既没有 result 也没有 error 不是合法的帧"
        );
        assert!(
            serde_json::from_str::<ServerFrame>(
                r#"{"id":4,"result":1,"error":{"code":-32603,"message":"x"}}"#
            )
            .is_err(),
            "两样都带也不是"
        );
        // 契约里也是「恰好一个」。
        let s = api_schema();
        let resp = &s["$defs"]["Response"];
        let one_of = resp["oneOf"].as_array().unwrap_or_else(|| panic!("{resp}"));
        let required: Vec<&Value> = one_of.iter().map(|b| &b["required"]).collect();
        assert_eq!(required, [&json!(["result"]), &json!(["error"])], "{resp}");
        assert!(s["frames"]["ServerFrame"]["$ref"].is_string());
    }
}

/// SQLite knowledge API; only implemented methods are registered.
pub mod team_data;

#[path = "sqlite_data_api.rs"]
pub mod team_data_api;

pub mod lifecycle;

pub mod codex;
