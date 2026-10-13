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
        assert_eq!(
            instructions,
            worldview::tools_file(Locale::Zh).get("instructions")
        );
        assert!(instructions.lines().count() <= 3);
        let version = m.init["result"]["serverInfo"]["version"].as_str().unwrap();
        assert_eq!(
            version,
            format!(
                "{}+{}",
                env!("CARGO_PKG_VERSION"),
                crewd_server::agent::surface::hash(Locale::Zh)
            )
        );
    }
    let wt = wm.tools().await;
    let ct = cm.tools().await;
    assert_eq!(wt.len(), 23);
    assert_eq!(ct.len(), 34);
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
    assert_eq!(wm.tools().await.len(), 34);
    assert_eq!(cm.tools().await.len(), 23);
    assert!(!wm.call("list_sessions", json!({})).await.1);
    assert!(cm.call("list_sessions", json!({})).await.1);
    f.set_role(&f.worker, Role::Worker).await;
    wm.changed().await;
    assert_eq!(wm.tools().await.len(), 23);
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
        let world = port::world_context(&f.state, &f.worker).await.unwrap();
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

fn synthetic_quota(
    claude: Option<&str>,
    codex: Option<&str>,
) -> crewd_server::control::QuotaReport {
    use crewd_server::control::{QuotaLine, QuotaReport};
    QuotaReport {
        lines: vec![
            QuotaLine {
                executor: "codex".into(),
                plan: codex.map(str::to_owned),
                limited_until: Some("2026-10-10T00:00:00Z".into()),
                detail: Some("synthetic-only".into()),
            },
            QuotaLine {
                executor: "claude".into(),
                plan: claude.map(str::to_owned),
                ..Default::default()
            },
        ],
    }
}

#[tokio::test]
async fn session_start_reads_typed_live_quota_after_launch() {
    for locale in [Locale::Zh, Locale::En] {
        let f = Fixture::new().await;
        f.state
            .collab(move |tx| crewd_collab::settings::set_locale(tx, locale))
            .await
            .unwrap();
        f.set_quota(synthetic_quota(
            Some("synthetic-Max100"),
            Some("synthetic-Plus"),
        ));
        let launch_ctx = port::world_context(&f.state, &f.worker).await.unwrap();
        let rt = tempfile::tempdir().unwrap();
        let w = support::launch_with_world(
            &f,
            &f.worker,
            rt.path(),
            Some(worldview::render(&launch_ctx)),
        );
        let launch_world = w.world.as_ref().unwrap();
        assert!(
            launch_world.contains("synthetic-Max100") && launch_world.contains("synthetic-Plus"),
            "launch dropped known typed quota: {launch_world}"
        );
        f.spawn_socket().await;
        // Same typed contract, changed after launch; not a launch-copy replay.
        f.set_quota(synthetic_quota(
            Some("synthetic-Max200"),
            Some("synthetic-Pro"),
        ));
        for source in ["resume", "compact", "clear", "fork"] {
            let out = fire(
                &w.settings,
                "SessionStart",
                json!({"hook_event_name":"SessionStart", "source":source}),
            )
            .await
            .unwrap();
            let ctx = out["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert!(
                ctx.contains("synthetic-Max200") && ctx.contains("synthetic-Pro"),
                "live quota wiring missing: {ctx}"
            );
            assert!(!ctx.contains("synthetic-Max100") && !ctx.contains("synthetic-Plus"));
            assert!(ctx.contains(&f.worker) && ctx.chars().count() <= 10_000);
        }
    }
}

