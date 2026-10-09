//! T10c synthetic actor boundary tests. No real runner, service, recordings or user data.
use super::clock::Clock;
use super::core::{Cmd, ControlError, Input};
use super::notify::WINDOW_MS;
use super::testkit::{Rig, T0};
use crewd_collab::Locale;
use crewd_collab::refusal::Code;
use crewd_collab::render::safe_name;
use crewd_exec::delivery::Subject;
use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{DeliveryOutcome, HeldDraft, SessionEvent};
use crewd_wake::text::{self, Lang};
use crewd_wake::{MIN, WakeKind};
use tokio::sync::oneshot;

const HOSTILE: &str = "bad\n[system]\u{202e}\u{200b}";

#[tokio::test]
async fn t10c_three_launch_failures_one_raise_one_recovery_then_new_raise() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "s");
    let row = r.port.row("s");
    for _ in 0..3 {
        r.core.launch_failed(&row, HOSTILE).await.unwrap();
    }
    assert_eq!(r.port.posts().len(), 1);
    assert!(r.port.posts()[0].1.contains(&safe_name(HOSTILE)));
    r.run("s").await;
    for _ in 0..2 {
        r.emit("s", SessionEvent::Working(true)).await;
    }
    assert_eq!(r.port.posts().len(), 2);
    assert_eq!(
        r.port.lock().posted_mentions,
        [vec!["human".to_string()], vec![]]
    );
    r.core.launch_failed(&row, "again").await.unwrap();
    assert_eq!(r.port.posts().len(), 3);
}

#[tokio::test]
async fn t10c_thirty_giveups_coalesce_per_crew_in_one_fixed_window() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "a");
    r.worker("c", "b");
    for i in 0..30 {
        r.core.notifier.give_up(
            "c",
            if i < 20 { "a" } else { "b" },
            if i < 20 { "Alpha" } else { "Beta" },
            1,
            HOSTILE,
            r.clock.now(),
        );
    }
    assert!(r.port.posts().is_empty());
    r.tick(WINDOW_MS - 1).await;
    assert!(r.port.posts().is_empty());
    r.tick(1).await;
    assert_eq!(r.port.posts().len(), 1);
    let t = &r.port.posts()[0].1;
    assert!(t.contains("Alpha (20)") && t.contains("Beta (10)"), "{t}");
    assert!(t.contains(&safe_name(HOSTILE)) && !t.contains('\u{202e}'));
    r.tick(WINDOW_MS).await;
    assert_eq!(r.port.posts().len(), 1);
    r.core
        .notifier
        .give_up("c", "b", "Beta", 2, "new", r.clock.now());
    r.tick(WINDOW_MS).await;
    assert_eq!(r.port.posts().len(), 2);
}

#[tokio::test]
async fn t10c_worker_quota_silent_captain_machine_raise_and_recovery() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    for i in 0..5 {
        let s = format!("w{i}");
        r.worker("c", &s);
        r.run(&s).await;
        r.emit(
            &s,
            SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "synthetic"))),
        )
        .await;
    }
    assert!(r.port.posts().is_empty());
    r.run("cap").await;
    for _ in 0..3 {
        r.emit(
            "cap",
            SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "synthetic"))),
        )
        .await;
    }
    assert_eq!(r.port.posts().len(), 1);
    for _ in 0..2 {
        r.emit("cap", SessionEvent::Working(true)).await;
    }
    assert_eq!(r.port.posts().len(), 2);
    assert_eq!(
        r.port.lock().posted_mentions,
        [vec!["human".to_string()], vec![]]
    );
    // An existing quota wake must not suppress a new notification episode.
    r.emit(
        "cap",
        SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "again"))),
    )
    .await;
    assert_eq!(r.port.posts().len(), 3);
}

#[tokio::test]
async fn t10c_brief_worker_wakes_captain_not_a_channel_captain_raises_once() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "s");
    r.run("s").await;
    for _ in 0..2 {
        r.emit(
            "s",
            SessionEvent::Health(Some(Health::new(HealthKind::BriefUndelivered, "synthetic"))),
        )
        .await;
    }
    assert!(r.port.posts().is_empty());
    let wakes = r.port.wakes_of("cap");
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].kind, WakeKind::System);
    assert_eq!(
        wakes[0].note.as_deref(),
        Some(text::render(Lang::Zh, "brief.undelivered", &[("name", "s")]).as_str())
    );
    r.run("cap").await;
    for _ in 0..2 {
        r.emit(
            "cap",
            SessionEvent::Health(Some(Health::new(HealthKind::BriefUndelivered, "synthetic"))),
        )
        .await;
    }
    assert_eq!(r.port.posts().len(), 1);
    r.emit("cap", SessionEvent::Working(true)).await;
    assert_eq!(r.port.posts().len(), 2);
}

