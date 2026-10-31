//! Durable typed escalation sources. Observation grants no execution authority.
use crate::{Result, Tx, judgments, work};
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
fn key(crew: &str, id: &str) -> Result<String> {
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
fn revision(view: &AdminCaseView) -> Result<String> {
    let mut v = view.clone();
    v.revision.clear();
    judgments::fingerprint(&v)
}
fn decode(raw: &str, crew: &str, id: &str) -> Result<AdminCaseView> {
    let s: Saved = serde_json::from_str(raw).map_err(StoreError::from)?;
    let v = s.view;
    if s.version != 1
        || v.id != id
        || v.crew_id != crew
        || v.source.budget_epoch < 1
        || v.source.session_id.is_empty()
        || v.source.announcement_id.is_empty()
        || v.source.restore_epoch.is_empty()
        || v.targets.is_empty()
        || revision(&v)? != v.revision
    {
        return Err(invalid("invalid admin case record"));
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
pub fn get(c: &Connection, p: &AdminCaseGetParams) -> Result<AdminCaseView> {
    sequence(&p.id).map_err(|_| crate::resource_acl::invalid("id"))?;
    let raw =
        meta_get(c, &key(&p.crew_id, &p.id)?)?.ok_or_else(|| crate::resource_acl::invalid("id"))?;
    decode(&raw, &p.crew_id, &p.id)
}
/// Cursor is a restore-bound monotonic sequence, not a timestamp or random id.
pub fn list(c: &Connection, p: &AdminCaseListParams) -> Result<AdminCaseList> {
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
    let s = crewd_store::sessions::get(tx.conn(), session)?
        .ok_or_else(|| invalid("missing escalation source"))?;
    let budget = crewd_store::work_stop_checks::get(tx.conn(), session)?
        .ok_or_else(|| invalid("missing escalation budget"))?;
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
    if let Some(id) = meta_get(tx.conn(), &source_key)? {
        let existing = get(
            tx.conn(),
            &AdminCaseGetParams {
                crew_id: s.crew_id,
                id,
            },
        )?;
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
        id,
        crew_id: s.crew_id,
        revision: String::new(),
        created_at: tx.now_ms(),
        source: AdminCaseSource {
            session_id: session.into(),
            budget_epoch: budget.epoch,
            restore_epoch: crewd_store::judgment_state::epoch(tx.conn())?,
            announcement_id: announcement.into(),
        },
        targets: full,
        decision: AdminCaseDecision::Observed,
        execution: AdminCaseExecution::NotStarted,
        retirement: AdminCaseRetirement::NotStarted,
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
    let case = get(
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
