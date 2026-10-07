//! stop hook (spec 22.3): the decision and the write live in `crewd_collab::turn_trace`; this file only
//! parses claude's payload. Never blocks, never outputs.
use crewd_collab::turn_trace;
use crewd_proto::Mention;
use crewd_proto::agent::HookResult;
use serde_json::Value;

use crate::AppState;
use crate::agent::Identity;

/// Called by the message-sending tools after a post (Tasks 12 / 14 wire the callers). A failure is
/// only logged: the message is already out, and a lost memo only costs one possible relay.
pub async fn note_post(st: &AppState, me: &Identity, mentions: &[Mention]) {
    let (session, mentions) = (me.session_id.clone(), mentions.to_vec());
    if let Err(e) = st
        .collab(move |tx| turn_trace::note_post(tx, &session, &mentions))
        .await
    {
        eprintln!("crewd: turn memo for {}: {e}", me.session_id);
    }
}

pub async fn stop(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    let event = payload.get("hook_event_name").and_then(Value::as_str);
    if event.is_some_and(|e| e != "Stop") || payload.get("agent_id").is_some() {
        return HookResult { stdout: None };
    }
    let prompt = payload
        .get("prompt_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let last = payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let session = me.session_id.clone();
    if let Err(e) = st
        .collab(move |tx| turn_trace::on_turn_end(tx, &session, &prompt, &last))
        .await
    {
        eprintln!("crewd: stop hook for {}: {e}", me.session_id);
    }
    HookResult { stdout: None }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;

    fn stop_payload(prompt: &str, text: &str) -> serde_json::Value {
        json!({"hook_event_name": "Stop", "prompt_id": prompt, "stop_hook_active": false, "last_assistant_message": text})
    }

    #[tokio::test]
    async fn stop_relays_through_collab_and_never_outputs() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        assert_eq!(
            stop(&f.state, &me, &stop_payload("p0", "收到。"))
                .await
                .stdout,
            None
        );
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        assert_eq!(
            stop(&f.state, &me, &stop_payload("p1", "要不要接着做 B？"))
                .await
                .stdout,
            None
        );
        assert_ne!(
            port::latest_seq(&f.state, &lobby).await.unwrap(),
            before,
            "代发了"
        );
    }

    #[tokio::test]
    async fn subagent_and_non_stop_payloads_are_ignored() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        stop(&f.state, &me, &stop_payload("p0", "收到。")).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        let mut sub = stop_payload("p1", "要不要做 B？");
        sub["agent_id"] = json!("sub-1");
        stop(&f.state, &me, &sub).await;
        let mut other = stop_payload("p2", "要不要做 B？");
        other["hook_event_name"] = json!("SubagentStop");
        stop(&f.state, &me, &other).await;
        assert_eq!(port::latest_seq(&f.state, &lobby).await.unwrap(), before);
    }

    #[tokio::test]
    async fn a_post_that_mentioned_the_captain_suppresses_the_relay_for_that_turn_only() {
        use crewd_proto::{Mention, MentionKind};
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        stop(&f.state, &me, &stop_payload("p0", "收到。")).await;
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        let call = [Mention {
            kind: MentionKind::Captain,
            target_id: None,
        }];
        note_post(&f.state, &me, &call).await;
        stop(&f.state, &me, &stop_payload("p1", "要不要做 B？")).await;
        assert_eq!(
            port::latest_seq(&f.state, &lobby).await.unwrap(),
            before,
            "already called the captain this turn"
        );
        stop(&f.state, &me, &stop_payload("p2", "要不要做 C？")).await;
        assert_ne!(
            port::latest_seq(&f.state, &lobby).await.unwrap(),
            before,
            "the memo is cleared by the turn end"
        );
    }
}
