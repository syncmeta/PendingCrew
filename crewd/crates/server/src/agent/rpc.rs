//! agent RPC (spec 22.3). One `AgentConn` per connection: only after a successful attach can the other
//! methods be called; the role is re-read on every call; when the role changes `agent.tools_changed`
//! is pushed (the helper turns it into an MCP list_changed).
use std::sync::Arc;

use crewd_collab::Change;
use crewd_proto::agent::{
    AttachParams, AttachResult, CallParams, HookKind, HookParams, HookResult, TOOLS_CHANGED,
    ToolsChanged, methods,
};
use crewd_proto::{Event, Request, Response, RpcError, ServerFrame, parse_params};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::AppState;
use crate::agent::{Identity, Role, catalog, port, tools};
use crate::dispatch::internal;

#[derive(Default)]
pub struct AgentConn {
    session: Option<String>,
    watcher: Option<JoinHandle<()>>,
}

impl AgentConn {
    pub(crate) fn is_attached(&self) -> bool {
        self.session.is_some()
    }
}

impl Drop for AgentConn {
    fn drop(&mut self) {
        if let Some(w) = self.watcher.take() {
            w.abort();
        }
    }
}

/// Token / session do not match: -32003 (M3a FORBIDDEN). One message for every cause, so the reply
/// does not tell a prober whether the session exists.
fn invalid() -> RpcError {
    RpcError::new(RpcError::FORBIDDEN, "invalid session or token")
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

pub async fn handle(
    state: &Arc<AppState>,
    conn: &mut AgentConn,
    req: Request,
    tx: &mpsc::Sender<ServerFrame>,
) -> Response {
    let id = req.id;
    match route(state, conn, req, tx).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

/// Identity of the attached session right now. A session deleted after attach is the same
/// condition as a bad attach (-32003); a store failure is internal (-32603 + log reference).
async fn current(state: &AppState, session: &str) -> Result<Identity, RpcError> {
    port::identity(state, session)
        .await
        .map_err(internal)?
        .ok_or_else(invalid)
}

async fn route(
    state: &Arc<AppState>,
    conn: &mut AgentConn,
    req: Request,
    tx: &mpsc::Sender<ServerFrame>,
) -> Result<Value, RpcError> {
    if req.method == methods::ATTACH {
        let p: AttachParams = parse_params(req.params)?;
        if !port::verify_token(state, &p.session_id, &p.token) {
            return Err(invalid());
        }
        // One connection serves one session: re-attaching as the same session is fine (it just
        // restarts the push task), attaching as another one is a conflict.
        if conn.session.as_deref().is_some_and(|s| s != p.session_id) {
            return Err(RpcError::new(
                RpcError::CONFLICT,
                "this connection is already attached to another session",
            ));
        }
        // Subscribe before reading the identity: a captain change between the two is not missed.
        let rx = port::subscribe_collab(state);
        let me = current(state, &p.session_id).await?;
        if let Some(old) = conn.watcher.take() {
            old.abort();
        }
        conn.watcher = Some(spawn_watcher(
            state.clone(),
            me.session_id.clone(),
            me.role,
            me.crew_id.clone(),
            rx,
            tx.clone(),
        ));
        conn.session = Some(me.session_id.clone());
        let loc = port::locale(state).await.unwrap_or_default();
        return to_value(AttachResult {
            role: me.role,
            crew_id: me.crew_id,
            label: me.label,
            instructions: crate::agent::text::get(loc, "instructions").into(),
            surface_hash: crate::agent::surface::hash(loc),
            build_check: Some({
                let status = crate::build_info::compare_helper_build(
                    &crate::build_info::build_info(),
                    p.helper_build.as_ref(),
                );
                let warning = (status == crewd_proto::agent::HelperBuildStatus::Skew)
                    .then(|| crate::agent::text::get(loc, "r.helper_build_skew").to_string());
                crewd_proto::agent::HelperBuildCheck { status, warning }
            }),
        });
    }
    let Some(session) = conn.session.clone() else {
        return Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "agent.* requires agent.attach first",
        ));
    };
    match req.method.as_str() {
        methods::TOOLS => {
            let me = current(state, &session).await?;
            let loc = port::locale(state).await.unwrap_or_default();
            to_value(catalog::tools_for(&me, loc))
        }
        methods::CALL => {
            let p: CallParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            to_value(tools::call(state, &me, &p.tool, p.args).await)
        }
        methods::HOOK => {
            let p: HookParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            let result = hook(state, &me, p.event, p.payload.clone()).await;
            #[cfg(any(test, feature = "testkit"))]
            crate::agent::testkit::record_hook(state, &me.session_id, p.event, &p.payload, &result);
            to_value(result)
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

async fn hook(state: &Arc<AppState>, me: &Identity, event: HookKind, payload: Value) -> HookResult {
    match event {
        HookKind::PostToolUse => crate::agent::inject::post_tool_use(state, me, &payload).await,
        HookKind::Stop => crate::agent::turn::stop(state, me, &payload).await,
        HookKind::SessionStart => match port::live_world(state, &me.session_id).await {
            Ok(context) => HookResult { stdout: Some(serde_json::json!({
                "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context },
            }).to_string()) },
            Err(e) => {
                eprintln!("crewd: session-start for {}: {e}", me.session_id);
                HookResult { stdout: None }
            }
        },
    }
}

/// A role only changes when my own session row or my crew's row changes (a captain change is
/// `crews::set_captain`, which emits CrewChanged for the crew and SessionChanged for the two
/// sessions involved). Anything else is not worth a database read.
pub(crate) fn relevant(c: &Change, session: &str, crew: &str) -> bool {
    match c {
        Change::SessionChanged { session_id } => session_id == session,
        Change::CrewChanged { crew_id } => crew_id == crew,
        _ => false,
    }
}

fn spawn_watcher(
    state: Arc<AppState>,
    session: String,
    mut last: Role,
    mut crew: String,
    mut rx: broadcast::Receiver<Change>,
    tx: mpsc::Sender<ServerFrame>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(c) if relevant(&c, &session, &crew) => {}
                Ok(_) => continue,
                // Missed some changes: re-read the role to be safe.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
            let Ok(Some(me)) = port::identity(&state, &session).await else {
                continue;
            };
            // A session can move to another crew: follow it.
            crew = me.crew_id.clone();
            if me.role != last {
                last = me.role;
                let data = serde_json::to_value(ToolsChanged { role: me.role })
                    .expect("ToolsChanged serialises");
                let frame = ServerFrame::Event(Event {
                    event: TOOLS_CHANGED.to_string(),
                    data,
                });
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    mod s6_build_tests;
    mod s72_tests;

    use std::time::Duration;

    use crewd_proto::agent::{TOOLS_CHANGED, methods};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::rpc::relevant;
    use crate::agent::testkit::{Fixture, RpcClient};
    use crate::agent::text::{self, Locale};
    use crewd_collab::Change;

    async fn attached(f: &Fixture, session: &str) -> RpcClient {
        let sock = f.spawn_socket().await;
        let mut c = RpcClient::connect(&sock).await;
        c.call(
            methods::ATTACH,
            json!({"sessionId": session, "token": f.token(session)}),
        )
        .await
        .unwrap();
        c
    }

    #[tokio::test]
    async fn agent_methods_need_attach() {
        let f = Fixture::new().await;
        let mut c = RpcClient::connect(&f.spawn_socket().await).await;
        for m in [methods::TOOLS, methods::CALL, methods::HOOK] {
            let (code, _) = c.call(m, json!({})).await.unwrap_err();
            assert_eq!(code, -32601, "{m}");
        }
        let (code, _) = c
            .call(
                methods::ATTACH,
                json!({"sessionId": f.worker, "token": "forged"}),
            )
            .await
            .unwrap_err();
        assert_eq!(code, -32003);
        let other = f.token(&f.captain);
        let (code, _) = c
            .call(
                methods::ATTACH,
                json!({"sessionId": f.worker, "token": other}),
            )
            .await
            .unwrap_err();
        assert_eq!(code, -32003, "别人的 token 不能冒用");
        let (code, _) = c.call(methods::TOOLS, json!({})).await.unwrap_err();
        assert_eq!(code, -32601, "attach 失败后仍是未 attach");
    }

    async fn requirement_readable(f: &Fixture, c: &mut RpcClient) {
        let detail=f.rpc("todo.create",json!({"requestId":"2188d9b0-cedb-4ed9-9b5e-ff8a73cf74fd","crewId":f.crew,"text":"role-current-source"})).await;
        let tools = c.call(methods::TOOLS, json!(null)).await.unwrap();
        let get = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "todo_get")
            .unwrap();
        assert_eq!(get["annotations"]["readOnlyHint"], true);
        let read = c
            .call(
                methods::CALL,
                json!({"tool":"todo_get","args":{"id":detail["requirementId"]}}),
            )
            .await
            .unwrap();
        assert_eq!(read["isError"], false, "{read}");
        assert!(
            read["text"]
                .as_str()
                .unwrap()
                .contains("role-current-source")
        );
    }
    #[tokio::test]
    async fn attach_reports_identity_and_tools_follow_role() {
        let f = Fixture::new().await;
        let sock = f.spawn_socket().await;
        let mut w = RpcClient::connect(&sock).await;
        let a = w
            .call(
                methods::ATTACH,
                json!({"sessionId": f.worker, "token": f.token(&f.worker)}),
            )
            .await
            .unwrap();
        assert_eq!(
            a,
            json!({"role": "worker", "crewId": f.crew, "label": "示例 worker",
                "instructions": text::get(Locale::Zh, "instructions"),
                "surfaceHash": crate::agent::surface::hash(Locale::Zh),
                "buildCheck": {"status":"unknown"}})
        );
        assert_eq!(
            w.call(methods::TOOLS, json!(null))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            28
        );
        let mut cap = attached(&f, &f.captain).await;
        assert_eq!(
            cap.call(methods::TOOLS, json!(null))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            39
        );
        requirement_readable(&f, &mut w).await;
        requirement_readable(&f, &mut cap).await;
    }

    #[tokio::test]
    async fn worker_calling_captain_tool_is_refused_with_md_text() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let r = w
            .call(methods::CALL, json!({"tool": "list_sessions", "args": {}}))
            .await
            .unwrap();
        assert_eq!(r["isError"], true);
        assert_eq!(
            r["text"],
            text::r(Locale::Zh, "r.captain_only", &[("name", "list_sessions")])
        );
        let r = w
            .call(methods::CALL, json!({"tool": "plan_add"}))
            .await
            .unwrap();
        assert_eq!(
            r["text"],
            text::r(
                Locale::Zh,
                "r.unknown_tool",
                &[("name", "plan_add"), ("hint", "")]
            )
        );
    }

    #[tokio::test]
    async fn role_change_pushes_tools_changed_and_takes_effect_without_restart() {
        // Review Focus 4
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        requirement_readable(&f, &mut w).await;
        f.set_role(&f.worker, Role::Captain).await;
        let ev = w
            .next_event(Duration::from_secs(5))
            .await
            .expect("应推 tools_changed");
        assert_eq!(ev.event, TOOLS_CHANGED);
        assert_eq!(ev.data["role"], "captain");
        assert_eq!(
            w.call(methods::TOOLS, json!(null))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            39
        );
        let r = w
            .call(methods::CALL, json!({"tool": "list_sessions"}))
            .await
            .unwrap();
        assert_ne!(
            r["text"],
            text::r(Locale::Zh, "r.captain_only", &[("name", "list_sessions")]),
            "换成机长后马上能用"
        );
        requirement_readable(&f, &mut w).await;
        f.set_role(&f.worker, Role::Worker).await;
        let ev = w.next_event(Duration::from_secs(5)).await.unwrap();
        assert_eq!(ev.data["role"], "worker");
        let r = w
            .call(methods::CALL, json!({"tool": "list_sessions"}))
            .await
            .unwrap();
        assert_eq!(r["isError"], true, "卸任后当场被拒，不靠 helper 刷新");
        requirement_readable(&f, &mut w).await;
    }

    #[tokio::test]
    async fn stop_hook_over_the_wire_relays_a_trailing_question_and_prints_nothing() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let lobby = crate::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        let stop = |prompt: &str, text: &str| json!({"event": "stop", "payload": {"hook_event_name": "Stop", "prompt_id": prompt, "last_assistant_message": text}});
        let r = w.call(methods::HOOK, stop("p0", "收到。")).await.unwrap();
        assert!(r["stdout"].is_null());
        let before = crate::agent::port::latest_seq(&f.state, &lobby.id)
            .await
            .unwrap();
        let r = w
            .call(methods::HOOK, stop("p1", "要不要接着做 B？"))
            .await
            .unwrap();
        assert!(r["stdout"].is_null());
        assert_ne!(
            crate::agent::port::latest_seq(&f.state, &lobby.id)
                .await
                .unwrap(),
            before,
            "Stop must reach turn_trace through the RPC"
        );
    }

    #[tokio::test]
    async fn unrelated_events_do_not_push() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let lobby = crate::agent::port::lobby_of(&f.state, &f.crew)
            .await
            .unwrap();
        f.human_post(&lobby.id, "示例消息", vec![]).await;
        assert!(w.next_event(Duration::from_millis(300)).await.is_none());
    }

    #[tokio::test]
    async fn push_survives_a_lagged_receiver() {
        // The collab broadcast holds 4096 changes. One transaction that changes the captain and then
        // notes more distinct changes than that overflows the watcher's receiver, so the
        // CrewChanged that matters is dropped: only the Lagged re-read can still find the new role.
        // The transaction runs on its own thread and runtime and the test thread blocks on it, so
        // the watcher cannot drain the queue while events are being emitted.
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let (st, crew, worker) = (f.state.clone(), f.crew.clone(), f.worker.clone());
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(st.collab(move |tx| {
                    crewd_collab::crews::set_captain(tx, &crew, &worker)?;
                    for i in 0..4200 {
                        tx.note(Change::Progress {
                            session_id: format!("noise-{i}"),
                            kind: crewd_collab::ProgressKind::ToolCall,
                        });
                    }
                    Ok(())
                }))
                .unwrap();
        })
        .join()
        .unwrap();
        let ev = w
            .next_event(Duration::from_secs(5))
            .await
            .expect("tools_changed must survive a Lagged receiver");
        assert_eq!(ev.event, TOOLS_CHANGED);
        assert_eq!(ev.data["role"], "captain");
    }

    #[tokio::test]
    async fn second_attach_for_another_session_is_refused_same_session_is_fine() {
        let f = Fixture::new().await;
        let mut c = attached(&f, &f.worker).await;
        let (code, _) = c
            .call(
                methods::ATTACH,
                json!({"sessionId": f.captain, "token": f.token(&f.captain)}),
            )
            .await
            .unwrap_err();
        assert_eq!(code, -32009);
        // Still the worker: the refused attach changed nothing.
        let n = c.call(methods::TOOLS, json!(null)).await.unwrap();
        assert_eq!(n.as_array().unwrap().len(), 28);
        let a = c
            .call(
                methods::ATTACH,
                json!({"sessionId": f.worker, "token": f.token(&f.worker)}),
            )
            .await
            .unwrap();
        assert_eq!(a["role"], "worker");
    }

    #[tokio::test]
    async fn a_session_deleted_after_attach_is_refused_with_32003() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let id = f.worker.clone();
        f.state
            .db
            .call(move |s| {
                s.conn()
                    .execute("DELETE FROM sessions WHERE id=?1", [&id])
                    .unwrap()
            })
            .await
            .unwrap();
        for m in [methods::TOOLS, methods::CALL] {
            let (code, _) = w
                .call(m, json!({"tool": "post", "args": {}}))
                .await
                .unwrap_err();
            assert_eq!(code, -32003, "{m}");
        }
    }

    #[test]
    fn only_my_session_or_my_crew_changes_can_change_my_role() {
        use crewd_collab::Change as C;
        let s = |id: &str| C::SessionChanged {
            session_id: id.into(),
        };
        let c = |id: &str| C::CrewChanged { crew_id: id.into() };
        assert!(relevant(&s("me"), "me", "crew-1"));
        assert!(relevant(&c("crew-1"), "me", "crew-1"));
        assert!(!relevant(&s("someone"), "me", "crew-1"));
        assert!(!relevant(&c("crew-2"), "me", "crew-1"));
        assert!(!relevant(
            &C::ChannelChanged {
                channel_id: "x".into()
            },
            "me",
            "crew-1"
        ));
    }
}
