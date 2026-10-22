use crate::AppState;
use crewd_proto::{
    Event, Lagged, Request, Response, RpcError, ServerFrame, SubscribeParams, Subscribed, events,
    methods, parse_params,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
#[cfg(test)]
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
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
    drain: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let policy = match state.peer_policy.get() {
        Some(p) => p.clone(),
        None => Arc::new(crate::peer::PeerPolicy::new(None)?),
    };
    let peer = policy.accept(&stream)?;
    let (rd, wr) = stream.into_split();
    run_io(state, rd, wr, drain, Caller::Local(peer)).await
}

pub(crate) async fn run_remote(
    state: Arc<AppState>,
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    drain: watch::Receiver<bool>,
    principal: crate::remote::Principal,
) -> anyhow::Result<()> {
    let (rd, wr) = tokio::io::split(stream);
    run_io(state, rd, wr, drain, Caller::Remote(principal)).await
}

enum Caller {
    Local(crate::peer::ConnectionPeer),
    Remote(crate::remote::Principal),
}
impl Caller {
    async fn authorize(
        &self,
        state: &Arc<AppState>,
        method: &str,
        drain: &mut watch::Receiver<bool>,
    ) -> Result<(), RpcError> {
        match self {
            Self::Local(peer) => peer.authorize_after_enrollment(method, drain).await,
            Self::Remote(principal) => principal.authorize(state, method).await,
        }
    }
    fn command_actor(&self) -> crate::public_commands::Actor {
        match self {
            Self::Local(peer) => crate::public_commands::Actor::Local(peer.clone()),
            Self::Remote(principal) => crate::public_commands::Actor::Remote(principal.clone()),
        }
    }
    fn remote(&self) -> Option<&crate::remote::Principal> {
        match self {
            Self::Remote(principal) => Some(principal),
            _ => None,
        }
    }
}

async fn run_io<R, W>(
    state: Arc<AppState>,
    rd: R,
    mut wr: W,
    mut drain: watch::Receiver<bool>,
    peer: Caller,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Every exit path (including task abort/panic) closes only this connection's
    // custody/watch lifetime. It does not Stop the remote crew or replay writes.
    let connection_scope = peer.command_actor().connection_scope().clone();
    let connection_guard = connection_scope.guard();
    let writer_scope = connection_scope.clone();
    let remote_writer = peer.remote().cloned();
    let writer_state = state.clone();

    let (tx, mut rx) = mpsc::channel::<ServerFrame>(256);
    let (writer_done_tx, mut writer_done) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let _connection_guard = writer_scope.guard();
        let _done = writer_done_tx;
        let mut changes = remote_writer.as_ref().and_then(|p| p.changes().ok());
        let mut locally_closed = writer_scope.closed_signal();
        'frames: loop {
            if !writer_scope.output_is_live() {
                break;
            }
            let frame = tokio::select! {
                biased;
                _=locally_closed.changed()=>break,
                _ = async { match changes.as_mut() { Some(changes) => { let _ = changes.changed().await; }, None => std::future::pending().await } } => {
                    if let Some(principal) = &remote_writer
                        && principal.check_live(&writer_state).await.is_err() { break; }
                    continue;
                },
                frame = rx.recv() => match frame { Some(frame) => frame, None => break },
            };
            if !writer_scope.output_is_live() {
                break;
            }
            if let Some(principal) = &remote_writer
                && principal.check_live(&writer_state).await.is_err()
            {
                break;
            }
            let mut line = match serde_json::to_vec(&frame) {
                Ok(l) => l,
                Err(_) => continue,
            };
            line.push(b'\n');
            if let Some(principal) = &remote_writer {
                // Keep one write future across unrelated grant changes. Cancelling a
                // partial frame closes this connection; it must never be replayed.
                let write = wr.write_all(&line);
                tokio::pin!(write);
                let deadline = tokio::time::sleep(std::time::Duration::from_secs(5));
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        biased;
                        _ = async { match changes.as_mut() { Some(changes) => { let _ = changes.changed().await; }, None => std::future::pending().await } } => {
                            if principal.check_live(&writer_state).await.is_err() { break 'frames; }
                        },
                        _ = &mut deadline => break 'frames,
                        result = &mut write => {
                            if result.is_err() { break 'frames; }
                            break;
                        },
                    }
                }
            } else {
                // A local signed-client grant revoke/slow-watch close cancels
                // even an already partial frame. Never finish or replay it later.
                let write = wr.write_all(&line);
                tokio::pin!(write);
                tokio::select! {
                    biased;
                    _=locally_closed.changed()=>break,
                    result=tokio::time::timeout(std::time::Duration::from_secs(5),&mut write)=>{
                        if !matches!(result,Ok(Ok(()))){break;}
                    }
                }
            }
        }
    });

    let _writer_guard = AbortOnDrop(writer.abort_handle());
    let mut forwarder: Option<JoinHandle<()>> = None;
    let mut forward_guard: Option<AbortOnDrop> = None;
    let mut lifecycle_exits = tokio::task::JoinSet::new();
    let result = read_loop(
        &state,
        rd,
        &tx,
        &mut forwarder,
        &mut forward_guard,
        ConnectionSignals {
            drain: &mut drain,
            writer_done: &mut writer_done,
        },
        &peer,
        &mut lifecycle_exits,
    )
    .await;

    // Close input/custody before any slow output drain. Retaining this abort
    // guard keeps only already accepted output alive, never manual permission;
    // revoke/slow-client closures independently cancel the writer's output.
    if connection_scope
        .fence_input_until(std::time::Instant::now() + std::time::Duration::from_secs(2))
        .is_err()
    {
        eprintln!("connection manual fence closure remains unknown");
    }
    // Accepted stops already belong to Core. Preserve their bounded terminal
    // replies through server drain, but never retain input authority after EOF.
    while lifecycle_exits.join_next().await.is_some() {}
    // 无论怎么退出（EOF、读错误、行过长）都要收尾。
    if let Some(f) = forwarder {
        if !*drain.borrow() {
            f.abort();
        }
        let _ = f.await;
    }
    drop(tx);
    let _ = writer.await;
    drop(connection_guard);
    result
}

