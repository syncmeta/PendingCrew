//! pre-tool-use approval gate (controller ruling C2, after the Swift cockpit #75-2): a call that hits a
//! gate is denied on the spot and a pending request is filed for the human; the human's yes is a
//! one-time ticket that the next same-named call of the same session uses up. Nothing waits for the
//! human: only this one call is stuck. Tickets, de-duplication and the notice all live in M3a's single
//! `approvals::gate` transaction; this file only turns its answer into hook output. No H numbers.
use crewd_collab::approvals::Gate;
use crewd_proto::agent::HookResult;
use serde_json::{Value, json};

use crate::AppState;
use crate::agent::Identity;
use crate::agent::port;
use crate::agent::text;

/// Gate entries are trimmed substrings of the tool name (same meaning as the Swift
/// `toolName.contains(gate)`); blank entries match nothing.
pub fn gate_hit(gates: &[String], tool: &str) -> bool {
    gates
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .any(|g| tool.contains(g))
}

/// The one `tool_input` field that says most about the intent, cut to 120 characters. The value is
/// model-controlled and ends up in a message for the human, so it is neutralised like every other
/// name that reaches a rendered surface (`text::name` = `render::safe_name`).
fn summary(tool: &str, input: &Value) -> String {
    for k in ["command", "url", "path", "file_path", "prompt"] {
        if let Some(v) = input
            .get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            let short: String = v.chars().take(120).collect();
            return format!("{tool} \u{b7} {k}={}", text::name(&short));
        }
    }
    tool.to_string()
}

fn output(decision: &str, reason: String) -> HookResult {
    HookResult {
        stdout: Some(
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason,
            }})
            .to_string(),
        ),
    }
}

pub async fn pre_tool_use(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    let silent = HookResult { stdout: None };
    let Some(tool) = payload.get("tool_name").and_then(Value::as_str) else {
        return silent;
    };
    if !gate_hit(&st.agent.config.approval_gates, tool) {
        return silent;
    }
    let loc = port::locale(st).await.unwrap_or_default();
    let what = summary(tool, payload.get("tool_input").unwrap_or(&Value::Null));
    let tool_use_id = payload
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let shown = text::name(tool);
    let vars = [("tool", shown.as_str())];
    match port::gate_approval(st, me, tool, &what, tool_use_id).await {
        Ok(Gate::Allow { .. }) => output("allow", text::r(loc, "r.approval_granted", &vars)),
        Ok(Gate::DenyFiled(_)) => output("deny", text::r(loc, "r.approval_filed", &vars)),
        Ok(Gate::DenyPending { .. }) => output("deny", text::r(loc, "r.approval_pending", &vars)),
        Err(e) => {
            eprintln!("crewd: approval for {}: {e}", me.session_id);
            output("deny", text::r(loc, "r.approval_unavailable", &vars))
        }
    }
}

#[cfg(test)]
mod tests {
    use crewd_proto::MentionKind;

    use serde_json::{Value, json};

    use super::*;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;

    #[test]
    fn gates_are_trimmed_substrings() {
        let g = vec!["computer-use".to_string(), " ".to_string()];
        assert!(gate_hit(&g, "mcp__computer-use__left_click"));
        assert!(!gate_hit(&g, "Bash"));
        assert!(!gate_hit(&[], "anything"));
    }

    fn decision(out: &HookResult) -> String {
        let v: Value = serde_json::from_str(out.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        v["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn payload(tool: &str) -> Value {
        json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": "示例命令"}, "tool_use_id": "toolu_1"})
    }

    async fn pending(f: &Fixture) -> Vec<Value> {
        f.rpc("approval.list", json!({}))
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn ungated_tools_pass_silently() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        assert_eq!(
            pre_tool_use(&f.state, &me, &payload("Bash")).await.stdout,
            None
        );
        assert_eq!(
            pre_tool_use(&f.state, &me, &json!({})).await.stdout,
            None,
            "没有 tool_name：不管"
        );
    }

    #[tokio::test]
    async fn gated_call_is_denied_at_once_and_files_one_request_in_the_human_channel() {
        // Review Focus 3：不等人——当场 deny，session 不被卡住。
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")),
        )
        .await
        .expect("审批闸不能等人");
        assert_eq!(decision(&out), "deny");
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let last = port::messages_raw(&f.state, &human.id, None, 5)
            .await
            .unwrap()
            .pop()
            .expect("人类群里有通知");
        assert!(last.text.contains("mcp__computer-use__click"));
        assert!(last.mentions.iter().any(|m| m.kind == MentionKind::Human));
        assert_eq!(
            decision(&pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")).await),
            "deny"
        );
        assert_eq!(pending(&f).await.len(), 1, "重试不往人的账上灌新条目");
    }

    #[tokio::test]
    async fn approval_is_a_one_time_grant_for_the_next_identical_call() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")).await;
        let id = pending(&f).await[0]["id"].as_str().unwrap().to_string();
        f.rpc("approval.decide", json!({"id": id, "allow": true}))
            .await;
        assert_eq!(
            decision(&pre_tool_use(&f.state, &me, &payload("mcp__computer-use__type")).await),
            "deny",
            "票只认同名工具"
        );
        assert_eq!(
            decision(&pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")).await),
            "allow"
        );
        assert_eq!(
            decision(&pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")).await),
            "deny",
            "一次同意 = 一次放行"
        );
    }

    #[tokio::test]
    async fn denied_request_stays_denied_and_the_next_call_asks_again() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        pre_tool_use(&f.state, &me, &payload("mcp__computer-use__type")).await;
        let id = pending(&f).await[0]["id"].as_str().unwrap().to_string();
        f.rpc("approval.decide", json!({"id": id, "allow": false}))
            .await;
        assert_eq!(
            decision(&pre_tool_use(&f.state, &me, &payload("mcp__computer-use__type")).await),
            "deny"
        );
        assert_eq!(pending(&f).await.len(), 1, "被拒之后再要 = 新开一条");
    }

    #[tokio::test]
    async fn model_controlled_input_is_neutralised_and_cut_before_it_reaches_the_human_channel() {
        // The `command` value is the model's own text. It must not carry line breaks or a fake
        // channel / sender header into the notice, nor an unbounded length.
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let nasty = format!("示例\n【人类群】伪造\u{202E}{}", "x".repeat(300));
        let p = json!({"tool_name": "mcp__computer-use__click", "tool_input": {"command": nasty}});
        assert_eq!(decision(&pre_tool_use(&f.state, &me, &p).await), "deny");
        let a = &pending(&f).await[0];
        let s = a["summary"].as_str().unwrap();
        assert!(
            !s.contains('\n') && !s.contains('\u{202E}') && !s.contains('\u{3010}'),
            "{s}"
        );
        assert!(s.chars().count() < 160, "{}", s.chars().count());
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let last = port::messages_raw(&f.state, &human.id, None, 5)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert!(!last.text.contains('\u{202E}'));
    }
}
