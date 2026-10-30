//! C13: literal binary serve callers plus separately labelled library callback proof.
#[path = "support/agent.rs"]
#[allow(dead_code)] // Shared fixture APIs used by other integration targets.
mod agent;
#[path = "support/c13.rs"]
mod c13;
#[path = "../../server/tests/support/t11.rs"]
mod t11;
use c13::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
#[tokio::test]
async fn c13_literal_session_mentions_and_reply_route_once() {
    let mut r = Rig::literal().await;
    let (cid, cap, worker) = r.pair().await;
    let lobby = format!("{cid}/lobby");
    let cg = r.generated(&cap);
    let wg = r.generated(&worker);
    let mut c = cg.mcp(&r.f).await;
    let mut w = wg.mcp(&r.f).await;
    let observed = async {
        post(&mut c, &lobby, Some(&worker), "C13_CAP_TO_WORKER").await;
        let original = r.message(&lobby, "C13_CAP_TO_WORKER").await;
        if !r.body_observed(&worker, "C13_CAP_TO_WORKER").await {
            return Err(anyhow::anyhow!("C13 original worker body missing within original 15s observation"));
        }
        let delivered = r.delivered(original["id"].as_str().unwrap(), &worker).await;
        let (_, err) = w
            .call(
                "post",
                json!({"reply_to":original["seq"],"message":"C13_WORKER_REPLY","category":"progress"}),
            )
            .await;
        let reply = r.message(&lobby, "C13_WORKER_REPLY").await;
        if !r.body_observed(&cap, "C13_WORKER_REPLY").await {
            return Err(anyhow::anyhow!("C13 reply captain body missing within original 15s observation"));
        }
        let replied = r.delivered(reply["id"].as_str().unwrap(), &cap).await;
        Ok::<_,anyhow::Error>((original, reply, delivered, replied, err))
    }.await;
    let mc = normal_mcp(c.close_observed().await);
    let mw = normal_mcp(w.close_observed().await);
    let clean = r.close().await;
    assert!(
        clean && mc && mw,
        "normal cleanup precedes final semantic assertions"
    );
    eprintln!(
        "C13 routing observation after normal cleanup: {:?}",
        observed.as_ref().err()
    );
    assert_eq!(
        (
            r.count(&worker, "C13_CAP_TO_WORKER"),
            r.count(&cap, "C13_CAP_TO_WORKER")
        ),
        (1, 0)
    );
    assert_eq!(
        (
            r.count(&cap, "C13_WORKER_REPLY"),
            r.count(&worker, "C13_WORKER_REPLY")
        ),
        (1, 0)
    );
    let (original, reply, delivered, replied, err) =
        observed.expect("C13 routing observations after normal cleanup");
    assert!(!err);
    assert_eq!(original["senderSessionId"], cap);
    assert_eq!(reply["senderSessionId"], worker);
    assert_eq!(reply["inReplyTo"], original["id"]);
    assert!(
        delivered
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["sessionId"] == worker && v["state"] == "delivered")
    );
    assert!(
        replied
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["sessionId"] == cap && v["state"] == "delivered")
    );
}
async fn busy(event: &str) {
    let mut r = Rig::literal().await;
    let (cid, cap, worker) = r.pair().await;
    let lobby = format!("{cid}/lobby");
    let cg = r.generated(&cap);
    let wg = r.generated(&worker);
    let other = r.topic(&cid, &cap, &worker).await;
    let mut c = cg.mcp(&r.f).await;
    wg.arm(&mut r, &worker).await;
    post(&mut c, &lobby, Some(&worker), "C13_BUSY_TARGET").await;
    let target = r.message(&lobby, "C13_BUSY_TARGET").await;
    let queued = r.receipts(target["id"].as_str().unwrap()).await;
    let before = r.count(&worker, "C13_BUSY_TARGET");
    // Same-call positive is deliberately unreserved: @human cannot enqueue an agent wake.
    post(&mut c, &other, None, "C13_OTHER_POSITIVE").await;
    let positive = r.message(&other, "C13_OTHER_POSITIVE").await;
    let first = wg.hook(&r.f, event).await;
    let firsttext = context(&first).to_owned();
    wg.release(&mut r, &worker).await;
    let next = wg.hook(&r.f, event).await;
    let nexttext = context(&next).to_owned();
    // Second full hook + actual idle settlement observes both routes after prefix commit.
    r.working(&worker, false).await;
    let idle_receipts = r.delivered(target["id"].as_str().unwrap(), &worker).await;
    let mc = normal_mcp(c.close_observed().await);
    let clean = r.close().await;
    assert!(clean && mc);
    assert_eq!(before, 0);
    assert!(
        queued
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["sessionId"] == worker && v["state"] == "queued")
    );
    assert_eq!(first["hookSpecificOutput"]["hookEventName"], event);
    assert!(
        idle_receipts
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["sessionId"] == worker && v["state"] == "delivered")
    );
    assert_eq!(firsttext.matches("C13_BUSY_TARGET").count(), 1);
    assert_eq!(firsttext.matches("C13_OTHER_POSITIVE").count(), 1);
    assert!(!nexttext.contains("C13_BUSY_TARGET") && !nexttext.contains("C13_OTHER_POSITIVE"));
    assert_eq!(
        (
            r.count(&worker, "C13_BUSY_TARGET"),
            r.count(&worker, "C13_OTHER_POSITIVE")
        ),
        (0, 0)
    );
    assert_eq!(r.cursor(&worker, &lobby), target["seq"].as_i64());
    assert_eq!(r.cursor(&worker, &other), positive["seq"].as_i64());
    let s = r.store();
    let wakes = crewd_collab::wakes::for_source(
        s.conn(),
        crewd_collab::wakes::WakeKind::Message,
        &crewd_collab::wakes::source::message(target["id"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].session, worker);
    assert_eq!(format!("{:?}", wakes[0].state), "Delivered");
    let integrity: String = s
        .conn()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}
