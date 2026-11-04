//! Evidence-backed closure uses the existing work authority and its genuine events.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Sender, Tx, asks, crews, work};
use crewd_proto::*;
use crewd_store::{
    StoreError,
    object_refs::{self, ObjectKind},
};
use crewd_work::{Evidence, Status};
use rusqlite::{Connection, params};
fn invalid() -> CollabError {
    StoreError::Invalid("invalid factual closure authority".into()).into()
}
fn evidence(e: &Evidence) -> WorkEvidence {
    let (kind, value) = match e {
        Evidence::Commit(v) => ("commit", v),
        Evidence::Url(v) => ("url", v),
        Evidence::Text(v) => ("text", v),
    };
    WorkEvidence {
        kind: kind.into(),
        value: value.clone(),
    }
}
pub(crate) fn view(item: &crewd_work::WorkItem) -> Option<WorkClosure> {
    match item.status {
        Status::Done => Some(WorkClosure {
            kind: WorkClosureKind::Done,
            note: item.completion_note.clone(),
            evidence: item.evidence.as_ref().map(evidence),
        }),
        Status::Dropped => item.drop_reason.as_ref().and_then(drop_view),
        _ => None,
    }
}
fn drop_view(r: &crewd_work::DropReason) -> Option<WorkClosure> {
    let kind = match r.kind {
        crewd_work::DropKind::Duplicate => WorkClosureKind::Duplicate,
        crewd_work::DropKind::Superseded => WorkClosureKind::Superseded,
        crewd_work::DropKind::NotNeeded => return None,
    };
    Some(WorkClosure {
        kind,
        note: Some(r.note.clone()),
        evidence: r.evidence.as_ref().map(evidence),
    })
}
pub fn close(tx: &mut Tx<'_>, by: &Sender, p: WorkCloseParams) -> Result<WorkPlan> {
    let r = object_refs::get(tx.conn(), ObjectKind::Plan, work::number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &r.crew_id)?;
    let actor = work::actor(tx.conn(), by, &r.crew_id)?;
    let local = work::core_plan(tx.conn(), &r)?;
    let revision = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|v| v.to_string() == p.expected_revision)
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    if p.note.trim().is_empty() || p.evidence.value.trim().is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let proof = match p.evidence.kind {
        WorkEvidenceKind::Commit => Evidence::Commit(p.evidence.value),
        WorkEvidenceKind::Url => Evidence::Url(p.evidence.value),
        WorkEvidenceKind::Text => Evidence::Text(p.evidence.value),
    };
    let current = asks::load_org(tx.conn())?;
    work::check_revision(current.item(local).ok_or_else(invalid)?, &p.id, revision)?;
    let (events, ready) = asks::with_book(tx, &r.crew_id, |b, now| {
        if p.kind != WorkClosureKind::Done {
            let ap = b.drop_item(
                &actor,
                now,
                local,
                crewd_work::DropReason {
                    kind: match p.kind {
                        WorkClosureKind::Duplicate => crewd_work::DropKind::Duplicate,
                        WorkClosureKind::Superseded => crewd_work::DropKind::Superseded,
                        _ => unreachable!(),
                    },
                    note: p.note,
                    evidence: Some(proof),
                },
            )?;
            return Ok((ap.events, Vec::new()));
        }
        let before: Vec<_> = b
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
        let ap = b.mark_done_noted(&actor, now, local, proof, p.note)?;
        let ready = before
            .into_iter()
            .filter_map(|id| {
                let i = &b.state().items[&id];
                (i.blocked_on.is_empty()
                    && i.depends_on
                        .iter()
                        .all(|d| b.state().items[d].status == Status::Done))
                .then(|| i.owner.clone().map(|o| (id, o)))
                .flatten()
            })
            .collect();
        Ok((ap.events, ready))
    })
    .map_err(work::command_refusal)?;
    work::wake_cleared(
        tx,
        &r.crew_id,
        &events,
        &ready,
        crate::wakes::WakeKind::System,
        crate::wakes::WakePriority::System,
        &format!("work-close:{}:{}", p.id, p.expected_revision),
    )?;
    work::changed(tx, by, &r.crew_id, &p.id)?;
    work::get(tx.conn(), &p.id)
}
pub fn history(c: &Connection, p: WorkClosureHistoryParams) -> Result<WorkClosureHistoryPage> {
    history_scoped(c, p, None)
}