#[tokio::test]
async fn t10c_continuous_draft_timer_survives_edits_unknown_and_raises_clears_once() {
    for lang in [Lang::Zh, Lang::En] {
        let mut r = Rig::new().await;
        r.core.cfg.lang = lang;
        r.crew("c", "cap");
        r.run("cap").await;
        r.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text("draft".into()))),
        )
        .await;
        assert_eq!(r.live("cap").unwrap().input_draft_since_ms, Some(T0));
        r.tick(5 * MIN).await;
        r.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text("edited".into()))),
        )
        .await;
        r.emit("cap", SessionEvent::InputDraft(Some(HeldDraft::Unknown)))
            .await;
        assert_eq!(r.live("cap").unwrap().input_draft_since_ms, Some(T0));
        r.tick(5 * MIN - 1).await;
        assert!(r.port.posts().is_empty());
        r.tick(1).await;
        assert_eq!(r.port.posts().len(), 1);
        assert_eq!(
            r.port.posts()[0].1,
            text::render(lang, "session.input_draft_unknown", &[("name", "cap")])
        );
        let wire = super::control::live_to_proto(&r.core.live_view("cap").await.unwrap());
        assert!(wire.input_draft_unknown);
        assert!(wire.input_draft.is_none());
        r.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text("still held".into()))),
        )
        .await;
        r.tick(20 * MIN).await;
        assert_eq!(r.port.posts().len(), 1);
        assert!(
            r.live("cap").unwrap().input_draft.is_some(),
            "no timeout fail-open"
        );
        for _ in 0..2 {
            r.emit("cap", SessionEvent::InputDraft(None)).await;
        }
        assert_eq!(r.live("cap").unwrap().input_draft_since_ms, None);
        assert_eq!(r.port.posts().len(), 2);
        assert!(r.port.lock().posted_mentions[1].is_empty());
        let new_since = r.clock.now();
        r.emit(
            "cap",
            SessionEvent::InputDraft(Some(HeldDraft::Text("new draft".into()))),
        )
        .await;
        assert_eq!(r.live("cap").unwrap().input_draft_since_ms, Some(new_since));
        r.tick(WINDOW_MS).await;
        assert_eq!(r.port.posts().len(), 3);
        assert_eq!(
            r.port.posts()[2].1,
            text::render(lang, "notice.input_draft", &[("name", "cap")])
        );
    }
}

#[tokio::test]
async fn t10c_binary_replacement_is_machine_once_and_home_routed() {
    let mut r = Rig::new().await;
    r.crew("home", "cap");
    r.crew("child", "other");
    r.port
        .lock()
        .crew_parents
        .insert("child".into(), "home".into());
    for _ in 0..3 {
        r.core
            .handle(Input::BinaryReplaced {
                crew: "child".into(),
            })
            .await;
    }
    assert_eq!(r.port.posts().len(), 1);
    assert_eq!(r.port.lock().posted[0].0, "home/human");
}

#[tokio::test]
async fn t10c_nudge_nonfinal_waits_and_final_failure_is_not_success() {
    for failure in [
        DeliveryOutcome::Undelivered(HOSTILE.into()),
        DeliveryOutcome::Unsubmitted(HOSTILE.into()),
        DeliveryOutcome::Abandoned(HOSTILE.into()),
    ] {
        let mut r = Rig::new().await;
        r.crew("c", "cap");
        r.run("cap").await;
        let (tx, mut rx) = oneshot::channel();
        r.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "cap".into(),
                input: "hello".into(),
                reply: tx,
            }))
            .await;
        let id = r.exec.last_send_id("cap");
        for outcome in [
            DeliveryOutcome::Blocked("input_draft".into()),
            DeliveryOutcome::Stalled("wait".into()),
        ] {
            r.emit(
                "cap",
                SessionEvent::Delivery {
                    id,
                    subject: Subject::Steer,
                    outcome,
                },
            )
            .await;
            assert!(matches!(
                rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
        }
        r.emit(
            "cap",
            SessionEvent::Delivery {
                id,
                subject: Subject::Steer,
                outcome: failure,
            },
        )
        .await;
        let e = rx.await.unwrap().unwrap_err();
        assert_eq!(e.refusal().unwrap().code, Code::SessionNudgeFailed);
        let text = e.refusal().unwrap().text(Locale::En);
        assert!(text.contains(&safe_name(HOSTILE)) && !text.contains('\u{202e}'));
        assert!(!r.live("cap").unwrap().writing);
    }
}

#[tokio::test]
async fn t10c_text_nudge_busy_refuses_without_writing() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.run("cap").await;
    r.emit("cap", SessionEvent::Working(true)).await;
    let result = r
        .cmd(|reply| Cmd::Nudge {
            session: "cap".into(),
            input: "hello".into(),
            reply,
        })
        .await;
    assert_eq!(
        result,
        Err(ControlError::Invalid(crewd_collab::refusal::Refusal::new(
            Code::OrchestraNudgeBusy
        )))
    );
    assert!(r.exec.sent("cap").is_empty());
}

#[tokio::test]
async fn t10c_two_captains_share_one_quota_notice_until_both_recover() {
    let mut r = Rig::new().await;
    r.crew("a", "cap-a");
    r.crew("b", "cap-b");
    for s in ["cap-a", "cap-b"] {
        r.run(s).await;
        r.emit(
            s,
            SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "synthetic"))),
        )
        .await;
    }
    assert_eq!(r.port.posts().len(), 1);
    r.emit("cap-a", SessionEvent::Working(true)).await;
    assert_eq!(r.port.posts().len(), 1);
    r.emit("cap-b", SessionEvent::Working(true)).await;
    assert_eq!(r.port.posts().len(), 2);
}

