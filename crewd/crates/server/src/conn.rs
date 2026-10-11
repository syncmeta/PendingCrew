use crate::AppState;
use crewd_proto::{
    Event, Lagged, Request, Response, RpcError, ServerFrame, SubscribeParams, Subscribed, events,
    methods, parse_params,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

/// 单行请求的上限；超过且仍没有换行就回错误并断开。
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// 一条连接：读一行请求 → 回一行响应；订阅的事件由独立任务推到同一个写通道。
#[cfg(any(test, feature = "testkit"))]
pub(crate) async fn run(state: Arc<AppState>, stream: UnixStream) -> anyhow::Result<()> {
    let (_tx, rx) = watch::channel(false);
    run_draining(state, stream, rx).await
}

// Force-aborted connection futures must not leave detached writers/watchers.
struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn run_draining(
    state: Arc<AppState>,
    stream: UnixStream,
    mut drain: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let policy = match state.peer_policy.get() {
        Some(p) => p.clone(),
        None => Arc::new(crate::peer::PeerPolicy::new(None)?),
    };
    let peer = policy.accept(&stream)?;
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

    let _writer_guard = AbortOnDrop(writer.abort_handle());
    let mut forwarder: Option<JoinHandle<()>> = None;
    let mut forward_guard: Option<AbortOnDrop> = None;
    let result = read_loop(
        &state,
        rd,
        &tx,
        &mut forwarder,
        &mut forward_guard,
        &mut drain,
        &peer,
    )
    .await;

    // 无论怎么退出（EOF、读错误、行过长）都要收尾。
    if let Some(f) = forwarder {
        if !*drain.borrow() {
            f.abort();
        }
        let _ = f.await;
    }
    drop(tx);
    let _ = writer.await;
    result
}

fn parse_error(msg: &str) -> ServerFrame {
    ServerFrame::Response(Response::err(0, RpcError::new(RpcError::PARSE, msg)))
}

async fn read_loop(
    state: &Arc<AppState>,
    rd: OwnedReadHalf,
    tx: &mpsc::Sender<ServerFrame>,
    forwarder: &mut Option<JoinHandle<()>>,
    forward_guard: &mut Option<AbortOnDrop>,
    drain: &mut watch::Receiver<bool>,
    peer: &crate::peer::ConnectionPeer,
) -> anyhow::Result<()> {
    let mut rd = BufReader::new(rd);
    let mut buf: Vec<u8> = Vec::new();
    // 同一条连接上的 agent.* 请求共享一份状态（attach 过没有、推送任务）；连接结束时随之 drop。
    let mut agent = crate::agent::rpc::AgentConn::default();
    loop {
        buf.clear();
        if *drain.borrow() {
            return Ok(());
        }
        // Only the read is cancellable. Dispatch and its response enqueue finish
        // even when hibernate closes the actor/watch while this RPC is in flight.
        let mut limited = (&mut rd).take(MAX_LINE_BYTES as u64 + 1);
        let n = tokio::select! {
            biased;
            _ = drain.changed() => return Ok(()),
            n = limited.read_until(b'\n', &mut buf) => n?,
        };
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
        let parsed = serde_json::from_str::<Request>(&line);
        let denied = match parsed.as_ref() {
            Ok(req) => peer
                .authorize_after_enrollment(&req.method, drain)
                .await
                .err(),
            Err(_) => None,
        };
        let frame = match parsed {
            Err(e) => parse_error(&format!("无法解析请求: {e}")),
            Ok(req) if denied.is_some() => {
                ServerFrame::Response(Response::err(req.id, denied.expect("checked denial")))
            }
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
                let task = spawn_forwarder(rx, p.topics, tx.clone(), drain.clone());
                *forward_guard = Some(AbortOnDrop(task.abort_handle()));
                *forwarder = Some(task);
                continue;
            }
            Ok(req) if req.method.starts_with("agent.") => {
                ServerFrame::Response(crate::agent::rpc::handle(state, &mut agent, req, tx).await)
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
    mut drain: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Shutdown was published before drain. Consume the already queued
            // broadcasts before dropping this sender, then let the writer flush.
            let received = if *drain.borrow() {
                match rx.try_recv() {
                    Ok(e) => Ok(e),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        Err(broadcast::error::RecvError::Lagged(n))
                    }
                    Err(
                        broadcast::error::TryRecvError::Empty
                        | broadcast::error::TryRecvError::Closed,
                    ) => break,
                }
            } else {
                tokio::select! {
                    received = rx.recv() => received,
                    _ = drain.changed() => continue,
                }
            };
            let frame = match received {
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
