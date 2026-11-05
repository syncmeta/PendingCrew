//! Historical maintenance is explicit and bounded, never a replay of work or model progress.
use crate::{
    CollabError, Result, Tx,
    managed_objects::{self as objects, Actor},
    refusal::{Code, Refusal},
};
use crewd_proto::{artifact_history::*, managed_objects::Scope};
use crewd_store::artifact_history as db;
use rusqlite::Connection;

#[cfg(test)]
#[path = "artifact_history_tests.rs"]
mod tests;
fn invalid(field: &str) -> CollabError {
    crate::resource_acl::invalid(field)
}
fn changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ArtifactHistoryChanged))
}
fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ResourceRequestConflict).arg("request", request))
}
fn forbidden() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}
fn authorize(c: &Connection, actor: Actor<'_>, crew: &str) -> Result<String> {
    // Human indexing maintains retained facts in archived crews. Agent authority stays live and own-crew.
    match actor {
        Actor::Human(_) => actor.owner(c, crew, false),
        Actor::Session(_) => actor.owner(c, crew, true),
    }
}
pub fn status(c: &Connection, actor: Actor<'_>, params: StatusParams) -> Result<Status> {
    authorize(c, actor, &params.crew_id)?;
    let scope = objects::scope(c)?;
    let job = db::load(c, &params.crew_id, &scope, params.job_id.as_deref())?;
    if job
        .as_ref()
        .is_some_and(|job| job.scope.db_id != scope.db_id)
    {
        return Err(forbidden());
    }
    let can_continue = job
        .as_ref()
        .is_some_and(|job| job.scope == scope && job.cursor.phase != Phase::Complete);
    Ok(Status {
        scope,
        crew_id: params.crew_id,
        job,
        can_continue,
    })
}
fn check_request(
    c: &Connection,
    actor: Actor<'_>,
    scope: &Scope,
    crew: &str,
    request: &str,
) -> Result<String> {
    if !crewd_proto::valid_remote_uuid(request) {
        return Err(invalid("requestId"));
    }
    let principal = authorize(c, actor, crew)?;
    if objects::scope(c)? != *scope {
        return Err(changed());
    }
    Ok(principal)
}
fn replay(
    c: &Connection,
    crew: &str,
    principal: &str,
    request: &str,
    digest: &[u8; 32],
) -> Result<Option<Receipt>> {
    match db::receipt(c, crew, principal, request)? {
        Some(db::Saved::Active {
            digest: saved,
            receipt,
        }) if &saved == digest => Ok(Some(receipt)),
        Some(_) => Err(conflict(request)),
        None => Ok(None),
    }
}
pub fn begin(tx: &mut Tx<'_>, actor: Actor<'_>, params: BeginParams) -> Result<Receipt> {
    let c = tx.conn();
    let principal = check_request(c, actor, &params.scope, &params.crew_id, &params.request_id)?;
    let operation = Operation::Begin;
    let digest = db::request_digest(
        &params.scope,
        &params.crew_id,
        &params.request_id,
        &operation,
    )?;
    if let Some(receipt) = replay(c, &params.crew_id, &principal, &params.request_id, &digest)? {
        return Ok(receipt);
    }
    let now = tx.now_ts();
    let (job, created) = db::begin(c, &params.scope, &params.crew_id, &params.request_id, &now)?;
    let receipt = Receipt {
        scope: params.scope,
        crew_id: params.crew_id,
        request_id: params.request_id,
        recorded_at: now,
        operation,
        job,
        effect: Effect::Begun { created },
    };
    db::record(tx.conn(), &principal, &digest, &receipt)?;
    if created {
        tx.note(crate::Change::ArtifactHistoryChanged {
            crew_id: receipt.crew_id.clone(),
            job_id: receipt.job.id.clone(),
        });
    }
    Ok(receipt)
}
pub fn step(tx: &mut Tx<'_>, actor: Actor<'_>, params: StepParams) -> Result<Receipt> {
    if !(1..=64).contains(&params.limit) || !crewd_proto::valid_remote_uuid(&params.job_id) {
        return Err(invalid("limit/jobId"));
    }
    let c = tx.conn();
    let principal = check_request(c, actor, &params.scope, &params.crew_id, &params.request_id)?;
    let operation = Operation::Step {
        job_id: params.job_id.clone(),
        expected_revision: params.expected_revision.clone(),
        expected_cursor: params.expected_cursor.clone(),
        limit: params.limit,
    };
    let digest = db::request_digest(
        &params.scope,
        &params.crew_id,
        &params.request_id,
        &operation,
    )?;
    if let Some(receipt) = replay(c, &params.crew_id, &principal, &params.request_id, &digest)? {
        return Ok(receipt);
    }
    let mut job =
        db::load(c, &params.crew_id, &params.scope, Some(&params.job_id))?.ok_or_else(changed)?;
    if job.scope != params.scope
        || job.cursor.phase == Phase::Complete
        || job.revision != params.expected_revision
        || job.cursor != params.expected_cursor
    {
        return Err(changed());
    }
    let now = tx.now_ts();
    let delta = db::advance(c, &mut job, params.limit, &params.request_id, &now)?;
    let artifact_changed =
        db::number(&delta.artifacts_added)? > 0 || db::number(&delta.edges_added)? > 0;
    let receipt = Receipt {
        scope: params.scope,
        crew_id: params.crew_id,
        request_id: params.request_id,
        recorded_at: now,
        operation,
        job,
        effect: Effect::Advanced {
            before_revision: params.expected_revision,
            before_cursor: params.expected_cursor,
            delta,
        },
    };
    db::record(tx.conn(), &principal, &digest, &receipt)?;
    tx.note(crate::Change::ArtifactHistoryChanged {
        crew_id: receipt.crew_id.clone(),
        job_id: receipt.job.id.clone(),
    });
    if artifact_changed {
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
    let principal = authorize(c, actor, crew)?;
    let current = objects::scope(c)?;
    if current.db_id != origin.db_id {
        return Err(forbidden());
    }
    let receipt = match db::receipt(c, crew, &principal, request)? {
        Some(db::Saved::Active { receipt, .. }) => Some(receipt),
        Some(db::Saved::Retired) | None => None,
    };
    if current.restore_epoch != origin.restore_epoch {
        return Ok(Lookup::Historical {
            receipt: receipt.filter(|r| r.scope == *origin),
        });
    }
    Ok(match receipt {
        Some(receipt) if receipt.scope == *origin => Lookup::Known { receipt },
        Some(_) => Lookup::Historical { receipt: None },
        None => Lookup::NotFound,
    })
}