fn parse_error(msg: &str) -> ServerFrame {
    ServerFrame::Response(Response::err(0, RpcError::new(RpcError::PARSE, msg)))
}

struct ConnectionSignals<'a> {
    drain: &'a mut watch::Receiver<bool>,
    writer_done: &'a mut oneshot::Receiver<()>,
}

async fn read_loop<R: AsyncRead + Unpin>(
    state: &Arc<AppState>,
    rd: R,
    tx: &mpsc::Sender<ServerFrame>,
    forwarder: &mut Option<JoinHandle<()>>,
    forward_guard: &mut Option<AbortOnDrop>,
    signals: ConnectionSignals<'_>,
    peer: &Caller,
    lifecycle_exits: &mut tokio::task::JoinSet<()>,
) -> anyhow::Result<()> {
    let ConnectionSignals { drain, writer_done } = signals;
    let mut rd = BufReader::new(rd);
    let mut buf: Vec<u8> = Vec::new();
    // 同一条连接上的 agent.* 请求共享一份状态（attach 过没有、推送任务）；连接结束时随之 drop。
    let mut agent = crate::agent::rpc::AgentConn::default();
    let mut changes = peer
        .remote()
        .map(|principal| principal.changes())
        .transpose()
        .map_err(|_| anyhow::anyhow!("remote connection authorization expired"))?;
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
            _ = &mut *writer_done => return Ok(()),
            _ = async { match changes.as_mut() { Some(changes) => { let _ = changes.changed().await; }, None => std::future::pending().await } } => {
                if let Some(principal) = peer.remote() { principal.check_live(state).await.map_err(|_| anyhow::anyhow!("remote connection authorization expired"))?; }
                continue;
            },
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
            lifecycle_exits,
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
    peer: &Caller,
    agent: &mut crate::agent::rpc::AgentConn,
    tx: &mpsc::Sender<ServerFrame>,
    forwarding: (&mut Option<JoinHandle<()>>, &mut Option<AbortOnDrop>),
    drain: &mut watch::Receiver<bool>,
    lifecycle_exits: &mut tokio::task::JoinSet<()>,
) -> RequestStep {
    let (forwarder, forward_guard) = forwarding;
    // The exact kernel-captured peer remains the sole authority. This executes inside
    // the same request future that stage B catches on every poll, before any fault/effect.
    let denied = match parsed.as_ref() {
        Ok(req) => peer.authorize(state, &req.method, drain).await.err(),
        Err(_) => None,
    };
    let frame = match parsed {
        Err(e) => parse_error(&format!("无法解析请求: {e}")),
        Ok(req) if denied.is_some() => {
            ServerFrame::Response(Response::err(req.id, denied.expect("checked denial")))
        }
        Ok(req) if agent.is_attached() && req.method.starts_with("remote.") => {
            ServerFrame::Response(Response::err(
                req.id,
                RpcError::new(
                    RpcError::FORBIDDEN,
                    "远程配对与设备管理只允许电脑上的人操作。",
                )
                .with_refusal("remote.local_human_only"),
            ))
        }
        Ok(req)
            if matches!(
                req.method.as_str(),
                "remote.enrollmentCreate" | "remote.enrollmentLookup"
            ) =>
        {
            let outcome = match peer.command_actor() {
                crate::public_commands::Actor::Local(local) => match state.remote.get() {
                    Some(remote) => {
                        remote
                            .local_enrollment_route(local, &req.method, req.params)
                            .await
                    }
                    None => Err(RpcError::new(
                        RpcError::METHOD_NOT_FOUND,
                        "remote management unavailable",
                    )),
                },
                crate::public_commands::Actor::Remote(_) => Err(RpcError::new(
                    RpcError::FORBIDDEN,
                    "local Human enrollment management required",
                )),
            };
            ServerFrame::Response(match outcome {
                Ok(value) => Response::ok(req.id, value),
                Err(error) => Response::err(req.id, error),
            })
        }
        Ok(req) if req.method == crewd_proto::commands::SESSION_TERMINAL_WATCH => {
            let id = req.id;
            if agent.is_attached() {
                return RequestStep::Reply(ServerFrame::Response(Response::err(
                    id,
                    RpcError::new(
                        RpcError::FORBIDDEN,
                        "agent cannot read Human terminal watch",
                    ),
                )));
            }
            let p = match parse_params(req.params) {
                Ok(p) => p,
                Err(e) => return RequestStep::Reply(ServerFrame::Response(Response::err(id, e))),
            };
            match crate::public_terminal::watch(state, &peer.command_actor(), p, tx.clone()).await {
                Ok((ack, ready)) => {
                    if tx
                        .send(ServerFrame::Response(Response::ok(
                            id,
                            serde_json::to_value(ack).unwrap(),
                        )))
                        .await
                        .is_err()
                    {
                        return RequestStep::Closed;
                    }
                    let _ = ready.send(());
                    return RequestStep::Continue;
                }
                Err(e) => ServerFrame::Response(Response::err(id, e)),
            }
        }
        Ok(req) if req.method.starts_with("lifecycle.") => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(RpcError::FORBIDDEN, "Agent cannot manage daemon lifecycle"),
                ))
            } else if req.method == "lifecycle.exit" {
                while lifecycle_exits.try_join_next().is_some() {}
                if lifecycle_exits.len() >= 4 {
                    return RequestStep::Reply(ServerFrame::Response(Response::err(
                        req.id,
                        RpcError::new(RpcError::CONFLICT, "stop completion limit reached"),
                    )));
                }
                let id = req.id;
                match crate::lifecycle::begin_exit(state, &peer.command_actor(), req.params).await {
                    Err(error) => ServerFrame::Response(Response::err(id, error)),
                    Ok(pending) => {
                        let state = state.clone();
                        let tx = tx.clone();
                        lifecycle_exits.spawn(async move {
                            let result =
                                catch_request(crate::lifecycle::finish_exit(&state, pending))
                                    .await
                                    .unwrap_or_else(|_| {
                                        Err(crate::dispatch::internal("stop completion panicked"))
                                    });
                            let response = match result {
                                Ok(value) => Response::ok(id, value),
                                Err(error) => Response::err(id, error),
                            };
                            let _ = tx.send(ServerFrame::Response(response)).await;
                        });
                        return RequestStep::Continue;
                    }
                }
            } else {
                let id = req.id;
                let result =
                    crate::lifecycle::handle(state, &peer.command_actor(), &req.method, req.params)
                        .await;
                ServerFrame::Response(match result {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::err(id, e),
                })
            }
        }
        Ok(req) if req.method == crewd_proto::executors::EXECUTOR_LIST => {
            let id = req.id;
            let result = async {
                if agent.is_attached() {
                    return Err(RpcError::new(
                        RpcError::FORBIDDEN,
                        "executor discovery requires Human authority",
                    ));
                }
                let p: crewd_proto::executors::ExecutorListParams =
                    crewd_proto::parse_params(req.params)?;
                let actor = peer.command_actor();
                let owner = state.clone();
                state
                    .db
                    .call_result(move |store| -> crewd_collab::Result<()> {
                        actor.authorize_tx(
                            &owner,
                            store.conn(),
                            crewd_proto::executors::EXECUTOR_LIST,
                        )?;
                        crewd_collab::team_data::authorize(
                            store.conn(),
                            &crewd_collab::Sender::Human,
                            &p.crew_id,
                            false,
                        )?;
                        Ok(())
                    })
                    .await
                    .map_err(crate::dispatch::internal)?
                    .map_err(|_| {
                        RpcError::new(RpcError::FORBIDDEN, "executor crew scope unavailable")
                    })?;
                let profiles = state
                    .orchestra
                    .get()
                    .map(|handle| handle.executor_profiles())
                    .unwrap_or_default();
                serde_json::to_value(crewd_proto::executors::ExecutorListResult { profiles })
                    .map_err(crate::dispatch::internal)
            }
            .await;
            ServerFrame::Response(match result {
                Ok(value) => Response::ok(id, value),
                Err(error) => Response::err(id, error),
            })
        }
        Ok(req) if crate::group_knowledge::METHODS.contains(&req.method.as_str()) => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(
                        RpcError::FORBIDDEN,
                        "group knowledge association requires the Human product entry",
                    ),
                ))
            } else {
                ServerFrame::Response(
                    crate::group_knowledge::handle(state.clone(), peer.command_actor(), req).await,
                )
            }
        }
        Ok(req) if crate::team_data::METHODS.contains(&req.method.as_str()) => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(
                        RpcError::FORBIDDEN,
                        "attached agents require the authenticated data tool surface",
                    ),
                ))
            } else if let Some(runtime) = state.data_runtime.get() {
                ServerFrame::Response(
                    runtime
                        .handle(state.clone(), peer.command_actor(), req)
                        .await,
                )
            } else {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(RpcError::FORBIDDEN, "SQLite knowledge runtime unavailable"),
                ))
            }
        }
        Ok(req) if crewd_proto::judgments::METHODS.contains(&req.method.as_str()) => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(
                        RpcError::FORBIDDEN,
                        "agent cannot access judgment configuration or inspection",
                    ),
                ))
            } else {
                ServerFrame::Response(
                    crate::judgments::handle(state, &peer.command_actor(), req).await,
                )
            }
        }
        Ok(req) if crate::notifications::is_method(&req.method) => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(req.id,RpcError::new(RpcError::FORBIDDEN,"agent cannot configure or register Human notifications")))
            } else {
                ServerFrame::Response(crate::notifications::handle(state,&peer.command_actor(),req).await)
            }
        }
        Ok(req) if crate::public_commands::is_method(&req.method) => {
            if agent.is_attached() {
                ServerFrame::Response(Response::err(
                    req.id,
                    RpcError::new(
                        RpcError::FORBIDDEN,
                        "agent connection cannot submit Human commands",
                    )
                    .with_refusal("command.human_only"),
                ))
            } else {
                ServerFrame::Response(
                    crate::public_commands::handle(state, &peer.command_actor(), req).await,
                )
            }
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
                    .any(|t| matches!(t.as_str(), "documents" | "viewFolders" | "data"))
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
                state.clone(),
                peer.command_actor(),
            );
            *forward_guard = Some(AbortOnDrop(task.abort_handle()));
            *forwarder = Some(task);
            return RequestStep::Continue;
        }
        Ok(req)
            if agent.is_attached()
                && matches!(
                    req.method.as_str(),
                    methods::TODO_ATTRIBUTES_GET
                        | methods::TODO_ASSIGN
                        | methods::TODO_SET_STATE
                        | methods::HUMAN_TASK_CREATE
                        | methods::CHANNEL_WORK_TAGS_SET
                        | methods::RESOURCE_REQUEST_LOOKUP
                        | methods::ASK_GET
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
                        } else if req.method.starts_with("work.") {
                            crewd_collab::refusal::Code::WorkActorForbidden
                        } else {
                            crewd_collab::refusal::Code::ResourceForbidden
                        },
                    ),
                ),
            ))
        }
        Ok(req) if req.method.starts_with("agent.") => {
            #[cfg(any(test, feature = "testkit"))]
            s5_poll_fault(state, req.id).await;
            ServerFrame::Response(
                crate::agent::rpc::handle(
                    state,
                    agent,
                    req,
                    tx,
                    peer.command_actor().connection_scope(),
                )
                .await,
            )
        }

        Ok(req) => {
            #[cfg(any(test, feature = "testkit"))]
            s5_poll_fault(state, req.id).await;
            let id = req.id;
            let is_hello = req.method == methods::HELLO;
            let mut response = crate::dispatch::handle(state, req).await;
            if is_hello
                && let (Some(principal), crewd_proto::Outcome::Result(value)) =
                    (peer.remote(), &mut response.outcome)
                && let Some(object) = value.as_object_mut()
            {
                object.insert(
                    "remoteBinding".into(),
                    serde_json::to_value(&principal.binding).expect("finite authenticated binding"),
                );
            }
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

/// A data hint has no authority of its own. Check the real connection and its
/// current target-crew read ACL on the DB thread immediately before queueing.
/// Malformed/extra fields are dropped rather than leaking a future payload.
async fn data_event_allowed(
    state: &Arc<AppState>,
    actor: &crate::public_commands::Actor,
    data: &serde_json::Value,
) -> bool {
    let Some(object) = data.as_object() else {
        return false;
    };
    if object.len() != 1 {
        return false;
    }
    let Some(crew) = object.get("crewId").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let crew = crew.to_owned();
    let actor = actor.clone();
    let owned_state = state.clone();
    state
        .db
        .call(move |store| {
            actor
                .authorize_tx(&owned_state, store.conn(), "data.workspace.get")
                .is_ok()
                && crewd_collab::team_data::authorize(
                    store.conn(),
                    &crewd_collab::Sender::Human,
                    &crew,
                    false,
                )
                .is_ok()
        })
        .await
        .unwrap_or(false)
}

fn spawn_forwarder(
    mut rx: broadcast::Receiver<Event>,
    topics: Vec<String>,
    tx: mpsc::Sender<ServerFrame>,
    mut drain: watch::Receiver<bool>,
    attached: Arc<std::sync::atomic::AtomicBool>,
    state: Arc<AppState>,
    actor: crate::public_commands::Actor,
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
                            events::TODO_ATTRIBUTES_CHANGED
                                | events::DATA_CHANGED
                                | events::DOCUMENT_CHANGED
                                | events::FOLDER_CHANGED
                                | "channel.workChanged"
                        ) =>
                {
                    continue;
                }
                Ok(e) if topics.iter().any(|t| t == topic_of(&e.event)) => {
                    if e.event == events::DATA_CHANGED
                        && !data_event_allowed(&state, &actor, &e.data).await
                    {
                        continue;
                    }
                    ServerFrame::Event(e)
                }
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
