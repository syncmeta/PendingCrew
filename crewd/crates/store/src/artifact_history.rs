//! Explicit current-scope indexing of real retained sources, never a work replay.
use crate::{
    Result, StoreError, managed_artifacts as artifacts,
    object_refs::{self, ObjectKind},
};
use crewd_proto::{
    artifact_history::*,
    managed_artifacts::{Origin, Subject},
    managed_objects::Scope,
};
use crewd_work::{BookState, Config, Event, EventKind, Status as WorkStatus, WorkBook, WorkId};
use rusqlite::{Connection, OptionalExtension, params};

pub const DDL: &str = r#"
CREATE TABLE artifact_history_jobs (
 ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
 id TEXT NOT NULL UNIQUE, crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 db_id TEXT NOT NULL, restore_epoch TEXT NOT NULL, active INTEGER NOT NULL CHECK(active IN (0,1)),
 body TEXT NOT NULL CHECK(json_valid(body))
);
CREATE UNIQUE INDEX artifact_history_active ON artifact_history_jobs(crew_id,db_id,restore_epoch) WHERE active=1;
CREATE INDEX artifact_history_latest ON artifact_history_jobs(crew_id,db_id,restore_epoch,ordinal);
CREATE TABLE artifact_history_receipts (
 crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
 principal TEXT NOT NULL, request_id TEXT NOT NULL, job_id TEXT NOT NULL,
 digest BLOB NOT NULL CHECK(length(digest)=32), body TEXT CHECK(body IS NULL OR json_valid(body)),
 retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
 PRIMARY KEY(crew_id,principal,request_id),
 CHECK((retired=0 AND body IS NOT NULL) OR (retired=1 AND body IS NULL))
) WITHOUT ROWID;
CREATE INDEX artifact_history_receipt_job ON artifact_history_receipts(job_id);
CREATE TRIGGER artifact_history_retire BEFORE DELETE ON artifact_history_jobs BEGIN
 UPDATE artifact_history_receipts SET body=NULL,retired=1 WHERE job_id=OLD.id;
