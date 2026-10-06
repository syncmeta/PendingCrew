//! crewd 线协议：客户端（Mac / iOS / CLI）与 crewd 之间的唯一契约。
//! 传输：每行一个 JSON（NDJSON）。客户端发 `Request`，服务端回 `ServerFrame`。
mod model;
mod params;

pub use model::*;
pub use params::*;
use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 2：`whiteboard.*` 删除、`Message.crewId` → `channelId`（裁定 18）。
pub const API_VERSION: u32 = 2;

pub mod methods {
    pub const HELLO: &str = "system.hello";
    pub const CREW_LIST: &str = "crew.list";
    pub const CREW_GET: &str = "crew.get";
    pub const CREW_CREATE: &str = "crew.create";
    pub const CREW_UPDATE: &str = "crew.update";
    pub const CREW_ADOPT: &str = "crew.adopt";
    pub const CREW_REORDER: &str = "crew.reorder";
    pub const CREW_RELEASE: &str = "crew.release";
    pub const CREW_DELETE: &str = "crew.delete";
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
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_GET: &str = "session.get";
    pub const ASK_LIST: &str = "ask.list";
    pub const ASK_ANSWER: &str = "ask.answer";
    pub const ASK_WITHDRAW: &str = "ask.withdraw";
    pub const APPROVAL_LIST: &str = "approval.list";
    pub const APPROVAL_DECIDE: &str = "approval.decide";
    pub const RESTART_PENDING: &str = "restart.pending";
    pub const RESTART_ANSWER: &str = "restart.answer";
    pub const DIRECTORY_LIST: &str = "directory.list";
    pub const TODO_LIST: &str = "todo.list";
    pub const SUBSCRIBE: &str = "events.subscribe";
    // M3b / M3c 的方法常量写在各自的段落里，并加进 ALL（契约 json 由后合者重新生成）。

    /// 客户端可调的全部方法（契约测试与 `crewd rpc` 端到端都按它逐个核）。
    pub const ALL: &[&str] = &[
        HELLO,
        CREW_LIST,
        CREW_GET,
        CREW_CREATE,
        CREW_UPDATE,
        CREW_ADOPT,
        CREW_REORDER,
        CREW_RELEASE,
        CREW_DELETE,
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
        SESSION_LIST,
        SESSION_GET,
        ASK_LIST,
        ASK_ANSWER,
        ASK_WITHDRAW,
        APPROVAL_LIST,
        APPROVAL_DECIDE,
        RESTART_PENDING,
        RESTART_ANSWER,
        DIRECTORY_LIST,
        TODO_LIST,
        SUBSCRIBE,
    ];
}

pub mod events {
    /// `events.subscribe` 接受的主题（= 事件名第一个点之前的部分）；其他一律 -32602。
    /// `session` 与 `wake`（排障用的 `wake.changed`，常量在 M3c 的 `proto::orchestra::events`）主题的事件由 M3c 发；
    /// 主题名在这里一次列全，M3c 不改 conn.rs / 这张表。
    pub const TOPICS: &[&str] = &[
        "message", "channel", "crew", "badge", "ask", "approval", "session", "restart", "wake",
    ];
    pub const MESSAGE_APPENDED: &str = "message.appended";
    pub const CHANNEL_CHANGED: &str = "channel.changed";
    pub const CREW_CHANGED: &str = "crew.changed";
    pub const CREW_REMOVED: &str = "crew.removed";
    pub const BADGE_CHANGED: &str = "badge.changed";
    pub const ASK_CHANGED: &str = "ask.changed";
    pub const APPROVAL_CHANGED: &str = "approval.changed";
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
}

