//! Verified loss selection; effect and both notifications commit atomically.
use crate::{Result, Tx, judgments, wakes, work};
use crewd_proto::{ReassignmentPhase, ReassignmentReason, ReassignmentView};
use crewd_store::{StoreError, meta_get, meta_set, object_refs::ObjectKind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub crew: String,
    pub owner: String,
    pub captain: String,
    pub title: String,
    pub revision: String,
    pub loss: String,
    pub candidates: Vec<judgments::Recipient>,
    pub fingerprint: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    snapshot: Snapshot,
    view: ReassignmentView,
    post: Option<String>,
}
fn key(id: &str) -> String {
    format!("work.reassignment.v1:{id}")
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid reassignment authority".into()).into()
}
fn record(c: &Connection, id: &str) -> Result<Option<Record>> {
    let r = meta_get(c, &key(id))?
        .map(|s| serde_json::from_str::<Record>(&s))
        .transpose()
        .map_err(StoreError::from)?;
    if r.as_ref()
        .is_some_and(|r| r.version != 1 || r.snapshot.id != id)
    {
        return Err(invalid());
    }
    Ok(r)
}
fn save(tx: &mut Tx<'_>, r: &Record) -> Result<()> {
    meta_set(
        tx.conn(),
        &key(&r.snapshot.id),
        &serde_json::to_string(r).map_err(StoreError::from)?,
    )?;
    Ok(())
}
fn snapshot(c: &Connection, id: &str, now: i64) -> Result<Option<Snapshot>> {
    let public = work::get(c, id)?;
    if public.frozen || matches!(public.status.as_str(), "done" | "dropped") {
        return Ok(None);
    }
    let Some(owner) = public.owner.clone() else {
        return Ok(None);
    };
    let Some(captain) = crate::crews::captain_of(c, &public.crew_id)? else {
        return Ok(None);
    };
    let cap = crate::sessions::get(c, &captain)?;
    if cap.crew_id != public.crew_id || cap.role != crewd_proto::Role::Captain {
        return Ok(None);
    }
    let reference = crewd_store::object_refs::get(c, ObjectKind::Plan, work::number(id)?)?
        .ok_or_else(invalid)?;
    let core = work::core_plan(c, &reference)?;
    let book = crate::asks::load_org(c)?;
    let item = book.item(core).ok_or_else(invalid)?;
    let owner_session = crate::sessions::get(c, &owner)?;
    let Some(loss) = book
        .patrol_item(
            core,
            crewd_work::Ts(now),
            owner_session.resume_failed_at.is_some(),
        )
        .ok_or_else(invalid)?
        .lost
    else {
        return Ok(None);
    };
    let loss = match loss {
        crewd_work::LostCause::Unrecoverable => "unrecoverable",
        crewd_work::LostCause::Silent { .. } => "silent",
    }
    .to_string();
    let channel = item.channel.as_ref().map(|c| c.as_str());
    if let Some(ch) = channel
        && !crate::channels::readable_by(c, ch, &captain)?
    {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for mut r in judgments::recipients(c, &public.crew_id, channel)? {
        if r.id == owner || wakes::automatic_work_paused(c, &r.id)? {
            continue;
        }
        r.name = r.name.chars().take(120).collect();
        candidates.push(r);
        // Matches the provider choice protocol MAX_MEMBERS; collab has no provider dependency.
        if candidates.len() == 15 {
            break;
        }
    }
    let fingerprint = judgments::fingerprint(&(
        id,
        &public.crew_id,
        &public.revision,
        &owner,
        &captain,
        &loss,
        &owner_session.resume_failed_at,
        item.missed_checks,
        item.next_check_at,
        &candidates,
        context(c, &owner, &captain)?,
    ))?;
    Ok(Some(Snapshot {
        id: id.into(),
        crew: public.crew_id,
        owner,
        captain,
        title: public.title.chars().take(200).collect(),
        revision: public.revision,
        loss,
        candidates,
        fingerprint,
    }))
}
fn context(c: &Connection, owner: &str, captain: &str) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
     "config":judgments::configuration(c)?,"epoch":crewd_store::judgment_state::epoch(c)?,
     "ownerFloor":wakes::human_stop_floor(c,owner)?,"ownerStop":judgments::stop_epoch(c,owner)?,
     "captainFloor":wakes::human_stop_floor(c,captain)?,"captainStop":judgments::stop_epoch(c,captain)?,
     "captainRole":crate::sessions::get(c,captain)?.role
    }))
}
fn paused(c: &Connection, s: &Snapshot) -> Result<bool> {
    Ok(wakes::automatic_work_paused(c, &s.owner)? || wakes::automatic_work_paused(c, &s.captain)?)
}
pub fn prepare(tx: &mut Tx<'_>, id: &str) -> Result<Option<Snapshot>> {
    let Some(s) = snapshot(tx.conn(), id, tx.now_ms())? else {
        return Ok(None);
    };
    if let Some(r) = record(tx.conn(), id)?
        && r.snapshot.fingerprint == s.fingerprint
    {
        return Ok(matches!(r.view.phase, ReassignmentPhase::Prepared).then_some(s));
    }
    let empty = s.candidates.is_empty();
    let r = Record {
        version: 1,
        snapshot: s.clone(),
        post: None,
        view: ReassignmentView {
            decision_id: uuid::Uuid::new_v4().to_string(),
            phase: if empty {
                ReassignmentPhase::Settled
            } else {
                ReassignmentPhase::Prepared
            },
            reason: if empty {
                ReassignmentReason::NoCandidates
            } else {
                ReassignmentReason::Prepared
            },
            old_owner: s.owner.clone(),
            new_owner: None,
            adopted: false,
            confidence: None,
            owner_wake_id: None,
            owner_wake_state: None,
            captain_wake_id: None,
            captain_wake_state: None,
        },
    };
    save(tx, &r)?;
    Ok((!empty).then_some(s))
}
pub fn dispatch(tx: &mut Tx<'_>, s: &Snapshot) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), &s.id)? else {
        return Ok(false);
    };
    if r.snapshot.fingerprint != s.fingerprint
        || !matches!(r.view.phase, ReassignmentPhase::Prepared)
    {
        return Ok(false);
    }
    if snapshot(tx.conn(), &s.id, tx.now_ms())?.is_none_or(|n| n.fingerprint != s.fingerprint) {
        r.view.phase = ReassignmentPhase::Stale;
        r.view.reason = ReassignmentReason::StateChanged;
        save(tx, &r)?;
        return Ok(false);
    }
    if paused(tx.conn(), s)? {
        r.view.phase = ReassignmentPhase::Suppressed;
        r.view.reason = ReassignmentReason::Stopped;
        save(tx, &r)?;
        return Ok(false);
    }
    r.view.phase = ReassignmentPhase::Dispatched;
    save(tx, &r)?;
    Ok(true)
}
// The post-effect witness intentionally uses the resulting owner/revision, not the pre-selection snapshot.
fn post(c: &Connection, s: &Snapshot, new_owner: &str) -> Result<Option<String>> {
    let p = work::get(c, &s.id)?;
    if p.frozen
        || matches!(p.status.as_str(), "done" | "dropped")
        || p.owner.as_deref() != Some(new_owner)
        || crate::crews::captain_of(c, &s.crew)?.as_deref() != Some(s.captain.as_str())
    {
        return Ok(None);
    }
    for who in [new_owner, s.captain.as_str()] {
        let a = crate::sessions::get(c, who)?;
        if a.crew_id != s.crew
            || (who == new_owner && a.resume_failed_at.is_some())
            || wakes::automatic_work_paused(c, who)?
        {
            return Ok(None);
        }
    }
    let reference = crewd_store::object_refs::get(c, ObjectKind::Plan, work::number(&s.id)?)?
        .ok_or_else(invalid)?;
    let book = crate::asks::load_org(c)?;
    let item = book
        .item(work::core_plan(c, &reference)?)
        .ok_or_else(invalid)?;
    if let Some(ch) = &item.channel {
        for who in [new_owner, s.captain.as_str()] {
            if !crate::channels::readable_by(c, ch.as_str(), who)? {
                return Ok(None);
            }
        }
    }
    Ok(Some(judgments::fingerprint(&(
        &p.revision,
        new_owner,
        &s.captain,
        context(c, new_owner, &s.captain)?,
    ))?))
}
pub fn settle(
    tx: &mut Tx<'_>,
    s: &Snapshot,
    selected: Option<String>,
    confidence: Option<f64>,
    reason: ReassignmentReason,
) -> Result<()> {
    let Some(mut r) = record(tx.conn(), &s.id)? else {
        return Ok(());
    };
    if r.snapshot.fingerprint != s.fingerprint
        || !matches!(r.view.phase, ReassignmentPhase::Dispatched)
    {
        return Ok(());
    }
    if snapshot(tx.conn(), &s.id, tx.now_ms())?.is_none_or(|n| n.fingerprint != s.fingerprint) {
        r.view.phase = ReassignmentPhase::Stale;
        r.view.reason = ReassignmentReason::StateChanged;
        return save(tx, &r);
    }
    r.view.phase = ReassignmentPhase::Settled;
    r.view.reason = reason;
    r.view.confidence = confidence;
    if let Some(target) = selected {
        if paused(tx.conn(), s)? {
            r.view.phase = ReassignmentPhase::Suppressed;
            r.view.reason = ReassignmentReason::Stopped;
            return save(tx, &r);
        }
        if !s.candidates.iter().any(|m| m.id == target) {
            return Err(invalid());
        }
        let plan = crate::work_lost::reassign_trusted(
            tx,
            crewd_proto::WorkAssignParams {
                id: s.id.clone(),
                expected_revision: s.revision.clone(),
                owner: target.clone(),
                note: "Verified lost owner; scheduler selected an eligible replacement".into(),
            },
            &r.view.decision_id,
        )?;
        let owner_wake: i64 = tx.conn().query_row(
            "SELECT id FROM wakes WHERE session_id=?1 AND source=?2",
            rusqlite::params![target, format!("work-lost:{}:{}", s.id, plan.revision)],
            |r| r.get(0),
        )?;
        let result = wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: s.captain.clone(),
                kind: wakes::WakeKind::System,
                source: format!("work-reassignment:{}:{}", s.id, r.view.decision_id),
                priority: wakes::WakePriority::Auto,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )?;
        let captain_wake = match result {
            wakes::Enqueued::New(id) | wakes::Enqueued::Duplicate(id) => id,
            _ => return Err(invalid()),
        };
        r.post = Some(post(tx.conn(), s, &target)?.ok_or_else(invalid)?);
        r.view.adopted = true;
        r.view.new_owner = Some(target);
        r.view.owner_wake_id = Some(owner_wake);
        r.view.captain_wake_id = Some(captain_wake);
    }
    save(tx, &r)
}
pub fn observe_lost(tx: &mut Tx<'_>) -> Result<()> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Cursor {
        epoch: String,
        after: Option<String>,
    }
    const KEY: &str = "work.reassignment.scan.v1";
    let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
    let saved = meta_get(tx.conn(), KEY)?
        .map(|s| serde_json::from_str::<Cursor>(&s))
        .transpose()
        .map_err(StoreError::from)?;
    let after = saved.filter(|s| s.epoch == epoch).and_then(|s| s.after);
    let page = work::list(
        tx.conn(),
        crewd_proto::WorkListParams {
            after,
            limit: Some(64),
            include_removed: Some(false),
            ..Default::default()
        },
    )?;
    for p in page.items {
        prepare(tx, &p.id)?;
    }
    meta_set(
        tx.conn(),
        KEY,
        &serde_json::to_string(&Cursor {
            epoch,
            after: page.next_after,
        })
        .map_err(StoreError::from)?,
    )?;
    Ok(())
}
/// Page over source keys, not only matches, so empty pages still advance.
pub fn pending_page(
    c: &Connection,
    after: Option<String>,
) -> Result<(Vec<String>, Option<String>)> {
    let mut rows = {
        let mut q=c.prepare("SELECT key,value FROM meta WHERE key LIKE 'work.reassignment.v1:%' AND key>?1 ORDER BY key LIMIT 65")?;
        q.query_map([after.unwrap_or_default()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let next = if rows.len() > 64 {
        rows.truncate(64);
        rows.last().map(|r| r.0.clone())
    } else {
        None
    };
    let mut ids = Vec::new();
    for (stored_key, raw) in rows {
        let r: Record = serde_json::from_str(&raw).map_err(StoreError::from)?;
        if r.version != 1 || stored_key != key(&r.snapshot.id) {
            return Err(invalid());
        }
        if matches!(r.view.phase, ReassignmentPhase::Prepared) {
            ids.push(r.snapshot.id);
        }
    }
    Ok((ids, next))
}
pub fn recover_page(
    tx: &mut Tx<'_>,
    after: Option<String>,
) -> Result<(usize, Vec<String>, Option<String>)> {
    let mut rows = {
        let mut q=tx.conn().prepare("SELECT key,value FROM meta WHERE key LIKE 'work.reassignment.v1:%' AND key>?1 ORDER BY key LIMIT 65")?;
        q.query_map([after.unwrap_or_default()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let next = if rows.len() > 64 {
        rows.truncate(64);
        rows.last().map(|r| r.0.clone())
    } else {
        None
    };
    let mut count = 0;
    let mut prepared = Vec::new();
    for (stored_key, raw) in rows {
        let mut r: Record = serde_json::from_str(&raw).map_err(StoreError::from)?;
        if r.version != 1 || stored_key != key(&r.snapshot.id) {
            return Err(invalid());
        }
        if matches!(r.view.phase, ReassignmentPhase::Prepared) {
            prepared.push(r.snapshot.id.clone());
        }
        if matches!(r.view.phase, ReassignmentPhase::Dispatched) {
            r.view.phase = ReassignmentPhase::Unknown;
            r.view.reason = ReassignmentReason::RestartUnknown;
            save(tx, &r)?;
            count += 1;
        }
    }
    Ok((count, prepared, next))
}

pub fn status(c: &Connection, crew: &str, id: &str) -> Result<crewd_proto::ReassignmentStatus> {
    if work::get(c, id)?.crew_id != crew {
        return Err(invalid());
    }
    let mut r = record(c, id)?;
    if let Some(r) = &mut r {
        let current = if r.view.adopted {
            post(
                c,
                &r.snapshot,
                r.view.new_owner.as_deref().ok_or_else(invalid)?,
            )? == r.post
                && r.post.is_some()
        } else {
            snapshot(c, id, chrono::Utc::now().timestamp_millis())?
                .is_some_and(|s| s.fingerprint == r.snapshot.fingerprint)
        };
        if !current {
            r.view.phase = ReassignmentPhase::Stale;
            r.view.reason = ReassignmentReason::StateChanged;
        }
        if let Some(id) = r.view.owner_wake_id {
            r.view.owner_wake_state = wakes::by_ids(c, &[id])?.first().map(|r| r.state);
        }
        if let Some(id) = r.view.captain_wake_id {
            r.view.captain_wake_state = wakes::by_ids(c, &[id])?.first().map(|r| r.state);
        }
    }
    Ok(crewd_proto::ReassignmentStatus {
        crew_id: crew.into(),
        id: id.into(),
        record: r.map(|r| r.view),
    })
}
pub(crate) fn owner_notification_current(
    c: &Connection,
    row: &wakes::WakeRow,
    id: &str,
    decision: &str,
) -> Result<bool> {
    let Some(r) = record(c, id)? else {
        return Ok(false);
    };
    let Some(owner) = r.view.new_owner.as_deref() else {
        return Ok(false);
    };
    Ok(r.view.adopted
        && matches!(r.view.phase, ReassignmentPhase::Settled)
        && r.view.decision_id == decision
        && r.view.owner_wake_id == Some(row.id)
        && row.session == owner
        && r.post.is_some()
        && post(c, &r.snapshot, owner)? == r.post)
}
pub(crate) fn render(c: &Connection, row: &wakes::WakeRow) -> Result<work::FollowupNote> {
    let Some(rest) = row.source.strip_prefix("work-reassignment:") else {
        return Ok(work::FollowupNote::Ordinary);
    };
    let Some((id, decision)) = rest.split_once(':') else {
        return Ok(work::FollowupNote::Stale);
    };
    let Some(r) = record(c, id)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if row.kind != wakes::WakeKind::System
        || row.priority != wakes::WakePriority::Auto
        || !r.view.adopted
        || r.view.decision_id != decision
        || r.view.captain_wake_id != Some(row.id)
        || row.session != r.snapshot.captain
        || !matches!(r.view.phase, ReassignmentPhase::Settled)
    {
        return Ok(work::FollowupNote::Stale);
    }
    let Some(owner) = r.view.new_owner.as_deref() else {
        return Ok(work::FollowupNote::Stale);
    };
    if r.post.is_none() || post(c, &r.snapshot, owner)? != r.post {
        return Ok(work::FollowupNote::Stale);
    }
    Ok(work::FollowupNote::Current(format!(
        "{} 的原负责人已核实失联，调度已将同一项工作重派给 {}。请查看当前负责人和进展；重派与通知不代表工作完成。",
        id, owner
    )))
}
/// Existing durable notification identities for guarded channel lifecycle checks.
pub(crate) fn archive_wake_ids(c: &Connection, id: &str) -> Result<Vec<i64>> {
    Ok(record(c, id)?
        .map(|r| {
            [r.view.owner_wake_id, r.view.captain_wake_id]
                .into_iter()
                .flatten()
                .collect()
        })
        .unwrap_or_default())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{err, ok, store},
        wakes, work,
    };
    fn lost(s: &mut crewd_store::Store) -> crewd_proto::WorkPlan {
        let p = ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "recover original work".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        s.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='failure' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        p
    }
    #[test]
    fn verified_loss_prepares_selection() {
        let mut s = store();
        let p = lost(&mut s);
        assert!(ok(&mut s, |tx| prepare(tx, &p.id)).is_some());
    }

    #[test]
    fn atomic_reassignment_and_two_notifications() {
        let mut db = store();
        let p = lost(&mut db);
        let s = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        assert!(ok(&mut db, |tx| dispatch(tx, &s)));
        db.conn().execute_batch("CREATE TRIGGER fail_captain BEFORE INSERT ON wakes WHEN NEW.source LIKE 'work-reassignment:%' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        let _ = err(&mut db, |tx| {
            settle(
                tx,
                &s,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        assert_eq!(work::get(db.conn(), &p.id).unwrap().owner, p.owner);
        assert!(
            wakes::list(db.conn(), Some("w-a2"), 100)
                .unwrap()
                .is_empty()
        );
        db.conn()
            .execute_batch("DROP TRIGGER fail_captain")
            .unwrap();
        ok(&mut db, |tx| {
            settle(
                tx,
                &s,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        let r = status(db.conn(), "crew-a", &p.id).unwrap().record.unwrap();
        assert!(r.adopted);
        assert_eq!(r.phase, ReassignmentPhase::Settled);
        assert_ne!(r.owner_wake_id, r.captain_wake_id);
        for id in [r.owner_wake_id.unwrap(), r.captain_wake_id.unwrap()] {
            let row = wakes::by_ids(db.conn(), &[id]).unwrap().remove(0);
            assert!(matches!(
                work::followup_note(db.conn(), &row, crate::testkit::NOW).unwrap(),
                work::FollowupNote::Current(_)
            ));
        }
        assert!(ok(&mut db, |tx| prepare(tx, &p.id)).is_none());
        assert!(
            status(db.conn(), "crew-a", &p.id)
                .unwrap()
                .record
                .unwrap()
                .adopted
        );
        ok(&mut db, |tx| {
            settle(
                tx,
                &s,
                Some("cap-a".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        assert_eq!(
            work::get(db.conn(), &p.id).unwrap().owner.as_deref(),
            Some("w-a2")
        );
        db.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='new failure' WHERE id='w-a2'",
                [],
            )
            .unwrap();
        for id in [r.owner_wake_id.unwrap(), r.captain_wake_id.unwrap()] {
            let row = wakes::by_ids(db.conn(), &[id]).unwrap().remove(0);
            assert!(matches!(
                work::followup_note(db.conn(), &row, crate::testkit::NOW).unwrap(),
                work::FollowupNote::Stale
            ));
        }
    }
    #[test]
    fn stopped_owner_never_dispatches_and_late_candidate_stop_never_reassigns() {
        let mut db = store();
        let p = lost(&mut db);
        let s = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        ok(&mut db, |tx| wakes::human_stop(tx, "w-a1"));
        assert!(!ok(&mut db, |tx| dispatch(tx, &s)));
        assert_eq!(work::get(db.conn(), &p.id).unwrap().owner, p.owner);
        let mut db = store();
        let p = lost(&mut db);
        let s = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        assert!(ok(&mut db, |tx| dispatch(tx, &s)));
        ok(&mut db, |tx| wakes::human_stop(tx, "w-a2"));
        ok(&mut db, |tx| {
            settle(
                tx,
                &s,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        assert_eq!(work::get(db.conn(), &p.id).unwrap().owner, p.owner);
        assert!(
            !status(db.conn(), "crew-a", &p.id)
                .unwrap()
                .record
                .unwrap()
                .adopted
        );
    }
    #[test]
    fn recovery_preserves_prepared_but_never_resends_dispatched() {
        let mut db = store();
        let p = lost(&mut db);
        let s = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        let (n, ids, _) = ok(&mut db, |tx| recover_page(tx, None));
        assert_eq!(n, 0);
        assert_eq!(ids, vec![p.id.clone()]);
        assert!(ok(&mut db, |tx| prepare(tx, &p.id)).is_some());
        assert!(ok(&mut db, |tx| dispatch(tx, &s)));
        assert_eq!(ok(&mut db, |tx| recover_page(tx, None)).0, 1);
        assert!(ok(&mut db, |tx| prepare(tx, &p.id)).is_none());
        ok(&mut db, |tx| {
            settle(
                tx,
                &s,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        assert_eq!(work::get(db.conn(), &p.id).unwrap().owner, p.owner);
    }
    #[test]
    fn scan_prepares_existing_loss_without_event_and_keeps_atomic_cursor() {
        let mut db = store();
        let p = lost(&mut db);
        ok(&mut db, observe_lost);
        assert_eq!(pending_page(db.conn(), None).unwrap().0, vec![p.id]);
    }

    #[test]
    fn scan_pages_cycle_and_reset_after_restore() {
        let mut db = store();
        let mut plans = Vec::new();
        for _ in 0..65 {
            plans.push(lost(&mut db));
        }
        ok(&mut db, observe_lost);
        assert!(record(db.conn(), &plans[63].id).unwrap().is_some());
        assert!(record(db.conn(), &plans[64].id).unwrap().is_none());
        ok(&mut db, observe_lost);
        assert!(record(db.conn(), &plans[64].id).unwrap().is_some());
        let cursor: serde_json::Value = serde_json::from_str(
            &meta_get(db.conn(), "work.reassignment.scan.v1")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(cursor["after"].is_null());
        ok(&mut db, observe_lost);
        let before = record(db.conn(), &plans[0].id)
            .unwrap()
            .unwrap()
            .snapshot
            .fingerprint;
        crewd_store::judgment_state::restored(db.conn()).unwrap();
        ok(&mut db, observe_lost);
        assert_ne!(
            before,
            record(db.conn(), &plans[0].id)
                .unwrap()
                .unwrap()
                .snapshot
                .fingerprint
        );
    }
    #[test]
    fn delivered_checkpoints_and_elapsed_deadline_prepare_silent_loss() {
        use crate::{
            cursor_reservations as cr,
            testkit::{MIN, NOW, ok_at},
        };
        let mut db = store();
        let p = ok(&mut db, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "silent original".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        ok(&mut db, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                crewd_proto::WorkUpdateParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    status: Some(crewd_proto::WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + MIN)),
                    note: Some("checking".into()),
                    ..Default::default()
                },
            )
        });
        let reference = crewd_store::object_refs::get(
            db.conn(),
            ObjectKind::Plan,
            work::number(&p.id).unwrap(),
        )
        .unwrap()
        .unwrap();
        let core = work::core_plan(db.conn(), &reference).unwrap();
        for _ in 0..2 {
            let now = crate::asks::load_org(db.conn())
                .unwrap()
                .item(core)
                .unwrap()
                .next_check_at
                .unwrap()
                .0;
            assert!(ok_at(&mut db, now, |tx| prepare(tx, &p.id)).0.is_none());
            ok_at(&mut db, now, crate::checkpoints::enqueue_due);
            let row = wakes::list(db.conn(), Some("w-a1"), 100)
                .unwrap()
                .into_iter()
                .find(|r| {
                    r.kind == wakes::WakeKind::Checkpoint && r.state == wakes::WakeState::Pending
                })
                .unwrap();
            let mut snap = cr::snapshot(db.conn(), "w-a1", &[], now).unwrap();
            snap.capture_work_followups(db.conn(), std::slice::from_ref(&row), now)
                .unwrap();
            let cr::Decision::Applied(lease) = ok_at(&mut db, now, |tx| {
                cr::claim_reserved(tx, &snap, &[], &[row.id], &[], cr::Owner::new(1).unwrap())
            })
            .0
            else {
                panic!("claim")
            };
            assert!(matches!(
                ok_at(&mut db, now, |tx| cr::ack_reserved_delivered(
                    tx,
                    &lease,
                    wakes::Riders::Release
                ))
                .0,
                cr::Decision::Applied(())
            ));
        }
        let now = crate::asks::load_org(db.conn())
            .unwrap()
            .item(core)
            .unwrap()
            .next_check_at
            .unwrap()
            .0;
        ok_at(&mut db, now, observe_lost);
        let r = record(db.conn(), &p.id).unwrap().unwrap();
        assert_eq!(r.snapshot.loss, "silent");
        assert_eq!(r.view.phase, ReassignmentPhase::Prepared);
    }

    #[test]
    fn scan_failure_rolls_back_preparations_and_cursor() {
        let mut db = store();
        let p = lost(&mut db);
        db.conn().execute_batch("CREATE TRIGGER reject_cursor BEFORE INSERT ON meta WHEN NEW.key='work.reassignment.scan.v1' BEGIN SELECT RAISE(ABORT,'scan fault'); END;").unwrap();
        let _ = err(&mut db, observe_lost);
        assert!(record(db.conn(), &p.id).unwrap().is_none());
        assert!(
            meta_get(db.conn(), "work.reassignment.scan.v1")
                .unwrap()
                .is_none()
        );
        db.conn()
            .execute_batch("DROP TRIGGER reject_cursor")
            .unwrap();
        ok(&mut db, observe_lost);
        assert!(record(db.conn(), &p.id).unwrap().is_some());
    }
    #[test]
    fn restore_invalidates_both_effect_notifications_without_erasing_adoption() {
        let mut db = store();
        let p = lost(&mut db);
        let snap = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        assert!(ok(&mut db, |tx| dispatch(tx, &snap)));
        ok(&mut db, |tx| {
            settle(
                tx,
                &snap,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        let r = status(db.conn(), "crew-a", &p.id).unwrap().record.unwrap();
        crewd_store::judgment_state::restored(db.conn()).unwrap();
        for id in [r.owner_wake_id.unwrap(), r.captain_wake_id.unwrap()] {
            let row = wakes::by_ids(db.conn(), &[id]).unwrap().remove(0);
            assert!(matches!(
                work::followup_note(db.conn(), &row, crate::testkit::NOW).unwrap(),
                work::FollowupNote::Stale
            ));
        }
        let r = status(db.conn(), "crew-a", &p.id).unwrap().record.unwrap();
        assert!(r.adopted);
        assert_eq!(r.phase, ReassignmentPhase::Stale);
    }

    #[test]
    fn lost_captain_work_moves_to_worker_and_notification_survives_recovery() {
        let mut db = store();
        let p = ok(&mut db, |tx| {
            work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "captain original".into(),
                    owner: Some("cap-a".into()),
                    ..Default::default()
                },
            )
        });
        db.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='failed' WHERE id='cap-a'",
                [],
            )
            .unwrap();
        let snap = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        assert!(snap.candidates.iter().all(|c| c.id != "cap-a"));
        assert!(ok(&mut db, |tx| dispatch(tx, &snap)));
        ok(&mut db, |tx| {
            settle(
                tx,
                &snap,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        let r = status(db.conn(), "crew-a", &p.id).unwrap().record.unwrap();
        assert!(r.adopted);
        assert_eq!(r.phase, ReassignmentPhase::Settled);
        assert!(
            crate::sessions::get(db.conn(), "cap-a")
                .unwrap()
                .resume_failed_at
                .is_some()
        );
        let row = wakes::by_ids(db.conn(), &[r.captain_wake_id.unwrap()])
            .unwrap()
            .remove(0);
        assert_eq!(row.state, wakes::WakeState::Pending);
        assert!(wakes::delivery_held(db.conn(), &row).unwrap());
        let _ = err(&mut db, |tx| wakes::claim(tx, &[row.id]));
        let unchanged = wakes::by_ids(db.conn(), &[row.id]).unwrap().remove(0);
        assert_eq!(unchanged.state, wakes::WakeState::Pending);
        assert_eq!(unchanged.attempts, 0);
        let owner_row = wakes::by_ids(db.conn(), &[r.owner_wake_id.unwrap()])
            .unwrap()
            .remove(0);
        assert!(!wakes::delivery_held(db.conn(), &owner_row).unwrap());
        assert_eq!(ok(&mut db, |tx| wakes::claim(tx, &[owner_row.id])), vec![1]);
        db.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at=NULL WHERE id='cap-a'",
                [],
            )
            .unwrap();
        assert!(!wakes::delivery_held(db.conn(), &row).unwrap());
        assert_eq!(ok(&mut db, |tx| wakes::claim(tx, &[row.id])), vec![1]);
        assert!(matches!(
            work::followup_note(db.conn(), &row, crate::testkit::NOW).unwrap(),
            work::FollowupNote::Current(_)
        ));
    }

    #[test]
    fn human_stop_drops_held_failed_captain_notice() {
        let mut db = store();
        let p = lost(&mut db);
        let snap = ok(&mut db, |tx| prepare(tx, &p.id)).unwrap();
        assert!(ok(&mut db, |tx| dispatch(tx, &snap)));
        ok(&mut db, |tx| {
            settle(
                tx,
                &snap,
                Some("w-a2".into()),
                Some(0.99),
                ReassignmentReason::Answered,
            )
        });
        let r = status(db.conn(), "crew-a", &p.id).unwrap().record.unwrap();
        let id = r.captain_wake_id.unwrap();
        db.conn()
            .execute(
                "UPDATE sessions SET resume_failed_at='failed' WHERE id='cap-a'",
                [],
            )
            .unwrap();
        let row = wakes::by_ids(db.conn(), &[id]).unwrap().remove(0);
        assert!(wakes::delivery_held(db.conn(), &row).unwrap());
        let dropped = ok(&mut db, |tx| wakes::human_stop(tx, "cap-a"));
        assert!(dropped.iter().any(|r| r.id == id));
        let row = wakes::by_ids(db.conn(), &[id]).unwrap().remove(0);
        assert_eq!(row.state, wakes::WakeState::Dropped);
        assert_eq!(row.attempts, 0);
    }
}
