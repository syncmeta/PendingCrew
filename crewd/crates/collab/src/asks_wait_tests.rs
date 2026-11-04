use super::*;
use crate::testkit::{MIN, NOW, count, ok_at, store};
use crewd_proto::{Mention, MentionKind, SenderKind};
use crewd_store::Store;

const HOUR: i64 = 60 * MIN;

#[test]
fn human_reminders_do_not_consume_agent_unread_window_or_hide_chat_history() {
    let mut s = store();
    let (last_notice, next) = ok_at(&mut s, NOW, |tx| {
        let mut last_notice = 0;
        for _ in 0..=crate::cursors::UNREAD_CAP {
            last_notice = messages::announce_to_human(
                tx,
                "crew-a",
                Sender::System,
                "isolated human-only notice".into(),
                messages::ASK_WAIT_REMINDER,
            )?
            .message
            .seq;
        }
        let next = messages::post(
            tx,
            Sender::System,
            messages::NewPost {
                channel_id: "crew-a/human".into(),
                text: "ordinary system message after reminders".into(),
                ..Default::default()
            },
        )?
        .message;
        Ok((last_notice, next))
    })
    .0;
    let unread = ok_at(&mut s, NOW, |tx| crate::cursors::unread_for(tx, "cap-a")).0;
    let (_, rows) = unread
        .into_iter()
        .find(|(ch, _)| ch == "crew-a/human")
        .unwrap();
    assert_eq!(rows, vec![next]);
    assert_eq!(
        crate::cursors::cursor(s.conn(), "cap-a", "crew-a/human").unwrap(),
        Some(last_notice)
    );
    assert_eq!(
        messages::list(s.conn(), "crew-a/human", None, None, 500)
            .unwrap()
            .len(),
        (crate::cursors::UNREAD_CAP + 2) as usize
    );
}

fn blocked(
    s: &mut Store,
    at: i64,
    crew: &str,
    owner: &str,
    default: Option<DefaultOption>,
) -> (AskId, String) {
    ok_at(s, at, |tx| {
        let id = with_book(tx, crew, |book, now| {
            let item = book
                .create_item(
                    &Actor::Human,
                    now,
                    crewd_work::NewItem::titled("fixture work").owned_by(owner),
                )?
                .value;
            Ok(book
                .block(
                    &Actor::Human,
                    now,
                    item,
                    vec![crewd_work::BlockOn::NewAsk {
                        question: "fixture private original question".into(),
                        default,
                    }],
                    crewd_work::Continuation::Nothing,
                )?
                .value
                .asks[0])
        })?;
        let ids = announce_block_questions(tx, &[id])?;
        Ok((id, ids[0].clone()))
    })
    .0
}

fn page(s: &mut Store, at: i64) -> Vec<DueWait> {
    ok_at(s, at, due_escalation_page).0
}

fn tier(s: &Store, id: AskId) -> WaitTier {
    load_org(s.conn()).unwrap().ask(id).unwrap().escalated
}

fn reminders(s: &Store) -> Vec<crewd_proto::Message> {
    messages::list(s.conn(), "crew-a/human", None, None, 100)
        .unwrap()
        .into_iter()
        .filter(|message| message.category.as_deref() == Some(messages::ASK_WAIT_REMINDER))
        .collect()
}

#[test]
fn exact_thresholds_pin_original_then_message_once_across_file_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crewd.db");
    let mut source = store();
    let (standalone, _) = ok_at(&mut source, NOW, |tx| {
        open(
            tx,
            "w-b1",
            OpenAsk {
                question: "older standalone".into(),
                ..Default::default()
            },
        )
    });
    let started = NOW + MIN;
    let (id, public) = blocked(&mut source, started, "crew-b", "w-b1", None);
    source
        .conn()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(source);
    let mut s = Store::open(&path).unwrap();
    let original = get(s.conn(), "crew-b", &public).unwrap();
    let pin = started + 24 * HOUR;
    assert!(page(&mut s, pin - 1).is_empty());
    assert_eq!(
        waits(s.conn(), pin - 1, Some("crew-a")).unwrap()[0].ask.ask,
        standalone.ask
    );
    let due = page(&mut s, pin).pop().unwrap();
    assert_eq!(due.ask, id);
    let (_, changes) = ok_at(&mut s, pin, |tx| escalate_wait(tx, due));
    assert_eq!(
        changes,
        vec![Change::AskChanged {
            crew_id: "crew-b".into(),
            ask: public.clone()
        }]
    );
    assert_eq!(tier(&s, id), WaitTier::Pinned);
    let inbox = waits(s.conn(), pin, Some("crew-a")).unwrap();
    assert_eq!(inbox[0].ask.ask, public);
    assert_eq!(inbox[0].tier, WaitTierView::Pinned);
    assert_eq!(inbox[0].waited_ms, 24 * HOUR);
    assert!(reminders(&s).is_empty());

    let overdue = started + 72 * HOUR;
    assert!(page(&mut s, overdue - 1).is_empty());
    let due = page(&mut s, overdue).pop().unwrap();
    ok_at(&mut s, overdue, |tx| escalate_wait(tx, due));
    assert_eq!(tier(&s, id), WaitTier::StillWanted);
    assert_eq!(count(&s, "SELECT COUNT(*) FROM wakes"), 0);
    let messages = reminders(&s);
    assert_eq!(messages.len(), 1);
    let reminder = &messages[0];
    assert_eq!(reminder.sender_kind, SenderKind::System);
    assert_eq!(
        reminder.mentions,
        vec![Mention {
            kind: MentionKind::Human,
            target_id: None
        }]
    );
    assert!(reminder.text.contains(&public) && !reminder.text.contains(&original.question));
    assert_eq!(get(s.conn(), "crew-b", &public).unwrap(), original);
    assert_eq!(count(&s, "SELECT COUNT(*) FROM ask_meta"), 2);
    let response = ok_at(&mut s, overdue, |tx| {
        messages::post(
            tx,
            Sender::Human,
            messages::NewPost {
                channel_id: reminder.channel_id.clone(),
                text: "reply to reminder is ordinary conversation".into(),
                in_reply_to: Some(reminder.id.clone()),
                ..Default::default()
            },
        )
    })
    .0;
    assert!(response.answered_ask.is_none());
    assert_eq!(
        get(s.conn(), "crew-b", &public).unwrap().state,
        AskStateView::Open
    );
    drop(s);
    let mut reopened = Store::open(&path).unwrap();
    for _ in 0..3 {
        assert!(page(&mut reopened, overdue + HOUR).is_empty());
    }
    assert_eq!(reminders(&reopened), messages);
    assert_eq!(tier(&reopened, id), WaitTier::StillWanted);
}