END;
"#;
fn bad() -> StoreError {
    StoreError::Invalid("invalid retained artifact history authority, cursor or receipt".into())
}
fn tx(c: &Connection) -> Result<()> {
    crate::managed_objects::transaction(c)
}
pub fn number(v: &str) -> Result<u64> {
    v.parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == v)
        .ok_or_else(bad)
}
fn sql(v: &str) -> Result<i64> {
    i64::try_from(number(v)?).map_err(|_| bad())
}
fn hash(v: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, v)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn zero() -> Counts {
    Counts {
        scanned: "0".into(),
        done_events: "0".into(),
        legacy_snapshots: "0".into(),
        lessons: "0".into(),
        missing_targets: "0".into(),
        artifacts_added: "0".into(),
        edges_added: "0".into(),
    }
}
fn add(v: &mut String, n: u64) -> Result<()> {
    *v = number(v)?.checked_add(n).ok_or_else(bad)?.to_string();
    Ok(())
}
pub fn add_counts(total: &mut Counts, delta: &Counts) -> Result<()> {
    add(&mut total.scanned, number(&delta.scanned)?)?;
    add(&mut total.done_events, number(&delta.done_events)?)?;
    add(
        &mut total.legacy_snapshots,
        number(&delta.legacy_snapshots)?,
    )?;
    add(&mut total.lessons, number(&delta.lessons)?)?;
    add(&mut total.missing_targets, number(&delta.missing_targets)?)?;
    add(&mut total.artifacts_added, number(&delta.artifacts_added)?)?;
    add(&mut total.edges_added, number(&delta.edges_added)?)?;
    Ok(())
}
fn valid_counts(v: &Counts) -> Result<()> {
    for value in [
        &v.scanned,
        &v.done_events,
        &v.legacy_snapshots,
        &v.lessons,
        &v.missing_targets,
        &v.artifacts_added,
        &v.edges_added,
    ] {
        number(value)?;
    }
    let described = number(&v.done_events)?
        .checked_add(number(&v.legacy_snapshots)?)
        .and_then(|n| n.checked_add(number(&v.lessons).ok()?))
        .ok_or_else(bad)?;
    if described > number(&v.scanned)? || number(&v.missing_targets)? > described {
        return Err(bad());
    }
    Ok(())
}
fn rank(phase: Phase) -> u8 {
    match phase {
        Phase::LegacyEvents => 0,
        Phase::LegacySnapshots => 1,
        Phase::Events => 2,
        Phase::Lessons => 3,
        Phase::Complete => 4,
    }
}
fn cursor_valid(cursor: &Cursor, cut: &Cut) -> Result<()> {
    let bound = match cursor.phase {
        Phase::LegacyEvents => &cut.legacy_event_seq,
        Phase::LegacySnapshots => &cut.legacy_item,
        Phase::Events => &cut.event_seq,
        Phase::Lessons => &cut.lesson_number,
        Phase::Complete => "0",
    };
    if number(&cursor.after)? > number(bound)? {
        return Err(bad());
    }
    Ok(())
}
fn valid_scope(v: &Scope) -> bool {
    !v.db_id.is_empty()
        && v.db_id.len() <= 256
        && !v.db_id.chars().any(char::is_control)
        && crewd_proto::valid_remote_uuid(&v.restore_epoch)
}
pub fn valid_job(job: &Job) -> Result<()> {
    if !crewd_proto::valid_remote_uuid(&job.id)
        || !valid_scope(&job.scope)
        || job.crew_id.is_empty()
        || number(&job.revision)? == 0
        || job.created_at.is_empty()
        || job.updated_at.is_empty()
        || job
            .last_request_id
            .as_deref()
            .is_some_and(|v| !crewd_proto::valid_remote_uuid(v))
    {
        return Err(bad());
    }
    for value in [
        &job.cut.legacy_event_seq,
        &job.cut.legacy_item,
        &job.cut.legacy_snapshot_seq,
        &job.cut.legacy_retained_events,
        &job.cut.event_seq,
        &job.cut.lesson_number,
        &job.cursor.after,
    ] {
        number(value)?;
    }
    if let Some(digest) = &job.cut.legacy_snapshot_digest
        && (digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err(bad());
    }
    if number(&job.cut.legacy_retained_events)? > number(&job.cut.legacy_event_seq)?
        || (job.cut.legacy_snapshot_digest.is_none()
            && (job.cut.legacy_item != "0" || job.cut.legacy_snapshot_seq != "0"))
    {
        return Err(bad());
    }
    cursor_valid(&job.cursor, &job.cut)?;
    valid_counts(&job.counts)
}
fn snapshot(c: &Connection, crew: &str) -> Result<Option<(String, BookState)>> {
    let raw: Option<String> = c
        .query_row(
            "SELECT state FROM legacy_work_books WHERE crew_id=?1",
            [crew],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let book: BookState = serde_json::from_str(&raw)?;
        if book.crew.as_str() != crew || book.organization.is_some() {
            return Err(bad());
        }
        WorkBook::restore(book.clone(), Config::default()).map_err(|_| bad())?;
        Ok((raw, book))
    })
    .transpose()
}
pub fn load(c: &Connection, crew: &str, scope: &Scope, id: Option<&str>) -> Result<Option<Job>> {
    if id.is_some_and(|v| !crewd_proto::valid_remote_uuid(v)) {
        return Err(bad());
    }
    let decode = |r: &rusqlite::Row<'_>| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, bool>(5)?,
            r.get::<_, String>(6)?,
        ))
    };
    let row=match id {
        Some(id)=>c.query_row("SELECT ordinal,id,crew_id,db_id,restore_epoch,active,body FROM artifact_history_jobs WHERE crew_id=?1 AND id=?2",params![crew,id],decode).optional()?,
        None=>c.query_row("SELECT ordinal,id,crew_id,db_id,restore_epoch,active,body FROM artifact_history_jobs WHERE crew_id=?1 AND db_id=?2 AND restore_epoch=?3 ORDER BY ordinal DESC LIMIT 1",params![crew,scope.db_id,scope.restore_epoch],decode).optional()?,
    };
    row.map(
        |(ordinal, stored_id, stored_crew, db_id, epoch, active, raw)| {
            let job: Job = serde_json::from_str(&raw)?;
            valid_job(&job)?;
            if ordinal <= 0
                || stored_id != job.id
                || stored_crew != job.crew_id
                || db_id != job.scope.db_id
                || epoch != job.scope.restore_epoch
                || active != (job.cursor.phase != Phase::Complete)
                || job.crew_id != crew
                || id.is_some_and(|id| id != job.id)
                || (id.is_none() && job.scope != *scope)
            {
                return Err(bad());
            }
            Ok(job)
        },
    )
    .transpose()
}

