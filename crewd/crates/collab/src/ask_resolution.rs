//! Human resolution is correlated by the public command receipt, not retried here.
use crate::{
    CollabError, Result, Sender, Tx, asks,
    refusal::{Code, Refusal},
    resource_acl,
};
use crewd_proto::{
    AskResolutionCheck, AskResolutionDisposition, AskResolutionParams, AskResolutionReceipt,
    AskStateView,
};
use crewd_store::StoreError;
use crewd_work::{Actor, Event, EventKind};
use rusqlite::{Connection, params};

fn invalid() -> CollabError {
    StoreError::Invalid("invalid authoritative Human Ask resolution receipt".into()).into()
}

pub fn check(c: &Connection, crew: &str, ask: &str) -> Result<AskResolutionCheck> {
    let subject = asks::resolution_subject(c, crew, ask)?;
    Ok(AskResolutionCheck {
        ask: subject.view,
        resolution_revision: subject.revision,
    })
}

pub fn resolve(tx: &mut Tx<'_>, p: AskResolutionParams) -> Result<AskResolutionReceipt> {
    let revision = &p.expected_resolution_revision;
    if revision.len() != 64
        || !revision
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(resource_acl::invalid("expectedResolutionRevision"));
    }
    let value = p.text.trim();
    if value.is_empty() || p.text.len() > 65_536 {
        return Err(resource_acl::invalid("text"));
    }
    let original = asks::resolution_subject(tx.conn(), &p.crew_id, &p.ask)?;
    if original.revision.as_deref() != Some(revision.as_str()) {
        return Err(CollabError::Conflict(Refusal::new(
            Code::AskResolutionChanged,
        )));
    }
    let seq = original.before_seq.checked_add(1).ok_or_else(invalid)?;
    let stored_seq = i64::try_from(seq).map_err(|_| invalid())?;
    let resolved = match p.disposition {
        AskResolutionDisposition::Answer => asks::answer(tx, &p.crew_id, &p.ask, &p.text)?,
        AskResolutionDisposition::Withdraw => {
            asks::withdraw(tx, &Sender::Human, &p.crew_id, &p.ask, &p.text)?
        }
    };

    // WorkBook writes the Human answer/withdrawal as the first event.
    // Released blockers and wakes, if any, stay in the existing transaction.
    let raw: String = tx.conn().query_row(
        "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
        params![stored_seq, p.crew_id],
        |row| row.get(0),
    )?;
    let event: Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if event.seq != seq || event.by != Actor::Human {
        return Err(invalid());
    }
    let mut expected = original.view;
    expected.closed_at = Some(crewd_store::ts_from_ms(event.at.0));
    match (&event.kind, p.disposition) {
        (EventKind::AskAnswered { ask, text }, AskResolutionDisposition::Answer)
            if *ask == original.id && text.as_str() == value =>
        {
            expected.state = AskStateView::Answered;
            expected.answer = Some(text.clone());
        }
        (EventKind::AskWithdrawn { ask, reason }, AskResolutionDisposition::Withdraw)
            if *ask == original.id && reason.as_str() == value =>
        {
            expected.state = AskStateView::Withdrawn;
            expected.withdraw_reason = Some(reason.clone());
        }
        _ => return Err(invalid()),
    }
    if resolved != expected {
        return Err(invalid());
    }
    Ok(AskResolutionReceipt {
        crew_id: p.crew_id,
        ask: p.ask,
        original_resolution_revision: p.expected_resolution_revision,
        disposition: p.disposition,
        event_seq: seq.to_string(),
        resolved_at_ms: event.at.0,
        resolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, ok, ok_at, store};
    use crewd_proto::{
        Ask, AskDefault, HumanTaskCreateParams, WorkBlockOn, WorkBlockParams, WorkContinuation,
        WorkCreateParams,
    };
    use crewd_store::Store;

    fn question(s: &mut Store, default: bool) -> Ask {
        ok(s, |tx| {
            asks::open(
                tx,
                "w-a1",
                asks::OpenAsk {
                    question: "original synthetic question".into(),
                    default: default.then(|| AskDefault {
                        text: "original default".into(),
                        after_minutes: 1,
                    }),
                    resume_note: Some("return to the original work".into()),
                },
            )
        })
    }

    fn request(
        s: &Store,
        original: &Ask,
        disposition: AskResolutionDisposition,
    ) -> AskResolutionParams {
        let witness = check(s.conn(), &original.crew_id, &original.ask).unwrap();
        assert_eq!(&witness.ask, original);
        AskResolutionParams {
            crew_id: original.crew_id.clone(),
            ask: original.ask.clone(),
            expected_resolution_revision: witness.resolution_revision.unwrap(),
            disposition,
            text: " explicit Human text ".into(),
        }
    }

    #[test]
    fn committed_human_ask_resolution_and_default_arbitrate_on_the_existing_ledger() {
        for human_first in [true, false] {
            let mut s = store();
            let original = question(&mut s, true);
            let before = s.conn().total_changes();
            let p = request(&s, &original, AskResolutionDisposition::Answer);
            assert_eq!(s.conn().total_changes(), before);
            assert!(check(s.conn(), "crew-b", &original.ask).is_err());
            assert!(check(s.conn(), &original.crew_id, &original.ask.to_lowercase()).is_err());
            let at = NOW + 2 * MIN;
            if human_first {
                // The deadline alone never applies a default. The already
                // existing first committed transaction resolves the race.
                let (receipt, _) = ok_at(&mut s, at, |tx| resolve(tx, p.clone()));
                assert_eq!(receipt.resolved_at_ms, at);
                assert_eq!(receipt.resolved.state, AskStateView::Answered);
                assert_eq!(
                    receipt.resolved.answer.as_deref(),
                    Some("explicit Human text")
                );
                assert_eq!(receipt.resolved.question, original.question);
                assert_eq!(receipt.resolved.default, original.default);
                assert_eq!(
                    receipt.original_resolution_revision,
                    p.expected_resolution_revision
                );
                assert!(
                    ok_at(&mut s, at, |tx| asks::apply_due_defaults(tx, "crew-a"))
                        .0
                        .is_empty()
                );
            } else {
                assert_eq!(
                    ok_at(&mut s, at, |tx| asks::apply_due_defaults(tx, "crew-a")).0,
                    [original.ask.clone()]
                );
            }
            let result = check(s.conn(), "crew-a", &original.ask).unwrap();
            assert_eq!(
                result.ask.state,
                if human_first {
                    AskStateView::Answered
                } else {
                    AskStateView::Defaulted
                }
            );
            assert!(result.resolution_revision.is_none());
            let events = count(&s, "SELECT count(*) FROM organization_work_events");
            let wakes = count(&s, "SELECT count(*) FROM wakes WHERE kind='ask_answer'");
            assert!(matches!(crate::run(&mut s, at, |tx| resolve(tx, p)),
                Err(CollabError::Conflict(ref r)) if r.code == Code::AskResolutionChanged));
            assert_eq!(
                count(&s, "SELECT count(*) FROM organization_work_events"),
                events
            );
            assert_eq!(
                count(&s, "SELECT count(*) FROM wakes WHERE kind='ask_answer'"),
                wakes
            );
        }
    }

    #[test]
    fn resolution_fences_restore_and_preserves_withdrawal_and_manual_task_authority() {
        let mut s = store();
        let original = question(&mut s, false);
        let old = request(&s, &original, AskResolutionDisposition::Withdraw);
        let id = asks::resolution_subject(s.conn(), "crew-a", &original.ask)
            .unwrap()
            .id;
        ok_at(&mut s, NOW + 30 * 60 * MIN, |tx| {
            asks::with_book(tx, "crew-a", |book, now| {
                book.note_escalation(&Actor::System, now, id, crewd_work::WaitTier::Pinned)
            })
        });
        assert_eq!(
            check(s.conn(), "crew-a", &original.ask)
                .unwrap()
                .resolution_revision
                .as_deref(),
            Some(old.expected_resolution_revision.as_str())
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(crate::run(&mut s, NOW, |tx| resolve(tx, old)).is_err());
        let latest = request(&s, &original, AskResolutionDisposition::Withdraw);
        let receipt = ok(&mut s, |tx| resolve(tx, latest));
        assert_eq!(receipt.resolved.state, AskStateView::Withdrawn);
        assert_eq!(
            receipt.resolved.withdraw_reason.as_deref(),
            Some("explicit Human text")
        );
        assert_eq!(receipt.resolved.message_id, original.message_id);
        assert!(receipt.resolved.answer.is_none());
        let authored = ok(&mut s, |tx| {
            crate::todo_attributes::create_human(
                tx,
                &Sender::Human,
                HumanTaskCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    channel_id: "crew-a/lobby".into(),
                    text: "manual fixture task".into(),
                },
            )
        });
        let manual = check(s.conn(), "crew-a", &authored.ask.ask).unwrap();
        assert_eq!(manual.ask.state, AskStateView::Open);
        assert!(manual.resolution_revision.is_none());
    }

    #[test]
    fn attached_work_cannot_be_withdrawn_and_failed_primary_proof_rolls_back_answer() {
        let mut s = store();
        let plan = ok(&mut s, |tx| {
            crate::work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    owner: Some("w-a1".into()),
                    title: "fixture awaiting input".into(),
                    ..Default::default()
                },
            )
        });
        let blocked = ok(&mut s, |tx| {
            crate::work_block::block(
                tx,
                &Sender::Human,
                WorkBlockParams {
                    id: plan.id.clone(),
                    expected_revision: plan.revision,
                    on: vec![WorkBlockOn::NewHuman {
                        question: "fixture blockers".into(),
                        default: None,
                    }],
                    can_continue: WorkContinuation::Nothing,
                },
            )
        });
        let original = asks::get(s.conn(), "crew-a", &blocked.human_tasks[0]).unwrap();
        let withdraw = request(&s, &original, AskResolutionDisposition::Withdraw);
        assert!(
            matches!(crate::run(&mut s, NOW, |tx| resolve(tx, withdraw)),
            Err(CollabError::Conflict(ref r)) if r.code == Code::AskInUse)
        );
        let answer = request(&s, &original, AskResolutionDisposition::Answer);
        let events = count(&s, "SELECT count(*) FROM organization_work_events");
        let wakes = count(&s, "SELECT count(*) FROM wakes WHERE kind='ask_answer'");
        s.conn().execute_batch(
            "CREATE TRIGGER corrupt_fixture_resolution AFTER INSERT ON organization_work_events
             WHEN json_extract(NEW.body,'$.kind.type')='ask_answered'
             BEGIN UPDATE organization_work_events SET body=json_set(body,'$.by.kind','system') WHERE seq=NEW.seq; END;"
        ).unwrap();
        assert!(crate::run(&mut s, NOW, |tx| resolve(tx, answer.clone())).is_err());
        assert_eq!(
            asks::get(s.conn(), "crew-a", &original.ask).unwrap(),
            original
        );
        assert_eq!(
            crate::work::get(s.conn(), &plan.id).unwrap().status,
            "blocked"
        );
        assert_eq!(
            count(&s, "SELECT count(*) FROM organization_work_events"),
            events
        );
        assert_eq!(
            count(&s, "SELECT count(*) FROM wakes WHERE kind='ask_answer'"),
            wakes
        );
        s.conn()
            .execute_batch("DROP TRIGGER corrupt_fixture_resolution")
            .unwrap();
        assert_eq!(
            ok(&mut s, |tx| resolve(tx, answer)).resolved.state,
            AskStateView::Answered
        );
        assert_eq!(
            crate::work::get(s.conn(), &plan.id).unwrap().status,
            "active"
        );
        assert_eq!(
            count(&s, "SELECT count(*) FROM wakes WHERE kind='ask_answer'"),
            wakes + 1
        );
    }
}