#[test]
fn answer_default_or_restore_wins_after_due_selection() {
    let mut s = store();
    let (answered, public) = blocked(&mut s, NOW, "crew-a", "w-a1", None);
    let (defaulted, _) = blocked(
        &mut s,
        NOW,
        "crew-a",
        "w-a1",
        Some(DefaultOption {
            text: "safe synthetic default".into(),
            after_ms: 72 * HOUR,
        }),
    );
    let (restored, _) = blocked(&mut s, NOW, "crew-a", "w-a1", None);
    let at = NOW + 72 * HOUR;
    let mut due = page(&mut s, at);
    assert_eq!(
        due.iter().map(|r| r.ask).collect::<Vec<_>>(),
        vec![answered, restored]
    );
    ok_at(&mut s, at, |tx| {
        answer(tx, "crew-a", &public, "explicit answer")
    });
    ok_at(&mut s, at, |tx| escalate_wait(tx, due.remove(0)));
    ok_at(&mut s, at, |tx| apply_all_due_defaults(tx));
    ok_at(&mut s, at, |tx| {
        Ok(crewd_store::judgment_state::restored(tx.conn())?)
    });
    ok_at(&mut s, at, |tx| escalate_wait(tx, due.remove(0)));
    for ask in [answered, defaulted, restored] {
        assert_eq!(tier(&s, ask), WaitTier::Fresh);
    }
    assert!(reminders(&s).is_empty());
}

#[test]
fn failed_message_rolls_back_tiers_and_does_not_starve_bounded_following_page() {
    let mut s = store();
    let mut ids = Vec::new();
    for _ in 0..(ESCALATIONS_PER_TICK + 1) {
        ids.push(blocked(&mut s, NOW, "crew-a", "w-a1", None).0);
    }
    let at = NOW + 73 * HOUR;
    let mut first = page(&mut s, at);
    assert_eq!(first.len(), ESCALATIONS_PER_TICK);
    s.conn().execute_batch("CREATE TRIGGER fail_human_wait_reminder BEFORE INSERT ON messages WHEN new.category='ask_wait_reminder' BEGIN SELECT RAISE(ABORT,'fixture refused reminder'); END;").unwrap();
    assert!(crate::run(&mut s, at, |tx| escalate_wait(tx, first.remove(0))).is_err());
    assert_eq!(tier(&s, ids[0]), WaitTier::Fresh);
    assert!(reminders(&s).is_empty());
    s.conn()
        .execute_batch("DROP TRIGGER fail_human_wait_reminder")
        .unwrap();
    let mut following = page(&mut s, at);
    assert_eq!(following.len(), 1);
    assert_eq!(Some(&following[0].ask), ids.last());
    ok_at(&mut s, at, |tx| escalate_wait(tx, following.remove(0)));
    assert!(
        page(&mut s, at).is_empty(),
        "cursor wrap is a distinct bounded tick"
    );
    let recovered = page(&mut s, at).remove(0);
    assert_eq!(recovered.ask, ids[0]);
    ok_at(&mut s, at, |tx| escalate_wait(tx, recovered));
    assert_eq!(tier(&s, ids[0]), WaitTier::StillWanted);
    assert_eq!(reminders(&s).len(), 2);
    assert_eq!(
        count(
            &s,
            "SELECT COUNT(*) FROM organization_work_events WHERE json_extract(body,'$.kind.type')='escalation_noted'"
        ),
        4
    );
}
