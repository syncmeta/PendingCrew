//! Reassignment accepts only loss facts read from the owning transaction.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Sender, Tx, asks, crews, sessions, wakes, work};
use crewd_proto::{WorkAssignParams, WorkPlan};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_work::{SessionId, Status};
use rusqlite::Connection;
use std::collections::BTreeSet;
fn invalid(s: &str) -> CollabError {
    StoreError::Invalid(s.into()).into()
}
pub fn reassign(tx: &mut Tx<'_>, by: &Sender, p: WorkAssignParams) -> Result<WorkPlan> {
    reassign_as(tx, by, p, None)
}
// This entry is crate-private; its caller must settle a validated durable decision.
pub(crate) fn reassign_trusted(
    tx: &mut Tx<'_>,
    p: WorkAssignParams,
    decision: &str,
) -> Result<WorkPlan> {
    reassign_as(tx, &Sender::System, p, Some(decision))
}
fn reassign_as(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: WorkAssignParams,
    decision: Option<&str>,
) -> Result<WorkPlan> {
    let trusted = decision.is_some();
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, work::number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = if trusted {
        crewd_work::Actor::System
    } else {
        work::actor(tx.conn(), by, &r.crew_id)?
    };
    let target = sessions::get(tx.conn(), &p.owner)?;
    crews::refuse_archived(tx.conn(), &target.crew_id)?;
    if trusted
        && (target.crew_id != r.crew_id
            || target.resume_failed_at.is_some()
            || wakes::automatic_work_paused(tx.conn(), &target.id)?)
    {
        return Err(invalid("unsafe automatic reassignment target"));
    }
    let id = work::core_plan(tx.conn(), &r)?;
    let rev = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|n| p.expected_revision == n.to_string())
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    if p.note.trim().is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let book = asks::load_org(tx.conn())?;
    let item = book
        .item(id)
        .ok_or_else(|| invalid("lost reassignment lacks authority"))?;
    work::check_revision(item, &p.id, rev)?;
    if trusted {
        if let Some(ch) = &item.channel
            && !crate::channels::readable_by(tx.conn(), ch.as_str(), &target.id)?
        {
            return Err(invalid("automatic reassignment target cannot read channel"));
        }
        if let Some(owner) = &item.owner
            && wakes::automatic_work_paused(tx.conn(), owner.as_str())?
        {
            return Err(invalid("automatic reassignment owner stopped"));
        }
    }
    let mut unrecoverable = BTreeSet::new();
    if let Some(owner) = &item.owner {
        let actual = sessions::get(tx.conn(), owner.as_str())?;
        if actual.resume_failed_at.is_some() {
            unrecoverable.insert(owner.clone());
        }
    }
    let owner_paused = match &item.owner {
        Some(o) => wakes::automatic_work_paused(tx.conn(), o.as_str())?,
        None => false,
    };
    if unrecoverable.is_empty() && owner_paused {
        return Err(CollabError::Work(crewd_work::Rejected::NotLost(id)));
    }
    asks::with_book(tx, &r.crew_id, |b, now| {
        b.reassign_lost(
            &actor,
            now,
            id,
            SessionId::new(&p.owner),
            &p.note,
            &unrecoverable,
        )
    })?;
    let plan = work::get(tx.conn(), &p.id)?;
    let note = Assigned {
        decision: decision.map(str::to_string),
        work: p.id.clone(),
        revision: plan.revision.clone(),
        owner: p.owner.clone(),
        stop_floor: wakes::human_stop_floor(tx.conn(), &p.owner)?,
    };
    // A reassignment is an ownership fact, not permission to undo a prior Human stop.
    if !wakes::automatic_work_paused(tx.conn(), &target.id)? {
        wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: p.owner,
                kind: wakes::WakeKind::System,
                source: format!("work-lost:{}:{}", p.id, plan.revision),
                priority: wakes::WakePriority::System,
                not_before_ms: None,
                note: Some(
                    serde_json::to_string(&note)
                        .map_err(|_| invalid("lost notification serialization"))?,
                ),
                origin: None,
            },
        )?;
    }
    work::changed(tx, by, &r.crew_id, &p.id)?;
    Ok(plan)
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Assigned {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision: Option<String>,
    work: String,
    revision: String,
    owner: String,
    stop_floor: i64,
}
pub(crate) fn followup_note(conn: &Connection, row: &wakes::WakeRow) -> Result<work::FollowupNote> {
    let n: Assigned = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("lost notification metadata invalid"))?;
    if n.owner != row.session
        || n.stop_floor < 0
        || row.priority != wakes::WakePriority::System
        || row.source != format!("work-lost:{}:{}", n.work, n.revision)
    {
        return Err(invalid("lost notification identity invalid"));
    }
    if let Some(decision) = &n.decision
        && !crate::work_reassignment::owner_notification_current(conn, row, &n.work, decision)?
    {
        return Ok(work::FollowupNote::Stale);
    }
    let Some(r) = object_refs::get(conn, ObjectKind::Plan, work::number(&n.work)?)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if crews::is_archived(conn, &r.crew_id)?
        || wakes::human_stop_floor(conn, &row.session)? != n.stop_floor
        || row.id <= n.stop_floor
    {
        return Ok(work::FollowupNote::Stale);
    }
    let Some(target) = crewd_store::sessions::get(conn, &row.session)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if target.resume_failed_at.is_some()
        || crews::is_archived(conn, &target.crew_id)?
        || wakes::automatic_work_paused(conn, &target.id)?
    {
        return Ok(work::FollowupNote::Stale);
    }
    let b = asks::load_org(conn)?;
    let id = work::core_plan(conn, &r)?;
    let i = b
        .item(id)
        .ok_or_else(|| invalid("lost notification lacks authority"))?;
    if i.owner.as_ref().is_none_or(|o| o.as_str() != row.session)
        || matches!(i.status, Status::Done | Status::Dropped)
        || i.revision.to_string() != n.revision
    {
        return Ok(work::FollowupNote::Stale);
    }
    Ok(work::FollowupNote::Current(serde_json::json!({"assignedWork":[n.work],"total":1,"partial":false,"reason":"verified_owner_lost"}).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, store};
    fn plan(s: &mut crewd_store::Store) -> WorkPlan {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "verified reassignment".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        })
    }
    fn input(p: &WorkPlan) -> WorkAssignParams {
        WorkAssignParams {
            id: p.id.clone(),
            expected_revision: p.revision.clone(),
            owner: "w-b1".into(),
            note: "actual runtime recovery failure".into(),
        }
    }
    #[test]
    fn healthy_stopped_and_other_session_failure_do_not_authorize_reassignment() {
        let mut s = store();
        let p = plan(&mut s);
        s.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='failure' WHERE id='w-a2'",
                [],
            )
            .unwrap();
        let before = asks::load_org(s.conn()).unwrap().state().clone();
        assert!(matches!(
            err(&mut s, |tx| reassign(tx, &Sender::Human, input(&p))),
            CollabError::Work(crewd_work::Rejected::NotLost(_))
        ));
        assert_eq!(asks::load_org(s.conn()).unwrap().state(), &before);
        assert!(wakes::list(s.conn(), Some("w-b1"), 100).unwrap().is_empty());
    }
    #[test]
    fn explicit_stop_before_first_wake_pauses_due_work_until_actual_resume() {
        let mut s = store();
        let p = plan(&mut s);
        ok(&mut s, |tx| wakes::human_stop(tx, "w-a1"));
        assert_eq!(wakes::human_stop_floor(s.conn(), "w-a1").unwrap(), 0);
        assert!(wakes::automatic_work_paused(s.conn(), "w-a1").unwrap());
        assert_eq!(ok(&mut s, crate::checkpoints::enqueue_due), 0);
        assert_eq!(work::get(s.conn(), &p.id).unwrap(), p);
        ok(&mut s, |tx| {
            sessions::set_desired(tx, "w-a1", crewd_proto::Desired::Running)
        });
        assert!(!wakes::automatic_work_paused(s.conn(), "w-a1").unwrap());
        assert_eq!(work::get(s.conn(), &p.id).unwrap(), p);
    }
    // This is a collab transaction fixture; the independent actor acceptance exercises
    // how the runtime genuinely records this failure, not this SQL seeding shortcut.
    #[test]
    fn transaction_rechecks_current_owner_and_rolls_back_queue_fault() {
        let mut s = store();
        let p = plan(&mut s);
        s.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='failure' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        s.conn().execute_batch("CREATE TRIGGER reject_lost_wake BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'lost wake fault'); END;").unwrap();
        let _ = err(&mut s, |tx| reassign(tx, &Sender::Human, input(&p)));
        assert_eq!(work::get(s.conn(), &p.id).unwrap(), p);
        s.conn()
            .execute_batch("DROP TRIGGER reject_lost_wake;")
            .unwrap();
        let out = ok(&mut s, |tx| reassign(tx, &Sender::Human, input(&p)));
        assert_eq!(out.owner.as_deref(), Some("w-b1"));
        let rows = wakes::list(s.conn(), Some("w-b1"), 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            followup_note(s.conn(), &rows[0]).unwrap(),
            work::FollowupNote::Current(_)
        ));
        ok(&mut s, |tx| wakes::human_stop(tx, "w-b1"));
        assert!(matches!(
            followup_note(s.conn(), &rows[0]).unwrap(),
            work::FollowupNote::Stale
        ));
        assert_eq!(work::get(s.conn(), &p.id).unwrap().owner, out.owner);
        let _ = NOW;
    }
}

/// Typed identity of a particular reassignment notice, independent of later
/// progress revisions. This does not grant a new delivery or execution right.
pub(crate) fn assignment_notification(
    c: &Connection,
    work: &str,
    owner: &str,
    revision: &str,
) -> Result<Option<wakes::WakeRow>> {
    use rusqlite::OptionalExtension;
    let source = format!("work-lost:{work}:{revision}");
    let id = c
        .query_row(
            "SELECT id FROM wakes WHERE session_id=?1 AND kind='system' AND source=?2",
            rusqlite::params![owner, source],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    let Some(id) = id else { return Ok(None) };
    let row = wakes::by_ids(c, &[id])?
        .pop()
        .ok_or_else(|| invalid("missing reassignment notification"))?;
    let note: Assigned = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("invalid reassignment notice"))?;
    if note.work != work
        || note.owner != owner
        || note.revision != revision
        || note.stop_floor < 0
        || row.id <= note.stop_floor
        || row.session != owner
        || row.kind != wakes::WakeKind::System
        || row.priority != wakes::WakePriority::System
        || row.source != source
    {
        return Err(invalid("reassignment notice identity mismatch"));
    }
    Ok(Some(row))
}
