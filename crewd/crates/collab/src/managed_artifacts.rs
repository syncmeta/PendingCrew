//! Explicit unverified pointers. Permission, receipt and mutation share one Core transaction.
use crate::{
    CollabError, Result, Tx,
    managed_objects::{self as objects, Actor},
};
use crewd_proto::{ResourceActor, managed_artifacts::*, managed_objects::Scope};
use crewd_store::managed_artifacts as db;
use rusqlite::Connection;

#[cfg(test)]
#[path = "managed_artifacts_tests.rs"]
mod tests;

fn invalid(field: &str) -> CollabError {
    crate::resource_acl::invalid(field)
}
fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(
        crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
            .arg("request", request),
    )
}
fn edge_changed() -> CollabError {
    CollabError::Conflict(crate::refusal::Refusal::new(
        crate::refusal::Code::ArtifactEdgeChanged,
    ))
}
fn forbidden() -> CollabError {
    CollabError::Forbidden(crate::refusal::Refusal::new(
        crate::refusal::Code::ResourceForbidden,
    ))
}
fn missing() -> CollabError {
    CollabError::NotFound(
        crate::refusal::Refusal::new(crate::refusal::Code::ResourceNotFound).arg("id", "artifact"),
    )
}

pub fn preview(c: &Connection, actor: Actor<'_>, crew: &str, input: &Input) -> Result<Reference> {
    actor.owner(c, crew, true)?;
    db::canonicalize(crew, input).map_err(|_| invalid("input"))
}
pub fn list(c: &Connection, actor: Actor<'_>, after: Option<&Reference>) -> Result<Page> {
    actor.principal(c)?;
    Ok(db::list(c, after)?)
}
pub fn get(c: &Connection, actor: Actor<'_>, reference: &Reference) -> Result<Artifact> {
    actor.principal(c)?;
    db::get(c, reference)?.ok_or_else(missing)
}
fn readable(c: &Connection, actor: Actor<'_>, target: &Subject) -> Result<i64> {
    actor.principal(c)?;
    match target {
        Subject::Work { id } => {
            crate::work::get(c, id)?;
        }
        Subject::Requirement { id } => {
            crate::requirements::get(c, id)?;
        }
        Subject::Decision { id }
        | Subject::Risk { id }
        | Subject::Lesson { id }
        | Subject::Row { id }
        | Subject::Table { id } => {
            objects::get_with_defaults(c, actor, id, true)?;
        }
    }
    Ok(db::subject_number(c, target)?)
}
pub fn backlinks(
    c: &Connection,
    actor: Actor<'_>,
    reference: &Reference,
    after: Option<&str>,
) -> Result<EdgePage> {
    backlinks_with_legacy(c, actor, reference, after, false)
}
pub fn backlinks_with_legacy(
    c: &Connection,
    actor: Actor<'_>,
    reference: &Reference,
    after: Option<&str>,
    include_legacy: bool,
) -> Result<EdgePage> {
    get(c, actor, reference)?;
    let page = db::edges_with_legacy(c, Some(reference), None, after, include_legacy)?;
    for edge in &page.items {
        readable(c, actor, &edge.subject)?;
    }
    Ok(page)
}
pub fn subject(
    c: &Connection,
    actor: Actor<'_>,
    target: &Subject,
    after: Option<&str>,
) -> Result<EdgePage> {
    subject_with_legacy(c, actor, target, after, false)
}
pub fn subject_with_legacy(
    c: &Connection,
    actor: Actor<'_>,
    target: &Subject,
    after: Option<&str>,
    include_legacy: bool,
) -> Result<EdgePage> {
    readable(c, actor, target)?;
    Ok(db::edges_with_legacy(
        c,
        None,
        Some(target),
        after,
        include_legacy,
    )?)
}
fn author(actor: Actor<'_>) -> ResourceActor {
    match actor {
        Actor::Human(_) => ResourceActor {
            kind: "human".into(),
            session_id: None,
        },
        Actor::Session(id) => ResourceActor {
            kind: "session".into(),
            session_id: Some(id.into()),
        },
    }
}

