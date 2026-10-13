//! Bounded explicit patrol projection. No scheduling, decisions, or mutations.
use crate::{Result, asks, sessions, work};
use crewd_proto::{
    WorkListParams, WorkPatrolItem, WorkPatrolLost, WorkPatrolPage, WorkPatrolParams,
    WorkPatrolWait,
};
use crewd_store::{StoreError, object_refs::ObjectKind, work_stop_checks};
use crewd_work::{LostCause, SessionId, Ts};
use rusqlite::Connection;
use std::collections::BTreeMap;

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
    let book = asks::load_org(conn)?;
    let mut owners: BTreeMap<SessionId, (Option<String>, bool)> = BTreeMap::new();
    let mut items = Vec::with_capacity(page.items.len());
    for row in page.items {
        let reference =
            crewd_store::object_refs::get(conn, ObjectKind::Plan, work::number(&row.id)?)?
                .ok_or_else(|| StoreError::Invalid("patrol item has no issued identity".into()))?;
        let local = work::core_plan(conn, &reference)?;
        let item =
            book.state().items.get(&local).ok_or_else(|| {
                StoreError::Invalid("patrol item has no authoritative state".into())
            })?;
        if let Some(owner) = &item.owner
            && let std::collections::btree_map::Entry::Vacant(entry) = owners.entry(owner.clone())
        {
            let session = sessions::get(conn, owner.as_str())?;
            let escalated =
                work_stop_checks::get(conn, owner.as_str())?.is_some_and(|b| b.streak.escalated);
            entry.insert((session.resume_failed_at, escalated));
        }
        let owner_facts = item.owner.as_ref().map(|o| &owners[o]);
        let facts = book
            .patrol_item(
                local,
                Ts(now_ms),
                owner_facts.is_some_and(|o| o.0.is_some()),
            )
            .ok_or_else(|| StoreError::Invalid("patrol item has no authoritative state".into()))?;
        let lost = facts.lost.map(|cause| match cause {
            LostCause::Unrecoverable => WorkPatrolLost::Unrecoverable {
                resume_failed_at: owner_facts
                    .and_then(|o| o.0.clone())
                    .expect("used actual failure fact"),
            },
            LostCause::Silent { missed } => WorkPatrolLost::Silent {
                missed_checks: missed,
            },
        });
        let stop_escalated = owner_facts.is_some_and(|o| o.1) && !facts.stop_violations.is_empty();
        let long_waits_truncated = facts.long_waits.len() > 8;
        let long_waits = facts
            .long_waits
            .iter()
            .take(8)
            .map(|ask| {
                let crew = book
                    .state()
                    .ask_crew(*ask)
                    .ok_or_else(|| {
                        StoreError::Invalid("patrol ask has no crew attribution".into())
                    })?
                    .to_string();
                let id = work::reference(conn, ObjectKind::HumanTask, &crew, ask)?;
                Ok(WorkPatrolWait {
                    crew_id: crew,
                    ask_id: id.clone(),
                    id,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        items.push(WorkPatrolItem {
            id: row.id,
            crew_id: row.crew_id,
            revision: row.revision,
            unowned: facts.unowned,
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
            asks::get(s.conn(), "crew-a", &h.id).unwrap().question,
            "synthetic blocker"
        );
        assert_eq!(s.conn().total_changes(), changes);
    }
    #[test]
    fn cross_crew_human_wait_is_a_complete_public_readback_handle() {
        let mut s = store();
        let a = create(&mut s, "crew-a", Some("w-a1"));
        let h = ok(&mut s, |tx| {
            asks::open(
                tx,
                "w-b1",
                asks::OpenAsk {
                    question: "cross-crew decision".into(),
                    ..Default::default()
                },
            )
        });
        assert_ne!(
            h.ask, "H1",
            "public H differs from pure H1 after W allocation"
        );
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &Actor::Human,
                    now,
                    WorkId(1),
                    vec![BlockOn::Ask("H1".parse().unwrap())],
                    Continuation::Nothing,
                )
            })
        });
        let before = s.conn().total_changes();
        let page = read(s.conn(), WorkPatrolParams::default(), NOW + 24 * 60 * MIN).unwrap();
        let wait = &page.items.iter().find(|i| i.id == a.id).unwrap().long_waits[0];
        assert_eq!(wait.crew_id, "crew-b");
        assert_eq!(wait.id, h.ask);
        assert_eq!(wait.ask_id, h.ask);
        let detail = asks::get(s.conn(), &wait.crew_id, &wait.ask_id).unwrap();
        assert_eq!(detail.question, "cross-crew decision");
        assert_eq!(detail.ask, wait.id);
        assert!(asks::get(s.conn(), "crew-a", &wait.ask_id).is_err());
        assert_eq!(s.conn().total_changes(), before);
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
            let h = crate::testkit::public_ask(&s, &format!("H{n}"));
            ok_at(&mut s, late, |tx| asks::answer(tx, "crew-a", &h, "answer"));
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
            .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
            .unwrap();
        assert!(read(s.conn(), WorkPatrolParams::default(), NOW).is_err());
    }
}
