//! codex app-server 的线协议（Swift `CodexRPCMessage` / `CodexRPCDispatcher` 的移植）。
//!
//! 一行一个 JSON 对象，**没有 `jsonrpc` 字段**（`codex app-server generate-json-schema`
//! 0.160.1 的 JSONRPCRequest / Response / Error / Notification 都不带它）。
//!
//! 两套 id 严格分开：我们发出的请求用我们自己的整数 id（[`Rpc`] 的 pending 表）；服务端发来的
//! 请求带它自己的 id——schema 的 `RequestId` 是 `string | int64`，**两种都要能原样回**
//! （Swift 只认整数：字符串 id 的服务端请求会被当成通知、永远不答，回合挂死）。
//!
//! Swift 的「早到响应缓冲」：它的请求方先写、后在 actor 上登记等待，响应可能先到。这里
//! [`Rpc::request`] **先登记、后交出要写的那一行**，同一个任务按顺序处理每一行输入，
//! 所以「响应比登记早」在结构上不会发生；配不上的响应只剩「超时之后迟到」与「根本不是
//! 我们的 id」两种，一律丢弃并计数（[`Rpc::stray_responses`]）。Swift 没有单请求超时，
//! 这里每个请求都有期限（[`Rpc::expire`]）。
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// 服务端请求的 id：整数或字符串，回的时候原样带回去。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

impl RequestId {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_i64().map(Self::Int),
            Value::String(s) => Some(Self::Str(s.clone())),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Int(i) => json!(i),
            Self::Str(s) => json!(s),
        }
    }
}

/// 服务端回的 error 对象。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 一个请求没拿到结果的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcFailure {
    /// 服务端回了 error。
    Server(RpcError),
    /// 到期没回（已从 pending 表删掉，之后迟到的响应会被丢弃）。
    TimedOut(Duration),
    /// 进程没了 / 管道断了。
    Disconnected,
}

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server(e) => write!(f, "{}（{}）", e.message, e.code),
            Self::TimedOut(d) => write!(f, "{} 秒内没有回应", d.as_secs_f32()),
            Self::Disconnected => write!(f, "codex app-server 已断开"),
        }
    }
}

/// 一行输入的分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// 对我们某个请求的回应（`id` 是我们发出去的那个）。
    Response {
        id: RequestId,
        result: Result<Value, RpcError>,
    },
    /// 服务端请求：**必须回**，否则它在等的那个回合永远挂着。
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 不是 JSON（原文截断后附上）。
    NotJson(String),
    /// 是 JSON 但不是对象，或者认不出是哪一种（有 method 但 id 既不是整数也不是字符串……）。
    Unclassifiable(String),
}

/// 分类一行。行尾的 `\r` 与空白先去掉。有 `method`+`id` = 服务端请求；只有 `method` = 通知；
/// 只有 `id` = 回应（带 `error` 就是失败）。
pub fn parse_line(line: &str) -> Result<Incoming, ParseError> {
    let line = line.trim_end();
    let value: Value =
        serde_json::from_str(line).map_err(|_| ParseError::NotJson(clip(line, 200)))?;
    let Value::Object(mut obj) = value else {
        return Err(ParseError::Unclassifiable(clip(line, 200)));
    };
    let params = obj.remove("params").unwrap_or(Value::Null);
    let id = obj.get("id");
    match obj.get("method").and_then(Value::as_str) {
        Some(method) => match id {
            None => Ok(Incoming::Notification {
                method: method.to_string(),
                params,
            }),
            Some(raw) => match RequestId::from_value(raw) {
                Some(id) => Ok(Incoming::Request {
                    id,
                    method: method.to_string(),
                    params,
                }),
                None => Err(ParseError::Unclassifiable(clip(line, 200))),
            },
        },
        None => {
            let Some(id) = id.and_then(RequestId::from_value) else {
                return Err(ParseError::Unclassifiable(clip(line, 200)));
            };
            let result = match obj.remove("error") {
                Some(err) => Err(RpcError {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("未知错误")
                        .to_string(),
                }),
                None => Ok(obj.remove("result").unwrap_or(Value::Null)),
            };
            Ok(Incoming::Response { id, result })
        }
    }
}

