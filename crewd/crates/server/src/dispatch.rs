use crate::AppState;
use crewd_proto::{
    API_VERSION, Event, Hello, Message, Request, Response, RpcError, TodoListParams,
    WhiteboardListParams, WhiteboardPostParams, events, methods, parse_params,
};
use serde::Serialize;
use serde_json::Value;

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

/// 在库线程上跑 `f`；线程侧的错误（含 panic）与 `f` 自己的错误都映射成 internal。
async fn db<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&mut crewd_store::Store) -> crewd_store::Result<T> + Send + 'static,
) -> Result<T, RpcError> {
    state.db.call(f).await.map_err(internal)?.map_err(internal)
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
        methods::CREW_LIST => to_value(db(state, |s| crewd_store::crews::list(s.conn())).await?),
        methods::WHITEBOARD_LIST => {
            let p: WhiteboardListParams = parse_params(req.params)?;
            let limit = p.limit.unwrap_or(200).clamp(1, 1000);
            let after_seq = p.after_seq.unwrap_or(0);
            to_value(
                db(state, move |s| {
                    crewd_store::messages::list(
                        s.conn(),
                        &crewd_store::channels::lobby_id(&p.crew_id),
                        after_seq,
                        None,
                        limit,
                    )
                })
                .await?,
            )
        }
        methods::WHITEBOARD_POST => {
            let p: WhiteboardPostParams = parse_params(req.params)?;
            if p.text.trim().is_empty() {
                return Err(invalid("text 不能为空"));
            }
            let kind = crewd_store::messages::normalize_sender_kind(&p.sender_kind)
                .ok_or_else(|| invalid(format!("未知 senderKind: {}", p.sender_kind)))?;
            let crew_id = p.crew_id;
            let mut msg = Message {
                seq: 0,
                id: uuid::Uuid::new_v4().to_string(),
                channel_id: crewd_store::channels::lobby_id(&crew_id),
                sender_kind: kind.into(),
                sender_session_id: p.sender_session_id,
                sender_name: p.sender_name,
                text: p.text,
                category: None,
                in_reply_to: p.in_reply_to,
                mentions: p.mentions,
                attachments: vec![],
                created_at: crewd_store::now_ts(),
            };
            // 在库线程里插入并发布：库线程串行 = 事件顺序与 seq 一致。
            let hub = state.hub.clone();
            let posted = state
                .db
                .call(move |s| -> Result<Value, RpcError> {
                    if !crewd_store::crews::exists(s.conn(), &crew_id).map_err(internal)? {
                        return Err(invalid(format!("机组不存在: {crew_id}")));
                    }
                    msg.seq = crewd_store::messages::insert(s.conn(), &msg, None)
                        .map_err(internal)?
                        .ok_or_else(|| internal("新消息 id 冲突"))?;
                    let v = to_value(&msg)?;
                    hub.publish(Event {
                        event: events::WHITEBOARD_APPENDED.into(),
                        data: v.clone(),
                    });
                    Ok(v)
                })
                .await
                .map_err(internal)??;
            Ok(posted)
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(req.params)?;
            to_value(
                db(state, move |s| {
                    crewd_store::todos::list(s.conn(), &p.crew_id, p.ledger)
                })
                .await?,
            )
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("未知方法: {other}"),
        )),
    }
}
