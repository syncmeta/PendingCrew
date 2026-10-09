//! Real serve + real helper driven by prepare_claude's generated config. No real Claude.
#[path = "support/agent.rs"]
mod support;
use crewd_collab::{Locale, worldview};
use crewd_proto::{Mention, MentionKind, SenderKind};
use crewd_server::agent::testkit::{Fixture, RpcClient};
use crewd_server::agent::{Role, catalog, port};
use serde_json::json;
use support::{Mcp, fire, launch};

#[tokio::test]
async fn captain_and_worker_tables_differ_and_role_change_applies_live() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let c = launch(&f, &f.captain, rt.path());
    let mut wm = Mcp::start(&w.mcp).await;
    let mut cm = Mcp::start(&c.mcp).await;
    for m in [&wm, &cm] {
        assert_eq!(
            m.init["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        let instructions = m.init["result"]["instructions"]
            .as_str()
            .expect("MCP instructions");
        assert!(!instructions.is_empty() && instructions.lines().count() <= 3);
        let version = m.init["result"]["serverInfo"]["version"].as_str().unwrap();
        assert!(
            version.contains('+'),
            "surface hash must reach initialize: {version}"
        );
    }
    let wt = wm.tools().await;
    let ct = cm.tools().await;
    assert_eq!(wt.len(), 17);
    assert_eq!(ct.len(), 28);
    for t in wt.iter().chain(&ct) {
        assert_eq!(t["annotations"]["openWorldHint"], false);
        for k in ["readOnlyHint", "destructiveHint", "idempotentHint"] {
            assert!(t["annotations"][k].is_boolean(), "{t}");
        }
    }
    assert!(wm.call("list_sessions", json!({})).await.1);
    f.set_role(&f.worker, Role::Captain).await;
    wm.changed().await;
    cm.changed().await;
    assert_eq!(wm.tools().await.len(), 28);
    assert_eq!(cm.tools().await.len(), 17);
    assert!(!wm.call("list_sessions", json!({})).await.1);
    assert!(cm.call("list_sessions", json!({})).await.1);
    f.set_role(&f.worker, Role::Worker).await;
    wm.changed().await;
    assert_eq!(wm.tools().await.len(), 17);
    wm.close().await;
    cm.close().await;
}

#[tokio::test]
async fn post_via_mcp_then_injection_via_post_tool_use_hook() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let c = launch(&f, &f.captain, rt.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let mut cm = Mcp::start(&c.mcp).await;
    let (text, err) = cm
        .call(
            "post",
            json!({"channel": lobby, "message": "synthetic-mcp-post"}),
        )
        .await;
    assert!(!err, "{text}");
    f.add_session(&f.crew, "s-peer", Role::Worker, "另一个 worker")
        .await;
    f.session_post(
        &f.captain,
        &lobby,
        "synthetic-directed-other",
        vec![Mention {
            kind: MentionKind::Session,
            target_id: Some("s-peer".into()),
        }],
    )
    .await;
    let peer_launch = launch(&f, "s-peer", rt.path());
    let mut peer = Mcp::start(&peer_launch.mcp).await;
    assert!(!peer.call("post", json!({"channel": lobby, "message": "synthetic-peer-progress", "category": "progress"})).await.1);
    let payload =
        json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": "synthetic"});
    let out = fire(&w.settings, "PostToolUse", payload.clone())
        .await
        .expect("new unread must be injected");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let ctx = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        ctx.contains("synthetic-mcp-post") && ctx.contains("synthetic-directed-other"),
        "{ctx}"
    );
    assert!(
        ctx.contains("另一个 worker"),
        "mention marker must survive: {ctx}"
    );
    assert!(
        !ctx.contains("synthetic-peer-progress"),
        "progress body must fold: {ctx}"
    );
    assert!(fire(&w.settings, "PostToolUse", payload).await.is_none());
    f.human_post(&lobby, "synthetic-after-failure", vec![])
        .await;
    let out = fire(&w.settings, "PostToolUseFailure", json!({"hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "error": "synthetic exit 1"})).await.unwrap();
    assert_eq!(
        out["hookSpecificOutput"]["hookEventName"],
        "PostToolUseFailure"
    );
    assert!(
        out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("synthetic-after-failure")
    );
    let observed = f.hook_observations(&f.worker);
    assert_eq!(
        observed
            .iter()
            .map(|r| (r.event.as_str(), r.tool_name.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("PostToolUse", Some("Bash")),
            ("PostToolUse", Some("Bash")),
            ("PostToolUseFailure", Some("Bash"))
        ]
    );
    assert!(
        observed[0].context_chars > 0
            && observed[1].context_chars == 0
            && observed[2].context_chars > 0
    );
    cm.close().await;
    peer.close().await;
}

#[tokio::test]
async fn no_pre_tool_use_hook_and_ask_user_question_denied() {
    let f = Fixture::new().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    assert!(w.settings["hooks"].get("PreToolUse").is_none());
    assert_eq!(
        w.settings["permissions"]["deny"],
        json!(["AskUserQuestion"])
    );
    assert!(
        fire(&w.settings, "PreToolUse", json!({"tool_name": "Bash"}))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn mcp_json_always_loads_crew() {
    let f = Fixture::new().await;
    let rt = tempfile::tempdir().unwrap();
    assert_eq!(
        launch(&f, &f.worker, rt.path()).mcp["mcpServers"]["crew"]["alwaysLoad"],
        true
    );
}

#[tokio::test]
async fn session_start_on_resume_returns_live_facts() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    f.rpc(
        "crew.update",
        json!({"crewId": f.crew, "title": "renamed-live-crew"}),
    )
    .await;
    f.set_role(&f.worker, Role::Captain).await;
    for source in ["resume", "compact", "clear", "fork"] {
        let out = fire(
            &w.settings,
            "SessionStart",
            json!({"hook_event_name": "SessionStart", "source": source}),
        )
        .await
        .expect("live refresh missing");
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let ctx = out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.contains("renamed-live-crew") && ctx.contains(&f.worker),
            "{ctx}"
        );
        assert!(ctx.chars().count() <= 10_000);
        let session = f.worker.clone();
        let world = f
            .state
            .db
            .call(move |s| worldview::context_for(s.conn(), &session, Locale::Zh))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(world.role, Role::Captain);
        assert_eq!(ctx, worldview::render_live(&world));
    }
    assert!(
        fire(&w.settings, "SessionStart", json!({"source": "startup"}))
            .await
            .is_none()
    );
    assert!(
        fire(
            &w.settings,
            "SessionStart",
            json!({"source": "resume-not-an-exact-value"})
        )
        .await
        .is_none()
    );
    assert_eq!(
        f.hook_observations(&f.worker).len(),
        4,
        "all four configured sources run; startup and near-match do not"
    );
}

#[tokio::test]
async fn session_start_context_is_bounded_without_losing_identity_or_role() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let session = f.worker.clone();
    f.state
        .db
        .call(move |s| {
            s.conn().execute(
                "UPDATE sessions SET display_name = ?1 WHERE id = ?2",
                (&"长".repeat(20_000), session),
            )
        })
        .await
        .unwrap()
        .unwrap();
    let out = fire(&w.settings, "SessionStart", json!({"source": "fork"}))
        .await
        .unwrap();
    let context = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.chars().count() <= 10_000);
    assert!(
        context.contains(&f.worker) && context.contains(&f.crew),
        "identity must fit"
    );
    assert!(context.contains("worker"), "role must fit");
}

#[tokio::test]
async fn stop_hook_relays_once_per_prompt() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let stop =
        |p, t| json!({"hook_event_name": "Stop", "prompt_id": p, "last_assistant_message": t});
    fire(&w.settings, "Stop", stop("p0", "收到。")).await;
    let before = port::latest_seq(&f.state, &lobby).await.unwrap();
    let payload = stop("p1", "A 做完了。要不要接着做 B？");
    fire(&w.settings, "Stop", payload.clone()).await;
    let after = port::latest_seq(&f.state, &lobby).await.unwrap();
    assert_ne!(before, after);
    let rows = port::messages_raw(&f.state, &lobby, None, 20)
        .await
        .unwrap();
    let relay = rows.last().unwrap();
    assert_eq!(relay.sender_kind, SenderKind::Session);
    assert_eq!(relay.sender_session_id.as_deref(), Some(f.worker.as_str()));
    assert_eq!(relay.category.as_deref(), Some("question"));
    fire(&w.settings, "Stop", payload).await;
    assert_eq!(port::latest_seq(&f.state, &lobby).await.unwrap(), after);
}

#[tokio::test]
async fn wrong_token_or_no_attach_is_refused() {
    let f = Fixture::new().await;
    let sock = f.spawn_socket().await;
    let mut rpc = RpcClient::connect(&sock).await;
    for method in ["agent.tools", "agent.call", "agent.hook"] {
        assert_eq!(rpc.call(method, json!({})).await.unwrap_err().0, -32601);
    }
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let crew = &w.mcp["mcpServers"]["crew"];
    let args = crew["args"].as_array().unwrap();
    let tf = args[6].as_str().unwrap();
    std::fs::write(tf, "synthetic-forged-token").unwrap();
    let out = tokio::process::Command::new(crew["command"].as_str().unwrap())
        .args(args.iter().map(|a| a.as_str().unwrap()))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid session or token"));
    assert!(
        fire(&w.settings, "SessionStart", json!({"source": "resume"}))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn removed_tool_name_suggests_its_replacement() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let mut m = Mcp::start(&w.mcp).await;
    let names = m.tools().await;
    for (old, replacement) in catalog::REMOVED.iter().filter(|(_, r)| !r.is_empty()) {
        assert!(!names.iter().any(|t| t["name"] == *old));
        let (text, err) = m.call(old, json!({})).await;
        assert!(err && text.contains(replacement), "{old}: {text}");
    }
    let (text, err) = m.call("synthetic-unknown", json!({})).await;
    assert!(err && text.contains("synthetic-unknown"));
    m.close().await;
}
