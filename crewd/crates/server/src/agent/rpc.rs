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
    data_token: Option<String>,
    attached_flag: Arc<std::sync::atomic::AtomicBool>,
    watcher: Option<JoinHandle<()>>,
    human_event_stream: bool,
}

impl AgentConn {
    pub(crate) fn attached_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.attached_flag.clone()
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub(crate) fn is_attached(&self) -> bool {
        self.session.is_some()
    }

    /// A buffered Human notification must never arrive on an Agent connection,
    /// even after a later subscription replaces the Human-only topic.
    pub(crate) fn reserve_human_event_stream(&mut self) {
        self.human_event_stream = true;
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

/// Only input-only, closed public refusals may cross the MCP boundary in
/// detail. Never forward arbitrary Core messages, SQL errors or ACL facts.
fn knowledge_failure_text(error: &RpcError) -> String {
    if error.code == RpcError::INVALID_PARAMS
        && let Some(problem) = error.data.as_ref().and_then(|data| {
            crewd_proto::team_data::DataPlacementIdentityError::from_refusal(&data.refusal)
        })
    {
        return serde_json::json!({
            "error": {
                "code": error.code,
                "refusal": problem.refusal(),
                "message": problem.message()
            },
            "thisCallApplied": false,
            "originalRequestOutcome": "notDetermined",
            "guidance": "Retain the original request UUID. Never resend an Unknown request; use data_request_get with the original context to determine any prior outcome."
        }).to_string();
    }
    format!(
        "Data operation unavailable or refused (RPC code {}); check the tool argument schema and current crew ACL. Obtain context from data_workspace_get; reads do not initialize a workspace. Retain the original request UUID and use pure lookup after an unknown outcome.",
        error.code
    )
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

pub(crate) async fn handle(
    state: &Arc<AppState>,
    conn: &mut AgentConn,
    req: Request,
    tx: &mpsc::Sender<ServerFrame>,
    scope: &crate::connection_scope::ConnectionScope,
) -> Response {
    let id = req.id;
    match route(state, conn, req, tx, scope).await {
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
    scope: &crate::connection_scope::ConnectionScope,
) -> Result<Value, RpcError> {
    if req.method == methods::ATTACH {
        if conn.human_event_stream {
            return Err(RpcError::new(
                RpcError::FORBIDDEN,
                "Human event streams cannot attach as agents",
            )
            .with_refusal("resource.forbidden"));
        }
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
        conn.data_token = Some(p.token);
        conn.attached_flag
            .store(true, std::sync::atomic::Ordering::Release);
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
            if super::admin_case_scope::bound(state, &session)
                .await
                .map_err(|_| invalid())?
            {
                let secretary = super::admin_case_scope::secretary_binding(state, &session)
                    .await
                    .map_err(|_| invalid())?;
                return to_value(catalog::case_tools_for_purpose(&me, loc, secretary));
            }
            let mut list = catalog::tools_for(&me, loc);
            if !scope.is_live()
                || !conn
                    .data_token
                    .as_ref()
                    .is_some_and(|token| state.tokens.verify(&session, token))
                || (state.management.get().is_some() && !state.admission.contains(&session))
            {
                list.retain(|tool| !catalog::requires_managed_scope(&tool.name));
            }
            if state.data_runtime.get().is_some() {
                list.retain(|tool| !tool.name.starts_with("document_"));
            }
            if scope.is_live()
                && conn
                    .data_token
                    .as_ref()
                    .is_some_and(|token| state.tokens.verify(&session, token))
                && state
                    .data_runtime
                    .get()
                    .is_some_and(|runtime| runtime.qualified())
            {
                list.extend(crewd_proto::team_data_api::agent_tools());
            }
            to_value(list)
        }
        methods::CALL => {
            let p: CallParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            let authority = super::admin_case_scope::prepare(
                state,
                &me,
                scope.clone(),
                conn.data_token.clone().unwrap_or_default(),
                &p.tool,
                &p.args,
            )
            .await
            .map_err(|_| invalid())?;
            if let Some(authority) = authority {
                return to_value(
                    super::admin_case_scope::within(
                        authority,
                        tools::call(state, &me, &p.tool, p.args),
                    )
                    .await,
                );
            }
            if let Some(method) = crewd_proto::team_data_api::agent_method(&p.tool) {
                let token = conn.data_token.clone().ok_or_else(invalid)?;
                if !scope.is_live() || !state.tokens.verify(&session, &token) {
                    return Err(invalid());
                }
                let runtime = state
                    .data_runtime
                    .get()
                    .filter(|r| r.qualified())
                    .ok_or_else(invalid)?;
                let result = runtime
                    .session(
                        state.clone(),
                        me.session_id,
                        token,
                        scope.clone(),
                        method,
                        p.args,
                    )
                    .await;
                return to_value(match result {
                    Ok(value) => crewd_proto::agent::CallResult::ok(
                        serde_json::to_string(&value).map_err(internal)?,
                    ),
                    Err(error) => {
                        crewd_proto::agent::CallResult::err(knowledge_failure_text(&error))
                    }
                });
            }
            if state.management.get().is_some() && !state.admission.contains(&session) {
                return Err(RpcError::new(
                    RpcError::FORBIDDEN,
                    "Agent execution requires current-run Human admission",
                ));
            }
            if catalog::requires_managed_scope(&p.tool)
                || matches!(
                    p.tool.as_str(),
                    // Search keeps its existing All-role tool visibility, but
                    // its unified read needs the original connection and token.
                    "search"
                        | "channel_archive_check"
                        | "channel_archive_verified"
                        | "channel_restore_verified"
                        | crewd_collab::work_coordination::SPLIT_TOOL
                        | crewd_collab::requirement_ruling::CAPTAIN_CHECK_TOOL
                        | crewd_collab::requirement_ruling::CAPTAIN_CLOSE_TOOL
                )
                || (p.tool == "work_item_assign"
                    && p.args.get("coordination_decision_id").is_some())
            {
                let token = conn.data_token.clone().ok_or_else(invalid)?;
                return to_value(
                    tools::call_scoped(state, &me, &p.tool, p.args, scope.clone(), token).await,
                );
            }
            to_value(tools::call(state, &me, &p.tool, p.args).await)
        }
        methods::HOOK => {
            let p: HookParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            if super::admin_case_scope::bound(state, &session)
                .await
                .map_err(|_| invalid())?
            {
                if !scope.is_live()
                    || !conn
                        .data_token
                        .as_ref()
                        .is_some_and(|token| state.tokens.verify(&session, token))
                {
                    return Err(invalid());
                }
                if let Some(factory) = state.native_helpers.get() {
                    let _ = crate::orchestra::exec_claude_managed::route(
                        factory, &session, p.event, p.payload, None,
                    )
                    .await;
                }
                return to_value(HookResult { stdout: None });
            }
            let review = conn
                .data_token
                .clone()
                .filter(|_| {
                    matches!(
                        me.role,
                        crewd_proto::Role::Worker | crewd_proto::Role::Captain
                    )
                })
                .map(|token| {
                    let scope = scope.clone();
                    let state = state.clone();
                    let session = session.clone();
                    let allowed: crate::orchestra::exec_claude_managed::RiskReviewAuthority =
                        Arc::new(move || {
                            scope.is_live()
                                && state.tokens.verify(&session, &token)
                                && !state.tokens.is_admin_case(&session)
                                && (state.management.get().is_none()
                                    || state.admission.contains(&session))
                        });
                    allowed
                });
            let result = hook(state, &me, p.event, p.payload.clone(), review).await;
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

async fn hook(
    state: &Arc<AppState>,
    me: &Identity,
    event: HookKind,
    payload: Value,
    review: Option<crate::orchestra::exec_claude_managed::RiskReviewAuthority>,
) -> HookResult {
    let managed = if let Some(factory) = state.native_helpers.get() {
        crate::orchestra::exec_claude_managed::route(
            factory,
            &me.session_id,
            event,
            payload.clone(),
            review.clone(),
        )
        .await
    } else {
        None
    };
    let output = match event {
        HookKind::UserPromptSubmit => managed.unwrap_or(HookResult {stdout:None}),
        HookKind::StopFailure | HookKind::SessionEnd | HookKind::Notification | HookKind::ManagedActivity => HookResult {stdout:None},
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
    };
    // A missing managed handle is never proof of a legacy runner.
    if state.native_helpers.get().is_some() {
        return output;
    }
    let Some(review) = review.filter(|allowed| allowed()) else {
        return output;
    };
    legacy_risk_context(state, me, event, &payload, output, &review).await
}

async fn legacy_risk_context(
    state: &AppState,
    me: &Identity,
    event: HookKind,
    payload: &Value,
    mut output: HookResult,
    review: &crate::orchestra::exec_claude_managed::RiskReviewAuthority,
) -> HookResult {
    let label = payload["hook_event_name"].as_str().unwrap_or("");
    if !matches!(
        (event, label),
        (HookKind::SessionStart, "SessionStart")
            | (HookKind::PostToolUse, "PostToolUse" | "PostToolUseFailure")
    ) || payload.get("agent_id").is_some()
    {
        return output;
    }
    let Some((native, stdout)) = payload["session_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .zip(output.stdout.as_ref())
    else {
        return output;
    };
    let Ok(mut content) = serde_json::from_str::<Value>(stdout) else {
        return output;
    };
    let Some(existing) = content["hookSpecificOutput"]["additionalContext"].as_str() else {
        return output;
    };
    if existing.is_empty() || content["hookSpecificOutput"]["hookEventName"] != label {
        return output;
    }
    let live = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        port::live(state, &me.session_id),
    )
    .await;
    if !matches!(live, Ok(Ok(live)) if live.running) {
        return output;
    }
    if let Some(risk) = crate::orchestra::exec_claude_managed::due_risk_review(
        &state.db,
        &me.session_id,
        Some(native.into()),
    )
    .await
    .filter(|_| review())
    {
        let line = crewd_proto::agent::managed_risk_line(risk);
        if existing.encode_utf16().count() + line.len() < state.agent.config.inject_cap_chars {
            content["hookSpecificOutput"]["additionalContext"] =
                Value::String(format!("{existing}\n{line}"));
            output.stdout = Some(content.to_string());
        }
    }
    output
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

    #[tokio::test]
    async fn managed_object_tools_use_the_exact_attached_session_and_original_request() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool": name, "args": args}))
                .await
                .unwrap()
        }

        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign managed object fixture", None).await;
        let other = "foreign-managed-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign worker")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, id) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(methods::ATTACH, json!({"sessionId":id,"token":f.token(id)}))
                .await
                .unwrap();
        }
        let tools = writer.call(methods::TOOLS, json!({})).await.unwrap();
        assert!(
            tools
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == "managed_object_submit")
        );
        let captured = decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await);
        assert_eq!(captured["crewId"], f.crew);
        assert!(
            captured["scope"]["dbId"].is_string() && captured["scope"]["restoreEpoch"].is_string()
        );
        let scope = captured["scope"].clone();
        let request = json!({
            "scope": scope, "crewId": f.crew, "requestId": uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"createTable","title":"Original managed fixture",
                      "fields":[{"name":"Name","fieldType":"text","nullable":false}]},
        });
        let committed = decoded(tool(&mut writer, "managed_object_submit", request.clone()).await);
        assert_eq!(committed["scope"], scope);
        assert_eq!(committed["requestId"], request["requestId"]);
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_submit", request.clone()).await),
            committed
        );
        let original = json!({"scope":scope,"crewId":f.crew,"requestId":request["requestId"]});
        let known =
            decoded(tool(&mut writer, "managed_object_request_get", original.clone()).await);
        assert_eq!(known["status"], "known");
        assert_eq!(known["receipt"], committed);
        assert_eq!(
            tool(&mut reader, "managed_object_request_get", original.clone()).await["isError"],
            true
        );
        let table_id = committed["object"]["table"]["id"].as_str().unwrap();
        assert!(table_id.starts_with('T'));
        assert_eq!(
            decoded(tool(&mut reader, "managed_object_get", json!({"id":table_id})).await),
            committed["object"]
        );
        let tables = decoded(tool(&mut reader, "managed_tables_list", json!({})).await);
        assert!(
            tables["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|table| table["id"] == table_id)
        );
        let add = json!({
            "scope":scope, "crewId":f.crew, "requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"addRow","tableId":table_id,"values":[{"type":"text","value":"original"}]},
        });
        let refused = tool(&mut reader, "managed_object_submit", add.clone()).await;
        assert_eq!(refused["isError"], true);
        let added = decoded(tool(&mut writer, "managed_object_submit", add).await);
        let row = &added["object"]["row"];
        assert!(row["id"].as_str().unwrap().starts_with('O'));
        let rows = decoded(
            tool(
                &mut reader,
                "managed_rows_list",
                json!({"tableId":table_id}),
            )
            .await,
        );
        assert_eq!(rows["items"], json!([row]));
        let edit = json!({
            "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"editRow","rowId":row["id"],"expectedRevision":row["revision"],
                      "values":[{"type":"text","value":"must not be written"}]},
        });
        f.state.tokens.revoke(&f.worker);
        let catalog = writer.call(methods::TOOLS, json!({})).await.unwrap();
        assert!(
            catalog
                .as_array()
                .unwrap()
                .iter()
                .all(|item| { !item["name"].as_str().unwrap().starts_with("managed_") })
        );
        assert_eq!(
            tool(&mut writer, "managed_object_submit", edit).await["isError"],
            true
        );
        assert_eq!(
            tool(&mut writer, "managed_object_request_get", original).await["isError"],
            true
        );
        assert_eq!(
            decoded(tool(&mut reader, "managed_object_get", json!({"id":row["id"]})).await),
            added["object"]
        );
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn v2_raw_socket_duplicates_refuse_before_effect_and_preserve_original_uuid() {
        use crewd_proto::managed_tables_v2::*;
        use serde_json::Value;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        async fn frame(
            rd: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
            wr: &mut tokio::net::unix::OwnedWriteHalf,
            id: u64,
            line: &str,
        ) -> Value {
            wr.write_all(format!("{line}\n").as_bytes()).await.unwrap();
            loop {
                let mut body = String::new();
                assert!(
                    tokio::time::timeout(Duration::from_secs(10), rd.read_line(&mut body))
                        .await
                        .unwrap()
                        .unwrap()
                        > 0
                );
                let result: Value = serde_json::from_str(&body).unwrap();
                if result["id"] == id {
                    return result;
                }
            }
        }
        async fn snapshot(f: &Fixture) -> Value {
            f.state
                .db
                .call(|s| -> crewd_store::Result<Value> {
                    let mut out = serde_json::Map::new();
                    for t in [
                        "managed_tables",
                        "managed_rows",
                        "managed_links",
                        "managed_v2_receipts",
                        "managed_link_events",
                        "object_refs",
                    ] {
                        let n: i64 =
                            s.conn()
                                .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| {
                                    r.get(0)
                                })?;
                        out.insert(t.into(), json!(n));
                    }
                    let clock: i64 = s.conn().query_row(
                        "SELECT value FROM managed_link_clock WHERE id=1",
                        [],
                        |r| r.get(0),
                    )?;
                    out.insert("clock".into(), json!(clock));
                    Ok(Value::Object(out))
                })
                .await
                .unwrap()
                .unwrap()
        }
        fn duplicate(line: &str, alias: bool, title: &str) -> String {
            let title = serde_json::to_string(title).unwrap();
            let marker = format!("\"title\":{title}");
            assert_eq!(line.matches(marker.as_str()).count(), 1);
            let key = if alias {
                r#""t\u0069tle""#
            } else {
                r#""title""#
            };
            let replacement = format!("\"title\":\"not the accepted intent\",{key}:{title}");
            line.replacen(marker.as_str(), &replacement, 1)
        }
        let f = Fixture::new().await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut observer = RpcClient::connect(&socket).await;
        let scope = observer
            .call("managedObject.scope", json!({}))
            .await
            .unwrap();
        let packet = |uuid: String, title: &str, purpose: &str| {
            json!({"scope":scope,"crewId":f.crew,"requestId":uuid,"operation":{
            "kind":"createTable","title":title,"purpose":purpose, "titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters",
            "fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}]}})
        };
        let human_uuid = uuid::Uuid::new_v4().to_string();
        let human_title = "Raw original";
        let human = packet(
            human_uuid.clone(),
            human_title,
            "A scalar may contain {\"x\":1,\"x\":2} without parsing it as JSON",
        );
        let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut rd = tokio::io::BufReader::new(rd);
        let valid =
            serde_json::to_string(&json!({"id":1,"method":TABLE_SUBMIT,"params":human})).unwrap();
        let before = snapshot(&f).await;
        for alias in [false, true] {
            let bad = frame(&mut rd, &mut wr, 1, &duplicate(&valid, alias, human_title)).await;
            assert_eq!(bad["error"]["code"], crewd_proto::RpcError::INVALID_PARAMS);
            assert_eq!(
                snapshot(&f).await,
                before,
                "rejected raw duplicate cannot allocate rows/receipts/events/IDs"
            );
        }
        assert_eq!(
            observer
                .call(
                    TABLE_REQUEST_GET,
                    json!({"scope":scope,"crewId":f.crew,"requestId":human_uuid})
                )
                .await
                .unwrap(),
            json!({"status":"notFound"})
        );
        let accepted = frame(&mut rd, &mut wr, 1, &valid).await;
        assert!(accepted.get("error").is_none());
        assert_eq!(accepted["result"]["requestId"], human_uuid);
        assert_eq!(
            observer
                .call(
                    TABLE_REQUEST_GET,
                    json!({"scope":scope,"crewId":f.crew,"requestId":human_uuid})
                )
                .await
                .unwrap(),
            json!({"status":"known","receipt":accepted["result"]})
        );
        let agent_uuid = uuid::Uuid::new_v4().to_string();
        // This tests raw-frame identity, not an intentional duplicate table definition.
        // V3 correctly requires explicit review for a legacy same-name/near-purpose create.
        let agent_title = "Warehouse inventory";
        let agent = packet(
            agent_uuid.clone(),
            agent_title,
            "Track storage locations and stock balances",
        );
        let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let (agent_rd, mut agent_wr) = stream.into_split();
        let mut agent_rd = tokio::io::BufReader::new(agent_rd);
        let agent_line=serde_json::to_string(&json!({"id":3,"method":methods::CALL,"params":{"tool":"managed_table_v2_submit","args":agent}})).unwrap();
        let unauth = frame(
            &mut agent_rd,
            &mut agent_wr,
            3,
            &duplicate(&agent_line, true, agent_title),
        )
        .await;
        assert_eq!(
            unauth["error"]["code"],
            crewd_proto::RpcError::METHOD_NOT_FOUND,
            "attachment must win before new raw input validation"
        );
        let attach=serde_json::to_string(&json!({"id":2,"method":methods::ATTACH,"params":{"sessionId":f.worker,"token":f.token(&f.worker)}})).unwrap();
        assert!(
            frame(&mut agent_rd, &mut agent_wr, 2, &attach)
                .await
                .get("error")
                .is_none()
        );
        let after_human = snapshot(&f).await;
        let bad = frame(
            &mut agent_rd,
            &mut agent_wr,
            3,
            &duplicate(&agent_line, true, agent_title),
        )
        .await;
        assert_eq!(bad["error"]["code"], crewd_proto::RpcError::INVALID_PARAMS);
        assert_eq!(snapshot(&f).await, after_human);
        let human_on_agent =
            serde_json::to_string(&json!({"id":4,"method":TABLE_SUBMIT,"params":agent})).unwrap();
        let refused = frame(
            &mut agent_rd,
            &mut agent_wr,
            4,
            &duplicate(&human_on_agent, false, agent_title),
        )
        .await;
        assert_eq!(
            refused["error"]["code"],
            crewd_proto::RpcError::FORBIDDEN,
            "Human-only route must win before duplicate oracle"
        );
        assert_eq!(snapshot(&f).await, after_human);
        let accepted = frame(&mut agent_rd, &mut agent_wr, 3, &agent_line).await;
        assert!(accepted.get("error").is_none());
        assert_eq!(accepted["result"]["isError"], false);
        let receipt: Value =
            serde_json::from_str(accepted["result"]["text"].as_str().unwrap()).unwrap();
        assert_eq!(receipt["requestId"], agent_uuid);
        let lookup=serde_json::to_string(&json!({"id":5,"method":methods::CALL,"params":{"tool":"managed_table_v2_request_get","args":{"scope":scope,"crewId":f.crew,"requestId":agent_uuid}}})).unwrap();
        let known = frame(&mut agent_rd, &mut agent_wr, 5, &lookup).await;
        assert_eq!(known["result"]["isError"], false);
        assert_eq!(
            serde_json::from_str::<Value>(known["result"]["text"].as_str().unwrap()).unwrap(),
            json!({"status":"known","receipt":receipt})
        );
        drop((rd, wr, agent_rd, agent_wr, observer));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn v2_tools_keep_real_message_scope_hidden_sets_and_current_row_authority() {
        use crewd_proto::managed_tables_v2::*;
        use serde_json::Value;
        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }
        fn decoded(r: Value) -> Value {
            assert_eq!(r["isError"], false, "{r}");
            serde_json::from_str(r["text"].as_str().unwrap()).unwrap()
        }
        fn packet(scope: &Value, crew: &str, operation: Value) -> Value {
            json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":operation})
        }
        let f = Fixture::new().await;
        f.enable_knowledge();
        let foreign = f.add_crew("Independent row owner", None).await;
        let message_crew = f
            .add_crew("Independent private message authority", None)
            .await;
        let other = "v2-independent-worker";
        let colleague = "v2-same-crew-worker";
        f.add_session(&foreign, other, Role::Worker, "Independent author")
            .await;
        f.add_session(&f.crew, colleague, Role::Worker, "Same crew reader")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        let mut same_crew = RpcClient::connect(&socket).await;
        let mut human = RpcClient::connect(&socket).await;
        for (peer, id) in [
            (&mut writer, f.worker.as_str()),
            (&mut reader, other),
            (&mut same_crew, colleague),
        ] {
            peer.call(methods::ATTACH, json!({"sessionId":id,"token":f.token(id)}))
                .await
                .unwrap();
        }
        let scope =
            decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await)["scope"]
                .clone();
        let topic=human.call("channel.create",json!({"crewId":message_crew,"kind":"topic","name":"Private V2 target","members":[{"kind":"session","id":f.worker}]})).await.unwrap();
        let message = human
            .call(
                "message.post",
                json!({"channelId":topic["id"],"text":"Synthetic private target e\u{301}"}),
            )
            .await
            .unwrap();
        let private_target = json!({"kind":"message","id":message["id"]});
        let resolved = decoded(
            tool(
                &mut writer,
                "managed_link_resolve_target",
                json!({"target":private_target}),
            )
            .await,
        );
        assert_eq!(resolved["target"], private_target);
        assert_eq!(resolved["messageChannelId"], topic["id"]);
        assert!(resolved["summary"].as_str().unwrap().chars().count() <= 280);
        assert_eq!(
            tool(
                &mut reader,
                "managed_link_resolve_target",
                json!({"target":private_target})
            )
            .await["isError"],
            true
        );
        let create = packet(
            &scope,
            &f.crew,
            json!({"kind":"createTable","title":"Author protected scalar table","purpose":"No V1 ACL bypass","titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman",
            "fields":[{"name":"Name","kind":{"kind":"text"},"nullable":false}]}),
        );
        let mut changes = f.state.hub.subscribe_collab();
        let created = decoded(tool(&mut writer, "managed_table_v2_submit", create.clone()).await);
        assert_eq!(
            decoded(tool(&mut writer, "managed_table_v2_submit", create.clone()).await),
            created
        );
        let original = json!({"scope":scope,"crewId":f.crew,"requestId":create["requestId"]});
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "managed_table_v2_request_get",
                    original.clone()
                )
                .await
            ),
            json!({"status":"known","receipt":created})
        );
        assert_eq!(
            std::iter::from_fn(|| changes.try_recv().ok())
                .filter(|v| matches!(v,Change::Progress{session_id,..} if session_id==&f.worker))
                .count(),
            1,
            "fresh create counts once; replay/lookup count zero"
        );
        let table_id = created["effect"]["tableId"].clone();
        let table =
            decoded(tool(&mut writer, "managed_table_v2_get", json!({"id":table_id})).await);
        assert_eq!(
            table["author"],
            json!({"kind":"session","sessionId":f.worker})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut same_crew,
                    "managed_table_v2_get",
                    json!({"id":table_id})
                )
                .await
            ),
            table
        );
        assert!(decoded(tool(&mut reader,"managed_tables_v2_list",json!({"after":null})).await)["items"].as_array().unwrap().iter().any(|t|t["id"]==table_id));
        let row_add = packet(
            &scope,
            &f.crew,
            json!({"kind":"addRow","tableId":table_id,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Author's original"}}}]}),
        );
        let added = decoded(tool(&mut writer, "managed_table_v2_submit", row_add).await);
        let row_id = added["effect"]["rowId"].clone();
        let row = decoded(tool(&mut writer, "managed_row_v2_get", json!({"id":row_id})).await);
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "managed_rows_v2_list",
                    json!({"tableId":table_id,"after":null})
                )
                .await
            )["items"],
            json!([row])
        );
        let other_patch = packet(
            &scope,
            &f.crew,
            json!({"kind":"patchRow","rowId":row_id,"expectedSchemaRevision":"1","expectedRowRevision":row["revision"],"patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Not the original row author"}}}]}),
        );
        assert_eq!(
            tool(&mut same_crew, "managed_table_v2_submit", other_patch).await["isError"],
            true
        );
        let legacy_edit = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"editRow","rowId":row_id,"expectedRevision":row["revision"],"values":[{"type":"text","value":"Must not bypass author policy"}]}});
        assert_eq!(
            tool(&mut same_crew, "managed_object_submit", legacy_edit).await["isError"],
            true,
            "V1 positional route must not bypass V2 existing-row author policy"
        );
        let legacy_create = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"createTable","title":"Fresh V1 collaboration","fields":[{"name":"Name","fieldType":"text","nullable":false}]}});
        let legacy = decoded(tool(&mut writer, "managed_object_submit", legacy_create).await);
        let legacy_id = legacy["object"]["table"]["id"].clone();
        let captured =
            decoded(tool(&mut writer, "managed_table_v2_get", json!({"id":legacy_id})).await);
        assert_eq!(
            captured["author"],
            json!({"kind":"session","sessionId":f.worker})
        );
        assert_eq!(captured["rowWritePolicy"], "ownerCrewWriters");
        let colleague_add = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"addRow","tableId":legacy_id,"values":[{"type":"text","value":"Preserved V1 crew-writer authority"}]}});
        assert_eq!(
            decoded(tool(&mut same_crew, "managed_object_submit", colleague_add).await)["object"]["row"]
                ["crewId"],
            f.crew
        );
        let req=human.call("todo.create",json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"text":"A public Core target"})).await.unwrap();
        let work=human.call("work.create",json!({"crewId":f.crew,"title":"Shared Core target","sources":[req["requirementId"]]})).await.unwrap();
        let work_target = json!({"kind":"work","id":work["id"]});
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "managed_link_resolve_target",
                    json!({"target":work_target})
                )
                .await
            )["target"],
            work_target,
            "organization Core reads remain available across crews"
        );
        let source = json!({"kind":"table","id":table_id});
        let outgoing = json!({"query":{"direction":"outgoing","source":source,"includeAdapters":false},"after":null,"limit":16});
        let empty = decoded(tool(&mut writer, "managed_links_list", outgoing.clone()).await);
        let link = packet(
            &scope,
            &f.crew,
            json!({"kind":"link","source":source,"target":{"kind":"reference","target":work_target},"expectedManualLinkRevision":empty["manualLinkRevision"]}),
        );
        let mut manual_changes = f.state.hub.subscribe_collab();
        let linked = decoded(tool(&mut writer, "managed_link_submit", link.clone()).await);
        assert_eq!(
            decoded(tool(&mut writer, "managed_link_submit", link.clone()).await),
            linked
        );
        let link_lookup = json!({"scope":scope,"crewId":f.crew,"requestId":link["requestId"]});
        assert_eq!(
            decoded(tool(&mut writer, "managed_link_request_get", link_lookup.clone()).await),
            json!({"status":"known","receipt":linked})
        );
        let mut duplicate = link.clone();
        duplicate["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        duplicate["operation"]["expectedManualLinkRevision"] =
            linked["resultManualLinkRevision"].clone();
        assert_eq!(
            decoded(tool(&mut writer, "managed_link_submit", duplicate).await)["effect"]["added"],
            false
        );
        assert_eq!(
            std::iter::from_fn(|| manual_changes.try_recv().ok())
                .filter(|v| matches!(v,Change::Progress{session_id,..} if session_id==&f.worker))
                .count(),
            1
        );
        assert_eq!(
            human
                .call("work.get", json!({"id":work["id"]}))
                .await
                .unwrap(),
            work,
            "manual relates never changes WorkBook"
        );
        let b_create = packet(
            &scope,
            &foreign,
            json!({"kind":"createTable","title":"Source-bound hidden collection","purpose":"Foreign private targets stay hidden","titleFieldIndex":0,"rowWritePolicy":"authorCaptainHuman","fields":[
            {"name":"Name","kind":{"kind":"text"},"nullable":false},
            {"name":"Private targets","kind":{"kind":"link","target":{"kind":"message"},"many":true},"nullable":true},
            {"name":"Other targets","kind":{"kind":"link","target":{"kind":"message"},"many":true},"nullable":true}]}),
        );
        let b_table=decoded(tool(&mut reader,"managed_table_v2_submit",b_create).await)["effect"]["tableId"].clone();
        let b_add = packet(
            &scope,
            &foreign,
            json!({"kind":"addRow","tableId":b_table,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"B's editable row"}}}]}),
        );
        let b_row_id = decoded(tool(&mut reader, "managed_table_v2_submit", b_add).await)["effect"]
            ["rowId"]
            .clone();
        let human_link = packet(
            &scope,
            &foreign,
            json!({"kind":"patchRow","rowId":b_row_id,"expectedSchemaRevision":"1","expectedRowRevision":"1","patches":[{"fieldId":"p2","change":{"kind":"replaceLinks","values":[{"kind":"reference","target":private_target}],"keepHiddenSetToken":null}}]}),
        );
        human.call(TABLE_SUBMIT, human_link).await.unwrap();
        let hidden = decoded(tool(&mut reader, "managed_row_v2_get", json!({"id":b_row_id})).await);
        let hidden_cell = hidden["linkCells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["fieldId"] == "p2")
            .unwrap();
        assert!(hidden_cell["visible"].as_array().unwrap().is_empty());
        assert_eq!(hidden_cell["hiddenSet"].as_object().unwrap().len(), 1);
        assert!(hidden_cell["hiddenSet"]["keepToken"].is_string());
        assert!(
            !hidden.to_string().contains(message["id"].as_str().unwrap())
                && !hidden.to_string().contains("Synthetic private target")
        );
        let b_outgoing = json!({"query":{"direction":"outgoing","source":{"kind":"row","id":b_row_id},"includeAdapters":false},"after":null,"limit":16});
        let hidden_page =
            decoded(tool(&mut reader, "managed_links_list", b_outgoing.clone()).await);
        assert!(
            hidden_page["items"].as_array().unwrap().is_empty()
                && hidden_page["outgoingHiddenSet"].is_object()
        );
        assert!(
            !hidden_page
                .to_string()
                .contains(message["id"].as_str().unwrap())
        );
        let scalar = packet(
            &scope,
            &foreign,
            json!({"kind":"patchRow","rowId":b_row_id,"expectedSchemaRevision":"1","expectedRowRevision":hidden["revision"],"patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Scalar edit preserves hidden field"}}}]}),
        );
        decoded(tool(&mut reader, "managed_table_v2_submit", scalar).await);
        let current =
            decoded(tool(&mut reader, "managed_row_v2_get", json!({"id":b_row_id})).await);
        let keep = current["linkCells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["fieldId"] == "p2")
            .unwrap()["hiddenSet"]["keepToken"]
            .clone();
        let noop = packet(
            &scope,
            &foreign,
            json!({"kind":"patchRow","rowId":b_row_id,"expectedSchemaRevision":"1","expectedRowRevision":current["revision"],"patches":[{"fieldId":"p2","change":{"kind":"replaceLinks","values":[],"keepHiddenSetToken":keep}}]}),
        );
        let mut hidden_changes = f.state.hub.subscribe_collab();
        let no_effect = decoded(tool(&mut reader, "managed_table_v2_submit", noop.clone()).await);
        assert_eq!(no_effect["effect"]["changed"], false);
        assert!(no_effect["eventId"].is_null());
        let b_message=human.call("message.post",json!({"channelId":crewd_store::channels::lobby_id(&foreign),"text":"Visible B target"})).await.unwrap();
        let mut increase = noop.clone();
        increase["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        increase["operation"]["patches"][0]["change"]["values"] =
            json!([{"kind":"reference","target":{"kind":"message","id":b_message["id"]}}]);
        assert_eq!(
            tool(&mut reader, "managed_table_v2_submit", increase).await["isError"],
            true,
            "hidden-set capacity must not become a cardinality oracle"
        );
        let mut launder = noop.clone();
        launder["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        launder["operation"]["patches"][0]["fieldId"] = json!("p3");
        assert_eq!(
            tool(&mut reader, "managed_table_v2_submit", launder).await["isError"],
            true,
            "hidden-set Keep cannot be copied to another field"
        );
        assert_eq!(
            decoded(tool(&mut reader, "managed_row_v2_get", json!({"id":b_row_id})).await),
            current
        );
        assert!(
            !std::iter::from_fn(|| hidden_changes.try_recv().ok())
                .any(|v| matches!(v, Change::Progress { .. })),
            "no-op, hidden increase and token laundering have no progress"
        );
        human
            .call(
                "channel.removeMember",
                json!({"channelId":topic["id"],"member":{"kind":"session","id":f.worker}}),
            )
            .await
            .unwrap();
        let revoked_message = tool(
            &mut writer,
            "managed_link_resolve_target",
            json!({"target":private_target}),
        )
        .await;
        assert_eq!(
            revoked_message["isError"], true,
            "live message membership is checked again"
        );
        for private in [
            message["id"].as_str().unwrap(),
            topic["id"].as_str().unwrap(),
            "Synthetic private target",
        ] {
            assert!(
                !revoked_message.to_string().contains(private),
                "revoked resolve leaked private message metadata: {revoked_message}"
            );
        }
        f.add_session(
            &foreign,
            &f.worker,
            Role::Worker,
            "Changed current identity",
        )
        .await;
        assert_eq!(
            tool(&mut writer, "managed_table_v2_submit", create.clone()).await["isError"],
            true
        );
        assert_eq!(
            tool(
                &mut writer,
                "managed_table_v2_request_get",
                original.clone()
            )
            .await["isError"],
            true
        );
        f.state.tokens.revoke(&f.worker);
        for (name, args) in [
            ("managed_table_v2_get", json!({"id":table_id})),
            ("managed_tables_v2_list", json!({"after":null})),
            ("managed_row_v2_get", json!({"id":row_id})),
            (
                "managed_rows_v2_list",
                json!({"tableId":table_id,"after":null}),
            ),
            ("managed_table_v2_submit", create),
            ("managed_table_v2_request_get", original),
            ("managed_link_resolve_target", json!({"target":work_target})),
            ("managed_links_list", outgoing),
            ("managed_link_submit", link),
            ("managed_link_request_get", link_lookup),
        ] {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "invalidated attachment used {name}"
            );
        }
        drop((writer, reader, same_crew, human));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn artifact_history_tools_recheck_current_identity_without_model_progress() {
        use serde_json::Value;
        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }
        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }
        let f = Fixture::new().await;
        let foreign = f.add_crew("Other history owner", None).await;
        let other = "history-foreign-worker";
        f.add_session(&foreign, other, Role::Worker, "Other historical worker")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, session) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(
                methods::ATTACH,
                json!({"sessionId":session,"token":f.token(session)}),
            )
            .await
            .unwrap();
        }
        let captured = decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await);
        let scope = captured["scope"].clone();
        let mut changes = f.state.hub.subscribe_collab();
        let begin =
            json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string()});
        let begun = decoded(tool(&mut writer, "artifact_history_begin", begin.clone()).await);
        assert_eq!(begun["effect"], json!({"kind":"begun","created":true}));
        assert_eq!(
            decoded(tool(&mut writer, "artifact_history_begin", begin.clone()).await),
            begun
        );
        let status = decoded(
            tool(
                &mut writer,
                "artifact_history_status",
                json!({"crewId":f.crew}),
            )
            .await,
        );
        assert_eq!(status["job"], begun["job"]);
        assert_eq!(status["canContinue"], true);
        let mut reuse = begin.clone();
        reuse["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let reused = decoded(tool(&mut writer, "artifact_history_begin", reuse.clone()).await);
        assert_eq!(reused["effect"], json!({"kind":"begun","created":false}));
        assert_eq!(reused["job"], begun["job"]);
        assert_eq!(
            reused["job"]["lastRequestId"], begin["requestId"],
            "reuse is not an invented new advancement"
        );
        let original = json!({"scope":scope,"crewId":f.crew,"requestId":begin["requestId"]});
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "artifact_history_request_get",
                    original.clone()
                )
                .await
            ),
            json!({"status":"known","receipt":begun})
        );
        let step = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"jobId":begun["job"]["id"],"expectedRevision":begun["job"]["revision"],"expectedCursor":begun["job"]["cursor"],"limit":64});
        let advanced = decoded(tool(&mut writer, "artifact_history_step", step.clone()).await);
        assert_eq!(
            advanced["job"]["cursor"],
            json!({"phase":"complete","after":"0"})
        );
        assert_eq!(
            advanced["effect"]["delta"]["scanned"], "0",
            "empty phases consume no invented rows"
        );
        assert_eq!(
            decoded(tool(&mut writer, "artifact_history_step", step.clone()).await),
            advanced,
            "original UUID replays before the now-complete CAS"
        );
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "artifact_history_request_get",
                    original.clone()
                )
                .await
            ),
            json!({"status":"known","receipt":begun}),
            "current complete cursor never substitutes for original Begin receipt"
        );
        let calls = [
            (
                "artifact_history_status",
                json!({"crewId":f.crew,"jobId":begun["job"]["id"]}),
            ),
            ("artifact_history_begin", begin.clone()),
            ("artifact_history_step", step.clone()),
            ("artifact_history_request_get", original.clone()),
        ];
        for (name, args) in calls.clone() {
            assert_eq!(
                tool(&mut reader, name, args).await["isError"],
                true,
                "foreign worker used {name}"
            );
        }
        let archived = f.crew.clone();
        f.state
            .collab(move |tx| crewd_collab::crews::archive(tx, &archived))
            .await
            .unwrap();
        for (name, args) in calls.clone() {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "archived Agent used {name}, even for original replay"
            );
        }
        let mut human = RpcClient::connect(&socket).await;
        assert_eq!(
            human
                .call("artifact.history.status", json!({"crewId":f.crew}))
                .await
                .unwrap()["job"],
            advanced["job"],
            "Human may inspect archived retained facts"
        );
        drop(human);
        f.add_session(&foreign, &f.worker, Role::Worker, "Changed current crew")
            .await;
        for (name, args) in calls.clone() {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "changed current crew borrowed old {name}"
            );
        }
        f.state.tokens.revoke(&f.worker);
        for (name, args) in calls {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "invalid attachment used {name}"
            );
        }
        assert!(
            !std::iter::from_fn(|| changes.try_recv().ok())
                .any(|change| matches!(change, Change::Progress { .. })),
            "historical maintenance, replay, reuse and empty steps never manufacture model progress"
        );
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn artifact_tools_keep_canonical_identity_and_recheck_current_attachment() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }
        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Other artifact owner", None).await;
        let other = "artifact-foreign-worker";
        f.add_session(&foreign, other, Role::Worker, "Artifact reader")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, session) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(
                methods::ATTACH,
                json!({"sessionId":session,"token":f.token(session)}),
            )
            .await
            .unwrap();
        }
        let captured = decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await);
        let scope = captured["scope"].clone();
        let input = json!({"kind":"commit","value":"b".repeat(40),"repository":null});
        let reference = decoded(
            tool(
                &mut writer,
                "artifact_preview",
                json!({"crewId":f.crew,"input":input}),
            )
            .await,
        );
        assert_eq!(
            reference,
            json!({"namespace":{"kind":"global"},"key":format!("commit:{}", "b".repeat(40))})
        );
        let file = json!({"kind":"file","value":"Sources/Fixture.swift","repository":null});
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "artifact_preview",
                    json!({"crewId":f.crew,"input":file})
                )
                .await
            ),
            json!({"namespace":{"kind":"unassigned","crewId":f.crew},"key":"file:Sources/Fixture.swift"})
        );
        assert_eq!(
            tool(
                &mut reader,
                "artifact_preview",
                json!({"crewId":f.crew,"input":file})
            )
            .await["isError"],
            true,
            "preview cannot create another crew's unassigned identity"
        );
        let mut repository_refs = Vec::new();
        for repository in ["repo-a", "repo-b"] {
            repository_refs.push(decoded(tool(&mut writer, "artifact_preview", json!({
                "crewId":f.crew,"input":{"kind":"file","value":"Sources/Fixture.swift","repository":repository}
            })).await));
        }
        assert_ne!(repository_refs[0], repository_refs[1]);
        let register = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
                              "change":{"kind":"register","input":input}});
        let mut changes = f.state.hub.subscribe_collab();
        let receipt = decoded(tool(&mut writer, "artifact_submit", register.clone()).await);
        assert_eq!(
            receipt["effect"],
            json!({"kind":"registered","created":true,"claimAdded":true})
        );
        assert_eq!(
            decoded(tool(&mut writer, "artifact_submit", register.clone()).await),
            receipt
        );
        let lookup = json!({"scope":scope,"crewId":f.crew,"requestId":register["requestId"]});
        assert_eq!(
            decoded(tool(&mut writer, "artifact_request_get", lookup.clone()).await),
            json!({"status":"known","receipt":receipt})
        );
        assert_eq!(
            decoded(tool(&mut reader, "artifact_get", json!({"reference":reference})).await),
            receipt["artifact"]
        );
        let mut no_op = register.clone();
        no_op["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let unchanged = decoded(tool(&mut writer, "artifact_submit", no_op).await);
        assert_eq!(
            unchanged["effect"],
            json!({"kind":"registered","created":false,"claimAdded":false})
        );
        let progress = std::iter::from_fn(|| changes.try_recv().ok())
            .filter(|change| matches!(change, Change::Progress { session_id, .. } if session_id == &f.worker))
            .count();
        assert_eq!(
            progress, 1,
            "lookup, exact replay, and no-op claim must not count as fresh progress"
        );
        let decision = decoded(tool(&mut writer, "managed_object_submit", json!({
            "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"recordDecision","draft":{"question":"Which artifact to retain?",
                "choice":"Keep the exact pointer","rationale":"Explicit fixture","about":[{"kind":"crew","id":f.crew}]}}
        })).await)["object"].clone();
        let subject = json!({"kind":"decision","id":decision["decision"]["id"]});
        let link = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
                          "change":{"kind":"link","input":input,"subject":subject,"relation":"relates"}});
        let linked = decoded(tool(&mut writer, "artifact_submit", link.clone()).await);
        let edge = linked["effect"]["edge"].clone();
        assert_eq!(linked["change"], link["change"]);
        assert_eq!(
            edge["origin"],
            json!({"kind":"manual","actor":{"kind":"session","sessionId":f.worker}})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "artifact_backlinks",
                    json!({"reference":reference})
                )
                .await
            ),
            json!({"items":[edge],"nextAfter":null})
        );
        assert_eq!(
            decoded(tool(&mut reader, "artifact_subject", json!({"subject":subject})).await),
            json!({"items":[edge],"nextAfter":null})
        );
        assert_eq!(
            decoded(tool(&mut reader, "artifacts_list", json!({})).await)["items"],
            json!([receipt["artifact"]])
        );
        let mut stolen = link.clone();
        stolen["crewId"] = json!(foreign);
        stolen["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        assert_eq!(
            tool(&mut reader, "artifact_submit", stolen.clone()).await["isError"],
            true,
            "global reads must not confer edit authority on another crew's Decision"
        );
        assert_eq!(
            tool(&mut reader, "artifact_request_get", lookup.clone()).await["isError"],
            true
        );
        let mut stale = register.clone();
        stale["scope"]["restoreEpoch"] = json!(uuid::Uuid::new_v4().to_string());
        stale["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        assert_eq!(
            tool(&mut writer, "artifact_submit", stale.clone()).await["isError"],
            true
        );
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "artifact_request_get",
                    json!({
                        "scope":stale["scope"],"crewId":f.crew,"requestId":register["requestId"]}
                    )
                )
                .await
            ),
            json!({"status":"historical","receipt":null}),
            "synthetic mismatched epoch checks parameter semantics only"
        );
        let unlink = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"unlink","reference":reference,"subject":subject,"relation":"relates","expectedEdgeId":edge["id"]}});
        let unlinked = decoded(tool(&mut writer, "artifact_submit", unlink.clone()).await);
        assert_eq!(
            unlinked["effect"],
            json!({"kind":"unlinked","edgeId":edge["id"]})
        );
        assert_eq!(
            decoded(tool(&mut writer, "artifact_submit", unlink).await),
            unlinked
        );
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "artifact_request_get",
                    json!({
                        "scope":scope,"crewId":f.crew,"requestId":link["requestId"]}
                    )
                )
                .await
            ),
            json!({"status":"known","receipt":linked})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "managed_object_get",
                    json!({"id":decision["decision"]["id"]})
                )
                .await
            ),
            decision
        );

        let unassigned = decoded(
            tool(
                &mut writer,
                "artifact_submit",
                json!({
                    "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
                    "change":{"kind":"register","input":file}
                }),
            )
            .await,
        )["artifact"]
            .clone();
        let other_decision = decoded(tool(&mut reader, "managed_object_submit", json!({
            "scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"recordDecision","draft":{"question":"Which shared pointer?",
                "choice":"Keep the existing unassigned pointer","rationale":"Explicit cross-crew fixture",
                "about":[{"kind":"crew","id":foreign}]}}
        })).await)["object"].clone();
        let other_subject = json!({"kind":"decision","id":other_decision["decision"]["id"]});
        let existing = json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"linkExisting","reference":unassigned["reference"],"subject":other_subject,"relation":"relates"}});
        let held = decoded(tool(&mut reader, "artifact_submit", existing.clone()).await);
        assert_eq!(held["effect"]["created"], false);
        assert_eq!(held["effect"]["added"], true);
        assert_eq!(held["artifact"], unassigned);
        let held_edge = held["effect"]["edge"].clone();
        assert_eq!(held_edge["crewId"], foreign);
        assert_eq!(
            held_edge["reference"]["namespace"],
            json!({"kind":"unassigned","crewId":f.crew})
        );
        let mut missing_foreign = existing.clone();
        missing_foreign["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        missing_foreign["change"]["reference"]["key"] = json!("file:NeverRegistered.swift");
        assert_eq!(
            tool(&mut reader, "artifact_submit", missing_foreign).await["isError"],
            true,
            "linkExisting cannot implicitly create a foreign unassigned pointer"
        );
        f.add_session(&foreign, &f.worker, Role::Worker, "Moved artifact user")
            .await;
        assert_eq!(
            tool(&mut writer, "artifact_submit", register.clone()).await["isError"],
            true
        );
        assert_eq!(
            tool(
                &mut writer,
                "artifact_preview",
                json!({"crewId":f.crew,"input":file})
            )
            .await["isError"],
            true
        );
        assert_eq!(
            decoded(tool(&mut writer, "artifact_get", json!({"reference":reference})).await),
            receipt["artifact"]
        );
        f.state.tokens.revoke(&f.worker);
        for (name, args) in [
            ("artifact_preview", json!({"crewId":foreign,"input":file})),
            ("artifacts_list", json!({})),
            ("artifact_get", json!({"reference":reference})),
            ("artifact_backlinks", json!({"reference":reference})),
            ("artifact_subject", json!({"subject":subject})),
            ("artifact_submit", stolen),
            ("artifact_request_get", lookup),
        ] {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "invalidated attachment used {name}"
            );
        }
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "artifact_backlinks",
                    json!({"reference":reference})
                )
                .await
            ),
            json!({"items":[],"nextAfter":null})
        );
        let requirement = f.rpc("todo.create", json!({
            "crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"text":"Preserve frozen ledger"
        })).await;
        let work = f.rpc("work.create", json!({
            "crewId":f.crew,"title":"Retained ledger","goal":"Preserve current ledger",
            "acceptance":"External edges never change the ledger","sources":[requirement["requirementId"]]
        })).await;
        let purged = f.crew.clone();
        f.state
            .collab(move |tx| crewd_collab::crews::archive(tx, &purged))
            .await
            .unwrap();
        let frozen = f.rpc("work.get", json!({"id":work["id"]})).await;
        assert_eq!(frozen["frozen"], true);
        let frozen_subject = json!({"kind":"work","id":work["id"]});
        let frozen_link = json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"linkExisting","reference":unassigned["reference"],
                      "subject":frozen_subject,"relation":"evidence"}});
        let retained_edge =
            decoded(tool(&mut reader, "artifact_submit", frozen_link).await)["effect"]["edge"]
                .clone();
        assert_eq!(retained_edge["crewId"], foreign);
        assert_eq!(f.rpc("work.get", json!({"id":work["id"]})).await, frozen);
        let clear_frozen = json!({"scope":scope,"crewId":foreign,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"unlink","reference":unassigned["reference"],"subject":frozen_subject,
                      "relation":"evidence","expectedEdgeId":retained_edge["id"]}});
        let cleared = decoded(tool(&mut reader, "artifact_submit", clear_frozen).await);
        assert_eq!(
            cleared["effect"],
            json!({"kind":"unlinked","edgeId":retained_edge["id"]})
        );
        assert_eq!(
            f.rpc("work.get", json!({"id":work["id"]})).await,
            frozen,
            "B may maintain its own edges without editing A's frozen ledger"
        );
        let purged = f.crew.clone();
        f.state
            .collab(move |tx| crewd_collab::crews::purge(tx, &purged))
            .await
            .unwrap();
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "artifact_get",
                    json!({"reference":unassigned["reference"]})
                )
                .await
            ),
            unassigned,
            "the original namespace crew's purge cannot remove another crew's retained edge"
        );
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "artifact_backlinks",
                    json!({"reference":unassigned["reference"]})
                )
                .await
            ),
            json!({"items":[held_edge],"nextAfter":null})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut reader,
                    "artifact_request_get",
                    json!({
                        "scope":scope,"crewId":foreign,"requestId":existing["requestId"]}
                    )
                )
                .await
            ),
            json!({"status":"known","receipt":held})
        );
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn managed_message_tools_recheck_attached_scope_and_keep_private_sources() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }
        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign source reader", None).await;
        let other = "foreign-source-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign reader")
            .await;
        let channel = crewd_store::channels::lobby_id(&f.crew);
        let message = f
            .human_post(&channel, "Synthetic source e\u{301} kept private", vec![])
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, session) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(
                methods::ATTACH,
                json!({"sessionId":session,"token":f.token(session)}),
            )
            .await
            .unwrap();
        }
        let captured = decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await);
        let scope = captured["scope"].clone();
        let packet = json!({
            "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
            "sourceMessageId":message.id,"change":{"kind":"recordDecision","draft":{
                "question":"How to retain the result?","choice":"Keep the original",
                "rationale":"Synthetic attached capture","about":[{"kind":"crew","id":f.crew}]
            }}
        });
        let mut changes = f.state.hub.subscribe_collab();
        let receipt =
            decoded(tool(&mut writer, "managed_object_from_message", packet.clone()).await);
        assert_eq!(receipt["created"], true);
        assert_eq!(receipt["sourceMessageId"], message.id);
        assert_eq!(receipt["requestId"], packet["requestId"]);
        assert_eq!(
            receipt["object"]["decision"]["author"],
            json!({"kind":"session","sessionId":f.worker})
        );
        let id = receipt["object"]["decision"]["id"].clone();
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_from_message", packet.clone()).await),
            receipt
        );
        let lookup = json!({"scope":scope,"crewId":f.crew,"requestId":packet["requestId"]});
        assert_eq!(
            decoded(tool(&mut writer, "managed_message_request_get", lookup.clone()).await),
            json!({"status":"known","receipt":receipt})
        );
        let mut duplicate = packet.clone();
        duplicate["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let second = decoded(
            tool(
                &mut writer,
                "managed_object_from_message",
                duplicate.clone(),
            )
            .await,
        );
        assert_eq!(second["created"], false);
        assert_eq!(second["object"], receipt["object"]);
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "managed_object_from_message",
                    duplicate.clone()
                )
                .await
            ),
            second
        );
        let progress: Vec<_> = std::iter::from_fn(|| changes.try_recv().ok())
            .filter(|change| matches!(change, Change::Progress { session_id, .. } if session_id == &f.worker))
            .collect();
        assert_eq!(
            progress.len(),
            1,
            "only the new object may count as actual progress"
        );
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_source_get", json!({"id":id})).await),
            json!({"status":"available","crewId":f.crew,"messageId":message.id,
                          "channelId":channel,"excerpt":message.text})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "managed_objects_from_message",
                    json!({"messageId":message.id})
                )
                .await
            ),
            json!({"items":[receipt["object"]],"nextAfter":null})
        );

        assert_eq!(
            decoded(tool(&mut reader, "managed_object_get", json!({"id":id})).await),
            receipt["object"]
        );
        assert_eq!(
            decoded(tool(&mut reader, "managed_object_source_get", json!({"id":id})).await),
            json!({"status":"restricted"}),
            "object visibility must not expose source IDs or excerpt"
        );
        let mut foreign_record = packet.clone();
        foreign_record["crewId"] = json!(foreign);
        foreign_record["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        foreign_record["change"]["draft"]["about"] = json!([{"kind":"crew","id":foreign}]);
        for (name, args) in [
            ("managed_object_from_message", foreign_record.clone()),
            (
                "managed_objects_from_message",
                json!({"messageId":message.id}),
            ),
            ("managed_message_request_get", lookup.clone()),
        ] {
            assert_eq!(
                tool(&mut reader, name, args).await["isError"],
                true,
                "{name}"
            );
        }

        // Keep this exact attachment and token, but change its authoritative crew row.
        f.add_session(&foreign, &f.worker, Role::Worker, "Moved worker")
            .await;
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_get", json!({"id":id})).await),
            receipt["object"]
        );
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_source_get", json!({"id":id})).await),
            json!({"status":"restricted"})
        );
        for (name, args) in [
            ("managed_object_from_message", packet.clone()),
            ("managed_object_from_message", foreign_record.clone()),
            (
                "managed_objects_from_message",
                json!({"messageId":message.id}),
            ),
            ("managed_message_request_get", lookup.clone()),
        ] {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "current crew must be read again for {name}"
            );
        }
        f.state.tokens.revoke(&f.worker);
        for (name, args) in [
            ("managed_object_from_message", foreign_record),
            ("managed_object_source_get", json!({"id":id})),
            (
                "managed_objects_from_message",
                json!({"messageId":message.id}),
            ),
            ("managed_message_request_get", lookup),
        ] {
            assert_eq!(
                tool(&mut writer, name, args).await["isError"],
                true,
                "invalidated attachment must not use {name}"
            );
        }
        let objects = decoded(tool(&mut reader, "managed_decisions_list", json!({})).await);
        assert_eq!(objects["items"], json!([receipt["object"]["decision"]]));
        assert!(!std::iter::from_fn(|| changes.try_recv().ok())
            .any(|change| matches!(change, Change::Progress { session_id, .. } if session_id == f.worker)),
            "refused or read-only calls cannot extend progress");
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn managed_decision_tools_keep_attached_authority_and_original_receipts() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }

        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign decision fixture", None).await;
        let other = "foreign-decision-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign worker")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut author = RpcClient::connect(&socket).await;
        let mut captain = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, id) in [
            (&mut author, f.worker.as_str()),
            (&mut captain, f.captain.as_str()),
            (&mut reader, other),
        ] {
            peer.call(methods::ATTACH, json!({"sessionId":id,"token":f.token(id)}))
                .await
                .unwrap();
            let catalog = peer.call(methods::TOOLS, json!({})).await.unwrap();
            assert!(
                catalog
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["name"] == "managed_decisions_list")
            );
        }
        let captured = decoded(tool(&mut author, "managed_object_scope_get", json!({})).await);
        let scope = captured["scope"].clone();
        let draft = json!({
            "question":"Where should we keep the review?",
            "choice":"In this crew",
            "rationale":"The owning crew can follow it up.",
            "alternatives":null,
            "reversibility":"reversible",
            "about":[{"kind":"crew","id":f.crew}],
        });
        let submit = |change: Value| {
            json!({
                "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":change,
            })
        };
        let record = submit(json!({"kind":"recordDecision","draft":draft}));
        let mut forged = record.clone();
        forged["change"]["draft"]["author"] = json!({"kind":"human"});
        assert_eq!(
            tool(&mut author, "managed_object_submit", forged).await["isError"],
            true
        );
        let original = decoded(tool(&mut author, "managed_object_submit", record.clone()).await);
        let first = &original["object"]["decision"];
        let id = first["id"].as_str().unwrap();
        assert!(id.starts_with('D'));
        assert_eq!(first["status"], "active");
        assert_eq!(
            first["author"],
            json!({"kind":"session","sessionId":f.worker})
        );
        assert_eq!(
            decoded(tool(&mut author, "managed_object_submit", record.clone()).await),
            original
        );
        assert_eq!(
            decoded(tool(&mut reader, "managed_object_get", json!({"id":id})).await),
            original["object"]
        );
        let list = decoded(tool(&mut reader, "managed_decisions_list", json!({})).await);
        assert_eq!(list["items"], json!([first]));
        assert_eq!(
            tool(
                &mut reader,
                "managed_decisions_list",
                json!({"crewId":f.crew})
            )
            .await["isError"],
            true
        );
        let mut changed_draft = draft;
        changed_draft["choice"] = json!("In the shared review folder");
        let replace = submit(json!({
            "kind":"replaceDecision","decisionId":id,"expectedRevision":first["revision"],
            "draft":changed_draft,
        }));
        assert_eq!(
            tool(&mut reader, "managed_object_submit", replace.clone()).await["isError"],
            true
        );
        let mut stale = replace.clone();
        stale["change"]["expectedRevision"] = json!("99");
        assert_eq!(
            tool(&mut captain, "managed_object_submit", stale).await["isError"],
            true
        );
        let replaced = decoded(tool(&mut captain, "managed_object_submit", replace).await);
        let second = &replaced["object"]["decision"];
        let next_id = second["id"].as_str().unwrap();
        assert_ne!(next_id, id);
        assert_eq!(second["status"], "active");
        assert_eq!(
            second["author"],
            json!({"kind":"session","sessionId":f.captain})
        );
        let previous = decoded(tool(&mut reader, "managed_object_get", json!({"id":id})).await);
        assert_eq!(previous["decision"]["status"], "superseded");
        let page = decoded(tool(&mut reader, "managed_decisions_list", json!({})).await);
        assert_eq!(page["items"], json!([second, previous["decision"]]));
        let next_page = decoded(
            tool(
                &mut reader,
                "managed_decisions_list",
                json!({"after":next_id}),
            )
            .await,
        );
        assert_eq!(next_page["items"], json!([previous["decision"]]));
        let end = decoded(tool(&mut reader, "managed_decisions_list", json!({"after":id})).await);
        assert_eq!(end["items"], json!([]));
        assert_eq!(
            tool(&mut reader, "managed_decisions_list", json!({"after":"W1"})).await["isError"],
            true
        );
        let withdraw = submit(json!({
            "kind":"withdrawDecision","decisionId":next_id,"expectedRevision":second["revision"],
            "reason":"The underlying review has been cancelled.",
        }));
        assert_eq!(
            tool(&mut author, "managed_object_submit", withdraw.clone()).await["isError"],
            true
        );
        let withdrawn = decoded(tool(&mut captain, "managed_object_submit", withdraw).await);
        assert_eq!(withdrawn["object"]["decision"]["id"], next_id);
        assert_eq!(withdrawn["object"]["decision"]["status"], "withdrawn");

        let request = json!({"scope":scope,"crewId":f.crew,"requestId":record["requestId"]});
        let looked_up =
            decoded(tool(&mut author, "managed_object_request_get", request.clone()).await);
        assert_eq!(looked_up, json!({"status":"known","receipt":original}));
        f.state.tokens.revoke(&f.worker);
        assert_eq!(
            tool(&mut author, "managed_decisions_list", json!({})).await["isError"],
            true
        );
        assert_eq!(
            tool(&mut author, "managed_object_request_get", request).await["isError"],
            true
        );
        drop((author, captain, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn managed_risk_lesson_tools_keep_attached_authority_and_atomic_receipts() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }

        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign risk fixture", None).await;
        let other = "foreign-risk-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign worker")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, id) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(methods::ATTACH, json!({"sessionId":id,"token":f.token(id)}))
                .await
                .unwrap();
            let catalog = peer.call(methods::TOOLS, json!({})).await.unwrap();
            for name in ["managed_risks_list", "managed_lessons_list"] {
                assert!(
                    catalog
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["name"] == name),
                    "missing {name}"
                );
            }
        }
        let captured = decoded(tool(&mut writer, "managed_object_scope_get", json!({})).await);
        let scope = captured["scope"].clone();
        let submit = |change: Value| {
            json!({
                "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":change,
            })
        };
        let lookup = |request: &Value| json!({"scope":scope,"crewId":f.crew,"requestId":request["requestId"]});
        let risk_draft = json!({
            "summary":"Synthetic checklist step may fail",
            "impact":"high",
            "likelihood":null,
            "mitigation":"Keep a trace of each checklist run.",
            "about":[{"kind":"crew","id":f.crew}],
            "mitigatedBy":[],
        });
        let record = submit(json!({
            "kind":"recordRisk","draft":risk_draft,"owner":{"kind":"session","sessionId":f.worker},
        }));
        let original = decoded(tool(&mut writer, "managed_object_submit", record.clone()).await);
        let risk = &original["object"]["risk"];
        let risk_id = risk["id"].as_str().unwrap();
        assert!(risk_id.starts_with('R'));
        assert_eq!(risk["status"], "open");
        assert_eq!(
            risk["author"],
            json!({"kind":"session","sessionId":f.worker})
        );
        let listed = decoded(tool(&mut reader, "managed_risks_list", json!({})).await);
        assert_eq!(listed["items"], json!([risk]));
        let lesson_draft = json!({
            "happened":"A synthetic checklist run missed a required step.",
            "lesson":"Check the checklist against the audit trace.",
            "learnedFrom":[],
            "evidence":["Synthetic fixture audit trace"],
            "appliesTo":[{"kind":"crew","id":f.crew}],
        });
        let occurrence = submit(json!({
            "kind":"recordRiskOccurrence","riskId":risk_id,"expectedRevision":risk["revision"],
            "lessonDraft":lesson_draft,
        }));
        assert_eq!(
            tool(&mut reader, "managed_object_submit", occurrence.clone()).await["isError"],
            true
        );
        let mut stale = occurrence.clone();
        stale["change"]["expectedRevision"] = json!("99");
        assert_eq!(
            tool(&mut writer, "managed_object_submit", stale).await["isError"],
            true
        );
        let mut forged = occurrence.clone();
        forged["change"]["lessonDraft"]["learnedFrom"] = json!([{"kind":"risk","id":risk_id}]);
        assert_eq!(
            tool(&mut writer, "managed_object_submit", forged).await["isError"],
            true
        );
        let empty = decoded(tool(&mut reader, "managed_lessons_list", json!({})).await);
        assert_eq!(empty["items"], json!([]));
        let committed =
            decoded(tool(&mut writer, "managed_object_submit", occurrence.clone()).await);
        assert_eq!(committed["object"]["kind"], "lesson");
        assert_eq!(
            decoded(tool(&mut writer, "managed_object_submit", occurrence.clone()).await),
            committed
        );
        let occurred =
            decoded(tool(&mut reader, "managed_object_get", json!({"id":risk_id})).await);
        assert_eq!(occurred["risk"]["status"], "occurred");
        let lesson_id = occurred["risk"]["becameLessonId"].as_str().unwrap();
        assert!(lesson_id.starts_with('L'));
        let lesson =
            decoded(tool(&mut reader, "managed_object_get", json!({"id":lesson_id})).await);
        assert_eq!(lesson["lesson"]["riskId"], risk_id);
        assert_eq!(
            lesson["lesson"]["draft"]["learnedFrom"],
            json!([{"kind":"risk","id":risk_id}])
        );
        assert_eq!(lesson, committed["object"]);
        assert_eq!(
            committed["occurrence"],
            json!({
                "riskId":risk_id,"originalRiskRevision":risk["revision"],
                "resultRiskRevision":occurred["risk"]["revision"],
                "lessonId":lesson_id,"lessonRevision":lesson["lesson"]["revision"],
            })
        );

        let new_lesson_request = submit(json!({"kind":"recordLesson","draft":lesson_draft}));
        let separate =
            decoded(tool(&mut writer, "managed_object_submit", new_lesson_request).await);
        let separate_id = separate["object"]["lesson"]["id"].as_str().unwrap();
        assert_ne!(separate_id, lesson_id);
        let listed = decoded(tool(&mut reader, "managed_lessons_list", json!({})).await);
        assert_eq!(
            listed["items"],
            json!([separate["object"]["lesson"], lesson["lesson"]])
        );
        let page = decoded(
            tool(
                &mut reader,
                "managed_lessons_list",
                json!({"after":separate_id}),
            )
            .await,
        );
        assert_eq!(page["items"], json!([lesson["lesson"]]));
        let end = decoded(
            tool(
                &mut reader,
                "managed_lessons_list",
                json!({"after":lesson_id}),
            )
            .await,
        );
        assert_eq!(end["items"], json!([]));
        assert_eq!(
            tool(
                &mut reader,
                "managed_lessons_list",
                json!({"after":risk_id})
            )
            .await["isError"],
            true
        );

        let confirm = submit(json!({
            "kind":"confirmLesson","lessonId":lesson_id,"expectedRevision":lesson["lesson"]["revision"],
        }));
        let confirmed = decoded(tool(&mut reader, "managed_object_submit", confirm.clone()).await);
        assert_eq!(confirmed["object"]["lesson"]["id"], lesson_id);
        assert_ne!(
            confirmed["object"]["lesson"]["revision"],
            lesson["lesson"]["revision"]
        );
        assert_eq!(
            decoded(tool(&mut reader, "managed_object_request_get", lookup(&confirm)).await),
            json!({"status":"known","receipt":confirmed})
        );
        assert_eq!(
            decoded(
                tool(
                    &mut writer,
                    "managed_object_request_get",
                    lookup(&occurrence)
                )
                .await
            ),
            json!({"status":"known","receipt":committed})
        );
        f.state.tokens.revoke(&f.worker);
        assert_eq!(
            tool(&mut writer, "managed_risks_list", json!({})).await["isError"],
            true
        );
        assert_eq!(
            tool(
                &mut writer,
                "managed_object_request_get",
                lookup(&occurrence)
            )
            .await["isError"],
            true
        );
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn legacy_due_risk_requires_existing_injection_and_current_native_session() {
        let f = Fixture::new().await;
        let risk = crate::orchestra::exec_claude_managed::seed_due_risk(&f).await;
        let session = f.worker.clone();
        f.state
            .collab(move |tx| {
                tx.conn()
                    .execute(
                        "UPDATE sessions SET agent_session_id=?2 WHERE id=?1",
                        [session.as_str(), "legacy-native"],
                    )
                    .map_err(crewd_store::StoreError::from)?;
                Ok(())
            })
            .await
            .unwrap();
        f.set_live(crewd_proto::SessionLive {
            session_id: f.worker.clone(),
            running: true,
            ..Default::default()
        });
        let lobby = super::port::lobby_of(&f.state, &f.crew).await.unwrap();
        let mut worker = attached(&f, &f.worker).await;
        let send = |native: &str| {
            json!({"event":"post-tool-use","payload":{
                "session_id":native,"hook_event_name":"PostToolUse","tool_name":"Read",
            }})
        };
        let idle = worker
            .call(methods::HOOK, send("legacy-native"))
            .await
            .unwrap();
        assert!(idle.get("stdout").is_none());
        f.human_post(&lobby.id, "Actual first injection", vec![])
            .await;
        let forged = worker
            .call(methods::HOOK, send("another-native"))
            .await
            .unwrap();
        assert!(!forged.to_string().contains(&risk));
        f.human_post(&lobby.id, "Actual second injection", vec![])
            .await;
        let exact = worker
            .call(methods::HOOK, send("legacy-native"))
            .await
            .unwrap();
        let stdout: serde_json::Value =
            serde_json::from_str(exact["stdout"].as_str().unwrap()).unwrap();
        let body = stdout["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            body.contains("Actual second injection")
                && body.ends_with(&format!("{risk} is due for review."))
        );
        f.human_post(&lobby.id, "Actual third injection", vec![])
            .await;
        f.state.tokens.revoke(&f.worker);
        let revoked = worker
            .call(methods::HOOK, send("legacy-native"))
            .await
            .unwrap();
        assert!(!revoked.to_string().contains(&risk));
    }

    #[tokio::test]
    async fn decision_ask_tools_keep_original_receipt_and_opt_in_to_system_defaults() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await
                .unwrap()
        }

        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign decision ask fixture", None).await;
        let other = "foreign-decision-ask-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign worker")
            .await;
        let admin = "decision-ask-admin";
        f.add_session(&f.crew, admin, Role::Admin, "Fixture admin")
            .await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut author = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        let mut restricted = RpcClient::connect(&socket).await;
        for (peer, id) in [
            (&mut author, f.worker.as_str()),
            (&mut reader, other),
            (&mut restricted, admin),
        ] {
            peer.call(methods::ATTACH, json!({"sessionId":id,"token":f.token(id)}))
                .await
                .unwrap();
            let catalog = peer.call(methods::TOOLS, json!({})).await.unwrap();
            for name in ["ask_decision", "ask_decision_request_get"] {
                assert_eq!(
                    catalog
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["name"] == name),
                    id != admin,
                );
            }
        }

        let captured = decoded(tool(&mut author, "managed_object_scope_get", json!({})).await);
        let scope = &captured["scope"];
        let uuid = uuid::Uuid::new_v4().to_string();
        let original = json!({
            "scope":scope,"requestId":uuid,
            "draft":{
                "question":"Where should the synthetic fixture run?",
                "context":"The original options were cleared for organization-wide capture.",
                "options":[{"label":"Isolated workspace"},{"label":"Separate test machine"}],
                "about":[{"kind":"crew","id":f.crew}],
                "default":{"optionId":"A","afterMinutes":1},
                "reversibility":"reversible",
            },
        });
        let lookup = json!({"scope":scope,"requestId":uuid});
        let mut changes = f.state.hub.subscribe_collab();
        assert_eq!(
            tool(&mut restricted, "ask_decision", original.clone()).await["isError"],
            true
        );
        let mut forged = original.clone();
        forged["crewId"] = json!(foreign);
        assert_eq!(
            tool(&mut author, "ask_decision", forged).await["isError"],
            true
        );
        let first = decoded(tool(&mut author, "ask_decision", original.clone()).await);
        assert_eq!(first["scope"], *scope);
        assert_eq!(first["requestId"], uuid);
        assert_eq!(first["opened"]["crewId"], f.crew);
        assert!(first["opened"]["ask"].as_str().unwrap().starts_with('H'));
        assert!(first["opened"]["capture"].is_null());
        assert_eq!(
            decoded(tool(&mut author, "ask_decision", original.clone()).await),
            first
        );
        let mut changed = original;
        changed["draft"]["context"] = json!("Different context using the original UUID");
        assert_eq!(
            tool(&mut author, "ask_decision", changed).await["isError"],
            true
        );
        let other_lookup = tool(&mut reader, "ask_decision_request_get", lookup.clone()).await;
        if other_lookup["isError"] == false {
            assert_eq!(decoded(other_lookup), json!({"status":"notFound"}));
        }

        let (mut progress, mut announced) = (0, 0);
        while let Ok(change) = changes.try_recv() {
            match change {
                Change::Progress { session_id, .. } if session_id == f.worker => progress += 1,
                Change::AskChanged { crew_id, ask }
                    if crew_id == f.crew && first["opened"]["ask"] == ask =>
                {
                    announced += 1
                }
                _ => {}
            }
        }
        assert_eq!((progress, announced), (1, 1));

        let crew = f.crew.clone();
        let applied = f
            .state
            .db
            .call_result(move |store| {
                crewd_collab::run(store, crewd_store::now_ms() + 120_000, |tx| {
                    crewd_collab::asks::apply_due_defaults(tx, &crew)
                })
            })
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(
            applied,
            vec![first["opened"]["ask"].as_str().unwrap().to_owned()]
        );

        let ordinary = decoded(tool(&mut reader, "managed_decisions_list", json!({})).await);
        assert_eq!(ordinary["items"], json!([]));
        let opted = decoded(
            tool(
                &mut reader,
                "managed_decisions_list",
                json!({"includeSystemDefaults":true}),
            )
            .await,
        );
        let defaults = opted["items"].as_array().unwrap();
        assert_eq!(defaults.len(), 1);
        let decision = &defaults[0];
        let id = decision["id"].as_str().unwrap();
        assert!(id.starts_with('D'));
        assert_eq!(decision["author"]["kind"], "systemDefault");
        assert_eq!(
            tool(&mut reader, "managed_object_get", json!({"id":id})).await["isError"],
            true
        );
        let detail = decoded(
            tool(
                &mut reader,
                "managed_object_get",
                json!({"id":id,"includeSystemDefaults":true}),
            )
            .await,
        );
        assert_eq!(detail["decision"], *decision);

        assert_eq!(
            decoded(tool(&mut author, "ask_decision_request_get", lookup.clone()).await),
            json!({"status":"known","receipt":first}),
        );
        f.state.tokens.revoke(&f.worker);
        assert_eq!(
            tool(&mut author, "ask_decision_request_get", lookup).await["isError"],
            true
        );
        drop((author, reader, restricted));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
    }

    fn assert_role_tools(value: &serde_json::Value, role: Role) {
        use std::collections::BTreeSet;
        let actual: BTreeSet<_> = value
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        let identity = crate::agent::Identity {
            session_id: "expected-session".into(),
            crew_id: "expected-crew".into(),
            role,
            label: "expected".into(),
        };
        let expected: BTreeSet<_> = crate::agent::catalog::tools_for(&identity, Locale::Zh)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            actual.len(),
            value.as_array().unwrap().len(),
            "duplicate tools"
        );
        assert_eq!(
            actual, expected,
            "RPC must expose the catalog for the expected role"
        );
        // Verify the named landed surface in addition to catalog parity.
        // Captain-only acceptance must never leak into the Worker table.
        for name in [
            "work_item_close",
            "work_closure_history",
            "channel_archive_check",
            "channel_archive_verified",
            "channel_restore_verified",
            "admin_case_handoff_request",
            "admin_case_handoff_get",
            "admin_case_handoff_list",
            "admin_case_custody_get",
            "admin_case_custody_list",
        ] {
            assert!(actual.contains(name), "missing landed tool {name}");
        }
        assert_eq!(
            actual.contains("admin_case_handoff_accept"),
            role == Role::Captain
        );
        assert_eq!(
            actual.contains("admin_case_custody_accept"),
            role == Role::Captain
        );
        assert_eq!(actual.contains("start_session"), role == Role::Captain);
    }

    #[test]
    fn knowledge_mcp_error_detail_is_closed_to_public_input_structure() {
        let typed = crewd_proto::RpcError::new(
            crewd_proto::RpcError::INVALID_PARAMS,
            "untrusted internal detail",
        )
        .with_refusal("data.placement.identity_required");
        let text = super::knowledge_failure_text(&typed);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["error"]["refusal"],
            "data.placement.identity_required"
        );
        assert_eq!(value["originalRequestOutcome"], "notDetermined");
        assert!(!text.contains("untrusted internal detail"));
        for error in [
            crewd_proto::RpcError::new(
                crewd_proto::RpcError::FORBIDDEN,
                "foreign object private path /private/unrelated",
            ),
            crewd_proto::RpcError::new(crewd_proto::RpcError::CONFLICT, "SQL/internal details"),
            crewd_proto::RpcError::new(
                crewd_proto::RpcError::INVALID_PARAMS,
                "unexpected input detail",
            )
            .with_refusal("foreign.other"),
        ] {
            let text = super::knowledge_failure_text(&error);
            assert!(!text.contains(&error.message));
            assert!(text.starts_with("Data operation unavailable or refused"));
        }
    }

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
        assert_role_tools(
            &w.call(methods::TOOLS, json!(null)).await.unwrap(),
            Role::Worker,
        );
        let mut cap = attached(&f, &f.captain).await;
        assert_role_tools(
            &cap.call(methods::TOOLS, json!(null)).await.unwrap(),
            Role::Captain,
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
        assert_role_tools(
            &w.call(methods::TOOLS, json!(null)).await.unwrap(),
            Role::Captain,
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
        assert_role_tools(&n, Role::Worker);
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
