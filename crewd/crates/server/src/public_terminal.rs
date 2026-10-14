//! Re-authorized bounded observation, never completion or write authority.
use crate::{AppState, public_commands::Actor};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_proto::{Event, RpcError, ServerFrame, commands::*};
use serde_json::{Value, json};
use std::sync::Arc;
fn unavailable() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "native terminal observation unavailable",
    )
    .with_refusal("terminal.unavailable")
}
fn cursor(generation: &str, epoch: u64, sequence: u64) -> String {
    format!("{generation}/{epoch}/{sequence}")
}
fn parse_cursor(value: &str, generation: &str, epoch: u64) -> Option<u64> {
    let mut p = value.split('/');
    if p.next()? != generation || p.next()?.parse::<u64>().ok()? != epoch {
        return None;
    }
    let seq = p.next()?.parse().ok()?;
    if p.next().is_some() {
        return None;
    }
    Some(seq)
}
async fn current(
    state: &Arc<AppState>,
    actor: &Actor,
    session: &str,
) -> Result<CommandRuntimeTarget, RpcError> {
    actor.authorize(state, "session.inspect").await?;
    state
        .orchestra
        .get()
        .ok_or_else(unavailable)?
        .public_target(session)
        .await
        .map_err(crate::orchestra::rpc::rpc_err)
}
pub(crate) async fn inspect(
    state: &Arc<AppState>,
    actor: &Actor,
    p: TerminalInspectParams,
) -> Result<Value, RpcError> {
    actor.authorize(state, SESSION_TERMINAL_INSPECT).await?;
    let target = current(state, actor, &p.session_id).await?;
    let driver = state
        .orchestra
        .get()
        .ok_or_else(unavailable)?
        .terminal_driver(&p.session_id)
        .await
        .map_err(crate::orchestra::rpc::rpc_err)?;
    let runtime = state
        .native_helpers
        .get()
        .ok_or_else(unavailable)?
        .terminal_for(&p.session_id, driver)
        .await
        .map_err(|_| unavailable())?;
    let view = runtime.snapshot().await.map_err(|_| unavailable())?;
    let binding = runtime.binding().map_err(|_| unavailable())?;
    if view["epoch"] != binding.epoch
        || current(state, actor, &p.session_id).await?.generation != target.generation
    {
        return Err(unavailable());
    }
    let sequence = view["sequence"].as_u64().ok_or_else(unavailable)?;
    let truncated = view["truncated"].as_bool().ok_or_else(unavailable)?;
    let oldest = runtime.oldest_sequence(binding.epoch).unwrap_or(sequence);
    Ok(
        json!({"target":target,"outputKind":"pty","screen":null,"oldestCursor":cursor(runtime.generation(),binding.epoch,oldest.saturating_sub(1)),"nextCursor":cursor(runtime.generation(),binding.epoch,sequence),"truncated":truncated,"exited":false,"manualControlAvailable":runtime.is_live(),"cols":view["cols"],"rows":view["rows"],"screenBytesBase64":view["bytesBase64"]}),
    )
}
pub(crate) async fn watch(
    state: &Arc<AppState>,
    actor: &Actor,
    p: TerminalWatchParams,
    tx: tokio::sync::mpsc::Sender<ServerFrame>,
) -> Result<(TerminalWatchStarted, tokio::sync::oneshot::Sender<()>), RpcError> {
    actor.authorize(state, SESSION_TERMINAL_WATCH).await?;
    let target = current(state, actor, &p.target.session_id).await?;
    if target != p.target {
        return Err(unavailable());
    }
    let driver = state
        .orchestra
        .get()
        .ok_or_else(unavailable)?
        .terminal_driver(&target.session_id)
        .await
        .map_err(crate::orchestra::rpc::rpc_err)?;
    let runtime = state
        .native_helpers
        .get()
        .ok_or_else(unavailable)?
        .terminal_for(&target.session_id, driver)
        .await
        .map_err(|_| unavailable())?;
    let binding = runtime.binding().map_err(|_| unavailable())?;
    // Subscribe BEFORE capturing a bounded retained cut; pushes cannot race
    // ahead of their ACK and buffered frames are deduplicated by exact cursor.
    let mut output = runtime.terminal();
    let (after, initial_snapshot) = if let Some(value) = p.after.as_deref() {
        (
            parse_cursor(value, runtime.generation(), binding.epoch).ok_or_else(unavailable)?,
            None,
        )
    } else {
        let initial = inspect(
            state,
            actor,
            TerminalInspectParams {
                session_id: target.session_id.clone(),
            },
        )
        .await?;
        let snapshot: TerminalInspection =
            serde_json::from_value(initial).map_err(|_| unavailable())?;
        if snapshot.truncated || snapshot.target != target {
            return Err(unavailable());
        }
        let cut = parse_cursor(&snapshot.next_cursor, runtime.generation(), binding.epoch)
            .ok_or_else(unavailable)?;
        (cut, Some(snapshot))
    };
    let retained = runtime
        .after(binding.epoch, after)
        .map_err(|_| unavailable())?;
    let scope = actor.connection_scope().clone();
    let watch_id = uuid::Uuid::new_v4().to_string();
    let ack = TerminalWatchStarted {
        watch_id: watch_id.clone(),
        target: target.clone(),
        after: cursor(runtime.generation(), binding.epoch, after),
        initial_snapshot,
    };
    let (ready, start) = tokio::sync::oneshot::channel();
    let state = state.clone();
    let actor = actor.clone();
    let task_scope = scope.clone();
    let task = tokio::spawn(async move {
        if start.await.is_err() {
            return;
        }
        let mut next = after;
        let mut initial = retained.into_iter();
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            let frame = match initial.next() {
                Some(frame) => Some(frame),
                None => tokio::select! {
                    frame=output.recv()=>frame.ok(),
                    _=heartbeat.tick()=>{
                        if runtime.is_live()
                            && task_scope.is_live()
                            && actor.authorize(&state,SESSION_TERMINAL_WATCH).await.is_ok()
                            && current(&state,&actor,&target.session_id).await.is_ok_and(|now|now.generation==target.generation)
                        {continue;}
                        None
                    },
                },
            };
            let Some(frame) = frame else {
                if task_scope.is_live()
                    && actor
                        .authorize(&state, SESSION_TERMINAL_WATCH)
                        .await
                        .is_ok()
                {
                    if let Ok(now) = current(&state, &actor, &target.session_id).await {
                        let output = TerminalOutput {
                            target: now,
                            cursor: cursor(runtime.generation(), binding.epoch, next),
                            next_cursor: cursor(runtime.generation(), binding.epoch, next),
                            bytes_base64: String::new(),
                            truncated: true,
                            exited: runtime.original_gone(),
                        };
                        let event = TerminalWatchEvent {
                            watch_id: watch_id.clone(),
                            output,
                            closed: true,
                        };
                        if tx
                            .try_send(ServerFrame::Event(Event {
                                event: TERMINAL_OUTPUT_EVENT.into(),
                                data: serde_json::to_value(event).unwrap_or(Value::Null),
                            }))
                            .is_err()
                        {
                            task_scope.close();
                        }
                    }
                }
                break;
            };
            if !task_scope.is_live()
                || actor
                    .authorize(&state, SESSION_TERMINAL_WATCH)
                    .await
                    .is_err()
            {
                break;
            }
            let Ok(now) = current(&state, &actor, &target.session_id).await else {
                break;
            };
            if now.generation != target.generation || frame.epoch != binding.epoch {
                break;
            }
            if frame.sequence <= next {
                continue;
            }
            let gap = frame.sequence != next.saturating_add(1)
                || frame.kind == crewd_exec::acp::wire::TerminalKind::Resync;
            let data = TerminalOutput {
                target: now,
                cursor: cursor(runtime.generation(), binding.epoch, next),
                next_cursor: cursor(runtime.generation(), binding.epoch, frame.sequence),
                bytes_base64: if gap {
                    String::new()
                } else {
                    STANDARD.encode(&frame.bytes)
                },
                truncated: gap,
                exited: false,
            };
            let event = TerminalWatchEvent {
                watch_id: watch_id.clone(),
                output: data,
                closed: gap,
            };
            let observed_sequence = frame.sequence;
            let frame = ServerFrame::Event(Event {
                event: TERMINAL_OUTPUT_EVENT.into(),
                data: serde_json::to_value(event).unwrap_or(Value::Null),
            });
            // No full/stalled viewer can block the owning PTY or another watch.
            if tx.try_send(frame).is_err() {
                task_scope.close();
                break;
            }
            if gap {
                break;
            }
            next = observed_sequence;
        }
        task_scope.remove_watch(&watch_id);
    });
    scope
        .add_watch(ack.watch_id.clone(), task.abort_handle())
        .map_err(|_| unavailable())?;
    Ok((ack, ready))
}
