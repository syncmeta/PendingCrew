//! Independent public Core-seam evidence. Physical PTY/syscall proof remains
//! in the driver's held-reader tests; these never pretend synthetic completion is IO.
use crewd_exec::{
    delivery::Subject,
    session::{Action, Core, CoreConfig, DeliveryOutcome, PendingText, SessionEvent},
};
use crewd_term::ScreenConfig;
use std::time::{Duration, Instant};
fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}
fn frame(text: &str) -> Vec<u8> {
    format!("\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {text}\x1b[7m \x1b[0m\r\n────────────────────\r\n ? for shortcuts").into_bytes()
}
fn keys(actions: &[Action]) -> Vec<Vec<u8>> {
    actions
        .iter()
        .filter_map(|a| {
            if let Action::DeliveryWrite { bytes, .. } = a {
                Some(bytes.clone())
            } else {
                None
            }
        })
        .collect()
}
fn complete(core: &mut Core, actions: &[Action], now: Instant) {
    for a in actions {
        if let Action::DeliveryWrite { id, token, .. } = a {
            core.delivery_write_completed(*id, *token, now);
        }
    }
}
fn delivered(actions: &[Action]) -> bool {
    actions.iter().any(|a| {
        matches!(
            a,
            Action::Event(SessionEvent::Delivery {
                outcome: DeliveryOutcome::Delivered,
                ..
            })
        )
    })
}
fn landed() -> (Core, Instant, Vec<Action>) {
    landed_with("BOT")
}

fn landed_with(text: &str) -> (Core, Instant, Vec<Action>) {
    let base = Instant::now();
    let mut core = Core::new(
        CoreConfig::default(),
        ScreenConfig::default(),
        base,
        vec![PendingText {
            id: 51,
            subject: Subject::Brief,
            text: text.into(),
        }],
        None,
    );
    core.on_output(&frame(""), base);
    core.take_actions();
    core.on_tick(at(base, 310));
    let body = core.take_actions();
    assert_eq!(keys(&body), vec![text.as_bytes().to_vec()]);
    complete(&mut core, &body, at(base, 310));
    core.on_output(&frame(text), at(base, 900));
    let space = core.take_actions();
    assert_eq!(keys(&space), vec![b" ".to_vec()]);
    complete(&mut core, &space, at(base, 900));
    (core, base, space)
}
#[test]
fn unrelated_native_output_cannot_acknowledge_space_or_backspace() {
    for unrelated in [b"\x1b[?25h".to_vec(), frame("BOT")] {
        let (mut core, base, _) = landed();
        core.on_output(&unrelated, at(base, 1100));
        assert!(
            keys(&core.take_actions()).is_empty(),
            "unrelated native output must not release Backspace"
        );
    }
    let (mut core, base, _) = landed();
    core.on_output(&frame("BOT "), at(base, 1100));
    let bs = core.take_actions();
    assert_eq!(keys(&bs), vec![vec![0x7f]]);
    complete(&mut core, &bs, at(base, 1100));
    core.on_output(b"\x1b[?25h", at(base, 1300));
    assert!(
        keys(&core.take_actions()).is_empty(),
        "unrelated output must not release final Enter"
    );
}
#[test]
fn no_echo_never_retries_submit_beats_and_revoked_completion_stays_dead() {
    let (mut core, base, space) = landed();
    let mut all = Vec::new();
    for ms in [1050, 2000, 3500, 6000, 10000] {
        core.on_tick(at(base, ms));
        all.extend(core.take_actions());
    }
    assert!(
        keys(&all).is_empty(),
        "no echo must not enqueue another Space/BS/Enter or body retry"
    );
    assert!(!delivered(&all));
    assert!(
        all.iter().any(|a| matches!(
            a,
            Action::Event(SessionEvent::Delivery {
                outcome: DeliveryOutcome::Unsubmitted(_),
                ..
            })
        )),
        "existing verify budget must produce an honest terminal unsubmitted outcome"
    );
    complete(&mut core, &space, at(base, 11000));
    core.on_output(&frame("BOT "), at(base, 11100));
    core.on_tick(at(base, 11200));
    let late = core.take_actions();
    assert!(
        keys(&late).is_empty() && !delivered(&late),
        "revoked old token must never resurrect delivery"
    );
}
#[test]
fn empty_frame_requires_completed_enter_and_a_fresh_post_completion_receipt() {
    let (mut core, base, _) = landed();
    core.on_output(&frame("BOT "), at(base, 1100));
    let bs = core.take_actions();
    assert_eq!(keys(&bs), vec![vec![0x7f]]);
    complete(&mut core, &bs, at(base, 1100));
    core.on_output(&frame("BOT"), at(base, 1300));
    let enter = core.take_actions();
    assert_eq!(keys(&enter), vec![b"\r".to_vec()]);
    core.on_output(&frame(""), at(base, 1400));
    assert!(
        !delivered(&core.take_actions()),
        "queued but uncompleted Enter cannot claim empty-frame success"
    );
    complete(&mut core, &enter, at(base, 1500));
    core.on_tick(at(base, 1600));
    assert!(
        !delivered(&core.take_actions()),
        "old empty frame predating final-byte completion cannot claim success"
    );
    core.on_output(&frame(""), at(base, 1700));
    let mut result = core.take_actions();
    core.on_tick(at(base, 2300));
    result.extend(core.take_actions());
    assert!(
        delivered(&result),
        "complete Enter plus fresh empty-frame receipt must remain a valid positive control"
    );
    assert!(
        keys(&result).is_empty(),
        "positive receipt must not retry submitted text"
    );
}