pub fn begin(
    c: &Connection,
    scope: &Scope,
    crew: &str,
    request: &str,
    now: &str,
) -> Result<(Job, bool)> {
    tx(c)?;
    if let Some(job) = load(c, crew, scope, None)?
        && job.cursor.phase != Phase::Complete
    {
        return Ok((job, false));
    }
    let legacy_max: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM work_events WHERE crew_id=?1",
        [crew],
        |r| r.get(0),
    )?;
    let retained: i64 = c.query_row(
        "SELECT count(*) FROM work_events WHERE crew_id=?1 AND seq<=?2",
        params![crew, legacy_max],
        |r| r.get(0),
    )?;
    let event_max: i64 = c.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM organization_work_events WHERE crew_id=?1",
        [crew],
        |r| r.get(0),
    )?;
    let lesson_max: i64 = c.query_row(
        "SELECT COALESCE(MAX(number),0) FROM managed_lessons WHERE crew_id=?1",
        [crew],
        |r| r.get(0),
    )?;
    if legacy_max < 0 || retained < 0 || event_max < 0 || lesson_max < 0 {
        return Err(bad());
    }
    let old = snapshot(c, crew)?;
    let cut = Cut {
        legacy_event_seq: legacy_max.to_string(),
        legacy_item: old.as_ref().map_or(0, |(_, v)| v.last_item).to_string(),
        legacy_snapshot_seq: old.as_ref().map_or(0, |(_, v)| v.last_seq).to_string(),
        legacy_snapshot_digest: old.as_ref().map(|(raw, _)| hash(raw.as_bytes())),
        legacy_retained_events: retained.to_string(),
        event_seq: event_max.to_string(),
        lesson_number: lesson_max.to_string(),
    };
    let job = Job {
        id: uuid::Uuid::new_v4().to_string(),
        scope: scope.clone(),
        crew_id: crew.into(),
        cut,
        cursor: Cursor {
            phase: Phase::LegacyEvents,
            after: "0".into(),
        },
        revision: "1".into(),
        counts: zero(),
        created_at: now.into(),
        updated_at: now.into(),
        last_request_id: Some(request.into()),
    };
    valid_job(&job)?;
    c.execute("INSERT INTO artifact_history_jobs(id,crew_id,db_id,restore_epoch,active,body) VALUES(?1,?2,?3,?4,1,?5)",params![job.id,crew,scope.db_id,scope.restore_epoch,serde_json::to_string(&job)?])?;
    Ok((job, true))
}
fn next(job: &mut Job) {
    job.cursor = Cursor {
        phase: match job.cursor.phase {
            Phase::LegacyEvents => Phase::LegacySnapshots,
            Phase::LegacySnapshots => Phase::Events,
            Phase::Events => Phase::Lessons,
            Phase::Lessons | Phase::Complete => Phase::Complete,
        },
        after: "0".into(),
    };
}
fn captured(delta: &mut Counts, added: (u64, u64)) -> Result<()> {
    add(&mut delta.artifacts_added, added.0)?;
    add(&mut delta.edges_added, added.1)
}
fn legacy_target(c: &Connection, crew: &str, id: WorkId) -> Result<Option<Subject>> {
    Ok(
        object_refs::resolve(c, ObjectKind::Plan, crew, &format!("work:{id}"))?
            .map(|reference| Subject::Work { id: reference.id() }),
    )
}
/// A step consumes at most `limit` source records, not seq gaps or pointer count.
pub fn advance(
    c: &Connection,
    job: &mut Job,
    limit: u16,
    request: &str,
    now: &str,
) -> Result<Counts> {
    tx(c)?;
    valid_job(job)?;
    if !(1..=64).contains(&limit) || job.cursor.phase == Phase::Complete {
        return Err(bad());
    }
    let before = job.clone();
    let mut delta = zero();
    for _ in 0..5 {
        let remaining = u64::from(limit)
            .checked_sub(number(&delta.scanned)?)
            .ok_or_else(bad)?;
        if remaining == 0 || job.cursor.phase == Phase::Complete {
            break;
        }
        let after = number(&job.cursor.after)?;
        match job.cursor.phase {
            Phase::LegacyEvents | Phase::Events => {
                let legacy = job.cursor.phase == Phase::LegacyEvents;
                let cut = if legacy {
                    sql(&job.cut.legacy_event_seq)?
                } else {
                    sql(&job.cut.event_seq)?
                };
                let table = if legacy {
                    "work_events"
                } else {
                    "organization_work_events"
                };
                let mut q=c.prepare(&format!("SELECT seq,body FROM {table} WHERE crew_id=?1 AND seq>?2 AND seq<=?3 ORDER BY seq LIMIT ?4"))?;
                let rows = q
                    .query_map(
                        params![
                            job.crew_id,
                            i64::try_from(after).map_err(|_| bad())?,
                            cut,
                            remaining as i64
                        ],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (seq, raw) in &rows {
                    let event: Event = serde_json::from_str(raw)?;
                    if *seq <= 0 || event.seq != *seq as u64 {
                        return Err(bad());
                    }
                    add(&mut delta.scanned, 1)?;
                    job.cursor.after = seq.to_string();
                    if let EventKind::Done { id, evidence, .. } = &event.kind {
                        add(&mut delta.done_events, 1)?;
                        let target = if legacy {
                            legacy_target(c, &job.crew_id, *id)?
                        } else {
                            crate::work_organization::reference_for_core(
                                c,
                                ObjectKind::Plan,
                                &id.to_string(),
                            )?
                            .map(|r| Subject::Work { id: r.id() })
                        };
                        if let Some(target) = target {
                            if legacy {
                                if let Some(proof) = evidence {
                                    captured(
                                        &mut delta,
                                        artifacts::capture_evidence(
                                            c,
                                            &job.crew_id,
                                            &target,
                                            proof,
                                            &Origin::LegacyWorkDone {
                                                crew_id: job.crew_id.clone(),
                                                local_seq: seq.to_string(),
                                                legacy_work_id: id.to_string(),
                                            },
                                            now,
                                        )?,
                                    )?;
                                }
                            } else {
                                captured(
                                    &mut delta,
                                    artifacts::capture_done_counts(c, &job.crew_id, &event, now)?,
                                )?;
                            }
                        } else {
                            add(&mut delta.missing_targets, 1)?;
                        }
                    }
                }
                if rows.len() < remaining as usize || number(&job.cursor.after)? >= cut as u64 {
                    next(job);
                } else {
                    break;
                }
            }
            Phase::LegacySnapshots => {
                let old = snapshot(c, &job.crew_id)?;
                if old.as_ref().map(|(raw, _)| hash(raw.as_bytes()))
                    != job.cut.legacy_snapshot_digest
                {
                    return Err(bad());
                }
                let Some((_, book)) = old else {
                    next(job);
                    continue;
                };
                if book.last_seq.to_string() != job.cut.legacy_snapshot_seq
                    || book.last_item.to_string() != job.cut.legacy_item
                {
                    return Err(bad());
                }
                let rows = book
                    .items
                    .iter()
                    .filter(|(id, _)| {
                        u64::from(id.0) > after
                            && u64::from(id.0) <= number(&job.cut.legacy_item).unwrap_or(0)
                    })
                    .take(remaining as usize)
                    .collect::<Vec<_>>();
                for (id, item) in &rows {
                    if item.id != **id {
                        return Err(bad());
                    }
                    add(&mut delta.scanned, 1)?;
                    job.cursor.after = id.0.to_string();
                    if item.status == WorkStatus::Done {
                        add(&mut delta.legacy_snapshots, 1)?;
                        if let Some(target) = legacy_target(c, &job.crew_id, **id)? {
                            if let Some(proof) = &item.evidence {
                                captured(
                                    &mut delta,
                                    artifacts::capture_evidence(
                                        c,
                                        &job.crew_id,
                                        &target,
                                        proof,
                                        &Origin::LegacyWorkSnapshot {
                                            crew_id: job.crew_id.clone(),
                                            legacy_work_id: id.to_string(),
                                            snapshot_seq: job.cut.legacy_snapshot_seq.clone(),
                                        },
                                        now,
                                    )?,
                                )?;
                            }
                        } else {
                            add(&mut delta.missing_targets, 1)?;
                        }
                    }
                }
                if rows.len() < remaining as usize
                    || number(&job.cursor.after)? >= number(&job.cut.legacy_item)?
                {
                    next(job);
                } else {
                    break;
                }
            }
            Phase::Lessons => {
                let cut = sql(&job.cut.lesson_number)?;
                let mut q=c.prepare("SELECT number FROM managed_lessons WHERE crew_id=?1 AND number>?2 AND number<=?3 ORDER BY number LIMIT ?4")?;
                let rows = q
                    .query_map(
                        params![
                            job.crew_id,
                            i64::try_from(after).map_err(|_| bad())?,
                            cut,
                            remaining as i64
                        ],
                        |r| r.get::<_, i64>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for n in &rows {
                    let lesson = crate::managed_risks_lessons::lesson(c, *n)?.ok_or_else(bad)?;
                    if lesson.crew_id != job.crew_id {
                        return Err(bad());
                    }
                    add(&mut delta.scanned, 1)?;
                    add(&mut delta.lessons, 1)?;
                    job.cursor.after = n.to_string();
                    captured(
                        &mut delta,
                        artifacts::capture_lesson_counts(c, &lesson, now)?,
                    )?;
                }
                if rows.len() < remaining as usize || number(&job.cursor.after)? >= cut as u64 {
                    next(job);
                } else {
                    break;
                }
            }
            Phase::Complete => break,
        }
    }
    add_counts(&mut job.counts, &delta)?;
    job.revision = number(&before.revision)?
        .checked_add(1)
        .ok_or_else(bad)?
        .to_string();
    job.updated_at = std::cmp::max(now, &before.updated_at).to_string();
    job.last_request_id = Some(request.into());
    valid_job(job)?;
    if c.execute(
        "UPDATE artifact_history_jobs SET active=?1,body=?2 WHERE id=?3 AND crew_id=?4 AND body=?5",
        params![
            job.cursor.phase != Phase::Complete,
            serde_json::to_string(job)?,
            job.id,
            job.crew_id,
            serde_json::to_string(&before)?
        ],
    )? != 1
    {
        return Err(bad());
    }
    Ok(delta)
}
pub enum Saved {
    Active { digest: [u8; 32], receipt: Receipt },
    Retired,
}
pub fn request_digest(
    scope: &Scope,
    crew: &str,
    request: &str,
    operation: &Operation,
) -> Result<[u8; 32]> {
    let raw = serde_json::to_vec(&(scope, crew, request, operation))?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &raw)
        .as_ref()
        .try_into()
        .map_err(|_| bad())?)
}
fn valid_receipt(receipt: &Receipt, digest: &[u8; 32]) -> Result<()> {
    valid_job(&receipt.job)?;
    if !valid_scope(&receipt.scope)
        || !crewd_proto::valid_remote_uuid(&receipt.request_id)
        || receipt.recorded_at.is_empty()
        || receipt.crew_id != receipt.job.crew_id
        || receipt.scope != receipt.job.scope
        || request_digest(
            &receipt.scope,
            &receipt.crew_id,
            &receipt.request_id,
            &receipt.operation,
        )? != *digest
    {
        return Err(bad());
    }
    match (&receipt.operation, &receipt.effect) {
        (Operation::Begin, Effect::Begun { created }) => {
            if !*created && receipt.job.cursor.phase == Phase::Complete {
                return Err(bad());
            }
            if *created
                && (receipt.job.revision != "1"
                    || receipt.job.cursor
                        != Cursor {
                            phase: Phase::LegacyEvents,
                            after: "0".into(),
                        }
                    || receipt.job.counts != zero()
                    || receipt.job.last_request_id.as_deref() != Some(&receipt.request_id))
            {
                return Err(bad());
            }
        }
        (
            Operation::Step {
                job_id,
                expected_revision,
                expected_cursor,
                limit,
            },
            Effect::Advanced {
                before_revision,
                before_cursor,
                delta,
            },
        ) => {
            if job_id != &receipt.job.id
                || expected_revision != before_revision
                || expected_cursor != before_cursor
                || !(1..=64).contains(limit)
                || number(&receipt.job.revision)?
                    != number(before_revision)?.checked_add(1).ok_or_else(bad)?
                || number(&delta.scanned)? > u64::from(*limit)
                || receipt.job.last_request_id.as_deref() != Some(&receipt.request_id)
            {
                return Err(bad());
            }
            valid_counts(delta)?;
            cursor_valid(before_cursor, &receipt.job.cut)?;
            if number(before_revision)? == 0
                || before_cursor.phase == Phase::Complete
                || rank(receipt.job.cursor.phase) < rank(before_cursor.phase)
                || (receipt.job.cursor.phase == before_cursor.phase
                    && number(&receipt.job.cursor.after)? <= number(&before_cursor.after)?)
            {
                return Err(bad());
            }
            for (part, total) in [
                (&delta.scanned, &receipt.job.counts.scanned),
                (&delta.done_events, &receipt.job.counts.done_events),
                (
                    &delta.legacy_snapshots,
                    &receipt.job.counts.legacy_snapshots,
                ),
                (&delta.lessons, &receipt.job.counts.lessons),
                (&delta.missing_targets, &receipt.job.counts.missing_targets),
                (&delta.artifacts_added, &receipt.job.counts.artifacts_added),
                (&delta.edges_added, &receipt.job.counts.edges_added),
            ] {
                if number(part)? > number(total)? {
                    return Err(bad());
                }
            }
        }
        _ => return Err(bad()),
    }
    Ok(())
}
pub fn receipt(
    c: &Connection,
    crew: &str,
    principal: &str,
    request: &str,
) -> Result<Option<Saved>> {
    let row=c.query_row("SELECT digest,body,retired,job_id FROM artifact_history_receipts WHERE crew_id=?1 AND principal=?2 AND request_id=?3",params![crew,principal,request],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,bool>(2)?,r.get::<_,String>(3)?))).optional()?;
    row.map(|(digest, body, retired, job_id)| {
        if retired {
            return Ok(Saved::Retired);
        }
        let digest = digest.try_into().map_err(|_| bad())?;
        let receipt: Receipt = serde_json::from_str(&body.ok_or_else(bad)?)?;
        if receipt.crew_id != crew || receipt.request_id != request || receipt.job.id != job_id {
            return Err(bad());
        }
        valid_receipt(&receipt, &digest)?;
        Ok(Saved::Active { digest, receipt })
    })
    .transpose()
}
pub fn record(c: &Connection, principal: &str, digest: &[u8; 32], receipt: &Receipt) -> Result<()> {
    tx(c)?;
    valid_receipt(receipt, digest)?;
    c.execute("INSERT INTO artifact_history_receipts(crew_id,principal,request_id,job_id,digest,body) VALUES(?1,?2,?3,?4,?5,?6)",params![receipt.crew_id,principal,receipt.request_id,receipt.job.id,digest.as_slice(),serde_json::to_string(receipt)?])?;
    Ok(())
}