fn clip(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn line(value: Value) -> String {
    // serde_json 把字符串里的换行转义成 `\n`，所以序列化结果本身就是一行。
    serde_json::to_string(&value).expect("Value 一定能序列化")
}

/// 我们的请求：`{"id","method","params"}`。
pub fn request_line(id: i64, method: &str, params: &Value) -> String {
    line(json!({ "id": id, "method": method, "params": params }))
}

/// 我们的通知。`params` 为 `None` 时不带这个键（`initialized` 在 schema 里就没有 params）。
pub fn notification_line(method: &str, params: Option<&Value>) -> String {
    match params {
        Some(p) => line(json!({ "method": method, "params": p })),
        None => line(json!({ "method": method })),
    }
}

/// 回服务端请求：成功。
pub fn response_line(id: &RequestId, result: &Value) -> String {
    line(json!({ "id": id.to_value(), "result": result }))
}

/// 回服务端请求：失败。
pub fn error_line(id: &RequestId, code: i64, message: &str) -> String {
    line(json!({ "id": id.to_value(), "error": { "code": code, "message": message } }))
}

struct Pending<T> {
    tag: T,
    method: String,
    timeout: Duration,
    deadline: Instant,
}

/// 我们发出、还没回的请求。`T` 是调用方给每个请求贴的标签（回应回来时原样交还）。
pub struct Rpc<T> {
    next_id: i64,
    pending: BTreeMap<i64, Pending<T>>,
    stray: u64,
}

impl<T> Default for Rpc<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            pending: BTreeMap::new(),
            stray: 0,
        }
    }
}

