//! Bounded global views over the authoritative work state.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, asks, crews};
use crewd_proto::{WorkEvidence, WorkListParams, WorkPage, WorkPlan};
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_work::{Blocker, DoneLevel, Evidence, Status, WorkBook, WorkItem};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
fn invalid(detail: &str) -> CollabError {
    StoreError::Invalid(detail.into()).into()
}
pub(crate) fn number(id: &str) -> Result<i64> {
    match id.strip_prefix('W').and_then(|n| n.parse::<i64>().ok()) {
        Some(n) if n > 0 && id == format!("W{n}") => Ok(n),
        _ => Err(CollabError::Invalid(
            Refusal::new(Code::WorkBadId).arg("input", id),
        )),
    }
}
pub(crate) fn reference(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    local: impl std::fmt::Display,
) -> Result<String> {
    let _ = crew; // Object crew is carried by the sole authority, not the caller's source scope.
    crewd_store::work_organization::reference_for_core(conn, kind, &local.to_string())?
        .map(|r| r.id())
        .ok_or_else(|| invalid("work link has no issued global identity"))
}
fn pending_view(t: &crewd_work::PendingTransfer, short: bool) -> crewd_proto::WorkPendingTransfer {
    crewd_proto::WorkPendingTransfer {
        to: t.to.to_string(),
        requested_by: t.requested_by.to_string(),
        note: if short {
            t.note.chars().take(256).collect()
        } else {
            t.note.clone()
        },
        note_truncated: short && t.note.chars().nth(256).is_some(),
        at: crewd_store::ts_from_ms(t.at.0),
    }
}
fn project(conn: &Connection, r: &ObjectReference, book: &WorkBook) -> Result<WorkPlan> {
    let local = core_plan(conn, r)?;
    let item: &WorkItem = book
        .state()
        .items
        .get(&local)
        .ok_or_else(|| invalid("plan reference has no authoritative work item"))?;
    let status = match item.status {
        Status::Open => "open",
        Status::Active => "active",
        Status::Blocked => "blocked",
        Status::Done => "done",
        Status::Dropped => "dropped",
    };
    let level = match item.body.level {
        DoneLevel::Local => "local",
        DoneLevel::Public => "public",
        DoneLevel::Production => "production",
    };
    let plans = |id| reference(conn, ObjectKind::Plan, &r.crew_id, id);
    let evidence = item.evidence.as_ref().map(|e| {
        let (kind, value) = match e {
            Evidence::Commit(v) => ("commit", v),
            Evidence::Url(v) => ("url", v),
            Evidence::Text(v) => ("text", v),
        };
        WorkEvidence {
            kind: kind.into(),
            value: value.clone(),
        }
    });
    Ok(WorkPlan {
        id: r.id(),
        crew_id: r.crew_id.clone(),
        frozen: crews::is_archived(conn, &r.crew_id)?,
        title: item.title.clone(),
        goal: item.body.goal.clone(),
        level: level.into(),
        acceptance: item.body.acceptance.clone(),
        owner: item.owner.as_ref().map(ToString::to_string),
        pending_transfer: item
            .pending_transfer
            .as_ref()
            .map(|t| pending_view(t, false)),
        status: status.into(),
        parent: item.parent.map(plans).transpose()?,
        depends_on: item
            .depends_on
            .iter()
            .copied()
            .map(plans)
            .collect::<Result<_>>()?,
        sources: item
            .sources
            .iter()
            .map(|id| reference(conn, ObjectKind::Requirement, &r.crew_id, id))
            .collect::<Result<_>>()?,
        blocked_on: item
            .blocked_on
            .iter()
            .map(|b| match b {
                Blocker::Human(id) => reference(conn, ObjectKind::HumanTask, &r.crew_id, id),
                Blocker::Item(id) => plans(*id),
            })
            .collect::<Result<_>>()?,
        next_check_at: item.next_check_at.map(|t| crewd_store::ts_from_ms(t.0)),
        revision: item.revision.to_string(),
        evidence,
        note: item.last_report.as_ref().map(|n| n.text.clone()),
        created_at: crewd_store::ts_from_ms(item.created_at.0),
        updated_at: crewd_store::ts_from_ms(item.updated_at.0),
    })
}
/// Commit follow-up with the state change, so a dropped observer cannot strand owners.
/// One trigger/owner produces one durable wake, even when many items are released.
pub(crate) fn wake_cleared(
    tx: &mut crate::Tx<'_>,
    _crew: &str,
    events: &[crewd_work::Event],
    ready: &[(crewd_work::WorkId, crewd_work::SessionId)],
    kind: crate::wakes::WakeKind,
    priority: crate::wakes::WakePriority,
    source: &str,
) -> Result<()> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for event in events {
        if let crewd_work::EventKind::BlockerCleared { id, owner, .. } = &event.kind {
            let reference = crewd_store::work_organization::reference_for_core(
                tx.conn(),
                ObjectKind::Plan,
                &id.to_string(),
            )?
            .ok_or_else(|| invalid("released work has no issued identity"))?;
            let id = reference.id();
            tx.note(crate::Change::WorkChanged {
                crew_id: reference.crew_id.clone(),
                id: id.clone(),
            });
            if let Some(owner) = owner
                && !crews::is_archived(tx.conn(), &reference.crew_id)?
            {
                owners.entry(owner.to_string()).or_default().push(id);
            }
        }
    }
    for (id, owner) in ready {
        let reference = crewd_store::work_organization::reference_for_core(
            tx.conn(),
            ObjectKind::Plan,
            &id.to_string(),
        )?
        .ok_or_else(|| invalid("ready work has no issued identity"))?;
        let id = reference.id();
        tx.note(crate::Change::WorkChanged {
            crew_id: reference.crew_id.clone(),
            id: id.clone(),
        });
        if !crews::is_archived(tx.conn(), &reference.crew_id)? {
            owners.entry(owner.to_string()).or_default().push(id);
        }
    }
    for (owner, mut ids) in owners {
        // Purged owners have no runnable target. Preserve the answer and ledger;
        // ownership repair belongs to patrol, not to the answering transaction.
        if crewd_store::sessions::get(tx.conn(), &owner)?.is_none() {
            continue;
        }
        ids.sort();
        ids.dedup();
        let total = ids.len();
        let mut note = serde_json::json!({"unblockedWork":ids.iter().take(50).collect::<Vec<_>>(),"total":total,"partial":total>50});
        if kind == crate::wakes::WakeKind::System {
            // All dispatch targets stay in the durable record. Rendering strips
            // this metadata and still sends at most 50 current IDs to the model.
            note["targets"] = serde_json::json!(ids);
        }
        let note = note.to_string();
        crate::wakes::enqueue(
            tx,
            crate::wakes::WakeRequest {
                session: owner,
                kind,
                source: source.into(),
                priority,
                not_before_ms: None,
                note: Some(note),
                origin: None,
            },
        )?;
    }
    Ok(())
}