impl RpcError {
    pub const PARSE: i32 = -32700;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL: i32 = -32603;
    /// 规则不允许（不是参数写错：换个身份 / 时机就行）。
    pub const FORBIDDEN: i32 = -32003;
    /// 引用的东西不存在（机组、群、消息、session、H 号、审批、重启询问……）。
    pub const NOT_FOUND: i32 = -32004;
    /// 与当前状态冲突（已经决过的审批、还有在跑 session 的机组……）。
    pub const CONFLICT: i32 = -32009;

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: u64, error: RpcError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ServerFrame {
    Response(Response),
    Event(Event),
}

pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
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

/// 整份 API 契约。`crewd schema` 打印它；仓库里提交一份，测试保证两者一致。
pub fn api_schema() -> Value {
    // 方法表此刻是类型名字符串（Task 14 整体改成 `$ref`）。`X | null` = 解析器也收 `null`（R11：契约与解析器一致）：
    // 无参数方法收 `null` 或 `{}`，全可选参数的方法 `null` 当 `{}`。
    // json! 一次展开这么多会撞宏递归上限：方法 / 事件 / 类型各用一张表拼。
    let methods: serde_json::Map<String, Value> = [
        (methods::HELLO, "NoParams | null", "Hello"),
        (methods::CREW_LIST, "NoParams | null", "Crew[]"),
        (methods::CREW_GET, "CrewIdParams", "CrewDetail"),
        (methods::CREW_CREATE, "CrewCreateParams", "Crew"),
        (methods::CREW_UPDATE, "CrewUpdateParams", "Crew"),
        (methods::CREW_ADOPT, "CrewAdoptParams", "Crew"),
        (methods::CREW_REORDER, "CrewReorderParams", "Crew[]"),
        (methods::CREW_RELEASE, "CrewReleaseParams", "Crew"),
        (methods::CREW_DELETE, "CrewIdParams", "Empty"),
        (methods::CHANNEL_LIST, "CrewIdParams", "ChannelSummary[]"),
        (methods::CHANNEL_CREATE, "ChannelCreateParams", "Channel"),
        (methods::CHANNEL_UPDATE, "ChannelUpdateParams", "Channel"),
        (
            methods::CHANNEL_ADD_MEMBERS,
            "ChannelMembersParams",
            "Channel",
        ),
        (
            methods::CHANNEL_REMOVE_MEMBER,
            "ChannelMemberParams",
            "Channel",
        ),
        (
            methods::CHANNEL_WATCH,
            "ChannelWatchParams",
            "ChannelSummary",
        ),
        (
            methods::CHANNEL_SET_OWNER,
            "ChannelSetOwnerParams",
            "Channel",
        ),
        (methods::MESSAGE_LIST, "MessageListParams", "Message[]"),
        (methods::MESSAGE_POST, "MessagePostParams", "Message"),
        (methods::MESSAGE_SEARCH, "MessageSearchParams", "Message[]"),
        (methods::READ_MARK, "ReadMarkParams", "ChannelSummary"),
        (
            methods::SESSION_LIST,
            "SessionListParams | null",
            "Session[]",
        ),
        (methods::SESSION_GET, "SessionGetParams", "Session"),
        (methods::ASK_LIST, "AskListParams | null", "HumanWait[]"),
        (methods::ASK_ANSWER, "AskAnswerParams", "Ask"),
        (methods::ASK_WITHDRAW, "AskWithdrawParams", "Ask"),
        (methods::APPROVAL_LIST, "NoParams | null", "Approval[]"),
        (methods::APPROVAL_DECIDE, "ApprovalDecideParams", "Approval"),
        (
            methods::RESTART_PENDING,
            "NoParams | null",
            "RestartPrompt | null",
        ),
        (
            methods::RESTART_ANSWER,
            "RestartAnswerParams",
            "RestartPrompt",
        ),
        (
            methods::DIRECTORY_LIST,
            "DirectoryListParams | null",
            "DirectoryRow[]",
        ),
        (methods::TODO_LIST, "TodoListParams", "Todo[]"),
        (
            methods::SUBSCRIBE,
            "SubscribeParams",
            "{ topics: string[] }",
        ),
    ]
    .into_iter()
    .map(|(m, p, r)| (m.to_string(), json!({ "params": p, "result": r })))
    .collect();
    let events: serde_json::Map<String, Value> = [
        (events::MESSAGE_APPENDED, "Message"),
        (events::CHANNEL_CHANGED, "ChannelSummary"),
        (events::CREW_CHANGED, "Crew"),
        (events::CREW_REMOVED, "{ crewId: string }"),
        (events::BADGE_CHANGED, "Badge"),
        (events::ASK_CHANGED, "Ask"),
        (events::APPROVAL_CHANGED, "Approval"),
        (events::SESSION_CHANGED, "SessionLive"),
        (events::RESTART_CHANGED, "RestartPrompt"),
        (events::LAGGED, "{ missed: number }"),
    ]
    .into_iter()
    .map(|(e, d)| (e.to_string(), json!(d)))
    .collect();
    let types: serde_json::Map<String, Value> = [
        ("Request", json!(schema_for!(Request))),
        ("Response", json!(schema_for!(Response))),
        ("Event", json!(schema_for!(Event))),
        ("Hello", json!(schema_for!(Hello))),
        ("Crew", json!(schema_for!(Crew))),
        ("CrewDetail", json!(schema_for!(CrewDetail))),
        ("Session", json!(schema_for!(Session))),
        ("SessionLive", json!(schema_for!(SessionLive))),
        ("Message", json!(schema_for!(Message))),
        ("Channel", json!(schema_for!(Channel))),
        ("ChannelSummary", json!(schema_for!(ChannelSummary))),
        ("Badge", json!(schema_for!(Badge))),
        ("Ask", json!(schema_for!(Ask))),
        ("HumanWait", json!(schema_for!(HumanWait))),
        ("Approval", json!(schema_for!(Approval))),
        ("RestartPrompt", json!(schema_for!(RestartPrompt))),
        ("DirectoryRow", json!(schema_for!(DirectoryRow))),
        ("Todo", json!(schema_for!(Todo))),
        ("Empty", json!(schema_for!(Empty))),
        ("NoParams", json!(schema_for!(NoParams))),
        ("CrewIdParams", json!(schema_for!(CrewIdParams))),
        ("CrewCreateParams", json!(schema_for!(CrewCreateParams))),
        ("CrewUpdateParams", json!(schema_for!(CrewUpdateParams))),
        ("CrewAdoptParams", json!(schema_for!(CrewAdoptParams))),
        ("CrewReorderParams", json!(schema_for!(CrewReorderParams))),
        ("CrewReleaseParams", json!(schema_for!(CrewReleaseParams))),
        (
            "ChannelCreateParams",
            json!(schema_for!(ChannelCreateParams)),
        ),
        (
            "ChannelUpdateParams",
            json!(schema_for!(ChannelUpdateParams)),
        ),
        (
            "ChannelMembersParams",
            json!(schema_for!(ChannelMembersParams)),
        ),
        (
            "ChannelMemberParams",
            json!(schema_for!(ChannelMemberParams)),
        ),
        ("ChannelWatchParams", json!(schema_for!(ChannelWatchParams))),
        (
            "ChannelSetOwnerParams",
            json!(schema_for!(ChannelSetOwnerParams)),
        ),
        ("MessageListParams", json!(schema_for!(MessageListParams))),
        ("MessagePostParams", json!(schema_for!(MessagePostParams))),
        (
            "MessageSearchParams",
            json!(schema_for!(MessageSearchParams)),
        ),
        ("ReadMarkParams", json!(schema_for!(ReadMarkParams))),
        ("SessionListParams", json!(schema_for!(SessionListParams))),
        ("SessionGetParams", json!(schema_for!(SessionGetParams))),
        ("AskListParams", json!(schema_for!(AskListParams))),
        ("AskAnswerParams", json!(schema_for!(AskAnswerParams))),
        ("AskWithdrawParams", json!(schema_for!(AskWithdrawParams))),
        (
            "ApprovalDecideParams",
            json!(schema_for!(ApprovalDecideParams)),
        ),
        (
            "RestartAnswerParams",
            json!(schema_for!(RestartAnswerParams)),
        ),
        (
            "DirectoryListParams",
            json!(schema_for!(DirectoryListParams)),
        ),
        ("TodoListParams", json!(schema_for!(TodoListParams))),
        ("SubscribeParams", json!(schema_for!(SubscribeParams))),
    ]
    .into_iter()
    .map(|(n, t)| (n.to_string(), t))
    .collect();
    json!({
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "methods": methods,
        "events": events,
        "types": types,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert_eq!(
            serde_json::to_value(MentionKind::Broadcast).unwrap(),
            json!("broadcast")
        );
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

    #[test]
    fn api_schema_lists_methods_and_types() {
        let s = api_schema();
        assert_eq!(s["apiVersion"], 2);
        assert!(s["methods"]["message.post"].is_object());
        assert!(s["types"]["Message"].is_object());
    }
}
