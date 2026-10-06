use crate::AppState;
use crewd_proto::{
    Event, Lagged, Request, Response, RpcError, ServerFrame, SubscribeParams, Subscribed, events,
    methods, parse_params,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// 单行请求的上限；超过且仍没有换行就回错误并断开。
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// 一条连接：读一行请求 → 回一行响应；订阅的事件由独立任务推到同一个写通道。
pub(crate) async fn run(state: Arc<AppState>, stream: UnixStream) -> anyhow::Result<()> {
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

    let mut forwarder: Option<JoinHandle<()>> = None;
    let result = read_loop(&state, rd, &tx, &mut forwarder).await;

    // 无论怎么退出（EOF、读错误、行过长）都要收尾。
    if let Some(f) = forwarder {
        f.abort();
    }
    drop(tx);
    let _ = writer.await;
    result
}

fn parse_error(msg: &str) -> ServerFrame {
    ServerFrame::Response(Response::err(0, RpcError::new(RpcError::PARSE, msg)))
}

async fn read_loop(
    state: &AppState,
    rd: OwnedReadHalf,
    tx: &mpsc::Sender<ServerFrame>,
    forwarder: &mut Option<JoinHandle<()>>,
) -> anyhow::Result<()> {
    let mut rd = BufReader::new(rd);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = (&mut rd)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buf)
            .await?;
        if n == 0 {
            return Ok(());
        }
        let had_newline = buf.last() == Some(&b'\n');
        if !had_newline && buf.len() > MAX_LINE_BYTES {
            let _ = tx.send(parse_error("请求过长")).await;
            return Ok(());
        }
        if had_newline {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }
        let line = match String::from_utf8(std::mem::take(&mut buf)) {
            Ok(l) => l,
            Err(_) => {
                if tx.send(parse_error("请求不是合法 UTF-8")).await.is_err() {
                    return Ok(());
                }
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let frame = match serde_json::from_str::<Request>(&line) {
            Err(e) => parse_error(&format!("无法解析请求: {e}")),
            Ok(req) if req.method == methods::SUBSCRIBE => {
                let id = req.id;
                let p = match parse_params::<SubscribeParams>(req.params)
                    .and_then(|p| check_topics(&p.topics).map(|()| p))
                {
                    Ok(p) => p,
                    Err(e) => {
                        if tx
                            .send(ServerFrame::Response(Response::err(id, e)))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                        continue;
                    }
                };
                // 先订阅，再排队 ack，最后才起转发任务：事件不会抢在 ack 前面，
                // 也不会丢（接收端会缓冲）。重复订阅替换旧的转发任务。
                let rx = state.hub.subscribe();
                let ack = ServerFrame::Response(Response::ok(
                    id,
                    serde_json::to_value(Subscribed {
                        topics: p.topics.clone(),
                    })
                    .unwrap(),
                ));
                if tx.send(ack).await.is_err() {
                    return Ok(());
                }
                if let Some(old) = forwarder.take() {
                    old.abort();
                }
                *forwarder = Some(spawn_forwarder(rx, p.topics, tx.clone()));
                continue;
            }
            Ok(req) => ServerFrame::Response(crate::dispatch::handle(state, req).await),
        };
        if tx.send(frame).await.is_err() {
            return Ok(());
        }
    }
}

/// 主题列表非空，且每个都在 `events::TOPICS` 里。
fn check_topics(topics: &[String]) -> Result<(), RpcError> {
    if topics.is_empty() {
        return Err(RpcError::new(RpcError::INVALID_PARAMS, "topics 不能为空"));
    }
    let unknown: Vec<&String> = topics
        .iter()
        .filter(|t| !events::TOPICS.contains(&t.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            format!(
                "未知主题 {unknown:?}；可订阅: {}",
                events::TOPICS.join(", ")
            ),
        ));
    }
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
                    data: serde_json::to_value(Lagged { missed: n }).unwrap(),
                }),
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    })
}