#[tokio::test]
async fn session_start_quota_unknown_unavailable_and_failed_are_distinct() {
    use crewd_server::control::{ControlError, FakeMethod};
    for locale in [Locale::Zh, Locale::En] {
        let f = Fixture::new().await;
        f.state
            .collab(move |tx| crewd_collab::settings::set_locale(tx, locale))
            .await
            .unwrap();
        f.spawn_socket().await;
        let rt = tempfile::tempdir().unwrap();
        let w = launch(&f, &f.worker, rt.path());
        for (error, key) in [
            (None, "unknown"),
            (
                Some(ControlError::Unsupported("synthetic source missing".into())),
                "quota.unavailable",
            ),
            (
                Some(ControlError::Failed("synthetic-private-error".into())),
                "quota.failed",
            ),
        ] {
            f.set_quota(synthetic_quota(None, None));
            if let Some(e) = error {
                f.fail_method(FakeMethod::Quota, e);
            }
            let out = fire(
                &w.settings,
                "SessionStart",
                json!({"hook_event_name":"SessionStart", "source":"resume"}),
            )
            .await
            .expect("quota error must not hide role/identity refresh");
            let ctx = out["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            let text = worldview::world_file(locale);
            let status = text.get(key);
            let expected_quota = crewd_collab::prompts::fill(
                text.get("quota"),
                &[("claude", status), ("codex", status)],
            );
            assert!(
                ctx.contains(&expected_quota),
                "quota state {key} not observable in quota block: {ctx}"
            );
            assert!(ctx.contains(&f.worker));
            assert!(!ctx.contains("synthetic-private-error"));
        }
    }
}

#[tokio::test]
async fn work_tools_use_real_helper_and_shared_transactions_without_echoing_full_details() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let launch = launch(&f, &f.worker, rt.path());
    let mut m = Mcp::start(&launch.mcp).await;
    let goal = "large detail retained only when requested".repeat(200);
    let (text, error) = m
        .call(
            "work_item_create",
            json!({"title":"MCP planned work","goal":goal}),
        )
        .await;
    assert!(!error, "{text}");
    let created: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(created["owner"], f.worker);
    assert!(
        created.get("goal").is_none(),
        "creation receipt echoed long detail"
    );
    assert!(!text.contains("large detail"));
    let (text, error) = m.call("my_work_items", json!({})).await;
    assert!(!error, "{text}");
    let page: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(page["items"][0]["id"], created["id"]);
    assert!(page["items"][0].get("goal").is_none());
    assert!(!text.contains("large detail"));
    let (text, error) = m.call("work_item_get", json!({"id":created["id"]})).await;
    assert!(!error, "{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["goal"],
        goal
    );
    let (text,error)=m.call("work_item_update",json!({"id":created["id"],"expected_revision":page["items"][0]["revision"],"status":"done"})).await;
    assert!(!error, "{text}");
    let completed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(completed["status"], "done");
    assert!(completed["evidence"].is_null());
    let (text, error) = m.call("my_work_items", json!({})).await;
    assert!(!error, "{text}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (text, error) = m
        .call("my_work_items", json!({"include_finished":true}))
        .await;
    assert!(!error, "{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["items"][0]["status"],
        "done"
    );
    let (text,error)=m.call("work_item_update",json!({"id":created["id"],"expected_revision":created["revision"],"note":"stale change"})).await;
    assert!(error, "{text}");
    assert!(
        text.contains("currentWork"),
        "missing current facts: {text}"
    );
    assert!(text.contains("\"status\":\"done\""));
    assert!(text.contains(&format!(
        "\"revision\":\"{}\"",
        completed["revision"].as_str().unwrap()
    )));
    assert!(!text.contains("large detail"), "conflict echoed long goal");
    let (text, error) = m
        .call("work_item_create", json!({"title":"spoof","actor":"human"}))
        .await;
    assert!(error, "{text}");
    let crew = f.crew.clone();
    f.state
        .collab(move |tx| {
            tx.conn()
                .execute("UPDATE crews SET archived_at='frozen' WHERE id=?1", [crew])?;
            Ok(())
        })
        .await
        .unwrap();
    let (text, error) = m
        .call("my_work_items", json!({"include_finished":true}))
        .await;
    assert!(!error, "{text}");
    assert!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (text, error) = m
        .call(
            "my_work_items",
            json!({"include_finished":true,"include_removed":true}),
        )
        .await;
    assert!(!error, "{text}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["items"][0]["frozen"],
        true
    );
    m.close().await;
}

#[tokio::test]
async fn work_assignment_uses_actual_helper_identity_and_compact_receipts() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let crew = f.crew.clone();
    let goal = "synthetic long assignment goal".repeat(200);
    let p = f
        .state
        .collab(move |tx| {
            crewd_collab::work::create(
                tx,
                &crewd_collab::Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: crew,
                    title: "unowned helper work".into(),
                    goal,
                    ..Default::default()
                },
            )
        })
        .await
        .unwrap();
    let rt = tempfile::tempdir().unwrap();
    let worker = launch(&f, &f.worker, rt.path());
    let captain = launch(&f, &f.captain, rt.path());
    let mut w = Mcp::start(&worker.mcp).await;
    let mut c = Mcp::start(&captain.mcp).await;
    let (text,error)=w.call("work_item_assign",json!({"id":p.id,"expected_revision":p.revision,"owner":f.worker,"note":"claim unowned work"})).await;
    assert!(!error, "{text}");
    assert!(!text.contains("long assignment goal"));
    let claimed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(claimed["owner"], f.worker);
    assert!(claimed.get("goal").is_none());
    let (text,error)=c.call("work_item_assign",json!({"id":p.id,"expected_revision":claimed["revision"],"owner":f.captain,"note":"captain takes ownership"})).await;
    assert!(!error, "{text}");
    let assigned: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(assigned["owner"], f.captain);
    let (history_text, history_error) = w
        .call("work_item_history", json!({"id":p.id,"limit":1}))
        .await;
    assert!(!history_error, "{history_text}");
    let history: serde_json::Value = serde_json::from_str(&history_text).unwrap();
    assert_eq!(history["items"][0]["note"], "captain takes ownership");
    assert_eq!(history["items"][0]["by"]["id"], f.captain);
    assert!(!history_text.contains("long assignment goal"));
    let (older_text, older_error) = w
        .call(
            "work_item_history",
            json!({"id":p.id,"before":history["nextBefore"],"limit":1}),
        )
        .await;
    assert!(!older_error, "{older_text}");
    let older: serde_json::Value = serde_json::from_str(&older_text).unwrap();
    assert_eq!(older["items"][0]["note"], "claim unowned work");
    assert!(older["nextBefore"].is_null());

    let (text,error)=w.call("work_item_assign",json!({"id":p.id,"expected_revision":assigned["revision"],"owner":f.worker,"note":"attempted steal"})).await;
    assert!(error, "{text}");
    let (text,error)=w.call("work_item_assign",json!({"id":p.id,"expected_revision":assigned["revision"],"owner":f.worker,"note":"spoof","actor":"human"})).await;
    assert!(error, "{text}");
    let (text,error)=c.call("work_item_assign",json!({"id":p.id,"expected_revision":claimed["revision"],"owner":f.worker,"note":"stale assignment"})).await;
    assert!(error, "{text}");
    assert!(text.contains("currentWork"));
    assert!(!text.contains("long assignment goal"));
    w.close().await;
    c.close().await;
}

