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
                return to_value(catalog::case_tools_for(&me, loc));
            }
            let mut list = catalog::tools_for(&me, loc);
            if !scope.is_live()
                || !conn.data_token.as_ref().is_some_and(|token| state.tokens.verify(&session, token))
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
            if catalog::requires_managed_scope(&p.tool) || matches!(
                p.tool.as_str(),
                "channel_archive_check" | "channel_archive_verified" | "channel_restore_verified"
                    | crewd_collab::work_coordination::SPLIT_TOOL
                    | crewd_collab::requirement_ruling::CAPTAIN_CHECK_TOOL
                    | crewd_collab::requirement_ruling::CAPTAIN_CLOSE_TOOL
            ) || (p.tool == "work_item_assign" && p.args.get("coordination_decision_id").is_some()) {
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
        if existing.encode_utf16().count() + line.len() + 1 <= state.agent.config.inject_cap_chars {
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
    async fn managed_message_tools_recheck_attached_scope_and_keep_private_sources() {
        use serde_json::Value;

        async fn tool(peer: &mut RpcClient, name: &str, args: Value) -> Value {
            peer.call(methods::CALL, json!({"tool":name,"args":args}))
                .await.unwrap()
        }
        fn decoded(result: Value) -> Value {
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }

        let f = Fixture::new().await;
        let foreign = f.add_crew("Foreign source reader", None).await;
        let other = "foreign-source-worker";
        f.add_session(&foreign, other, Role::Worker, "Foreign reader").await;
        let channel = crewd_store::channels::lobby_id(&f.crew);
        let message = f.human_post(&channel, "Synthetic source e\u{301} kept private", vec![]).await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut writer = RpcClient::connect(&socket).await;
        let mut reader = RpcClient::connect(&socket).await;
        for (peer, session) in [(&mut writer, f.worker.as_str()), (&mut reader, other)] {
            peer.call(methods::ATTACH, json!({"sessionId":session,"token":f.token(session)}))
                .await.unwrap();
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
        let receipt = decoded(tool(&mut writer, "managed_object_from_message", packet.clone()).await);
        assert_eq!(receipt["created"], true);
        assert_eq!(receipt["sourceMessageId"], message.id);
        assert_eq!(receipt["requestId"], packet["requestId"]);
        assert_eq!(receipt["object"]["decision"]["author"],
                   json!({"kind":"session","sessionId":f.worker}));
        let id = receipt["object"]["decision"]["id"].clone();
        assert_eq!(decoded(tool(&mut writer, "managed_object_from_message", packet.clone()).await), receipt);
        let lookup = json!({"scope":scope,"crewId":f.crew,"requestId":packet["requestId"]});
        assert_eq!(decoded(tool(&mut writer, "managed_message_request_get", lookup.clone()).await),
                   json!({"status":"known","receipt":receipt}));
        let mut duplicate = packet.clone();
        duplicate["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let second = decoded(tool(&mut writer, "managed_object_from_message", duplicate.clone()).await);
        assert_eq!(second["created"], false);
        assert_eq!(second["object"], receipt["object"]);
        assert_eq!(decoded(tool(&mut writer, "managed_object_from_message", duplicate.clone()).await), second);
        let progress: Vec<_> = std::iter::from_fn(|| changes.try_recv().ok())
            .filter(|change| matches!(change, Change::Progress { session_id, .. } if session_id == &f.worker))
            .collect();
        assert_eq!(progress.len(), 1, "only the new object may count as actual progress");
        assert_eq!(decoded(tool(&mut writer, "managed_object_source_get", json!({"id":id})).await),
                   json!({"status":"available","crewId":f.crew,"messageId":message.id,
                          "channelId":channel,"excerpt":message.text}));
        assert_eq!(decoded(tool(&mut writer, "managed_objects_from_message", json!({"messageId":message.id})).await),
                   json!({"items":[receipt["object"]],"nextAfter":null}));

        assert_eq!(decoded(tool(&mut reader, "managed_object_get", json!({"id":id})).await), receipt["object"]);
        assert_eq!(decoded(tool(&mut reader, "managed_object_source_get", json!({"id":id})).await),
                   json!({"status":"restricted"}), "object visibility must not expose source IDs or excerpt");
        let mut foreign_record = packet.clone();
        foreign_record["crewId"] = json!(foreign);
        foreign_record["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        foreign_record["change"]["draft"]["about"] = json!([{"kind":"crew","id":foreign}]);
        for (name, args) in [
            ("managed_object_from_message", foreign_record.clone()),
            ("managed_objects_from_message", json!({"messageId":message.id})),
            ("managed_message_request_get", lookup.clone()),
        ] {
            assert_eq!(tool(&mut reader, name, args).await["isError"], true, "{name}");
        }

        // Keep this exact attachment and token, but change its authoritative crew row.
        f.add_session(&foreign, &f.worker, Role::Worker, "Moved worker").await;
        assert_eq!(decoded(tool(&mut writer, "managed_object_get", json!({"id":id})).await), receipt["object"]);
        assert_eq!(decoded(tool(&mut writer, "managed_object_source_get", json!({"id":id})).await),
                   json!({"status":"restricted"}));
        for (name, args) in [
            ("managed_object_from_message", packet.clone()),
            ("managed_object_from_message", foreign_record.clone()),
            ("managed_objects_from_message", json!({"messageId":message.id})),
            ("managed_message_request_get", lookup.clone()),
        ] {
            assert_eq!(tool(&mut writer, name, args).await["isError"], true,
                       "current crew must be read again for {name}");
        }
        f.state.tokens.revoke(&f.worker);
        for (name, args) in [
            ("managed_object_from_message", foreign_record),
            ("managed_object_source_get", json!({"id":id})),
            ("managed_objects_from_message", json!({"messageId":message.id})),
            ("managed_message_request_get", lookup),
        ] {
            assert_eq!(tool(&mut writer, name, args).await["isError"], true,
                       "invalidated attachment must not use {name}");
        }
        let objects = decoded(tool(&mut reader, "managed_decisions_list", json!({})).await);
        assert_eq!(objects["items"], json!([receipt["object"]["decision"]]));
        assert!(!std::iter::from_fn(|| changes.try_recv().ok())
            .any(|change| matches!(change, Change::Progress { session_id, .. } if session_id == f.worker)),
            "refused or read-only calls cannot extend progress");
        drop((writer, reader));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), server).await.unwrap().unwrap();
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
        let next_page =
            decoded(tool(&mut reader, "managed_decisions_list", json!({"after":next_id})).await);
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
