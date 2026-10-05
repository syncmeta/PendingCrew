use crate::AppState;
use crewd_proto::{
    API_VERSION, Event, Hello, Message, Request, Response, RpcError, TodoListParams,
    WhiteboardListParams, WhiteboardPostParams, events, methods, parse_params,
};
use serde::Serialize;
use serde_json::Value;

const SENDER_KINDS: &[&str] = &["user", "session", "captain", "pendingcrew"];

pub async fn handle(state: &AppState, req: Request) -> Response {
    let id = req.id;
    match route(state, req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(RpcError::INTERNAL, e.to_string())
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, msg)
}

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

async fn route(state: &AppState, req: Request) -> Result<Value, RpcError> {
    match req.method.as_str() {
        methods::HELLO => to_value(Hello {
            server: "crewd".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
        }),
        methods::CREW_LIST => {
            let s = state.store.lock().await;
            to_value(crewd_store::crews::list(s.conn()).map_err(internal)?)
        }
        methods::WHITEBOARD_LIST => {
            let p: WhiteboardListParams = parse_params(req.params)?;
            let limit = p.limit.unwrap_or(200).clamp(1, 1000);
            let s = state.store.lock().await;
            to_value(
                crewd_store::messages::list(s.conn(), &p.crew_id, p.after_seq.unwrap_or(0), limit)
                    .map_err(internal)?,
            )
        }
        methods::WHITEBOARD_POST => {
            let p: WhiteboardPostParams = parse_params(req.params)?;
            if p.text.trim().is_empty() {
                return Err(invalid("text 不能为空"));
            }
            if !SENDER_KINDS.contains(&p.sender_kind.as_str()) {
                return Err(invalid(format!("未知 senderKind: {}", p.sender_kind)));
            }
            let mut msg = Message {
                seq: 0,
                id: uuid::Uuid::new_v4().to_string(),
                crew_id: p.crew_id,
                sender_kind: p.sender_kind,
                sender_session_id: p.sender_session_id,
                sender_name: p.sender_name,
                text: p.text,
                category: None,
                in_reply_to: p.in_reply_to,
                mentions: p.mentions,
                attachments: vec![],
                created_at: crewd_store::now_ts(),
            };
            {
                let s = state.store.lock().await;
                if !crewd_store::crews::exists(s.conn(), &msg.crew_id).map_err(internal)? {
                    return Err(invalid(format!("机组不存在: {}", msg.crew_id)));
                }
                msg.seq = crewd_store::messages::insert(s.conn(), &msg, None)
                    .map_err(internal)?
                    .ok_or_else(|| internal("新消息 id 冲突"))?;
            }
            let v = to_value(&msg)?;
            state.hub.publish(Event {
                event: events::WHITEBOARD_APPENDED.into(),
                data: v.clone(),
            });
            Ok(v)
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(req.params)?;
            let s = state.store.lock().await;
            to_value(crewd_store::todos::list(s.conn(), &p.crew_id, p.ledger).map_err(internal)?)
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("未知方法: {other}"),
        )),
    }
}