impl<T> Rpc<T> {
    /// 登记一个请求，返回要写进 stdin 的那一行（不含换行）。**先登记、后写**。
    pub fn request(
        &mut self,
        tag: T,
        method: &str,
        params: &Value,
        now: Instant,
        timeout: Duration,
    ) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                tag,
                method: method.to_string(),
                timeout,
                deadline: now + timeout,
            },
        );
        request_line(id, method, params)
    }

    /// 收到回应：配上了就交出 `(标签, 方法, 结果)`；配不上（超时后迟到 / 不是我们的 id）
    /// 返回 `None` 并计一笔。
    pub fn resolve(
        &mut self,
        id: &RequestId,
        result: Result<Value, RpcError>,
    ) -> Option<(T, String, Result<Value, RpcFailure>)> {
        let found = match id {
            RequestId::Int(i) => self.pending.remove(i),
            RequestId::Str(_) => None,
        };
        match found {
            Some(p) => Some((p.tag, p.method, result.map_err(RpcFailure::Server))),
            None => {
                self.stray += 1;
                None
            }
        }
    }

    /// 到期的请求：从表里拿掉，交出 `(标签, 方法, TimedOut)`。
    pub fn expire(&mut self, now: Instant) -> Vec<(T, String, RpcFailure)> {
        let due: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        due.into_iter()
            .filter_map(|id| self.pending.remove(&id))
            .map(|p| (p.tag, p.method, RpcFailure::TimedOut(p.timeout)))
            .collect()
    }

    /// 连接断了：所有等待中的请求一起失败。
    pub fn fail_all(&mut self) -> Vec<(T, String, RpcFailure)> {
        std::mem::take(&mut self.pending)
            .into_values()
            .map(|p| (p.tag, p.method, RpcFailure::Disconnected))
            .collect()
    }

    /// 最早的期限（驱动层按它定时）。
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 配不上的回应累计条数。
    pub fn stray_responses(&self) -> u64 {
        self.stray
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_omits_jsonrpc_and_is_a_single_line() {
        let l = request_line(7, "turn/start", &json!({ "text": "a\nb" }));
        assert!(!l.contains("jsonrpc"), "{l}");
        assert!(!l.contains('\n'), "{l}");
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "turn/start");
        assert_eq!(v["params"]["text"], "a\nb");
    }

    #[test]
    fn initialized_has_no_params_key() {
        assert_eq!(
            notification_line("initialized", None),
            r#"{"method":"initialized"}"#
        );
    }

    #[test]
    fn classifies_the_three_shapes() {
        assert_eq!(
            parse_line(r#"{"id":3,"result":{"ok":true}}"#).unwrap(),
            Incoming::Response {
                id: RequestId::Int(3),
                result: Ok(json!({ "ok": true })),
            }
        );
        assert_eq!(
            parse_line(r#"{"id":4,"method":"item/tool/call","params":{"x":1}}"#).unwrap(),
            Incoming::Request {
                id: RequestId::Int(4),
                method: "item/tool/call".into(),
                params: json!({ "x": 1 }),
            }
        );
        assert_eq!(
            parse_line(r#"{"method":"turn/started","params":{"turn":{"id":"t"}}}"#).unwrap(),
            Incoming::Notification {
                method: "turn/started".into(),
                params: json!({ "turn": { "id": "t" } }),
            }
        );
    }

    #[test]
    fn string_request_ids_are_classified_and_echoed_verbatim() {
        let Incoming::Request { id, .. } =
            parse_line(r#"{"id":"srv-1","method":"mcpServer/elicitation/request","params":{}}"#)
                .unwrap()
        else {
            panic!("字符串 id 的服务端请求必须认成请求，不能当通知");
        };
        assert_eq!(id, RequestId::Str("srv-1".into()));
        let reply: Value = serde_json::from_str(&error_line(&id, -32601, "no")).unwrap();
        assert_eq!(reply["id"], "srv-1");
        assert_eq!(reply["error"]["code"], -32601);
        let ok: Value =
            serde_json::from_str(&response_line(&id, &json!({ "action": "accept" }))).unwrap();
        assert_eq!(
            ok,
            json!({ "id": "srv-1", "result": { "action": "accept" } })
        );
    }

    #[test]
    fn error_response_and_garbage() {
        assert_eq!(
            parse_line(r#"{"id":1,"error":{"code":-32600,"message":"bad"}}"#).unwrap(),
            Incoming::Response {
                id: RequestId::Int(1),
                result: Err(RpcError {
                    code: -32600,
                    message: "bad".into(),
                }),
            }
        );
        assert!(matches!(
            parse_line("not json"),
            Err(ParseError::NotJson(_))
        ));
        assert!(matches!(
            parse_line("[1,2]"),
            Err(ParseError::Unclassifiable(_))
        ));
        assert!(matches!(
            parse_line(r#"{"id":null,"method":"x"}"#),
            Err(ParseError::Unclassifiable(_))
        ));
        // 行尾的 \r 不影响。
        assert!(parse_line("{\"method\":\"x\"}\r").is_ok());
    }

    #[test]
    fn our_ids_and_server_ids_live_in_separate_tables() {
        let mut rpc: Rpc<&str> = Rpc::default();
        let now = Instant::now();
        let l = rpc.request(
            "init",
            "initialize",
            &json!({}),
            now,
            Duration::from_secs(5),
        );
        assert!(l.contains(r#""id":0"#), "{l}");
        // 服务端用同一个 0 发请求：分类成请求，不碰我们的表。
        let srv = parse_line(r#"{"id":0,"method":"item/tool/call","params":{}}"#).unwrap();
        assert!(matches!(srv, Incoming::Request { .. }));
        assert_eq!(rpc.pending_len(), 1);
        let (tag, method, result) = rpc.resolve(&RequestId::Int(0), Ok(json!({}))).unwrap();
        assert_eq!(
            (tag, method.as_str(), result),
            ("init", "initialize", Ok(json!({})))
        );
        assert_eq!(rpc.pending_len(), 0);
    }

    #[test]
    fn a_reply_written_back_before_anyone_awaits_is_still_matched() {
        // Swift 的早到缓冲守的场景：回应在「登记等待」之前就到了。这里登记发生在交出那一行之前，
        // 所以哪怕回应紧跟着在同一次 read 里到，也一定配得上。
        let mut rpc: Rpc<u8> = Rpc::default();
        let now = Instant::now();
        let _ = rpc.request(1, "thread/start", &json!({}), now, Duration::from_secs(5));
        let batch =
            "{\"id\":0,\"result\":{\"thread\":{\"id\":\"t\"}}}\n{\"method\":\"thread/started\"}\n";
        let mut matched = 0;
        for l in batch.lines() {
            if let Incoming::Response { id, result } = parse_line(l).unwrap() {
                matched += usize::from(rpc.resolve(&id, result).is_some());
            }
        }
        assert_eq!(matched, 1);
    }

    #[test]
    fn requests_expire_and_late_replies_are_dropped() {
        let mut rpc: Rpc<&str> = Rpc::default();
        let t0 = Instant::now();
        let _ = rpc.request("a", "turn/start", &json!({}), t0, Duration::from_secs(1));
        let _ = rpc.request(
            "b",
            "turn/interrupt",
            &json!({}),
            t0,
            Duration::from_secs(3),
        );
        assert_eq!(rpc.next_deadline(), Some(t0 + Duration::from_secs(1)));
        assert!(rpc.expire(t0 + Duration::from_millis(999)).is_empty());
        let expired = rpc.expire(t0 + Duration::from_secs(1));
        assert_eq!(
            expired,
            vec![(
                "a",
                "turn/start".to_string(),
                RpcFailure::TimedOut(Duration::from_secs(1))
            )]
        );
        assert_eq!(rpc.next_deadline(), Some(t0 + Duration::from_secs(3)));
        // 超时之后迟到的回应：丢弃、计数。
        assert!(rpc.resolve(&RequestId::Int(0), Ok(json!({}))).is_none());
        assert!(
            rpc.resolve(&RequestId::Str("x".into()), Ok(json!({})))
                .is_none()
        );
        assert_eq!(rpc.stray_responses(), 2);
        let failed = rpc.fail_all();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].2, RpcFailure::Disconnected);
        assert_eq!(rpc.next_deadline(), None);
    }

    #[test]
    fn a_string_id_never_resolves_one_of_our_int_requests() {
        let mut rpc: Rpc<&str> = Rpc::default();
        let _ = rpc.request(
            "a",
            "turn/start",
            &json!({}),
            Instant::now(),
            Duration::from_secs(5),
        );
        assert!(
            rpc.resolve(&RequestId::Str("0".into()), Ok(json!({})))
                .is_none()
        );
        assert_eq!(rpc.pending_len(), 1);
        assert_eq!(rpc.stray_responses(), 1);
        assert!(rpc.resolve(&RequestId::Int(0), Ok(json!({}))).is_some());
    }
}
