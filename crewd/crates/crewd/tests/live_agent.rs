//! Authorized opt-in smoke: exactly two short prompts, one test-owned serve and Claude CLI.
//! Run only this named test; never in check.sh and never record a terminal fixture.
#[path = "support/live.rs"]
mod live;
use crewd_server::agent::testkit::Fixture;
use crewd_server::agent::{Role, port};
use serde_json::json;

#[tokio::test]
#[ignore = "real Claude: requires CREWD_LIVE_CLAUDE=1 and --ignored --exact"]
async fn real_claude_uses_crewd_mcp_and_receives_injection() {
    live::authorized();
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let mut cli = live::Claude::start(&f, "live-smoke")
        .await
        .expect("test-owned Claude startup");
    let first = cli.prompt(&format!("用 crew 的 post 工具往大厅（channel={lobby}）发一句「冒烟：通了」，然后结束这一轮。不要做其他任务。")).await;
    let rows = port::messages_raw(&f.state, &lobby, None, 100)
        .await
        .unwrap();
    let posted = rows.iter().any(|m| {
        m.sender_session_id.as_deref() == Some(&f.worker) && m.text.contains("冒烟：通了")
    });
    let mut second = Err("first prompt did not complete/post; second prompt not sent".to_string());
    if first.is_ok() && posted {
        f.human_post(&lobby, "冒烟暗号：青蛙九号", vec![]).await;
        f.set_role(&f.worker, Role::Captain).await;
        second = cli.prompt(&format!("你现在是机长了。先调用 crew 的 list_sessions，再调用一次 list_channels。工具调用后会收到群里新来的消息：把其中的冒烟暗号用 post 发到大厅（channel={lobby}）。最后用 Bash 运行一次 false（无文件操作、故意退出 1），不要重试，然后结束这一轮。")).await;
    }
    // Always close the exact child before assertions, including auth/policy/failure paths.
    let trace = cli.finish().await;
    let rows = port::messages_raw(&f.state, &lobby, None, 100)
        .await
        .unwrap();
    let injected = rows
        .iter()
        .any(|m| m.sender_session_id.as_deref() == Some(&f.worker) && m.text.contains("青蛙九号"));
    let hooks = f.hook_observations(&f.worker);
    let failure_fired = hooks
        .iter()
        .any(|r| r.event == "PostToolUseFailure" && r.tool_name.as_deref() == Some("Bash"));
    let search_order = trace.no_search_before_first_crew_call();
    eprintln!(
        "SMOKE {}",
        json!({
            "posted": posted, "secret_returned": injected,
            "first_result": first.as_ref().map(|v| v.get("usage")).map_err(String::as_str),
            "second_result": second.as_ref().map(|v| v.get("usage")).map_err(String::as_str),
            "tool_search_order": search_order.as_ref().map(|_| "verified complete structured trace").map_err(String::as_str),
            "post_tool_use_failure_started_false": failure_fired,
            "role_refresh_list_sessions_succeeded": trace.succeeded("mcp__crew__list_sessions"),
            "ask_user_question_absent_from_init": trace.ask_user_question_absent(),
            "hooks": hooks.iter().map(|r| json!({"event": r.event, "tool": r.tool_name, "context_chars": r.context_chars})).collect::<Vec<_>>(),
            "results": trace.results,
        })
    );
    assert!(first.is_ok(), "first authorized prompt failed: {first:?}");
    assert!(posted, "post did not reach serve");
    assert!(
        second.is_ok(),
        "second authorized prompt failed: {second:?}"
    );
    assert!(
        injected,
        "PostToolUse secret did not come back through post"
    );
    assert!(
        search_order.is_ok(),
        "no-search ordering unverified/failing: {search_order:?}"
    );
}
