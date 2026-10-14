//! Durable due-work reminders. Enqueue is not a delivered checkpoint.
use crate::{Change, Result, Tx, asks, wakes, work};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_work::{Actor, SessionId, Ts, WorkId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    id: String,
    revision: String,
    next_check_at: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Note {
    due_work: Vec<String>,
    total: usize,
    partial: bool,
    checkpoint_targets: Vec<Target>,
}
fn invalid(s: &str) -> crate::CollabError {
    StoreError::Invalid(s.into()).into()
}
fn saved(row: &wakes::WakeRow) -> Result<Option<Note>> {
    if row.kind != wakes::WakeKind::Checkpoint || !row.source.starts_with("work-check:") {
        return Ok(None);
    }
    let n: Note = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("checkpoint target metadata is invalid"))?;
    if n.total != n.checkpoint_targets.len()
        || n.partial != (n.total > 50)
        || n.due_work
            != n.checkpoint_targets
                .iter()
                .take(50)
                .map(|t| t.id.clone())
                .collect::<Vec<_>>()
    {
        return Err(invalid("checkpoint target metadata is inconsistent"));
    }
    Ok(Some(n))
}
struct Resolved {
    id: String,
    crew: String,
    local: WorkId,
}
fn current(
    conn: &Connection,
    row: &wakes::WakeRow,
    n: &Note,
    now: i64,
    acknowledged: bool,
) -> Result<Vec<Resolved>> {
    if wakes::automatic_work_paused(conn, &row.session)? {
        return Ok(Vec::new());
    }
    let book = asks::load_org(conn)?;
    let due: BTreeSet<WorkId> = book
        .due_checkpoints(Ts(now))
        .into_iter()
        .filter(|d| d.owner.as_str() == row.session)
        .flat_map(|d| d.items)
        .collect();
    // Only a never-dispatched durable fact can refresh its nonsemantic revision.
    // Acknowledgement uses the revision captured in the claim transaction.
    let refreshable = !acknowledged
        && row.state == wakes::WakeState::Pending
        && row.attempts == 0
        && crewd_store::work_followups::available(conn)?;
    let mut out = Vec::new();
    for t in &n.checkpoint_targets {
        let Some(r) = object_refs::get(conn, ObjectKind::Plan, work::number(&t.id)?)? else {
            continue;
        };
        let frozen = crate::crews::is_archived(conn, &r.crew_id)?;
        let local = work::core_plan(conn, &r)?;
        let i = book
            .item(local)
            .ok_or_else(|| invalid("checkpoint target has no authoritative item"))?;
        if !frozen
            && due.contains(&local)
            && (refreshable || i.revision.to_string() == t.revision)
            && i.next_check_at == Some(Ts(t.next_check_at))
            && crate::work_followups::checkpoint_subject_admitted(
                conn,
                row,
                &t.id,
                t.next_check_at,
                now,
                acknowledged,
            )?
        {
            out.push(Resolved {
                id: t.id.clone(),
                crew: r.crew_id,
                local,
            });
        }
    }
    Ok(out)
}
/// Capture metadata for the same semantic fact and wake before its first claim.
/// No new input is created, and the rendered reminder body remains unchanged.
pub(crate) fn capture_for_claim(tx: &mut Tx<'_>, row: &wakes::WakeRow) -> Result<()> {
    if row.state != wakes::WakeState::Pending
        || row.attempts != 0
        || !crewd_store::work_followups::available(tx.conn())?
    {
        return Ok(());
    }
    let Some(mut note) = saved(row)? else {
        return Ok(());
    };
    let valid = current(tx.conn(), row, &note, tx.now_ms(), false)?;
    let book = asks::load_org(tx.conn())?;
    for target in &mut note.checkpoint_targets {
        if let Some(resolved) = valid.iter().find(|r| r.id == target.id) {
            target.revision = book
                .item(resolved.local)
                .ok_or_else(|| invalid("claim checkpoint item missing"))?
                .revision
                .to_string();
        }
    }
    let encoded =
        serde_json::to_string(&note).map_err(|_| invalid("claim checkpoint serialization"))?;
    let changed = tx.conn().execute(
        "UPDATE wakes SET note=?2 WHERE id=?1 AND state='pending' AND attempts=0 AND note=?3",
        params![row.id, encoded, row.note],
    )?;
    if changed != 1 {
        return Err(invalid("checkpoint changed during claim"));
    }
    Ok(())
}
pub(crate) fn render_note(
    conn: &Connection,
    row: &wakes::WakeRow,
    now: i64,
) -> Result<work::FollowupNote> {
    let Some(n) = saved(row)? else {
        return Ok(work::FollowupNote::Ordinary);
    };
    if !crate::work_followups::checkpoint_admitted(conn, row, now)? {
        return Ok(work::FollowupNote::Stale);
    }
    let targets = current(conn, row, &n, now, false)?;
    if targets.is_empty() {
        return Ok(work::FollowupNote::Stale);
    }
    let total = targets.len();
    Ok(work::FollowupNote::Current(serde_json::json!({"dueWork":targets.iter().take(50).map(|t|&t.id).collect::<Vec<_>>(),"total":total,"partial":total>50}).to_string()))
}
#[cfg(test)]
fn candidate_crews(conn: &Connection, now: i64) -> Result<Vec<String>> {
    let book = asks::load_org(conn)?;
    let mut crews = BTreeSet::new();
    for due in book.due_checkpoints(Ts(now)) {
        for id in due.items {
            let crew = book
                .state()
                .item_crew(id)
                .ok_or_else(|| invalid("due item lacks crew attribution"))?
                .as_str();
            if !crate::crews::is_archived(conn, crew)? {
                crews.insert(crew.to_string());
            }
        }
    }
    Ok(crews.into_iter().collect())
}
/// One merged due reminder per owner across crews. Existing queued work never
/// advances missedChecks; duplicate settled manifests do not restart fail loops.
pub fn enqueue_due(tx: &mut Tx<'_>) -> Result<usize> {
    crate::work_followups::baseline(tx)?;
    let book = asks::load_org(tx.conn())?;
    let mut owners: BTreeMap<String, Vec<Target>> = BTreeMap::new();
    for due in book.due_checkpoints(Ts(tx.now_ms())) {
        for local in due.items {
            let crew = book
                .state()
                .item_crew(local)
                .ok_or_else(|| invalid("due work lacks crew attribution"))?;
            if crate::crews::is_archived(tx.conn(), crew.as_str())? {
                continue;
            }
            let i = book.item(local).expect("due item exists");
            owners
                .entry(due.owner.to_string())
                .or_default()
                .push(Target {
                    id: work::reference(tx.conn(), ObjectKind::Plan, crew.as_str(), local)?,
                    revision: i.revision.to_string(),
                    next_check_at: i.next_check_at.expect("due has a check").0,
                });
        }
    }
    let mut count = 0;
    for (owner, mut targets) in owners {
        if wakes::automatic_work_paused(tx.conn(), &owner)?
            || crewd_store::sessions::get(tx.conn(), &owner)?.is_none()
        {
            continue;
        }
        let active:bool=tx.conn().query_row("SELECT EXISTS(SELECT 1 FROM wakes WHERE session_id=?1 AND kind='checkpoint' AND source LIKE 'work-check:%' AND state IN ('pending','inflight'))",[&owner],|r|r.get(0))?;
        if active {
            continue;
        }
        targets.sort_by(|a, b| a.id.cmp(&b.id));
        if crewd_store::work_followups::available(tx.conn())? {
            let mut fresh = Vec::new();
            for target in targets {
                if fresh.len() < 50
                    && crate::work_followups::checkpoint_new(
                        tx,
                        &target.id,
                        &owner,
                        target.next_check_at,
                    )?
                {
                    fresh.push(target);
                }
            }
            targets = fresh;
        }
        if targets.is_empty() {
            continue;
        }
        let attributed: Vec<(String, i64)> = targets
            .iter()
            .map(|t| (t.id.clone(), t.next_check_at))
            .collect();
        let note = serde_json::to_string(&Note {
            due_work: targets.iter().take(50).map(|t| t.id.clone()).collect(),
            total: targets.len(),
            partial: targets.len() > 50,
            checkpoint_targets: targets,
        })
        .map_err(|_| invalid("checkpoint metadata serialization failed"))?;
        let existing:Option<i64>=tx.conn().query_row("SELECT id FROM wakes WHERE session_id=?1 AND kind='checkpoint' AND source LIKE 'work-check:%' AND note=?2 LIMIT 1",params![owner,note],|r|r.get(0)).optional()?;
        if existing.is_some() {
            continue;
        }
        let source = if crewd_store::work_followups::available(tx.conn())? {
            crate::work_followups::checkpoint_source(&attributed, &owner)
        } else {
            format!("work-check:{}", uuid::Uuid::new_v4())
        };
        if let wakes::Enqueued::New(id) = wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: owner.clone(),
                kind: wakes::WakeKind::Checkpoint,
                source,
                priority: wakes::WakePriority::System,
                not_before_ms: None,
                note: Some(note),
                origin: None,
            },
        )? {
            crate::work_followups::checkpoint_link(tx, &attributed, &owner, id)?;
            count += 1;
        }
    }
    Ok(count)
}
/// Called only inside the actual successful-delivery transaction. A report or
/// ownership change after preparation supersedes the old reminder's accounting.
pub(crate) fn delivered(tx: &mut Tx<'_>, before: &[wakes::WakeRow]) -> Result<()> {
    for row in before {
        if row.state == wakes::WakeState::Delivered {
            continue;
        }
        let Some(n) = saved(row)? else {
            continue;
        };
        let delivered = wakes::by_ids(tx.conn(), &[row.id])?
            .into_iter()
            .any(|r| r.state == wakes::WakeState::Delivered);
        if !delivered {
            continue;
        }
        let targets = current(tx.conn(), row, &n, tx.now_ms(), true)?;
        if let Some(first) = targets.first() {
            let ids: Vec<_> = targets.iter().map(|t| t.local).collect();
            asks::with_book(tx, &first.crew, |b, now| {
                b.record_checkpoint_wake(&Actor::System, now, &SessionId::new(&row.session), &ids)
            })?;
            for t in targets {
                tx.note(Change::WorkChanged {
                    crew_id: t.crew,
                    id: t.id,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, ok, ok_at, store};
    use crate::{Sender, cursor_reservations as cr};
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    fn active(s: &mut crewd_store::Store, crew: &str) -> crewd_proto::WorkPlan {
        let p = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: "synthetic checkpoint work".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        ok(s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + MIN)),
                    note: Some("next synthetic step".into()),
                    ..Default::default()
                },
            )
        })
    }
    fn rows(s: &crewd_store::Store) -> Vec<wakes::WakeRow> {
        wakes::list(s.conn(), Some("w-a1"), 100).unwrap()
    }
    fn missed(s: &crewd_store::Store, crew: &str) -> u32 {
        let book = asks::load(s.conn(), crew).unwrap();
        book.state()
            .items
            .iter()
            .filter(|(id, _)| {
                book.state()
                    .item_crew(**id)
                    .is_some_and(|c| c.as_str() == crew)
            })
            .map(|(_, i)| i.missed_checks)
            .sum()
    }
    fn lease(s: &mut crewd_store::Store, row: &wakes::WakeRow, now: i64) -> cr::Lease {
        let mut snap = cr::snapshot(s.conn(), "w-a1", &[], now).unwrap();
        snap.capture_work_followups(s.conn(), std::slice::from_ref(row), now)
            .unwrap();
        match ok_at(s, now, |tx| {
            cr::claim_reserved(tx, &snap, &[], &[row.id], &[], cr::Owner::new(1).unwrap())
        })
        .0
        {
            cr::Decision::Applied(l) => l,
            other => panic!("unexpected {other:?}"),
        }
    }
    #[test]
    fn cross_crew_due_items_merge_and_queue_does_not_count_as_delivery() {
        let mut s = store();
        active(&mut s, "crew-a");
        active(&mut s, "crew-b");
        assert_eq!(ok(&mut s, enqueue_due), 0);
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 1);
        let r = rows(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].priority, wakes::WakePriority::System);
        assert_eq!(missed(&s, "crew-a"), 0);
        assert_eq!(missed(&s, "crew-b"), 0);
        assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue_due).0, 0);
        let text =
            crate::render::build_batch(s.conn(), "w-a1", crate::Locale::En, &r, NOW + MIN).unwrap();
        assert_eq!(text.items.len(), 1);
        assert!(
            !text.items[0]
                .note
                .as_ref()
                .unwrap()
                .contains("checkpointTargets")
        );
        let l = lease(&mut s, &r[0], NOW + MIN);
        assert_eq!(missed(&s, "crew-a"), 0);
        assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue_due).0, 0);
        assert!(matches!(
            ok_at(&mut s, NOW + MIN, |tx| cr::ack_reserved_delivered(
                tx,
                &l,
                wakes::Riders::Release
            ))
            .0,
            cr::Decision::Applied(())
        ));
        assert_eq!(missed(&s, "crew-a"), 1);
        assert_eq!(missed(&s, "crew-b"), 1);
        assert!(matches!(
            ok_at(&mut s, NOW + MIN, |tx| cr::ack_reserved_delivered(
                tx,
                &l,
                wakes::Riders::Release
            ))
            .0,
            cr::Decision::Stale
        ));
        assert_eq!(missed(&s, "crew-a"), 1);
        assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue_due).0, 0);
    }
    #[test]
    fn failed_or_dropped_delivery_does_not_increment_or_recreate_manifest() {
        let mut s = store();
        active(&mut s, "crew-a");
        ok_at(&mut s, NOW + MIN, enqueue_due);
        let r = rows(&s);
        let l = lease(&mut s, &r[0], NOW + MIN);
        ok_at(&mut s, NOW + MIN, |tx| {
            cr::ack_reserved_failed(tx, &l, "synthetic failure", |_| Some(NOW + 2 * MIN))
        });
        assert_eq!(missed(&s, "crew-a"), 0);
        assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue_due).0, 0);
        ok_at(&mut s, NOW + 2 * MIN, |tx| {
            wakes::mark_dropped(tx, r[0].id, "synthetic terminal failure")
        });
        assert_eq!(ok_at(&mut s, NOW + 3 * MIN, enqueue_due).0, 0);
        assert_eq!(rows(&s).len(), 1);
    }
    fn note_only(s: &mut crewd_store::Store, id: &str, at: i64) {
        let p = work::get(s.conn(), id).unwrap();
        ok_at(s, at, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    note: Some("new progress without changing the checkpoint".into()),
                    ..Default::default()
                },
            )
        });
    }
    #[test]
    fn undispatched_rider_refund_preserves_fact_for_its_only_actual_input() {
        let mut s = store();
        active(&mut s, "crew-a");
        ok_at(&mut s, NOW + MIN, enqueue_due);
        let original = rows(&s)[0].clone();
        ok_at(&mut s, NOW + MIN, |tx| wakes::claim(tx, &[original.id]));
        ok_at(&mut s, NOW + MIN, |tx| {
            wakes::ack_delivered(tx, "w-a1", &[], wakes::Riders::Release, &[original.id], &[])
        });
        let refunded = rows(&s)[0].clone();
        assert_eq!(refunded.state, wakes::WakeState::Pending);
        assert_eq!(refunded.attempts, 0);
        assert!(matches!(
            render_note(s.conn(), &refunded, NOW + MIN).unwrap(),
            work::FollowupNote::Current(_)
        ));
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 0);
        let l = lease(&mut s, &refunded, NOW + MIN);
        ok_at(&mut s, NOW + MIN, |tx| {
            cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
        });
        assert_eq!(missed(&s, "crew-a"), 1);
        assert_eq!(rows(&s).len(), 1);
    }
    #[test]
    fn semantic_deadline_stop_and_archive_never_refresh_old_fact() {
        for mode in ["deadline", "stop", "archive"] {
            let mut s = store();
            active(&mut s, "crew-a");
            ok_at(&mut s, NOW + MIN, enqueue_due);
            let original = rows(&s)[0].clone();
            let id = saved(&original).unwrap().unwrap().checkpoint_targets[0]
                .id
                .clone();
            match mode {
                "deadline" => {
                    let p = work::get(s.conn(), &id).unwrap();
                    ok_at(&mut s, NOW + MIN, |tx| {
                        work::update(
                            tx,
                            &Sender::Session("w-a1".into()),
                            WorkUpdateParams {
                                id: p.id,
                                expected_revision: p.revision,
                                next_check_at: Some(crewd_store::ts_from_ms(NOW + 5 * MIN)),
                                note: Some("explicit future checkpoint report".into()),
                                ..Default::default()
                            },
                        )
                    });
                }
                "stop" => {
                    ok_at(&mut s, NOW + MIN, |tx| wakes::human_stop(tx, "w-a1"));
                }
                "archive" => {
                    ok_at(&mut s, NOW + MIN, |tx| crate::crews::archive(tx, "crew-b"));
                    ok_at(&mut s, NOW + MIN, |tx| crate::crews::archive(tx, "crew-a"));
                }
                _ => unreachable!(),
            }
            let actual = rows(&s)[0].clone();
            assert!(
                matches!(
                    render_note(s.conn(), &actual, NOW + MIN).unwrap(),
                    work::FollowupNote::Stale
                ),
                "{mode}"
            );
            ok_at(&mut s, NOW + MIN, |tx| capture_for_claim(tx, &actual));
            assert_eq!(rows(&s)[0].note, original.note, "{mode}");
            assert_eq!(missed(&s, "crew-a"), 0);
        }
    }
    #[test]
    fn queued_note_change_keeps_same_fact_and_captures_revision_once() {
        let mut s = store();
        active(&mut s, "crew-a");
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 1);
        let before = rows(&s)[0].clone();
        let id = saved(&before).unwrap().unwrap().checkpoint_targets[0]
            .id
            .clone();
        note_only(&mut s, &id, NOW + MIN);
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 0);
        assert!(matches!(
            render_note(s.conn(), &before, NOW + MIN).unwrap(),
            work::FollowupNote::Current(_)
        ));
        let l = lease(&mut s, &before, NOW + MIN);
        let claimed = rows(&s)[0].clone();
        assert_eq!(claimed.id, before.id);
        assert_eq!(claimed.source, before.source);
        assert_ne!(claimed.note, before.note);
        assert_eq!(
            saved(&claimed).unwrap().unwrap().checkpoint_targets[0].revision,
            work::get(s.conn(), &id).unwrap().revision
        );
        ok_at(&mut s, NOW + MIN, |tx| {
            cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
        });
        assert_eq!(missed(&s, "crew-a"), 1);
        assert_eq!(rows(&s).len(), 1);
    }
    #[test]
    fn postclaim_note_change_delivers_without_old_miss_and_unknown_cannot_refresh() {
        for unknown in [false, true] {
            let mut s = store();
            active(&mut s, "crew-a");
            ok_at(&mut s, NOW + MIN, enqueue_due);
            let row = rows(&s)[0].clone();
            let id = saved(&row).unwrap().unwrap().checkpoint_targets[0]
                .id
                .clone();
            let l = lease(&mut s, &row, NOW + MIN);
            let claimed = rows(&s)[0].clone();
            note_only(&mut s, &id, NOW + MIN);
            if unknown {
                ok_at(&mut s, NOW + MIN, |tx| {
                    cr::ack_reserved_failed(tx, &l, "unknown physical outcome", |_| {
                        Some(NOW + 2 * MIN)
                    })
                });
                let pending = rows(&s)[0].clone();
                assert!(matches!(
                    render_note(s.conn(), &pending, NOW + 2 * MIN).unwrap(),
                    work::FollowupNote::Stale
                ));
                ok_at(&mut s, NOW + 2 * MIN, |tx| capture_for_claim(tx, &pending));
                assert_eq!(rows(&s)[0].note, claimed.note);
                assert_eq!(ok_at(&mut s, NOW + 2 * MIN, enqueue_due).0, 0);
            } else {
                ok_at(&mut s, NOW + MIN, |tx| {
                    cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
                });
                assert_eq!(rows(&s)[0].state, wakes::WakeState::Delivered);
            }
            assert_eq!(missed(&s, "crew-a"), 0);
        }
    }
    #[test]
    fn new_report_before_ack_supersedes_checkpoint_accounting() {
        let mut s = store();
        active(&mut s, "crew-a");
        ok_at(&mut s, NOW + MIN, enqueue_due);
        let r = rows(&s);
        let l = lease(&mut s, &r[0], NOW + MIN);
        let id = saved(&r[0]).unwrap().unwrap().checkpoint_targets[0]
            .id
            .clone();
        let p = work::get(s.conn(), &id).unwrap();
        ok_at(&mut s, NOW + MIN, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id,
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 5 * MIN)),
                    note: Some("already made progress".into()),
                    ..Default::default()
                },
            )
        });
        ok_at(&mut s, NOW + MIN, |tx| {
            cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
        });
        assert_eq!(missed(&s, "crew-a"), 0);
        assert_eq!(
            work::get(s.conn(), &id).unwrap().next_check_at,
            Some(crewd_store::ts_from_ms(NOW + 5 * MIN))
        );
    }
    #[test]
    fn bookkeeping_failure_preserves_original_receipt_for_retry() {
        let mut s = store();
        active(&mut s, "crew-a");
        ok_at(&mut s, NOW + MIN, enqueue_due);
        let r = rows(&s);
        let l = lease(&mut s, &r[0], NOW + MIN);
        s.conn().execute_batch("CREATE TRIGGER reject_checkpoint_history BEFORE INSERT ON organization_work_events BEGIN SELECT RAISE(ABORT,'synthetic checkpoint history fault'); END;").unwrap();
        assert!(
            crate::run(&mut s, NOW + MIN, |tx| cr::ack_reserved_delivered(
                tx,
                &l,
                wakes::Riders::Release
            ))
            .is_err()
        );
        assert_eq!(rows(&s)[0].state, wakes::WakeState::Inflight);
        assert_eq!(missed(&s, "crew-a"), 0);
        s.conn()
            .execute_batch("DROP TRIGGER reject_checkpoint_history")
            .unwrap();
        ok_at(&mut s, NOW + MIN, |tx| {
            cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
        });
        assert_eq!(rows(&s)[0].state, wakes::WakeState::Delivered);
        assert_eq!(missed(&s, "crew-a"), 1);
    }
    #[test]
    fn two_delivered_receipts_stop_further_checks_until_owner_reports() {
        let mut s = store();
        active(&mut s, "crew-a");
        for (n, at) in [(1, NOW + MIN), (2, NOW + 16 * MIN)] {
            assert_eq!(ok_at(&mut s, at, enqueue_due).0, 1);
            let row = rows(&s)
                .into_iter()
                .find(|r| r.state == wakes::WakeState::Pending)
                .unwrap();
            let l = lease(&mut s, &row, at);
            ok_at(&mut s, at, |tx| {
                cr::ack_reserved_delivered(tx, &l, wakes::Riders::Release)
            });
            assert_eq!(missed(&s, "crew-a"), n);
        }
        assert!(
            candidate_crews(s.conn(), NOW + 31 * MIN)
                .unwrap()
                .is_empty(),
            "exhausted work must not restore its book every tick"
        );
        assert_eq!(ok_at(&mut s, NOW + 31 * MIN, enqueue_due).0, 0);
        assert_eq!(rows(&s).len(), 2);
    }
    #[test]
    fn empty_scan_and_queue_fault_never_invent_a_work_book_or_miss() {
        let mut s = store();
        assert_eq!(ok(&mut s, enqueue_due), 0);
        assert_eq!(
            crate::testkit::count(&s, "SELECT COUNT(*) FROM work_books"),
            0
        );
        active(&mut s, "crew-a");
        s.conn().execute_batch("CREATE TRIGGER reject_checkpoint_queue BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'synthetic checkpoint queue fault'); END;").unwrap();
        assert!(crate::run(&mut s, NOW + MIN, enqueue_due).is_err());
        assert!(rows(&s).is_empty());
        assert_eq!(missed(&s, "crew-a"), 0);
        s.conn()
            .execute_batch("DROP TRIGGER reject_checkpoint_queue")
            .unwrap();
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 1);
    }
    #[test]
    fn missing_authoritative_book_or_item_is_not_an_empty_due_scan() {
        for mode in ["book", "item"] {
            let mut s = store();
            active(&mut s, "crew-a");
            match mode {
                "book" => {
                    s.conn()
                        .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
                        .unwrap();
                }
                "item" => {
                    s.conn().execute("UPDATE organization_work_book SET state=json_remove(state,'$.items.W1') WHERE singleton=1",[]).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                crate::run(&mut s, NOW + MIN, enqueue_due).is_err(),
                "{mode}: missing issued authority was treated as empty"
            );
            assert!(rows(&s).is_empty());
        }
    }
    #[test]
    fn scan_preserves_book_clock_and_skips_blocked_and_unowned_work() {
        let mut s = store();
        active(&mut s, "crew-a");
        ok_at(&mut s, NOW + 2 * MIN, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "synthetic clock advance".into(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(candidate_crews(s.conn(), NOW).unwrap(), vec!["crew-a"]);
        assert_eq!(
            ok(&mut s, enqueue_due),
            1,
            "book clock determines due even when caller clock moved back"
        );
        let mut s = store();
        active(&mut s, "crew-a");
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.block(
                    &Actor::worker("w-a1"),
                    now,
                    "W1".parse().unwrap(),
                    vec![crewd_work::BlockOn::NewAsk {
                        question: "synthetic blocker".into(),
                        default: None,
                    }],
                    crewd_work::Continuation::Nothing,
                )
            })
        });
        assert!(candidate_crews(s.conn(), NOW + MIN).unwrap().is_empty());
        ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "unowned work".into(),
                    ..Default::default()
                },
            )
        });
        assert!(candidate_crews(s.conn(), NOW + MIN).unwrap().is_empty());
        assert_eq!(ok_at(&mut s, NOW + MIN, enqueue_due).0, 0);
    }
}
