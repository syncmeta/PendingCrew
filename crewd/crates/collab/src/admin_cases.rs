//! Durable typed escalation sources. Observation grants no execution authority.
use crate::{Result, Tx, judgments, work};
use crewd_proto::judgments::JudgmentSettings;
use crewd_proto::*;
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
const PREFIX: &str = "admin.case.v1:";
const COUNTER: &str = "admin.case.sequence.v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    view: AdminCaseView,
}
fn invalid(message: &str) -> crate::CollabError {
    StoreError::Invalid(message.into()).into()
}
fn crew_prefix(crew: &str) -> Result<String> {
    Ok(format!("{PREFIX}{}:", judgments::fingerprint(&crew)?))
}
pub(crate) fn key(crew: &str, id: &str) -> Result<String> {
    sequence(id)?;
    Ok(format!("{}{id}", crew_prefix(crew)?))
}
fn sequence(id: &str) -> Result<u64> {
    let n = id
        .strip_prefix("AC")
        .filter(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("invalid admin case id"))?;
    Ok(n)
}
fn revision(v: &AdminCaseView) -> Result<String> {
    // Preserve the exact version-1 source serialization and its field order.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Source<'a> {
        id: &'a str,
        crew_id: &'a str,
        revision: &'a str,
        created_at: i64,
        source: &'a AdminCaseSource,
        targets: &'a [AdminCaseTarget],
        decision: &'a AdminCaseDecision,
        execution: &'a AdminCaseExecution,
        retirement: &'a AdminCaseRetirement,
    }
    judgments::fingerprint(&Source {
        id: &v.id,
        crew_id: &v.crew_id,
        revision: "",
        created_at: v.created_at,
        source: &v.source,
        targets: &v.targets,
        decision: &v.decision,
        execution: &v.execution,
        retirement: &v.retirement,
    })
}

