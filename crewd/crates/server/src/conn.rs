use crate::AppState;
use crewd_proto::{
    Event, Request, Response, RpcError, ServerFrame, SubscribeParams, events, methods, parse_params,
};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// 一条连接：读一行请求 → 回一行响应；订阅的事件由独立任务推到同一个写通道。
pub async fn run(state: Arc<AppState>, stream: UnixStream) -> anyhow::Result<()> {
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<ServerFrame>(256);
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let mut line = match serde_json::to_vec(&frame) {
                Ok(l) => l,
                Err(_) => continue,
            };
            line.push(b'\n');
            if wr.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut subs: Vec<JoinHandle<()>> = Vec::new();
    let mut lines = BufReader::new(rd).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let frame = match serde_json::from_str::<Request>(&line) {
            Err(e) => ServerFrame::Response(Response::err(
                0,
                RpcError::new(RpcError::PARSE, format!("无法解析请求: {e}")),
            )),
            Ok(req) if req.method == methods::SUBSCRIBE => {
                let id = req.id;
                match parse_params::<SubscribeParams>(req.params) {
                    Ok(p) => {
                        subs.push(spawn_forwarder(
                            state.hub.subscribe(),
                            p.topics.clone(),
                            tx.clone(),
                        ));
                        ServerFrame::Response(Response::ok(id, json!({ "topics": p.topics })))
                    }
                    Err(e) => ServerFrame::Response(Response::err(id, e)),
                }
            }
            Ok(req) => ServerFrame::Response(crate::dispatch::handle(&state, req).await),
        };
        if tx.send(frame).await.is_err() {
            break;
        }
    }
    for s in subs {
        s.abort();
    }
    drop(tx);
    let _ = writer.await;
    Ok(())
}

fn topic_of(event: &str) -> &str {
    event.split('.').next().unwrap_or(event)
}

fn spawn_forwarder(
    mut rx: broadcast::Receiver<Event>,
    topics: Vec<String>,
    tx: mpsc::Sender<ServerFrame>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let frame = match rx.recv().await {
                Ok(e) if topics.iter().any(|t| t == topic_of(&e.event)) => ServerFrame::Event(e),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => ServerFrame::Event(Event {
                    event: events::LAGGED.into(),
                    data: json!({ "missed": n }),
                }),
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    })
}
