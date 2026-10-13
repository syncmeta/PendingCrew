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
        let id = parsed.as_ref().map_or(0, |req| req.id);
        let step = match catch_request(request_step(
            state,
            parsed,
            peer,
            &mut agent,
            tx,
            (forwarder, forward_guard),
            drain,
        ))
        .await
        {
            Ok(step) => step,
            Err(()) => RequestStep::Reply(ServerFrame::Response(Response::err(
                id,
                crate::dispatch::internal("request handler panicked"),
            ))),
        };
        match step {
            RequestStep::Reply(frame) => {
                if tx.send(frame).await.is_err() {
                    return Ok(());
                }
            }
            RequestStep::Continue => {}
            RequestStep::Closed => return Ok(()),
        }
    }
}

/// Catch every poll, not only construction. Dropping the failed future does not undo commits.
async fn catch_request<F: std::future::Future>(future: F) -> Result<F::Output, ()> {
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Ok(std::task::Poll::Ready(output)) => std::task::Poll::Ready(Ok(output)),
            Err(_) => std::task::Poll::Ready(Err(())),
        }
    })
    .await
}

enum RequestStep {
    Reply(ServerFrame),
    Continue,
    Closed,
}

#[cfg(any(test, feature = "testkit"))]
async fn s5_poll_fault(state: &AppState, id: u64) {
    let armed = {
        let mut fault = state.s5_request_fault.lock().unwrap();
        if *fault == Some(id) {
            fault.take();
            true
        } else {
            false
        }
    };
    if armed {
        tokio::task::yield_now().await;
        panic!("S5 owned request fault");
    }
}

async fn request_step(
    state: &Arc<AppState>,
    parsed: Result<Request, serde_json::Error>,
    peer: &crate::peer::ConnectionPeer,
    agent: &mut crate::agent::rpc::AgentConn,
    tx: &mpsc::Sender<ServerFrame>,
    forwarding: (&mut Option<JoinHandle<()>>, &mut Option<AbortOnDrop>),
    drain: &mut watch::Receiver<bool>,
) -> RequestStep {
    let (forwarder, forward_guard) = forwarding;
    // The exact kernel-captured peer remains the sole authority. This executes inside
    // the same request future that stage B catches on every poll, before any fault/effect.
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
            #[cfg(any(test, feature = "testkit"))]
            s5_poll_fault(state, req.id).await;
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
                        return RequestStep::Closed;
                    }
                    return RequestStep::Continue;
                }
            };
            if agent.is_attached()
                && p.topics
                    .iter()
                    .any(|t| matches!(t.as_str(), "documents" | "viewFolders"))
            {
                return RequestStep::Reply(ServerFrame::Response(Response::err(
                    id,
                    crate::dispatch::refused(
                        RpcError::FORBIDDEN,
                        crewd_collab::refusal::Refusal::new(
                            crewd_collab::refusal::Code::ResourceForbidden,
                        ),
                    ),
                )));
            }
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
                return RequestStep::Closed;
            }
            if let Some(old) = forwarder.take() {
                old.abort();
            }
            let task = spawn_forwarder(
                rx,
                p.topics,
                tx.clone(),
                drain.clone(),
                agent.attached_flag(),
            );
            *forward_guard = Some(AbortOnDrop(task.abort_handle()));
            *forwarder = Some(task);
            return RequestStep::Continue;
        }
        Ok(req)
            if agent.is_attached()
                && matches!(
                    req.method.as_str(),
                    methods::ASK_GET
                        | methods::CHANNEL_WORK_LIST
                        | methods::CHANNEL_WORK_LINK
                        | methods::CHANNEL_WORK_UNLINK
                        | methods::DOCUMENT_CREATE
                        | methods::DOCUMENT_GET
                        | methods::DOCUMENT_LIST
                        | methods::DOCUMENT_UPDATE
                        | methods::DOCUMENT_DELETE
                        | methods::FOLDER_LIST
                        | methods::FOLDER_CREATE
                        | methods::FOLDER_UPDATE
                        | methods::FOLDER_DELETE
                        | methods::FOLDER_ASSIGN
                        | methods::TODO_CREATE
                        | methods::TODO_CLOSE
                        | methods::WORK_REASSIGN_LOST
                        | methods::WORK_TRANSFER_REQUEST
                        | methods::WORK_TRANSFER_ACCEPT
                        | methods::WORK_TRANSFER_WITHDRAW
                )
                || (agent.is_attached()
                    && req.method == methods::WORK_CREATE
                    && (req.params.get("requestId").is_some_and(|v| !v.is_null())
                        || req.params.get("channelId").is_some_and(|v| !v.is_null()))) =>
        {
            ServerFrame::Response(Response::err(
                req.id,
                crate::dispatch::refused(
                    RpcError::FORBIDDEN,
                    crewd_collab::refusal::Refusal::new(
                        if matches!(
                            req.method.as_str(),
                            methods::TODO_CREATE | methods::TODO_CLOSE
                        ) {
                            crewd_collab::refusal::Code::TodoHumanOnly
                        } else {
                            if matches!(
                                req.method.as_str(),
                                methods::ASK_GET
                                    | methods::CHANNEL_WORK_LIST
                                    | methods::CHANNEL_WORK_LINK
                                    | methods::CHANNEL_WORK_UNLINK
                                    | methods::DOCUMENT_CREATE
                                    | methods::DOCUMENT_GET
                                    | methods::DOCUMENT_LIST
                                    | methods::DOCUMENT_UPDATE
                                    | methods::DOCUMENT_DELETE
                                    | methods::FOLDER_LIST
                                    | methods::FOLDER_CREATE
                                    | methods::FOLDER_UPDATE
                                    | methods::FOLDER_DELETE
                                    | methods::FOLDER_ASSIGN
                            ) {
                                crewd_collab::refusal::Code::ResourceForbidden
                            } else {
                                crewd_collab::refusal::Code::WorkActorForbidden
                            }
                        },
                    ),
                ),
            ))
        }
        Ok(req) if req.method.starts_with("agent.") => {
            #[cfg(any(test, feature = "testkit"))]
            s5_poll_fault(state, req.id).await;
            ServerFrame::Response(crate::agent::rpc::handle(state, agent, req, tx).await)
        }

        Ok(req) => {
            #[cfg(any(test, feature = "testkit"))]
            s5_poll_fault(state, req.id).await;
            let id = req.id;
            let response = crate::dispatch::handle(state, req).await;
            #[cfg(test)]
            {
                let armed = {
                    let mut fault = state.s5_after_route_fault.lock().unwrap();
                    if *fault == Some(id) {
                        fault.take();
                        true
                    } else {
                        false
                    }
                };
                if armed {
                    // Actual route completed first; no fabricated commit, retry or rollback.
                    tokio::task::yield_now().await;
                    panic!("S5 owned after-route fault");
                }
            }
            #[cfg(not(test))]
            let _ = id;
            ServerFrame::Response(response)
        }
    };
    RequestStep::Reply(frame)
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
    match event.split('.').next().unwrap_or(event) {
        "document" => "documents",
        "viewFolder" => "viewFolders",
        other => other,
    }
}

