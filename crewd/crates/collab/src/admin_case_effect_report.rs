//! Human inspection of verified historical effects, independent of current outcome.
use crate::admin_case_execution::{self as execution, EffectHistory, EffectReceipt};
use crate::{Result, admin_case_link, admin_cases, judgments, resource_acl, work};
use crewd_proto::{
    AdminCaseEffect, AdminCaseEffectAction as Action, AdminCaseEffectEdge, AdminCaseEffectsPage,
    AdminCaseEffectsParams, AdminCaseGetParams, AdminCaseScope, WorkClosureKind, WorkEvidenceKind,
    WorkEvidenceParam,
};
use crewd_store::{
    StoreError, judgment_state,
    object_refs::{self, ObjectKind},
};
use crewd_work::{DropKind, Event, EventKind, Evidence, HowChanged, WorkId};
use rusqlite::{Connection, OptionalExtension, params};

fn invalid() -> crate::CollabError {
    StoreError::Invalid("invalid administrative effect history".into()).into()
}

fn number(raw: &str) -> Option<u64> {
    raw.parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == raw)
}

pub fn list(c: &Connection, p: &AdminCaseEffectsParams) -> Result<AdminCaseEffectsPage> {
    let limit = p.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(resource_acl::invalid("limit"));
    }
    let source = admin_cases::source_get(
        c,
        &AdminCaseGetParams {
            crew_id: p.crew_id.clone(),
            id: p.id.clone(),
        },
    )?;
    let mut page = AdminCaseEffectsPage {
        crew_id: p.crew_id.clone(),
        id: p.id.clone(),
        items: Vec::new(),
        next_after: None,
    };
    let Some(history) = execution::effect_history(c, &p.crew_id, &p.id)? else {
        if p.after.is_some() {
            return Err(resource_acl::invalid("after"));
        }
        return Ok(page);
    };
    let mut targets: Vec<_> = source
        .targets
        .iter()
        .filter(|t| t.scope == AdminCaseScope::InScope)
        .map(|t| t.id.clone())
        .collect();
    targets.sort();
    targets.dedup();
    if history.witness.source_session_id() != source.source.session_id || history.targets != targets
    {
        return Err(invalid());
    }
    let epoch = judgments::fingerprint(&(
        judgment_state::epoch(c)?,
        &p.crew_id,
        &p.id,
        history.witness.attempt_id(),
    ))?;
    let (last, start) = match p.after.as_deref() {
        None => (history.count, 0),
        Some(after) => {
            cursor(after, &epoch, history.count).ok_or_else(|| resource_acl::invalid("after"))?
        }
    };
    let end = last.min(start.saturating_add(u64::from(limit)));
    let mut previous_event = if start == 0 {
        0
    } else {
        history.receipt(c, start)?.last_event
    };
    for ordinal in (start + 1)..=end {
        let effect = history.receipt(c, ordinal)?;
        if effect.first_event <= previous_event {
            return Err(invalid());
        }
        page.items.push(project(c, &history, ordinal, &effect)?);
        previous_event = effect.last_event;
    }
    if end < last {
        page.next_after = Some(format!("{epoch}:{last}:{end}"));
    }
    Ok(page)
}

fn cursor(after: &str, epoch: &str, count: u64) -> Option<(u64, u64)> {
    let (bound, rest) = after.split_once(':')?;
    let (last, start) = rest.split_once(':')?;
    let (last, start) = (number(last)?, number(start)?);
    (bound == epoch && start < last && last <= count).then_some((last, start))
}

fn core(c: &Connection, history: &EffectHistory, id: &str) -> Result<WorkId> {
    if !history.targets.iter().any(|target| target == id) {
        return Err(invalid());
    }
    let r = object_refs::get(c, ObjectKind::Plan, work::number(id)?)?.ok_or_else(invalid)?;
    if r.crew_id != history.witness.crew_id() {
        return Err(invalid());
    }
    work::core_plan(c, &r)
}

