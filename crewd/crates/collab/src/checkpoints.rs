//! Durable due-work reminders. Enqueue is not a delivered checkpoint.
use crate::{Change, Result, Tx, asks, wakes, work};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_work::{Actor, SessionId, Ts, WorkBook, WorkId};
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
fn current(conn: &Connection, row: &wakes::WakeRow, n: &Note, now: i64) -> Result<Vec<Resolved>> {
    let mut books: BTreeMap<String, (WorkBook, BTreeSet<WorkId>, bool)> = BTreeMap::new();
    let mut out = Vec::new();
    for t in &n.checkpoint_targets {
        let Some(r) = object_refs::get(conn, ObjectKind::Plan, work::number(&t.id)?)? else {
            continue;
        };
        if !books.contains_key(&r.crew_id) {
            let book = asks::load(conn, &r.crew_id)?;
            let due = book
                .due_checkpoints(Ts(now))
                .into_iter()
                .filter(|d| d.owner.as_str() == row.session)
                .flat_map(|d| d.items)
                .collect();
            let frozen = crate::crews::is_archived(conn, &r.crew_id)?;
            books.insert(r.crew_id.clone(), (book, due, frozen));
        }
        let (book, due, frozen) = &books[&r.crew_id];
        let local = r
            .source_id
            .strip_prefix("work:")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| invalid("checkpoint source identity is invalid"))?;
        let i = book
            .item(local)
            .ok_or_else(|| invalid("checkpoint target has no authoritative item"))?;
        if !frozen
            && due.contains(&local)
            && i.revision.to_string() == t.revision
            && i.next_check_at == Some(Ts(t.next_check_at))
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
pub(crate) fn render_note(
    conn: &Connection,
    row: &wakes::WakeRow,
    now: i64,
) -> Result<work::FollowupNote> {
    let Some(n) = saved(row)? else {
        return Ok(work::FollowupNote::Ordinary);
    };
    let targets = current(conn, row, &n, now)?;
    if targets.is_empty() {
        return Ok(work::FollowupNote::Stale);
    }
    let total = targets.len();
    Ok(work::FollowupNote::Current(serde_json::json!({"dueWork":targets.iter().take(50).map(|t|&t.id).collect::<Vec<_>>(),"total":total,"partial":total>50}).to_string()))
}
/// One merged due reminder per owner across crews. Existing queued work never
/// advances missedChecks; duplicate settled manifests do not restart fail loops.
pub fn enqueue_due(tx: &mut Tx<'_>) -> Result<usize> {
    let crews = {
        let mut q=tx.conn().prepare("SELECT b.crew_id FROM work_books b JOIN crews c ON c.id=b.crew_id WHERE c.archived_at IS NULL AND EXISTS(SELECT 1 FROM json_each(b.state,'$.items') i WHERE json_extract(i.value,'$.next_check_at') <= max(?1,json_extract(b.state,'$.clock'))) ORDER BY b.crew_id")?;
        q.query_map([tx.now_ms()], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut owners: BTreeMap<String, Vec<Target>> = BTreeMap::new();
    for crew in crews {
        let book = asks::load(tx.conn(), &crew)?;
        for due in book.due_checkpoints(Ts(tx.now_ms())) {
            for local in due.items {
                let i = book.item(local).expect("due item exists");
                owners
                    .entry(due.owner.to_string())
                    .or_default()
                    .push(Target {
                        id: work::reference(tx.conn(), ObjectKind::Plan, &crew, local)?,
                        revision: i.revision.to_string(),
                        next_check_at: i.next_check_at.expect("due has a check").0,
                    });
            }
        }
    }
    let mut count = 0;
    for (owner, mut targets) in owners {
        if crewd_store::sessions::get(tx.conn(), &owner)?.is_none() {
            continue;
        }
        let active:bool=tx.conn().query_row("SELECT EXISTS(SELECT 1 FROM wakes WHERE session_id=?1 AND kind='checkpoint' AND source LIKE 'work-check:%' AND state IN ('pending','inflight'))",[&owner],|r|r.get(0))?;
        if active {
            continue;
        }
        targets.sort_by(|a, b| a.id.cmp(&b.id));
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
        if matches!(
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: owner,
                    kind: wakes::WakeKind::Checkpoint,
                    source: format!("work-check:{}", uuid::Uuid::new_v4()),
                    priority: wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some(note),
                    origin: None
                }
            )?,
            wakes::Enqueued::New(_)
        ) {
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
        let mut crews: BTreeMap<String, Vec<Resolved>> = BTreeMap::new();
        for t in current(tx.conn(), row, &n, tx.now_ms())? {
            crews.entry(t.crew.clone()).or_default().push(t);
        }
        for (crew, targets) in crews {
            let ids: Vec<_> = targets.iter().map(|t| t.local).collect();
            asks::with_book(tx, &crew, |b, now| {
                b.record_checkpoint_wake(&Actor::System, now, &SessionId::new(&row.session), &ids)
            })?;
            for t in targets {
                tx.note(Change::WorkChanged {
                    crew_id: crew.clone(),
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
        asks::load(s.conn(), crew)
            .unwrap()
            .state()
            .items
            .values()
            .map(|i| i.missed_checks)
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
        s.conn().execute_batch("CREATE TRIGGER reject_checkpoint_history BEFORE INSERT ON work_events BEGIN SELECT RAISE(ABORT,'synthetic checkpoint history fault'); END;").unwrap();
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
}
