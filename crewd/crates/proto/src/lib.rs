//! crewd 线协议：客户端（Mac / iOS / CLI）与 crewd 之间的唯一契约。
//! 传输：每行一个 JSON（NDJSON）。客户端发 `Request`，服务端回 `ServerFrame`。
mod model;

pub use model::*;
use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const API_VERSION: u32 = 1;

pub mod methods {
    pub const HELLO: &str = "system.hello";
    pub const CREW_LIST: &str = "crew.list";
    pub const WHITEBOARD_LIST: &str = "whiteboard.list";
    pub const WHITEBOARD_POST: &str = "whiteboard.post";
    pub const TODO_LIST: &str = "todo.list";
    pub const SUBSCRIBE: &str = "events.subscribe";
}

pub mod events {
    /// `events.subscribe` 接受的主题；其他一律 -32602。
    pub const TOPICS: &[&str] = &["whiteboard"];
    pub const WHITEBOARD_APPENDED: &str = "whiteboard.appended";
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

/// 整份 API 契约。`crewd schema` 打印它；仓库里提交一份，测试保证两者一致。
pub fn api_schema() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "methods": {
            methods::HELLO: { "params": null, "result": "Hello" },
            methods::CREW_LIST: { "params": null, "result": "Crew[]" },
            methods::WHITEBOARD_LIST: { "params": "WhiteboardListParams", "result": "Message[]" },
            methods::WHITEBOARD_POST: { "params": "WhiteboardPostParams", "result": "Message" },
            methods::TODO_LIST: { "params": "TodoListParams", "result": "Todo[]" },
            methods::SUBSCRIBE: { "params": "SubscribeParams", "result": "{ topics: string[] }" },
        },
        "events": {
            events::WHITEBOARD_APPENDED: "Message",
            events::LAGGED: "{ missed: number }",
        },
        "types": {
            "Request": schema_for!(Request),
            "Response": schema_for!(Response),
            "Event": schema_for!(Event),
            "Hello": schema_for!(Hello),
            "Crew": schema_for!(Crew),
            "Session": schema_for!(Session),
            "Message": schema_for!(Message),
            "Todo": schema_for!(Todo),
            "WhiteboardListParams": schema_for!(WhiteboardListParams),
            "WhiteboardPostParams": schema_for!(WhiteboardPostParams),
            "TodoListParams": schema_for!(TodoListParams),
            "SubscribeParams": schema_for!(SubscribeParams),
        }
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
            serde_json::from_str(r#"{"event":"whiteboard.appended","data":{}}"#).unwrap();
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
            crew_id: "local-a".into(),
            sender_kind: "user".into(),
            sender_session_id: None,
            sender_name: None,
            text: "你好 👋".into(),
            category: None,
            in_reply_to: Some("m0".into()),
            mentions: vec![Mention {
                kind: "session".into(),
                target_id: Some("worker-1".into()),
            }],
            attachments: vec![],
            created_at: "2026-10-05T00:00:00.000Z".into(),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["crewId"], "local-a");
        assert_eq!(v["inReplyTo"], "m0");
        assert_eq!(v["mentions"][0]["targetId"], "worker-1");
        assert!(v.get("senderName").is_none());
    }

    #[test]
    fn parse_params_reports_invalid_params() {
        let e = parse_params::<WhiteboardListParams>(json!({"limit": 3})).unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
    }

    #[test]
    fn ledger_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Ledger::Human).unwrap(), json!("human"));
        assert_eq!(Ledger::Agent.as_str(), "agent");
    }

    #[test]
    fn api_schema_lists_methods_and_types() {
        let s = api_schema();
        assert_eq!(s["apiVersion"], 1);
        assert!(s["methods"]["whiteboard.post"].is_object());
        assert!(s["types"]["Message"].is_object());
    }
}