fn decode(raw: &str, crew: &str, id: &str) -> Result<AdminCaseView> {
    let s: Saved = serde_json::from_str(raw).map_err(StoreError::from)?;
    let v = s.view;
    if s.version != 1
        || v.decision_record.is_some()
        || v.execution_record.is_some()
        || v.outcome_record.is_some()
        || v.retirement_record.is_some()
        || v.handoff_record.is_some()
        || v.decision != AdminCaseDecision::Observed
        || v.id != id
        || v.crew_id != crew
        || v.source.budget_epoch < 0
        || (v.source.budget_epoch == 0 && v.source.provenance.is_none())
        || v.source.session_id.is_empty()
        || v.source.announcement_id.is_empty()
        || v.source.restore_epoch.is_empty()
        || v.targets.is_empty()
        || revision(&v)? != v.revision
    {
        return Err(invalid("invalid admin case record"));
    }
    if let Some(AdminCaseProvenance::HumanRevalidation {
        request_id,
        legacy_message_id,
        announcement_digest,
        preview_revision,
        note,
        revalidated_at,
    }) = &v.source.provenance
        && (uuid::Uuid::parse_str(request_id).is_err()
            || legacy_message_id != &v.source.announcement_id
            || announcement_digest.is_empty()
            || preview_revision.is_empty()
            || note.trim().is_empty()
            || *revalidated_at != v.created_at)
    {
        return Err(invalid("invalid human source provenance"));
    }

    sequence(id)?;
    let mut seen = std::collections::BTreeSet::new();
    for t in &v.targets {
        work::number(&t.id)?;
        if t.revision.is_empty()
            || t.crew_id.is_empty()
            || (t.scope == AdminCaseScope::InScope) != (t.crew_id == crew)
            || !seen.insert((&t.id, format!("{:?}", t.kind)))
        {
            return Err(invalid("invalid admin case target"));
        }
    }
    Ok(v)
}
pub(crate) fn source_get(c: &Connection, p: &AdminCaseGetParams) -> Result<AdminCaseView> {
    sequence(&p.id).map_err(|_| crate::resource_acl::invalid("id"))?;
    let raw =
        meta_get(c, &key(&p.crew_id, &p.id)?)?.ok_or_else(|| crate::resource_acl::invalid("id"))?;
    decode(&raw, &p.crew_id, &p.id)
}
pub fn get(c: &Connection, p: &AdminCaseGetParams) -> Result<AdminCaseView> {
    get_at(c, p, crewd_store::now_ms())
}
/// Cursor is a restore-bound monotonic sequence, not a timestamp or random id.
fn source_list(c: &Connection, p: &AdminCaseListParams) -> Result<AdminCaseList> {
    if crewd_store::crews::get(c, &p.crew_id)?.is_none() {
        return Err(crate::resource_acl::invalid("crewId"));
    }
    let limit = p.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(crate::resource_acl::invalid("limit"));
    }
    let epoch = judgments::fingerprint(&(crewd_store::judgment_state::epoch(c)?, &p.crew_id))?;
    let after = match p.after.as_deref() {
        None => 0,
        Some(s) => {
            let (e, id) = s
                .split_once(':')
                .ok_or_else(|| crate::resource_acl::invalid("after"))?;
            if e != epoch {
                return Err(crate::resource_acl::invalid("after"));
            }
            sequence(id).map_err(|_| crate::resource_acl::invalid("after"))?
        }
    };
    let prefix = crew_prefix(&p.crew_id)?;
    let lower = format!("{prefix}AC{after:020}");
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let rows = q
        .query_map(
            params![lower, format!("{prefix}\u{10ffff}"), i64::from(limit) + 1],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for (k, v) in rows.into_iter().take(limit as usize) {
        let id = k
            .strip_prefix(&prefix)
            .ok_or_else(|| invalid("invalid case key"))?;
        items.push(decode(&v, &p.crew_id, id)?);
    }
    let next_after = if more {
        items.last().map(|v| format!("{epoch}:{}", v.id))
    } else {
        None
    };
    Ok(AdminCaseList {
        crew_id: p.crew_id.clone(),
        items,
        next_after,
    })
}
/// Called only by accepted typed Stop escalation, in its budget+announcement Tx.
pub(crate) fn record_stop_escalation(
    tx: &mut Tx<'_>,
    session: &str,
    targets: &[crate::stop_checks::Target],
    announcement: &str,
) -> Result<AdminCaseView> {
    record_source(tx, session, targets, announcement, None)
}
pub(crate) fn record_revalidated_source(
    tx: &mut Tx<'_>,
    session: &str,
    targets: &[crate::stop_checks::Target],
    announcement: &str,
    provenance: AdminCaseProvenance,
) -> Result<AdminCaseView> {
    record_source(tx, session, targets, announcement, Some(provenance))
}
fn record_source(
    tx: &mut Tx<'_>,
    session: &str,
    targets: &[crate::stop_checks::Target],
    announcement: &str,
    provenance: Option<AdminCaseProvenance>,
) -> Result<AdminCaseView> {
    let s = crewd_store::sessions::get(tx.conn(), session)?
        .ok_or_else(|| invalid("missing escalation source"))?;
    let budget = crewd_store::work_stop_checks::get(tx.conn(), session)?
        .ok_or_else(|| invalid("missing escalation budget"))?;
    if budget.epoch < 0 || (budget.epoch == 0 && provenance.is_none()) {
        return Err(invalid("invalid escalation budget epoch"));
    }
    if !budget.streak.escalated || targets.is_empty() || budget.streak.last.len() != targets.len() {
        return Err(invalid("unaccepted escalation source"));
    }
    let msg = crate::messages::get(tx.conn(), announcement)?;
    if msg.sender_kind != SenderKind::System
        || msg.category.as_deref() != Some("work_stop_escalation")
    {
        return Err(invalid("invalid escalation announcement"));
    }
    let mut full = Vec::with_capacity(targets.len());
    for t in targets {
        let number = work::number(&t.id)?;
        if !budget
            .streak
            .last
            .iter()
            .any(|v| i64::from(v.item.0) == number && v.kind == t.kind)
        {
            return Err(invalid("escalation target budget mismatch"));
        }
        let p = work::get(tx.conn(), &t.id)?;
        if p.revision != t.revision {
            return Err(invalid("stale escalation target"));
        }
        let scope = if p.crew_id == s.crew_id {
            AdminCaseScope::InScope
        } else {
            AdminCaseScope::OutOfScope
        };
        let kind = match t.kind {
            crewd_work::StopKind::Unanswered => AdminCaseTargetKind::Unanswered,
            crewd_work::StopKind::Overdue => AdminCaseTargetKind::Overdue,
            crewd_work::StopKind::ReadyUnscheduled => AdminCaseTargetKind::ReadyUnscheduled,
        };
        full.push(AdminCaseTarget {
            id: t.id.clone(),
            crew_id: p.crew_id,
            revision: t.revision.clone(),
            kind,
            scope,
        });
    }
    let source_key = format!(
        "admin.case.source.v1:{}",
        judgments::fingerprint(&(session, budget.epoch, &s.crew_id))?
    );
    if let Some(id) =
        crate::admin_case_legacy::current_source(tx.conn(), session, budget.epoch, &s.crew_id)?
    {
        let existing = source_get(
            tx.conn(),
            &AdminCaseGetParams {
                crew_id: s.crew_id,
                id,
            },
        )?;
        if provenance.is_some() {
            return Ok(existing);
        }
        if existing.targets != full || existing.source.announcement_id != announcement {
            return Err(invalid("conflicting escalation source"));
        }
        return Ok(existing);
    }
    let counter = meta_get(tx.conn(), COUNTER)?;
    if counter.is_none()
        && tx.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key>=?1 AND key<?2)",
            params![PREFIX, format!("{PREFIX}\u{10ffff}")],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid("missing admin case sequence"));
    }
    let seq = counter
        .map(|s| {
            s.parse::<u64>()
                .map_err(|_| invalid("invalid case sequence"))
        })
        .transpose()?
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("case sequence overflow"))?;
    let id = format!("AC{seq:020}");
    let global_key = format!("admin.case.id.v1:{id}");
    if meta_get(tx.conn(), &global_key)?.is_some() {
        return Err(invalid("admin case global sequence collision"));
    }
    let mut view = AdminCaseView {
        custody_record: None,
        launch_record: None,
        id,
        crew_id: s.crew_id,
        revision: String::new(),
        created_at: tx.now_ms(),
        source: AdminCaseSource {
            provenance,
            session_id: session.into(),
            budget_epoch: budget.epoch,
            restore_epoch: crewd_store::judgment_state::epoch(tx.conn())?,
            announcement_id: announcement.into(),
        },
        targets: full,
        decision: AdminCaseDecision::Observed,
        decision_record: None,
        execution: AdminCaseExecution::NotStarted,
        execution_record: None,
        retirement: AdminCaseRetirement::NotStarted,
        retirement_record: None,
        handoff_record: None,
        outcome_record: None,
    };
    view.revision = revision(&view)?;
    let k = key(&view.crew_id, &view.id)?;
    if meta_get(tx.conn(), &k)?.is_some() {
        return Err(invalid("admin case sequence collision"));
    }
    let raw = serde_json::to_string(&Saved {
        version: 1,
        view: view.clone(),
    })
    .map_err(StoreError::from)?;
    decode(&raw, &view.crew_id, &view.id)?;
    meta_set(tx.conn(), &k, &raw)?;
    meta_set(tx.conn(), COUNTER, &seq.to_string())?;
    meta_set(tx.conn(), &global_key, &k)?;
    meta_set(tx.conn(), &source_key, &view.id)?;
    crate::admin_case_legacy::index_source(tx, &view)?;
    Ok(view)
}
#[derive(Debug, Clone, Serialize)]
pub struct CurrentTarget {
    pub source: AdminCaseTarget,
    pub current: WorkPlan,
    pub still_violating: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub case: AdminCaseView,
    pub restore_matches: bool,
    pub targets: Vec<CurrentTarget>,
    pub fingerprint: String,
}
/// Reads authoritative current work, never interpreting the announcement body.
/// This is evidence only, not permission to execute any target (including in-scope).
pub fn snapshot(c: &Connection, crew: &str, id: &str, now: i64) -> Result<Snapshot> {
    let case = source_get(
        c,
        &AdminCaseGetParams {
            crew_id: crew.into(),
            id: id.into(),
        },
    )?;
    let epoch = crewd_store::judgment_state::epoch(c)?;
    let book = crate::asks::load_org(c)?;
    let violations = book.stop_violations(
        &crewd_work::SessionId::new(&case.source.session_id),
        crewd_work::Ts(now),
    );
    let mut targets = Vec::new();
    for source in &case.targets {
        let current = work::get(c, &source.id)?;
        let reference = crewd_store::object_refs::get(
            c,
            crewd_store::object_refs::ObjectKind::Plan,
            work::number(&source.id)?,
        )?
        .ok_or_else(|| invalid("missing case work reference"))?;
        let core = work::core_plan(c, &reference)?;
        let still_violating = violations.iter().any(|v| {
            v.item == core
                && match source.kind {
                    AdminCaseTargetKind::Unanswered => v.kind == crewd_work::StopKind::Unanswered,
                    AdminCaseTargetKind::Overdue => v.kind == crewd_work::StopKind::Overdue,
                    AdminCaseTargetKind::ReadyUnscheduled => {
                        v.kind == crewd_work::StopKind::ReadyUnscheduled
                    }
                }
        });
        targets.push(CurrentTarget {
            source: source.clone(),
            current,
            still_violating,
        });
    }
    let session = crewd_store::sessions::get(c, &case.source.session_id)?;
    let fingerprint = judgments::fingerprint(&(
        &case.revision,
        &epoch,
        &targets,
        session,
        crewd_store::crews::get(c, crew)?,
        judgments::configuration(c)?,
        judgments::stop_epoch(c, &case.source.session_id)?,
        crate::wakes::human_stop_floor(c, &case.source.session_id)?,
        crewd_store::work_stop_checks::get(c, &case.source.session_id)?.map(|b| b.epoch),
    ))?;
    Ok(Snapshot {
        restore_matches: case.source.restore_epoch == epoch,
        case,
        targets,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Sender, stop_checks,
        testkit::{NOW, ok, store},
    };
    use crewd_proto::judgments::JudgmentConfig;
    fn create_case(s: &mut crewd_store::Store, owner: &str) -> AdminCaseView {
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id=?1", [owner])
            .unwrap();
        ok(s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    owner: Some(owner.into()),
                    title: "synthetic admin source".into(),
                    ..Default::default()
                },
            )
        });
        for n in 0..4 {
            s.conn()
                .execute(
                    "UPDATE wakes SET state='delivered' WHERE session_id=?1",
                    [owner],
                )
                .unwrap();
            let c = stop_checks::candidate(s.conn(), owner, NOW)
                .unwrap()
                .unwrap();
            let accepted = ok(s, |tx| stop_checks::accept(tx, &c));
            if n == 3 {
                assert_eq!(accepted, stop_checks::Accepted::Escalated);
            }
        }
        list(
            s.conn(),
            &AdminCaseListParams {
                crew_id: "crew-a".into(),
                after: None,
                limit: None,
            },
        )
        .unwrap()
        .items
        .into_iter()
        .find(|v| v.source.session_id == owner)
        .unwrap()
    }
    #[test]
    fn configured_case_prepares_native_judgment_without_changing_source() {
        let mut s = store();
        let a = create_case(&mut s, "w-a1");
        let settings = JudgmentSettings {
            revision: Some("synthetic".into()),
            config: Some(JudgmentConfig {
                enabled: true,
                provider: "typesafe".into(),
                base_url: "http://127.0.0.1:1".into(),
                model: "fake".into(),
                choice_min: 0.8,
                wake_yes: 0.8,
                similar_requirements: None,
                credential_source: "file".into(),
            }),
        };
        meta_set(
            s.conn(),
            judgments::CONFIG_KEY,
            &serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &a.id)).is_some());
    }
    fn configure_real(s: &mut crewd_store::Store) {
        let prior = judgments::configuration(s.conn()).unwrap();
        ok(s, |tx| {
            crewd_store::judgment_state::prepare_configuration(tx.conn())?;
            judgments::set_configuration(
                tx,
                &Sender::Human,
                crewd_proto::judgments::JudgmentSettingsSet {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: prior.revision,
                    credential_update: Default::default(),
                    config: Some(crewd_proto::judgments::JudgmentConfig {
                        enabled: true,
                        provider: "typesafe".into(),
                        base_url: "http://127.0.0.1:1".into(),
                        model: "fake".into(),
                        choice_min: 0.8,
                        wake_yes: 0.8,
                        similar_requirements: None,
                        credential_source: "file".into(),
                    }),
                },
            )
        });
    }
    #[test]
    fn first_configuration_revalidates_only_unsent_and_unknown_never_replays() {
        let mut s = store();
        let source = create_case(&mut s, "w-a1");
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).is_none());
        assert_eq!(
            read_decision(s.conn(), &source.id)
                .unwrap()
                .unwrap()
                .view
                .phase,
            AdminCaseDecision::Unconfigured
        );
        configure_real(&mut s);
        let snap = ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).unwrap();
        assert_eq!(snap.summary["targets"][0]["id"], "t0");
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        ok(&mut s, |tx| recover_page(tx, None));
        assert_eq!(
            read_decision(s.conn(), &source.id)
                .unwrap()
                .unwrap()
                .view
                .phase,
            AdminCaseDecision::Unknown
        );
        configure_real(&mut s);
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).is_none());
        assert!(!ok(&mut s, |tx| settle(
            tx,
            &snap,
            Some(0.99),
            AdminCaseDecisionReason::NativeAnswered
        )));
        assert_eq!(
            source_get(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: source.id.clone()
                }
            )
            .unwrap(),
            source
        );
    }
    #[test]
    fn native_answer_is_advice_generated_is_unknown_and_admission_loss_is_terminal() {
        let mut s = store();
        configure_real(&mut s);
        for (owner, reason, expected) in [
            (
                "w-a1",
                AdminCaseDecisionReason::NativeAnswered,
                AdminCaseDecision::Answered,
            ),
            (
                "w-a2",
                AdminCaseDecisionReason::Generated,
                AdminCaseDecision::Unknown,
            ),
            (
                "cap-a",
                AdminCaseDecisionReason::NoAdmission,
                AdminCaseDecision::Unknown,
            ),
        ] {
            let source = create_case(&mut s, owner);
            let snap = ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).unwrap();
            assert!(ok(&mut s, |tx| defer(
                tx,
                &snap,
                AdminCaseDecisionReason::NoAdmission
            )));
            let again = ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).unwrap();
            assert_eq!(snap.decision_id, again.decision_id);
            assert!(ok(&mut s, |tx| dispatch(tx, &again)));
            let native = reason == AdminCaseDecisionReason::NativeAnswered;
            assert!(ok(&mut s, |tx| settle(tx, &again, Some(0.95), reason)));
            let view = get_at(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: source.id.clone(),
                },
                NOW,
            )
            .unwrap();
            assert_eq!(view.decision, expected);
            assert_eq!(view.execution, AdminCaseExecution::NotStarted);
            assert_eq!(view.decision_record.unwrap().adopted, native);
            assert_eq!(view.revision, source.revision);
            assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).is_none());
        }
    }
    #[test]
    fn rejected_epoch_cannot_be_laundered_by_later_config_change() {
        let mut s = store();
        configure_real(&mut s);
        let source = create_case(&mut s, "w-a1");
        let snap = ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        assert!(ok(&mut s, |tx| settle(
            tx,
            &snap,
            Some(0.9),
            AdminCaseDecisionReason::NativeAnswered
        )));
        ok(&mut s, |tx| {
            crewd_store::judgment_state::restored(tx.conn())?;
            Ok(())
        });
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).is_none());
        let accepted = read_decision(s.conn(), &source.id)
            .unwrap()
            .unwrap()
            .accepted_epoch;
        assert_ne!(
            accepted,
            crewd_store::judgment_state::epoch(s.conn()).unwrap()
        );
        configure_real(&mut s);
        assert!(ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).is_none());
    }
    #[test]
    fn global_scan_and_recovery_cover_sixty_fifth_case_past_damaged_record() {
        let mut s = store();
        configure_real(&mut s);
        let source = create_case(&mut s, "w-a1");
        for n in 2..=65 {
            let mut v = source.clone();
            v.id = format!("AC{n:020}");
            v.revision = revision(&v).unwrap();
            let k = key("crew-a", &v.id).unwrap();
            meta_set(s.conn(), &format!("admin.case.id.v1:{}", v.id), &k).unwrap();
            meta_set(
                s.conn(),
                &k,
                &serde_json::to_string(&Saved {
                    version: 1,
                    view: v,
                })
                .unwrap(),
            )
            .unwrap();
        }
        let (first, next) = source_page(s.conn(), None).unwrap();
        assert_eq!(first.len(), 64);
        let (last, end) = source_page(s.conn(), next.clone()).unwrap();
        assert_eq!(last.len(), 1);
        assert!(end.is_none());
        let snap = ok(&mut s, |tx| prepare(tx, &last[0].crew, &last[0].id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &snap)));
        meta_set(s.conn(), &decision_key(&source.id), "broken").unwrap();
        let (_, cursor) = ok(&mut s, |tx| recover_page(tx, None));
        assert_eq!(cursor, next);
        ok(&mut s, |tx| recover_page(tx, cursor));
        assert_eq!(
            read_decision(s.conn(), &snap.id)
                .unwrap()
                .unwrap()
                .view
                .phase,
            AdminCaseDecision::Unknown
        );
        assert_eq!(
            meta_get(s.conn(), &decision_key(&source.id))
                .unwrap()
                .unwrap(),
            "broken"
        );
    }
    #[test]
    fn legacy_v1_without_decision_record_keeps_exact_source_hash_and_bytes() {
        let mut s = store();
        let source = create_case(&mut s, "w-a1");
        let k = key("crew-a", &source.id).unwrap();
        let raw = meta_get(s.conn(), &k).unwrap().unwrap();
        let mut legacy: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            legacy["view"]
                .as_object_mut()
                .unwrap()
                .remove("decisionRecord")
                .is_some()
        );
        legacy["view"]
            .as_object_mut()
            .unwrap()
            .remove("executionRecord");
        legacy["view"]
            .as_object_mut()
            .unwrap()
            .remove("outcomeRecord");
        legacy["view"]
            .as_object_mut()
            .unwrap()
            .remove("retirementRecord");
        legacy["view"]
            .as_object_mut()
            .unwrap()
            .remove("handoffRecord");
        let old = serde_json::to_string(&legacy).unwrap();
        meta_set(s.conn(), &k, &old).unwrap();
        assert_eq!(
            get_at(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: source.id.clone()
                },
                NOW
            )
            .unwrap(),
            source
        );
        assert_eq!(
            list_at(
                s.conn(),
                &AdminCaseListParams {
                    crew_id: "crew-a".into(),
                    after: None,
                    limit: None
                },
                NOW
            )
            .unwrap()
            .items,
            vec![source.clone()]
        );
        configure_real(&mut s);
        let prepared = ok(&mut s, |tx| prepare(tx, "crew-a", &source.id)).unwrap();
        assert!(ok(&mut s, |tx| dispatch(tx, &prepared)));
        assert_eq!(
            get_at(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: source.id.clone()
                },
                NOW
            )
            .unwrap()
            .revision,
            source.revision
        );
        assert_eq!(meta_get(s.conn(), &k).unwrap().unwrap(), old);
    }
    #[test]
    fn missing_or_rewound_counter_cannot_reuse_global_id_in_another_crew() {
        let mut s = store();
        let a = create_case(&mut s, "w-a1");
        s.conn()
            .execute("UPDATE sessions SET desired='running' WHERE id='w-b1'", [])
            .unwrap();
        ok(&mut s, |tx| {
            work::create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-b".into(),
                    owner: Some("w-b1".into()),
                    title: "other source".into(),
                    ..Default::default()
                },
            )
        });
        for _ in 0..3 {
            s.conn()
                .execute(
                    "UPDATE wakes SET state='delivered' WHERE session_id='w-b1'",
                    [],
                )
                .unwrap();
            let c = stop_checks::candidate(s.conn(), "w-b1", NOW)
                .unwrap()
                .unwrap();
            assert!(matches!(
                ok(&mut s, |tx| stop_checks::accept(tx, &c)),
                stop_checks::Accepted::Rewake(_)
            ));
        }
        s.conn()
            .execute(
                "UPDATE wakes SET state='delivered' WHERE session_id='w-b1'",
                [],
            )
            .unwrap();
        let c = stop_checks::candidate(s.conn(), "w-b1", NOW)
            .unwrap()
            .unwrap();
        let before = crewd_store::work_stop_checks::get(s.conn(), "w-b1").unwrap();
        let messages: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [COUNTER])
            .unwrap();
        assert!(crate::run(&mut s, NOW, |tx| stop_checks::accept(tx, &c)).is_err());
        meta_set(s.conn(), COUNTER, "0").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| stop_checks::accept(tx, &c)).is_err());
        assert_eq!(
            crewd_store::work_stop_checks::get(s.conn(), "w-b1").unwrap(),
            before
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            messages
        );
        assert_eq!(
            get(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: a.id.clone()
                }
            )
            .unwrap(),
            a
        );
    }
    #[test]
    fn same_time_pages_include_later_insertions_and_restore_rejects_cursor() {
        let mut s = store();
        let a = create_case(&mut s, "w-a1");
        let b = create_case(&mut s, "w-a2");
        assert_eq!(a.created_at, b.created_at);
        let mut params = AdminCaseListParams {
            crew_id: "crew-a".into(),
            after: None,
            limit: Some(1),
        };
        let first = list(s.conn(), &params).unwrap();
        assert_eq!(first.items[0].id, a.id);
        params.after = first.next_after;
        let c = create_case(&mut s, "cap-a");
        let second = list(s.conn(), &params).unwrap();
        assert_eq!(second.items[0].id, b.id);
        params.after = second.next_after;
        let last = list(s.conn(), &params).unwrap();
        assert_eq!(last.items[0].id, c.id);
        assert!(last.next_after.is_none());
        ok(&mut s, |tx| {
            crewd_store::judgment_state::restored(tx.conn())?;
            Ok(())
        });
        assert!(list(s.conn(), &params).is_err());
        assert!(
            !snapshot(s.conn(), "crew-a", &a.id, NOW)
                .unwrap()
                .restore_matches
        );
        assert_eq!(
            get(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: a.id
                }
            )
            .unwrap()
            .source,
            a.source
        );
    }
    #[test]
    fn corrupt_unknown_record_is_neither_absent_nor_processed() {
        let mut s = store();
        let a = create_case(&mut s, "w-a1");
        let k = key("crew-a", &a.id).unwrap();
        let raw = meta_get(s.conn(), &k).unwrap().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["version"] = 2.into();
        meta_set(s.conn(), &k, &value.to_string()).unwrap();
        assert!(
            get(
                s.conn(),
                &AdminCaseGetParams {
                    crew_id: "crew-a".into(),
                    id: a.id
                }
            )
            .is_err()
        );
        assert!(
            list(
                s.conn(),
                &AdminCaseListParams {
                    crew_id: "crew-a".into(),
                    after: None,
                    limit: None
                }
            )
            .is_err()
        );
        assert_eq!(meta_get(s.conn(), &k).unwrap().unwrap(), value.to_string());
    }
    #[test]
    fn snapshot_tracks_real_revision_and_stop_witness_without_mutating_source() {
        let mut s = store();
        let a = create_case(&mut s, "w-a1");
        let before = snapshot(s.conn(), "crew-a", &a.id, NOW).unwrap();
        let plan = &before.targets[0].current;
        ok(&mut s, |tx| {
            work::update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: plan.id.clone(),
                    expected_revision: plan.revision.clone(),
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        });
        let after = snapshot(s.conn(), "crew-a", &a.id, NOW).unwrap();
        assert_eq!(before.case, after.case);
        assert_ne!(before.fingerprint, after.fingerprint);
        assert!(before.targets[0].still_violating);
        assert!(!after.targets[0].still_violating);
        assert_eq!(after.targets[0].current.status, "done");
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSnapshot {
    pub crew: String,
    pub id: String,
    pub source_session: String,
    pub decision_id: String,
    pub fingerprint: String,
    pub summary: serde_json::Value,
    pub settings: JudgmentSettings,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionSaved {
    version: u32,
    observed_at: i64,
    accepted_epoch: String,
    snapshot: DecisionSnapshot,
    view: AdminCaseDecisionView,
}
fn decision_key(id: &str) -> String {
    format!("admin.case.decision.v1:{id}")
}
fn read_decision(c: &Connection, id: &str) -> Result<Option<DecisionSaved>> {
    let Some(raw) = meta_get(c, &decision_key(id))? else {
        return Ok(None);
    };
    let r: DecisionSaved = serde_json::from_str(&raw).map_err(StoreError::from)?;
    use AdminCaseDecision as P;
    use AdminCaseDecisionReason as R;
    let phase_valid = match r.view.phase {
        P::Observed => false,
        P::Prepared | P::Dispatched => r.view.reason == R::Prepared && r.view.probability.is_none(),
        P::Answered => r.view.reason == R::NativeAnswered && r.view.probability.is_some(),
        P::Unknown => {
            matches!(
                r.view.reason,
                R::NativeAnswered
                    | R::Generated
                    | R::ProviderUnavailable
                    | R::RestartUnknown
                    | R::NoAdmission
            ) && r.view.probability.is_none()
        }
        P::Unconfigured => r.view.reason == R::Unconfigured && r.view.probability.is_none(),
        P::WaitingAdmission => r.view.reason == R::NoAdmission && r.view.probability.is_none(),
        P::Stale => matches!(
            r.view.reason,
            R::StateChanged | R::Stopped | R::NoInScopeTargets
        ),
    };
    if r.version != 1
        || !phase_valid
        || r.accepted_epoch.is_empty()
        || uuid::Uuid::parse_str(&r.view.decision_id).is_err()
        || r.snapshot.id != id
        || r.snapshot.decision_id != r.view.decision_id
        || r.snapshot.fingerprint != r.view.snapshot_witness
        || r.view
            .probability
            .is_some_and(|p| !p.is_finite() || !(0.0..=1.0).contains(&p))
        || r.view.adopted
            && (r.view.phase != AdminCaseDecision::Answered
                || r.view.reason != AdminCaseDecisionReason::NativeAnswered
                || r.view.probability.is_none())
    {
        return Err(invalid("invalid admin decision record"));
    }
    Ok(Some(r))
}
fn save_decision(tx: &mut Tx<'_>, r: &DecisionSaved) -> Result<()> {
    let raw = serde_json::to_string(r).map_err(StoreError::from)?;
    meta_set(tx.conn(), &decision_key(&r.snapshot.id), &raw)?;
    meta_set(
        tx.conn(),
        &format!("admin.case.decision.history.v1:{}", r.snapshot.decision_id),
        &raw,
    )?;
    Ok(())
}
fn blocked(c: &Connection, s: &Snapshot) -> Result<Option<AdminCaseDecisionReason>> {
    use AdminCaseDecisionReason as R;
    let Some(session) = crewd_store::sessions::get(c, &s.case.source.session_id)? else {
        return Ok(Some(R::StateChanged));
    };
    if session.crew_id != s.case.crew_id || crate::crews::is_archived(c, &s.case.crew_id)? {
        return Ok(Some(R::StateChanged));
    }
    if session.desired != Desired::Running
        || session.resume_failed_at.is_some()
        || crate::wakes::automatic_work_paused(c, &session.id)?
    {
        return Ok(Some(R::Stopped));
    }
    if !s.targets.iter().any(|t| {
        t.source.scope == AdminCaseScope::InScope
            && t.current.crew_id == s.case.crew_id
            && t.still_violating
    }) {
        return Ok(Some(R::NoInScopeTargets));
    }
    if judgments::configuration(c)?
        .config
        .is_none_or(|cfg| !cfg.enabled)
    {
        return Ok(Some(R::Unconfigured));
    }
    Ok(None)
}
fn summary(s: &Snapshot) -> serde_json::Value {
    let all: Vec<_> = s
        .targets
        .iter()
        .filter(|t| {
            t.source.scope == AdminCaseScope::InScope && t.current.crew_id == s.case.crew_id
        })
        .collect();
    let mut truncated = all.len() > 16;
    let targets:Vec<_>=all.iter().take(16).enumerate().map(|(index,t)| {
        let mut text=|v:&str| {if v.chars().count()>200 {truncated=true;}v.chars().take(200).collect::<String>()};
        serde_json::json!({"id":format!("t{index}"),"kind":t.source.kind,"status":t.current.status,"stillViolating":t.still_violating,"title":text(&t.current.title),"goal":text(&t.current.goal),"note":text(t.current.note.as_deref().unwrap_or(""))})
    }).collect();
    serde_json::json!({"scope":"source_crew_only","sampleOnly":true,"truncated":truncated,"totalInScope":all.len(),"targets":targets})
}
pub fn prepare(tx: &mut Tx<'_>, crew: &str, id: &str) -> Result<Option<DecisionSnapshot>> {
    use AdminCaseDecision as P;
    if crate::admin_case_execution::binding(tx.conn(), crew, id)?.is_some() {
        return Ok(None);
    }
    let now = tx.now_ms();
    let s = snapshot(tx.conn(), crew, id, now)?;
    let old = read_decision(tx.conn(), id)?;
    if old
        .as_ref()
        .is_some_and(|r| matches!(r.view.phase, P::Dispatched | P::Unknown))
    {
        return Ok(None);
    }
    if let Some(r) = &old
        && r.snapshot.fingerprint == s.fingerprint
        && !matches!(
            r.view.phase,
            P::Prepared | P::WaitingAdmission | P::Unconfigured
        )
    {
        return Ok(None);
    }
    let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
    let epoch_accepted = s.restore_matches
        || old.as_ref().is_some_and(|r| r.accepted_epoch == epoch)
        || can_revalidate(tx.conn(), &s, old.as_ref())?;
    let reason = if epoch_accepted {
        blocked(tx.conn(), &s)?
    } else {
        Some(AdminCaseDecisionReason::StateChanged)
    };
    let snap = DecisionSnapshot {
        crew: crew.into(),
        id: id.into(),
        source_session: s.case.source.session_id.clone(),
        decision_id: old
            .as_ref()
            .filter(|r| r.snapshot.fingerprint == s.fingerprint)
            .map(|r| r.snapshot.decision_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        fingerprint: s.fingerprint.clone(),
        summary: summary(&s),
        settings: judgments::configuration(tx.conn())?,
    };
    let phase = match reason {
        None => P::Prepared,
        Some(AdminCaseDecisionReason::Unconfigured) => P::Unconfigured,
        _ => P::Stale,
    };
    let r = DecisionSaved {
        version: 1,
        observed_at: now,
        accepted_epoch: if epoch_accepted {
            epoch
        } else {
            old.as_ref()
                .map(|r| r.accepted_epoch.clone())
                .unwrap_or_else(|| s.case.source.restore_epoch.clone())
        },
        snapshot: snap.clone(),
        view: AdminCaseDecisionView {
            decision_id: snap.decision_id.clone(),
            phase: phase.clone(),
            probability: None,
            adopted: false,
            reason: reason.unwrap_or(AdminCaseDecisionReason::Prepared),
            snapshot_witness: snap.fingerprint.clone(),
        },
    };
    save_decision(tx, &r)?;
    Ok((phase == P::Prepared).then_some(snap))
}
fn matching(
    tx: &mut Tx<'_>,
    s: &DecisionSnapshot,
    phase: AdminCaseDecision,
) -> Result<Option<DecisionSaved>> {
    let Some(mut r) = read_decision(tx.conn(), &s.id)? else {
        return Ok(None);
    };
    if r.snapshot.decision_id != s.decision_id
        || r.snapshot.fingerprint != s.fingerprint
        || r.snapshot.crew != s.crew
        || r.view.phase != phase
    {
        return Ok(None);
    }
    let current = snapshot(tx.conn(), &s.crew, &s.id, tx.now_ms())?;
    if current.fingerprint != s.fingerprint
        || r.accepted_epoch != crewd_store::judgment_state::epoch(tx.conn())?
        || blocked(tx.conn(), &current)?.is_some()
    {
        r.view.phase = AdminCaseDecision::Stale;
        r.view.adopted = false;
        r.view.reason = AdminCaseDecisionReason::StateChanged;
        save_decision(tx, &r)?;
        return Ok(None);
    }
    Ok(Some(r))
}
pub fn dispatch(tx: &mut Tx<'_>, s: &DecisionSnapshot) -> Result<bool> {
    let Some(mut r) = matching(tx, s, AdminCaseDecision::Prepared)? else {
        return Ok(false);
    };
    r.view.phase = AdminCaseDecision::Dispatched;
    save_decision(tx, &r)?;
    Ok(true)
}
pub fn defer(
    tx: &mut Tx<'_>,
    s: &DecisionSnapshot,
    reason: AdminCaseDecisionReason,
) -> Result<bool> {
    let phase = match reason {
        AdminCaseDecisionReason::NoAdmission => AdminCaseDecision::WaitingAdmission,
        AdminCaseDecisionReason::Unconfigured => AdminCaseDecision::Unconfigured,
        _ => return Err(invalid("invalid admin defer reason")),
    };
    let Some(mut r) = matching(tx, s, AdminCaseDecision::Prepared)? else {
        return Ok(false);
    };
    r.view.phase = phase;
    r.view.reason = reason;
    save_decision(tx, &r)?;
    Ok(true)
}
pub fn settle(
    tx: &mut Tx<'_>,
    s: &DecisionSnapshot,
    probability: Option<f64>,
    reason: AdminCaseDecisionReason,
) -> Result<bool> {
    let Some(mut r) = matching(tx, s, AdminCaseDecision::Dispatched)? else {
        return Ok(false);
    };
    if !matches!(
        reason,
        AdminCaseDecisionReason::NativeAnswered
            | AdminCaseDecisionReason::Generated
            | AdminCaseDecisionReason::ProviderUnavailable
            | AdminCaseDecisionReason::NoAdmission
    ) {
        return Err(invalid("invalid admin settlement reason"));
    }
    let native = reason == AdminCaseDecisionReason::NativeAnswered;
    let p = probability
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        .filter(|_| native);
    r.view.phase = if p.is_some() {
        AdminCaseDecision::Answered
    } else {
        AdminCaseDecision::Unknown
    };
    r.view.probability = p;
    r.view.reason = reason;
    let settings = judgments::configuration(tx.conn())?;
    r.view.adopted = p
        .zip(settings.config)
        .is_some_and(|(p, cfg)| cfg.enabled && p >= cfg.wake_yes);
    save_decision(tx, &r)?;
    Ok(true)
}
#[derive(Clone, Debug)]
pub struct CaseRef {
    pub crew: String,
    pub id: String,
}
/// Global durable sequence, independent of the per-crew display sample.
pub fn source_page(
    c: &Connection,
    after: Option<String>,
) -> Result<(Vec<CaseRef>, Option<String>)> {
    const P: &str = "admin.case.id.v1:";
    let lower = after.unwrap_or_else(|| P.into());
    if !lower.starts_with(P) {
        return Err(invalid("invalid admin scan cursor"));
    }
    let mut q =
        c.prepare("SELECT key,value FROM meta WHERE key>?1 AND key<?2 ORDER BY key LIMIT 65")?;
    let rows = q
        .query_map(params![lower, format!("{P}\u{10ffff}")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > 64;
    let next = if more { Some(rows[63].0.clone()) } else { None };
    let mut out = Vec::new();
    for (index, k) in rows.into_iter().take(64) {
        let id = index
            .strip_prefix(P)
            .ok_or_else(|| invalid("invalid admin index"))?
            .to_string();
        sequence(&id)?;
        // Identity is used only to route a strict subsequent get/prepare; damaged
        // payloads remain untouched and cannot stall healthy later source pages.
        let crew = meta_get(c, &k)?
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.get("view")?.get("crewId")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        out.push(CaseRef { crew, id });
    }
    Ok((out, next))
}
pub fn recover_page(tx: &mut Tx<'_>, after: Option<String>) -> Result<(usize, Option<String>)> {
    let (refs, next) = source_page(tx.conn(), after)?;
    let count = refs.len();
    for item in refs {
        // A corrupt optional decision is fail-closed for that case, not a reason
        // to prevent the entire runtime from starting. Preserve the raw payload.
        let Ok(Some(mut r)) = read_decision(tx.conn(), &item.id) else {
            continue;
        };
        if r.view.phase == AdminCaseDecision::Dispatched {
            r.view.phase = AdminCaseDecision::Unknown;
            r.view.adopted = false;
            r.view.reason = AdminCaseDecisionReason::RestartUnknown;
            save_decision(tx, &r)?;
        }
    }
    Ok((count, next))
}
fn project_at(c: &Connection, mut view: AdminCaseView, now: Option<i64>) -> Result<AdminCaseView> {
    if let Some(mut r) = read_decision(c, &view.id)? {
        if r.snapshot.crew != view.crew_id {
            return Err(invalid("admin decision crew mismatch"));
        }
        let changed = match snapshot(c, &view.crew_id, &view.id, now.unwrap_or(r.observed_at)) {
            Ok(current) => current.fingerprint != r.snapshot.fingerprint,
            Err(crate::CollabError::NotFound(_)) => true,
            Err(error) => return Err(error),
        };
        if changed || r.accepted_epoch != crewd_store::judgment_state::epoch(c)? {
            r.view.phase = AdminCaseDecision::Stale;
            r.view.adopted = false;
            r.view.reason = AdminCaseDecisionReason::StateChanged;
        }
        view.decision = r.view.phase.clone();
        view.decision_record = Some(r.view);
    }
    if let Some(execution) = crate::admin_case_execution::view(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )? {
        view.execution = execution.phase.clone();
        view.execution_record = Some(execution);
    }
    view.launch_record = crate::admin_case_launch::status(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )?;
    view.handoff_record = crate::admin_case_handoff::status_for_case(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )?;
    view.custody_record = crate::admin_case_custody::status_for_case(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )?;
    view.outcome_record = crate::admin_case_outcome::status_at(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )?;
    view.retirement_record = crate::session_retirement::case_record(
        c,
        &view.crew_id,
        &view.id,
        now.unwrap_or_else(crewd_store::now_ms),
    )?;
    if let Some(record) = &view.retirement_record {
        use crewd_proto::RetirementPhase as P;
        view.retirement = match record.phase {
            P::Prepared => AdminCaseRetirement::Prepared,
            P::Dispatched => AdminCaseRetirement::Dispatched,
            P::DecisionReady => AdminCaseRetirement::DecisionReady,
            P::ExitRequested => AdminCaseRetirement::ExitRequested,
            P::Parked => AdminCaseRetirement::Parked,
            P::Unknown => AdminCaseRetirement::Unknown,
            P::Stale => AdminCaseRetirement::Stale,
            P::Suppressed => AdminCaseRetirement::Suppressed,
        };
    }
    Ok(view)
}

fn can_revalidate(c: &Connection, s: &Snapshot, old: Option<&DecisionSaved>) -> Result<bool> {
    if old.is_some_and(|r| {
        !matches!(
            r.view.phase,
            AdminCaseDecision::Unconfigured
                | AdminCaseDecision::WaitingAdmission
                | AdminCaseDecision::Prepared
        )
    }) {
        return Ok(false);
    }
    let Some(b) = crewd_store::work_stop_checks::get(c, &s.case.source.session_id)? else {
        return Ok(false);
    };
    if b.epoch != s.case.source.budget_epoch
        || !b.streak.escalated
        || b.streak.last.len() != s.targets.len()
    {
        return Ok(false);
    }
    for t in &s.targets {
        if t.current.crew_id != t.source.crew_id
            || t.current.revision != t.source.revision
            || !t.still_violating
        {
            return Ok(false);
        }
        let n = work::number(&t.source.id)?;
        if !b.streak.last.iter().any(|v| {
            i64::from(v.item.0) == n
                && match t.source.kind {
                    AdminCaseTargetKind::Unanswered => v.kind == crewd_work::StopKind::Unanswered,
                    AdminCaseTargetKind::Overdue => v.kind == crewd_work::StopKind::Overdue,
                    AdminCaseTargetKind::ReadyUnscheduled => {
                        v.kind == crewd_work::StopKind::ReadyUnscheduled
                    }
                }
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn get_at(c: &Connection, p: &AdminCaseGetParams, now: i64) -> Result<AdminCaseView> {
    project_at(c, source_get(c, p)?, Some(now))
}
pub fn list_at(c: &Connection, p: &AdminCaseListParams, now: i64) -> Result<AdminCaseList> {
    let mut page = source_list(c, p)?;
    page.items = page
        .items
        .into_iter()
        .map(|v| {
            get_at(
                c,
                &AdminCaseGetParams {
                    crew_id: v.crew_id,
                    id: v.id,
                },
                now,
            )
        })
        .collect::<Result<_>>()?;
    Ok(page)
}

pub fn list(c: &Connection, p: &AdminCaseListParams) -> Result<AdminCaseList> {
    list_at(c, p, crewd_store::now_ms())
}

pub(crate) fn execution_eligibility(
    c: &Connection,
    crew: &str,
    id: &str,
    now: i64,
) -> Result<Option<(Snapshot, String)>> {
    let Some(record) = read_decision(c, id)? else {
        return Ok(None);
    };
    if record.view.phase != AdminCaseDecision::Answered
        || !record.view.adopted
        || record.snapshot.crew != crew
    {
        return Ok(None);
    }
    let current = snapshot(c, crew, id, now)?;
    if record.snapshot.fingerprint != current.fingerprint
        || record.accepted_epoch != crewd_store::judgment_state::epoch(c)?
        || blocked(c, &current)?.is_some()
    {
        return Ok(None);
    }
    Ok(Some((current, record.view.decision_id)))
}