fn project(
    c: &Connection,
    history: &EffectHistory,
    ordinal: u64,
    effect: &EffectReceipt,
) -> Result<AdminCaseEffect> {
    let subject = core(c, history, &effect.work_id)?;
    let first = i64::try_from(effect.first_event).map_err(|_| invalid())?;
    // The factual work command emits its primary event before releasing any
    // blocked work. The original range stays in the receipt. Auxiliary events
    // belong to the affected crews and may later be removed by their purge.
    let raw: String = c
        .query_row(
            "SELECT body FROM organization_work_events WHERE seq=?1 AND crew_id=?2",
            params![first, history.witness.crew_id()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(invalid)?;
    let event: Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
    if event.seq != effect.first_event
        || event.by.session().map(|s| s.as_str()) != Some(effect.session_id.as_str())
    {
        return Err(invalid());
    }
    let single = effect.first_event == effect.last_event;
    let mut action = match event.kind {
        EventKind::Done {
            id,
            evidence: Some(evidence),
            note: Some(note),
        } if id == subject => closure(effect, WorkClosureKind::Done, evidence, note)?,
        EventKind::Dropped { id, reason } if id == subject => {
            let kind = match reason.kind {
                DropKind::Duplicate => WorkClosureKind::Duplicate,
                DropKind::Superseded => WorkClosureKind::Superseded,
                DropKind::NotNeeded => return Err(invalid()),
            };
            closure(
                effect,
                kind,
                reason.evidence.ok_or_else(invalid)?,
                reason.note,
            )?
        }
        EventKind::OwnerChanged {
            id,
            from,
            to,
            how: HowChanged::Lost,
            note,
            ..
        } if single && id == subject && effect.link_request_id.is_none() => {
            if note.trim().is_empty() || effect.wake_id.is_some_and(|n| n <= 0) {
                return Err(invalid());
            }
            Action::Reassigned {
                previous_owner: from.map(|s| s.to_string()),
                new_owner: to.to_string(),
                note,
                wake_id: effect.wake_id.map(|n| n.to_string()),
            }
        }
        EventKind::Relinked {
            id,
            parent,
            depends_on,
        } if single && id == subject && effect.wake_id.is_none() => {
            if let Some(proof) = admin_case_link::report_receipt(c, &history.witness, effect)? {
                let peer = core(c, history, &proof.related_id)?;
                if peer == subject
                    || proof.id != effect.work_id
                    || effect.result_revision.as_deref() != Some(proof.revision.as_str())
                    || number(&proof.related_revision).is_none()
                    || proof.note.trim().is_empty()
                    || proof.evidence.value.trim().is_empty()
                    || resource_acl::uuid(&proof.request_id)? != proof.request_id
                {
                    return Err(invalid());
                }
                let edge = match proof.edge {
                    admin_case_link::Edge::Parent if parent.is_none() => {
                        AdminCaseEffectEdge::Parent
                    }
                    admin_case_link::Edge::Dependency if !depends_on.contains(&peer) => {
                        AdminCaseEffectEdge::Dependency
                    }
                    _ => return Err(invalid()),
                };
                Action::Unlinked {
                    related_work_id: Some(proof.related_id),
                    edge: Some(edge),
                    request_id: Some(proof.request_id),
                    evidence: Some(proof.evidence),
                    note: Some(proof.note),
                }
            } else {
                Action::Unlinked {
                    related_work_id: None,
                    edge: None,
                    request_id: None,
                    evidence: None,
                    note: None,
                }
            }
        }
        _ => return Err(invalid()),
    };
    if effect
        .result_revision
        .as_deref()
        .is_some_and(|r| number(r).is_none())
    {
        return Err(invalid());
    }
    let details_truncated = preview(&mut action);
    Ok(AdminCaseEffect {
        ordinal: ordinal.to_string(),
        work_id: effect.work_id.clone(),
        actor_session_id: effect.session_id.clone(),
        occurred_at_ms: event.at.0,
        first_event_seq: effect.first_event.to_string(),
        last_event_seq: effect.last_event.to_string(),
        result_revision: effect.result_revision.clone(),
        details_truncated,
        action,
    })
}

fn preview(action: &mut Action) -> bool {
    fn shorten(value: &mut String) -> bool {
        const MAX_FIELD_BYTES: usize = 4096;
        if value.len() <= MAX_FIELD_BYTES {
            return false;
        }
        value.truncate(value.floor_char_boundary(MAX_FIELD_BYTES));
        true
    }
    let (note, evidence) = match action {
        Action::Closed { note, evidence, .. } => (Some(note), Some(evidence)),
        Action::Reassigned { note, .. } => (Some(note), None),
        Action::Unlinked { note, evidence, .. } => (note.as_mut(), evidence.as_mut()),
    };
    let note_truncated = note.is_some_and(shorten);
    let evidence_truncated = evidence.is_some_and(|e| shorten(&mut e.value));
    note_truncated || evidence_truncated
}

fn closure(
    effect: &EffectReceipt,
    closure_kind: WorkClosureKind,
    evidence: Evidence,
    note: String,
) -> Result<Action> {
    if effect.wake_id.is_some()
        || effect.link_request_id.is_some()
        || evidence.is_blank()
        || note.trim().is_empty()
    {
        return Err(invalid());
    }
    let (kind, value) = match evidence {
        Evidence::Commit(v) => (WorkEvidenceKind::Commit, v),
        Evidence::Url(v) => (WorkEvidenceKind::Url, v),
        Evidence::Text(v) => (WorkEvidenceKind::Text, v),
    };
    Ok(Action::Closed {
        closure_kind,
        evidence: WorkEvidenceParam { kind, value },
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_case_execution::{
        ActionScope, Registration,
        tests::{close, running_case_with_edge},
    };
    use crate::testkit::{ok, store};
    use crewd_proto::{
        AdminCaseResolutionActor, AdminCaseTargetOutcomeReason, AdminCaseTargetOutcomeState,
    };
    use crewd_store::{Store, meta_get, meta_set};
    use serde_json::{Value, json};

    fn read(s: &Store, p: &AdminCaseEffectsParams) -> Value {
        serde_json::to_value(list(s.conn(), p).unwrap()).unwrap()
    }

    fn finish_close(s: &mut Store, reg: &Registration, id: &str, details: Option<&str>) {
        ok(s, |tx| {
            let effect = execution::validate_effect(
                tx,
                &reg.witness,
                ActionScope::WorkClose {
                    id: id.into(),
                    kind: WorkClosureKind::Done,
                },
            )?;
            if let Some(details) = details {
                let plan = work::get(tx.conn(), id)?;
                work::close_factual(
                    tx,
                    &crate::Sender::Session(reg.session.id.clone()),
                    crewd_proto::WorkCloseParams {
                        id: id.into(),
                        expected_revision: plan.revision,
                        kind: WorkClosureKind::Done,
                        evidence: WorkEvidenceParam {
                            kind: WorkEvidenceKind::Text,
                            value: details.into(),
                        },
                        note: details.into(),
                    },
                )?;
            } else {
                close(tx, &reg.session.id, id)?;
            }
            execution::finish_effect(tx, effect)
        });
    }

    #[test]
    fn admin_effect_report_preserves_primary_after_another_crew_purges_its_auxiliary_event() {
        let mut s = store();
        let (case, reg) = execution::tests::running_case(&mut s);
        let history = execution::effect_history(s.conn(), &case.crew_id, &case.id)
            .unwrap()
            .unwrap();
        let subject = core(s.conn(), &history, "W1").unwrap();
        let foreign = ok(&mut s, |tx| {
            let work = work::create(
                tx,
                &crate::Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: "crew-x".into(),
                    owner: Some("cap-x".into()),
                    title: "foreign waiting work".into(),
                    ..Default::default()
                },
            )?;
            let target = work::local_plan(tx.conn(), "crew-x", &work.id)?;
            let actor = work::actor(tx.conn(), &crate::Sender::Session("cap-x".into()), "crew-x")?;
            crate::asks::with_book(tx, "crew-x", |book, now| {
                book.block(
                    &actor,
                    now,
                    target,
                    vec![crewd_work::BlockOn::Item(subject)],
                    crewd_work::Continuation::Nothing,
                )
            })?;
            Ok(work.id)
        });
        assert_eq!(
            work::get(s.conn(), &foreign).unwrap().blocked_on,
            vec!["W1"]
        );
        finish_close(&mut s, &reg, "W1", None);
        let p = AdminCaseEffectsParams {
            crew_id: case.crew_id,
            id: case.id,
            after: None,
            limit: None,
        };
        let before = read(&s, &p);
        let outcome_before = ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, &p.crew_id, &p.id)
        });
        let attribution = outcome_before
            .targets
            .iter()
            .find(|t| t.id == "W1")
            .unwrap();
        assert_eq!(attribution.state, AdminCaseTargetOutcomeState::Resolved);
        assert_eq!(
            attribution.resolved_by,
            Some(AdminCaseResolutionActor::CaseAdmin)
        );
        let first = before["items"][0]["firstEventSeq"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let last = before["items"][0]["lastEventSeq"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        assert_eq!(last, first + 1);
        let auxiliary_crew: String = s
            .conn()
            .query_row(
                "SELECT crew_id FROM organization_work_events WHERE seq=?1",
                [last],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(auxiliary_crew, "crew-x");
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
        let remaining: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM organization_work_events WHERE seq BETWEEN ?1 AND ?2",
                params![first, last],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
        assert_eq!(read(&s, &p), before);
        let outcome_after = ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, &p.crew_id, &p.id)
        });
        assert_eq!(
            outcome_after.targets.iter().find(|t| t.id == "W1"),
            Some(attribution)
        );
        s.conn()
            .execute("DELETE FROM organization_work_events WHERE seq=?1", [first])
            .unwrap();
        assert!(
            list(s.conn(), &p).is_err(),
            "missing original primary must still fail closed"
        );
        let outcome_missing = ok(&mut s, |tx| {
            crate::admin_case_outcome::verify(tx, &p.crew_id, &p.id)
        });
        let missing = outcome_missing
            .targets
            .iter()
            .find(|t| t.id == "W1")
            .unwrap();
        assert_eq!(missing.state, AdminCaseTargetOutcomeState::Blocked);
        assert_eq!(
            missing.reason,
            AdminCaseTargetOutcomeReason::NoMatchingEvent
        );
        assert!(missing.resolved_by.is_none());
    }

    #[test]
    fn admin_effect_report_snapshots_real_effects_and_recovers_legacy_proof_without_writes() {
        let mut s = store();
        let edge = admin_case_link::Edge::Parent;
        let (case, reg) = running_case_with_edge(&mut s, Some(edge));
        let mut p = AdminCaseEffectsParams {
            crew_id: case.crew_id,
            id: case.id,
            after: None,
            limit: Some(1),
        };
        assert_eq!(read(&s, &p)["items"], json!([]));
        let request_id = uuid::Uuid::new_v4().to_string();
        let args = admin_case_link::UnlinkParams {
            request_id: request_id.clone(),
            id: "W2".into(),
            expected_revision: work::get(s.conn(), "W2").unwrap().revision,
            related_id: "W1".into(),
            expected_related_revision: work::get(s.conn(), "W1").unwrap().revision,
            edge,
            evidence: WorkEvidenceParam {
                kind: WorkEvidenceKind::Text,
                value: "independent source".into(),
            },
            note: "separate work".into(),
        };
        let receipt = ok(&mut s, |tx| admin_case_link::unlink(tx, &reg.witness, args));
        finish_close(&mut s, &reg, "W1", None);
        let first = read(&s, &p);
        let mut expected_first = json!({
            "ordinal":"1", "workId":"W2", "kind":"unlinked", "actorSessionId":reg.session.id,
            "firstEventSeq":receipt.event_seq, "lastEventSeq":receipt.event_seq,
            "resultRevision":receipt.revision, "relatedWorkId":"W1", "edge":"parent",
            "requestId":request_id, "detailsTruncated":false,
            "evidence":{"kind":"text","value":"independent source"},
            "note":"separate work"
        });
        expected_first["occurredAtMs"] = first["items"][0]["occurredAtMs"].clone();
        assert_eq!(first["items"], json!([expected_first]));
        let first_key = format!(
            "admin.case.effect.v1:{}:{:020}",
            reg.witness.attempt_id(),
            1
        );
        let raw = meta_get(s.conn(), &first_key).unwrap().unwrap();
        let mut old: Value = serde_json::from_str(&raw).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("linkRequestId")
            .unwrap();
        meta_set(s.conn(), &first_key, &old.to_string()).unwrap();
        assert_eq!(
            read(&s, &p),
            first,
            "old dedicated receipt still proves the exact correction"
        );

        p.after = first["nextAfter"].as_str().map(str::to_owned);
        assert!(p.after.is_some());
        let long = "证据".repeat(900);
        finish_close(&mut s, &reg, "W2", Some(&long));
        let second = read(&s, &p);
        assert_eq!(second["items"].as_array().unwrap().len(), 1);
        assert_eq!(second["items"][0]["ordinal"], "2");
        assert_eq!(second["items"][0]["workId"], "W1");
        assert_eq!(second["items"][0]["kind"], "closed");
        assert_eq!(second["items"][0]["closureKind"], "done");
        assert_eq!(
            second["items"][0]["evidence"]["value"],
            "synthetic checked artifact"
        );
        assert!(
            second["nextAfter"].is_null(),
            "new effects must not move an existing page boundary"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        let before = s.conn().total_changes();
        assert_eq!(read(&s, &p), second);
        let mut full = p.clone();
        full.after = None;
        full.limit = Some(50);
        let all = read(&s, &full);
        assert_eq!(all["items"].as_array().unwrap().len(), 3);
        let preview = &long[..long.floor_char_boundary(4096)];
        assert_eq!(all["items"][2]["detailsTruncated"], true);
        assert_eq!(all["items"][2]["note"], preview);
        assert_eq!(all["items"][2]["evidence"]["value"], preview);
        assert_eq!(
            work::get(s.conn(), "W2")
                .unwrap()
                .closure
                .unwrap()
                .note
                .as_deref(),
            Some(long.as_str())
        );
        assert_eq!(
            s.conn().total_changes(),
            before,
            "inspection must be passive after reopen"
        );

        let mut foreign = p.clone();
        foreign.crew_id = "crew-b".into();
        assert!(list(s.conn(), &foreign).is_err());
        judgment_state::restored(s.conn()).unwrap();
        assert!(
            list(s.conn(), &p).is_err(),
            "restored authority invalidates old cursors"
        );
        assert_eq!(read(&s, &full)["items"].as_array().unwrap().len(), 3);
        let key = format!("admin.case.execution.v1:{}", p.id);
        let raw = meta_get(s.conn(), &key).unwrap().unwrap();
        let mut stale: Value = serde_json::from_str(&raw).unwrap();
        for stale_count in [2, 0] {
            stale["view"]["effectsCount"] = json!(stale_count);
            meta_set(s.conn(), &key, &stale.to_string()).unwrap();
            assert!(
                list(s.conn(), &full).is_err(),
                "indexed later receipts cannot be silently hidden"
            );
        }
        meta_set(s.conn(), &key, &raw).unwrap();
        s.conn()
            .execute(
                "DELETE FROM organization_work_events WHERE seq=?1",
                [receipt.event_seq.parse::<i64>().unwrap()],
            )
            .unwrap();
        assert!(
            list(s.conn(), &full).is_err(),
            "missing event cannot look like an empty or successful report"
        );
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [&first_key])
            .unwrap();
        assert!(
            list(s.conn(), &full).is_err(),
            "missing receipt cannot look like an empty report"
        );
    }
}