#[tokio::test]
async fn pending_transfer_is_detailed_on_request_and_bounded_in_stale_reply() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let crew = f.crew.clone();
    let owner = f.worker.clone();
    let target = f.captain.clone();
    let note = "synthetic full pending offer 🙂".repeat(400);
    let original = note.clone();
    let p = f
        .state
        .collab(move |tx| {
            let p = crewd_collab::work::create(
                tx,
                &crewd_collab::Sender::Session(owner.clone()),
                crewd_proto::WorkCreateParams {
                    crew_id: crew.clone(),
                    title: "pending detail fixture".into(),
                    ..Default::default()
                },
            )?;
            crewd_collab::asks::with_book(tx, &crew, |b, now| {
                b.request_transfer(
                    &crewd_work::Actor::worker(&owner),
                    now,
                    "W1".parse().unwrap(),
                    crewd_work::SessionId::new(&target),
                    &note,
                )
            })?;
            Ok(p)
        })
        .await
        .unwrap();
    let rt = tempfile::tempdir().unwrap();
    let l = launch(&f, &f.worker, rt.path());
    let mut m = Mcp::start(&l.mcp).await;
    let (text, error) = m.call("work_item_get", json!({"id":p.id})).await;
    assert!(!error, "{text}");
    let plan: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(plan["owner"], f.worker);
    assert_eq!(plan["pendingTransfer"]["to"], f.captain);
    assert_eq!(plan["pendingTransfer"]["requestedBy"], f.worker);
    assert_eq!(plan["pendingTransfer"]["note"], original);
    assert_eq!(plan["pendingTransfer"]["noteTruncated"], false);
    let (text, error) = m.call("my_work_items", json!({})).await;
    assert!(!error, "{text}");
    assert!(!text.contains("pendingTransfer"));
    assert!(!text.contains("synthetic full pending offer"));
    let (text, error) = m
        .call(
            "work_item_update",
            json!({"id":p.id,"expected_revision":p.revision,"note":"stale proposal report"}),
        )
        .await;
    assert!(error, "{text}");
    assert!(text.contains("pendingTransfer"));
    assert!(text.contains("\"noteTruncated\":true"));
    assert!(!text.contains(&original));
    assert!(text.len() < 4096);
    m.close().await;
}
