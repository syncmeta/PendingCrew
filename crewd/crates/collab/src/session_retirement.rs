//! Durable semantic decisions and exact actor-owned process retirement receipts.
use crate::{Result, Tx, judgments, wakes};
use crewd_proto::{RetirementPhase as Phase, RetirementReason as Reason, RetirementView};
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorWitness {
    pub incarnation: String,
    pub generation: u64,
    pub activity: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub session: String,
    pub crew: String,
    pub name: String,
    pub role: String,
    pub executor: String,
    pub summary: serde_json::Value,
    pub actor: ActorWitness,
    pub fingerprint: String,
    pub decision_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    snapshot: Snapshot,
    view: RetirementView,
    epoch: String,
    continuity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checksum: Option<String>,
}
fn key(id: &str) -> String {
    format!("session.retirement.v1:{id}")
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid retirement authority".into()).into()
}
fn record(c: &Connection, id: &str) -> Result<Option<Record>> {
    let r = meta_get(c, &key(id))?
        .map(|s| serde_json::from_str::<Record>(&s))
        .transpose()
        .map_err(StoreError::from)?;
    if let Some(value) = &r {
        validate_record(c, id, value)?;
    }
    Ok(r)
}
fn checksum(r: &Record) -> Result<String> {
    let mut content = r.clone();
    content.checksum = None;
    judgments::fingerprint(&content)
}
fn validate_record(c: &Connection, id: &str, r: &Record) -> Result<()> {
    if r.version != 1 || r.snapshot.session != id {
        return Err(invalid());
    }
    if let Some(stored) = &r.checksum {
        if *stored != checksum(r)? {
            return Err(invalid());
        }
    } else if crate::admin_case_execution::session_binding(c, id)?.is_some() {
        return Err(invalid());
    }
    Ok(())
}
fn save(tx: &mut Tx<'_>, r: &Record) -> Result<()> {
    let mut record = r.clone();
    record.checksum = Some(checksum(r)?);
    meta_set(
        tx.conn(),
        &key(&r.snapshot.session),
        &serde_json::to_string(&record).map_err(StoreError::from)?,
    )?;
    Ok(())
}