pub(crate) enum FollowupNote {
    Ordinary,
    Stale,
    Current(String),
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredFollowup {
    #[serde(alias = "assignedWork")]
    unblocked_work: Vec<String>,
    total: usize,
    partial: bool,
    targets: Option<Vec<String>>,
    assignment_note: Option<String>,
    note_truncated: Option<bool>,
    assigned_revision: Option<String>,
}
/// Recheck current ownership/terminal state at render time. Corrupt books remain
/// errors, so a temporary DB fault cannot permanently discard a wake.
pub(crate) fn followup_note(
    conn: &Connection,
    row: &crate::wakes::WakeRow,
    now: i64,
) -> Result<FollowupNote> {
    if row.kind == crate::wakes::WakeKind::StopCheck {
        return crate::stop_checks::render_note(conn, row, now);
    }
    if row.kind == crate::wakes::WakeKind::Checkpoint {
        return crate::checkpoints::render_note(conn, row, now);
    }
    if row.kind != crate::wakes::WakeKind::System
        || !(row.source.starts_with("work-done:") || row.source.starts_with("work-assigned:"))
    {
        return Ok(FollowupNote::Ordinary);
    }
    let note: StoredFollowup = serde_json::from_str(row.note.as_deref().unwrap_or(""))
        .map_err(|_| invalid("work follow-up metadata is invalid"))?;
    if let Some(ids) = &note.targets
        && (ids.len() != note.total
            || note.partial != (note.total > 50)
            || note.unblocked_work != ids.iter().take(50).cloned().collect::<Vec<_>>())
    {
        return Err(invalid("work follow-up target metadata is inconsistent"));
    }
    let assignment = row.source.starts_with("work-assigned:");
    if assignment
        && (note.total != 1
            || note
                .assignment_note
                .as_ref()
                .is_none_or(|n| n.chars().nth(256).is_some())
            || note.note_truncated.is_none()
            || note.assigned_revision.is_none())
    {
        return Err(invalid("assignment follow-up metadata is incomplete"));
    }
    let targets = match note.targets {
        Some(ids) => ids,
        None if !note.partial && note.total == note.unblocked_work.len() => note.unblocked_work,
        // Old truncated records do not identify the omitted items. Conservatively
        // retain them instead of incorrectly declaring all targets stale.
        None if note.partial => return Ok(FollowupNote::Ordinary),
        None => return Err(invalid("work follow-up target count is invalid")),
    };
    let book = asks::load_org(conn)?;
    let mut current = Vec::new();
    for id in targets {
        let Some(r) = object_refs::get(conn, ObjectKind::Plan, number(&id)?)? else {
            continue;
        };
        let frozen = crews::is_archived(conn, &r.crew_id)?;
        let local = core_plan(conn, &r)?;
        let item = book
            .state()
            .items
            .get(&local)
            .ok_or_else(|| invalid("work follow-up target has no authoritative item"))?;
        if !frozen
            && item
                .owner
                .as_ref()
                .is_some_and(|o| o.as_str() == row.session)
            && !matches!(item.status, Status::Done | Status::Dropped)
            && (!assignment
                || note.assigned_revision.as_deref() == Some(item.revision.to_string().as_str()))
        {
            current.push(id);
        }
    }
    current.sort();
    current.dedup();
    if current.is_empty() {
        return Ok(FollowupNote::Stale);
    }
    let total = current.len();
    current.truncate(50);
    Ok(FollowupNote::Current(
        if row.source.starts_with("work-assigned:") {
            serde_json::json!({"assignedWork":current,"total":total,"partial":total>50,"assignmentNote":note.assignment_note,"noteTruncated":note.note_truncated}).to_string()
        } else {
            serde_json::json!({"unblockedWork":current,"total":total,"partial":total>50})
                .to_string()
        },
    ))
}

/// All organization objects remain readable, including removed crews.
pub fn get(conn: &Connection, id: &str) -> Result<WorkPlan> {
    let r = object_refs::get(conn, ObjectKind::Plan, number(id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", id)))?;
    project(conn, &r, &asks::load(conn, &r.crew_id)?)
}
pub fn list(conn: &Connection, p: WorkListParams) -> Result<WorkPage> {
    let limit = p.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadLimit)));
    }
    let after = p.after.as_deref().map(number).transpose()?.unwrap_or(0);
    if let Some(crew) = &p.crew_id {
        crews::get(conn, crew)?;
    }
    let mut stmt = conn.prepare("SELECT r.number,r.crew_id,r.source_id,r.created_at FROM object_refs r LEFT JOIN organization_work_book b ON b.singleton=1 LEFT JOIN crews c ON c.id=r.crew_id WHERE r.kind='plan' AND r.number>?1 AND (?2 IS NULL OR r.crew_id=?2) AND (?3 IS NULL OR b.state IS NULL OR json_type(b.state,'$.items.'||r.core_id) IS NULL OR json_extract(b.state,'$.items.'||r.core_id||'.owner')=?3) AND (?4=0 OR b.state IS NULL OR json_type(b.state,'$.items.'||r.core_id) IS NULL OR json_extract(b.state,'$.items.'||r.core_id||'.status') NOT IN ('done','dropped')) AND (?6=1 OR c.archived_at IS NULL) ORDER BY r.number LIMIT ?5")?;
    let mut refs = stmt
        .query_map(
            params![
                after,
                p.crew_id,
                p.owner,
                p.live_only,
                i64::from(limit) + 1,
                p.include_removed.unwrap_or(true)
            ],
            |r| {
                Ok(ObjectReference {
                    number: r.get(0)?,
                    kind: ObjectKind::Plan,
                    crew_id: r.get(1)?,
                    source_id: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = refs.len() > usize::from(limit);
    refs.truncate(usize::from(limit));
    let next_after = if more {
        refs.last().map(ObjectReference::id)
    } else {
        None
    };
    let book = asks::load_org(conn)?;
    let mut items = Vec::with_capacity(refs.len());
    for r in refs {
        items.push(project(conn, &r, &book)?.into());
    }
    Ok(WorkPage { items, next_after })
}
pub(crate) fn actor(
    conn: &Connection,
    by: &crate::Sender,
    _crew: &str,
) -> Result<crewd_work::Actor> {
    use crewd_work::{Actor, AuthorityScope, CrewId, Role};
    match by {
        crate::Sender::Human => Ok(Actor::Human),
        crate::Sender::System => Err(CollabError::Forbidden(Refusal::new(
            Code::WorkActorForbidden,
        ))),
        crate::Sender::Session(id) => {
            let session = crate::sessions::get(conn, id)?;
            crews::refuse_archived(conn, &session.crew_id)?;
            let mut roles = BTreeMap::new();
            let mut q =
                conn.prepare("SELECT id FROM crews WHERE archived_at IS NULL ORDER BY id")?;
            for row in q.query_map([], |r| r.get::<_, String>(0))? {
                let crew = row?;
                if crews::acts_as_captain_of(conn, id, &crew)? {
                    roles.insert(CrewId::new(crew), Role::Captain);
                } else if session.crew_id == crew && session.role == crewd_proto::Role::Admin {
                    roles.insert(CrewId::new(crew), Role::Admin);
                }
            }
            Ok(Actor::scoped(
                id,
                AuthorityScope {
                    home: CrewId::new(session.crew_id),
                    roles,
                },
            ))
        }
    }
}
pub(crate) fn core_plan(conn: &Connection, r: &ObjectReference) -> Result<crewd_work::WorkId> {
    crewd_store::work_organization::core_for_reference(conn, ObjectKind::Plan, r.number)?
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| invalid("plan reference has no authoritative core identity"))
}
fn timestamp(value: Option<&str>) -> Result<Option<crewd_work::Ts>> {
    value
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|t| crewd_work::Ts(t.timestamp_millis()))
                .map_err(|_| CollabError::Invalid(Refusal::new(Code::WorkBadTime).arg("value", v)))
        })
        .transpose()
}
pub(crate) fn local_reference(
    conn: &Connection,
    kind: ObjectKind,
    crew: &str,
    id: &str,
) -> Result<String> {
    let n = id
        .strip_prefix(kind.prefix())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n > 0 && id == format!("{}{n}", kind.prefix()))
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadId).arg("input", id)))?;
    let r = object_refs::get(conn, kind, n)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", id)))?;
    let _ = crew;
    crews::refuse_archived(conn, &r.crew_id)?;
    crewd_store::work_organization::core_for_reference(conn, kind, n)?
        .ok_or_else(|| invalid("work command reference has no core identity"))
}
pub(crate) fn local_plan(conn: &Connection, crew: &str, id: &str) -> Result<crewd_work::WorkId> {
    local_reference(conn, ObjectKind::Plan, crew, id)?
        .parse()
        .map_err(|_| invalid("work command plan identity is invalid"))
}
pub(crate) fn changed(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    crew: &str,
    id: &str,
) -> Result<()> {
    if let crate::Sender::Session(session) = by {
        crate::sessions::note_progress(tx, session, crate::ProgressKind::WorkItem)?;
    }
    tx.note(crate::Change::WorkChanged {
        crew_id: crew.into(),
        id: id.into(),
    });
    Ok(())
}
pub(crate) fn prepare_item(
    conn: &Connection,
    actor: &crewd_work::Actor,
    p: crewd_proto::WorkCreateParams,
) -> Result<crewd_work::NewItem> {
    use crewd_work::{Body, ItemRef, NewItem, SessionId};
    crews::refuse_archived(conn, &p.crew_id)?;
    let owner = p.owner.or_else(|| actor.session().map(ToString::to_string));
    if let Some(owner) = &owner {
        let owner = crate::sessions::get(conn, owner)?;
        crews::refuse_archived(conn, &owner.crew_id)?;
    }
    Ok(NewItem {
        crew_id: Some(crewd_work::CrewId::new(&p.crew_id)),
        title: p.title,
        body: Body {
            goal: p.goal,
            acceptance: p.acceptance,
            level: match p.level {
                crewd_proto::WorkLevel::Local => DoneLevel::Local,
                crewd_proto::WorkLevel::Public => DoneLevel::Public,
                crewd_proto::WorkLevel::Production => DoneLevel::Production,
            },
        },
        owner: owner.map(|id| SessionId::new(&id)),
        parent: p
            .parent
            .as_deref()
            .map(|id| local_plan(conn, &p.crew_id, id).map(ItemRef::Id))
            .transpose()?,
        depends_on: p
            .depends_on
            .iter()
            .map(|id| local_plan(conn, &p.crew_id, id).map(ItemRef::Id))
            .collect::<Result<_>>()?,
        sources: p
            .sources
            .iter()
            .map(|id| {
                local_reference(conn, ObjectKind::Requirement, &p.crew_id, id)?
                    .parse()
                    .map_err(|_| invalid("work command requirement identity is invalid"))
            })
            .collect::<Result<_>>()?,
        next_check_at: timestamp(p.next_check_at.as_deref())?,
        channel: None,
    })
}

