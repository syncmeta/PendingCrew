//! Bounded explicit patrol projection. No scheduling, decisions, or mutations.
use crate::{Result, asks, sessions, work};
use crewd_proto::{
    WorkListParams, WorkPatrolChannel, WorkPatrolChannelBlocker, WorkPatrolEntry, WorkPatrolItem,
    WorkPatrolLost, WorkPatrolPage, WorkPatrolParams, WorkPatrolScan, WorkPatrolWait,
};
use crewd_store::{StoreError, object_refs::ObjectKind, work_stop_checks};
use crewd_work::{LostCause, SessionId, Ts};
use rusqlite::Connection;
use std::collections::BTreeMap;

pub fn read(conn: &Connection, p: WorkPatrolParams, now_ms: i64) -> Result<WorkPatrolPage> {
    if p.scan != WorkPatrolScan::Work {
        return read_other(conn, p, now_ms);
    }
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
        let paused = match &item.owner {
            Some(o) => crate::wakes::automatic_work_paused(conn, o.as_str())?,
            None => false,
        };
        let lost = facts
            .lost
            .filter(|c| !paused || matches!(c, LostCause::Unrecoverable))
            .map(|cause| match cause {
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
        scan: WorkPatrolScan::Work,
        entries: vec![],
        channels: vec![],
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

fn read_other(conn: &Connection, p: WorkPatrolParams, now: i64) -> Result<WorkPatrolPage> {
    use rusqlite::params;
    let bad = || StoreError::Invalid("invalid patrol cursor or limit".into());
    let limit = p.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return Err(bad().into());
    }
    if let Some(crew) = &p.crew_id {
        crate::crews::get(conn, crew)?;
    }
    let mut out = WorkPatrolPage {
        scan: p.scan,
        entries: vec![],
        channels: vec![],
        observed_at: crewd_store::ts_from_ms(now),
        live_only: false,
        include_removed: false,
        unavailable_categories: vec![],
        items: vec![],
        next_after: None,
    };
    let book = asks::load_org(conn)?;
    match p.scan {
        WorkPatrolScan::Entries => {
            let after = match p.after.as_deref() {
                None => 0,
                Some(id) => id
                    .strip_prefix('#')
                    .and_then(|n| n.parse::<i64>().ok())
                    .filter(|n| *n > 0 && id == format!("#{n}"))
                    .ok_or_else(bad)?,
            };
            let mut query = conn.prepare("SELECT r.number,r.crew_id FROM object_refs r JOIN crews c ON c.id=r.crew_id WHERE r.kind='requirement' AND r.number>?1 AND (?2 IS NULL OR r.crew_id=?2) AND c.archived_at IS NULL ORDER BY r.number LIMIT ?3")?;
            let mut rows = query
                .query_map(params![after, p.crew_id, i64::from(limit) + 1], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() > usize::from(limit) {
                rows.truncate(usize::from(limit));
                out.next_after = rows.last().map(|r| format!("#{}", r.0));
            }
            for (number, crew_id) in rows {
                let id = format!("#{number}");
                let core = crewd_store::work_organization::core_for_reference(
                    conn,
                    ObjectKind::Requirement,
                    number,
                )?
                .ok_or_else(bad)?
                .parse()
                .map_err(|_| bad())?;
                let view = book.entry_view(core).ok_or_else(bad)?;
                let resolved = book.state().entries.get(&core).ok_or_else(bad)?.manual_done;
                out.entries.push(WorkPatrolEntry {
                    id,
                    crew_id,
                    unclaimed: !resolved && view.status == crewd_work::EntryStatus::Unclaimed,
                    needs_ruling: !resolved && view.status == crewd_work::EntryStatus::NeedsRuling,
                });
            }
            out.unavailable_categories = vec!["work_items".into(), "archivable_channels".into()];
        }
        WorkPatrolScan::Channels => {
            if let Some(id) = &p.after
                && (id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
            {
                return Err(bad().into());
            }
            let mut query=conn.prepare("SELECT ch.id,ch.crew_id FROM channels ch JOIN crews c ON c.id=ch.crew_id WHERE ch.kind='topic' AND ch.archived_at IS NULL AND c.archived_at IS NULL AND (?1 IS NULL OR ch.id>?1) AND (?2 IS NULL OR ch.crew_id=?2) ORDER BY ch.id LIMIT ?3")?;
            let mut rows = query
                .query_map(params![p.after, p.crew_id, i64::from(limit) + 1], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() > usize::from(limit) {
                rows.truncate(usize::from(limit));
                out.next_after = rows.last().map(|r| r.0.clone());
            }
            let page_ids: std::collections::BTreeSet<_> =
                rows.iter().map(|r| r.0.as_str()).collect();
            let mut removed_sources = BTreeMap::<String, bool>::new();
            let mut legacy = BTreeMap::<String, (bool, bool, bool)>::new();
            for (work_id, item) in &book.state().items {
                if let Some(channel) = &item.channel {
                    if !page_ids.contains(channel.as_str()) {
                        continue;
                    }
                    let fact = legacy.entry(channel.to_string()).or_default();
                    fact.0 = true;
                    fact.1 |= item.status.is_live();
                    let source = book.state().item_crew(*work_id).ok_or_else(bad)?;
                    let removed = match removed_sources.get(source.as_str()) {
                        Some(v) => *v,
                        None => {
                            let v = crate::crews::get(conn, source.as_str())?
                                .removed_at
                                .is_some();
                            removed_sources.insert(source.to_string(), v);
                            v
                        }
                    };
                    fact.2 |= removed;
                }
            }
            for (id, crew_id) in rows {
                let (mut any, mut live, mut removed) = legacy.get(&id).copied().unwrap_or_default();
                let mut links=conn.prepare("SELECT object_kind,object_id,source_crew_id FROM channel_work_links WHERE channel_id=?1 ORDER BY object_kind,object_id LIMIT 201")?;
                let linked = links
                    .query_map([&id], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let truncated = linked.len() > 200;
                let mut non_work = false;
                for (kind, object, source) in linked.into_iter().take(200) {
                    if kind != "work" {
                        non_work = true;
                        continue;
                    }
                    let reference = crewd_store::object_refs::get(
                        conn,
                        ObjectKind::Plan,
                        work::number(&object)?,
                    )?
                    .ok_or_else(bad)?;
                    if reference.crew_id != source {
                        return Err(bad().into());
                    }
                    removed |= match removed_sources.get(&source) {
                        Some(v) => *v,
                        None => {
                            let v = crate::crews::get(conn, &source)?.removed_at.is_some();
                            removed_sources.insert(source, v);
                            v
                        }
                    };
                    let core = work::core_plan(conn, &reference)?;
                    let item = book.state().items.get(&core).ok_or_else(bad)?;
                    any = true;
                    live |= item.status.is_live();
                }
                let mut blocked_by = vec![];
                for (yes, reason) in [
                    (!any, WorkPatrolChannelBlocker::NoLinkedWork),
                    (live, WorkPatrolChannelBlocker::LiveWork),
                    (removed, WorkPatrolChannelBlocker::SourceRemoved),
                    (non_work, WorkPatrolChannelBlocker::NonWorkLinks),
                    (truncated, WorkPatrolChannelBlocker::LinksTruncated),
                ] {
                    if yes {
                        blocked_by.push(reason);
                    }
                }
                out.channels.push(WorkPatrolChannel {
                    id,
                    crew_id,
                    archive_candidate: blocked_by.is_empty(),
                    blocked_by,
                });
            }
            out.unavailable_categories = vec![
                "work_items".into(),
                "unclaimed_entries".into(),
                "needs_ruling".into(),
            ];
        }
        WorkPatrolScan::Work => unreachable!(),
    }
    Ok(out)
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
    #[test]
    fn entries_scan_accepts_independent_cursor_without_work() {
        let s = store();
        let params: WorkPatrolParams =
            serde_json::from_value(serde_json::json!({"scan":"entries","limit":1})).unwrap();
        let page = read(s.conn(), params, NOW).unwrap();
        let wire = serde_json::to_value(page).unwrap();
        assert_eq!(wire["scan"], "entries");
        assert_eq!(wire["entries"], serde_json::json!([]));
    }
    #[test]
    fn independent_sources_are_bounded_scoped_and_read_only() {
        use crewd_proto::{TodoCloseParams, TodoCreateParams};
        let mut s = store();
        let a = ok(&mut s, |tx| {
            crate::requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    text: "closed source".into(),
                    channel_id: None,
                },
            )
        })
        .todo
        .requirement_id
        .unwrap();
        ok(&mut s, |tx| {
            crate::requirements::close(
                tx,
                &Sender::Human,
                TodoCloseParams {
                    id: a.clone(),
                    expected_status: "unclaimed".into(),
                    note: "resolved".into(),
                },
            )
        });
        let b = ok(&mut s, |tx| {
            crate::requirements::create(
                tx,
                &Sender::Human,
                TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-b".into(),
                    text: "unclaimed source".into(),
                    channel_id: None,
                },
            )
        })
        .todo
        .requirement_id
        .unwrap();
        let changes = s.conn().total_changes();
        let first = read(
            s.conn(),
            WorkPatrolParams {
                scan: WorkPatrolScan::Entries,
                limit: Some(1),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(first.entries[0].id, a);
        assert!(!first.entries[0].unclaimed);
        assert_eq!(first.next_after, Some(a));
        let second = read(
            s.conn(),
            WorkPatrolParams {
                scan: WorkPatrolScan::Entries,
                after: first.next_after,
                limit: Some(1),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(second.entries[0].id, b);
        assert!(second.entries[0].unclaimed);
        assert!(second.next_after.is_none());
        assert!(
            read(
                s.conn(),
                WorkPatrolParams {
                    scan: WorkPatrolScan::Entries,
                    after: Some("W1".into()),
                    ..Default::default()
                },
                NOW
            )
            .is_err()
        );
        assert_eq!(s.conn().total_changes(), changes);
        let scoped = read(
            s.conn(),
            WorkPatrolParams {
                scan: WorkPatrolScan::Entries,
                crew_id: Some("crew-a".into()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert_eq!(scoped.entries.len(), 1);
        let plan = ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-b".into(),
                    title: "dropped plan".into(),
                    sources: vec![b.clone()],
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: plan.id,
                    expected_revision: plan.revision,
                    status: Some(WorkUpdateStatus::Dropped),
                    note: Some("obsolete approach".into()),
                    ..Default::default()
                },
            )
        });
        let ruling = read(
            s.conn(),
            WorkPatrolParams {
                scan: WorkPatrolScan::Entries,
                crew_id: Some("crew-b".into()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert!(ruling.entries[0].needs_ruling);
        assert!(!ruling.entries[0].unclaimed);
    }

    #[test]
    fn channel_scan_covers_terminal_links_without_live_work() {
        let mut s = store();
        let work = create(&mut s, "crew-a", None);
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Human,
                WorkUpdateParams {
                    id: work.id.clone(),
                    expected_revision: work.revision,
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        });
        for id in ["patrol-a", "patrol-b"] {
            s.conn().execute("INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,'crew-a','topic','test','human','2026-10-01T00:00:00Z')",[id]).unwrap();
        }
        s.conn()
            .execute(
                "INSERT INTO channel_work_scopes(channel_id,revision) VALUES('patrol-b',1)",
                [],
            )
            .unwrap();
        s.conn().execute("INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) VALUES('patrol-b','work',?1,'crew-a')",[&work.id]).unwrap();
        s.conn()
            .execute("DELETE FROM channels WHERE id='patrol-a'", [])
            .unwrap();
        let changes = s.conn().total_changes();
        let p = WorkPatrolParams {
            scan: WorkPatrolScan::Channels,
            after: Some("patrol-a".into()),
            limit: Some(1),
            ..Default::default()
        };
        let page = read(s.conn(), p.clone(), NOW).unwrap();
        assert_eq!(page.channels[0].id, "patrol-b");
        assert!(page.channels[0].archive_candidate);
        assert_eq!(read(s.conn(), p, NOW).unwrap(), page);
        assert_eq!(s.conn().total_changes(), changes);
        let active = create(&mut s, "crew-a", None);
        s.conn().execute("INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) VALUES('patrol-b','work',?1,'crew-a')",[&active.id]).unwrap();
        let inspect = |s: &crewd_store::Store| {
            read(
                s.conn(),
                WorkPatrolParams {
                    scan: WorkPatrolScan::Channels,
                    after: Some("patrol-a".into()),
                    limit: Some(1),
                    ..Default::default()
                },
                NOW,
            )
            .unwrap()
            .channels
            .remove(0)
        };
        assert!(
            inspect(&s)
                .blocked_by
                .contains(&WorkPatrolChannelBlocker::LiveWork)
        );
        for n in 0..201 {
            s.conn().execute("INSERT INTO channel_work_links(channel_id,object_kind,object_id,source_crew_id) VALUES('patrol-b','requirement',?1,'crew-a')",[format!("#{n}")]).unwrap();
        }
        let fact = inspect(&s);
        assert!(!fact.archive_candidate);
        assert!(
            fact.blocked_by
                .contains(&WorkPatrolChannelBlocker::NonWorkLinks)
        );
        assert!(
            fact.blocked_by
                .contains(&WorkPatrolChannelBlocker::LinksTruncated)
        );
        s.conn()
            .execute(
                "UPDATE channels SET archived_at='2026-10-01' WHERE id='patrol-b'",
                [],
            )
            .unwrap();
        let page = read(
            s.conn(),
            WorkPatrolParams {
                scan: WorkPatrolScan::Channels,
                after: Some("patrol-a".into()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        assert!(page.channels.iter().all(|x| x.id != "patrol-b"));
    }
    #[test]
    fn all_scans_reject_missing_or_corrupt_authority() {
        let s = store();
        s.conn()
            .execute("UPDATE organization_work_book SET state='{}'", [])
            .unwrap();
        for scan in [WorkPatrolScan::Entries, WorkPatrolScan::Channels] {
            assert!(
                read(
                    s.conn(),
                    WorkPatrolParams {
                        scan,
                        ..Default::default()
                    },
                    NOW
                )
                .is_err()
            );
        }
        s.conn()
            .execute("DELETE FROM organization_work_book", [])
            .unwrap();
        for scan in [WorkPatrolScan::Entries, WorkPatrolScan::Channels] {
            assert!(
                read(
                    s.conn(),
                    WorkPatrolParams {
                        scan,
                        ..Default::default()
                    },
                    NOW
                )
                .is_err()
            );
        }
    }
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
