//! Independent responsibility-chain acceptance at the actual transaction boundary.
//! DAO ACKs in this file are not a claim of physical runner delivery.
use super::*;
use crate::{
    admin_case_execution::tests::running_case,
    admin_case_handoff as handoff,
    testkit::{NOW, ok, store},
    wakes,
};

fn accepted_and_replaced() -> (crewd_store::Store, AdminCaseView, String) {
    let mut s = store();
    let (case, registration) = running_case(&mut s);
    let request = uuid::Uuid::new_v4().to_string();
    let offer = ok(&mut s, |tx| {
        handoff::request(
            tx,
            &Sender::Session(registration.session.id),
            &request,
            "original explicit custody",
        )
    });
    ok(&mut s, |tx| {
        wakes::claim(tx, &[offer.wake_id])?;
        wakes::ack_delivered(
            tx,
            &offer.captain_session_id,
            &[offer.wake_id],
            wakes::Riders::Release,
            &[],
            &[],
        )
    });
    let cap = Sender::Session(offer.captain_session_id.clone());
    let delivered = handoff::get(s.conn(), &cap, &request, NOW).unwrap();
    ok(&mut s, |tx| {
        handoff::accept(
            tx,
            &cap,
            &request,
            &delivered.revision,
            "I accept the original responsibility",
        )
    });
    ok(&mut s, |tx| {
        crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
    });
    let next = ok(&mut s, |tx| {
        crate::sessions::register(
            tx,
            crate::sessions::NewSession {
                crew_id: "crew-a".into(),
                role: Role::Worker,
                executor: "claude".into(),
                title: Some("new captain".into()),
                ..Default::default()
            },
        )
    });
    ok(&mut s, |tx| {
        crate::sessions::set_desired(tx, &next.id, Desired::Running)?;
        crate::crews::set_captain(tx, "crew-a", &next.id)
    });
    (s, case, next.id)
}

#[test]
fn actual_accepted_custody_remains_readable_after_current_captain_changes() {
    let (s, case, _) = accepted_and_replaced();
    assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
    let view = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW)
        .expect("current Captain change must not erase the old accepted responsibility");
    assert_eq!(view.current_custodian_session_id, "cap-a");
    assert!(view.pending.is_none());
}