fn spawn_forwarder(
    mut rx: broadcast::Receiver<Event>,
    topics: Vec<String>,
    tx: mpsc::Sender<ServerFrame>,
    mut drain: watch::Receiver<bool>,
    attached: Arc<std::sync::atomic::AtomicBool>,
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
                Ok(e)
                    if attached.load(std::sync::atomic::Ordering::Acquire)
                        && matches!(
                            e.event.as_str(),
                            events::DOCUMENT_CHANGED
                                | events::FOLDER_CHANGED
                                | "channel.workChanged"
                        ) =>
                {
                    continue;
                }
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

#[cfg(test)]
mod s5_request_tests {
    use super::*;
    use crewd_proto::Outcome;
    use std::time::Duration;

    async fn read_response(rd: &mut BufReader<OwnedReadHalf>) -> Option<Response> {
        let mut line = String::new();
        match tokio::time::timeout(Duration::from_secs(2), rd.read_line(&mut line)).await {
            Ok(Ok(n)) if n > 0 => serde_json::from_str(&line).ok(),
            _ => None,
        }
    }

    async fn exercise(method: &str, params: serde_json::Value) {
        let db = crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap();
        let state = Arc::new(AppState::new(db.clone()));
        *state.s5_request_fault.lock().unwrap() = Some(41);
        let (client, server) = UnixStream::pair().unwrap();
        let mut task = tokio::spawn(run(state.clone(), server));
        let (rd, mut wr) = client.into_split();
        let mut rd = BufReader::new(rd);
        let request = serde_json::json!({"id":41,"method":method,"params":params});
        let sent = wr.write_all(format!("{request}\n").as_bytes()).await;
        let fault_response = read_response(&mut rd).await;
        let next_sent = wr
            .write_all(b"{\"id\":42,\"method\":\"system.hello\",\"params\":{}}\n")
            .await;
        let next_response = read_response(&mut rd).await;
        drop(wr);
        drop(rd);
        let joined = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
        let normal = matches!(&joined, Ok(Ok(Ok(()))));
        if joined.is_err() {
            task.abort();
            let _ = task.await;
        }
        drop(state);
        let db_closed = tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .is_ok();
        assert!(db_closed, "owned DB shutdown must join before semantics");
        // Primary old-baseline oracle: not an EOF, unknown method or permission refusal.
        let response =
            fault_response.expect("polled panic must return the numbered request response");
        assert_eq!(response.id, 41);
        match response.outcome {
            Outcome::Error(e) => {
                assert_eq!(e.code, RpcError::INTERNAL);
                assert!(e.message.contains('#'));
                assert!(!e.message.contains("S5 owned request fault"));
            }
            other => panic!("panic must be Internal: {other:?}"),
        }
        assert!(sent.is_ok() && next_sent.is_ok());
        let next = next_response.expect("same connection must survive the request panic");
        assert_eq!(next.id, 42);
        assert!(matches!(next.outcome, Outcome::Result(_)));
        assert!(
            normal,
            "connection reader/writer/watchers must clean up normally"
        );
    }
    #[tokio::test]
    async fn ordinary_second_poll_panic_returns_internal_and_next_rpc_works() {
        exercise(methods::HELLO, serde_json::json!({})).await;
    }
    #[tokio::test]
    async fn agent_second_poll_panic_returns_internal_and_next_rpc_works() {
        exercise("agent.unknown", serde_json::json!({})).await;
    }
    #[tokio::test]
    async fn subscribe_second_poll_panic_returns_internal_and_next_rpc_works() {
        exercise(methods::SUBSCRIBE, serde_json::json!({"topics":["system"]})).await;
    }
}

// Proposed conn.rs appended cfg(test) module, after S1/S6/S5 narrow rebase.
// Uses the real captured same-process peer and production conn/dispatch/Hub, no policy override.
#[cfg(test)]
mod s5_recovered_effect_tests {
    use super::*;
    use serde_json::{Value, json};
    use std::time::Duration;
    const LIMIT: Duration = Duration::from_secs(5);

    struct Wire {
        reader: BufReader<OwnedReadHalf>,
        writer: tokio::net::unix::OwnedWriteHalf,
        events: Vec<Value>,
    }
    impl Wire {
        async fn frame(&mut self) -> Result<Value, String> {
            let mut line = String::new();
            let n = tokio::time::timeout(LIMIT, self.reader.read_line(&mut line))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("EOF before functional receipt".into());
            }
            serde_json::from_str(&line).map_err(|e| e.to_string())
        }
        async fn call(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
            let request = json!({"id":id,"method":method,"params":params});
            tokio::time::timeout(
                LIMIT,
                self.writer.write_all(format!("{request}\n").as_bytes()),
            )
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
            let deadline = tokio::time::Instant::now() + LIMIT;
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let frame = tokio::time::timeout(remaining, self.frame())
                    .await
                    .map_err(|e| e.to_string())??;
                if frame.get("event").is_some() {
                    self.events.push(frame);
                } else if frame["id"] == id {
                    return Ok(frame);
                } else {
                    return Err("unexpected actual response ID".into());
                }
            }
        }
        async fn witness_text(&mut self, text: &str) -> Result<(), String> {
            let deadline = tokio::time::Instant::now() + LIMIT;
            while !self
                .events
                .iter()
                .any(|e| e["event"] == events::MESSAGE_APPENDED && e["data"]["text"] == text)
            {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let frame = tokio::time::timeout(remaining, self.frame())
                    .await
                    .map_err(|e| e.to_string())??;
                if frame.get("event").is_none() {
                    return Err("response without outstanding request".into());
                }
                self.events.push(frame);
            }
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn human_committed_post_survives_polled_panic_and_subscription_replacement_once() {
        let fixture = crate::agent::testkit::Fixture::new().await;
        let state = fixture.state.clone();
        let db = state.db.clone();
        let channel = format!("{}/lobby", fixture.crew);
        let (client, server) = UnixStream::pair().unwrap();
        let mut task = tokio::spawn(run(state.clone(), server));
        let (reader, writer) = client.into_split();
        let mut wire = Wire {
            reader: BufReader::new(reader),
            writer,
            events: Vec::new(),
        };
        let observed = async {
            let subscribed = wire
                .call(701, methods::SUBSCRIBE, json!({"topics":["message"]}))
                .await?;
            let old_post = wire
                .call(
                    702,
                    methods::MESSAGE_POST,
                    json!({"channelId":channel,"text":"S5_EXISTING_COMMIT","mentions":[]}),
                )
                .await?;
            wire.witness_text("S5_EXISTING_COMMIT").await?;
            // New test-only after-route fault: real message post and its Tx/Hub finished first.
            *state.s5_after_route_fault.lock().unwrap() = Some(703);
            let fault = wire
                .call(
                    703,
                    methods::MESSAGE_POST,
                    json!({"channelId":channel,"text":"S5_COMMIT_BEFORE_PANIC","mentions":[]}),
                )
                .await?;
            wire.witness_text("S5_COMMIT_BEFORE_PANIC").await?;
            let fault_entered = state.s5_after_route_fault.lock().unwrap().is_none();
            let replaced = wire
                .call(704, methods::SUBSCRIBE, json!({"topics":["message"]}))
                .await?;
            let new_post = wire
                .call(
                    705,
                    methods::MESSAGE_POST,
                    json!({"channelId":channel,"text":"S5_AFTER_RECOVERY","mentions":[]}),
                )
                .await?;
            wire.witness_text("S5_AFTER_RECOVERY").await?;
            let rows = wire
                .call(706, methods::MESSAGE_LIST, json!({"channelId":channel}))
                .await?;
            let hello = wire.call(707, methods::HELLO, json!({})).await?;
            Ok::<_, String>((
                subscribed,
                old_post,
                fault,
                fault_entered,
                replaced,
                new_post,
                rows,
                hello,
            ))
        }
        .await;
        // EOF stops reader, original/ replacement forwarder and writer; collect all queued events.
        let eof_sent = tokio::time::timeout(LIMIT, wire.writer.shutdown()).await;
        let mut tail_error = None;
        let eof_deadline = tokio::time::Instant::now() + LIMIT;
        loop {
            let mut line = String::new();
            match tokio::time::timeout(
                eof_deadline.saturating_duration_since(tokio::time::Instant::now()),
                wire.reader.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(_)) => match serde_json::from_str::<Value>(&line) {
                    Ok(v) if v.get("event").is_some() => wire.events.push(v),
                    other => {
                        tail_error = Some(format!("unexpected cleanup frame {other:?}"));
                        break;
                    }
                },
                other => {
                    tail_error = Some(format!("EOF cleanup {other:?}"));
                    break;
                }
            }
        }
        drop(wire.writer);
        drop(wire.reader);
        let joined = tokio::time::timeout(LIMIT, &mut task).await;
        let normal = matches!(&joined, Ok(Ok(Ok(()))));
        if joined.is_err() {
            task.abort();
            let _ = task.await;
        }
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        assert!(
            closed.is_ok(),
            "owned DB shutdown joined before functional assertions"
        );
        // An old uncaught reader panic is reported after attempted cleanup, not called normal.
        eprintln!("S5 effect cleanup normal={normal}, eof={eof_sent:?}, tail={tail_error:?}");
        let (subscribed, old_post, fault, fault_entered, replaced, new_post, rows, hello) =
            observed.expect("actual complete effect/ subscription observations");
        assert_eq!(fault["id"], 703);
        assert_eq!(fault["error"]["code"], RpcError::INTERNAL);
        assert!(
            fault["error"]["message"]
                .as_str()
                .is_some_and(|s| s.contains('#'))
        );
        assert!(
            fault_entered,
            "authorization refusal cannot replace the actual after-route fault"
        );
        for success in [&subscribed, &old_post, &replaced, &new_post, &rows, &hello] {
            assert!(success.get("error").is_none() && success.get("result").is_some());
        }
        assert!(
            normal && matches!(eof_sent, Ok(Ok(()))) && tail_error.is_none(),
            "normal owned reader/watcher/writer cleanup is required on GREEN"
        );
        let rows = rows["result"].as_array().expect("real message.list rows");
        for text in [
            "S5_EXISTING_COMMIT",
            "S5_COMMIT_BEFORE_PANIC",
            "S5_AFTER_RECOVERY",
        ] {
            assert_eq!(
                rows.iter().filter(|r| r["text"] == text).count(),
                1,
                "already committed post remains exactly once; no automatic retry or rollback promise"
            );
            assert_eq!(
                wire.events
                    .iter()
                    .filter(|e| e["event"] == events::MESSAGE_APPENDED && e["data"]["text"] == text)
                    .count(),
                1,
                "original and replaced real subscription each deliver the actual committed event once"
            );
        }
    }
}
