//! crewd 线协议：客户端（Mac / iOS / CLI）与 crewd 之间的唯一契约。
//! 传输：每行一个 JSON（NDJSON）。客户端发 `Request`，服务端回 `ServerFrame`。
mod model;
mod params;

pub use model::*;
pub use params::*;
use schemars::JsonSchema;
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
    /// 群随机组删除被级联删掉（`{channelId}`）。
    pub const CHANNEL_REMOVED: &str = "channel.removed";
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

/// 每个事件名 → 它的 `data` 类型。契约的 `events` 表与 `Event` 的 schema 都从这一张表来。
fn event_payloads(g: &mut schemars::SchemaGenerator) -> Vec<(&'static str, Value)> {
    vec![
        (events::MESSAGE_APPENDED, sub::<Message>(g)),
        (events::CHANNEL_CHANGED, sub::<ChannelSummary>(g)),
        (events::CHANNEL_REMOVED, sub::<ChannelRef>(g)),
        (events::CREW_CHANGED, sub::<Crew>(g)),
        (events::CREW_REMOVED, sub::<CrewRef>(g)),
        (events::BADGE_CHANGED, sub::<Badge>(g)),
        (events::ASK_CHANGED, sub::<Ask>(g)),
        (events::APPROVAL_CHANGED, sub::<Approval>(g)),
        (events::SESSION_CHANGED, sub::<SessionLive>(g)),
        (events::RESTART_CHANGED, sub::<RestartPrompt>(g)),
        (events::LAGGED, sub::<Lagged>(g)),
    ]
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
    // json! 一次展开 32 个方法会撞宏递归上限：先列成表再收成对象（输出形状不变）。
    let methods: serde_json::Map<String, Value> = [
        (methods::HELLO, m(sub::<NoParams>(g), sub::<Hello>(g))),
        (
            methods::CREW_LIST,
            m(sub::<NoParams>(g), many(sub::<Crew>(g))),
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
            methods::CREW_DELETE,
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
            methods::APPROVAL_LIST,
            m(sub::<NoParams>(g), many(sub::<Approval>(g))),
        ),
        (
            methods::APPROVAL_DECIDE,
            m(sub::<ApprovalDecideParams>(g), sub::<Approval>(g)),
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
            methods::TODO_LIST,
            m(sub::<TodoListParams>(g), many(sub::<Todo>(g))),
        ),
        (
            methods::SUBSCRIBE,
            m(sub::<SubscribeParams>(g), sub::<Subscribed>(g)),
        ),
    ]
    .into_iter()
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
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "topics": events::TOPICS,
        "frames": frames,
        "methods": methods,
        "events": events,
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
        assert_eq!(s["apiVersion"], 2);
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

    #[test]
    fn every_params_type_denies_unknown_fields_and_vocabularies_are_enums() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        for (m, spec) in s["methods"].as_object().unwrap() {
            let r = spec["params"]["$ref"]
                .as_str()
                .unwrap_or_else(|| panic!("{m} 的 params 不是 $ref"));
            let def = &defs[r.trim_start_matches("#/$defs/")];
            assert_eq!(
                def["additionalProperties"],
                json!(false),
                "{m} 的参数没有 deny_unknown_fields"
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

    /// R11：契约与解析器一致。解析器收 `null` 的方法（无参 / 全可选参数），参数类型在契约里写成 `object | null`；
    /// 其余的不收 null。`restart.pending` 的结果可以是 null。
    #[test]
    fn methods_whose_parser_accepts_null_say_so_in_the_contract() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        let null_ok = [
            methods::HELLO,
            methods::CREW_LIST,
            methods::APPROVAL_LIST,
            methods::RESTART_PENDING,
            methods::SESSION_LIST,
            methods::ASK_LIST,
            methods::DIRECTORY_LIST,
        ];
        for (m, spec) in s["methods"].as_object().unwrap() {
            let r = spec["params"]["$ref"].as_str().unwrap();
            let def = &defs[r.trim_start_matches("#/$defs/")];
            let takes_null = def["type"]
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

    /// `crew.removed` / `channel.removed` / `events.lagged` / 订阅回执有了类型，线上形状与 Task 13 的手写 JSON 相同。
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