fn meta_snapshot(s: &crewd_store::Store) -> Vec<(String, String)> {
    s.conn()
        .prepare("SELECT key,value FROM meta ORDER BY key")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn proposal(case: &AdminCaseView, revision: String) -> AdminCaseCustodyPropose {
    AdminCaseCustodyPropose {
        crew_id: "crew-a".into(),
        case_id: case.id.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: revision,
        reason: "Human explicitly appoints current captain as successor".into(),
    }
}

#[test]
fn failed_proposal_transaction_keeps_original_custody_and_creates_no_notification() {
    let (mut s, case, _) = accepted_and_replaced();
    let before_read = meta_snapshot(&s);
    let current = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap();
    assert_eq!(
        meta_snapshot(&s),
        before_read,
        "status read must not materialize custody"
    );
    let p = proposal(&case, current.revision);
    let before = meta_snapshot(&s);
    let wakes_before: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get(0))
        .unwrap();
    let mut mutation_reached = false;
    let failed = crate::run::<()>(&mut s, NOW, |tx| {
        propose(tx, &Sender::Human, p)?;
        mutation_reached = true;
        Err(crate::resource_acl::invalid(
            "deliberate custody transaction rollback",
        ))
    });
    assert!(
        mutation_reached,
        "test must reach the real proposal mutation before injecting failure"
    );
    assert!(failed.is_err());
    assert_eq!(meta_snapshot(&s), before);
    assert_eq!(
        s.conn()
            .query_row("SELECT COUNT(*) FROM wakes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        wakes_before
    );
    assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
}

#[test]
fn dao_delivery_is_not_acceptance_and_atomic_acceptance_moves_only_responsibility() {
    let (mut s, case, next) = accepted_and_replaced();
    let work_before: Vec<_> = case
        .targets
        .iter()
        .map(|t| crate::work::get(s.conn(), &t.id).unwrap())
        .collect();
    let current = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap();
    let p = proposal(&case, current.revision);
    let proposed = ok(&mut s, |tx| propose(tx, &Sender::Human, p));
    let transfer = proposed.pending.unwrap();
    assert_eq!(proposed.current_custodian_session_id, "cap-a");
    assert!(
        crate::run(&mut s, NOW, |tx| accept(
            tx,
            &Sender::Session(next.clone()),
            &transfer.request_id,
            &transfer.revision,
            "I accept responsibility"
        ))
        .is_err()
    );
    ok(&mut s, |tx| {
        wakes::claim(tx, &[transfer.wake_id])?;
        wakes::ack_delivered(
            tx,
            &next,
            &[transfer.wake_id],
            wakes::Riders::Release,
            &[],
            &[],
        )
    });
    let delivered = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap();
    assert_eq!(delivered.current_custodian_session_id, "cap-a");
    assert!(handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
    let pending = delivered.pending.unwrap();
    ok(&mut s, |tx| {
        accept(
            tx,
            &Sender::Session(next.clone()),
            &pending.request_id,
            &pending.revision,
            "I accept responsibility",
        )
    });
    assert!(!handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
    assert!(handoff::has_unresolved_custody(s.conn(), &next, NOW).unwrap());
    assert!(
        handoff::writes_relinquished(s.conn(), &case.id).unwrap(),
        "original Admin writes stay permanently relinquished"
    );
    assert_eq!(
        case.targets
            .iter()
            .map(|t| crate::work::get(s.conn(), &t.id).unwrap())
            .collect::<Vec<_>>(),
        work_before
    );
}

#[test]
fn renewed_custody_after_restore_discharges_on_current_typed_closures_without_rewriting_old_outcome()
 {
    let (mut s, case, next) = accepted_and_replaced();
    ok(&mut s, |tx| {
        crewd_store::judgment_state::restored(tx.conn())?;
        wakes::human_stop(tx, &case.source.session_id)?;
        Ok(())
    });
    let old_outcome = crate::admin_case_outcome::status_at(s.conn(), "crew-a", &case.id, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(old_outcome.phase, AdminCaseOutcomePhase::Stale);
    let current = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW).unwrap();
    let p = proposal(&case, current.revision);
    let offered = ok(&mut s, |tx| propose(tx, &Sender::Human, p));
    let pending = offered.pending.unwrap();
    ok(&mut s, |tx| {
        wakes::claim(tx, &[pending.wake_id])?;
        wakes::ack_delivered(
            tx,
            &next,
            &[pending.wake_id],
            wakes::Riders::Release,
            &[],
            &[],
        )
    });
    let pending = get(s.conn(), &Sender::Human, "crew-a", &case.id, NOW)
        .unwrap()
        .pending
        .unwrap();
    ok(&mut s, |tx| {
        accept(
            tx,
            &Sender::Session(next.clone()),
            &pending.request_id,
            &pending.revision,
            "I accept under the current restored authority",
        )
    });
    assert!(handoff::has_unresolved_custody(s.conn(), &next, NOW).unwrap());
    let verified = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
    assert_ne!(
        verified.phase,
        AdminCaseCustodyPhase::Discharged,
        "acceptance alone never resolves the targets"
    );
    for id in case
        .targets
        .iter()
        .map(|t| t.id.clone())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let w = crate::work::get(s.conn(), &id).unwrap();
        ok(&mut s, |tx| {
            crate::work::close_factual(
                tx,
                &Sender::Session(next.clone()),
                WorkCloseParams {
                    id: w.id,
                    expected_revision: w.revision,
                    kind: WorkClosureKind::Done,
                    evidence: WorkEvidenceParam {
                        kind: WorkEvidenceKind::Text,
                        value: "independent verifiable completion evidence".into(),
                    },
                    note: "current captain confirms the actual completed work".into(),
                },
            )
        });
    }
    let discharged = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
    assert_eq!(discharged.phase, AdminCaseCustodyPhase::Discharged);
    assert!(!handoff::has_unresolved_custody(s.conn(), &next, NOW).unwrap());
    assert!(!handoff::has_unresolved_custody(s.conn(), "cap-a", NOW).unwrap());
    assert!(handoff::writes_relinquished(s.conn(), &case.id).unwrap());
    let original = crate::admin_case_outcome::status_at(s.conn(), "crew-a", &case.id, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(original.phase, AdminCaseOutcomePhase::Stale);
    assert_eq!(
        original.verified_at, old_outcome.verified_at,
        "custody verification must not overwrite old case verification"
    );
    assert!(
        discharged
            .discharge
            .unwrap()
            .targets
            .iter()
            .all(|t| t.resolved_by == Some(AdminCaseResolutionActor::Other)),
        "new captain's actual closures are not old Admin's achievements"
    );
}
