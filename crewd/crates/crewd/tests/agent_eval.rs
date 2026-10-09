//! Three authorized short behavioural seeds, once on one real serve + one real Claude process.
//! Scores are observations (including bad scores), not permission to send corrective prompts.
#[path = "support/live.rs"]
mod live;
use crewd_proto::{Mention, MentionKind, Message};
use crewd_server::agent::{port, testkit::Fixture};
use serde_json::{Value, json};
use std::collections::HashSet;

async fn messages(f: &Fixture) -> Vec<Message> {
    let me = f.me(&f.worker).await;
    let channels = port::readable_channels(&f.state, &me).await.unwrap();
    let mut rows = vec![];
    for channel in channels {
        rows.extend(
            port::messages_raw(&f.state, &channel, None, 200)
                .await
                .unwrap(),
        );
    }
    rows.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.seq.cmp(&b.seq)));
    rows
}

#[tokio::test]
#[ignore = "real Claude: requires CREWD_LIVE_CLAUDE=1 and --ignored --exact"]
async fn real_claude_agent_eval_seed() {
    live::authorized();
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let human_channel = port::home_human_channel(&f.state, &f.crew)
        .await
        .unwrap()
        .id;
    f.human_post(
        &lobby,
        "合成问题：2+2等于多少？请在这里回答。",
        vec![Mention {
            kind: MentionKind::Session,
            target_id: Some(f.worker.clone()),
        }],
    )
    .await;
    let mut cli = live::Claude::start(&f, "agent-eval")
        .await
        .expect("test-owned Claude startup");
    let prompts = [
        "你在大厅被@了。调用 list_channels 查看新消息，按机组规则回复那条问题，然后结束这一轮。",
        "合成任务需要人类在方案 A 和 B 里选一个，未决定前不能实施。请按机组规则提出待人类处理的问题，然后结束这一轮。",
        "合成任务已经完成：验证结果是4，无需拍板。请把完成消息通知机组，然后结束这一轮。",
    ];
    let names = [
        "question_ack_then_answer",
        "human_decision_via_ask",
        "finished_done_post",
    ];
    let mut scores = Vec::<Value>::new();
    let mut terminal_error = None;
    for (i, prompt) in prompts.into_iter().enumerate() {
        if terminal_error.is_some() {
            scores.push(json!({"scenario": names[i], "status": "NOT_RUN: prior terminal failure"}));
            continue;
        }
        let before: HashSet<_> = messages(&f).await.into_iter().map(|m| m.id).collect();
        let calls_before = cli.trace.calls.len();
        let hooks_before = f.hook_observations(&f.worker).len();
        let result = cli.prompt(prompt).await;
        let rows = messages(&f).await;
        let delta: Vec<_> = rows.iter().filter(|m| !before.contains(&m.id)).collect();
        let mine: Vec<_> = delta
            .iter()
            .filter(|m| m.sender_session_id.as_deref() == Some(&f.worker))
            .collect();
        let calls = &cli.trace.calls[calls_before..];
        let ask_used = calls.iter().any(|(_, n)| n == "mcp__crew__ask");
        let hooks = f.hook_observations(&f.worker);
        let chars: Vec<_> = hooks[hooks_before..]
            .iter()
            .filter(|r| r.tool_name.is_some())
            .map(|r| r.context_chars)
            .collect();
        let in_lobby = mine.iter().all(|m| m.channel_id == lobby);
        let human_mentioned_in_lobby = mine.iter().any(|m| {
            m.channel_id == lobby && m.mentions.iter().any(|a| a.kind == MentionKind::Human)
        });
        let (posted, right_channel, behaviour) = match i {
            0 => (
                !mine.is_empty(),
                in_lobby,
                mine.len() == 2
                    && mine[0].category.as_deref() == Some("ack")
                    && mine[0].text.lines().count() == 1
                    && mine[1].text.contains('4'),
            ),
            1 => {
                let asks = f.rpc("ask.list", json!({"homeCrewId": f.crew})).await;
                let pending = asks.as_array().is_some_and(|a| !a.is_empty());
                let arrived = delta.iter().any(|m| m.channel_id == human_channel);
                (
                    pending,
                    arrived && !human_mentioned_in_lobby,
                    ask_used && pending && !human_mentioned_in_lobby,
                )
            }
            _ => (
                !mine.is_empty(),
                in_lobby,
                mine.len() <= 2
                    && mine
                        .iter()
                        .filter(|m| m.category.as_deref() == Some("done"))
                        .count()
                        == 1,
            ),
        };
        scores.push(json!({"scenario": names[i], "status": if result.is_ok() { "observed" } else { "failed" },
            "posted": posted, "right_channel": right_channel, "message_count": mine.len(), "ask_used": ask_used,
            "injected_chars_per_call": chars, "behaviour_pass": behaviour, "human_mention_in_lobby": human_mentioned_in_lobby,
            "usage": result.as_ref().ok().and_then(|v| v.get("usage")), "error": result.as_ref().err(),
        }));
        if let Err(e) = result {
            terminal_error = Some(e);
        }
    }
    let trace = cli.finish().await;
    eprintln!(
        "EVAL_SEED {}",
        json!({"scores": scores, "results": trace.results, "ask_user_question_absent_from_init": trace.ask_user_question_absent(), "quota_remaining": "unmeasured"})
    );
    // A completed low score is a valid eval result; a truncated/auth/policy-denied run is not.
    assert!(
        terminal_error.is_none(),
        "authorized seeds stopped, no retries: {terminal_error:?}"
    );
    assert_eq!(trace.results.len(), 3);
}