pub fn create(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    p: crewd_proto::WorkCreateParams,
) -> Result<WorkPlan> {
    let actor = actor(tx.conn(), by, &p.crew_id)?;
    let item = prepare_item(tx.conn(), &actor, p.clone())?;
    let local = asks::with_book(tx, &p.crew_id, |b, now| {
        b.create_item(&actor, now, item).map(|a| a.value)
    })?;
    let id = reference(tx.conn(), ObjectKind::Plan, &p.crew_id, local)?;
    changed(tx, by, &p.crew_id, &id)?;
    get(tx.conn(), &id)
}
pub(crate) fn check_revision(item: &WorkItem, id: &str, revision: u64) -> Result<()> {
    if item.revision != revision {
        let note = item
            .last_report
            .as_ref()
            .map(|n| n.text.chars().take(256).collect::<String>());
        let note_truncated = item
            .last_report
            .as_ref()
            .is_some_and(|n| n.text.chars().nth(256).is_some());
        let mut refusal = Refusal::new(Code::WorkRevisionConflict).arg("id", id);
        refusal.current_work = Some(Box::new(crewd_proto::WorkConflictFacts {
            id: id.into(),
            revision: item.revision.to_string(),
            owner: item.owner.as_ref().map(ToString::to_string),
            status: match item.status {
                Status::Open => "open",
                Status::Active => "active",
                Status::Blocked => "blocked",
                Status::Done => "done",
                Status::Dropped => "dropped",
            }
            .into(),
            next_check_at: item.next_check_at.map(|t| crewd_store::ts_from_ms(t.0)),
            blocker_count: item.blocked_on.len(),
            note,
            note_truncated,
            pending_transfer: item
                .pending_transfer
                .as_ref()
                .map(|t| pending_view(t, true)),
        }));
        return Err(CollabError::Conflict(refusal));
    }
    Ok(())
}
pub fn update(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    p: crewd_proto::WorkUpdateParams,
) -> Result<WorkPlan> {
    use crewd_proto::WorkUpdateStatus as S;
    use crewd_work::{Progress, Report};
    let n = number(&p.id)?;
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, n)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = actor(tx.conn(), by, &r.crew_id)?;
    let local = local_plan(tx.conn(), &r.crew_id, &p.id)?;
    let revision = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|n| p.expected_revision == n.to_string())
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    let done = p.status == Some(S::Done);
    if (!done && (p.evidence.is_some() || p.note.as_ref().is_none_or(|n| n.trim().is_empty())))
        || (done && p.next_check_at.is_some())
    {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let next = timestamp(p.next_check_at.as_deref())?;
    let evidence = p.evidence.map(|e| match e.kind {
        crewd_proto::WorkEvidenceKind::Commit => Evidence::Commit(e.value),
        crewd_proto::WorkEvidenceKind::Url => Evidence::Url(e.value),
        crewd_proto::WorkEvidenceKind::Text => Evidence::Text(e.value),
    });
    // The revision comparison and command run inside the same single-writer transaction.
    let current = asks::load(tx.conn(), &r.crew_id)?;
    let item = current
        .item(local)
        .ok_or_else(|| invalid("plan reference has no authoritative item"))?;
    check_revision(item, &p.id, revision)?;
    let (released, ready) = asks::with_book(tx, &r.crew_id, |b, now| {
        if !done || p.note.is_some() {
            b.report(
                &actor,
                now,
                local,
                Report {
                    status: match p.status {
                        Some(S::Open) => Some(Progress::Open),
                        Some(S::Active) => Some(Progress::Active),
                        _ => None,
                    },
                    next_check_at: next,
                    note: p.note.unwrap_or_default(),
                },
            )?;
        }
        if done {
            // Plain dependency edges can become ready without an explicit BlockOn.
            // Compare real before/after state, rather than inventing ledger events.
            let candidates: Vec<_> = b
                .state()
                .items
                .values()
                .filter(|i| {
                    matches!(i.status, Status::Open | Status::Active)
                        && i.depends_on.contains(&local)
                        && !i
                            .depends_on
                            .iter()
                            .all(|d| b.state().items[d].status == Status::Done)
                })
                .map(|i| i.id)
                .collect();
            let ap = b.mark_done(&actor, now, local, evidence)?;
            let ready = candidates
                .into_iter()
                .filter_map(|id| {
                    let i = &b.state().items[&id];
                    (i.blocked_on.is_empty()
                        && i.depends_on
                            .iter()
                            .all(|d| b.state().items[d].status == Status::Done))
                    .then(|| i.owner.clone().map(|owner| (id, owner)))
                    .flatten()
                })
                .collect();
            return Ok((ap.events, ready));
        }
        Ok((Vec::new(), Vec::new()))
    })?;
    wake_cleared(
        tx,
        &r.crew_id,
        &released,
        &ready,
        crate::wakes::WakeKind::System,
        crate::wakes::WakePriority::System,
        &format!("work-done:{}:{}", p.id, p.expected_revision),
    )?;
    changed(tx, by, &r.crew_id, &p.id)?;
    get(tx.conn(), &p.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};
    use crewd_work::{Actor, ItemRef, NewItem};
    fn create(s: &mut crewd_store::Store, crew: &str, title: &str) -> String {
        ok(s, |tx| {
            asks::with_book(tx, crew, |b, now| {
                b.create_item(&Actor::Human, now, NewItem::titled(title))
            })
        });
        list(
            s.conn(),
            WorkListParams {
                crew_id: Some(crew.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .items
        .last()
        .unwrap()
        .id
        .clone()
    }
    #[test]
    fn global_ids_do_not_alias_local_ids_and_pagination_spans_crews() {
        let mut s = store();
        let a = create(&mut s, "crew-a", "first");
        let b = create(&mut s, "crew-b", "second");
        assert_ne!(a, b);
        let first = list(
            s.conn(),
            WorkListParams {
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.items[0].title, "first");
        assert_eq!(first.next_after, Some(a.clone()));
        let second = list(
            s.conn(),
            WorkListParams {
                after: first.next_after,
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second.items[0].title, "second");
        assert_eq!(second.next_after, None);
        assert_eq!(get(s.conn(), &b).unwrap().crew_id, "crew-b");
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='removed' WHERE id='crew-b'",
                [],
            )
            .unwrap();
        assert_eq!(get(s.conn(), &b).unwrap().title, "second");
        assert_eq!(
            list(s.conn(), WorkListParams::default())
                .unwrap()
                .items
                .len(),
            2
        );
    }
    #[test]
    fn links_are_global_and_large_identifiers_stay_exact_strings() {
        let mut s = store();
        s.conn()
            .execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES('object_refs',9007199254740992)",
                [],
            )
            .unwrap();
        let parent = create(&mut s, "crew-a", "parent");
        create(&mut s, "crew-b", "different crew local W1");
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.create_item(
                    &Actor::Human,
                    now,
                    NewItem::titled("child").under(ItemRef::Id(crewd_work::WorkId(1))),
                )
            })
        });
        let page = list(
            s.conn(),
            WorkListParams {
                crew_id: Some("crew-a".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.items[1].parent, Some(parent.clone()));
        assert_eq!(parent, "W9007199254740993");
        let wire = serde_json::to_value(&page).unwrap();
        assert_eq!(wire["items"][0]["id"], parent);
        assert!(wire["items"][0]["revision"].is_string());
        assert!(wire["items"][0].get("goal").is_none());
        assert!(wire["items"][0].get("acceptance").is_none());
    }
    #[test]
    fn malformed_queries_and_missing_authority_are_refused() {
        let mut s = store();
        for id in ["W0", "W01", "H1", "W+1", "W9223372036854775808"] {
            assert!(matches!(get(s.conn(), id), Err(CollabError::Invalid(_))));
        }
        assert!(matches!(
            get(s.conn(), "W999"),
            Err(CollabError::NotFound(_))
        ));
        for limit in [0, 201] {
            assert!(matches!(
                list(
                    s.conn(),
                    WorkListParams {
                        limit: Some(limit),
                        ..Default::default()
                    }
                ),
                Err(CollabError::Invalid(_))
            ));
        }
        let id = create(&mut s, "crew-a", "real");
        s.conn()
            .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
            .unwrap();
        assert!(matches!(get(s.conn(), &id), Err(CollabError::Store(_))));
        assert!(matches!(
            list(s.conn(), WorkListParams::default()),
            Err(CollabError::Store(_))
        ));
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, store};
    use crate::{Change, ProgressKind, Sender};
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    fn input() -> WorkCreateParams {
        WorkCreateParams {
            crew_id: "crew-a".into(),
            title: "synthetic plan".into(),
            ..Default::default()
        }
    }
    fn finish(p: &WorkPlan) -> WorkUpdateParams {
        WorkUpdateParams {
            id: p.id.clone(),
            expected_revision: p.revision.clone(),
            status: Some(WorkUpdateStatus::Done),
            ..Default::default()
        }
    }
    #[test]
    fn owned_work_reports_real_progress_and_completes_without_evidence() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let (p, changes) = crate::run(&mut s, NOW, |tx| create(tx, &by, input())).unwrap();
        assert_eq!(p.owner.as_deref(), Some("w-a1"));
        assert!(changes.contains(&Change::Progress {
            session_id: "w-a1".into(),
            kind: ProgressKind::WorkItem
        }));
        assert!(changes.contains(&Change::WorkChanged {
            crew_id: "crew-a".into(),
            id: p.id.clone()
        }));
        let active = ok(&mut s, |tx| {
            update(
                tx,
                &by,
                WorkUpdateParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    status: Some(WorkUpdateStatus::Active),
                    note: Some("next concrete step".into()),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60_000)),
                    ..Default::default()
                },
            )
        });
        assert_eq!(active.status, "active");
        assert_eq!(active.note.as_deref(), Some("next concrete step"));
        let done = ok(&mut s, |tx| update(tx, &by, finish(&active)));
        assert_eq!(done.status, "done");
        assert!(done.evidence.is_none());
        assert!(done.next_check_at.is_none());
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            3
        );
    }
    #[test]
    fn stale_revision_and_unauthorized_actor_leave_all_state_unchanged() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        assert!(matches!(
            err(&mut s, |tx| update(
                tx,
                &Sender::Session("w-a2".into()),
                finish(&p)
            )),
            CollabError::Work(crewd_work::Rejected::Forbidden(_))
        ));
        assert!(matches!(
            err(&mut s, |tx| update(
                tx,
                &Sender::Session("cap-b".into()),
                finish(&p)
            )),
            CollabError::Work(crewd_work::Rejected::Forbidden(_))
        ));
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
        let done = ok(&mut s, |tx| update(tx, &by, finish(&p)));
        let stale = err(&mut s, |tx| update(tx, &by, finish(&p)));
        assert!(matches!(stale, CollabError::Conflict(_)));
        assert_eq!(get(s.conn(), &p.id).unwrap(), done);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn revision_conflict_captures_bounded_unicode_facts_without_writing() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let mut args = input();
        args.goal = "LONG_GOAL_PRIVATE".repeat(1000);
        args.acceptance = "LONG_ACCEPTANCE_PRIVATE".repeat(1000);
        let p = ok(&mut s, |tx| create(tx, &by, args));
        let active = ok(&mut s, |tx| {
            update(
                tx,
                &by,
                WorkUpdateParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    status: Some(WorkUpdateStatus::Active),
                    next_check_at: Some(crewd_store::ts_from_ms(NOW + 60_000)),
                    note: Some("下一步🙂".repeat(400)),
                    ..Default::default()
                },
            )
        });
        let error = err(&mut s, |tx| update(tx, &by, finish(&p)));
        let CollabError::Conflict(r) = error else {
            panic!("stale CAS must be a conflict")
        };
        assert_eq!(r.code, Code::WorkRevisionConflict);
        let facts = r.current_work.unwrap();
        assert_eq!(facts.id, p.id);
        assert_eq!(facts.revision, active.revision);
        assert_eq!(facts.owner, active.owner);
        assert_eq!(facts.status, "active");
        assert_eq!(facts.next_check_at, active.next_check_at);
        assert_eq!(facts.note.as_ref().unwrap().chars().count(), 256);
        assert!(facts.note_truncated);
        let encoded = serde_json::to_string(&facts).unwrap();
        assert!(encoded.len() < 4096);
        assert!(!encoded.contains("LONG_GOAL"));
        assert!(!encoded.contains("LONG_ACCEPTANCE"));
        assert_eq!(get(s.conn(), &p.id).unwrap(), active);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn owner_pages_span_crews_filter_finished_and_do_not_hide_missing_books() {
        let mut s = store();
        let a = ok(&mut s, |tx| {
            create(tx, &Sender::Session("w-a1".into()), input())
        });
        let mut cross = input();
        cross.crew_id = "crew-b".into();
        cross.owner = Some("w-a1".into());
        let b = ok(&mut s, |tx| create(tx, &Sender::Human, cross));
        let mut other = input();
        other.crew_id = "crew-b".into();
        other.owner = Some("w-b1".into());
        ok(&mut s, |tx| create(tx, &Sender::Human, other));
        let mine = |after, live_only| WorkListParams {
            owner: Some("w-a1".into()),
            live_only,
            after,
            limit: Some(1),
            ..Default::default()
        };
        let first = list(s.conn(), mine(None, true)).unwrap();
        assert_eq!(first.items[0].id, a.id);
        assert_eq!(first.next_after, Some(a.id.clone()));
        let second = list(s.conn(), mine(first.next_after, true)).unwrap();
        assert_eq!(second.items[0].id, b.id);
        assert!(second.next_after.is_none());
        ok(&mut s, |tx| update(tx, &Sender::Human, finish(&a)));
        assert_eq!(list(s.conn(), mine(None, true)).unwrap().items[0].id, b.id);
        assert_eq!(list(s.conn(), mine(None, false)).unwrap().items[0].id, a.id);
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='frozen' WHERE id='crew-b'",
                [],
            )
            .unwrap();
        let mut active_only = mine(None, true);
        active_only.include_removed = Some(false);
        assert!(list(s.conn(), active_only).unwrap().items.is_empty());
        assert!(list(s.conn(), mine(None, true)).unwrap().items[0].frozen);
        s.conn()
            .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
            .unwrap();
        assert!(matches!(
            list(s.conn(), mine(None, true)),
            Err(CollabError::Store(_))
        ));
    }
    #[test]
    fn stale_report_cannot_overwrite_a_newer_next_step() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let original = ok(&mut s, |tx| create(tx, &by, input()));
        let update_request = |note: &str| WorkUpdateParams {
            id: original.id.clone(),
            expected_revision: original.revision.clone(),
            note: Some(note.into()),
            ..Default::default()
        };
        let current = ok(&mut s, |tx| {
            update(tx, &by, update_request("newer next step"))
        });
        let e = err(&mut s, |tx| {
            update(tx, &by, update_request("stale next step"))
        });
        assert_eq!(e.refusal().unwrap().code, Code::WorkRevisionConflict);
        assert_eq!(get(s.conn(), &original.id).unwrap(), current);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn target_captain_and_management_captain_use_current_authority() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        let done = ok(&mut s, |tx| {
            update(tx, &Sender::Session("cap-a".into()), finish(&p))
        });
        assert_eq!(done.status, "done");
        let chief = crate::testkit::org_captain(&mut s);
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        assert_eq!(
            ok(&mut s, |tx| update(tx, &Sender::Session(chief), finish(&p))).status,
            "done"
        );
    }
    #[test]
    fn failed_progress_write_rolls_back_work_events_and_issued_references() {
        let mut s = store();
        s.conn().execute_batch("CREATE TRIGGER fail_work_progress BEFORE UPDATE OF last_progress_at ON sessions BEGIN SELECT RAISE(ABORT,'synthetic progress failure'); END;").unwrap();
        assert!(matches!(
            err(&mut s, |tx| create(
                tx,
                &Sender::Session("w-a1".into()),
                input()
            )),
            CollabError::Store(_)
        ));
        assert!(
            list(s.conn(), WorkListParams::default())
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.conn()
                .query_row("SELECT COUNT(*) FROM work_books", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(
            s.conn()
                .query_row(
                    "SELECT last_progress_at FROM sessions WHERE id='w-a1'",
                    [],
                    |r| r.get::<_, Option<String>>(0)
                )
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn frozen_crews_reject_writes_and_cross_crew_links_never_alias() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        let mut cross = input();
        cross.crew_id = "crew-b".into();
        cross.parent = Some(p.id.clone());
        let child = ok(&mut s, |tx| create(tx, &Sender::Human, cross));
        assert_eq!(child.parent.as_deref(), Some(p.id.as_str()));
        assert_eq!(child.crew_id, "crew-b");
        s.conn()
            .execute(
                "UPDATE crews SET archived_at='frozen' WHERE id='crew-a'",
                [],
            )
            .unwrap();
        let e = err(&mut s, |tx| update(tx, &Sender::Human, finish(&p)));
        assert_eq!(e.refusal().unwrap().code, Code::CrewRemoved);
        let mut expected = p;
        expected.frozen = true;
        assert_eq!(get(s.conn(), &expected.id).unwrap(), expected);
    }
    #[test]
    fn completion_wakes_blocked_owners_and_queue_failure_is_atomic() {
        use crewd_work::{BlockOn, Continuation};
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let p = ok(&mut s, |tx| create(tx, &by, input()));
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                for _ in 0..2 {
                    let w = b
                        .create_item(
                            &crewd_work::Actor::Human,
                            now,
                            crewd_work::NewItem::titled("dependent").owned_by("w-a2"),
                        )?
                        .value;
                    b.block(
                        &crewd_work::Actor::Human,
                        now,
                        w,
                        vec![BlockOn::Item("W1".parse().unwrap())],
                        Continuation::Nothing,
                    )?;
                }
                Ok(())
            })
        });
        let before = crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap();
        s.conn().execute_batch("CREATE TRIGGER reject_done_wake BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'synthetic done wake fault'); END;").unwrap();
        err(&mut s, |tx| update(tx, &by, finish(&p)));
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap(),
            before
        );
        assert!(crate::wakes::list(s.conn(), None, 10).unwrap().is_empty());
        s.conn()
            .execute_batch("DROP TRIGGER reject_done_wake")
            .unwrap();
        ok(&mut s, |tx| update(tx, &by, finish(&p)));
        let rows = crate::wakes::list(s.conn(), Some("w-a2"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, crate::wakes::WakeKind::System);
        assert_eq!(rows[0].priority, crate::wakes::WakePriority::System);
        let note: serde_json::Value = serde_json::from_str(rows[0].note.as_ref().unwrap()).unwrap();
        assert_eq!(note["unblockedWork"].as_array().unwrap().len(), 2);
        err(&mut s, |tx| update(tx, &by, finish(&p)));
        assert_eq!(
            crate::wakes::list(s.conn(), Some("w-a2"), 10)
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn last_plain_dependency_wakes_ready_owner_without_fabricated_events() {
        let mut s = store();
        let by = Sender::Session("w-a1".into());
        let a = ok(&mut s, |tx| create(tx, &by, input()));
        let b = ok(&mut s, |tx| create(tx, &by, input()));
        let mut child = input();
        child.owner = Some("w-a2".into());
        child.depends_on = vec![a.id.clone(), b.id.clone()];
        let c = ok(&mut s, |tx| create(tx, &Sender::Human, child));
        ok(&mut s, |tx| update(tx, &by, finish(&a)));
        assert!(crate::wakes::list(s.conn(), None, 10).unwrap().is_empty());
        let done = ok(&mut s, |tx| update(tx, &by, finish(&b)));
        let rows = crate::wakes::list(s.conn(), Some("w-a2"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        let note: serde_json::Value = serde_json::from_str(rows[0].note.as_ref().unwrap()).unwrap();
        assert_eq!(note["unblockedWork"], serde_json::json!([c.id]));
        let history = crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap();
        assert!(
            !history
                .iter()
                .any(|e| matches!(e.kind, crewd_work::EventKind::BlockerCleared { .. })),
            "plain readiness is not a made-up blocker release"
        );
        // An already-done predecessor is refused and must not re-notify.
        err(&mut s, |tx| update(tx, &by, finish(&done)));
        assert_eq!(
            crate::wakes::list(s.conn(), Some("w-a2"), 10)
                .unwrap()
                .len(),
            1
        );
    }
    fn pending_followup(
        s: &mut crewd_store::Store,
        n: usize,
    ) -> (Vec<WorkPlan>, Vec<crate::wakes::WakeRow>) {
        let by = Sender::Session("w-a1".into());
        let parent = ok(s, |tx| create(tx, &by, input()));
        let mut children = Vec::new();
        for _ in 0..n {
            let mut child = input();
            child.owner = Some("w-a2".into());
            child.depends_on = vec![parent.id.clone()];
            children.push(ok(s, |tx| create(tx, &Sender::Human, child)));
        }
        let (_, changes) = crate::run(s, NOW, |tx| update(tx, &by, finish(&parent))).unwrap();
        for c in &children {
            assert!(changes.contains(&Change::WorkChanged {
                crew_id: "crew-a".into(),
                id: c.id.clone()
            }));
        }
        let rows = crate::wakes::list(s.conn(), Some("w-a2"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        (children, rows)
    }
    #[test]
    fn stale_followups_are_filtered_and_missing_authority_is_not_empty() {
        for mode in ["done", "transfer", "frozen", "purged", "missing-book"] {
            let mut s = store();
            let (children, rows) = pending_followup(&mut s, 1);
            let before =
                crate::render::build_batch(s.conn(), "w-a2", crate::Locale::En, &rows, NOW)
                    .unwrap();
            assert_eq!(before.items.len(), 1);
            assert!(!before.items[0].note.as_ref().unwrap().contains("targets"));
            match mode {
                "done" => {
                    ok(&mut s, |tx| {
                        update(tx, &Sender::Human, finish(&children[0]))
                    });
                }
                "transfer" => {
                    ok(&mut s, |tx| {
                        asks::with_book(tx, "crew-a", |b, now| {
                            b.assign(
                                &crewd_work::Actor::Human,
                                now,
                                "W2".parse().unwrap(),
                                crewd_work::SessionId::new("w-a1"),
                                "synthetic transfer",
                            )
                        })
                    });
                }
                "frozen" => {
                    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
                    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
                }
                "purged" => {
                    ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
                    ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
                    ok(&mut s, |tx| crate::crews::purge(tx, "crew-a"));
                }
                "missing-book" => {
                    s.conn()
                        .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let after = crate::render::build_batch(s.conn(), "w-a2", crate::Locale::En, &rows, NOW);
            if mode == "missing-book" {
                assert!(after.is_err(), "corruption must retain the wake for retry");
            } else {
                let after = after.unwrap();
                assert!(after.items.is_empty(), "{mode}");
                assert_eq!(after.unusable.len(), 1, "{mode}");
            }
        }
    }
    #[test]
    fn omitted_fifty_first_target_stays_deliverable_without_metadata_echo() {
        let mut s = store();
        let (children, rows) = pending_followup(&mut s, 51);
        let raw: serde_json::Value = serde_json::from_str(rows[0].note.as_ref().unwrap()).unwrap();
        let omitted = raw["targets"][50].as_str().unwrap();
        for c in &children {
            if c.id != omitted {
                ok(&mut s, |tx| update(tx, &Sender::Human, finish(c)));
            }
        }
        let batch =
            crate::render::build_batch(s.conn(), "w-a2", crate::Locale::En, &rows, NOW).unwrap();
        assert_eq!(batch.items.len(), 1);
        assert!(batch.unusable.is_empty());
        let note: serde_json::Value =
            serde_json::from_str(batch.items[0].note.as_ref().unwrap()).unwrap();
        assert_eq!(note["unblockedWork"], serde_json::json!([omitted]));
        assert_eq!(note["total"], 1);
        assert_eq!(note["partial"], false);
        assert!(note.get("targets").is_none());
        let mut tampered = rows.clone();
        let mut bad = raw.clone();
        bad["total"] = serde_json::json!(50);
        tampered[0].note = Some(bad.to_string());
        assert!(
            crate::render::build_batch(s.conn(), "w-a2", crate::Locale::En, &tampered, NOW)
                .is_err()
        );
    }
    #[test]
    fn ordinary_system_and_human_answer_wakes_keep_their_contract() {
        let mut s = store();
        let (_, rows) = pending_followup(&mut s, 1);
        let mut ordinary = rows[0].clone();
        ordinary.source = "synthetic ordinary".into();
        ordinary.note = Some("ordinary note".into());
        assert!(matches!(
            followup_note(s.conn(), &ordinary, NOW).unwrap(),
            FollowupNote::Ordinary
        ));
        ordinary.kind = crate::wakes::WakeKind::AskAnswer;
        ordinary.source = "work-done:synthetic".into();
        assert!(matches!(
            followup_note(s.conn(), &ordinary, NOW).unwrap(),
            FollowupNote::Ordinary
        ));
        let mut legacy = rows[0].clone();
        let mut note: serde_json::Value =
            serde_json::from_str(legacy.note.as_ref().unwrap()).unwrap();
        note.as_object_mut().unwrap().remove("targets");
        legacy.note = Some(note.to_string());
        assert!(matches!(
            followup_note(s.conn(), &legacy, NOW).unwrap(),
            FollowupNote::Current(_)
        ));
        note["partial"] = serde_json::json!(true);
        note["total"] = serde_json::json!(51);
        legacy.note = Some(note.to_string());
        assert!(
            matches!(
                followup_note(s.conn(), &legacy, NOW).unwrap(),
                FollowupNote::Ordinary
            ),
            "legacy omitted targets are not evidence of an empty set"
        );
    }
    #[test]
    fn work_changed_between_render_and_claim_invalidates_exact_body() {
        use crate::cursor_reservations as cr;
        let mut s = store();
        let (children, rows) = pending_followup(&mut s, 2);
        let mut snap = cr::snapshot(s.conn(), "w-a2", &[], NOW).unwrap();
        snap.capture_work_followups(s.conn(), &rows, NOW).unwrap();
        ok(&mut s, |tx| {
            update(tx, &Sender::Human, finish(&children[0]))
        });
        let ids = [rows[0].id];
        let old = ok(&mut s, |tx| {
            cr::claim_reserved(tx, &snap, &[], &ids, &[], cr::Owner::new(1).unwrap())
        });
        assert!(matches!(old, cr::Decision::Stale));
        let pending = crate::wakes::by_ids(s.conn(), &ids).unwrap();
        assert_eq!(pending[0].state, crate::wakes::WakeState::Pending);
        assert_eq!(pending[0].attempts, 0);
        let mut fresh = cr::snapshot(s.conn(), "w-a2", &[], NOW).unwrap();
        fresh
            .capture_work_followups(s.conn(), &pending, NOW)
            .unwrap();
        let claimed = ok(&mut s, |tx| {
            cr::claim_reserved(tx, &fresh, &[], &ids, &[], cr::Owner::new(1).unwrap())
        });
        assert!(matches!(claimed, cr::Decision::Applied(_)));
    }
}

/// Assign a live plan; the pure model also permits workers to claim unowned work.
pub fn assign(
    tx: &mut crate::Tx<'_>,
    by: &crate::Sender,
    p: crewd_proto::WorkAssignParams,
) -> Result<WorkPlan> {
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = actor(tx.conn(), by, &r.crew_id)?;
    let target = crate::sessions::get(tx.conn(), &p.owner)?;
    crews::refuse_archived(tx.conn(), &target.crew_id)?;
    let local = local_plan(tx.conn(), &r.crew_id, &p.id)?;
    let revision = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|n| p.expected_revision == n.to_string())
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    if p.note.trim().is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let book = asks::load(tx.conn(), &r.crew_id)?;
    let item = book
        .item(local)
        .ok_or_else(|| invalid("plan reference has no authoritative item"))?;
    check_revision(item, &p.id, revision)?;
    asks::with_book(tx, &r.crew_id, |b, now| {
        b.assign(
            &actor,
            now,
            local,
            crewd_work::SessionId::new(&p.owner),
            &p.note,
        )
    })?;
    let plan = get(tx.conn(), &p.id)?;
    crate::wakes::enqueue(tx,crate::wakes::WakeRequest {session:p.owner,kind:crate::wakes::WakeKind::System,source:format!("work-assigned:{}:{}",p.id,plan.revision),priority:crate::wakes::WakePriority::System,not_before_ms:None,note:Some(serde_json::json!({"assignedWork":[p.id],"targets":[p.id],"total":1,"partial":false,"assignmentNote":p.note.chars().take(256).collect::<String>(),"noteTruncated":p.note.chars().nth(256).is_some(),"assignedRevision":plan.revision}).to_string()),origin:None})?;
    changed(tx, by, &r.crew_id, &p.id)?;
    Ok(plan)
}

#[cfg(test)]
mod assignment_tests {
    use super::*;
    use crate::{
        Change, Sender,
        testkit::{NOW, err, ok, store},
        wakes,
    };
    use crewd_proto::{WorkAssignParams, WorkCreateParams};
    fn input(p: &WorkPlan, owner: &str) -> WorkAssignParams {
        WorkAssignParams {
            id: p.id.clone(),
            expected_revision: p.revision.clone(),
            owner: owner.into(),
            note: "synthetic assignment reason".into(),
        }
    }
    fn create_plan(s: &mut crewd_store::Store, owner: Option<&str>) -> WorkPlan {
        ok(s, |tx| {
            create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "synthetic assigned work".into(),
                    owner: owner.map(str::to_owned),
                    ..Default::default()
                },
            )
        })
    }
    #[test]
    fn assignment_commits_owner_history_global_notice_and_filters_old_owner() {
        let mut s = store();
        let p = create_plan(&mut s, Some("w-a1"));
        ok(&mut s, |tx| {
            wakes::enqueue(tx,wakes::WakeRequest {session:"w-a1".into(),kind:wakes::WakeKind::System,source:"work-done:synthetic".into(),priority:wakes::WakePriority::System,not_before_ms:None,note:Some(serde_json::json!({"unblockedWork":[p.id],"targets":[p.id],"total":1,"partial":false}).to_string()),origin:None})
        });
        let (assigned, changes) = crate::run(&mut s, NOW, |tx| {
            assign(tx, &Sender::Human, input(&p, "w-b1"))
        })
        .unwrap();
        assert_eq!(assigned.id, p.id);
        assert_eq!(assigned.owner.as_deref(), Some("w-b1"));
        assert_ne!(assigned.revision, p.revision);
        assert!(changes.contains(&Change::WorkChanged {
            crew_id: "crew-a".into(),
            id: p.id.clone()
        }));
        let old = wakes::list(s.conn(), Some("w-a1"), 100).unwrap().remove(0);
        assert!(matches!(
            followup_note(s.conn(), &old, NOW).unwrap(),
            FollowupNote::Stale
        ));
        let rows = wakes::list(s.conn(), Some("w-b1"), 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(changes.contains(&Change::WakeEnqueued {
            wake_id: rows[0].id,
            session_id: "w-b1".into()
        }));
        let FollowupNote::Current(body) = followup_note(s.conn(), &rows[0], NOW).unwrap() else {
            panic!("new owner must be notified")
        };
        assert!(body.contains("assignedWork"));
        assert!(body.contains("synthetic assignment reason"));
        assert!(!body.contains("assignedRevision"));
        assert!(!body.contains("unblockedWork"));
        assert!(!body.contains("targets"));
        assert!(body.contains(&p.id));
        let book = asks::load(s.conn(), "crew-a").unwrap();
        let item = book.item("W1".parse().unwrap()).unwrap();
        assert_eq!(item.owner_history.len(), 2);
        assert_eq!(
            item.owner_history
                .last()
                .unwrap()
                .from
                .as_ref()
                .unwrap()
                .as_str(),
            "w-a1"
        );
    }
    #[test]
    fn worker_claim_is_not_steal_or_foreign_captain_privilege_and_stale_cas_is_read_only() {
        let mut s = store();
        let p = create_plan(&mut s, None);
        let claimed = ok(&mut s, |tx| {
            assign(tx, &Sender::Session("w-a1".into()), input(&p, "w-a1"))
        });
        for (actor, owner) in [("w-a2", "w-a2"), ("cap-b", "w-b1")] {
            assert!(matches!(
                err(&mut s, |tx| assign(
                    tx,
                    &Sender::Session(actor.into()),
                    input(&claimed, owner)
                )),
                CollabError::Work(crewd_work::Rejected::Forbidden(_))
            ));
        }
        let moved = ok(&mut s, |tx| {
            assign(
                tx,
                &Sender::Session("cap-a".into()),
                input(&claimed, "w-a2"),
            )
        });
        let error = err(&mut s, |tx| {
            assign(tx, &Sender::Human, input(&claimed, "w-b1"))
        });
        let CollabError::Conflict(r) = error else {
            panic!("strict CAS required")
        };
        assert_eq!(r.current_work.unwrap().revision, moved.revision);
        assert_eq!(get(s.conn(), &p.id).unwrap(), moved);
        assert_eq!(wakes::list(s.conn(), Some("w-a2"), 100).unwrap().len(), 1);
        assert!(wakes::list(s.conn(), Some("w-b1"), 100).unwrap().is_empty());
        let mut blank = input(&moved, "w-b1");
        blank.note = " ".into();
        err(&mut s, |tx| assign(tx, &Sender::Human, blank));
        assert_eq!(get(s.conn(), &p.id).unwrap(), moved);
    }
    #[test]
    fn queue_failure_rolls_back_assignment_history_and_target_frozen_refuses() {
        let mut s = store();
        let p = create_plan(&mut s, Some("w-a1"));
        let before = crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap();
        s.conn().execute_batch("CREATE TRIGGER deny_assign BEFORE INSERT ON wakes BEGIN SELECT RAISE(ABORT,'synthetic assignment queue failure'); END;").unwrap();
        err(&mut s, |tx| assign(tx, &Sender::Human, input(&p, "w-b1")));
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap(),
            before
        );
        assert!(wakes::list(s.conn(), Some("w-b1"), 100).unwrap().is_empty());
        s.conn().execute_batch("DROP TRIGGER deny_assign;").unwrap();
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
        assert!(matches!(
            err(&mut s, |tx| assign(tx, &Sender::Human, input(&p, "w-b1"))),
            CollabError::Conflict(r) if r.code==Code::CrewRemoved
        ));
        assert_eq!(get(s.conn(), &p.id).unwrap(), p);
    }
    #[test]
    fn assignment_reason_is_bounded_and_new_owner_report_invalidates_old_notice() {
        let mut s = store();
        let p = create_plan(&mut s, Some("w-a1"));
        let mut params = input(&p, "w-b1");
        params.note = "交接🙂".repeat(400);
        let assigned = ok(&mut s, |tx| assign(tx, &Sender::Human, params));
        let row = wakes::list(s.conn(), Some("w-b1"), 100).unwrap().remove(0);
        let FollowupNote::Current(body) = followup_note(s.conn(), &row, NOW).unwrap() else {
            panic!("notice required")
        };
        let n: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(n["assignmentNote"].as_str().unwrap().chars().count(), 256);
        assert_eq!(n["noteTruncated"], true);
        assert!(!body.contains("assignedRevision"));
        ok(&mut s, |tx| {
            update(
                tx,
                &Sender::Session("w-b1".into()),
                crewd_proto::WorkUpdateParams {
                    id: assigned.id,
                    expected_revision: assigned.revision,
                    status: Some(crewd_proto::WorkUpdateStatus::Open),
                    note: Some("new owner already reported".into()),
                    ..Default::default()
                },
            )
        });
        assert!(matches!(
            followup_note(s.conn(), &row, NOW).unwrap(),
            FollowupNote::Stale
        ));
    }
}

/// Latest handoffs first; older pages never repeat an appended newer record.
pub fn history(
    conn: &Connection,
    p: crewd_proto::WorkHistoryParams,
) -> Result<crewd_proto::WorkHistoryPage> {
    use crewd_proto::{WorkHistoryActor as A, WorkOwnerChange};
    let limit = p.limit.unwrap_or(20);
    if limit == 0 || limit > 200 {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadLimit)));
    }
    let before = match p.before {
        Some(s) => Some(
            s.parse::<usize>()
                .ok()
                .filter(|n| *n > 0 && n.to_string() == s)
                .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkHistoryCursor)))?,
        ),
        None => None,
    };
    let r = object_refs::get(conn, ObjectKind::Plan, number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    let book = asks::load(conn, &r.crew_id)?;
    let local = core_plan(conn, &r)?;
    let item = book
        .item(local)
        .ok_or_else(|| invalid("history plan has no authoritative item"))?;
    let total = item.owner_history.len();
    let mut rows = item
        .owner_history
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, _)| before.is_none_or(|b| i + 1 < b));
    let mut out = Vec::new();
    for (i, h) in rows.by_ref().take(usize::from(limit)) {
        let by = match &h.by {
            crewd_work::Actor::Human => A::Human,
            crewd_work::Actor::System => A::System,
            actor => A::Session {
                id: actor.session().expect("session actor").to_string(),
                role: match actor
                    .role_in(&crewd_work::CrewId::new(&r.crew_id), false)
                    .expect("session role")
                {
                    crewd_work::Role::Worker => crewd_proto::Role::Worker,
                    crewd_work::Role::Captain => crewd_proto::Role::Captain,
                    crewd_work::Role::Admin => crewd_proto::Role::Admin,
                },
            },
        };
        out.push(WorkOwnerChange {
            seq: (i + 1).to_string(),
            from: h.from.as_ref().map(ToString::to_string),
            to: h.to.to_string(),
            by,
            how: match h.how {
                crewd_work::HowChanged::Claim => "claim",
                crewd_work::HowChanged::Transfer => "transfer",
                crewd_work::HowChanged::Assign => "assign",
                crewd_work::HowChanged::Lost => "lost",
            }
            .into(),
            note: h.note.clone(),
            at: crewd_store::ts_from_ms(h.at.0),
        });
    }
    let next_before = rows.next().and_then(|_| out.last().map(|h| h.seq.clone()));
    Ok(crewd_proto::WorkHistoryPage {
        id: p.id,
        revision: item.revision.to_string(),
        total,
        items: out,
        next_before,
    })
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{NOW, ok, store},
    };
    use crewd_proto::{WorkAssignParams, WorkCreateParams, WorkHistoryActor, WorkHistoryParams};
    fn query(id: &str, before: Option<String>, limit: u16) -> WorkHistoryParams {
        WorkHistoryParams {
            id: id.into(),
            before,
            limit: Some(limit),
        }
    }
    #[test]
    fn latest_full_handoff_and_stable_older_pages_are_read_only() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "history fixture".into(),
                    goal: "LONG_GOAL_NOT_IN_HISTORY".repeat(100),
                    owner: Some("w-a1".into()),
                    ..Default::default()
                },
            )
        });
        let reason = "完整交接说明🙂".repeat(400);
        let moved = ok(&mut s, |tx| {
            assign(
                tx,
                &Sender::Session("cap-a".into()),
                WorkAssignParams {
                    id: p.id.clone(),
                    expected_revision: p.revision.clone(),
                    owner: "w-a2".into(),
                    note: reason.clone(),
                },
            )
        });
        let latest = history(s.conn(), query(&p.id, None, 1)).unwrap();
        assert_eq!(latest.id, p.id);
        assert_eq!(latest.revision, moved.revision);
        assert_eq!(latest.total, 2);
        assert_eq!(latest.items[0].seq, "2");
        assert_eq!(latest.items[0].note, reason);
        assert_eq!(latest.items[0].from.as_deref(), Some("w-a1"));
        assert_eq!(latest.items[0].to, "w-a2");
        assert_eq!(latest.items[0].how, "assign");
        assert!(
            matches!(&latest.items[0].by,WorkHistoryActor::Session{id,role} if id=="cap-a" && *role==crewd_proto::Role::Captain)
        );
        assert_eq!(latest.items[0].at, crewd_store::ts_from_ms(NOW));
        assert!(
            !serde_json::to_string(&latest)
                .unwrap()
                .contains("LONG_GOAL")
        );
        let moved = ok(&mut s, |tx| {
            assign(
                tx,
                &Sender::Human,
                WorkAssignParams {
                    id: p.id.clone(),
                    expected_revision: moved.revision,
                    owner: "w-a1".into(),
                    note: "new append after first page".into(),
                },
            )
        });
        let events = crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap();
        let wakes = crate::testkit::count(&s, "SELECT COUNT(*) FROM wakes");
        let older = history(s.conn(), query(&p.id, latest.next_before, 1)).unwrap();
        assert_eq!(older.items.len(), 1);
        assert_eq!(older.items[0].seq, "1");
        assert!(older.next_before.is_none());
        assert!(matches!(older.items[0].by, WorkHistoryActor::Human));
        assert_eq!(get(s.conn(), &p.id).unwrap(), moved);
        assert_eq!(
            crate::testkit::organization_events(s.conn(), "crew-a", 0, 200).unwrap(),
            events
        );
        assert_eq!(
            crate::testkit::count(&s, "SELECT COUNT(*) FROM wakes"),
            wakes
        );
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
        assert_eq!(history(s.conn(), query(&p.id, None, 20)).unwrap().total, 3);
    }
    #[test]
    fn empty_history_is_not_invented_and_bad_bounds_or_missing_authority_refuse() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            create(
                tx,
                &Sender::Human,
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "unowned history fixture".into(),
                    ..Default::default()
                },
            )
        });
        let empty = history(s.conn(), query(&p.id, None, 20)).unwrap();
        assert_eq!(empty.total, 0);
        assert!(empty.items.is_empty());
        assert!(empty.next_before.is_none());
        for limit in [0, 201] {
            assert!(
                matches!(history(s.conn(),query(&p.id,None,limit)),Err(CollabError::Invalid(r)) if r.code==Code::WorkBadLimit)
            );
        }
        for before in [
            "0",
            "01",
            "-1",
            "1.0",
            "9999999999999999999999999999999999999",
        ] {
            assert!(
                matches!(history(s.conn(),query(&p.id,Some(before.into()),1)),Err(CollabError::Invalid(r)) if r.code==Code::WorkHistoryCursor)
            );
        }
        s.conn()
            .execute("DELETE FROM organization_work_book WHERE singleton=1", [])
            .unwrap();
        assert!(history(s.conn(), query(&p.id, None, 20)).is_err());
    }
}