#[test]
fn native_cursor_wrapped_input_preserves_submission_liveness() {
    // Default production geometry is 80 columns. Prefix is the 2-cell ❯ plus
    // space; 77 cells places the inverse blank in the final column, and 97
    // cells keeps the prompt visible while the native cursor is on a folded row.
    for count in [77, 97] {
        let body = "x".repeat(count);
        let (mut core, base, _) = landed_with(&body);
        core.on_output(&frame(&format!("{body} ")), at(base, 1100));
        let backspace = core.take_actions();
        assert_eq!(
            keys(&backspace),
            vec![vec![0x7f]],
            "native Space consumption across a wrapped input must permit its paired Backspace at width {count}"
        );
        complete(&mut core, &backspace, at(base, 1100));
        core.on_output(&frame(&body), at(base, 1300));
        let enter = core.take_actions();
        assert_eq!(
            keys(&enter),
            vec![b"\r".to_vec()],
            "restored native folded input must permit final Enter at width {count}"
        );
        complete(&mut core, &enter, at(base, 1300));
        core.on_output(&frame(""), at(base, 1500));
        let mut result = core.take_actions();
        core.on_tick(at(base, 2100));
        result.extend(core.take_actions());
        assert!(delivered(&result));
    }
}

#[test]
fn opaque_same_count_suffix_and_cursor_change_cannot_acknowledge_space() {
    for body in ["x".repeat(9000), format!("first\n{}", "x".repeat(9000))] {
        let base = Instant::now();
        let mut core = Core::new(
            CoreConfig::default(),
            ScreenConfig::default(),
            base,
            vec![PendingText {
                id: 52,
                subject: Subject::Brief,
                text: body.clone(),
            }],
            None,
        );
        core.on_output(&frame(""), base);
        core.take_actions();
        core.on_tick(at(base, 310));
        let writes = core.take_actions();
        assert_eq!(keys(&writes), vec![body.as_bytes().to_vec()]);
        complete(&mut core, &writes, at(base, 310));
        // Real native metadata can have the final total line count while the last
        // line's hidden suffix has not yet been processed. This is no body receipt.
        let caption = format!("[Pasted text #1 +{} lines]", body.lines().count());
        core.on_output(&frame(&caption), at(base, 900));
        let probe = core.take_actions();
        // An implementation may refuse the opaque paint before the Space probe.
        // If it probes, neither same count nor a native cursor shift certifies it.
        assert!(keys(&probe).is_empty() || keys(&probe) == vec![b" ".to_vec()]);
        complete(&mut core, &probe, at(base, 900));
        core.on_output(&frame(&format!("{caption} ")), at(base, 1100));
        let after = core.take_actions();
        assert!(
            keys(&after).is_empty() && !delivered(&after),
            "same-count collapsed suffix redraw/cursor movement must not release BS"
        );
        core.on_tick(at(base, 3500));
        let terminal = core.take_actions();
        assert!(keys(&terminal).is_empty() && !delivered(&terminal));
        assert!(
            terminal.iter().any(|a| matches!(
                a,
                Action::Event(SessionEvent::Delivery {
                    outcome: DeliveryOutcome::Unsubmitted(_) | DeliveryOutcome::Undelivered(_),
                    ..
                })
            )),
            "unsupported opaque body must terminate unknown within the existing budget"
        );
    }
}
