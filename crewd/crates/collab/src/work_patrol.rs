//! Bounded explicit patrol projection. No scheduling, decisions, or mutations.
use crate::{Result, asks, sessions, work};
use crewd_proto::{
    WorkListParams, WorkPatrolItem, WorkPatrolLost, WorkPatrolPage, WorkPatrolParams,
    WorkPatrolWait,
};
use crewd_store::{StoreError, object_refs::ObjectKind, work_stop_checks};
use crewd_work::{Blocker, LostCause, SessionId, Ts, WorkBook};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};

struct CrewFacts {
    book: WorkBook,
    patrol: crewd_work::Patrol,
    violations: BTreeMap<SessionId, BTreeSet<crewd_work::WorkId>>,
}

pub fn read(conn: &Connection, p: WorkPatrolParams, now_ms: i64) -> Result<WorkPatrolPage> {
    // Reuse the issued-ID, limit, scope, and authority checks; do not add a second ledger.
    let page = work::list(
        conn,
        WorkListParams {
            crew_id: p.crew_id,
            after: p.after,
            limit: p.limit,
            live_only: true,
            include_removed: Some(false),
            ..Default::default()
        },
    )?;
    let mut crews: BTreeMap<String, CrewFacts> = BTreeMap::new();
    let mut owners: BTreeMap<SessionId, (Option<String>, bool)> = BTreeMap::new();
    for row in &page.items {
        if crews.contains_key(&row.crew_id) {
            continue;
        }
        let book = asks::load(conn, &row.crew_id)?;
        let mut unrecoverable = BTreeSet::new();
        let mut violations = BTreeMap::new();
        let page_owners: BTreeSet<SessionId> = page
            .items
            .iter()
            .filter(|i| i.crew_id == row.crew_id)
            .filter_map(|i| i.owner.as_deref())
            .map(SessionId::new)
            .collect();
        for owner in &page_owners {
            if let std::collections::btree_map::Entry::Vacant(entry) = owners.entry(owner.clone()) {
                let session = sessions::get(conn, owner.as_str())?;
                let escalated = work_stop_checks::get(conn, owner.as_str())?
                    .is_some_and(|b| b.streak.escalated);
                entry.insert((session.resume_failed_at, escalated));
            }
            if owners[owner].0.is_some() {
                unrecoverable.insert(owner.clone());
            }
            violations.entry(owner.clone()).or_insert_with(|| {
                book.stop_violations(owner, Ts(now_ms))
                    .into_iter()
                    .map(|v| v.item)
                    .collect()
            });
        }
        // Archive candidates and entry rulings are intentionally not projected here.
        let patrol = book.patrol(Ts(now_ms), &unrecoverable, &BTreeSet::new());
        crews.insert(
            row.crew_id.clone(),
            CrewFacts {
                book,
                patrol,
                violations,
            },
        );
    }
    let mut items = Vec::with_capacity(page.items.len());
    for row in page.items {
        let facts = &crews[&row.crew_id];
        let reference =
            crewd_store::object_refs::get(conn, ObjectKind::Plan, work::number(&row.id)?)?
                .ok_or_else(|| StoreError::Invalid("patrol item has no issued identity".into()))?;
        let local: crewd_work::WorkId = reference
            .source_id
            .strip_prefix("work:")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| StoreError::Invalid("patrol item identity is invalid".into()))?;
        let item =
            facts.book.state().items.get(&local).ok_or_else(|| {
                StoreError::Invalid("patrol item has no authoritative state".into())
            })?;
        let lost = facts
            .patrol
            .lost
            .iter()
            .find(|l| l.item == local)
            .map(|l| match l.cause {
                LostCause::Unrecoverable => WorkPatrolLost::Unrecoverable {
                    resume_failed_at: owners[&l.owner]
                        .0
                        .clone()
                        .expect("patrol used actual failure set"),
                },
                LostCause::Silent { missed } => WorkPatrolLost::Silent {
                    missed_checks: missed,
                },
            });
        let stop_escalated = item
            .owner
            .as_ref()
            .is_some_and(|o| owners[o].1 && facts.violations[o].contains(&local));
        let mut long_waits = Vec::new();
        for blocker in &item.blocked_on {
            if let Blocker::Human(ask) = blocker
                && facts.patrol.long_waits.contains(ask)
            {
                long_waits.push(WorkPatrolWait {
                    id: work::reference(conn, ObjectKind::HumanTask, &row.crew_id, ask)?,
                    ask_id: ask.to_string(),
                });
            }
        }
        let long_waits_truncated = long_waits.len() > 8;
        long_waits.truncate(8);
        items.push(WorkPatrolItem {
            id: row.id,
            crew_id: row.crew_id,
            revision: row.revision,
            unowned: facts.patrol.unowned_items.contains(&local),
            lost,
            stop_escalated,
            long_waits,
            long_waits_truncated,
        });
    }
    Ok(WorkPatrolPage {
        observed_at: crewd_store::ts_from_ms(now_ms),
        live_only: true,
        include_removed: false,
        unavailable_categories: ["unclaimed_entries", "needs_ruling", "archivable_channels"]
            .into_iter()
            .map(String::from)
            .collect(),
        items,
        next_after: page.next_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{MIN, NOW, ok, ok_at, store},
    };
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    use crewd_work::{Actor, BlockOn, Continuation, StopStreak, WorkId};
    fn create(
        s: &mut crewd_store::Store,
        crew: &str,
        owner: Option<&str>,
    ) -> crewd_proto::WorkPlan {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: "synthetic work".into(),
                    owner: owner.map(String::from),
                    ..Default::default()
                },
            )
        })
    }
    #[test]
    fn patrol_is_bounded_global_read_only_and_excludes_no_scope_silently() {
        let mut s = store();
        let a = create(&mut s, "crew-a", None);
        let b = create(&mut s, "crew-b", Some("w-b1"));
        let done = create(&mut s, "crew-a", None);
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: done.id,
                    expected_revision: done.revision,
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        });
        let removed = create(&mut s, "crew-x", None);
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='removed' WHERE id='crew-x'",
                [],
            )
            .unwrap();
        let changes = s.conn().total_changes();
        let first = read(
            s.conn(),
            WorkPatrolParams {
                limit: Some(1),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].id, a.id);
        assert!(first.items[0].unowned);
        assert_eq!(first.next_after, Some(a.id.clone()));
        let second = read(
            s.conn(),
            WorkPatrolParams {
                after: first.next_after,
                limit: Some(1),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(second.items[0].id, b.id);
        assert_ne!(a.id, b.id); // Both books issued local W1.
        assert!(!second.items[0].unowned);
        assert!(second.items[0].lost.is_none()); // idle/desired-stopped is not loss.
        assert!(second.next_after.is_none());
        assert_eq!(first.observed_at, second.observed_at);
        assert!(first.live_only && !first.include_removed);
        assert_eq!(
            first.unavailable_categories,
            ["unclaimed_entries", "needs_ruling", "archivable_channels"]
        );
        assert!(work::get(s.conn(), &removed.id).unwrap().frozen);
        assert_eq!(s.conn().total_changes(), changes);
    }
    #[test]
    fn actual_resume_failure_and_current_global_escalation_are_facts_not_actions() {
        let mut s = store();
        let a = create(&mut s, "crew-a", Some("w-a1"));
        ok(&mut s, |tx| sessions::set_resume_failed(tx, "w-a1", true));
        ok(&mut s, |tx| {
            assert!(work_stop_checks::save(
                tx.conn(),
                "w-a1",
                None,
                &StopStreak {
                    escalated: true,
                    ..Default::default()
                }
            )?);
            Ok(())
        });
        let changes = s.conn().total_changes();
        let p = read(s.conn(), WorkPatrolParams::default(), NOW).unwrap();
        assert_eq!(p.items[0].id, a.id);
        assert!(matches!(
            p.items[0].lost,
            Some(WorkPatrolLost::Unrecoverable { .. })
        ));
        assert!(p.items[0].stop_escalated);
        assert_eq!(s.conn().total_changes(), changes);
        // A valid next check removes this item's violation even if an old budget remains.
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: a.id,
                    expected_revision: a.revision,
                    note: Some("real progress".into()),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + MIN)),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| sessions::set_resume_failed(tx, "w-a1", false));
        // Isolate the current-violation guard from normal progress clearing the budget.
        ok(&mut s, |tx| {
            let expected = work_stop_checks::get(tx.conn(), "w-a1")?.map(|b| b.epoch);
            assert!(work_stop_checks::save(
                tx.conn(),
                "w-a1",
                expected,
                &StopStreak {
                    escalated: true,
                    ..Default::default()
                }
            )?);
            Ok(())
        });
        let p = read(s.conn(), WorkPatrolParams::default(), NOW).unwrap();
        assert!(!p.items[0].stop_escalated);
        assert!(p.items[0].lost.is_none());
    }
    #[test]
    fn human_wait_uses_existing_threshold_and_global_readback_identity() {
        let mut s = store();
        let a = create(&mut s, "crew-a", Some("w-a1"));
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &Actor::worker("w-a1"),
                    now,
                    WorkId(1),
                    vec![BlockOn::NewAsk {
                        question: "synthetic blocker".into(),
                        default: None,
                    }],
                    Continuation::Nothing,
                )
            })
        });
        let early = read(s.conn(), WorkPatrolParams::default(), NOW).unwrap();
        assert!(early.items[0].long_waits.is_empty());
        let changes = s.conn().total_changes();
        let late = read(s.conn(), WorkPatrolParams::default(), NOW + 24 * 60 * MIN).unwrap();
        assert_eq!(late.items[0].id, a.id);
        assert_eq!(late.items[0].long_waits.len(), 1);
        let h = &late.items[0].long_waits[0];
        let reference = crewd_store::object_refs::get(
            s.conn(),
            ObjectKind::HumanTask,
            h.id.strip_prefix('H').unwrap().parse().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(reference.crew_id, "crew-a");
        assert_eq!(
            asks::get(s.conn(), "crew-a", &h.ask_id).unwrap().question,
            "synthetic blocker"
        );
        assert_eq!(s.conn().total_changes(), changes);
    }
    #[test]
    fn long_wait_facts_are_capped_and_answered_blockers_disappear() {
        let mut s = store();
        create(&mut s, "crew-a", Some("w-a1"));
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &Actor::worker("w-a1"),
                    now,
                    WorkId(1),
                    (0..9)
                        .map(|n| BlockOn::NewAsk {
                            question: format!("synthetic question {n}"),
                            default: None,
                        })
                        .collect(),
                    Continuation::Nothing,
                )
            })
        });
        let late = NOW + 24 * 60 * MIN;
        let changes = s.conn().total_changes();
        let page = read(s.conn(), WorkPatrolParams::default(), late).unwrap();
        assert_eq!(page.items[0].long_waits.len(), 8);
        assert!(page.items[0].long_waits_truncated);
        assert_eq!(s.conn().total_changes(), changes);
        for n in 1..=9 {
            ok_at(&mut s, late, |tx| {
                asks::answer(tx, "crew-a", &format!("H{n}"), "answer")
            });
        }
        let changes = s.conn().total_changes();
        let page = read(s.conn(), WorkPatrolParams::default(), late).unwrap();
        assert!(page.items[0].long_waits.is_empty());
        assert!(!page.items[0].long_waits_truncated);
        assert_eq!(s.conn().total_changes(), changes);
    }
    #[test]
    fn silent_requires_recorded_checkpoint_wakes_and_due_existing_rule() {
        let mut s = store();
        let work = create(&mut s, "crew-a", Some("w-a1"));
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: work.id,
                    expected_revision: work.revision,
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + MIN)),
                    note: Some("scheduled".into()),
                    ..Default::default()
                },
            )
        });
        let owner = SessionId::new("w-a1");
        assert!(
            read(s.conn(), WorkPatrolParams::default(), NOW + 60 * MIN)
                .unwrap()
                .items[0]
                .lost
                .is_none()
        );
        for now in [NOW, NOW + 15 * MIN] {
            ok_at(&mut s, now, |tx| {
                asks::with_book(tx, "crew-a", |b, at| {
                    b.record_checkpoint_wake(&Actor::System, at, &owner, &[WorkId(1)])
                })
            });
        }
        let early = read(s.conn(), WorkPatrolParams::default(), NOW + 29 * MIN).unwrap();
        assert!(early.items[0].lost.is_none());
        let changes = s.conn().total_changes();
        let late = read(s.conn(), WorkPatrolParams::default(), NOW + 30 * MIN).unwrap();
        assert_eq!(
            late.items[0].lost,
            Some(WorkPatrolLost::Silent { missed_checks: 2 })
        );
        assert_eq!(s.conn().total_changes(), changes);
    }
    #[test]
    fn invalid_queries_and_missing_authority_fail_instead_of_empty_success() {
        let mut s = store();
        create(&mut s, "crew-a", None);
        for after in ["W0", "W01", "H1"] {
            assert!(
                read(
                    s.conn(),
                    WorkPatrolParams {
                        after: Some(after.into()),
                        ..Default::default()
                    },
                    NOW
                )
                .is_err()
            );
        }
        for limit in [0, 201] {
            assert!(
                read(
                    s.conn(),
                    WorkPatrolParams {
                        limit: Some(limit),
                        ..Default::default()
                    },
                    NOW
                )
                .is_err()
            );
        }
        s.conn()
            .execute("DELETE FROM work_books WHERE crew_id='crew-a'", [])
            .unwrap();
        assert!(read(s.conn(), WorkPatrolParams::default(), NOW).is_err());
    }
}