#[cfg(test)]
mod pending_transfer_view_tests {
    use super::*;
    use crate::{
        Sender,
        testkit::{NOW, err, ok, store},
    };
    use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
    #[test]
    fn detail_and_conflict_show_actual_offer_without_implying_acceptance() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "synthetic pending transfer".into(),
                    ..Default::default()
                },
            )
        });
        let before = serde_json::to_value(&p).unwrap();
        assert!(before.get("pendingTransfer").is_none());
        assert!(
            serde_json::from_value::<WorkPlan>(before)
                .unwrap()
                .pending_transfer
                .is_none()
        );
        let note = "转交说明🙂".repeat(400);
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.request_transfer(
                    &crewd_work::Actor::worker("w-a1"),
                    now,
                    "W1".parse().unwrap(),
                    crewd_work::SessionId::new("w-a2"),
                    &note,
                )
            })
        });
        let offered = get(s.conn(), &p.id).unwrap();
        assert_eq!(offered.owner.as_deref(), Some("w-a1"));
        let t = offered.pending_transfer.as_ref().unwrap();
        assert_eq!(t.to, "w-a2");
        assert_eq!(t.requested_by, "w-a1");
        assert_eq!(t.note, note);
        assert!(!t.note_truncated);
        assert_eq!(t.at, crewd_store::ts_from_ms(NOW));
        let summary =
            serde_json::to_value(crewd_proto::WorkSummary::from(offered.clone())).unwrap();
        assert!(summary.get("pendingTransfer").is_none());
        assert_eq!(
            history(
                s.conn(),
                crewd_proto::WorkHistoryParams {
                    id: p.id.clone(),
                    ..Default::default()
                }
            )
            .unwrap()
            .total,
            1,
            "an offer is not a completed handoff"
        );
        let error = err(&mut s, |tx| {
            update(
                tx,
                &Sender::Session("w-a1".into()),
                WorkUpdateParams {
                    id: p.id.clone(),
                    expected_revision: p.revision,
                    status: Some(WorkUpdateStatus::Open),
                    note: Some("stale report".into()),
                    ..Default::default()
                },
            )
        });
        let CollabError::Conflict(r) = error else {
            panic!("strict CAS required")
        };
        let t = r.current_work.unwrap().pending_transfer.unwrap();
        assert_eq!(t.to, "w-a2");
        assert_eq!(t.requested_by, "w-a1");
        assert_eq!(t.note.chars().count(), 256);
        assert!(t.note_truncated);
        assert_eq!(get(s.conn(), &p.id).unwrap(), offered);
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.accept_transfer(
                    &crewd_work::Actor::worker("w-a2"),
                    now,
                    "W1".parse().unwrap(),
                )
            })
        });
        let accepted = get(s.conn(), &p.id).unwrap();
        assert_eq!(accepted.owner.as_deref(), Some("w-a2"));
        assert!(accepted.pending_transfer.is_none());
        assert!(
            serde_json::to_value(accepted)
                .unwrap()
                .get("pendingTransfer")
                .is_none()
        );
    }
    #[test]
    fn withdrawal_removes_offer_and_frozen_reads_remain_honest() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            create(
                tx,
                &Sender::Session("w-a1".into()),
                WorkCreateParams {
                    crew_id: "crew-a".into(),
                    title: "withdrawal view fixture".into(),
                    ..Default::default()
                },
            )
        });
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.request_transfer(
                    &crewd_work::Actor::worker("w-a1"),
                    now,
                    "W1".parse().unwrap(),
                    crewd_work::SessionId::new("w-a2"),
                    "original offer",
                )
            })
        });
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-b"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-a"));
        let frozen = get(s.conn(), &p.id).unwrap();
        assert!(frozen.frozen);
        assert_eq!(frozen.pending_transfer.unwrap().note, "original offer");
        ok(&mut s, |tx| crate::crews::unarchive(tx, "crew-a"));
        ok(&mut s, |tx| {
            asks::with_book(tx, "crew-a", |b, now| {
                b.withdraw_transfer(
                    &crewd_work::Actor::worker("w-a1"),
                    now,
                    "W1".parse().unwrap(),
                )
            })
        });
        let current = get(s.conn(), &p.id).unwrap();
        assert!(current.pending_transfer.is_none());
        assert_eq!(current.owner.as_deref(), Some("w-a1"));
    }
}