pub(crate) fn history_scoped(
    c: &Connection,
    p: WorkClosureHistoryParams,
    review_scope: Option<&str>,
) -> Result<WorkClosureHistoryPage> {
    let limit = p.limit.unwrap_or(20);
    if limit == 0 || limit > 200 {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadLimit)));
    }
    let before = p
        .before_seq
        .map(|s| {
            s.parse::<i64>()
                .ok()
                .filter(|n| *n > 0 && n.to_string() == s)
                .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkHistoryCursor)))
        })
        .transpose()?;
    let r = object_refs::get(c, ObjectKind::Plan, work::number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    if review_scope.is_some_and(|crew| crew != r.crew_id) {
        return Err(crate::resource_acl::invalid("crewId"));
    }
    let core = work::core_plan(c, &r)?;
    let _ = work::get(c, &p.id)?;
    let mut q=c.prepare("SELECT seq,body FROM organization_work_events WHERE crew_id=?1 AND json_extract(body,'$.kind.id')=?2 AND (json_extract(body,'$.kind.type')='done' OR (json_extract(body,'$.kind.type')='dropped' AND json_extract(body,'$.kind.reason.kind') IN ('duplicate','superseded'))) AND (?3 IS NULL OR seq<?3) ORDER BY seq DESC LIMIT ?4")?;
    let mut rows = q
        .query_map(
            params![r.crew_id, core.to_string(), before, i64::from(limit) + 1],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let more = rows.len() > usize::from(limit);
    rows.truncate(usize::from(limit));
    let mut items = Vec::new();
    for (seq, raw) in rows {
        let e: crewd_work::Event = serde_json::from_str(&raw).map_err(StoreError::from)?;
        if seq <= 0 || e.seq != seq as u64 {
            return Err(invalid());
        }
        let duplicate_review = if review_scope.is_some() {
            crate::work_duplicate_review::history(c, &r.crew_id, &p.id, core, &e)?
        } else {
            None
        };
        let closure = match e.kind {
            crewd_work::EventKind::Done {
                id,
                evidence: proof,
                note,
            } if id == core => WorkClosure {
                kind: WorkClosureKind::Done,
                note,
                evidence: proof.as_ref().map(evidence),
            },
            crewd_work::EventKind::Dropped { id, reason } if id == core => {
                drop_view(&reason).ok_or_else(invalid)?
            }
            _ => return Err(invalid()),
        };
        let actor = match e.by {
            crewd_work::Actor::Human => WorkHistoryActor::Human,
            crewd_work::Actor::System => WorkHistoryActor::System,
            ref a => WorkHistoryActor::Session {
                id: a.session().ok_or_else(invalid)?.to_string(),
                role: match a
                    .role_in(&crewd_work::CrewId::new(&r.crew_id), false)
                    .ok_or_else(invalid)?
                {
                    crewd_work::Role::Admin => Role::Admin,
                    crewd_work::Role::Captain => Role::Captain,
                    crewd_work::Role::Worker => Role::Worker,
                },
            },
        };
        items.push(WorkClosureEvent {
            seq: seq.to_string(),
            at: crewd_store::ts_from_ms(e.at.0),
            actor,
            closure,
            duplicate_review,
        });
    }
    let next_before_seq = more.then(|| items.last().map(|i| i.seq.clone())).flatten();
    Ok(WorkClosureHistoryPage {
        items,
        next_before_seq,
    })
}