#[tokio::test]
async fn c13_literal_busy_posttool_context_once() {
    busy("PostToolUse").await;
}
#[tokio::test]
async fn c13_literal_busy_failure_context_once() {
    busy("PostToolUseFailure").await;
}
#[tokio::test]
async fn c13_literal_stopped_mention_resume_firsttool_once() {
    let mut r = Rig::literal().await;
    let (cid, cap, worker) = r.pair().await;
    let lobby = format!("{cid}/lobby");
    let other = r.topic(&cid, &cap, &worker).await;
    let old = r.generated(&worker);
    let oldm = old.mcp(&r.f).await;
    let oldinit = oldm.init.clone();
    let oldclose = normal_mcp(oldm.close_observed().await);
    r.client
        .call(
            "session.stop",
            json!({"sessionId":worker,"reason":"C13 synthetic stopped-target scenario"}),
        )
        .await
        .unwrap();
    r.client.stopped(&worker).await;
    t11::wait("old actual process reaped", || {
        matches!(
            crewd_exec::registry::probe(old.identity.pid),
            crewd_exec::registry::ProbeResult::Missing
        )
    })
    .await;
    let gone = matches!(
        crewd_exec::registry::probe(old.identity.pid),
        crewd_exec::registry::ProbeResult::Missing
    );
    let cg = r.generated(&cap);
    let mut c = cg.mcp(&r.f).await;
    post(&mut c, &lobby, Some(&worker), "C13_PRE_RESUME_NOT_AUTO").await;
    assert_eq!(
        r.client
            .call("session.live", json!({"sessionId":worker}))
            .await
            .unwrap()["running"],
        false
    );
    r.client
        .call("session.resume", json!({"sessionId":worker}))
        .await
        .unwrap();
    // A child can write its launch witness before the parent records its birth.
    // Keep one original 15s bound for Core readiness and physical identity;
    // neither a journal entry nor queue admission certifies a resumed runtime.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        r.working(&worker, false).await;
        t11::wait("explicit resumed registered exact birth", || {
            let identities: Vec<_> = t11::registry(&r.f.data)
                .into_iter()
                .filter(|(sid, _)| sid == &worker)
                .collect();
            if identities.len() != 1 {
                return false;
            }
            let identity = &identities[0].1;
            identity != &old.identity
                && matches!(crewd_exec::registry::probe(identity.pid),
                    crewd_exec::registry::ProbeResult::Found(ref actual) if actual == identity)
                && r.records()
                    .iter()
                    .filter(|v| {
                        v["event"] == "launch"
                            && v["session_id"] == worker
                            && v["pid"] == identity.pid
                    })
                    .count()
                    == 1
        })
        .await;
    })
    .await
    .expect("explicit resume must reach Core-ready and exact registered birth within original 15s");
    let fresh = r.generated(&worker);
    fresh.arm(&mut r, &worker).await;
    assert_eq!(r.count(&worker, "C13_PRE_RESUME_NOT_AUTO"), 0);
    post(&mut c, &lobby, Some(&worker), "C13_STOPPED_RESUME").await;
    let target = r.message(&lobby, "C13_STOPPED_RESUME").await;
    let mut freshm = fresh.mcp(&r.f).await;
    let freshtools = freshm.tools().await;
    // Positive created only AFTER initial body, proving that exact fresh first hook executes.
    post(&mut c, &other, None, "C13_RESUMED_OTHER").await;
    let positive = r.message(&other, "C13_RESUMED_OTHER").await;
    let first = fresh.hook(&r.f, "PostToolUse").await;
    let next = fresh.hook(&r.f, "PostToolUse").await;
    fresh.release(&mut r, &worker).await;
    let mc = normal_mcp(c.close_observed().await);
    let mf = normal_mcp(freshm.close_observed().await);
    let clean = r.close().await;
    assert!(
        clean && mc && mf && oldclose && oldinit.get("error").is_none() && !freshtools.is_empty()
    );
    assert!(gone);
    assert_ne!(old.identity, fresh.identity);
    assert_ne!(old.generation, fresh.generation);
    assert_eq!(old.agent_id, fresh.agent_id);
    assert!(fresh.resumed);
    assert_eq!(context(&first).matches("C13_STOPPED_RESUME").count(), 1);
    assert_eq!(context(&first).matches("C13_RESUMED_OTHER").count(), 1);
    assert!(!context(&next).contains("C13_RESUMED_OTHER"));
    assert_eq!(
        (
            r.count(&worker, "C13_STOPPED_RESUME"),
            r.count(&cap, "C13_STOPPED_RESUME")
        ),
        (0, 0)
    );
    assert_eq!(r.cursor(&worker, &lobby), target["seq"].as_i64());
    assert_eq!(r.cursor(&worker, &other), positive["seq"].as_i64());
}
#[derive(Clone, Debug)]
struct Asked {
    crew: String,
    ask: String,
    cleared: Vec<(String, Option<String>)>,
    now: i64,
}
struct RecordAsk(Arc<Mutex<Vec<Asked>>>);
impl crewd_server::orchestra::hooks::AskAnsweredHook for RecordAsk {
    fn on_answered<'a>(
        &'a self,
        crew: &'a str,
        ask: &'a str,
        cleared: &'a [(String, Option<String>)],
        now: crewd_wake::Ms,
    ) -> crewd_server::orchestra::exec::BoxFut<'a, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().push(Asked {
                crew: crew.into(),
                ask: ask.into(),
                cleared: cleared.to_vec(),
                now: now.0,
            });
        })
    }
}
async fn actual_ask(library: bool) {
    let calls = Arc::new(Mutex::new(vec![]));
    let hooks = crewd_server::orchestra::hooks::Hooks {
        ask: Arc::new(RecordAsk(calls.clone())),
        ..Default::default()
    };
    let mut r = if library {
        Rig::library(hooks).await
    } else {
        Rig::literal().await
    };
    let (cid, cap, worker) = r.pair().await;
    let g = r.generated(&worker);
    let mut m = g.mcp(&r.f).await;
    let (_, error) = m
        .call(
            "ask",
            json!({"question":"C13_ASK_QUESTION","resume_note":"C13_ASK_RESUME_NOTE"}),
        )
        .await;
    let detail = r
        .client
        .call("crew.get", json!({"crewId":cid}))
        .await
        .unwrap();
    let home = detail["homeId"].as_str().expect("actual crew home");
    let list = r
        .client
        .call("ask.list", json!({"homeCrewId":home}))
        .await
        .unwrap();
    let ask = list
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["ask"]["question"] == "C13_ASK_QUESTION")
        .expect("actual home ask row")["ask"]
        .clone();
    r.client
        .call(
            "ask.answer",
            json!({"crewId":ask["crewId"],"ask":ask["ask"],"text":"C13_HUMAN_ANSWER"}),
        )
        .await
        .unwrap();
    let note_seen = r.body_observed(&worker, "C13_ASK_RESUME_NOTE").await;
    // Missing callback is collected as an observation, never an early setup panic.
    // Keep the original 15s observation bound; duplicate callbacks also reach the
    // exact post-cleanup counter instead of waiting for len == 1 forever.
    let callback_seen = if library {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if !calls.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok()
    } else {
        false
    };
    let mc = normal_mcp(m.close_observed().await);
    let clean = r.close().await;
    assert!(clean && mc && !error);
    eprintln!("C13 callback observation before normal cleanup: seen={callback_seen}");
    eprintln!("C13 Ask note observation before normal cleanup: seen={note_seen}");
    if library {
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "C13 actual library AskAnswered future must be polled exactly once after normal cleanup"
        );
    }
    assert_eq!(ask["askedBy"], worker);
    assert_eq!(
        (
            r.count(&worker, "C13_ASK_RESUME_NOTE"),
            r.count(&cap, "C13_ASK_RESUME_NOTE")
        ),
        (1, 0)
    );
    let crew = ask["crewId"].as_str().unwrap();
    let key = ask["ask"].as_str().unwrap();
    let s = r.store();
    let wakes = crewd_collab::wakes::for_source(
        s.conn(),
        crewd_collab::wakes::WakeKind::AskAnswer,
        &crewd_collab::wakes::source::ask(crew, key),
    )
    .unwrap();
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].session, worker);
    assert_eq!(wakes[0].priority, crewd_collab::wakes::WakePriority::Human);
    assert!(
        wakes[0].note.is_none(),
        "Ask wake references persisted Ask metadata, not a duplicate note field"
    );
    let persisted = crewd_collab::asks::get(s.conn(), crew, key).unwrap();
    assert_eq!(
        persisted.resume_note.as_deref(),
        Some("C13_ASK_RESUME_NOTE")
    );
    assert_eq!(format!("{:?}", wakes[0].state), "Delivered");
    let calls = calls.lock().unwrap();
    if library {
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].crew, crew);
        assert_eq!(calls[0].ask, key);
        assert!(calls[0].cleared.is_empty());
        assert!(calls[0].now > 0);
    } else {
        assert!(
            calls.is_empty(),
            "literal default hook is not an instrumented callback proof"
        );
    }
}
#[tokio::test]
async fn c13_literal_ask_answer_resume_note_target() {
    actual_ask(false).await;
}
#[tokio::test]
async fn c13_library_serve_polled_ask_callback_and_resume_note() {
    actual_ask(true).await;
}
