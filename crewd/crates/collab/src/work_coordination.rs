//! Optional semantic observation. Only the captain may coordinate actual work.
use crate::{Result, Tx, judgments, wakes, work};
use crewd_proto::{CoordinationPhase, CoordinationReason, CoordinationView};
use crewd_store::{StoreError, meta_get, meta_set, object_refs::ObjectKind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
mod assignment;
mod outcome;
mod split;
pub use assignment::assign;
pub use split::{SplitChild, SplitParams, TOOL as SPLIT_TOOL, apply as apply_split};

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub crew: String,
    pub owner: String,
    pub captain: String,
    pub title: String,
    pub reports: Vec<String>,
    pub fingerprint: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    snapshot: Snapshot,
    view: CoordinationView,
}
fn key(id: &str) -> String {
    format!("work.coordination.v1:{id}")
}
fn wake_source(id: &str, decision: &str) -> String {
    format!("work-coordination:{id}:{decision}")
}
fn matches_wake(r: &Record, wake: &wakes::WakeRow) -> bool {
    wake.kind == wakes::WakeKind::System
        && r.view.wake_id == Some(wake.id)
        && wake.session == r.snapshot.captain
        && r.view.recipient.as_deref() == Some(wake.session.as_str())
        && wake.source == wake_source(&r.snapshot.id, &r.view.decision_id)
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid coordination authority".into()).into()
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
pub fn snapshot(c: &Connection, id: &str) -> Result<Option<Snapshot>> {
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
    let reference = crewd_store::object_refs::get(c, ObjectKind::Plan, work::number(id)?)?
        .ok_or_else(invalid)?;
    let core = work::core_plan(c, &reference)?;
    let book = crate::asks::load_org(c)?;
    let item = book.item(core).ok_or_else(invalid)?;
    if let Some(ch) = &item.channel
        && !crate::channels::readable_by(c, ch.as_str(), &captain)?
    {
        return Ok(None);
    }
    let reports = crewd_store::work_organization::recent_reports(c, &public.crew_id, core)?;
    let eligible = reports.len() == 2
        && reports
            .iter()
            .all(|e| e.by.session().map(|s| s.as_str()) == Some(owner.as_str()));
    let texts = reports
        .iter()
        .filter(|_| eligible)
        .rev()
        .map(|e| match &e.kind {
            crewd_work::EventKind::Reported { note, .. } => {
                note.chars().take(1200).collect::<String>()
            }
            _ => unreachable!(),
        })
        .collect();
    let settings = judgments::configuration(c)?;
    let fingerprint = judgments::fingerprint(&(
        id,
        &public.crew_id,
        public.revision,
        &owner,
        &captain,
        &reports,
        settings,
        crewd_store::judgment_state::epoch(c)?,
        wakes::human_stop_floor(c, &owner)?,
        judgments::stop_epoch(c, &owner)?,
        wakes::human_stop_floor(c, &captain)?,
        judgments::stop_epoch(c, &captain)?,
    ))?;
    Ok(Some(Snapshot {
        id: id.into(),
        crew: public.crew_id,
        owner,
        captain,
        title: public.title.chars().take(200).collect(),
        reports: texts,
        fingerprint,
    }))
}
fn save(tx: &mut Tx<'_>, r: &Record) -> Result<()> {
    meta_set(
        tx.conn(),
        &key(&r.snapshot.id),
        &serde_json::to_string(r).map_err(StoreError::from)?,
    )?;
    Ok(())
}
pub fn prepare(tx: &mut Tx<'_>, id: &str) -> Result<Option<Snapshot>> {
    let Some(s) = snapshot(tx.conn(), id)? else {
        return Ok(None);
    };
    if let Some(r) = record(tx.conn(), id)?
        && r.snapshot.fingerprint == s.fingerprint
    {
        return Ok(matches!(r.view.phase, CoordinationPhase::Prepared).then_some(s));
    }
    let r = Record {
        version: 1,
        snapshot: s.clone(),
        view: CoordinationView {
            decision_id: uuid::Uuid::new_v4().to_string(),
            phase: if s.reports.len() < 2 {
                CoordinationPhase::Settled
            } else {
                CoordinationPhase::Prepared
            },
            reason: if s.reports.len() < 2 {
                CoordinationReason::InsufficientReports
            } else {
                CoordinationReason::Prepared
            },
            meaningful_progress: None,
            needs_coordination: None,
            adopted: false,
            recipient: None,
            wake_id: None,
            wake_state: None,
        },
    };
    save(tx, &r)?;
    Ok((s.reports.len() == 2).then_some(s))
}
fn paused(c: &Connection, s: &Snapshot) -> Result<bool> {
    for id in [&s.owner, &s.captain] {
        if wakes::automatic_work_paused(c, id)?
            || crate::sessions::get(c, id)?.resume_failed_at.is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn dispatch(tx: &mut Tx<'_>, s: &Snapshot) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), &s.id)? else {
        return Ok(false);
    };
    if r.snapshot.fingerprint != s.fingerprint
        || !matches!(r.view.phase, CoordinationPhase::Prepared)
    {
        return Ok(false);
    }
    if snapshot(tx.conn(), &s.id)?.is_none_or(|n| n.fingerprint != s.fingerprint) {
        r.view.phase = CoordinationPhase::Stale;
        r.view.reason = CoordinationReason::StateChanged;
        save(tx, &r)?;
        return Ok(false);
    }
    if paused(tx.conn(), s)? {
        r.view.phase = CoordinationPhase::Suppressed;
        r.view.reason = CoordinationReason::Stopped;
        save(tx, &r)?;
        return Ok(false);
    }
    r.view.phase = CoordinationPhase::Dispatched;
    save(tx, &r)?;
    Ok(true)
}
/// Durable event recovery. The first observed epoch establishes a baseline;
/// subsequent pages commit new preparations and their cursor atomically.
pub fn observe_reports(tx: &mut Tx<'_>) -> Result<()> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Cursor {
        epoch: String,
        seq: i64,
    }
    const CURSOR: &str = "work.coordination.reports.v1";
    let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
    let saved = meta_get(tx.conn(), CURSOR)?
        .map(|v| serde_json::from_str::<Cursor>(&v))
        .transpose()
        .map_err(StoreError::from)?;
    let Some(saved) = saved.filter(|s| s.epoch == epoch) else {
        let seq: i64 = tx.conn().query_row(
            "SELECT coalesce(max(seq),0) FROM organization_work_events",
            [],
            |r| r.get(0),
        )?;
        meta_set(
            tx.conn(),
            CURSOR,
            &serde_json::to_string(&Cursor { epoch, seq }).map_err(StoreError::from)?,
        )?;
        return Ok(());
    };
    let rows = {
        let mut q=tx.conn().prepare("SELECT seq,crew_id,body FROM organization_work_events WHERE seq>?1 ORDER BY seq LIMIT 64")?;
        q.query_map([saved.seq], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut cursor = saved.seq;
    let mut ids = std::collections::BTreeSet::new();
    for (seq, crew, raw) in rows {
        let event: crewd_work::Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
        if seq <= 0 || event.seq != seq as u64 {
            return Err(invalid());
        }
        if let crewd_work::EventKind::Reported { id, .. } = event.kind {
            let reference = crewd_store::work_organization::reference_for_core(
                tx.conn(),
                ObjectKind::Plan,
                &id.to_string(),
            )?
            .ok_or_else(invalid)?;
            if reference.crew_id != crew {
                return Err(invalid());
            }
            ids.insert(reference.id());
        }
        cursor = seq;
    }
    for id in ids {
        prepare(tx, &id)?;
    }
    meta_set(
        tx.conn(),
        CURSOR,
        &serde_json::to_string(&Cursor { epoch, seq: cursor }).map_err(StoreError::from)?,
    )?;
    Ok(())
}
/// Page over source keys, not only matches, so empty pages still advance.
pub fn pending_page(
    c: &Connection,
    after: Option<String>,
) -> Result<(Vec<String>, Option<String>)> {
    let mut rows = {
        let mut q=c.prepare("SELECT key,value FROM meta WHERE key LIKE 'work.coordination.v1:%' AND key>?1 ORDER BY key LIMIT 65")?;
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
        if matches!(r.view.phase, CoordinationPhase::Prepared) {
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
        let mut q=tx.conn().prepare("SELECT key,value FROM meta WHERE key LIKE 'work.coordination.v1:%' AND key>?1 ORDER BY key LIMIT 65")?;
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
        if matches!(r.view.phase, CoordinationPhase::Prepared) {
            prepared.push(r.snapshot.id.clone());
        }
        if matches!(r.view.phase, CoordinationPhase::Dispatched) {
            r.view.phase = CoordinationPhase::Unknown;
            r.view.reason = CoordinationReason::RestartUnknown;
            save(tx, &r)?;
            count += 1;
        }
    }
    Ok((count, prepared, next))
}
pub fn status(
    c: &Connection,
    crew: &str,
    id: &str,
    decision: Option<&str>,
) -> Result<crewd_proto::CoordinationStatus> {
    let work = work::get(c, id)?;
    if work.crew_id != crew {
        return Err(invalid());
    }
    let mut record = record(c, id)?;
    if let Some(r) = &mut record {
        if snapshot(c, id)?.is_none_or(|s| s.fingerprint != r.snapshot.fingerprint) {
            r.view.phase = CoordinationPhase::Stale;
            r.view.reason = CoordinationReason::StateChanged;
        }
        r.view.wake_state = if let Some(id) = r.view.wake_id {
            wakes::by_ids(c, &[id])?
                .iter()
                .find(|row| matches_wake(r, row))
                .map(|row| row.state)
        } else {
            None
        };
    }
    Ok(crewd_proto::CoordinationStatus {
        crew_id: crew.into(),
        id: id.into(),
        record: record.map(|r| r.view),
        last_assignment: assignment::latest(c, crew, id)?,
        requested_assignment: decision
            .map(|decision| assignment::lookup(c, crew, id, decision))
            .transpose()?
            .flatten(),
        last_split: split::latest(c, crew, id)?,
        requested_split: decision
            .map(|decision| split::lookup(c, crew, id, decision))
            .transpose()?
            .flatten(),
    })
}
pub fn settle(
    tx: &mut Tx<'_>,
    s: &Snapshot,
    progress: Option<f64>,
    coordination: Option<f64>,
    adopt: bool,
    reason: CoordinationReason,
) -> Result<()> {
    let Some(mut r) = record(tx.conn(), &s.id)? else {
        return Ok(());
    };
    if r.snapshot.fingerprint != s.fingerprint
        || !matches!(r.view.phase, CoordinationPhase::Dispatched)
    {
        return Ok(());
    }
    if snapshot(tx.conn(), &s.id)?.is_none_or(|n| n.fingerprint != s.fingerprint) {
        r.view.phase = CoordinationPhase::Stale;
        r.view.reason = CoordinationReason::StateChanged;
        return save(tx, &r);
    }
    r.view.phase = CoordinationPhase::Settled;
    r.view.reason = reason;
    r.view.meaningful_progress = progress;
    r.view.needs_coordination = coordination;
    if adopt {
        if paused(tx.conn(), s)? {
            r.view.phase = CoordinationPhase::Suppressed;
            r.view.reason = CoordinationReason::Stopped;
            return save(tx, &r);
        }
        let source = wake_source(&s.id, &r.view.decision_id);
        let result = wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: s.captain.clone(),
                kind: wakes::WakeKind::System,
                source,
                priority: wakes::WakePriority::Auto,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )?;
        if let wakes::Enqueued::New(id) | wakes::Enqueued::Duplicate(id) = result {
            r.view.adopted = true;
            r.view.recipient = Some(s.captain.clone());
            r.view.wake_id = Some(id)
        }
    }
    save(tx, &r)
}
pub(crate) fn render(c: &Connection, row: &wakes::WakeRow) -> Result<work::FollowupNote> {
    let Some(rest) = row.source.strip_prefix("work-coordination:") else {
        return Ok(work::FollowupNote::Ordinary);
    };
    let Some((id, decision)) = rest.split_once(':') else {
        return Ok(work::FollowupNote::Stale);
    };
    let Some(r) = record(c, id)? else {
        return Ok(work::FollowupNote::Stale);
    };
    if !matches_wake(&r, row)
        || paused(c, &r.snapshot)?
        || !r.view.adopted
        || !matches!(r.view.phase, CoordinationPhase::Settled)
        || r.view.decision_id != decision
        || wakes::automatic_work_paused(c, &row.session)?
        || snapshot(c, id)?.is_none_or(|s| s.fingerprint != r.snapshot.fingerprint)
    {
        return Ok(work::FollowupNote::Stale);
    }
    Ok(work::FollowupNote::Current(crate::prompts::fill(
        crate::prompts::render_file(crate::Locale::Zh).get("notice.work.coordination"),
        &[("work", id), ("decision", decision)],
    )))
}
/// Existing durable notification identities for guarded channel lifecycle checks.
pub(crate) fn archive_wake_ids(c: &Connection, id: &str) -> Result<Vec<i64>> {
    Ok(record(c, id)?
        .and_then(|r| r.view.wake_id)
        .into_iter()
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{NOW, ok, store},
    };
    use crewd_proto::{WorkCreateParams, WorkUpdateParams};
    fn work(s: &mut crewd_store::Store) -> String {
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Session("w-a1".into()),
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "implement feature".into(),
                    ..Default::default()
                },
            )
        })
        .id
    }
    fn report(s: &mut crewd_store::Store, id: &str, note: &str) {
        let p = work::get(s.conn(), id).unwrap();
        ok(s, |tx| {
            work::update(
                tx,
                &Sender::Session(p.owner.unwrap()),
                WorkUpdateParams {
                    id: id.into(),
                    expected_revision: p.revision,
                    note: Some(note.into()),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60000)),
                    ..Default::default()
                },
            )
        });
    }

    fn adopt(s: &mut crewd_store::Store, id: &str) -> CoordinationView {
        report(s, id, "one");
        report(s, id, "two");
        let snap = ok(s, |tx| prepare(tx, id)).unwrap();
        assert!(ok(s, |tx| dispatch(tx, &snap)));
        ok(s, |tx| {
            settle(
                tx,
                &snap,
                Some(0.1),
                Some(0.9),
                true,
                CoordinationReason::Answered,
            )
        });
        status(s.conn(), "crew-a", id, None)
            .unwrap()
            .record
            .unwrap()
    }

    fn deliver(s: &mut crewd_store::Store, wake_id: i64) {
        ok(s, |tx| {
            wakes::claim(tx, &[wake_id])?;
            wakes::ack_delivered(tx, "cap-a", &[wake_id], wakes::Riders::Release, &[], &[])
        });
    }

    fn assignment(s: &crewd_store::Store, id: &str, target: &str) -> crewd_proto::WorkAssignParams {
        crewd_proto::WorkAssignParams {
            id: id.into(),
            owner: target.into(),
            expected_revision: work::get(s.conn(), id).unwrap().revision,
            note: "coordination assignment reason".into(),
        }
    }

    fn split_params(s: &crewd_store::Store, id: &str, decision: &str) -> SplitParams {
        let child = |title: &str, owner: Option<&str>| SplitChild {
            title: title.into(),
            goal: "isolate independently executable work".into(),
            acceptance: "verify the child's distinct result".into(),
            owner: owner.map(str::to_owned),
            depends_on: Vec::new(),
        };
        SplitParams {
            id: id.into(),
            expected_revision: work::get(s.conn(), id).unwrap().revision,
            coordination_decision_id: decision.into(),
            children: vec![
                child("Independent first part", Some("w-a2")),
                child("Independent second part", None),
            ],
        }
    }

    #[test]
    fn split_preserves_parent_and_requirements_and_rolls_back_whole_batch() {
        let mut s = store();
        let source = ok(&mut s, |tx| {
            crate::requirements::create(
                tx,
                &Sender::Human,
                crewd_proto::TodoCreateParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    crew_id: "crew-a".into(),
                    text: "Original requirement for coordination".into(),
                    channel_id: None,
                },
            )
        })
        .todo
        .requirement_id
        .unwrap();
        let parent = ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Session("w-a1".into()),
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "Original plan".into(),
                    level: crewd_proto::WorkLevel::Production,
                    sources: vec![source.clone()],
                    ..Default::default()
                },
            )
        })
        .id;
        let dependency = work(&mut s);
        let view = adopt(&mut s, &parent);
        let mut p = split_params(&s, &parent, &view.decision_id);
        p.children[0].depends_on.push(dependency.clone());
        let before = work::get(s.conn(), &parent).unwrap();
        let requirement = crate::requirements::get(s.conn(), &source).unwrap();
        let count = |s: &crewd_store::Store| {
            let work = crate::asks::load_org(s.conn()).unwrap();
            let wakes = wakes::list(s.conn(), None, 100).unwrap().len();
            (work.state().last_item, work.state().last_seq, wakes)
        };
        let initial = count(&s);
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", p.clone())).is_err());
        deliver(&mut s, view.wake_id.unwrap());
        let mut invalid = p.clone();
        invalid.children[1].owner = Some("w-b1".into());
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", invalid)).is_err());
        let mut invalid = p.clone();
        invalid.children[1].depends_on = vec![parent.clone()];
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", invalid)).is_err());
        s.conn().execute_batch("CREATE TRIGGER deny_split_receipt BEFORE INSERT ON meta WHEN NEW.key LIKE 'work.coordination.split.latest.v1:%' BEGIN SELECT RAISE(ABORT,'split receipt failure'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", p.clone())).is_err());
        assert_eq!(count(&s), initial);
        s.conn()
            .execute_batch("DROP TRIGGER deny_split_receipt")
            .unwrap();
        let result = ok(&mut s, |tx| apply_split(tx, "cap-a", p.clone()));
        assert_eq!(work::get(s.conn(), &parent).unwrap(), before);
        assert_eq!(
            crate::requirements::get(s.conn(), &source).unwrap(),
            requirement
        );
        for (index, row) in result.children.iter().enumerate() {
            let child = work::get(s.conn(), &row.id).unwrap();
            assert_eq!(child.parent.as_deref(), Some(parent.as_str()));
            assert_eq!(child.level, before.level);
            assert_eq!(child.status, "open");
            assert!(child.sources.is_empty());
            assert_eq!(child.owner, p.children[index].owner);
            assert_eq!(child.depends_on, p.children[index].depends_on);
        }
        assert_eq!(count(&s), (initial.0 + 2, initial.1 + 2, initial.2));
        let other = assignment(&s, &parent, "w-a2");
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &view.decision_id,
                other
            ))
            .is_err()
        );
        let read = status(s.conn(), "crew-a", &parent, Some(&view.decision_id)).unwrap();
        assert_eq!(read.last_split, Some(result.clone()));
        assert_eq!(read.requested_split, Some(result.clone()));
        assert!(read.requested_assignment.is_none());
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert_eq!(ok(&mut s, |tx| apply_split(tx, "cap-a", p.clone())), result);
        p.children[1].title.push_str(" changed");
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", p)).is_err());
        assert_eq!(count(&s), (initial.0 + 2, initial.1 + 2, initial.2));
    }

    #[test]
    fn old_assignment_receipt_precludes_split_and_dual_history_is_refused() {
        let mut s = store();
        let id = work(&mut s);
        let view = adopt(&mut s, &id);
        deliver(&mut s, view.wake_id.unwrap());
        let split = split_params(&s, &id, &view.decision_id);
        let p = assignment(&s, &id, "w-a2");
        let receipt = ok(&mut s, |tx| {
            assign(tx, "cap-a", &view.decision_id, p.clone())
        });
        assert!(crate::run(&mut s, NOW, |tx| apply_split(tx, "cap-a", split)).is_err());
        assert_eq!(
            ok(&mut s, |tx| assign(tx, "cap-a", &view.decision_id, p)),
            receipt
        );
        let key = format!("work.coordination.split.v1:{id}:{}", view.decision_id);
        meta_set(s.conn(), &key, "{}").unwrap();
        assert!(status(s.conn(), "crew-a", &id, Some(&view.decision_id)).is_err());
    }

    #[test]
    fn delivered_assignment_is_atomic_and_previous_result_survives_new_decision() {
        let mut s = store();
        let id = work(&mut s);
        let first = adopt(&mut s, &id);
        let p = assignment(&s, &id, "w-a2");
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                p.clone()
            ))
            .is_err()
        );
        deliver(&mut s, first.wake_id.unwrap());
        let wrong_crew = assignment(&s, &id, "w-b1");
        for (actor, params) in [
            ("w-a1", p.clone()),
            ("cap-b", p.clone()),
            ("cap-a", wrong_crew),
        ] {
            assert!(
                crate::run(&mut s, NOW, |tx| assign(
                    tx,
                    actor,
                    &first.decision_id,
                    params
                ))
                .is_err()
            );
        }
        s.conn().execute_batch("CREATE TRIGGER deny_coord_assignment BEFORE INSERT ON meta WHEN NEW.key LIKE 'work.coordination.assignment.latest.v1:%' BEGIN SELECT RAISE(ABORT,'receipt failure'); END;").unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                p.clone()
            ))
            .is_err()
        );
        assert_eq!(
            work::get(s.conn(), &id).unwrap().revision,
            p.expected_revision
        );
        let count = |s: &crewd_store::Store| {
            s.conn()
                .query_row(
                    "SELECT count(*) FROM wakes WHERE source LIKE 'work-assigned:%'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(count(&s), 0);
        s.conn()
            .execute_batch("DROP TRIGGER deny_coord_assignment")
            .unwrap();
        let one = ok(&mut s, |tx| {
            assign(tx, "cap-a", &first.decision_id, p.clone())
        });
        assert_eq!(one.previous_owner, "w-a1");
        assert_eq!(one.owner, "w-a2");
        let second = adopt(&mut s, &id);
        deliver(&mut s, second.wake_id.unwrap());
        let second_p = assignment(&s, &id, "w-a1");
        let two = ok(&mut s, |tx| {
            assign(tx, "cap-a", &second.decision_id, second_p)
        });
        assert_ne!(one.event_seq, two.event_seq);
        s.conn()
            .execute("DELETE FROM wakes WHERE id=?1", [first.wake_id.unwrap()])
            .unwrap();
        let read = status(s.conn(), "crew-a", &id, Some(&first.decision_id)).unwrap();
        assert_eq!(read.last_assignment, Some(two));
        assert_eq!(read.requested_assignment, Some(one.clone()));
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert_eq!(
            ok(&mut s, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                p.clone()
            )),
            one
        );
        let changed = crewd_proto::WorkAssignParams {
            note: "changed intent".into(),
            ..p
        };
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                changed
            ))
            .is_err()
        );
        assert_eq!(count(&s), 2);
        let key = format!("work.coordination.assignment.v1:{id}:{}", first.decision_id);
        let raw = meta_get(s.conn(), &key).unwrap().unwrap();
        let mut corrupt: serde_json::Value = serde_json::from_str(&raw).unwrap();
        corrupt["delivery"]["attempts"] = serde_json::json!(31);
        meta_set(s.conn(), &key, &corrupt.to_string()).unwrap();
        assert!(status(s.conn(), "crew-a", &id, Some(&first.decision_id)).is_err());
    }

    #[test]
    fn stop_restore_transfer_and_unrelated_wake_cannot_authorize_assignment() {
        let mut s = store();
        let id = work(&mut s);
        let first = adopt(&mut s, &id);
        let wake = first.wake_id.unwrap();
        deliver(&mut s, wake);
        s.conn()
            .execute("UPDATE wakes SET source='different' WHERE id=?1", [wake])
            .unwrap();
        assert_eq!(
            status(s.conn(), "crew-a", &id, None)
                .unwrap()
                .record
                .unwrap()
                .wake_state,
            None
        );
        let p = assignment(&s, &id, "w-a2");
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                p.clone()
            ))
            .is_err()
        );
        s.conn()
            .execute(
                "UPDATE wakes SET source=?1 WHERE id=?2",
                (wake_source(&id, &first.decision_id), wake),
            )
            .unwrap();
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &first.decision_id,
                p.clone()
            ))
            .is_err()
        );
        let second = adopt(&mut s, &id);
        deliver(&mut s, second.wake_id.unwrap());
        ok(&mut s, |tx| wakes::human_stop(tx, "cap-a"));
        assert!(
            crate::run(&mut s, NOW, |tx| assign(
                tx,
                "cap-a",
                &second.decision_id,
                p
            ))
            .is_err()
        );

        let mut s = store();
        let id = work(&mut s);
        let p = assignment(&s, &id, "w-a2");
        ok(&mut s, |tx| {
            crate::work_transfer::request(
                tx,
                &Sender::Session("w-a1".into()),
                crewd_proto::WorkTransferRequestParams {
                    id: id.clone(),
                    expected_revision: p.expected_revision,
                    to: p.owner,
                    note: p.note,
                },
            )
        });
        let third = adopt(&mut s, &id);
        deliver(&mut s, third.wake_id.unwrap());
        let p = assignment(&s, &id, "w-a2");
        assert!(crate::run(&mut s, NOW, |tx| assign(tx, "cap-a", &third.decision_id, p)).is_err());
        assert_eq!(
            work::get(s.conn(), &id).unwrap().owner.as_deref(),
            Some("w-a1")
        );
    }
    #[test]
    fn report_cursor_baselines_then_recovers_lost_events_and_resets_on_restore() {
        let mut s = store();
        let id = work(&mut s);
        report(&mut s, &id, "old one");
        report(&mut s, &id, "old two");
        ok(&mut s, observe_reports);
        assert!(record(s.conn(), &id).unwrap().is_none());
        report(&mut s, &id, "new report");
        ok(&mut s, observe_reports);
        assert!(matches!(
            record(s.conn(), &id).unwrap().unwrap().view.phase,
            CoordinationPhase::Prepared
        ));
        let before = meta_get(s.conn(), "work.coordination.reports.v1").unwrap();
        report(&mut s, &id, "lost while busy");
        s.conn().execute_batch("CREATE TRIGGER deny_coord_prepare BEFORE INSERT ON meta WHEN NEW.key LIKE 'work.coordination.v1:%' BEGIN SELECT RAISE(ABORT,'prepare failure'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, observe_reports).is_err());
        assert_eq!(
            meta_get(s.conn(), "work.coordination.reports.v1").unwrap(),
            before
        );
        s.conn()
            .execute_batch("DROP TRIGGER deny_coord_prepare")
            .unwrap();
        ok(&mut s, observe_reports);
        assert_ne!(
            meta_get(s.conn(), "work.coordination.reports.v1").unwrap(),
            before
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        report(&mut s, &id, "restored historical report");
        let old = record(s.conn(), &id).unwrap().unwrap().view.decision_id;
        ok(&mut s, observe_reports);
        assert_eq!(
            record(s.conn(), &id).unwrap().unwrap().view.decision_id,
            old
        );
    }
    #[test]
    fn queue_failure_rolls_back_adoption_and_duplicate_settle_cannot_enqueue_twice() {
        let mut s = store();
        let id = work(&mut s);
        report(&mut s, &id, "one");
        report(&mut s, &id, "two");
        let snap = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        s.conn().execute_batch("CREATE TRIGGER deny_coordination BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'queue failure'); END;").unwrap();
        assert!(
            crate::run(&mut s, NOW, |tx| settle(
                tx,
                &snap,
                Some(0.1),
                Some(0.9),
                true,
                CoordinationReason::Answered
            ))
            .is_err()
        );
        let v = status(s.conn(), "crew-a", &id, None)
            .unwrap()
            .record
            .unwrap();
        assert!(matches!(v.phase, CoordinationPhase::Dispatched));
        assert!(!v.adopted);
        assert!(v.wake_id.is_none());
        s.conn()
            .execute_batch("DROP TRIGGER deny_coordination")
            .unwrap();
        for _ in 0..2 {
            ok(&mut s, |tx| {
                settle(
                    tx,
                    &snap,
                    Some(0.1),
                    Some(0.9),
                    true,
                    CoordinationReason::Answered,
                )
            });
        }
        let n: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM wakes WHERE source LIKE 'work-coordination:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
    #[test]
    fn prepared_recovery_requeues_but_stopped_dispatch_never_proceeds() {
        let mut s = store();
        let id = work(&mut s);
        report(&mut s, &id, "one");
        report(&mut s, &id, "two");
        let prepared = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        let result = ok(&mut s, |tx| recover_page(tx, None));
        assert_eq!(result.0, 0);
        assert_eq!(result.1, [id.clone()]);
        assert_eq!(
            ok(&mut s, |tx| prepare(tx, &id)).unwrap().fingerprint,
            prepared.fingerprint
        );
        ok(&mut s, |tx| wakes::human_stop(tx, "cap-a"));
        let stopped = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        assert!(!ok(&mut s, |tx| dispatch(tx, &stopped)));
        let view = status(s.conn(), "crew-a", &id, None)
            .unwrap()
            .record
            .unwrap();
        assert!(matches!(view.phase, CoordinationPhase::Suppressed));
        assert!(view.wake_id.is_none());
    }
    #[test]
    fn actual_reports_deduplicate_settle_and_render_rechecks_current_revision() {
        let mut s = store();
        let id = work(&mut s);
        assert!(ok(&mut s, |tx| prepare(tx, &id)).is_none());
        assert!(matches!(
            status(s.conn(), "crew-a", &id, None)
                .unwrap()
                .record
                .unwrap()
                .reason,
            CoordinationReason::InsufficientReports
        ));
        report(&mut s, &id, "first attempt");
        assert!(ok(&mut s, |tx| prepare(tx, &id)).is_none());
        report(&mut s, &id, "second attempt");
        let snap = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        assert_eq!(snap.reports, ["first attempt", "second attempt"]);
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        ok(&mut s, |tx| {
            settle(
                tx,
                &snap,
                Some(0.2),
                Some(0.9),
                true,
                CoordinationReason::Answered,
            )
        });
        let view = status(s.conn(), "crew-a", &id, None)
            .unwrap()
            .record
            .unwrap();
        assert!(view.adopted);
        let row = wakes::by_ids(s.conn(), &[view.wake_id.unwrap()])
            .unwrap()
            .remove(0);
        assert!(matches!(
            render(s.conn(), &row).unwrap(),
            work::FollowupNote::Current(_)
        ));
        assert!(ok(&mut s, |tx| prepare(tx, &id)).is_none());
        report(&mut s, &id, "third actual change");
        assert!(matches!(
            render(s.conn(), &row).unwrap(),
            work::FollowupNote::Stale
        ));
    }
    #[test]
    fn recovery_never_redispatches_and_stale_answers_do_not_enqueue() {
        let mut s = store();
        let id = work(&mut s);
        report(&mut s, &id, "one");
        report(&mut s, &id, "two");
        let snap = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        assert_eq!(ok(&mut s, |tx| recover_page(tx, None)).0, 1);
        assert!(ok(&mut s, |tx| prepare(tx, &id)).is_none());
        ok(&mut s, |tx| {
            settle(
                tx,
                &snap,
                None,
                Some(1.0),
                true,
                CoordinationReason::Answered,
            )
        });
        assert!(
            status(s.conn(), "crew-a", &id, None)
                .unwrap()
                .record
                .unwrap()
                .wake_id
                .is_none()
        );
        report(&mut s, &id, "three");
        let snap = ok(&mut s, |tx| prepare(tx, &id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        report(&mut s, &id, "four");
        ok(&mut s, |tx| {
            settle(
                tx,
                &snap,
                None,
                Some(1.0),
                true,
                CoordinationReason::Answered,
            )
        });
        assert!(matches!(
            status(s.conn(), "crew-a", &id, None)
                .unwrap()
                .record
                .unwrap()
                .phase,
            CoordinationPhase::Stale
        ));
    }
}