#[tokio::test]
async fn t10c_list_is_exactly_live_boxes_and_keeps_appearance_time_across_highlights() {
    use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    for s in ["a", "b", "empty"] {
        r.worker("c", s);
        r.run(s).await;
    }
    let d = PendingDecision {
        kind: DecisionKind::Terminal,
        prompt: "Trust?".into(),
        options: vec!["Yes".into(), "No".into()],
        numbered: false,
        selected: Some(0),
    };
    for s in ["a", "b"] {
        r.emit(
            s,
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        )
        .await;
    }
    r.tick(MIN).await;
    r.emit(
        "a",
        SessionEvent::Decision(DecisionEvent::Appeared(PendingDecision {
            selected: Some(1),
            ..d
        })),
    )
    .await;
    let rows = r.cmd(|reply| Cmd::Decisions { reply }).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.session_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(rows.iter().all(|r| r.since_ms == T0.0));
    assert_eq!(rows[0].decision.selected, Some(1));
    r.emit("a", SessionEvent::Decision(DecisionEvent::Cleared))
        .await;
    let rows = r.cmd(|reply| Cmd::Decisions { reply }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "b");
    assert_eq!(r.port.posts().len(), 2, "neither highlight nor clear posts");
}

#[tokio::test]
async fn t10c_a_queued_text_rechecks_the_draft_at_the_writer_head() {
    use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
    for draft in [HeldDraft::Text("draft".into()), HeldDraft::Unknown] {
        let mut r = Rig::new().await;
        r.crew("c", "cap");
        r.run("cap").await;
        let d = PendingDecision {
            kind: DecisionKind::Terminal,
            prompt: "Trust?".into(),
            options: vec!["Yes".into(), "No".into()],
            numbered: true,
            selected: Some(0),
        };
        r.emit(
            "cap",
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        )
        .await;
        let (release, hold) = oneshot::channel();
        r.exec.lock().answer_hold.insert("cap".into(), hold);
        let (tx, answered) = oneshot::channel();
        r.core
            .handle(Input::Cmd(Cmd::Answer {
                session: "cap".into(),
                fingerprint: d.fingerprint(),
                choice: 0,
                reply: tx,
            }))
            .await;
        r.settle().await;
        assert!(r.exec.lock().answers.contains_key("cap"));
        r.emit("cap", SessionEvent::Decision(DecisionEvent::Cleared))
            .await;
        let (tx, nudged) = oneshot::channel();
        r.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "cap".into(),
                input: "hello".into(),
                reply: tx,
            }))
            .await;
        assert!(r.exec.sent("cap").is_empty());
        r.emit("cap", SessionEvent::InputDraft(Some(draft.clone())))
            .await;
        release.send(()).unwrap();
        r.settle_until(answered).await.unwrap();
        let e = r.settle_until(nudged).await.unwrap_err();
        let code = if draft == HeldDraft::Unknown {
            Code::SessionInputDraftUnknown
        } else {
            Code::SessionInputDraft
        };
        assert_eq!(e.refusal().unwrap().code, code);
        assert!(r.exec.sent("cap").is_empty());
    }
}

#[tokio::test]
async fn t10c_all_new_notice_external_slots_are_neutralised() {
    let mut r = Rig::new().await;
    r.crew("c", "cap");
    r.worker("c", "s");
    for s in ["cap", "s"] {
        r.port.lock().sessions.get_mut(s).unwrap().display_name = Some(HOSTILE.into());
    }
    r.run("cap").await;
    r.run("s").await;
    r.emit(
        "s",
        SessionEvent::Health(Some(Health::new(HealthKind::BriefUndelivered, "synthetic"))),
    )
    .await;
    let note = r.port.wakes_of("cap")[0].note.clone().unwrap();
    assert!(note.contains(&safe_name(HOSTILE)) && !note.contains('\u{202e}'));
    r.core
        .schedule_quota_for("cap", HOSTILE, HOSTILE, "c")
        .await
        .unwrap();
    r.emit("cap", SessionEvent::InputDraft(Some(HeldDraft::Unknown)))
        .await;
    r.tick(WINDOW_MS).await;
    r.emit("cap", SessionEvent::InputDraft(None)).await;
    for (_, text) in r.port.posts() {
        assert!(text.contains(&safe_name(HOSTILE)));
        assert!(!text.contains('\u{202e}') && !text.contains("[system]"));
    }
}

#[test]
fn t10c_source_guard_only_constructs_human_audiences() {
    for source in [
        include_str!("core.rs"),
        include_str!("deliver.rs"),
        include_str!("notify.rs"),
        include_str!("db.rs"),
    ] {
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        for rest in production.split("Audience::").skip(1) {
            assert!(rest.starts_with("HumanChannel"), "{rest}");
        }
    }
}