pub fn submit(tx: &mut Tx<'_>, actor: Actor<'_>, request: Submit) -> Result<Receipt> {
    if !crewd_proto::valid_remote_uuid(&request.request_id) {
        return Err(invalid("requestId"));
    }
    let c = tx.conn();
    let principal = actor.owner(c, &request.crew_id, false)?;
    if objects::scope(c)? != request.scope {
        return Err(conflict(&request.request_id));
    }
    let digest: [u8; 32] = crate::resource_acl::digest(&request)?
        .try_into()
        .map_err(|_| invalid("request"))?;
    match db::receipt(c, &request.crew_id, &principal, &request.request_id)? {
        Some(db::Saved::Active {
            digest: saved,
            receipt,
        }) if saved == digest && receipt.scope == request.scope => return Ok(receipt),
        Some(_) => return Err(conflict(&request.request_id)),
        None => {}
    }
    actor.owner(c, &request.crew_id, true)?;
    let now = tx.now_ts();
    let (artifact, effect, target, changed) = match &request.change {
        Change::Register { input } => {
            let reference =
                db::canonicalize(&request.crew_id, input).map_err(|_| invalid("input"))?;
            let (artifact, created) = db::ensure(c, &reference, &now)?;
            let claim_added = db::claim(c, &request.crew_id, &reference)?;
            (
                artifact,
                Effect::Registered {
                    created,
                    claim_added,
                },
                None,
                created || claim_added,
            )
        }
        Change::Link {
            input,
            subject,
            relation,
        } => {
            let target = readable(c, actor, subject)?;
            objects::artifact_link_authority(c, actor, subject)?;
            let reference =
                db::canonicalize(&request.crew_id, input).map_err(|_| invalid("input"))?;
            if !db::relation_valid(subject, *relation, &reference) {
                return Err(invalid("relation"));
            }
            let (artifact, created) = db::ensure(c, &reference, &now)?;
            let (edge, added) = db::link(
                c,
                &request.crew_id,
                &reference,
                subject,
                *relation,
                &Origin::Manual {
                    actor: author(actor),
                },
                &now,
            )?;
            (
                artifact,
                Effect::Linked {
                    created,
                    added,
                    edge,
                },
                Some(target),
                created || added,
            )
        }
        Change::LinkExisting {
            reference,
            subject,
            relation,
        } => {
            let target = readable(c, actor, subject)?;
            objects::artifact_link_authority(c, actor, subject)?;
            if !db::relation_valid(subject, *relation, reference) {
                return Err(invalid("relation"));
            }
            // This route can preserve another crew's historical unassigned identity,
            // but can never mint an artifact in that foreign namespace.
            let artifact = db::get(c, reference)?.ok_or_else(missing)?;
            let (edge, added) = db::link(
                c,
                &request.crew_id,
                reference,
                subject,
                *relation,
                &Origin::Manual {
                    actor: author(actor),
                },
                &now,
            )?;
            (
                artifact,
                Effect::Linked {
                    created: false,
                    added,
                    edge,
                },
                Some(target),
                added,
            )
        }
        Change::Unlink {
            reference,
            subject,
            relation,
            expected_edge_id,
        } => {
            let target = readable(c, actor, subject)?;
            objects::artifact_link_authority(c, actor, subject)?;
            if !db::relation_valid(subject, *relation, reference) {
                return Err(invalid("relation"));
            }
            let artifact = db::get(c, reference)?.ok_or_else(edge_changed)?;
            db::unlink(
                c,
                &request.crew_id,
                reference,
                subject,
                *relation,
                expected_edge_id,
            )
            .map_err(|error| match error {
                crewd_store::StoreError::Invalid(_) => edge_changed(),
                other => other.into(),
            })?;
            (
                artifact,
                Effect::Unlinked {
                    edge_id: expected_edge_id.clone(),
                },
                Some(target),
                true,
            )
        }
    };
    let receipt = Receipt {
        scope: request.scope,
        crew_id: request.crew_id,
        request_id: request.request_id,
        recorded_at: now,
        change: request.change,
        artifact,
        effect,
    };
    db::record_receipt(tx.conn(), &principal, &digest, &receipt, target)?;
    if changed {
        if let Actor::Session(session) = actor {
            crate::sessions::note_progress(tx, session, crate::ProgressKind::Ontology)?;
        }
        tx.note(crate::Change::ArtifactChanged {
            crew_id: receipt.crew_id.clone(),
        });
    }
    Ok(receipt)
}
pub fn lookup(
    c: &Connection,
    actor: Actor<'_>,
    origin: &Scope,
    crew: &str,
    request: &str,
) -> Result<Lookup> {
    if !crewd_proto::valid_remote_uuid(request)
        || !crewd_proto::valid_remote_uuid(&origin.restore_epoch)
    {
        return Err(invalid("requestId/restoreEpoch"));
    }
    let principal = actor.owner(c, crew, false)?;
    let current = objects::scope(c)?;
    if current.db_id != origin.db_id {
        return Err(forbidden());
    }
    let saved = match db::receipt(c, crew, &principal, request)? {
        Some(db::Saved::Active { receipt, .. }) => Some(receipt),
        Some(db::Saved::Retired) | None => None,
    };
    if current.restore_epoch != origin.restore_epoch {
        return Ok(Lookup::Historical {
            receipt: saved.filter(|r| r.scope == *origin),
        });
    }
    Ok(match saved {
        Some(receipt) if receipt.scope == *origin => Lookup::Known { receipt },
        Some(_) => Lookup::Historical { receipt: None },
        None => Lookup::NotFound,
    })
}
