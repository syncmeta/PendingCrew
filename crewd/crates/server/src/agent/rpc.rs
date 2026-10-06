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
            rx,
            tx.clone(),
        ));
        conn.session = Some(me.session_id.clone());
        return to_value(AttachResult {
            role: me.role,
            crew_id: me.crew_id,
            label: me.label,
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
            to_value(hook(state, &me, p.event, p.payload).await)
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

/// Tasks 9-11 wire the three hooks in here.
async fn hook(
    _state: &Arc<AppState>,
    _me: &Identity,
    event: HookKind,
    _payload: Value,
) -> HookResult {
    match event {
        HookKind::PostToolUse | HookKind::PreToolUse | HookKind::Stop => {
            HookResult { stdout: None }
        }
    }
}

/// A role only changes when a session row or a crew row changes (a captain change is
/// `crews::set_captain`, which emits CrewChanged / SessionChanged).
fn relevant(c: &Change) -> bool {
    matches!(
        c,
        Change::SessionChanged { .. } | Change::CrewChanged { .. }
    )
}

fn spawn_watcher(
    state: Arc<AppState>,
    session: String,
    mut last: Role,
    mut rx: broadcast::Receiver<Change>,
    tx: mpsc::Sender<ServerFrame>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(c) if relevant(&c) => {}
                Ok(_) => continue,
                // Missed some changes: re-read the role to be safe.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
            let Ok(Some(me)) = port::identity(&state, &session).await else {
                continue;
            };
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
    use std::time::Duration;

    use crewd_proto::agent::{TOOLS_CHANGED, methods};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::testkit::{Fixture, RpcClient};
    use crate::agent::text::{self, Locale};

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
            json!({"role": "worker", "crewId": f.crew, "label": "示例 worker"})
        );
        assert_eq!(
            w.call(methods::TOOLS, json!(null))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            21
        );
        let mut cap = attached(&f, &f.captain).await;
        assert_eq!(
            cap.call(methods::TOOLS, json!(null))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            40
        );
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
            text::r(Locale::Zh, "r.unknown_tool", &[("name", "plan_add")])
        );
    }

    #[tokio::test]
    async fn role_change_pushes_tools_changed_and_takes_effect_without_restart() {
        // Review Focus 4
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
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
            40
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
        f.set_role(&f.worker, Role::Worker).await;
        let ev = w.next_event(Duration::from_secs(5)).await.unwrap();
        assert_eq!(ev.data["role"], "worker");
        let r = w
            .call(methods::CALL, json!({"tool": "list_sessions"}))
            .await
            .unwrap();
        assert_eq!(r["isError"], true, "卸任后当场被拒，不靠 helper 刷新");
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
}