fn facts(
    c: &Connection,
    id: &str,
    now: i64,
) -> Result<Option<(crewd_proto::Session, serde_json::Value)>> {
    // An accepted custody obligation survives stale authority; it is discharged
    // only by verified business resolution, not by notification or process exit.
    if crate::admin_case_handoff::has_unresolved_custody(c, id, now)? {
        return Ok(None);
    }
    let administrative = if let Some(binding) = crate::admin_case_execution::session_binding(c, id)?
    {
        let outcome =
            crate::admin_case_outcome::status_at(c, binding.crew_id(), binding.case_id(), now)?;
        let Some(generation) = crate::admin_case_execution::generation(c, &binding)? else {
            return Ok(None);
        };
        if let Some(outcome) =
            outcome.filter(|o| o.phase == crewd_proto::AdminCaseOutcomePhase::Resolved)
        {
            Some(
                serde_json::json!({"caseId":binding.case_id(),"attemptId":binding.attempt_id(),"outcomeRevision":outcome.revision,"generation":generation,"targetCount":outcome.targets.len(),"resolvedByAdmin":outcome.resolved_by_admin.len(),"resolvedByOthers":outcome.resolved_by_others.len()}),
            )
        } else if let Some(handoff) = crate::admin_case_handoff::current_accepted(
            c,
            binding.crew_id(),
            binding.case_id(),
            now,
        )? {
            Some(
                serde_json::json!({"caseId":binding.case_id(),"attemptId":binding.attempt_id(),"generation":generation,"releaseReason":"handed_over","handoffRevision":handoff.revision,"requestId":handoff.request_id,"captainSessionId":handoff.captain_session_id,"targetCount":handoff.targets.len()}),
            )
        } else {
            return Ok(None);
        }
    } else {
        None
    };
    let s = crate::sessions::get(c, id)?;
    if s.desired != crewd_proto::Desired::Running
        || s.was_working
        || s.resume_failed_at.is_some()
        || s.awaiting_question.is_some()
        || s.agent_session_id
            .as_ref()
            .is_none_or(|v| v.trim().is_empty())
        || crate::crews::is_archived(c, &s.crew_id)?
        || wakes::automatic_work_paused(c, id)?
    {
        return Ok(None);
    }
    let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM wakes WHERE session_id=?1 AND state IN ('pending','inflight'))",[id],|r|r.get(0))?;
    if pending {
        return Ok(None);
    }
    let book = crate::asks::load_org(c)?;
    if book.state().items.values().any(|i| {
        (i.owner.as_ref().is_some_and(|o| o.as_str() == id)
            || i.pending_transfer
                .as_ref()
                .is_some_and(|t| t.to.as_str() == id || t.requested_by.as_str() == id))
            && !matches!(
                i.status,
                crewd_work::Status::Done | crewd_work::Status::Dropped
            )
    }) || book.state().asks.values().any(|a| {
        a.asked_by.session().is_some_and(|v| v.as_str() == id)
            && matches!(a.state, crewd_work::AskState::Open)
    }) {
        return Ok(None);
    }
    // Include the entire visible source frontier, including committed input that has no derived wake yet.
    let channels = crate::channels::readable_channels(c, id)?;
    let encoded = serde_json::to_string(&channels).map_err(StoreError::from)?;
    let mut q=c.prepare("SELECT seq,sender_session_id,text FROM messages WHERE channel_id IN (SELECT value FROM json_each(?1)) ORDER BY seq DESC LIMIT 7")?;
    let recent = q
        .query_map([&encoded], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let frontier = recent.first().map(|r| r.0).unwrap_or(0);
    let sample:Vec<_>=recent.iter().take(6).map(|(_,from,text)|serde_json::json!({"from_self":from.as_deref()==Some(id),"text":text.chars().take(600).collect::<String>()})).collect();
    let wakes: i64 = c.query_row(
        "SELECT coalesce(max(id),0) FROM wakes WHERE session_id=?1",
        [id],
        |r| r.get(0),
    )?;
    let owned: Vec<_> = book
        .state()
        .items
        .values()
        .filter(|i| i.owner.as_ref().is_some_and(|o| o.as_str() == id))
        .collect();
    let asked: Vec<_> = book
        .state()
        .asks
        .values()
        .filter(|a| a.asked_by.session().is_some_and(|o| o.as_str() == id))
        .collect();
    let work = judgments::fingerprint(&(&owned, &asked))?;
    let mut context = serde_json::json!({"session":s,"work":work,"messages":frontier,"channels":channels,"recent":sample,"truncated":recent.len()>6 || recent.iter().take(6).any(|r|r.2.chars().count()>600),"wakes":wakes,"config":judgments::configuration(c)?,"epoch":crewd_store::judgment_state::epoch(c)?,"stopFloor":wakes::human_stop_floor(c,id)?,"stopEpoch":judgments::stop_epoch(c,id)?});
    if let Some(administrative) = administrative {
        context["adminCase"] = administrative;
    }
    Ok(Some((s, context)))
}
fn continuity(c: &Connection, id: &str, now: i64) -> Result<Option<String>> {
    let Some((_, mut f)) = facts(c, id, now)? else {
        return Ok(None);
    };
    if let Some(o) = f["session"].as_object_mut() {
        o.remove("updatedAt");
        o.remove("lastActiveAt");
        o.remove("wasWorking");
    }
    Ok(Some(judgments::fingerprint(&f)?))
}
fn snapshot(c: &Connection, id: &str, actor: ActorWitness, now: i64) -> Result<Option<Snapshot>> {
    let Some((s, facts)) = facts(c, id, now)? else {
        return Ok(None);
    };
    if let Some(generation) = facts["adminCase"]["generation"].as_u64()
        && generation != actor.generation
    {
        return Ok(None);
    }
    let fingerprint = judgments::fingerprint(&(&facts, &actor))?;
    let summary = if !facts["adminCase"].is_null() {
        serde_json::json!({"role":s.role,"verifiedCaseOutcome":if facts["adminCase"]["releaseReason"]=="handed_over" {"handed_over_unresolved"} else {"resolved"},"targetCount":facts["adminCase"]["targetCount"],"resolvedByAdmin":facts["adminCase"]["resolvedByAdmin"],"resolvedByOthers":facts["adminCase"]["resolvedByOthers"],"idle":true,"pendingWork":false,"pendingWake":false,"pendingAskOrApproval":false,"resumableIdentity":true,"evidenceScope":if facts["adminCase"]["releaseReason"]=="handed_over" {"verified captain custody and current session obligations; work remains unresolved; physical exit has not been requested"} else {"verified case outcome and current session obligations; physical exit has not been requested"}})
    } else {
        serde_json::json!({"role":s.role,"title":s.title.as_ref().map(|v|v.chars().take(120).collect::<String>()),"brief":s.brief.as_ref().map(|v|v.chars().take(400).collect::<String>()),"recent":facts["recent"],"sample_only":true,"truncated":facts["truncated"],"text_truncated":s.title.as_ref().is_some_and(|v|v.chars().count()>120)||s.brief.as_ref().is_some_and(|v|v.chars().count()>400),"idle":true,"pendingWork":false,"pendingWake":false,"pendingAskOrApproval":false,"resumableIdentity":true,"evidenceScope":"current session facts; no claim that conversation history is exhaustive"})
    };
    Ok(Some(Snapshot {
        session: id.into(),
        crew: s.crew_id,
        name: s
            .display_name
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect(),
        role: s.role.as_str().into(),
        executor: s.executor,
        summary,
        actor,
        fingerprint,
        decision_id: String::new(),
    }))
}
fn current(c: &Connection, s: &Snapshot, now: i64) -> Result<bool> {
    Ok(snapshot(c, &s.session, s.actor.clone(), now)?
        .is_some_and(|v| v.fingerprint == s.fingerprint))
}
pub fn prepare(tx: &mut Tx<'_>, id: &str, actor: ActorWitness) -> Result<Option<Snapshot>> {
    let Some(mut s) = snapshot(tx.conn(), id, actor, tx.now_ms())? else {
        return Ok(None);
    };
    if let Some(r) = record(tx.conn(), id)?
        && r.snapshot.fingerprint == s.fingerprint
    {
        return Ok(matches!(r.view.phase, Phase::Prepared).then_some(r.snapshot));
    }
    s.decision_id = uuid::Uuid::new_v4().to_string();
    let r = Record {
        version: 1,
        checksum: None,
        continuity: continuity(tx.conn(), id, tx.now_ms())?.ok_or_else(invalid)?,
        epoch: crewd_store::judgment_state::epoch(tx.conn())?,
        snapshot: s.clone(),
        view: RetirementView {
            decision_id: s.decision_id.clone(),
            phase: Phase::Prepared,
            reason: Reason::Prepared,
            probability: None,
            adopted: false,
            generation: s.actor.generation.to_string(),
        },
    };
    save(tx, &r)?;
    Ok(Some(s))
}
pub fn dispatch(tx: &mut Tx<'_>, s: &Snapshot) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), &s.session)? else {
        return Ok(false);
    };
    if r.snapshot.decision_id != s.decision_id || r.view.phase != Phase::Prepared {
        return Ok(false);
    }
    if !current(tx.conn(), s, tx.now_ms())? {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
        save(tx, &r)?;
        return Ok(false);
    }
    r.view.phase = Phase::Dispatched;
    save(tx, &r)?;
    Ok(true)
}
pub fn settle(
    tx: &mut Tx<'_>,
    s: &Snapshot,
    probability: Option<f64>,
    approved: bool,
    reason: Reason,
) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), &s.session)? else {
        return Ok(false);
    };
    if r.snapshot.decision_id != s.decision_id || r.view.phase != Phase::Dispatched {
        return Ok(false);
    }
    r.view.probability = probability;
    r.view.reason = reason;
    if !current(tx.conn(), s, tx.now_ms())? {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
        save(tx, &r)?;
        return Ok(false);
    }
    r.view.phase = if approved {
        Phase::DecisionReady
    } else {
        Phase::Suppressed
    };
    save(tx, &r)?;
    Ok(approved)
}
/// The actor calls this only after comparing its exact live witness. Commit is the retirement linearization point.
pub fn accept(tx: &mut Tx<'_>, s: &Snapshot) -> Result<bool> {
    let Some(mut r) = record(tx.conn(), &s.session)? else {
        return Ok(false);
    };
    if r.snapshot.decision_id != s.decision_id || r.view.phase != Phase::DecisionReady {
        return Ok(false);
    }
    if !current(tx.conn(), s, tx.now_ms())? {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
        save(tx, &r)?;
        return Ok(false);
    }
    r.view.phase = Phase::ExitRequested;
    save(tx, &r)?;
    Ok(true)
}
pub fn stale(tx: &mut Tx<'_>, s: &Snapshot) -> Result<()> {
    if let Some(mut r) = record(tx.conn(), &s.session)?
        && r.snapshot.decision_id == s.decision_id
        && matches!(r.view.phase, Phase::DecisionReady | Phase::Prepared)
    {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
        save(tx, &r)?;
    }
    Ok(())
}
pub fn confirm_exit(tx: &mut Tx<'_>, s: &Snapshot, confirmed: bool) -> Result<()> {
    let Some(mut r) = record(tx.conn(), &s.session)? else {
        return Ok(());
    };
    if r.snapshot.decision_id != s.decision_id
        || r.snapshot.actor != s.actor
        || r.view.phase != Phase::ExitRequested
    {
        return Ok(());
    }
    r.view.phase = if confirmed {
        Phase::Parked
    } else {
        Phase::Unknown
    };
    r.view.reason = if confirmed {
        Reason::Answered
    } else {
        Reason::ExitUnknown
    };
    r.view.adopted = confirmed;
    save(tx, &r)
}
/// Successful retirement is not a crash: keep it out of boot's Human restart gate.
/// A later ordinary wake still enters the existing admission and resume pipeline.
pub fn parked(c: &Connection, id: &str) -> Result<bool> {
    Ok(record(c, id).unwrap_or(None).is_some_and(|r| {
        r.view.phase == Phase::Parked
            && crewd_store::judgment_state::epoch(c).is_ok_and(|e| e == r.epoch)
    }))
}
pub fn status_at(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<crewd_proto::SessionRetirementStatus> {
    let s = crate::sessions::get(c, id)?;
    if s.crew_id != crew {
        return Err(invalid());
    }
    let mut r = record(c, id)?;
    if let Some(r) = &mut r
        && (crewd_store::judgment_state::epoch(c)? != r.epoch
            || (r.view.phase == Phase::Parked
                && continuity(c, id, now)?.as_deref() != Some(r.continuity.as_str()))
            || (!matches!(r.view.phase, Phase::Parked | Phase::Unknown)
                && !current(c, &r.snapshot, now)?))
    {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
    }
    Ok(crewd_proto::SessionRetirementStatus {
        crew_id: crew.into(),
        session_id: id.into(),
        record: r.map(|r| r.view),
    })
}
pub fn resumed(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    // Corrupt optional judgments confer no boot exemption and cannot revoke a
    // Human's original resume authority. Actual reads/writes still fail closed.
    let Some(raw) = meta_get(tx.conn(), &key(id))? else {
        return Ok(());
    };
    let Ok(mut r) = serde_json::from_str::<Record>(&raw) else {
        return Ok(());
    };
    if validate_record(tx.conn(), id, &r).is_err() {
        return Ok(());
    }
    if matches!(r.view.phase, Phase::Parked | Phase::ExitRequested) {
        r.view.phase = Phase::Stale;
        r.view.reason = Reason::StateChanged;
        save(tx, &r)?;
    }
    Ok(())
}
pub fn recover_page(
    tx: &mut Tx<'_>,
    after: Option<String>,
) -> Result<(usize, Vec<String>, Option<String>)> {
    let mut rows = {
        let mut q=tx.conn().prepare("SELECT key,value FROM meta WHERE key LIKE 'session.retirement.v1:%' AND key>?1 ORDER BY key LIMIT 65")?;
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
    for (k, raw) in rows {
        let Ok(mut r) = serde_json::from_str::<Record>(&raw) else {
            eprintln!("crewd: unreadable retirement record retained during recovery");
            continue;
        };
        if r.version != 1 || k != key(&r.snapshot.session) {
            eprintln!("crewd: unsupported retirement record retained during recovery");
            continue;
        }
        if validate_record(tx.conn(), &r.snapshot.session, &r).is_err() {
            eprintln!("crewd: invalid retirement checksum retained during recovery");
            continue;
        }
        if matches!(r.view.phase, Phase::Dispatched | Phase::ExitRequested) {
            r.view.phase = Phase::Unknown;
            r.view.reason = Reason::RestartUnknown;
            save(tx, &r)?;
            count += 1;
        } else if matches!(r.view.phase, Phase::Prepared | Phase::DecisionReady) {
            r.view.phase = Phase::Stale;
            r.view.reason = Reason::StateChanged;
            save(tx, &r)?;
        }
    }
    Ok((count, Vec::new(), next))
}

pub fn status(
    c: &Connection,
    crew: &str,
    id: &str,
) -> Result<crewd_proto::SessionRetirementStatus> {
    status_at(c, crew, id, crewd_store::now_ms())
}

/// A case may display only the retirement receipt for its exact bound physical generation.
pub(crate) fn case_record(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<RetirementView>> {
    let Some(binding) = crate::admin_case_execution::binding(c, crew, id)? else {
        return Ok(None);
    };
    let Some(generation) = crate::admin_case_execution::generation(c, &binding)? else {
        return Ok(None);
    };
    let Some(raw) = record(c, binding.session_id())? else {
        return Ok(None);
    };
    if raw.snapshot.actor.generation != generation || raw.view.generation != generation.to_string()
    {
        return Ok(None);
    }
    let mut status = status_at(c, crew, binding.session_id(), now)?.record;
    let resolved = crate::admin_case_outcome::status_at(c, crew, id, now)?
        .is_some_and(|o| o.phase == crewd_proto::AdminCaseOutcomePhase::Resolved);
    let handed_over = crate::admin_case_handoff::current_accepted(c, crew, id, now)?.is_some();
    if !resolved
        && !handed_over
        && let Some(r) = &mut status
    {
        r.phase = Phase::Stale;
        r.reason = Reason::StateChanged;
        r.adopted = false;
    }
    Ok(status)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};
    fn resolved_admin(
        s: &mut crewd_store::Store,
    ) -> (
        crewd_proto::AdminCaseView,
        crate::admin_case_execution::Registration,
    ) {
        use crate::admin_case_execution::{
            self as execution, ActionScope,
            tests::{close, running_case},
        };
        let (case, reg) = running_case(s);
        s.conn()
            .execute(
                "UPDATE sessions SET agent_session_id='synthetic-resume' WHERE id=?1",
                [reg.witness.session_id()],
            )
            .unwrap();
        for target in &case.targets {
            ok(s, |tx| {
                let permit = execution::validate_effect(
                    tx,
                    &reg.witness,
                    ActionScope::WorkClose {
                        id: target.id.clone(),
                        kind: crewd_proto::WorkClosureKind::Done,
                    },
                )?;
                close(tx, reg.witness.session_id(), &target.id)?;
                execution::finish_effect(tx, permit)
            });
        }
        ok(s, |tx| {
            crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
        });
        (case, reg)
    }
    #[test]
    fn resolved_bound_case_can_enter_existing_semantic_retirement() {
        let mut s = store();
        let (_case, reg) = resolved_admin(&mut s);
        let actor = ActorWitness {
            incarnation: "synthetic-case-core".into(),
            generation: 1,
            activity: 0,
        };
        assert!(ok(&mut s, |tx| prepare(tx, &reg.session.id, actor)).is_some());
    }
    fn witness() -> ActorWitness {
        ActorWitness {
            incarnation: "test-boot".into(),
            generation: 1,
            activity: 0,
        }
    }
    #[test]
    fn legacy_ordinary_parked_without_admin_case_context_remains_parked() {
        let mut db = store();
        let snapshot = ready(&mut db);
        assert!(ok(&mut db, |tx| accept(tx, &snapshot)));
        ok(&mut db, |tx| confirm_exit(tx, &snapshot, true));
        let (_, mut legacy) = facts(db.conn(), "w-a1", crate::testkit::NOW)
            .unwrap()
            .unwrap();
        legacy.as_object_mut().unwrap().remove("adminCase");
        for field in ["updatedAt", "lastActiveAt", "wasWorking"] {
            legacy["session"].as_object_mut().unwrap().remove(field);
        }
        // This is the old persisted continuity shape, intentionally lacking the new key.
        let mut saved = record(db.conn(), "w-a1").unwrap().unwrap();
        saved.continuity = judgments::fingerprint(&legacy).unwrap();
        saved.checksum = None;
        let raw = serde_json::to_string(&saved).unwrap();
        meta_set(db.conn(), &key("w-a1"), &raw).unwrap();
        assert_eq!(
            status_at(db.conn(), "crew-a", "w-a1", crate::testkit::NOW)
                .unwrap()
                .record
                .unwrap()
                .phase,
            Phase::Parked
        );
        assert_eq!(meta_get(db.conn(), &key("w-a1")).unwrap().unwrap(), raw);
    }
    #[test]
    fn bound_retirement_checks_true_generation_and_preserves_idle_obligations() {
        let mut s = store();
        let (case, reg) = resolved_admin(&mut s);
        let mut actor = witness();
        actor.generation = 2;
        assert!(ok(&mut s, |tx| prepare(tx, &reg.session.id, actor)).is_none());
        let prepared = ok(&mut s, |tx| prepare(tx, &reg.session.id, witness())).unwrap();
        let payload = prepared.summary.to_string();
        assert!(!payload.contains(&case.id));
        assert!(!payload.contains(&reg.session.id));
        assert!(!payload.contains("attemptId"));
        assert!(!payload.contains("outcomeRevision"));
        assert_eq!(prepared.summary["verifiedCaseOutcome"], "resolved");
        assert_eq!(prepared.summary["targetCount"], 2);
        ok(&mut s, |tx| {
            crate::wakes::enqueue(
                tx,
                crate::wakes::WakeRequest {
                    session: reg.session.id.clone(),
                    kind: crate::wakes::WakeKind::System,
                    source: "synthetic-pending-obligation".into(),
                    priority: crate::wakes::WakePriority::System,
                    not_before_ms: None,
                    note: Some("pending".into()),
                    origin: None,
                },
            )
        });
        assert!(ok(&mut s, |tx| prepare(tx, &reg.session.id, witness())).is_none());
        assert!(!ok(&mut s, |tx| dispatch(tx, &prepared)));
    }
    #[test]
    fn case_projection_distinguishes_requested_unknown_and_actual_receipt_generation() {
        let mut s = store();
        let (case, reg) = resolved_admin(&mut s);
        let prepared = ok(&mut s, |tx| prepare(tx, &reg.session.id, witness())).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &prepared)));
        assert!(ok(&mut s, |tx| settle(
            tx,
            &prepared,
            Some(0.99),
            true,
            Reason::Answered
        )));
        assert!(ok(&mut s, |tx| accept(tx, &prepared)));
        let params = crewd_proto::AdminCaseGetParams {
            crew_id: "crew-a".into(),
            id: case.id.clone(),
        };
        let requested = crate::admin_cases::get_at(s.conn(), &params, crate::testkit::NOW).unwrap();
        assert_eq!(
            requested.retirement,
            crewd_proto::AdminCaseRetirement::ExitRequested
        );
        ok(&mut s, |tx| confirm_exit(tx, &prepared, false));
        assert_eq!(
            crate::admin_cases::get_at(s.conn(), &params, crate::testkit::NOW)
                .unwrap()
                .retirement,
            crewd_proto::AdminCaseRetirement::Unknown
        );
        let mut wrong = record(s.conn(), &reg.session.id).unwrap().unwrap();
        wrong.snapshot.actor.generation = 2;
        wrong.view.generation = "2".into();
        ok(&mut s, |tx| save(tx, &wrong));
        let view = crate::admin_cases::get_at(s.conn(), &params, crate::testkit::NOW).unwrap();
        assert!(view.retirement_record.is_none());
        assert_eq!(
            view.retirement,
            crewd_proto::AdminCaseRetirement::NotStarted
        );
    }
    #[test]
    fn modified_ready_or_requested_receipt_cannot_forge_parked() {
        for request in [false, true] {
            let mut db = store();
            let (case, reg) = resolved_admin(&mut db);
            let prepared = ok(&mut db, |tx| prepare(tx, &reg.session.id, witness())).unwrap();
            assert!(ok(&mut db, |tx| dispatch(tx, &prepared)));
            assert!(ok(&mut db, |tx| settle(
                tx,
                &prepared,
                Some(0.99),
                true,
                Reason::Answered
            )));
            if request {
                assert!(ok(&mut db, |tx| accept(tx, &prepared)));
            }
            let raw = meta_get(db.conn(), &key(&reg.session.id)).unwrap().unwrap();
            let mut broken: serde_json::Value = serde_json::from_str(&raw).unwrap();
            broken["view"]["phase"] = "parked".into();
            let broken = broken.to_string();
            meta_set(db.conn(), &key(&reg.session.id), &broken).unwrap();
            assert!(status_at(db.conn(), "crew-a", &reg.session.id, crate::testkit::NOW).is_err());
            assert!(!parked(db.conn(), &reg.session.id).unwrap());
            assert!(
                crate::admin_cases::get_at(
                    db.conn(),
                    &crewd_proto::AdminCaseGetParams {
                        crew_id: "crew-a".into(),
                        id: case.id
                    },
                    crate::testkit::NOW
                )
                .is_err()
            );
            ok(&mut db, |tx| recover_page(tx, None));
            assert_eq!(
                meta_get(db.conn(), &key(&reg.session.id)).unwrap().unwrap(),
                broken
            );
        }
    }
    #[test]
    fn eligible_idle_session_prepares_a_judgment() {
        let mut db = store();
        db.conn()
            .execute(
                "UPDATE sessions SET desired='running',agent_session_id='retained' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        assert!(ok(&mut db, |tx| prepare(tx, "w-a1", witness())).is_some());
    }

    fn ready(db: &mut crewd_store::Store) -> Snapshot {
        db.conn()
            .execute(
                "UPDATE sessions SET desired='running',agent_session_id='retained' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        let s = ok(db, |tx| prepare(tx, "w-a1", witness())).unwrap();
        assert!(ok(db, |tx| dispatch(tx, &s)));
        assert!(ok(db, |tx| settle(
            tx,
            &s,
            Some(0.99),
            true,
            Reason::Answered
        )));
        s
    }
    #[test]
    fn request_and_confirm_are_distinct_and_preserve_identity() {
        let mut db = store();
        let s = ready(&mut db);
        assert!(ok(&mut db, |tx| accept(tx, &s)));
        let v = status(db.conn(), "crew-a", "w-a1").unwrap().record.unwrap();
        assert_eq!(v.phase, Phase::ExitRequested);
        assert!(!v.adopted);
        ok(&mut db, |tx| confirm_exit(tx, &s, true));
        let v = status(db.conn(), "crew-a", "w-a1").unwrap().record.unwrap();
        assert_eq!(v.phase, Phase::Parked);
        assert!(v.adopted);
        assert!(parked(db.conn(), "w-a1").unwrap());
        let row = crate::sessions::get(db.conn(), "w-a1").unwrap();
        assert_eq!(row.desired, crewd_proto::Desired::Running);
        assert_eq!(row.agent_session_id.as_deref(), Some("retained"));
        assert!(!wakes::automatic_work_paused(db.conn(), "w-a1").unwrap());
        ok(&mut db, |tx| resumed(tx, "w-a1"));
        assert!(!parked(db.conn(), "w-a1").unwrap());
        assert!(
            status(db.conn(), "crew-a", "w-a1")
                .unwrap()
                .record
                .unwrap()
                .adopted
        );
    }
    #[test]
    fn pending_input_between_answer_and_accept_invalidates() {
        let mut db = store();
        let s = ready(&mut db);
        ok(&mut db, |tx| {
            wakes::enqueue(
                tx,
                wakes::WakeRequest {
                    session: "w-a1".into(),
                    kind: wakes::WakeKind::System,
                    source: "new-real-input".into(),
                    priority: wakes::WakePriority::System,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
            )
        });
        assert!(!ok(&mut db, |tx| accept(tx, &s)));
        assert_eq!(
            record(db.conn(), "w-a1").unwrap().unwrap().view.phase,
            Phase::Stale
        );
    }
    #[test]
    fn restart_keeps_unknown_and_skips_bad_record_without_overwriting() {
        let mut db = store();
        let s = ready(&mut db);
        assert!(ok(&mut db, |tx| accept(tx, &s)));
        meta_set(db.conn(), "session.retirement.v1:bad", "not json").unwrap();
        assert_eq!(ok(&mut db, |tx| recover_page(tx, None)).0, 1);
        assert_eq!(
            meta_get(db.conn(), "session.retirement.v1:bad")
                .unwrap()
                .as_deref(),
            Some("not json")
        );
        let r = record(db.conn(), "w-a1").unwrap().unwrap();
        assert_eq!(r.view.phase, Phase::Unknown);
        assert!(!r.view.adopted);
        ok(&mut db, |tx| confirm_exit(tx, &s, true));
        assert_eq!(
            record(db.conn(), "w-a1").unwrap().unwrap().view.phase,
            Phase::Unknown
        );
    }
    #[test]
    fn no_resumable_identity_or_work_obligation_means_no_candidate() {
        let mut db = store();
        db.conn()
            .execute(
                "UPDATE sessions SET desired='running',agent_session_id=NULL WHERE id='w-a1'",
                [],
            )
            .unwrap();
        assert!(ok(&mut db, |tx| prepare(tx, "w-a1", witness())).is_none());
        db.conn()
            .execute(
                "UPDATE sessions SET agent_session_id='retained' WHERE id='w-a1'",
                [],
            )
            .unwrap();
        ok(&mut db, |tx| {
            crate::work::create(
                tx,
                &crate::Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "live obligation".into(),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        assert!(ok(&mut db, |tx| prepare(tx, "w-a1", witness())).is_none());
    }

    #[test]
    fn corrupt_optional_record_does_not_deny_resume_but_real_write_failure_does() {
        let mut db = store();
        meta_set(db.conn(), &key("w-a1"), "corrupt retained").unwrap();
        ok(&mut db, |tx| resumed(tx, "w-a1"));
        assert_eq!(
            meta_get(db.conn(), &key("w-a1")).unwrap().as_deref(),
            Some("corrupt retained")
        );
        assert!(!parked(db.conn(), "w-a1").unwrap());
        db.conn()
            .execute("DELETE FROM meta WHERE key=?1", [key("w-a1")])
            .unwrap();
        let s = ready(&mut db);
        assert!(ok(&mut db, |tx| accept(tx, &s)));
        ok(&mut db, |tx| confirm_exit(tx, &s, true));
        db.conn().execute_batch("CREATE TRIGGER refuse_retirement_invalidation BEFORE UPDATE ON meta WHEN NEW.key='session.retirement.v1:w-a1' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        let _ = crate::testkit::err(&mut db, |tx| resumed(tx, "w-a1"));
        assert!(parked(db.conn(), "w-a1").unwrap());
        db.conn()
            .execute_batch("DROP TRIGGER refuse_retirement_invalidation")
            .unwrap();
        ok(&mut db, |tx| resumed(tx, "w-a1"));
        assert!(!parked(db.conn(), "w-a1").unwrap());
    }
    #[test]
    fn committed_visible_input_invalidates_before_derived_wakes_and_samples_are_bounded() {
        let mut db = store();
        let s = ready(&mut db);
        ok(&mut db, |tx| {
            crate::messages::announce_to_human(
                tx,
                "crew-x",
                crate::Sender::System,
                "other crew private evidence".into(),
                "retirement_fixture",
            )
        });
        assert!(current(db.conn(), &s, crate::testkit::NOW).unwrap());
        ok(&mut db, |tx| {
            crate::messages::announce_to_human(
                tx,
                "crew-a",
                crate::Sender::Human,
                "v".repeat(650),
                "retirement_fixture",
            )
        });
        assert!(!current(db.conn(), &s, crate::testkit::NOW).unwrap());
        assert!(!ok(&mut db, |tx| accept(tx, &s)));
        let fresh = ok(&mut db, |tx| prepare(tx, "w-a1", witness())).unwrap();
        assert_eq!(fresh.summary["truncated"], true);
        assert!(
            !fresh
                .summary
                .to_string()
                .contains("other crew private evidence")
        );
    }
}
