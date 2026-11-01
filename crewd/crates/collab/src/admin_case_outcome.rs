//! Verified business outcomes are independent from executor retirement.
use crate::{Result, Tx, admin_case_execution as execution, admin_cases, judgments};
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set, object_refs::ObjectKind};
use crewd_work::{Event, EventKind, Status, WorkId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    attempt: String,
    fingerprint: String,
    view_checksum: String,
    view: AdminCaseOutcomeView,
}
fn key(id: &str) -> String {
    format!("admin.case.outcome.v1:{id}")
}
fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid administrative outcome evidence".into()).into()
}
fn read(c: &Connection, crew: &str, id: &str) -> Result<Option<Saved>> {
    let Some(raw) = meta_get(c, &key(id))? else {
        return Ok(None);
    };
    let s: Saved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if s.version != 1
        || s.view.case_id != id
        || s.view.crew_id != crew
        || s.view.revision != s.fingerprint
        || s.attempt.is_empty()
        || judgments::fingerprint(&s.view)? != s.view_checksum
    {
        return Err(invalid());
    }
    Ok(Some(s))
}
fn latest_event(c: &Connection, crew: &str, id: WorkId) -> Result<Option<Event>> {
    let row=c.query_row("SELECT seq,body FROM organization_work_events WHERE crew_id=?1 AND json_extract(body,'$.kind.id')=?2 AND json_extract(body,'$.kind.type') IN ('done','dropped','owner_changed') ORDER BY seq DESC LIMIT 1",params![crew,id.to_string()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()?;
    row.map(|(seq,raw)| {
  let event:Event=serde_json::from_str(&raw).map_err(StoreError::from)?;
  if seq<=0 || event.seq!=seq as u64 || !matches!(&event.kind,EventKind::Done{id:i,..}|EventKind::Dropped{id:i,..}|EventKind::OwnerChanged{id:i,..} if *i==id){return Err(invalid())}Ok(event)
 }).transpose()
}
fn verified_receipt<'a>(
    c: &Connection,
    receipts: &'a [execution::EffectReceipt],
    w: &execution::ExecutionWitness,
    id: &str,
    event: &Event,
) -> Result<Option<&'a execution::EffectReceipt>> {
    for receipt in receipts
        .iter()
        .filter(|r| r.work_id == id && r.first_event <= event.seq && r.last_event >= event.seq)
    {
        let mut q = c.prepare(
            "SELECT seq,body FROM organization_work_events WHERE seq>=?1 AND seq<=?2 ORDER BY seq",
        )?;
        let rows = q
            .query_map(
                params![
                    i64::try_from(receipt.first_event).map_err(|_| invalid())?,
                    i64::try_from(receipt.last_event).map_err(|_| invalid())?
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() as u64 != receipt.last_event - receipt.first_event + 1 {
            continue;
        }
        let mut valid = true;
        for (offset, (seq, raw)) in rows.iter().enumerate() {
            let actual: Event = serde_json::from_str(raw).map_err(StoreError::from)?;
            if *seq <= 0
                || actual.seq != *seq as u64
                || actual.seq != receipt.first_event + offset as u64
                || actual.by.session().map(|s| s.as_str()) != Some(w.session_id())
            {
                valid = false;
                break;
            }
        }
        if valid {
            return Ok(Some(receipt));
        }
    }
    Ok(None)
}
fn matches_kind(kind: &AdminCaseTargetKind, value: crewd_work::StopKind) -> bool {
    matches!(
        (kind, value),
        (
            AdminCaseTargetKind::Unanswered,
            crewd_work::StopKind::Unanswered
        ) | (AdminCaseTargetKind::Overdue, crewd_work::StopKind::Overdue)
            | (
                AdminCaseTargetKind::ReadyUnscheduled,
                crewd_work::StopKind::ReadyUnscheduled
            )
    )
}
fn target(
    c: &Connection,
    book: &crewd_work::WorkBook,
    w: &execution::ExecutionWitness,
    id: &str,
    kinds: Vec<AdminCaseTargetKind>,
    receipts: &[execution::EffectReceipt],
    now: i64,
) -> Result<(AdminCaseTargetOutcome, serde_json::Value)> {
    use AdminCaseTargetOutcomeReason as R;
    use AdminCaseTargetOutcomeState as S;
    let mut result = AdminCaseTargetOutcome {
        id: id.into(),
        source_kinds: kinds,
        state: S::Pending,
        resolution: None,
        resolved_by: None,
        actor_session_id: None,
        event_seq: None,
        wake_id: None,
        reason: R::OriginalProblemRemains,
    };
    let Some(reference) =
        crewd_store::object_refs::get(c, ObjectKind::Plan, crate::work::number(id)?)?
    else {
        result.state = S::Missing;
        result.reason = R::TargetMissing;
        return Ok((result, serde_json::Value::Null));
    };
    if reference.crew_id != w.crew_id() {
        return Err(invalid());
    }
    let core = crate::work::core_plan(c, &reference)?;
    let Some(item) = book.item(core) else {
        result.state = S::Missing;
        result.reason = R::TargetMissing;
        return Ok((result, serde_json::Value::Null));
    };
    let event = latest_event(c, w.crew_id(), core)?;
    let mut facts = serde_json::json!({"item":item,"event":event});
    let Some(event) = event else {
        if matches!(item.status, Status::Done | Status::Dropped) {
            result.state = S::Blocked;
            result.reason = R::NoMatchingEvent;
        }
        return Ok((result, facts));
    };
    result.actor_session_id = event.by.session().map(|s| s.as_str().into());
    result.event_seq = Some(event.seq.to_string());
    let by_admin = result.actor_session_id.as_deref() == Some(w.session_id());
    let receipt = if by_admin {
        verified_receipt(c, receipts, w, id, &event)?
    } else {
        None
    };
    if by_admin && receipt.is_none() {
        result.state = S::Blocked;
        result.reason = R::NoVerifiedAdminReceipt;
        return Ok((result, facts));
    }
    let closed = match &event.kind {
        EventKind::Done {
            evidence: Some(proof),
            ..
        } => {
            item.status == Status::Done
                && !proof.is_blank()
                && item.evidence.as_ref() == Some(proof)
        }
        EventKind::Dropped { reason, .. } => {
            item.status == Status::Dropped
                && reason.kind.is_factual()
                && reason.evidence.as_ref().is_some_and(|p| !p.is_blank())
                && item.drop_reason.as_ref() == Some(reason)
        }
        _ => false,
    };
    if matches!(item.status, Status::Done | Status::Dropped) {
        if closed {
            result.state = S::Resolved;
            result.resolution = Some(AdminCaseResolutionKind::Closed);
            result.resolved_by = Some(if by_admin {
                AdminCaseResolutionActor::CaseAdmin
            } else {
                AdminCaseResolutionActor::Other
            });
            result.reason = R::ClosedWithEvidence;
        } else {
            result.state = S::Blocked;
            result.reason = R::NoClosureEvidence;
        }
        return Ok((result, facts));
    }
    if let EventKind::OwnerChanged {
        to,
        how: crewd_work::HowChanged::Lost,
        ..
    } = &event.kind
    {
        if item.owner.as_ref() != Some(to)
            || item.owner_history.last().is_none_or(|h| {
                h.to != *to
                    || h.by != event.by
                    || h.at != event.at
                    || h.how != crewd_work::HowChanged::Lost
            })
        {
            result.state = S::Blocked;
            result.reason = R::NoMatchingEvent;
            return Ok((result, facts));
        }
        let Some(owner) = crewd_store::sessions::get(c, to.as_str())? else {
            result.state = S::Blocked;
            result.reason = R::OwnerUnavailable;
            return Ok((result, facts));
        };
        facts["owner"] = serde_json::to_value(&owner).map_err(StoreError::from)?;
        if owner.desired != Desired::Running
            || owner.crew_id != w.crew_id()
            || owner.resume_failed_at.is_some()
            || crate::wakes::automatic_work_paused(c, &owner.id)?
            || crate::crews::is_archived(c, &owner.crew_id)?
        {
            result.state = S::Blocked;
            result.reason = R::OwnerUnavailable;
            return Ok((result, facts));
        }
        let Some(receipt) = receipt else {
            result.state = S::Blocked;
            result.reason = R::NoMatchingEvent;
            return Ok((result, facts));
        };
        let (Some(revision), Some(wake_id)) = (&receipt.result_revision, receipt.wake_id) else {
            result.state = S::Blocked;
            result.reason = R::NoMatchingEvent;
            return Ok((result, facts));
        };
        let Some(row) = crate::work_lost::assignment_notification(c, id, to.as_str(), revision)?
        else {
            result.state = S::Missing;
            result.reason = R::NoMatchingEvent;
            return Ok((result, facts));
        };
        if row.id != wake_id {
            return Err(invalid());
        }
        result.wake_id = Some(wake_id);
        facts["wake"] = serde_json::json!({"id":row.id,"state":row.state.as_str(),"attempts":row.attempts,"settledAt":row.settled_at});
        match row.state {
            crate::wakes::WakeState::Pending | crate::wakes::WakeState::Inflight => {
                result.reason = R::NotificationPending;
                return Ok((result, facts));
            }
            crate::wakes::WakeState::Dropped => {
                result.state = S::Blocked;
                result.reason = R::NotificationDropped;
                return Ok((result, facts));
            }
            crate::wakes::WakeState::Delivered => {
                if row.attempts == 0 || row.settled_at.is_none() {
                    result.state = S::Blocked;
                    result.reason = R::NoMatchingEvent;
                    return Ok((result, facts));
                }
            }
        }
        let original_remains = book
            .stop_violations(to, crewd_work::Ts(now))
            .iter()
            .any(|v| v.item == core && result.source_kinds.iter().any(|k| matches_kind(k, v.kind)));
        if original_remains {
            return Ok((result, facts));
        }
        result.state = S::Resolved;
        result.reason = R::ReassignedAndAcknowledged;
        result.resolution = Some(AdminCaseResolutionKind::Reassigned);
        result.resolved_by = Some(if by_admin {
            AdminCaseResolutionActor::CaseAdmin
        } else {
            AdminCaseResolutionActor::Other
        });
    }
    Ok((result, facts))
}
fn collect(c: &Connection, crew: &str, id: &str, now: i64) -> Result<Saved> {
    let source = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: crew.into(),
            id: id.into(),
        },
    )?;
    let w = execution::binding(c, crew, id)?.ok_or_else(invalid)?;
    let authority = execution::outcome_authority(c, &w)?;
    if !authority.actually_started {
        return Err(invalid());
    }
    let receipts = execution::effect_receipts(c, &w)?;
    let book = crate::asks::load_org(c)?;
    let mut groups: BTreeMap<String, Vec<AdminCaseTargetKind>> = BTreeMap::new();
    let mut out_of_scope = Vec::new();
    for t in source.targets {
        if t.scope == AdminCaseScope::InScope {
            groups.entry(t.id).or_default().push(t.kind)
        } else {
            out_of_scope.push(t.id)
        }
    }
    out_of_scope.sort();
    out_of_scope.dedup();
    let mut view = AdminCaseOutcomeView {
        case_id: id.into(),
        crew_id: crew.into(),
        revision: String::new(),
        verified_at: now,
        phase: AdminCaseOutcomePhase::Unresolved,
        reason: AdminCaseOutcomeReason::TargetsUnresolved,
        targets: Vec::new(),
        out_of_scope,
        missing: Vec::new(),
        pending: Vec::new(),
        blocked: Vec::new(),
        resolved_by_admin: Vec::new(),
        resolved_by_others: Vec::new(),
    };
    let mut facts = Vec::new();
    for (id, kinds) in groups {
        let (t, fact) = target(c, &book, &w, &id, kinds, &receipts, now)?;
        match t.state {
            AdminCaseTargetOutcomeState::Missing => view.missing.push(id.clone()),
            AdminCaseTargetOutcomeState::Pending => view.pending.push(id.clone()),
            AdminCaseTargetOutcomeState::Blocked => view.blocked.push(id.clone()),
            AdminCaseTargetOutcomeState::Resolved => match t.resolved_by {
                Some(AdminCaseResolutionActor::CaseAdmin) => {
                    view.resolved_by_admin.push(id.clone())
                }
                Some(AdminCaseResolutionActor::Other) => view.resolved_by_others.push(id.clone()),
                None => return Err(invalid()),
            },
        }
        view.targets.push(t);
        facts.push(fact);
    }
    if !authority.current {
        view.phase = AdminCaseOutcomePhase::Stale;
        view.reason = AdminCaseOutcomeReason::AuthorityChanged;
    } else if !view.out_of_scope.is_empty() {
        view.reason = AdminCaseOutcomeReason::OutOfScope;
    } else if !view.targets.is_empty()
        && view
            .targets
            .iter()
            .all(|t| t.state == AdminCaseTargetOutcomeState::Resolved)
    {
        view.phase = AdminCaseOutcomePhase::Resolved;
        view.reason = AdminCaseOutcomeReason::Verified;
    } else if view
        .targets
        .iter()
        .any(|t| t.reason == AdminCaseTargetOutcomeReason::NoVerifiedAdminReceipt)
    {
        view.reason = AdminCaseOutcomeReason::NoVerifiedAdminEffect;
    }
    // Do not include wall time or executor phase: an unchanged result remains the
    // same durable verification, including after real executor exit.
    let mut normalized = view.clone();
    normalized.verified_at = 0;
    let fingerprint = judgments::fingerprint(&(
        source.revision,
        w.attempt_id(),
        authority.fingerprint,
        &facts,
        &receipts,
        normalized,
    ))?;
    view.revision = fingerprint.clone();
    Ok(Saved {
        version: 1,
        attempt: w.attempt_id().into(),
        fingerprint,
        view_checksum: judgments::fingerprint(&view)?,
        view,
    })
}
pub fn verify(tx: &mut Tx<'_>, crew: &str, id: &str) -> Result<AdminCaseOutcomeView> {
    let current = collect(tx.conn(), crew, id, tx.now_ms())?;
    if let Some(old) = read(tx.conn(), crew, id)?
        && old.fingerprint == current.fingerprint
        && old.attempt == current.attempt
    {
        return Ok(old.view);
    }
    meta_set(
        tx.conn(),
        &key(id),
        &serde_json::to_string(&current).map_err(StoreError::from)?,
    )?;
    Ok(current.view)
}
pub fn status_at(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<AdminCaseOutcomeView>> {
    let Some(mut saved) = read(c, crew, id)? else {
        return Ok(None);
    };
    let current = collect(c, crew, id, now)?;
    if current.fingerprint != saved.fingerprint || current.attempt != saved.attempt {
        saved.view.phase = AdminCaseOutcomePhase::Stale;
        saved.view.reason = if current.view.phase == AdminCaseOutcomePhase::Stale {
            AdminCaseOutcomeReason::AuthorityChanged
        } else {
            AdminCaseOutcomeReason::TargetsUnresolved
        };
    }
    Ok(Some(saved.view))
}
pub fn status(c: &Connection, crew: &str, id: &str) -> Result<Option<AdminCaseOutcomeView>> {
    status_at(c, crew, id, crewd_store::now_ms())
}
/// Current typed business evidence only. A fresh custody authority must independently
/// authorize discharge; this never rewrites the original outcome or execution permit.
pub(crate) fn custody_facts(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<AdminCaseOutcomeView> {
    Ok(collect(c, crew, id, now)?.view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender,
        admin_case_execution::{
            ActionScope,
            tests::{close, running_case},
        },
        testkit::{NOW, ok, store},
        work,
    };
    fn close_admin(s: &mut crewd_store::Store, w: &execution::ExecutionWitness, id: &str) {
        ok(s, |tx| {
            let p = execution::validate_effect(
                tx,
                w,
                ActionScope::WorkClose {
                    id: id.into(),
                    kind: WorkClosureKind::Done,
                },
            )?;
            close(tx, w.session_id(), id)?;
            execution::finish_effect(tx, p)
        });
    }
    #[test]
    fn only_all_actual_targets_resolve_and_reads_never_create_results() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        assert!(
            status_at(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .is_none()
        );
        assert!(
            admin_cases::get_at(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: case.id.clone()
                },
                NOW
            )
            .unwrap()
            .outcome_record
            .is_none()
        );
        close_admin(&mut s, &reg.witness, &case.targets[0].id);
        let partial = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        assert_eq!(partial.phase, AdminCaseOutcomePhase::Unresolved);
        assert_eq!(partial.resolved_by_admin.len(), 1);
        assert_eq!(partial.pending.len(), 1);
        close_admin(&mut s, &reg.witness, &case.targets[1].id);
        assert_eq!(
            status_at(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .unwrap()
                .phase,
            AdminCaseOutcomePhase::Stale
        );
        let full = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        assert_eq!(full.phase, AdminCaseOutcomePhase::Resolved);
        assert_eq!(full.resolved_by_admin.len(), 2);
    }
    #[test]
    fn others_can_resolve_all_without_claiming_any_admin_work() {
        let mut s = store();
        let (case, _reg) = running_case(&mut s);
        for t in &case.targets {
            ok(&mut s, |tx| {
                let p = work::get(tx.conn(), &t.id)?;
                work::close_factual(
                    tx,
                    &Sender::Human,
                    WorkCloseParams {
                        id: t.id.clone(),
                        expected_revision: p.revision,
                        kind: WorkClosureKind::Done,
                        note: "external verified".into(),
                        evidence: WorkEvidenceParam {
                            kind: WorkEvidenceKind::Text,
                            value: "external artifact".into(),
                        },
                    },
                )
            });
        }
        let out = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        assert_eq!(out.phase, AdminCaseOutcomePhase::Resolved);
        assert!(out.resolved_by_admin.is_empty());
        assert_eq!(out.resolved_by_others.len(), 2);
    }
    #[test]
    fn same_facts_are_idempotent_and_process_unknown_does_not_erase_resolution() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        for t in &case.targets {
            close_admin(&mut s, &reg.witness, &t.id);
        }
        let first = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        let raw = meta_get(s.conn(), &key(&case.id)).unwrap();
        let second = crate::run(&mut s, NOW + 60_000, |tx| verify(tx, "crew-a", &case.id))
            .unwrap()
            .0;
        assert_eq!(first, second);
        assert_eq!(meta_get(s.conn(), &key(&case.id)).unwrap(), raw);
        ok(&mut s, |tx| execution::mark_unknown(tx, &reg.witness));
        s.conn()
            .execute(
                "UPDATE sessions SET desired='stopped' WHERE id=?1",
                [reg.witness.session_id()],
            )
            .unwrap();
        assert_eq!(
            status_at(s.conn(), "crew-a", &case.id, NOW + 60_000)
                .unwrap()
                .unwrap(),
            first
        );
        ok(&mut s, |tx| {
            crewd_store::judgment_state::restored(tx.conn())?;
            Ok(())
        });
        assert_eq!(
            status_at(s.conn(), "crew-a", &case.id, NOW)
                .unwrap()
                .unwrap()
                .phase,
            AdminCaseOutcomePhase::Stale
        );
    }
    #[test]
    fn admin_event_without_matching_receipt_is_not_credited_or_resolved() {
        let mut s = store();
        let (case, reg) = running_case(&mut s);
        for t in &case.targets {
            close_admin(&mut s, &reg.witness, &t.id);
        }
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key LIKE ?1",
                [format!(
                    "admin.case.effect.v1:{}:%",
                    reg.witness.attempt_id()
                )],
            )
            .unwrap();
        let out = ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        assert_eq!(out.phase, AdminCaseOutcomePhase::Unresolved);
        assert!(out.resolved_by_admin.is_empty());
        assert_eq!(out.blocked.len(), 2);
    }
    #[test]
    fn corrupted_outcome_payload_is_preserved_and_rejected() {
        let mut s = store();
        let (case, _) = running_case(&mut s);
        ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        let mut raw: serde_json::Value =
            serde_json::from_str(&meta_get(s.conn(), &key(&case.id)).unwrap().unwrap()).unwrap();
        raw["version"] = 99.into();
        let broken = raw.to_string();
        meta_set(s.conn(), &key(&case.id), &broken).unwrap();
        assert!(status_at(s.conn(), "crew-a", &case.id, NOW).is_err());
        assert!(crate::run(&mut s, NOW, |tx| verify(tx, "crew-a", &case.id)).is_err());
        assert_eq!(meta_get(s.conn(), &key(&case.id)).unwrap().unwrap(), broken);
    }
    #[test]
    fn altered_outcome_phase_or_attribution_cannot_reuse_old_revision() {
        let mut s = store();
        let (case, _) = running_case(&mut s);
        ok(&mut s, |tx| verify(tx, "crew-a", &case.id));
        let original = meta_get(s.conn(), &key(&case.id)).unwrap().unwrap();
        for field in ["phase", "resolvedByAdmin"] {
            let mut raw: serde_json::Value = serde_json::from_str(&original).unwrap();
            if field == "phase" {
                raw["view"][field] = "resolved".into();
            } else {
                raw["view"][field] = serde_json::json!([case.targets[0].id]);
            }
            let altered = raw.to_string();
            meta_set(s.conn(), &key(&case.id), &altered).unwrap();
            assert!(status_at(s.conn(), "crew-a", &case.id, NOW).is_err());
            assert!(crate::run(&mut s, NOW, |tx| verify(tx, "crew-a", &case.id)).is_err());
            assert_eq!(
                meta_get(s.conn(), &key(&case.id)).unwrap().unwrap(),
                altered
            );
        }
    }
}
